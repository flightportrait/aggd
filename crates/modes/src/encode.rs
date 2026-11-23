//! Frame encoders, for tests and the traffic generator.

use crate::{alt, cpr, crc_of};

struct Me(u64);
impl Me {
    /// Set bits `first..=last` (1-based, of 56).
    fn set(&mut self, first: u32, last: u32, v: u64) {
        let w = last - first + 1;
        self.0 |= (v & ((1 << w) - 1)) << (56 - last);
    }
}

fn es(df: u8, low3: u8, addr: u32, me: Me) -> [u8; 14] {
    let mut f = [0u8; 14];
    f[0] = df << 3 | low3;
    f[1..4].copy_from_slice(&addr.to_be_bytes()[1..]);
    f[4..11].copy_from_slice(&me.0.to_be_bytes()[1..]);
    let c = crc_of(&f[..11]);
    f[11..].copy_from_slice(&c.to_be_bytes()[1..]);
    f
}
fn df17(addr: u32, me: Me) -> [u8; 14] {
    es(17, 5, addr, me)
}

pub fn ident(addr: u32, callsign: &[u8; 8], category: u8) -> [u8; 14] {
    let mut me = Me(0);
    me.set(1, 5, (0x0E - (category >> 4)) as u64);
    me.set(6, 8, (category & 7) as u64);
    for (i, c) in callsign.iter().enumerate() {
        let v = match c {
            b'A'..=b'Z' => c - b'A' + 1,
            b'0'..=b'9' => c - b'0' + 48,
            _ => 32,
        };
        me.set(9 + 6 * i as u32, 14 + 6 * i as u32, v as u64);
    }
    df17(addr, me)
}

pub fn airborne_position(addr: u32, lat: f64, lon: f64, alt_ft: i32, odd: bool, tc: u8) -> [u8; 14] {
    let (la, lo) = cpr::encode(lat, lon, odd, false);
    let mut me = Me(0);
    me.set(1, 5, tc as u64);
    me.set(9, 20, alt::ac12_encode(alt_ft) as u64);
    me.set(22, 22, odd as u64);
    me.set(23, 39, la as u64);
    me.set(40, 56, lo as u64);
    df17(addr, me)
}

pub fn surface_position(addr: u32, lat: f64, lon: f64, gs_kt: f32, track: f32, odd: bool) -> [u8; 14] {
    let (la, lo) = cpr::encode(lat, lon, odd, true);
    let mov = match gs_kt {
        g if g < 0.125 => 1,
        g if g < 1.0 => 3 + ((g - 0.125) / 0.125) as u64,
        g if g < 2.0 => 9 + ((g - 1.0) / 0.25) as u64,
        g if g < 15.0 => 13 + ((g - 2.0) / 0.5) as u64,
        g if g < 70.0 => 39 + (g - 15.0) as u64,
        g if g < 100.0 => 94 + ((g - 70.0) / 2.0) as u64,
        g if g < 175.0 => 109 + ((g - 100.0) / 5.0) as u64,
        _ => 124,
    };
    let mut me = Me(0);
    me.set(1, 5, 7);
    me.set(6, 12, mov);
    me.set(13, 13, 1);
    me.set(14, 20, (track as f64 * 128.0 / 360.0).round() as u64 & 0x7F);
    me.set(22, 22, odd as u64);
    me.set(23, 39, la as u64);
    me.set(40, 56, lo as u64);
    es(17, 4, addr, me)
}

/// Subtype 1 velocity: ground speed and track, barometric rate, and the
/// GNSS-minus-barometric altitude difference.
pub fn velocity(addr: u32, gs_kt: f32, track: f32, baro_rate: i32, geom_delta: Option<i32>) -> [u8; 14] {
    let t = (track as f64).to_radians();
    let (vx, vy) = (gs_kt as f64 * t.sin(), gs_kt as f64 * t.cos());
    let mut me = Me(0);
    me.set(1, 5, 19);
    me.set(6, 8, 1);
    me.set(11, 13, 2);
    me.set(14, 14, (vx < 0.0) as u64);
    me.set(15, 24, vx.abs().round() as u64 + 1);
    me.set(25, 25, (vy < 0.0) as u64);
    me.set(26, 35, vy.abs().round() as u64 + 1);
    me.set(36, 36, 1);
    me.set(37, 37, (baro_rate < 0) as u64);
    me.set(38, 46, (baro_rate.unsigned_abs() / 64) as u64 + 1);
    if let Some(d) = geom_delta {
        me.set(49, 49, (d < 0) as u64);
        me.set(50, 56, (d.unsigned_abs() / 25) as u64 + 1);
    }
    df17(addr, me)
}

/// Type 28 subtype 1: emergency state and squawk.
pub fn emergency(addr: u32, state: u8, squawk: u16) -> [u8; 14] {
    let mut me = Me(0);
    me.set(1, 5, 28);
    me.set(6, 8, 1);
    me.set(9, 11, state as u64);
    me.set(12, 24, alt::id13_encode(squawk) as u64);
    df17(addr, me)
}

