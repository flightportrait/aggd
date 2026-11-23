//! The query API readsb calls re-api: one base query (all, circle, box,
//! closest, find_*) narrowed by filters, answered from memory.

use crate::json::{self, View};
use crate::track::{bearing, dist_m, Aircraft, Ms};
use crate::Ctx;

#[derive(Default)]
struct Query {
    /// an aircraft is returned when any of these selects it
    bases: Vec<Base>,
    squawk: Option<u16>,
    callsign_exact: Option<String>,
    callsign_prefix: Option<String>,
    types: Vec<String>,
    db_flags: u32,
    with_pos: bool,
    above: Option<i32>,
    below: Option<i32>,
    jv2: bool,
    bin: bool,
    zstd: bool,
}

enum Base {
    All,
    Circle(f64, f64, f64, bool),
    Box(f64, f64, f64, f64),
    Hex(Vec<u32>),
    Callsign(Vec<String>),
    Reg(Vec<String>),
    Type(Vec<String>),
}

fn nums(v: &str, n: usize) -> Option<Vec<f64>> {
    let f: Vec<f64> = v.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>().ok()?;
    (f.len() == n).then_some(f)
}

fn list(v: &str) -> Vec<String> {
    v.split(',').map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty()).collect()
}

fn parse(q: &str) -> Result<Query, String> {
    let mut out = Query::default();
    for part in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let v = v.replace("%2C", ",").replace("%2c", ",").replace("%20", " ").replace('+', " ");
        let bad = || format!("invalid value for {k}");
        let base = |b: Base, out: &mut Query| -> Result<(), String> {
            out.bases.push(b);
            Ok(())
        };
        match k {
            "all" => base(Base::All, &mut out)?,
            "all_with_pos" => {
                base(Base::All, &mut out)?;
                out.with_pos = true;
            }
            "circle" | "closest" => {
                let f = nums(&v, 3).ok_or_else(bad)?;
                base(Base::Circle(f[0], f[1], f[2], k == "closest"), &mut out)?;
            }
            "box" => {
                let f = nums(&v, 4).ok_or_else(bad)?;
                base(Base::Box(f[0], f[1], f[2], f[3]), &mut out)?;
            }
            "find_hex" | "hexlist" => {
                let mut hexes = Vec::new();
                for h in v.split(',').filter(|h| !h.is_empty()) {
                    let (h, flag) = match h.strip_prefix('~') {
                        Some(r) => (r, modes::NON_ICAO),
                        None => (h, 0),
                    };
                    hexes.push(u32::from_str_radix(h.trim(), 16).map_err(|_| bad())? | flag);
                }
                base(Base::Hex(hexes), &mut out)?;
            }
            "find_callsign" => base(Base::Callsign(list(&v)), &mut out)?,
            "find_reg" => base(Base::Reg(list(&v)), &mut out)?,
            "find_type" => base(Base::Type(list(&v)), &mut out)?,
            "filter_squawk" => out.squawk = Some(u16::from_str_radix(&v, 16).map_err(|_| bad())?),
            "filter_callsign_exact" => out.callsign_exact = Some(v.to_ascii_uppercase()),
            "filter_callsign_prefix" => out.callsign_prefix = Some(v.to_ascii_uppercase()),
            "filter_type" => out.types = list(&v),
            "filter_mil" => out.db_flags |= 1,
            "filter_pia" => out.db_flags |= 4,
            "filter_ladd" => out.db_flags |= 8,
            "filter_dbFlag" => out.db_flags |= v.parse::<u32>().map_err(|_| bad())?,
            "filter_with_pos" => out.with_pos = true,
            "above_alt_baro" => out.above = Some(v.parse().map_err(|_| bad())?),
            "below_alt_baro" => out.below = Some(v.parse().map_err(|_| bad())?),
            "jv2" => out.jv2 = true,
            "binCraft" => out.bin = true,
            "zstd" => out.zstd = true,
            _ => {}
        }
    }
    if out.bases.is_empty() {
        if out.squawk.is_some() || out.db_flags != 0 || out.callsign_exact.is_some() || out.callsign_prefix.is_some() || !out.types.is_empty() {
            out.bases.push(Base::All);
        } else {
            return Err("no query: use all, circle, closest, box, find_hex, find_callsign, find_reg or find_type".into());
        }
    }
    // closest answers with one aircraft; it does not combine
    if out.bases.len() > 1 && out.bases.iter().any(|b| matches!(b, Base::Circle(_, _, _, true))) {
        return Err("closest cannot be combined with another query".into());
    }
    Ok(out)
}

