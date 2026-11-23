//! Magnetic declination from the World Magnetic Model, so a magnetic
//! heading can be shown as a true one.

use crate::wmm_data::{COEFFS, EPOCH};
use std::sync::atomic::{AtomicI16, Ordering};
use std::sync::OnceLock;

const N: usize = 12;

/// Declination in degrees, east positive, at a geodetic position,
/// altitude in km above the ellipsoid, and decimal year.
pub fn declination(lat: f64, lon: f64, alt_km: f64, year: f64) -> f64 {
    let dt = year - EPOCH;
    let mut g = [[0f64; N + 1]; N + 1];
    let mut h = [[0f64; N + 1]; N + 1];
    for &(n, m, gv, hv, gd, hd) in COEFFS.iter() {
        g[n as usize][m as usize] = gv as f64 + gd as f64 * dt;
        h[n as usize][m as usize] = hv as f64 + hd as f64 * dt;
    }
    // geodetic to geocentric
    const A: f64 = 6378.137;
    const B: f64 = 6356.752_314_2;
    const RE: f64 = 6371.2;
    let e2 = 1.0 - (B * B) / (A * A);
    let phi = lat.clamp(-89.999, 89.999).to_radians();
    let rc = A / (1.0 - e2 * phi.sin().powi(2)).sqrt();
    let p = (rc + alt_km) * phi.cos();
    let z = (rc * (1.0 - e2) + alt_km) * phi.sin();
    let r = (p * p + z * z).sqrt();
    let phi_c = (z / r).asin();
    let (s, c) = (phi_c.sin(), phi_c.cos());
    let lam = lon.to_radians();

    // Schmidt semi-normalised associated Legendre functions of sin(lat)
    let mut pn = [[0f64; N + 1]; N + 1];
    pn[0][0] = 1.0;
    pn[1][0] = s;
    pn[1][1] = c;
    for n in 2..=N {
        let nf = n as f64;
        pn[n][n] = c * ((2.0 * nf - 1.0) / (2.0 * nf)).sqrt() * pn[n - 1][n - 1];
        for m in 0..n {
            let mf = m as f64;
            let prev2 = if n >= 2 && m <= n - 2 { ((nf - 1.0).powi(2) - mf * mf).sqrt() * pn[n - 2][m] } else { 0.0 };
            pn[n][m] = ((2.0 * nf - 1.0) * s * pn[n - 1][m] - prev2) / (nf * nf - mf * mf).sqrt();
        }
    }
    let (mut x, mut y, mut zc) = (0.0, 0.0, 0.0);
    for n in 1..=N {
        let nf = n as f64;
        let ratio = (RE / r).powi(n as i32 + 2);
        for m in 0..=n {
            let mf = m as f64;
            let (sm, cm) = (mf * lam).sin_cos();
            let below = if m < n { (nf * nf - mf * mf).sqrt() * pn[n - 1][m] } else { 0.0 };
            let dp = (below - nf * s * pn[n][m]) / c;
            let gc = g[n][m] * cm + h[n][m] * sm;
            x -= ratio * gc * dp;
            y += ratio * mf * (g[n][m] * sm - h[n][m] * cm) * pn[n][m] / c;
            zc -= (nf + 1.0) * ratio * gc * pn[n][m];
        }
    }
    // back to the geodetic frame
    let d = phi_c - phi;
    let xg = x * d.cos() - zc * d.sin();
    y.atan2(xg).to_degrees()
}

fn year_now() -> f64 {
    1970.0 + crate::now_ms() as f64 / (365.2425 * 86_400_000.0)
}

/// Declination at a whole-degree grid point, at cruise altitude,
/// worked out on first use.
fn grid(la: i32, lo: i32) -> f64 {
    static T: OnceLock<Vec<AtomicI16>> = OnceLock::new();
    let t = T.get_or_init(|| (0..181 * 360).map(|_| AtomicI16::new(i16::MIN)).collect());
    let la = la.clamp(-90, 90);
    let lo = (lo + 180).rem_euclid(360);
    let cell = &t[((la + 90) * 360 + lo) as usize];
    let v = cell.load(Ordering::Relaxed);
    if v != i16::MIN {
        return v as f64 / 100.0;
    }
    let d = declination(la as f64, (lo - 180) as f64, 10.0, year_now());
    cell.store((d * 100.0).round() as i16, Ordering::Relaxed);
    d
}

/// Declination at a position: interpolated between the four grid points
/// around it. The field is smooth enough for that away from the
/// magnetic poles, where no heading means much anyway.
pub fn lookup(lat: f64, lon: f64) -> f32 {
    let (la0, lo0) = (lat.floor(), lon.floor());
    let (fy, fx) = (lat - la0, lon - lo0);
    let (la0, lo0) = (la0 as i32, lo0 as i32);
    // differences between corners are taken the short way round, so the
    // jump from +180 to -180 near a pole does not average to nonsense
    let base = grid(la0, lo0);
    let rel = |la: i32, lo: i32| (grid(la, lo) - base + 540.0).rem_euclid(360.0) - 180.0;
    let (b, c, d) = (rel(la0, lo0 + 1), rel(la0 + 1, lo0), rel(la0 + 1, lo0 + 1));
    let v = base + b * fx * (1.0 - fy) + c * (1.0 - fx) * fy + d * fx * fy;
    ((v + 540.0).rem_euclid(360.0) - 180.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a live readsb network: a JSON list of [true minus magnetic
    /// heading, lat, lon] taken from its aircraft.json.
    ///   AGGD_DECL_SAMPLES=file cargo test --release -- --ignored declination_on_live_data
    #[test]
    #[ignore]
    fn declination_on_live_data() {
        let path = std::env::var("AGGD_DECL_SAMPLES").expect("AGGD_DECL_SAMPLES");
        let text = std::fs::read_to_string(path).unwrap();
        let nums: Vec<f64> = text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e')).filter_map(|t| t.parse().ok()).collect();
        let mut errs: Vec<f64> = nums.chunks_exact(3).map(|s| ((lookup(s[1], s[2]) as f64 - s[0] + 540.0) % 360.0 - 180.0).abs()).collect();
        errs.sort_by(|a, b| a.total_cmp(b));
        let (median, p99) = (errs[errs.len() / 2], errs[errs.len() * 99 / 100]);
        println!("{} samples: median error {median:.3}, 99th percentile {p99:.3}, max {:.3}", errs.len(), errs[errs.len() - 1]);
        assert!(median < 0.1 && p99 < 0.5);
    }

    #[test]
    fn matches_the_published_test_values() {
        // WMM2025 test values (NOAA): date, altitude km, lat, lon, declination
        for (year, alt, lat, lon, want) in [
            (2025.0, 0.0, 80.0, 0.0, 1.28),
            (2025.0, 0.0, 0.0, 120.0, -0.16),
            (2025.0, 0.0, -80.0, 240.0, 68.78),
            (2025.0, 100.0, 80.0, 0.0, 0.85),
        ] {
            let got = declination(lat, lon, alt, year);
            assert!((got - want).abs() < 0.05, "{year} {alt} {lat} {lon}: {got} vs {want}");
        }
    }
}
