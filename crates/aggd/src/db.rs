//! The airframe database: registration, type and flags by ICAO address,
//! from a tar1090-db `aircraft.csv.gz` (semicolon-separated:
//! icao;registration;type;flags;description;year;owner).
//!
//! Hundreds of thousands of airframes are on file and a few thousand
//! are in the air, so the file is kept compressed, in blocks of a few
//! hundred lines, with a sorted index; a block is opened and a record
//! unpacked only for an aircraft we hear.

use std::io::Read;
use std::sync::Arc;

#[derive(Default, Debug)]
pub struct Entry {
    pub reg: String,
    pub typ: String,
    pub flags: u32,
    pub desc: String,
    pub year: String,
    pub own_op: String,
}

#[derive(Default)]
pub struct Db {
    /// (address, offset of the rest of its line in the text), by address
    index: Vec<(u32, u32)>,
    /// the text in compressed blocks, each a whole number of lines:
    /// (offset of its first byte in the text, length, compressed bytes)
    blocks: Vec<(u32, u32, Box<[u8]>)>,
}

/// Text per block: a lookup opens one, and they are rare (once for each
/// aircraft that appears).
const BLOCK: usize = 32 * 1024;

impl Db {
    pub fn load(path: &str) -> anyhow::Result<Db> {
        let raw = std::fs::read(path)?;
        let text = if raw.starts_with(&[0x1f, 0x8b]) {
            let mut t = Vec::new();
            flate2::read::MultiGzDecoder::new(&raw[..]).read_to_end(&mut t)?;
            t
        } else {
            raw
        };
        Ok(Db::parse(text))
    }

    fn parse(text: Vec<u8>) -> Db {
        let mut index = Vec::with_capacity(text.len() / 48);
        let mut start = 0;
        while start < text.len() {
            let end = text[start..].iter().position(|&b| b == b'\n').map_or(text.len(), |i| start + i);
            let line = &text[start..end];
            if let Some(semi) = line.iter().position(|&b| b == b';') {
                let hex = std::str::from_utf8(&line[..semi]).unwrap_or("").trim();
                if let Ok(addr) = u32::from_str_radix(hex, 16) {
                    index.push((addr, (start + semi + 1) as u32));
                }
            }
            start = end + 1;
        }
        index.sort_unstable_by_key(|e| e.0);
        index.dedup_by_key(|e| e.0);
        index.shrink_to_fit();
        let mut blocks = Vec::new();
        let mut start = 0;
        while start < text.len() {
            let want = (start + BLOCK).min(text.len());
            let end = text[want..].iter().position(|&b| b == b'\n').map_or(text.len(), |i| want + i + 1);
            let z = zstd::bulk::compress(&text[start..end], 9).expect("compressing in memory");
            blocks.push((start as u32, (end - start) as u32, z.into_boxed_slice()));
            start = end;
        }
        Db { index, blocks }
    }

    pub fn get(&self, addr: u32) -> Option<Arc<Entry>> {
        let i = self.index.binary_search_by_key(&addr, |e| e.0).ok()?;
        let at = self.index[i].1;
        let (first, len, z) = &self.blocks[self.blocks.partition_point(|b| b.0 <= at).checked_sub(1)?];
        let text = zstd::bulk::decompress(z, *len as usize).ok()?;
        let start = (at - first) as usize;
        let end = text[start..].iter().position(|&b| b == b'\n').map_or(text.len(), |n| start + n);
        let line = String::from_utf8_lossy(&text[start..end]);
        let mut f = line.trim_end_matches('\r').split(';');
        let mut next = || f.next().unwrap_or("").trim().to_string();
        let reg = next();
        let typ = next();
        // flags are binary digits: military, interesting, PIA, LADD
        let flags = next().bytes().enumerate().fold(0u32, |a, (i, b)| if b == b'1' { a | 1 << i } else { a });
        // long descriptions and owners are cut at 64 bytes, as readsb cuts them
        let cut = |mut v: String| {
            if v.len() > 64 {
                let mut n = 64;
                while !v.is_char_boundary(n) {
                    n -= 1;
                }
                v.truncate(n);
            }
            v
        };
        let (desc, year, own_op) = (cut(next()), next(), cut(next()));
        Some(Arc::new(Entry { reg, typ, flags, desc, year, own_op }))
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// What is known of an airframe: its database entry, and whether
    /// its address lies in a military block (which marks it military
    /// whatever the entry says, or without one).
    pub fn describe(&self, addr: u32) -> Option<Arc<Entry>> {
        match (self.get(addr), military(addr)) {
            (Some(e), true) if e.flags & 1 == 0 => Some(Arc::new(Entry {
                reg: e.reg.clone(),
                typ: e.typ.clone(),
                flags: e.flags | 1,
                desc: e.desc.clone(),
                year: e.year.clone(),
                own_op: e.own_op.clone(),
            })),
            (Some(e), _) => Some(e),
            (None, true) => Some(military_entry()),
            (None, false) => None,
        }
    }
}

/// Whether an address lies in a block allocated to military operators.
pub fn military(addr: u32) -> bool {
    use crate::mil_ranges::MILITARY;
    let i = MILITARY.partition_point(|r| r.1 < addr);
    i < MILITARY.len() && MILITARY[i].0 <= addr
}

fn military_entry() -> Arc<Entry> {
    static E: std::sync::OnceLock<Arc<Entry>> = std::sync::OnceLock::new();
    E.get_or_init(|| Arc::new(Entry { flags: 1, ..Default::default() })).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup() {
        let db = Db::parse(b"300068;I-BIXQ;A321;00;AIRBUS A-321;;;\n43C6F2;ZZ336;A332;10;AIRBUS A-330-200;2011;Royal Air Force\n000001;;;;;;\n".to_vec());
        assert_eq!(db.len(), 3);
        let e = db.get(0x43C6F2).unwrap();
        assert_eq!((e.reg.as_str(), e.typ.as_str(), e.flags, e.year.as_str(), e.own_op.as_str()), ("ZZ336", "A332", 1, "2011", "Royal Air Force"));
        assert_eq!(db.get(0x300068).unwrap().desc, "AIRBUS A-321");
        assert!(db.get(0x123456).is_none());
    }

    /// Many blocks: every record comes back as written, wherever its
    /// line falls in its block.
    #[test]
    fn lookup_across_blocks() {
        let text: String = (0..20_000u32).map(|i| format!("{:06X};R-{i};T{};0{};desc {i};{};owner {i}\n", i * 7 + 1, i % 50, i % 2, 1950 + i % 70)).collect();
        let db = Db::parse(text.into_bytes());
        assert!(db.len() == 20_000 && db.blocks.len() > 20);
        for i in (0..20_000u32).step_by(37).chain([0, 19_999]) {
            let e = db.get(i * 7 + 1).unwrap();
            assert_eq!((e.reg.clone(), e.typ.clone(), e.flags, e.desc.clone(), e.own_op.clone()), (format!("R-{i}"), format!("T{}", i % 50), (i % 2) << 1, format!("desc {i}"), format!("owner {i}")));
            assert!(db.get(i * 7 + 2).is_none());
        }
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    #[test]
    fn military_blocks() {
        assert!(military(0xAE1234) && military(0x43C6F2) && military(0x3F5000) && military(0xAFFFFF));
        assert!(!military(0x4CA2D6) && !military(0xA00000) && !military(0) && !military(0xFFFFFF));
        // the table is sorted and its ranges do not overlap
        let t = crate::mil_ranges::MILITARY;
        assert!(t.windows(2).all(|w| w[0].1 < w[1].0) && t.iter().all(|r| r.0 <= r.1));
    }
}