fn callsign(a: &Aircraft, now: Ms) -> Option<String> {
    a.callsign.get(now).map(|c| String::from_utf8_lossy(&c).trim_end().to_string())
}

fn filters(q: &Query, a: &Aircraft, now: Ms) -> bool {
    if q.with_pos && !a.pos.valid(now) {
        return false;
    }
    if let Some(s) = q.squawk {
        if a.squawk.get(now) != Some(s) {
            return false;
        }
    }
    if q.db_flags != 0 && a.db.as_ref().map_or(true, |d| d.flags & q.db_flags == 0) {
        return false;
    }
    if !q.types.is_empty() && a.db.as_ref().map_or(true, |d| !q.types.iter().any(|t| *t == d.typ)) {
        return false;
    }
    if q.callsign_exact.is_some() || q.callsign_prefix.is_some() {
        let Some(cs) = callsign(a, now) else { return false };
        if q.callsign_exact.as_ref().map_or(false, |e| *e != cs) {
            return false;
        }
        if q.callsign_prefix.as_ref().map_or(false, |p| !cs.starts_with(p.as_str())) {
            return false;
        }
    }
    if q.above.is_some() || q.below.is_some() {
        let alt = if a.on_ground(now) { Some(0) } else { a.alt_baro.get(now) };
        let Some(alt) = alt else { return false };
        if q.above.map_or(false, |x| alt <= x) || q.below.map_or(false, |x| alt >= x) {
            return false;
        }
    }
    true
}

