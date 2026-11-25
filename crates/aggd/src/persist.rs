//! What outlives the process: per-day trace files in tar1090's
//! globe_history layout, and a state file so traces survive a restart.

use crate::trace::{self, Packed, Point};
use crate::track::Ms;
use crate::Ctx;
use std::io::{Read, Write};

/// UTC calendar date of a timestamp.
pub fn civil(ms: Ms) -> (i64, i64, i64) {
    let z = (ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

const DAY: Ms = 86_400_000;

/// Write the day files of every aircraft in shards `from..to` whose
/// trace has grown since its last write. Rendering happens under the
/// shard lock, compression and file I/O after it is released.
pub fn write_history(ctx: &Ctx, dir: &str, from: usize, to: usize) -> usize {
    let mut files = 0;
    for s in &ctx.store.shards()[from..to] {
        let mut jobs: Vec<(String, String, String)> = Vec::new();
        {
            let mut s = s.lock();
            let s = &mut *s;
            let live = s.map.values_mut().map(|a| (a.addr, a.db.clone(), &mut a.trace));
            let gone = s.gone.iter_mut().map(|(addr, g)| (*addr, None, &mut g.trace));
            for (addr, db, trace) in live.chain(gone) {
                let n = trace.len();
                if trace.written >= n {
                    continue;
                }
                let db = db.or_else(|| ctx.db.as_ref().and_then(|d| d.describe(addr)));
                let (points, details) = trace.all();
                // every day that has an unwritten point gets its file rewritten
                let first_day = points[trace.written].t() / DAY;
                let last_day = points[n - 1].t() / DAY;
                for day in first_day..=last_day {
                    let lo = points.partition_point(|p| p.t() / DAY < day);
                    let hi = points.partition_point(|p| p.t() / DAY <= day);
                    if lo == hi {
                        continue;
                    }
                    let (y, m, d) = civil(day * DAY);
                    let hex = crate::json::hex(addr);
                    let mut body = String::with_capacity((hi - lo) * 70 + 256);
                    trace::render(addr, db.as_deref(), &points[lo..hi], &details, &mut body);
                    jobs.push((
                        format!("{dir}/{y:04}/{m:02}/{d:02}/traces/{}", &hex[hex.len() - 2..]),
                        format!("trace_full_{hex}.json"),
                        body,
                    ));
                }
                drop((points, details));
                trace.written = n;
            }
        }
        for (path, name, body) in jobs {
            if std::fs::create_dir_all(&path).is_err() {
                continue;
            }
            let mut e = flate2::write::GzEncoder::new(Vec::with_capacity(body.len() / 5), flate2::Compression::new(4));
            let _ = e.write_all(body.as_bytes());
            let Ok(gz) = e.finish() else { continue };
            let tmp = format!("{path}/.{name}.tmp");
            if std::fs::write(&tmp, gz).is_ok() && std::fs::rename(&tmp, format!("{path}/{name}")).is_ok() {
                files += 1;
            }
        }
    }
    files
}

const MAGIC: &[u8; 8] = b"aggdst01";
const POINT: usize = 36;

fn put_point(o: &mut Vec<u8>, p: &Point) {
    o.extend_from_slice(&p.t.to_le_bytes());
    o.extend_from_slice(&p.lat.to_le_bytes());
    o.extend_from_slice(&p.lon.to_le_bytes());
    o.extend_from_slice(&p.alt.to_le_bytes());
    o.extend_from_slice(&p.alt_geom.to_le_bytes());
    o.extend_from_slice(&p.gs.to_le_bytes());
    o.extend_from_slice(&p.track.to_le_bytes());
    o.extend_from_slice(&p.rate.to_le_bytes());
    o.extend_from_slice(&p.geom_rate.to_le_bytes());
    o.extend_from_slice(&p.ias.to_le_bytes());
    o.push(p.flags);
    o.push(p.addr_type);
}

fn get_point(b: &[u8]) -> Point {
    let u16_ = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let i32_ = |i: usize| i32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    Point {
        t: u64::from_le_bytes(b[0..8].try_into().unwrap()),
        lat: i32_(8),
        lon: i32_(12),
        alt: i32_(16),
        alt_geom: i32_(20),
        gs: u16_(24),
        track: u16_(26),
        rate: u16_(28) as i16,
        geom_rate: u16_(30) as i16,
        ias: u16_(32),
        flags: b[34],
        addr_type: b[35],
    }
}

/// Save every aircraft's trace. Returns the number of aircraft saved.
/// The file is written a shard at a time through the compressor: built
/// whole in memory it was 36 bytes a point, a transient that grew with
/// the table and that the allocator did not give back.
pub fn save_state(ctx: &Ctx, dir: &str) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let tmp = format!("{dir}/.state.zst.tmp");
    let mut z = zstd::stream::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&tmp)?), 3)?;
    z.write_all(MAGIC)?;
    let mut raw = Vec::with_capacity(1 << 16);
    let mut n = 0usize;
    for s in ctx.store.shards() {
        raw.clear();
        {
            let s = s.lock();
            let live = s.map.values().map(|a| (a.addr, &a.trace));
            for (addr, trace) in live.chain(s.gone.iter().map(|(addr, g)| (*addr, &g.trace))) {
                if trace.is_empty() {
                    continue;
                }
                let (points, _) = trace.all();
                n += 1;
                raw.extend_from_slice(&addr.to_le_bytes());
                raw.extend_from_slice(&(points.len() as u32).to_le_bytes());
                raw.extend_from_slice(&(trace.written.min(points.len()) as u32).to_le_bytes());
                for p in points.iter() {
                    put_point(&mut raw, &p.unpack());
                }
            }
        }
        // compression and the disk are kept outside the shard's lock
        z.write_all(&raw)?;
    }
    z.finish()?.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(&tmp, format!("{dir}/state.zst"))?;
    Ok(n)
}

