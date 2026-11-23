//! Compact position reporting.

use std::sync::OnceLock;

const SCALE: f64 = 131072.0;

fn nl_table() -> &'static [f64; 58] {
    static T: OnceLock<[f64; 58]> = OnceLock::new();
    T.get_or_init(|| {
        // T[k]: the latitude above which fewer than 59-k zones fit
        let mut t = [0f64; 58];
        let a = 1.0 - (std::f64::consts::PI / 30.0).cos();
        for (k, v) in t.iter_mut().enumerate() {
            let nl = 59 - k;
            let b = 1.0 - (2.0 * std::f64::consts::PI / nl as f64).cos();
            *v = (a / b).sqrt().acos().to_degrees();
        }
        t
    })
}

/// Number of longitude zones at a latitude.
pub fn nl(lat: f64) -> u32 {
    let lat = lat.abs();
    if lat >= 87.0 {
        return if lat > 87.0 { 1 } else { 2 };
    }
    let t = nl_table();
    // t is increasing: t[0] is the 59->58 boundary
    let k = t.partition_point(|&b| b <= lat);
    59 - k as u32
}

fn modf(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r < 0.0 {
        r + b
    } else {
        r
    }
}
fn modi(a: i64, b: i64) -> i64 {
    a.rem_euclid(b)
}

/// Global airborne decode from an even/odd pair. `odd_latest`: the odd
/// message is the newer one, so the position is its position.
pub fn global_airborne(elat: u32, elon: u32, olat: u32, olon: u32, odd_latest: bool) -> Option<(f64, f64)> {
    let (lat0, lat1, lon0, lon1) = (elat as f64, olat as f64, elon as f64, olon as f64);
    let j = ((59.0 * lat0 - 60.0 * lat1) / SCALE + 0.5).floor() as i64;
    let mut rlat0 = 6.0 * (modi(j, 60) as f64 + lat0 / SCALE);
    let mut rlat1 = 360.0 / 59.0 * (modi(j, 59) as f64 + lat1 / SCALE);
    if rlat0 >= 270.0 {
        rlat0 -= 360.0;
    }
    if rlat1 >= 270.0 {
        rlat1 -= 360.0;
    }
    if !(-90.0..=90.0).contains(&rlat0) || !(-90.0..=90.0).contains(&rlat1) {
        return None;
    }
    if nl(rlat0) != nl(rlat1) {
        return None;
    }
    let (rlat, lonv, i) = if odd_latest { (rlat1, lon1, 1) } else { (rlat0, lon0, 0) };
    let n = nl(rlat) as i64;
    let ni = (n - i).max(1);
    let m = ((lon0 * (n - 1) as f64 - lon1 * n as f64) / SCALE + 0.5).floor() as i64;
    let mut lon = 360.0 / ni as f64 * (modi(m, ni) as f64 + lonv / SCALE);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    Some((rlat, lon))
}

/// Global surface decode: the pair leaves four-fold ambiguity in
/// longitude and two-fold in latitude, resolved by the nearest solution
/// to a reference position.
pub fn global_surface(
    elat: u32,
    elon: u32,
    olat: u32,
    olon: u32,
    odd_latest: bool,
    ref_lat: f64,
    ref_lon: f64,
) -> Option<(f64, f64)> {
    let (lat0, lat1, lon0, lon1) = (elat as f64, olat as f64, elon as f64, olon as f64);
    let j = ((59.0 * lat0 - 60.0 * lat1) / SCALE + 0.5).floor() as i64;
    let mut rlat0 = 1.5 * (modi(j, 60) as f64 + lat0 / SCALE);
    let mut rlat1 = 90.0 / 59.0 * (modi(j, 59) as f64 + lat1 / SCALE);
    // northern solution in 0..90; the southern one is 90 lower
    if (rlat0 - 90.0 - ref_lat).abs() < (rlat0 - ref_lat).abs() {
        rlat0 -= 90.0;
        rlat1 -= 90.0;
    }
    if !(-90.0..=90.0).contains(&rlat0) || !(-90.0..=90.0).contains(&rlat1) {
        return None;
    }
    if nl(rlat0) != nl(rlat1) {
        return None;
    }
    let (rlat, lonv, i) = if odd_latest { (rlat1, lon1, 1) } else { (rlat0, lon0, 0) };
    let n = nl(rlat) as i64;
    let ni = (n - i).max(1);
    let m = ((lon0 * (n - 1) as f64 - lon1 * n as f64) / SCALE + 0.5).floor() as i64;
    let base = 90.0 / ni as f64 * (modi(m, ni) as f64 + lonv / SCALE);
    let mut best = (f64::MAX, 0.0);
    for q in 0..4 {
        let mut lon = base + 90.0 * q as f64;
        if lon >= 180.0 {
            lon -= 360.0;
        }
        let mut d = (lon - ref_lon).abs();
        if d > 180.0 {
            d = 360.0 - d;
        }
        if d < best.0 {
            best = (d, lon);
        }
    }
    Some((rlat, best.1))
}