/// Answer a query string. Err is a message for a 400 response.
pub fn query(ctx: &Ctx, qs: &str, now: Ms) -> Result<(Vec<u8>, &'static str), String> {
    let started = std::time::Instant::now();
    let q = parse(qs)?;
    let view: View = ctx.view(now);
    let window = ctx.cfg.list_window_ms;
    let mut items: Vec<(f64, Vec<u8>)> = Vec::new();
    let mut with_pos = 0u32;
    let mut emit = |a: &mut Aircraft, extra: Option<(f64, f64)>| {
        let dist = extra.map_or(0.0, |e| e.0);
        if q.bin {
            let mut rec = Vec::with_capacity(crate::bincraft::STRIDE);
            with_pos += a.pos.valid(now) as u32;
            crate::bincraft::record(a, now, &mut rec);
            items.push((dist, rec));
        } else {
            let mut s = String::with_capacity(700);
            json::aircraft(&mut s, a, &view, extra);
            items.push((dist, s.into_bytes()));
        }
    };
    let only_hex = q.bases.iter().all(|b| matches!(b, Base::Hex(_)));
    if only_hex {
        let mut seen: Vec<u32> = Vec::new();
        for b in &q.bases {
            let Base::Hex(hexes) = b else { continue };
            for h in hexes {
                if seen.contains(h) {
                    continue;
                }
                seen.push(*h);
                let mut s = ctx.store.shard(*h).lock();
                if let Some(a) = s.map.get_mut(h) {
                    if json::listed(a, now, window) && filters(&q, a, now) {
                        emit(a, None);
                    }
                }
            }
        }
    } else {
        for s in ctx.store.shards() {
            let mut s = s.lock();
            for a in s.map.values_mut() {
                if !json::listed(a, now, window) {
                    continue;
                }
                let mut extra = None;
                let mut ok = false;
                for base in &q.bases {
                    ok = match base {
                        Base::All => true,
                        Base::Circle(lat, lon, r, _) => {
                            a.pos.valid(now) && {
                                let (alat, alon) = a.pos.v;
                                // cheap latitude test before the trigonometry
                                (alat - lat).abs() * 60.0 <= *r && {
                                    let d = dist_m(*lat, *lon, alat, alon) / 1852.0;
                                    extra = Some((d, bearing(*lat, *lon, alat, alon)));
                                    d <= *r
                                }
                            }
                        }
                        Base::Box(s, n, w, e) => {
                            a.pos.valid(now) && {
                                let (alat, alon) = a.pos.v;
                                alat >= *s && alat <= *n && if w <= e { alon >= *w && alon <= *e } else { alon >= *w || alon <= *e }
                            }
                        }
                        Base::Hex(l) => l.contains(&a.addr),
                        Base::Callsign(l) => callsign(a, now).map_or(false, |c| l.iter().any(|x| *x == c)),
                        Base::Reg(l) => a.db.as_ref().map_or(false, |d| l.iter().any(|x| *x == d.reg)),
                        Base::Type(l) => a.db.as_ref().map_or(false, |d| l.iter().any(|x| *x == d.typ)),
                    };
                    if ok {
                        break;
                    }
                }
                if ok && filters(&q, a, now) {
                    emit(a, extra);
                }
            }
        }
    }
    if let [Base::Circle(_, _, _, closest)] = q.bases[..] {
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        if closest {
            items.truncate(1);
        }
    }
    if q.bin {
        let mut o = Vec::with_capacity(items.len() * crate::bincraft::STRIDE + crate::bincraft::STRIDE);
        crate::bincraft::header(ctx, now, crate::bincraft::WHOLE, with_pos, ctx.store.messages(), &mut o);
        for it in &items {
            o.extend_from_slice(&it.1);
        }
        return Ok(if q.zstd {
            (zstd::bulk::compress(&o, 1).unwrap_or_default(), "application/zstd")
        } else {
            (o, "application/octet-stream")
        });
    }
    let mut o: Vec<u8> = Vec::with_capacity(items.iter().map(|i| i.1.len() + 2).sum::<usize>() + 128);
    let ptime = started.elapsed().as_secs_f64() * 1000.0;
    use std::io::Write as _;
    if q.jv2 {
        o.extend_from_slice(b"{\"ac\":[");
        for (i, it) in items.iter().enumerate() {
            if i > 0 {
                o.push(b',');
            }
            o.extend_from_slice(&it.1);
        }
        let _ = write!(o, "],\"msg\":\"No error\",\"now\":{now},\"total\":{},\"ctime\":{now},\"ptime\":{:.0}}}\n", items.len(), ptime);
    } else {
        let _ = write!(o, "{{\"now\": {:.3}\n,\"aircraft\":[", now as f64 / 1000.0);
        for (i, it) in items.iter().enumerate() {
            o.extend_from_slice(if i > 0 { b",\n" } else { b"\n" });
            o.extend_from_slice(&it.1);
        }
        let _ = write!(o, "\n]\n,\"resultCount\": {}\n,\"ptime\": {:.3}\n}}\n", items.len(), ptime);
    }
    Ok((o, "application/json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_parse() {
        // what tar1090 sends once an aircraft is selected
        let q = parse("binCraft&zstd&box=40.75,43.02,10.33,14.66&find_hex=302025").unwrap();
        assert!(q.bin && q.zstd && q.bases.len() == 2);
        assert!(matches!(&q.bases[1], Base::Hex(h) if h == &vec![0x302025]));
        assert!(parse("circle=41.9,12.5,100&filter_squawk=7700").unwrap().squawk == Some(0x7700));
        assert!(parse("filter_mil").unwrap().bases.len() == 1);
        assert!(parse("").is_err());
        assert!(parse("circle=41.9,12.5").is_err());
        assert!(parse("closest=41.9,12.5,100&find_hex=302025").is_err());
        assert!(matches!(&parse("find_hex=~abc123,4ca2d6").unwrap().bases[0], Base::Hex(h) if h == &vec![0xabc123 | modes::NON_ICAO, 0x4ca2d6]));
    }
}