/// Restore traces saved by `save_state`. Aircraft come back unlisted
/// (not heard since) but with their history, and are forgotten on the
/// usual schedule if they stay silent.
pub fn load_state(ctx: &Ctx, dir: &str, now: Ms) -> std::io::Result<usize> {
    let mut z = Vec::new();
    std::fs::File::open(format!("{dir}/state.zst"))?.read_to_end(&mut z)?;
    let raw = zstd::stream::decode_all(&z[..])?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "state file is damaged");
    if raw.len() < 8 || &raw[..8] != MAGIC {
        return Err(bad());
    }
    let mut i = 8;
    let mut n = 0;
    while i + 12 <= raw.len() {
        let addr = u32::from_le_bytes(raw[i..i + 4].try_into().unwrap());
        let count = u32::from_le_bytes(raw[i + 4..i + 8].try_into().unwrap()) as usize;
        let written = u32::from_le_bytes(raw[i + 8..i + 12].try_into().unwrap()) as usize;
        i += 12;
        let end = i.checked_add(count.checked_mul(POINT).ok_or_else(bad)?).ok_or_else(bad)?;
        if end > raw.len() {
            return Err(bad());
        }
        let saved = &raw[i..end];
        i = end;
        let Some(gone) = restore_one(saved, written, now, ctx.cfg.trace_keep_ms) else { continue };
        ctx.store.shard(addr).lock().gone.insert(addr, gone);
        n += 1;
    }
    Ok(n)
}

/// One aircraft's saved points, back as an aircraft forgotten while
/// running: its trace, and where it last was for its surface messages
/// to be read against. Without that place, an aircraft parked through
/// a restart and heard only by receivers with no location of their own
/// went unplaced until it moved (seen beside readsb, which went on
/// showing it). None when no point is recent enough to keep.
fn restore_one(saved: &[u8], written: usize, now: Ms, keep_ms: Ms) -> Option<crate::track::Gone> {
    let count = saved.len() / POINT;
    let points: Vec<Packed> = saved.chunks_exact(POINT).map(get_point).filter(|p| now.saturating_sub(p.t) <= keep_ms).map(|p| Packed::pack(&p)).collect();
    let seen = points.last()?.t();
    // points are saved oldest first, so the last saved is the last kept
    let p = get_point(&saved[saved.len() - POINT..]);
    let last_place = Some((p.lat as f64 / 1e6, p.lon as f64 / 1e6));
    let trace = trace::Trace::restored(&points, written.saturating_sub(count - points.len()).min(points.len()));
    Some(crate::track::Gone { seen, pos: Default::default(), nic: 0, rc: 0, last_place, heat_tag: 0, trace })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(t: Ms, lat: i32, lon: i32) -> Point {
        Point { t, lat, lon, alt: trace::GROUND, alt_geom: trace::NONE, gs: 0, track: 0, rate: 0, geom_rate: 0, ias: 0, flags: 0, addr_type: 0 }
    }

    #[test]
    fn a_restored_aircraft_keeps_where_it_last_was() {
        let now: Ms = 1_795_000_000_000;
        let mut saved = Vec::new();
        put_point(&mut saved, &point(now - 7_200_000, 40_640_000, -73_780_000));
        put_point(&mut saved, &point(now - 100_000, 40_689_500, -74_174_400));
        let g = restore_one(&saved, 2, now, 86_400_000).unwrap();
        assert_eq!(g.seen / 100, (now - 100_000) / 100);
        let (lat, lon) = g.last_place.unwrap();
        assert!((lat - 40.6895).abs() < 1e-6 && (lon + 74.1744).abs() < 1e-6);
        assert!(g.pos.src == modes::Source::Invalid && g.trace.len() == 2);
        // a trace older than the kept window brings nothing back
        assert!(restore_one(&saved, 2, now + 86_400_000, 86_400_000).is_none());
    }
}
