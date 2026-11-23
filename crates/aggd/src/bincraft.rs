//! binCraft: the fixed-width binary aircraft list tar1090 reads instead
//! of JSON, whole or cut into globe tiles. Records are little-endian,
//! 112 bytes, after a header of the same size.

use crate::tiles_data::SPECIAL;
use crate::track::{Aircraft, Ms};
use crate::{json, Ctx};
use modes::Source;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const STRIDE: usize = 112;
const VERSION: u32 = 20250403;
/// The index a file carries when it is the whole list, not a tile.
pub const WHOLE: u32 = 314159;
pub const GRID: i32 = 3;
const LAT_MULT: i32 = 360 / GRID + 1;
const MIN_INDEX: i32 = 1000;

/// Tile index for every whole-degree cell, special tiles first.
fn lookup() -> &'static Vec<u16> {
    static T: OnceLock<Vec<u16>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![0u16; 180 * 360];
        for lat in -90..90i32 {
            for lon in -180..180i32 {
                let special = SPECIAL.iter().position(|&(s, w, n, e)| {
                    let (s, w, n, e) = (s as i32, w as i32, n as i32, e as i32);
                    lat >= s && lat < n && if w <= e { lon >= w && lon < e } else { lon >= w || lon < e }
                });
                let idx = match special {
                    Some(i) => i as i32,
                    None => MIN_INDEX + ((lat + 90) / GRID) * LAT_MULT + (lon + 180) / GRID,
                };
                t[((lat + 90) * 360 + lon + 180) as usize] = idx as u16;
            }
        }
        t
    })
}

pub fn tile_of(lat: f64, lon: f64) -> u16 {
    let la = (lat.floor() as i32).clamp(-90, 89);
    let lo = (lon.floor() as i32 + 180).rem_euclid(360) - 180;
    lookup()[((la + 90) * 360 + lo + 180) as usize]
}

/// (south, west, north, east) of a tile.
pub fn bounds(index: u32) -> (i16, i16, i16, i16) {
    if let Some(&b) = SPECIAL.get(index as usize) {
        return b;
    }
    let i = index as i32 - MIN_INDEX;
    if i < 0 {
        return (-90, -180, 90, 180);
    }
    let (la, lo) = (i / LAT_MULT * GRID - 90, i % LAT_MULT * GRID - 180);
    (la as i16, lo as i16, (la + GRID) as i16, (lo + GRID) as i16)
}

pub fn special_tiles_json() -> String {
    let v: Vec<String> = SPECIAL
        .iter()
        .map(|(s, w, n, e)| format!("{{\"south\":{s},\"east\":{e},\"north\":{n},\"west\":{w}}}"))
        .collect();
    format!("[ {} ]", v.join(","))
}