/// Type 29 subtype 1: selected altitude, QNH, selected heading, modes.
pub fn target_state(addr: u32, alt_mcp_ft: u32, qnh: f32, heading: f32, modes: u8) -> [u8; 14] {
    let mut me = Me(0);
    me.set(1, 5, 29);
    me.set(6, 7, 1);
    me.set(10, 20, (alt_mcp_ft / 32) as u64 + 1);
    me.set(21, 29, ((qnh - 800.0) / 0.8).round() as u64 + 1);
    me.set(30, 30, 1);
    me.set(31, 39, (heading as f64 * 256.0 / 180.0).round() as u64 & 0x1FF);
    me.set(40, 43, 10);
    me.set(44, 44, 1);
    me.set(45, 46, 3);
    me.set(47, 47, 1);
    me.set(48, 48, (modes & crate::NAV_AUTOPILOT != 0) as u64);
    me.set(49, 49, (modes & crate::NAV_VNAV != 0) as u64);
    me.set(50, 50, (modes & crate::NAV_ALTHOLD != 0) as u64);
    me.set(52, 52, (modes & crate::NAV_APPROACH != 0) as u64);
    me.set(53, 53, (modes & crate::NAV_TCAS != 0) as u64);
    me.set(54, 54, (modes & crate::NAV_LNAV != 0) as u64);
    df17(addr, me)
}

/// Type 31 subtype 0: airborne operational status, version 2.
pub fn op_status(addr: u32, nac_p: u8, sil: u8, gva: u8, sda: u8) -> [u8; 14] {
    let mut me = Me(0);
    me.set(1, 5, 31);
    me.set(31, 32, sda as u64);
    me.set(41, 43, 2);
    me.set(44, 44, 1);
    me.set(45, 48, nac_p as u64);
    me.set(49, 50, gva as u64);
    me.set(51, 52, sil as u64);
    me.set(53, 53, 1);
    df17(addr, me)
}

fn short(b0: u8, b1: u8, field: u16, addr: u32) -> [u8; 7] {
    let mut f = [b0, b1, (field >> 8) as u8, field as u8, 0, 0, 0];
    let c = crc_of(&f[..4]) ^ addr;
    f[4..].copy_from_slice(&c.to_be_bytes()[1..]);
    f
}

/// DF4 altitude reply.
pub fn df4(addr: u32, alt_ft: i32, ground: bool) -> [u8; 7] {
    short(4 << 3 | ground as u8, 0, alt::ac13_encode(alt_ft), addr)
}

/// DF5 identity reply.
pub fn df5(addr: u32, squawk: u16) -> [u8; 7] {
    short(5 << 3, 0, alt::id13_encode(squawk), addr)
}

/// DF11 all-call reply, interrogator id 0.
pub fn df11(addr: u32, ground: bool) -> [u8; 7] {
    let mut f = [0u8; 7];
    f[0] = 11 << 3 | if ground { 4 } else { 5 };
    f[1..4].copy_from_slice(&addr.to_be_bytes()[1..]);
    let c = crc_of(&f[..4]);
    f[4..].copy_from_slice(&c.to_be_bytes()[1..]);
    f
}

/// Any extended squitter: DF17 or DF18 with its 3-bit CA/CF field and a
/// 56-bit ME.
pub fn raw_es(df: u8, low3: u8, addr: u32, me: u64) -> [u8; 14] {
    es(df, low3, addr, Me(me))
}

/// Subtype 3 velocity: magnetic heading and airspeed (true when `tas`).
pub fn velocity_airspeed(addr: u32, heading: f32, speed_kt: u16, tas: bool, geom_rate: i32) -> [u8; 14] {
    let mut me = Me(0);
    me.set(1, 5, 19);
    me.set(6, 8, 3);
    me.set(14, 14, 1);
    me.set(15, 24, (heading as f64 * 1024.0 / 360.0).round() as u64 & 0x3FF);
    me.set(25, 25, tas as u64);
    me.set(26, 35, speed_kt as u64 + 1);
    me.set(37, 37, (geom_rate < 0) as u64);
    me.set(38, 46, (geom_rate.unsigned_abs() / 64) as u64 + 1);
    df17(addr, me)
}

/// A long reply with address parity: DF20 (altitude) or DF21 (identity)
/// carrying a Comm-B register, or DF16.
pub fn long_ap(df: u8, field13: u16, addr: u32, mb: u64) -> [u8; 14] {
    let mut f = [0u8; 14];
    f[0] = df << 3;
    f[2] = (field13 >> 8) as u8;
    f[3] = field13 as u8;
    f[4..11].copy_from_slice(&mb.to_be_bytes()[1..]);
    let c = crc_of(&f[..11]) ^ addr;
    f[11..].copy_from_slice(&c.to_be_bytes()[1..]);
    f
}

