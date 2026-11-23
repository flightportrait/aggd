//! UAT (978 MHz) ADS-B downlink messages, as dump978 prints them: a line
//! `-<hex>;rs=..;rssi=..;` holding the 18 or 34 payload bytes. Field
//! layout per DO-282B; checked by sending frames built from that layout
//! to readsb and reading back what it showed.

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Uat {
    pub addr: u32,
    /// address qualifier: 0 ADS-B ICAO, 2 TIS-B ICAO, 3 TIS-B track file, 6 rebroadcast, ...
    pub qualifier: u8,
    pub pos: Option<(f64, f64)>,
    pub alt_baro: Option<i32>,
    pub alt_geom: Option<i32>,
    pub ground: bool,
    pub gs: Option<f32>,
    pub track: Option<f32>,
    pub baro_rate: Option<i32>,
    pub geom_rate: Option<i32>,
    pub callsign: Option<[u8; 8]>,
    /// four octal digits in hex nibbles
    pub squawk: Option<u16>,
    pub category: Option<u8>,
    pub emergency: Option<u8>,
    /// dBFS, when the line states it
    pub rssi: Option<f32>,
}

const B40: &[u8; 40] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ  ..";

/// One line of dump978 raw output. None for uplink (`+`), malformed or
/// unknown lines.
pub fn parse_line(line: &str) -> Option<Uat> {
    let line = line.trim();
    let body = line.strip_prefix('-')?;
    let (hexs, rest) = body.split_once(';').unwrap_or((body, ""));
    if hexs.len() != 36 && hexs.len() != 68 && hexs.len() != 96 {
        return None;
    }
    let mut f = [0u8; 48];
    for (i, o) in f.iter_mut().enumerate().take(hexs.len() / 2) {
        *o = u8::from_str_radix(hexs.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    let long = hexs.len() >= 68;
    let mut u = decode(&f, long)?;
    u.rssi = rest.split(';').find_map(|p| p.strip_prefix("rssi=")).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
    Some(u)
}

pub fn decode(f: &[u8; 48], long: bool) -> Option<Uat> {
    let mdb = f[0] >> 3;
    let mut u = Uat { qualifier: f[0] & 7, addr: (f[1] as u32) << 16 | (f[2] as u32) << 8 | f[3] as u32, ..Default::default() };
    if u.addr == 0 || mdb > 10 {
        return None;
    }
    // state vector
    let nic = f[11] & 15;
    let raw_lat = (f[4] as u32) << 15 | (f[5] as u32) << 7 | (f[6] as u32) >> 1;
    let raw_lon = ((f[6] & 1) as u32) << 23 | (f[7] as u32) << 15 | (f[8] as u32) << 7 | (f[9] as u32) >> 1;
    if nic != 0 || raw_lat != 0 || raw_lon != 0 {
        let mut lat = raw_lat as f64 * 360.0 / 16_777_216.0;
        if lat > 90.0 {
            lat -= 180.0;
        }
        let mut lon = raw_lon as f64 * 360.0 / 16_777_216.0;
        if lon > 180.0 {
            lon -= 360.0;
        }
        if (-90.0..=90.0).contains(&lat) {
            u.pos = Some((lat, lon));
        }
    }
    let alt = |raw: u32| (raw != 0).then(|| (raw as i32 - 1) * 25 - 1000);
    let primary = alt((f[10] as u32) << 4 | (f[11] as u32) >> 4);
    let geometric_primary = f[9] & 1 != 0;
    if geometric_primary {
        u.alt_geom = primary;
    } else {
        u.alt_baro = primary;
    }
    let ag = f[12] >> 6;
    let a = ((f[12] & 0x1f) as u32) << 6 | (f[13] as u32) >> 2;
    let b = ((f[13] & 3) as u32) << 9 | (f[14] as u32) << 1 | (f[15] as u32) >> 7;
    match ag {
        0 | 1 => {
            let mul = if ag == 1 { 4.0 } else { 1.0 };
            if a & 0x3ff != 0 && b & 0x3ff != 0 {
                let mut ns = ((a & 0x3ff) - 1) as f32 * mul;
                let mut ew = ((b & 0x3ff) - 1) as f32 * mul;
                if a & 0x400 != 0 {
                    ns = -ns;
                }
                if b & 0x400 != 0 {
                    ew = -ew;
                }
                let gs = (ns * ns + ew * ew).sqrt();
                u.gs = Some(gs);
                if gs > 0.0 {
                    u.track = Some(ew.atan2(ns).to_degrees().rem_euclid(360.0));
                }
            }
            let v = ((f[15] & 0x7f) as u32) << 4 | (f[16] as u32) >> 4;
            if v & 0x1ff != 0 {
                let mut r = ((v & 0x1ff) as i32 - 1) * 64;
                if v & 0x200 != 0 {
                    r = -r;
                }
                if v & 0x400 != 0 {
                    u.geom_rate = Some(r);
                } else {
                    u.baro_rate = Some(r);
                }
            }
        }
        2 => {
            u.ground = true;
            if a & 0x3ff != 0 {
                u.gs = Some(((a & 0x3ff) - 1) as f32);
            }
            if (b >> 9) & 3 != 0 {
                u.track = Some((b & 0x1ff) as f32 * 360.0 / 512.0);
            }
        }
        _ => {}
    }
    if !long {
        return Some(u);
    }
    // mode status
    if matches!(mdb, 1 | 3) {
        let w = |i: usize| (f[i] as u32) << 8 | f[i + 1] as u32;
        let (v0, v1, v2) = (w(17), w(19), w(21));
        let idx = [(v0 / 40) % 40, v0 % 40, (v1 / 1600) % 40, (v1 / 40) % 40, v1 % 40, (v2 / 1600) % 40, (v2 / 40) % 40, v2 % 40];
        let mut cs = [b' '; 8];
        for (o, i) in cs.iter_mut().zip(idx) {
            *o = B40[i as usize];
        }
        let category = ((v0 / 1600) % 40) as u8;
        if f[26] & 2 != 0 {
            if cs.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b' ') && cs != *b"        " {
                u.callsign = Some(cs);
            }
            // 1..7 are the light-to-heavy aircraft classes A1..A7, and so on
            u.category = Some(0xA0 + (category / 8) * 0x10 + category % 8).filter(|_| category != 0 && category < 32);
        } else if cs[..4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            // the field holds the Mode A code instead
            u.squawk = Some(cs[..4].iter().fold(0u16, |acc, c| acc << 4 | (c - b'0') as u16));
            u.emergency = Some((f[23] >> 5) & 7);
        }
    }
    // auxiliary state vector: the other kind of altitude
    if matches!(mdb, 1 | 2 | 5 | 6) {
        let other = alt((f[29] as u32) << 4 | (f[30] as u32) >> 4);
        if geometric_primary {
            u.alt_baro = other;
        } else {
            u.alt_geom = other;
        }
    }
    Some(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let mut f = [0u8; 48];
        f[0] = 1 << 3;
        f[1..4].copy_from_slice(&[0xA1, 0xB2, 0xC3]);
        // 40.5 N, 74.25 W
        let rlat = (40.5f64 / (360.0 / 16_777_216.0)).round() as u32;
        let rlon = ((360.0 - 74.25f64) / (360.0 / 16_777_216.0)).round() as u32;
        f[4] = (rlat >> 15) as u8;
        f[5] = (rlat >> 7) as u8;
        f[6] = ((rlat & 0x7f) << 1) as u8 | (rlon >> 23) as u8;
        f[7] = (rlon >> 15) as u8;
        f[8] = (rlon >> 7) as u8;
        f[9] = ((rlon & 0x7f) << 1) as u8;
        let ralt = (5500 + 1000) / 25 + 1;
        f[10] = (ralt >> 4) as u8;
        f[11] = ((ralt & 15) << 4) as u8 | 9;
        // 100 kt north, 0 east
        let (rns, rew) = (101u32, 1u32);
        f[12] = (rns >> 6) as u8;
        f[13] = ((rns & 0x3f) << 2) as u8 | (rew >> 9) as u8;
        f[14] = (rew >> 1) as u8;
        f[15] = ((rew & 1) << 7) as u8;
        let v = 1 * 1600 + 23 * 40 + 1; // category 1, "N1"
        f[17] = (v >> 8) as u8;
        f[18] = v as u8;
        f[26] = 2;
        let u = decode(&f, true).unwrap();
        assert_eq!((u.addr, u.alt_baro, u.category), (0xA1B2C3, Some(5500), Some(0xA1)));
        let (lat, lon) = u.pos.unwrap();
        assert!((lat - 40.5).abs() < 1e-4 && (lon + 74.25).abs() < 1e-4);
        assert_eq!(u.gs, Some(100.0));
        assert!(u.track.unwrap().abs() < 0.01);
        assert_eq!(&u.callsign.unwrap()[..2], b"N1");
        assert!(parse_line("+00;").is_none() && parse_line("-zz;").is_none() && parse_line("").is_none());
    }
}