struct Rec([u8; STRIDE]);
impl Rec {
    fn i32(&mut self, i: usize, v: i32) {
        self.0[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, i: usize, v: i32) {
        let v = v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        self.0[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn u16(&mut self, i: usize, v: u32) {
        self.0[i * 2..i * 2 + 2].copy_from_slice(&(v.min(65535) as u16).to_le_bytes());
    }
    fn bit(&mut self, byte: usize, bit: u8, on: bool) {
        if on {
            self.0[byte] |= 1 << bit;
        }
    }
}

/// One aircraft's record.
pub fn record(a: &mut Aircraft, now: Ms, out: &mut Vec<u8>) {
    let mut r = Rec([0; STRIDE]);
    r.i32(0, (a.addr & 0x1FF_FFFF) as i32);
    let pos = a.pos.valid(now);
    if pos {
        r.i32(1, (a.pos.age_s(now) * 10.0) as i32);
        r.i32(2, (a.pos.v.1 * 1e6).round() as i32);
        r.i32(3, (a.pos.v.0 * 1e6).round() as i32);
        r.0[65] = a.nic;
        r.u16(30, a.rc);
    }
    r.bit(73, 6, pos);
    macro_rules! f {
        ($field:ident, $byte:literal, $bit:literal, |$v:ident| $set:expr) => {
            if let Some($v) = a.$field.get(now) {
                $set;
                r.bit($byte, $bit, true);
            }
        };
    }
    f!(baro_rate, 75, 0, |v| r.i16(8, v / 8));
    f!(geom_rate, 75, 1, |v| r.i16(9, v / 8));
    let ground = a.on_ground(now);
    if !ground {
        f!(alt_baro, 73, 4, |v| r.i16(10, v / 25));
    }
    f!(alt_geom, 73, 5, |v| r.i16(11, v / 25));
    f!(nav_alt_mcp, 76, 6, |v| r.u16(12, v / 4));
    f!(nav_alt_fms, 76, 7, |v| r.u16(13, v / 4));
    f!(nav_qnh, 76, 5, |v| r.i16(14, (v * 10.0).round() as i32));
    f!(nav_heading, 77, 1, |v| r.i16(15, (v * 90.0).round() as i32));
    f!(squawk, 76, 2, |v| r.u16(16, v as u32));
    f!(gs, 73, 7, |v| r.i16(17, (v * 10.0).round() as i32));
    f!(mach, 74, 2, |v| r.i16(18, (v * 1000.0).round() as i32));
    f!(roll, 74, 5, |v| r.i16(19, (v * 100.0).round() as i32));
    f!(track, 74, 3, |v| r.i16(20, (v * 90.0).round() as i32));
    f!(track_rate, 74, 4, |v| r.i16(21, (v * 100.0).round() as i32));
    f!(mag_heading, 74, 6, |v| r.i16(22, (v * 90.0).round() as i32));
    if let Some((v, _)) = a.true_heading_at(now) {
        r.i16(23, (v * 90.0).round() as i32);
        r.bit(74, 7, true);
    }
    if let Some((wd, ws)) = json::wind(a, now) {
        r.i16(24, wd.round() as i32);
        r.i16(25, ws.round() as i32);
        r.bit(77, 4, true);
    }
    if let Some((oat, tat)) = json::temps(a, now) {
        r.i16(26, oat.round() as i32);
        r.i16(27, tat.round() as i32);
        r.bit(77, 5, true);
    }
    f!(tas, 74, 1, |v| r.u16(28, v as u32));
    f!(ias, 74, 0, |v| r.u16(29, v as u32));
    // messages per second over the last few seconds, in tenths
    let span = now.saturating_sub(a.rate_t);
    if span >= 4000 {
        a.rate = (a.messages.wrapping_sub(a.rate_n) as f32 * 1000.0 / span as f32).min(6000.0);
        a.rate_n = a.messages;
        a.rate_t = now;
    }
    r.u16(31, (a.rate * 10.0) as u32);
    r.0[64] = a.category;
    f!(nav_modes, 77, 2, |v| r.0[66] = v);
    let mut emergency = 0;
    f!(emergency, 76, 3, |v| emergency = v & 15);
    let ty = match json::type_str(a, now) {
        "mlat" => 5,
        "adsc" => 4,
        "other" => 6,
        _ => (a.addr_type as u8).min(13),
    };
    r.0[67] = emergency | ty << 4;
    r.0[68] = if ground {
        1
    } else if a.ground.valid(now) {
        2
    } else {
        0
    };
    // 0 invalid, 1 unknown, 2 per sample, 3 per hour
    let sil_type = [1u8, 3, 2].get(a.sil_type as usize).copied().unwrap_or(0);
    r.0[69] = sil_type | ((a.version as u8) & 15) << 4;
    r.0[70] = ((a.adsr_version.max(0) as u8) & 15) | ((a.tisb_version.max(0) as u8) & 15) << 4;
    let mut b71 = 0;
    f!(nac_p, 75, 5, |v| b71 |= v & 15);
    f!(nac_v, 75, 6, |v| b71 |= (v & 15) << 4);
    r.0[71] = b71;
    let mut b72 = 0;
    f!(sil, 75, 7, |v| b72 |= v & 3);
    f!(gva, 76, 0, |v| b72 |= (v & 3) << 2);
    f!(sda, 76, 1, |v| b72 |= (v & 3) << 4);
    if a.version >= 1 {
        b72 |= (a.nic_a as u8) << 6 | (a.nic_c as u8) << 7;
        r.bit(75, 2, true);
        r.bit(75, 3, a.pos_surface);
    }
    r.0[72] = b72;
    f!(nic_baro, 75, 4, |v| r.bit(73, 0, v));
    f!(alert, 77, 3, |v| r.bit(73, 1, v));
    f!(spi, 76, 4, |v| r.bit(73, 2, v));
    if let Some(cs) = a.callsign.get(now) {
        r.0[78..86].copy_from_slice(&cs);
        r.bit(73, 3, true);
    }
    if let Some(d) = &a.db {
        r.u16(43, d.flags);
        for (o, b) in r.0[88..92].iter_mut().zip(d.typ.bytes()) {
            *o = b;
        }
        for (o, b) in r.0[92..104].iter_mut().zip(d.reg.bytes()) {
            *o = b;
        }
    }
    r.0[104] = a.receiver_count(now);
    let rssi = if a.pos.src == Source::Jaero || a.pos.src == Source::Mlat { -50.0 } else { a.rssi() };
    r.0[105] = ((rssi + 50.0) * 255.0 / 50.0).clamp(0.0, 255.0) as u8;
    r.i32(27, (now.saturating_sub(a.seen) / 100) as i32);
    out.extend_from_slice(&r.0);
}

pub fn header(ctx: &Ctx, now: Ms, index: u32, with_pos: u32, messages: u64, out: &mut Vec<u8>) {
    let mut h = Rec([0; STRIDE]);
    h.i32(0, (now & 0xFFFF_FFFF) as u32 as i32);
    h.i32(1, (now >> 32) as i32);
    h.i32(2, STRIDE as i32);
    h.i32(3, with_pos as i32);
    h.i32(4, index as i32);
    let (s, w, n, e) = if index == WHOLE { (-90, -180, 90, 180) } else { bounds(index) };
    h.i16(10, s as i32);
    h.i16(11, w as i32);
    h.i16(12, n as i32);
    h.i16(13, e as i32);
    h.i32(7, messages as u32 as i32);
    if let Some((lat, lon)) = ctx.cfg.site {
        h.i32(8, (lat * 1e6) as i32);
        h.i32(9, (lon * 1e6) as i32);
    }
    h.i32(10, VERSION as i32);
    h.i32(11, (ctx.message_rate() * 10.0) as u32 as i32);
    h.i32(12, 1);
    out.extend_from_slice(&h.0);
}

/// The whole list as one binCraft file.
pub fn whole(ctx: &Ctx, now: Ms) -> Vec<u8> {
    let mut body = Vec::with_capacity(1 << 18);
    let (mut with_pos, mut messages) = (0u32, 0u64);
    for s in ctx.store.shards() {
        let mut s = s.lock();
        messages += s.messages;
        for a in s.map.values_mut() {
            if !json::listed(a, now, ctx.cfg.list_window_ms) {
                continue;
            }
            with_pos += a.pos.valid(now) as u32;
            record(a, now, &mut body);
        }
    }
    let mut out = Vec::with_capacity(body.len() + STRIDE);
    header(ctx, now, WHOLE, with_pos, messages, &mut out);
    out.extend_from_slice(&body);
    out
}

/// Every globe tile in one pass over the table: (all aircraft, military
/// only), keyed by tile index. Tiles with nobody in them are absent.
pub fn tiles(ctx: &Ctx, now: Ms) -> (HashMap<u16, Vec<u8>>, HashMap<u16, Vec<u8>>) {
    let mut all: HashMap<u16, Vec<u8>> = HashMap::new();
    let mut mil: HashMap<u16, Vec<u8>> = HashMap::new();
    let (mut with_pos, mut messages) = (0u32, 0u64);
    let mut rec = Vec::with_capacity(STRIDE);
    for s in ctx.store.shards() {
        let mut s = s.lock();
        messages += s.messages;
        for a in s.map.values_mut() {
            if !a.pos.valid(now) || !json::listed(a, now, ctx.cfg.list_window_ms) {
                continue;
            }
            with_pos += 1;
            let t = tile_of(a.pos.v.0, a.pos.v.1);
            rec.clear();
            record(a, now, &mut rec);
            all.entry(t).or_default().extend_from_slice(&rec);
            if a.db.as_ref().map_or(false, |d| d.flags & 1 != 0) {
                mil.entry(t).or_default().extend_from_slice(&rec);
            }
        }
    }
    let finish = |m: HashMap<u16, Vec<u8>>| {
        m.into_iter()
            .map(|(t, body)| {
                let mut out = Vec::with_capacity(body.len() + STRIDE);
                header(ctx, now, t as u32, with_pos, messages, &mut out);
                out.extend_from_slice(&body);
                (t, out)
            })
            .collect()
    };
    (finish(all), finish(mil))
}

/// A tile nobody is in.
pub fn empty(ctx: &Ctx, now: Ms, index: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(STRIDE);
    header(ctx, now, index, 0, ctx.store.messages(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiles_cover_the_globe() {
        // inside a special tile, and on the regular grid
        assert_eq!(tile_of(70.0, -100.0), 0);
        let t = tile_of(52.4, 13.5);
        let (s, w, n, e) = bounds(t as u32);
        assert!(52.4 >= s as f64 && 52.4 < n as f64 && 13.5 >= w as f64 && 13.5 < e as f64, "{t}: {s} {w} {n} {e}");
        for (lat, lon) in [(-89.9, -179.9), (89.9, 179.9), (0.0, 0.0), (-33.9, 151.2), (64.0, 170.0), (64.0, -170.0)] {
            let (s, w, n, e) = bounds(tile_of(lat, lon) as u32);
            let lon_in = if w <= e { lon >= w as f64 && lon < e as f64 } else { lon >= w as f64 || lon < e as f64 };
            assert!(lat >= s as f64 && lat < n as f64 && lon_in, "{lat} {lon}: {s} {w} {n} {e}");
        }
    }
}