/// BDS 4,0: selected altitudes (feet) and barometric setting (hPa).
pub fn bds40(mcp_ft: u32, fms_ft: u32, qnh: f32) -> u64 {
    let mut me = Me(0);
    me.set(1, 1, 1);
    me.set(2, 13, (mcp_ft / 16) as u64);
    me.set(14, 14, 1);
    me.set(15, 26, (fms_ft / 16) as u64);
    me.set(27, 27, 1);
    me.set(28, 39, ((qnh - 800.0) * 10.0).round() as u64);
    me.0
}

/// BDS 5,0: roll, true track, ground speed, track rate, true airspeed.
pub fn bds50(roll: f32, track: f32, gs: u32, track_rate: f32, tas: u32) -> u64 {
    let signed = |v: f64, bits: u32| (v.round() as i64 as u64) & ((1 << bits) - 1);
    let t = if track > 180.0 { track - 360.0 } else { track };
    let mut me = Me(0);
    me.set(1, 1, 1);
    me.set(2, 11, signed(roll as f64 * 256.0 / 45.0, 10));
    me.set(12, 12, 1);
    me.set(13, 23, signed(t as f64 * 512.0 / 90.0, 11));
    me.set(24, 24, 1);
    me.set(25, 34, (gs / 2) as u64);
    me.set(35, 35, 1);
    me.set(36, 45, signed(track_rate as f64 * 256.0 / 8.0, 10));
    me.set(46, 46, 1);
    me.set(47, 56, (tas / 2) as u64);
    me.0
}

/// BDS 6,0: magnetic heading, IAS, Mach, barometric and inertial rates.
pub fn bds60(heading: f32, ias: u32, mach: f32, baro_rate: i32, inertial_rate: i32) -> u64 {
    let signed = |v: f64, bits: u32| (v.round() as i64 as u64) & ((1 << bits) - 1);
    let h = if heading > 180.0 { heading - 360.0 } else { heading };
    let mut me = Me(0);
    me.set(1, 1, 1);
    me.set(2, 12, signed(h as f64 * 512.0 / 90.0, 11));
    me.set(13, 13, 1);
    me.set(14, 23, ias as u64);
    me.set(24, 24, 1);
    me.set(25, 34, (mach as f64 * 512.0 / 2.048).round() as u64);
    me.set(35, 35, 1);
    me.set(36, 45, signed(baro_rate as f64 / 32.0, 10));
    me.set(46, 46, 1);
    me.set(47, 56, signed(inertial_rate as f64 / 32.0, 10));
    me.0
}

/// BDS 2,0: aircraft identification.
pub fn bds20(callsign: &[u8; 8]) -> u64 {
    let mut me = Me(0);
    me.set(1, 8, 0x20);
    for (i, c) in callsign.iter().enumerate() {
        let v = match c {
            b'A'..=b'Z' => c - b'A' + 1,
            b'0'..=b'9' => c - b'0' + 48,
            _ => 32,
        };
        me.set(9 + 6 * i as u32, 14 + 6 * i as u32, v as u64);
    }
    me.0
}

/// DF0 short air-air surveillance reply.
pub fn df0(addr: u32, alt_ft: i32) -> [u8; 7] {
    short(0, 0, alt::ac13_encode(alt_ft), addr)
}

/// The extended squitters that say what a solved position says: an even
/// and an odd airborne position, and a velocity when speed and track
/// are known. Sent as DF18 TIS-B (control field 2), the way a
/// multilateration result is put on a Beast stream.
pub fn synthetic_position(addr: u32, lat: f64, lon: f64, alt_ft: Option<i32>, gs_track: Option<(f32, f32)>, rate: Option<i32>) -> Vec<[u8; 14]> {
    let mut out = Vec::with_capacity(3);
    for odd in [false, true] {
        let (la, lo) = cpr::encode(lat, lon, odd, false);
        let mut me = Me(0);
        me.set(1, 5, 18); // type code 18: no integrity claimed
        if let Some(a) = alt_ft {
            me.set(9, 20, alt::ac12_encode(a.clamp(-1000, 50_000)) as u64);
        }
        me.set(22, 22, odd as u64);
        me.set(23, 39, la as u64);
        me.set(40, 56, lo as u64);
        out.push(es(18, 2, addr, me));
    }
    if let Some((gs, track)) = gs_track {
        let f = velocity(addr, gs.min(1000.0), track, rate.unwrap_or(0).clamp(-30_000, 30_000), None);
        let mut me = [0u8; 8];
        me[1..].copy_from_slice(&f[4..11]);
        let mut me = u64::from_be_bytes(me);
        if rate.is_none() {
            me &= !(0x1FF << 10); // vertical rate field: not available
        }
        out.push(es(18, 2, addr, Me(me)));
    }
    out
}