/// Local decode of one message against a nearby reference (the
/// aircraft's last position). None when the answer would lie outside
/// the unambiguous half-zone around the reference.
pub fn local(lat: u32, lon: u32, odd: bool, surface: bool, ref_lat: f64, ref_lon: f64) -> Option<(f64, f64)> {
    let i = odd as i64;
    let span = if surface { 90.0 } else { 360.0 };
    let dlat = span / (60 - i) as f64;
    let j = (ref_lat / dlat).floor() + (0.5 + modf(ref_lat, dlat) / dlat - lat as f64 / SCALE).floor();
    let rlat = dlat * (j + lat as f64 / SCALE);
    if !(-90.0..=90.0).contains(&rlat) || (rlat - ref_lat).abs() > dlat / 2.0 {
        return None;
    }
    let ni = (nl(rlat) as i64 - i).max(1);
    let dlon = span / ni as f64;
    let m = (ref_lon / dlon).floor() + (0.5 + modf(ref_lon, dlon) / dlon - lon as f64 / SCALE).floor();
    let mut rlon = dlon * (m + lon as f64 / SCALE);
    if (rlon - ref_lon).abs() > dlon / 2.0 {
        return None;
    }
    if rlon >= 180.0 {
        rlon -= 360.0;
    } else if rlon < -180.0 {
        rlon += 360.0;
    }
    Some((rlat, rlon))
}

/// Encode a position (17-bit lat, lon).
pub fn encode(lat: f64, lon: f64, odd: bool, surface: bool) -> (u32, u32) {
    let i = odd as i64;
    let span = if surface { 90.0 } else { 360.0 };
    let dlat = span / (60 - i) as f64;
    let yz = (SCALE * modf(lat, dlat) / dlat + 0.5).floor();
    let rlat = dlat * (yz / SCALE + (lat / dlat).floor());
    let dlon = span / (nl(rlat) as i64 - i).max(1) as f64;
    let xz = (SCALE * modf(lon, dlon) / dlon + 0.5).floor();
    ((yz as i64 & 0x1FFFF) as u32, (xz as i64 & 0x1FFFF) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zones() {
        assert_eq!(nl(0.0), 59);
        assert_eq!(nl(10.0), 59);
        assert_eq!(nl(10.5), 58);
        assert_eq!(nl(52.0), 36);
        assert_eq!(nl(86.9), 2);
        assert_eq!(nl(89.0), 1);
    }
    #[test]
    fn roundtrip_everywhere() {
        for &(lat, lon) in &[(45.5, 9.25), (-33.9, 151.2), (1.35, 103.82), (64.1, -21.9), (-54.8, -68.3), (0.01, -0.01)] {
            let e = encode(lat, lon, false, false);
            let o = encode(lat, lon, true, false);
            for odd_latest in [false, true] {
                let (a, b) = global_airborne(e.0, e.1, o.0, o.1, odd_latest).unwrap();
                assert!((a - lat).abs() < 1e-4 && (b - lon).abs() < 1e-4, "{lat} {lon} -> {a} {b}");
            }
            let (a, b) = local(e.0, e.1, false, false, lat + 0.3, lon - 0.4).unwrap();
            assert!((a - lat).abs() < 1e-4 && (b - lon).abs() < 1e-4);
            let es = encode(lat, lon, false, true);
            let os = encode(lat, lon, true, true);
            let (a, b) = global_surface(es.0, es.1, os.0, os.1, true, lat + 1.0, lon + 1.0).unwrap();
            assert!((a - lat).abs() < 1e-4 && (b - lon).abs() < 1e-4, "surface {lat} {lon} -> {a} {b}");
            let (a, b) = local(os.0, os.1, true, true, lat + 0.05, lon - 0.05).unwrap();
            assert!((a - lat).abs() < 1e-4 && (b - lon).abs() < 1e-4);
        }
    }
}
