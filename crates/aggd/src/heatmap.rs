//! Heat map files as tar1090 reads them: every half hour, one gzipped
//! file of positions sampled every N seconds, under
//! `<history>/YYYY/MM/DD/heatmap/NN.bin.ttf` (NN: half hour of the UTC
//! day). The layout was read off readsb's own files: little-endian
//! 32-bit words in records of four,
//!
//!   index   one record per sample: [record number where it starts, 0, 0, 0]
//!   sample  [0x0e7f7c9d, time ms high word, time ms low word, interval ms]
//!           then for each aircraft with a position
//!           [address | kind << 27, lat * 1e6, lon * 1e6, alt/25 ft | gs*10 << 16]
//!           and, when its callsign or squawk is new,
//!           [address, 1 << 30 | squawk as a decimal number, callsign (8 bytes)]

use crate::track::Ms;
use crate::{json, Ctx};
use parking_lot::Mutex;
use std::io::Write;

const MAGIC: u32 = 0x0e7f_7c9d;
const HALF_HOUR: Ms = 1_800_000;

#[derive(Default)]
pub struct Heatmap {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// the half hour being collected (ms / HALF_HOUR)
    slot: u64,
    /// one buffer of words per sample taken so far
    samples: Vec<Vec<u32>>,
}

impl Heatmap {
    /// Take one sample of the table; at the turn of the half hour, write
    /// the file for the one that ended. Called every `interval_ms`.
    pub fn sample(&self, ctx: &Ctx, dir: &str, now: Ms, interval_ms: Ms) {
        let slot = now / HALF_HOUR;
        let mut st = self.state.lock();
        if st.slot != slot {
            if !st.samples.is_empty() {
                let (done, samples) = (st.slot, std::mem::take(&mut st.samples));
                let per_file = (HALF_HOUR / interval_ms.max(1)) as usize;
                if let Err(e) = write_file(dir, done, &samples, per_file) {
                    eprintln!("aggd: heat map not written: {e}");
                }
            }
            st.slot = slot;
        }
        let mut w: Vec<u32> = Vec::with_capacity(4096);
        w.extend_from_slice(&[MAGIC, (now >> 32) as u32, now as u32, interval_ms as u32]);
        for s in ctx.store.shards() {
            let mut s = s.lock();
            for a in s.map.values_mut() {
                if !a.pos.valid(now) || !json::listed(a, now, ctx.cfg.list_window_ms) {
                    continue;
                }
                let kind = match json::type_str(a, now) {
                    "mlat" => 5,
                    "adsc" => 4,
                    "other" => 6,
                    _ => (a.addr_type as u32).min(13),
                };
                let alt: i16 = if a.on_ground(now) { -123 } else { a.alt_baro.get(now).map_or(-124, |v| (v / 25).clamp(-100, 32000) as i16) };
                let gs: i16 = a.gs.get(now).map_or(-1, |g| (g * 10.0).min(32000.0) as i16);
                w.extend_from_slice(&[
                    (a.addr & 0x1FF_FFFF) | kind << 27,
                    (a.pos.v.0 * 1e6) as i32 as u32,
                    (a.pos.v.1 * 1e6) as i32 as u32,
                    (alt as u16 as u32) | (gs as u16 as u32) << 16,
                ]);
                // who it is, once, and again when that changes
                let cs = a.callsign.get(now).unwrap_or(*b"        ");
                let sq = a.squawk.get(now).map_or(0, |q| {
                    (q >> 12 & 15) as u32 * 1000 + (q >> 8 & 15) as u32 * 100 + (q >> 4 & 15) as u32 * 10 + (q & 15) as u32
                });
                let tag = (u64::from_le_bytes(cs) ^ (sq as u64) << 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
                if a.heat_tag != tag || st.samples.is_empty() {
                    a.heat_tag = tag;
                    w.extend_from_slice(&[
                        a.addr & 0x1FF_FFFF,
                        1 << 30 | sq,
                        u32::from_le_bytes([cs[0], cs[1], cs[2], cs[3]]),
                        u32::from_le_bytes([cs[4], cs[5], cs[6], cs[7]]),
                    ]);
                }
            }
        }
        st.samples.push(w);
    }
}

fn write_file(dir: &str, slot: u64, samples: &[Vec<u32>], per_file: usize) -> std::io::Result<()> {
    let n = per_file.max(samples.len());
    let mut words: Vec<u32> = Vec::with_capacity(n * 4 + samples.iter().map(|s| s.len()).sum::<usize>());
    // the index, then the samples; a sample never taken is an empty one
    let mut start = n as u32;
    let mut starts = Vec::with_capacity(n);
    for i in 0..n {
        starts.push(start);
        start += samples.get(i).map_or(1, |s| (s.len() / 4) as u32);
    }
    for s in &starts {
        words.extend_from_slice(&[*s, 0, 0, 0]);
    }
    let t0 = slot * HALF_HOUR;
    for i in 0..n {
        match samples.get(i) {
            Some(s) => words.extend_from_slice(s),
            None => words.extend_from_slice(&[MAGIC, (t0 >> 32) as u32, t0 as u32, 0]),
        }
    }
    let mut raw = Vec::with_capacity(words.len() * 4);
    for w in &words {
        raw.extend_from_slice(&w.to_le_bytes());
    }
    let (y, m, d) = crate::persist::civil(t0);
    let path = format!("{dir}/{y:04}/{m:02}/{d:02}/heatmap");
    std::fs::create_dir_all(&path)?;
    let mut e = flate2::write::GzEncoder::new(Vec::with_capacity(raw.len() / 3), flate2::Compression::new(5));
    e.write_all(&raw)?;
    let gz = e.finish()?;
    let name = format!("{:02}.bin.ttf", (t0 % 86_400_000) / HALF_HOUR);
    let tmp = format!("{path}/.{name}.tmp");
    std::fs::write(&tmp, gz)?;
    std::fs::rename(&tmp, format!("{path}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn file_layout() {
        let dir = std::env::temp_dir().join(format!("aggd-heat-{}", std::process::id()));
        let dir = dir.to_str().unwrap().to_string();
        let slot = 1_794_835_800_000 / HALF_HOUR; // 2026-11-16 13:30 UTC, half hour 27
        let sample = |t: u64| vec![MAGIC, (t >> 32) as u32, t as u32, 30_000, 0x3c7f9e, 49_080_816, 21_323_379, 1509 | 4712 << 16];
        let samples = vec![sample(slot * HALF_HOUR), sample(slot * HALF_HOUR + 30_000)];
        write_file(&dir, slot, &samples, 60).unwrap();
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(format!("{dir}/2026/11/16/heatmap/27.bin.ttf")).unwrap()).read_to_end(&mut raw).unwrap();
        let w: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        // sixty index records, two samples of two records, 58 empty ones
        assert_eq!(w.len(), (60 + 2 * 2 + 58) * 4);
        assert_eq!((w[0], w[4], w[8], w[12]), (60, 62, 64, 65));
        assert_eq!(w[60 * 4], MAGIC);
        assert_eq!(&w[61 * 4..62 * 4], &[0x3c7f9e, 49_080_816, 21_323_379, 1509 | 4712 << 16]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
