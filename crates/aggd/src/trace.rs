//! Per-aircraft position history, kept compact in memory and rendered
//! to tar1090's trace JSON on request.

use crate::track::{Aircraft, Ms};
use modes::AddrType;

#[derive(Clone, Copy)]
pub struct Point {
    /// milliseconds since the Unix epoch
    pub t: Ms,
    /// degrees * 1e6
    pub lat: i32,
    pub lon: i32,
    /// feet; i32::MIN unknown, i32::MIN+1 on the ground
    pub alt: i32,
    pub alt_geom: i32,
    /// knots * 10; u16::MAX unknown
    pub gs: u16,
    /// degrees * 10; u16::MAX unknown
    pub track: u16,
    /// ft/min; i16::MIN unknown
    pub rate: i16,
    pub geom_rate: i16,
    pub ias: u16,
    pub flags: u8,
    pub addr_type: u8,
}

pub const NONE: i32 = i32::MIN;
pub const GROUND: i32 = i32::MIN + 1;
pub const FLAG_STALE: u8 = 1;
pub const FLAG_LEG: u8 = 2;
pub const FLAG_RATE_GEOM: u8 = 4;
pub const FLAG_ALT_GEOM: u8 = 8;

/// What else was known about the aircraft at a point: kept for one
/// point a minute, so a history view can say who it was and what it
/// was doing. Absent values are the type's maximum (or -1).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Detail {
    /// time of the point it belongs to
    pub t: Ms,
    pub callsign: [u8; 8],
    pub squawk: u16,
    pub category: u8,
    pub emergency: u8,
    pub nav_alt_mcp: u32,
    pub nav_qnh: f32,
    pub nav_heading: f32,
    pub nav_modes: u8,
    pub nic: u8,
    pub rc: u16,
    pub version: i8,
    pub nic_baro: i8,
    pub nac_p: i8,
    pub nac_v: i8,
    pub sil: i8,
    pub sil_type: u8,
    pub gva: i8,
    pub sda: i8,
    pub alert: i8,
    pub spi: i8,
    /// explicit padding, always zero, so the struct has no hidden bytes
    pub pad1: [u8; 2],
    pub track: f32,
    pub tas: u16,
    pub pad2: [u8; 2],
    pub mach: f32,
    pub roll: f32,
    pub mag_heading: f32,
    pub true_heading: f32,
}

/// A point as it is kept in memory: 24 bytes. Time is in tenths of a
/// second, altitude in 25 ft steps, vertical rates in 64 ft/min steps
/// (what the transponder sends), airspeed in 2 kt steps.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct Packed {
    t: u32,
    lat: i32,
    lon: i32,
    alt: i16,
    gs: u16,
    track: u16,
    geom_delta: i8,
    rate: i8,
    geom_rate: i8,
    ias: u8,
    flags: u8,
    addr_type: u8,
}

/// 2020-01-01T00:00:00Z, the origin of packed times.
const T0: Ms = 1_577_836_800_000;

impl Packed {
    #[inline]
    pub fn t(&self) -> Ms {
        T0 + self.t as u64 * 100
    }

    pub fn pack(p: &Point) -> Packed {
        let mut flags = p.flags & !FLAG_ALT_GEOM;
        // without a barometric altitude the geometric one takes its place
        let (alt_ft, geom_delta) = match (p.alt, p.alt_geom) {
            (NONE, NONE) => (NONE, i8::MIN),
            (NONE, g) => {
                flags |= FLAG_ALT_GEOM;
                (g, i8::MIN)
            }
            (GROUND, _) => (GROUND, i8::MIN),
            (a, NONE) => (a, i8::MIN),
            (a, g) => (a, ((g - a) / 25).clamp(-127, 127) as i8),
        };
        let alt = match alt_ft {
            NONE => i16::MIN,
            GROUND => i16::MIN + 1,
            a => (a / 25).clamp(i16::MIN as i32 + 2, i16::MAX as i32) as i16,
        };
        let rate8 = |r: i16| if r == i16::MIN { i8::MIN } else { (r / 64).clamp(-127, 127) as i8 };
        Packed {
            t: (p.t.saturating_sub(T0) / 100) as u32,
            lat: p.lat,
            lon: p.lon,
            alt,
            gs: p.gs,
            track: if p.track == u16::MAX { u16::MAX } else { p.track.min(3600) },
            geom_delta,
            rate: rate8(p.rate),
            geom_rate: rate8(p.geom_rate),
            ias: if p.ias == u16::MAX { u8::MAX } else { (p.ias / 2).min(254) as u8 },
            flags,
            addr_type: p.addr_type,
        }
    }

    pub fn unpack(&self) -> Point {
        let alt = match self.alt {
            i16::MIN => NONE,
            v if v == i16::MIN + 1 => GROUND,
            v => v as i32 * 25,
        };
        let geom_alt = self.flags & FLAG_ALT_GEOM != 0;
        let rate16 = |r: i8| if r == i8::MIN { i16::MIN } else { r as i16 * 64 };
        Point {
            t: self.t(),
            lat: self.lat,
            lon: self.lon,
            alt: if geom_alt { NONE } else { alt },
            alt_geom: if geom_alt {
                alt
            } else if self.geom_delta == i8::MIN || alt <= GROUND {
                NONE
            } else {
                alt + self.geom_delta as i32 * 25
            },
            gs: self.gs,
            track: self.track,
            rate: rate16(self.rate),
            geom_rate: rate16(self.geom_rate),
            ias: if self.ias == u8::MAX { u16::MAX } else { self.ias as u16 * 2 },
            flags: self.flags & !FLAG_ALT_GEOM,
            addr_type: self.addr_type,
        }
    }
}

/// A trace is its older part, compressed in chunks, followed by its
/// newest points as they are. An aircraft in flight keeps a few dozen
/// points open and packs the rest away as it goes; one that has gone
/// silent keeps none open. A chunk once written is never rewritten, only
/// dropped whole when it has aged out: the table is then made of buffers
/// that live a day, with no reshuffling to leave holes in the heap.
#[derive(Default)]
pub struct Trace {
    /// the newest points, after those in `cold`
    pub points: Vec<Packed>,
    /// the details belonging to `points` (and possibly a few older)
    pub details: Vec<Detail>,
    /// points before this index, counted over the whole trace, are
    /// already in a history file
    pub written: usize,
    /// oldest first
    cold: Vec<Cold>,
    /// the last point and detail packed away, for when nothing is open
    tail: Option<Box<(Packed, Option<Detail>)>>,
}

struct Cold {
    z: Box<[u8]>,
    points: u32,
    details: u32,
    /// times of the first and last points
    first: Ms,
    last: Ms,
}

const PACKED: usize = std::mem::size_of::<Packed>();
const DETAIL: usize = std::mem::size_of::<Detail>();
// neither struct may have hidden padding: their bytes are read as they are
const _: () = assert!(PACKED == 24 && DETAIL == 72 && std::mem::align_of::<Detail>() == 8);

/// Open points at which the older ones are packed away, and how many stay.
const OPEN_MAX: usize = 128;
const OPEN_KEEP: usize = 32;

/// The bytes of `v`. Sound for `Packed` and `Detail` only: plain numbers,
/// `repr(C)`, no padding.
fn bytes_of<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn from_bytes<T: Copy>(b: &[u8]) -> Vec<T> {
    let n = b.len() / std::mem::size_of::<T>();
    let mut v = Vec::<T>::with_capacity(n);
    unsafe {
        std::ptr::copy_nonoverlapping(b.as_ptr(), v.as_mut_ptr() as *mut u8, n * std::mem::size_of::<T>());
        v.set_len(n);
    }
    v
}

/// Successive points of a flight are nearly alike: each is stored as its
/// difference from the one before, which is what lets them compress.
/// (Storing time and place as the change in that difference was tried on
/// a day of real traces and gained nothing: points are not evenly spaced.)
fn delta(points: &[Packed]) -> Vec<Packed> {
    let mut prev = Packed::default();
    points
        .iter()
        .map(|p| {
            let d = Packed {
                t: p.t.wrapping_sub(prev.t),
                lat: p.lat.wrapping_sub(prev.lat),
                lon: p.lon.wrapping_sub(prev.lon),
                alt: p.alt.wrapping_sub(prev.alt),
                gs: p.gs.wrapping_sub(prev.gs),
                track: p.track.wrapping_sub(prev.track),
                ..*p
            };
            prev = *p;
            d
        })
        .collect()
}

fn undelta(points: &mut [Packed]) {
    let mut prev = Packed::default();
    for p in points {
        p.t = p.t.wrapping_add(prev.t);
        p.lat = p.lat.wrapping_add(prev.lat);
        p.lon = p.lon.wrapping_add(prev.lon);
        p.alt = p.alt.wrapping_add(prev.alt);
        p.gs = p.gs.wrapping_add(prev.gs);
        p.track = p.track.wrapping_add(prev.track);
        prev = *p;
    }
}

/// Lay records of `stride` bytes out by column; with `diff`, each byte
/// as its difference from the same byte of the record before.
fn columns(b: &[u8], stride: usize, diff: bool, out: &mut Vec<u8>) {
    let n = b.len() / stride;
    for c in 0..stride {
        let mut prev = 0u8;
        for r in 0..n {
            let v = b[r * stride + c];
            out.push(v.wrapping_sub(prev));
            if diff {
                prev = v;
            }
        }
    }
}

fn rows(cols: &[u8], stride: usize, diff: bool) -> Vec<u8> {
    let n = cols.len() / stride;
    let mut out = vec![0u8; n * stride];
    for c in 0..stride {
        let mut prev = 0u8;
        for r in 0..n {
            let v = prev.wrapping_add(cols[c * n + r]);
            out[r * stride + c] = v;
            if diff {
                prev = v;
            }
        }
    }
    out
}

impl Cold {
    fn pack(points: &[Packed], details: &[Detail]) -> Option<Cold> {
        let last = points.last()?.t();
        let mut raw = Vec::with_capacity(points.len() * PACKED + details.len() * DETAIL);
        columns(bytes_of(&delta(points)), PACKED, false, &mut raw);
        columns(bytes_of(details), DETAIL, true, &mut raw);
        let z = zstd::bulk::compress(&raw, 3).ok()?;
        Some(Cold { z: z.into_boxed_slice(), points: points.len() as u32, details: details.len() as u32, first: points[0].t(), last })
    }

    /// (our own bytes, so this cannot fail short of memory corruption;
    /// if it did the chunk would read as empty)
    fn unpack(&self, p: &mut Vec<Packed>, d: &mut Vec<Detail>) {
        let (np, nd) = (self.points as usize * PACKED, self.details as usize * DETAIL);
        let Some(raw) = zstd::bulk::decompress(&self.z, np + nd).ok().filter(|r| r.len() == np + nd) else { return };
        let mut points: Vec<Packed> = from_bytes(&rows(&raw[..np], PACKED, false));
        undelta(&mut points);
        p.append(&mut points);
        d.append(&mut from_bytes(&rows(&raw[np..], DETAIL, true)));
    }
}

impl Trace {
    /// A trace of `points`, all packed away.
    pub fn restored(points: &[Packed], written: usize) -> Trace {
        let mut t = Trace { written, ..Default::default() };
        for c in points.chunks(OPEN_MAX - OPEN_KEEP) {
            t.points.extend_from_slice(c);
            t.compact(0);
        }
        t
    }

    pub fn len(&self) -> usize {
        self.cold.iter().map(|c| c.points as usize).sum::<usize>() + self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cold.is_empty() && self.points.is_empty()
    }

    /// Time of the oldest point.
    pub fn first_t(&self) -> Option<Ms> {
        self.cold.first().map(|c| c.first).or_else(|| self.points.first().map(|p| p.t()))
    }

    pub fn last_point(&self) -> Option<Packed> {
        self.points.last().copied().or_else(|| self.tail.as_ref().map(|t| t.0))
    }

    pub fn last_detail(&self) -> Option<Detail> {
        self.details.last().copied().or_else(|| self.tail.as_ref().and_then(|t| t.1))
    }

    /// Bytes of memory the trace holds.
    pub fn bytes(&self) -> usize {
        self.points.capacity() * PACKED
            + self.details.capacity() * DETAIL
            + self.cold.capacity() * std::mem::size_of::<Cold>()
            + self.cold.iter().map(|c| c.z.len()).sum::<usize>()
            + self.tail.as_ref().map_or(0, |t| std::mem::size_of_val(&**t))
    }

    /// The whole trace, without changing it.
    pub fn all(&self) -> (std::borrow::Cow<'_, [Packed]>, std::borrow::Cow<'_, [Detail]>) {
        use std::borrow::Cow;
        if self.cold.is_empty() {
            return (Cow::Borrowed(&self.points), Cow::Borrowed(&self.details));
        }
        let (mut p, mut d) = (Vec::with_capacity(self.len()), Vec::new());
        for c in &self.cold {
            c.unpack(&mut p, &mut d);
        }
        p.extend_from_slice(&self.points);
        d.extend_from_slice(&self.details);
        (Cow::Owned(p), Cow::Owned(d))
    }

    /// Pack away all but the newest `keep` points, as one more chunk.
    pub fn compact(&mut self, keep: usize) {
        if self.points.len() <= keep {
            return;
        }
        let n = self.points.len() - keep;
        // details go with their points
        let nd = match self.points.get(n) {
            Some(first_open) => self.details.partition_point(|x| x.t < first_open.t()),
            None => self.details.len(),
        };
        let Some(cold) = Cold::pack(&self.points[..n], &self.details[..nd]) else { return };
        let last_detail = self.details[..nd].last().copied().or_else(|| self.tail.as_ref().and_then(|t| t.1));
        let tail = (self.points[n - 1], last_detail);
        match &mut self.tail {
            Some(t) => **t = tail,
            None => self.tail = Some(Box::new(tail)),
        }
        self.cold.push(cold);
        if keep == 0 {
            self.cold.shrink_to_fit();
            self.points = Vec::new();
            self.details = Vec::new();
        } else {
            // (in place: the open buffers keep their size while in flight)
            self.points.drain(..n);
            self.details.drain(..nd);
        }
    }

    /// Pack away the older points of a trace that has grown long.
    pub fn tidy(&mut self) {
        if self.points.len() >= OPEN_MAX {
            self.compact(OPEN_KEEP);
        }
    }

    /// Drop what is older than `cut`: whole chunks, so up to one chunk of
    /// older points may stay a while longer. Returns how many points went.
    pub fn trim_before(&mut self, cut: Ms) -> usize {
        let chunks = self.cold.iter().take_while(|c| c.last < cut).count();
        let mut n: usize = self.cold.drain(..chunks).map(|c| c.points as usize).sum();
        if self.cold.is_empty() {
            let open = self.points.partition_point(|p| p.t() < cut);
            if open > 0 {
                let first = self.points.get(open).map_or(Ms::MAX, |p| p.t());
                let nd = self.details.partition_point(|d| d.t < first);
                self.details.drain(..nd);
                self.points.drain(..open);
                n += open;
            }
            if self.points.is_empty() {
                self.tail = None;
            }
        }
        self.written = self.written.saturating_sub(n);
        n
    }
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

/// Record the aircraft's new position if it adds information: a regular
/// sample every `interval`, sooner when the aircraft turns, changes
/// altitude or changes between ground and air.
pub fn add(a: &mut Aircraft, now: Ms, interval: Ms) {
    let (lat, lon) = a.pos.v;
    let ground = a.on_ground(now);
    let alt = if ground { GROUND } else { a.alt_baro.get(now).unwrap_or(NONE) };
    let track = a.track.get(now);
    let mut flags = 0;
    if let Some(last) = a.trace.last_point().map(|p| p.unpack()) {
        let dt = now.saturating_sub(last.t);
        if dt < 1000 {
            return;
        }
        let turned = match (track, last.track) {
            (Some(t), l) if l != u16::MAX => angle_diff(t, l as f32 / 10.0) > 2.0,
            _ => false,
        };
        let moved_alt = alt != last.alt && (alt <= GROUND || last.alt <= GROUND || (alt - last.alt).abs() >= 200);
        if dt < interval && !turned && !moved_alt {
            return;
        }
        if dt > 300_000 {
            flags |= FLAG_STALE;
        }
        if dt > 1_800_000 || (last.alt == GROUND && alt != GROUND && dt > 600_000) {
            flags |= FLAG_LEG;
        }
    }
    let (rate, rflag) = match (a.baro_rate.get(now), a.geom_rate.get(now)) {
        (Some(r), _) => (r.clamp(-32000, 32000) as i16, 0),
        (None, Some(r)) => (r.clamp(-32000, 32000) as i16, FLAG_RATE_GEOM),
        _ => (i16::MIN, 0),
    };
    let first_or_leg = a.trace.is_empty() || flags & FLAG_LEG != 0;
    let last_detail = a.trace.last_detail();
    let due = last_detail.map_or(true, |d| now.saturating_sub(d.t) >= 60_000);
    if first_or_leg || due {
        let b = |f: &crate::track::F<bool>| f.get(now).map_or(-1, |v| v as i8);
        let n = |f: &crate::track::F<u8>| f.get(now).map_or(-1, |v| v as i8);
        let d = Detail {
            t: now,
            callsign: a.callsign.get(now).unwrap_or([0; 8]),
            squawk: a.squawk.get(now).unwrap_or(u16::MAX),
            category: a.category,
            emergency: a.emergency.get(now).unwrap_or(u8::MAX),
            nav_alt_mcp: a.nav_alt_mcp.get(now).unwrap_or(u32::MAX),
            nav_qnh: a.nav_qnh.get(now).unwrap_or(f32::NAN),
            nav_heading: a.nav_heading.get(now).unwrap_or(f32::NAN),
            nav_modes: a.nav_modes.get(now).unwrap_or(u8::MAX),
            nic: a.nic,
            rc: a.rc.min(65535) as u16,
            version: a.version,
            nic_baro: b(&a.nic_baro),
            nac_p: n(&a.nac_p),
            nac_v: n(&a.nac_v),
            sil: n(&a.sil),
            sil_type: a.sil_type,
            gva: n(&a.gva),
            sda: n(&a.sda),
            alert: b(&a.alert),
            spi: b(&a.spi),
            pad1: [0; 2],
            pad2: [0; 2],
            track: track.unwrap_or(f32::NAN),
            tas: a.tas.get(now).unwrap_or(u16::MAX),
            mach: a.mach.get(now).unwrap_or(f32::NAN),
            roll: a.roll.get(now).unwrap_or(f32::NAN),
            mag_heading: a.mag_heading.get(now).unwrap_or(f32::NAN),
            true_heading: a.true_heading.get(now).unwrap_or(f32::NAN),
        };
        // steady flight says the same thing every minute: keep what
        // changed, and one in ten otherwise
        let news = last_detail.map_or(true, |l| {
            now.saturating_sub(l.t) >= 600_000
                || (l.callsign, l.squawk, l.emergency, l.nav_alt_mcp, l.nav_modes, l.version, l.nac_p, l.sil, l.alert, l.spi)
                    != (d.callsign, d.squawk, d.emergency, d.nav_alt_mcp, d.nav_modes, d.version, d.nac_p, d.sil, d.alert, d.spi)
        });
        if first_or_leg || news {
            a.trace.details.push(d);
        }
    }
    let point = Point {
        t: now,
        lat: (lat * 1e6).round() as i32,
        lon: (lon * 1e6).round() as i32,
        alt,
        alt_geom: a.alt_geom.get(now).unwrap_or(NONE),
        gs: a.gs.get(now).map_or(u16::MAX, |g| (g * 10.0).round().min(65000.0) as u16),
        track: track.map_or(u16::MAX, |t| (t * 10.0).round() as u16),
        rate,
        geom_rate: a.geom_rate.get(now).map_or(i16::MIN, |r| r.clamp(-32000, 32000) as i16),
        ias: a.ias.get(now).unwrap_or(u16::MAX),
        flags: flags | rflag,
        addr_type: a.addr_type as u8,
    };
    // (the open buffer grows to its limit and no further: doubling would
    // take it to twice that on every long flight)
    let open = &mut a.trace.points;
    if open.len() == open.capacity() && open.len() >= OPEN_MAX / 2 {
        open.reserve_exact(OPEN_MAX - open.len());
    }
    open.push(Packed::pack(&point));
    a.trace.tidy();
}

fn addr_type_str(v: u8) -> &'static str {
    const ALL: [AddrType; 14] = [
        AddrType::AdsbIcao,
        AddrType::AdsbIcaoNt,
        AddrType::AdsrIcao,
        AddrType::TisbIcao,
        AddrType::Jaero,
        AddrType::Mlat,
        AddrType::Other,
        AddrType::ModeS,
        AddrType::AdsbOther,
        AddrType::AdsrOther,
        AddrType::TisbTrackfile,
        AddrType::TisbOther,
        AddrType::ModeA,
        AddrType::Unknown,
    ];
    ALL.get(v as usize).map_or("unknown", |t| t.as_str())
}

fn detail_json(o: &mut String, d: &Detail, p: &Point) {
    use std::fmt::Write;
    let _ = write!(o, "{{\"type\":\"{}\"", addr_type_str(p.addr_type));
    if d.callsign[0] != 0 {
        let _ = write!(o, ",\"flight\":\"{}\"", crate::json::esc(std::str::from_utf8(&d.callsign).unwrap_or("")));
    }
    if p.alt_geom != NONE {
        let _ = write!(o, ",\"alt_geom\":{}", p.alt_geom);
    }
    if d.tas != u16::MAX {
        let _ = write!(o, ",\"tas\":{}", d.tas);
    }
    if !d.mach.is_nan() {
        let _ = write!(o, ",\"mach\":{:.3}", d.mach);
    }
    if !d.track.is_nan() {
        let _ = write!(o, ",\"track\":{:.2}", d.track);
    }
    if !d.roll.is_nan() {
        let _ = write!(o, ",\"roll\":{:.2}", d.roll);
    }
    if !d.mag_heading.is_nan() {
        let _ = write!(o, ",\"mag_heading\":{:.2}", d.mag_heading);
    }
    if !d.true_heading.is_nan() {
        let _ = write!(o, ",\"true_heading\":{:.2}", d.true_heading);
    }
    if p.rate != i16::MIN && p.flags & FLAG_RATE_GEOM == 0 {
        let _ = write!(o, ",\"baro_rate\":{}", p.rate);
    }
    if p.geom_rate != i16::MIN {
        let _ = write!(o, ",\"geom_rate\":{}", p.geom_rate);
    }
    if d.squawk != u16::MAX {
        let _ = write!(o, ",\"squawk\":\"{:04x}\"", d.squawk);
    }
    if d.emergency != u8::MAX {
        let _ = write!(o, ",\"emergency\":\"{}\"", crate::json::EMERGENCY[d.emergency as usize & 7]);
    }
    if d.category != 0 {
        let _ = write!(o, ",\"category\":\"{:02X}\"", d.category);
    }
    if !d.nav_qnh.is_nan() {
        let _ = write!(o, ",\"nav_qnh\":{:.1}", d.nav_qnh);
    }
    if d.nav_alt_mcp != u32::MAX {
        let _ = write!(o, ",\"nav_altitude_mcp\":{}", d.nav_alt_mcp);
    }
    if !d.nav_heading.is_nan() {
        let _ = write!(o, ",\"nav_heading\":{:.2}", d.nav_heading);
    }
    if d.nav_modes != u8::MAX {
        o.push_str(",\"nav_modes\":");
        crate::json::nav_modes(o, d.nav_modes);
    }
    let _ = write!(o, ",\"nic\":{},\"rc\":{}", d.nic, d.rc);
    let mut num = |name: &str, v: i8| {
        if v >= 0 {
            let _ = write!(o, ",\"{name}\":{v}");
        }
    };
    num("version", d.version);
    num("nic_baro", d.nic_baro);
    num("nac_p", d.nac_p);
    num("nac_v", d.nac_v);
    num("sil", d.sil);
    if d.sil_type < 3 {
        let _ = write!(o, ",\"sil_type\":\"{}\"", ["unknown", "perhour", "persample"][d.sil_type as usize]);
    }
    let mut num = |name: &str, v: i8| {
        if v >= 0 {
            let _ = write!(o, ",\"{name}\":{v}");
        }
    };
    num("gva", d.gva);
    num("sda", d.sda);
    num("alert", d.alert);
    num("spi", d.spi);
    o.push('}');
}

/// tar1090's trace file for `points`, a slice of the aircraft's trace.
pub fn render(addr: u32, db: Option<&crate::db::Entry>, points: &[Packed], details: &[Detail], out: &mut String) {
    use std::fmt::Write;
    let mut di = details.partition_point(|d| points.first().map_or(true, |p| d.t < p.t()));
    let _ = write!(out, "{{\"icao\":\"{}\",\n", crate::json::hex(addr));
    if let Some(d) = db {
        if !d.reg.is_empty() {
            let _ = write!(out, "\"r\":\"{}\",\n", crate::json::esc(&d.reg));
        }
        if !d.typ.is_empty() {
            let _ = write!(out, "\"t\":\"{}\",\n", crate::json::esc(&d.typ));
        }
        if d.flags != 0 {
            let _ = write!(out, "\"dbFlags\":{},\n", d.flags);
        }
        if !d.desc.is_empty() {
            let _ = write!(out, "\"desc\":\"{}\",\n", crate::json::esc(&d.desc));
        }
        if !d.own_op.is_empty() {
            let _ = write!(out, "\"ownOp\":\"{}\",\n", crate::json::esc(&d.own_op));
        }
        if !d.year.is_empty() {
            let _ = write!(out, "\"year\":\"{}\",\n", crate::json::esc(&d.year));
        }
    }
    let base = points.first().map_or(0, |p| p.t());
    let _ = write!(out, "\"version\": \"aggd {}\",\n\"timestamp\": {:.3},\n\"trace\":[ \n", env!("CARGO_PKG_VERSION"), base as f64 / 1000.0);
    for (i, p) in points.iter().enumerate() {
        let p = &p.unpack();
        let _ = write!(
            out,
            "[{:.2},{:.6},{:.6},",
            (p.t - base) as f64 / 1000.0,
            p.lat as f64 / 1e6,
            p.lon as f64 / 1e6
        );
        match p.alt {
            NONE => out.push_str("null"),
            GROUND => out.push_str("\"ground\""),
            v => {
                let _ = write!(out, "{v}");
            }
        }
        if p.gs == u16::MAX {
            out.push_str(",null");
        } else {
            let _ = write!(out, ",{:.1}", p.gs as f64 / 10.0);
        }
        if p.track == u16::MAX {
            out.push_str(",null");
        } else {
            let _ = write!(out, ",{:.1}", p.track as f64 / 10.0);
        }
        let _ = write!(out, ",{}", p.flags);
        if p.rate == i16::MIN {
            out.push_str(",null");
        } else {
            let _ = write!(out, ",{}", p.rate);
        }
        // the detail recorded with this point, if there is one
        while di < details.len() && details[di].t < p.t {
            di += 1;
        }
        if di < details.len() && details[di].t == p.t {
            out.push(',');
            detail_json(out, &details[di], p);
            di += 1;
        } else {
            out.push_str(",null");
        }
        let _ = write!(out, ",\"{}\"", addr_type_str(p.addr_type));
        if p.alt_geom == NONE {
            out.push_str(",null");
        } else {
            let _ = write!(out, ",{}", p.alt_geom);
        }
        if p.geom_rate == i16::MIN {
            out.push_str(",null");
        } else {
            let _ = write!(out, ",{}", p.geom_rate);
        }
        if p.ias == u16::MAX {
            out.push_str(",null");
        } else {
            let _ = write!(out, ",{}", p.ias);
        }
        out.push_str(",null]");
        if i + 1 < points.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("]}\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_survive_packing() {
        assert_eq!(std::mem::size_of::<Packed>(), 24);
        let p = Point { t: 1_794_831_525_100, lat: 44_938_272, lon: -18_733_297, alt: 35_025, alt_geom: 35_600, gs: 4321, track: 3176, rate: -1280, geom_rate: 640, ias: 281, flags: FLAG_LEG, addr_type: 5 };
        let q = Packed::pack(&p).unpack();
        assert_eq!((q.t, q.lat, q.lon, q.alt, q.alt_geom, q.gs, q.track, q.rate, q.geom_rate, q.flags, q.addr_type), (p.t, p.lat, p.lon, 35_025, 35_600, 4321, 3176, -1280, 640, FLAG_LEG, 5));
        assert_eq!(q.ias, 280);
        // nothing known, on the ground, and geometric altitude only
        let e = Point { alt: NONE, alt_geom: NONE, gs: u16::MAX, track: u16::MAX, rate: i16::MIN, geom_rate: i16::MIN, ias: u16::MAX, ..p };
        let q = Packed::pack(&e).unpack();
        assert_eq!((q.alt, q.alt_geom, q.gs, q.track, q.rate, q.geom_rate, q.ias), (NONE, NONE, u16::MAX, u16::MAX, i16::MIN, i16::MIN, u16::MAX));
        assert_eq!(Packed::pack(&Point { alt: GROUND, ..p }).unpack().alt, GROUND);
        let q = Packed::pack(&Point { alt: NONE, alt_geom: 12_000, ..p }).unpack();
        assert_eq!((q.alt, q.alt_geom), (NONE, 12_000));
    }
}

#[cfg(test)]
mod cold_tests {
    use super::*;

    fn point(i: u32) -> Packed {
        Packed { t: 1000 + i * 200, lat: 45_000_000 + i as i32 * 913, lon: 9_000_000 - i as i32 * 1291, alt: (i % 90) as i16 - 40, gs: 4400 + (i % 7) as u16, track: (i * 3 % 3600) as u16, geom_delta: -3, rate: (i % 5) as i8 - 2, geom_rate: i8::MIN, ias: 140, flags: (i % 3) as u8, addr_type: 0 }
    }

    fn detail(i: u32) -> Detail {
        let mut d: Detail = unsafe { std::mem::zeroed() };
        d.t = point(i).t();
        d.callsign = *b"AZA1234 ";
        d.nav_qnh = f32::NAN;
        d.mach = 0.78 + i as f32 / 1e4;
        d.sil = -1;
        d
    }

    /// Built a point at a time, as in flight, with the older part packed
    /// away along the way: the trace read back is the trace put in.
    #[test]
    fn a_trace_packed_away_reads_back_the_same() {
        let (mut t, mut plain) = (Trace::default(), Trace::default());
        for i in 0..1000u32 {
            for x in [&mut t, &mut plain] {
                x.points.push(point(i));
                if i % 6 == 0 {
                    x.details.push(detail(i));
                }
            }
            t.tidy();
            assert!(t.points.len() < OPEN_MAX);
            assert_eq!(bytes_of(&[t.last_point().unwrap()]), bytes_of(&[point(i)]));
            assert_eq!(t.last_detail().unwrap().t, detail(i - i % 6).t);
        }
        assert_eq!((t.len(), t.first_t()), (1000, Some(point(0).t())));
        let (p, d) = t.all();
        assert_eq!((bytes_of(&p), bytes_of(&d)), (bytes_of(&plain.points), bytes_of(&plain.details)));
        // silent: nothing stays open, and it is a fraction of the size
        t.compact(0);
        assert!(t.points.is_empty() && t.details.is_empty() && t.len() == 1000);
        assert!(t.bytes() * 3 < plain.bytes(), "{} of {}", t.bytes(), plain.bytes());
        assert_eq!(bytes_of(&[t.last_point().unwrap()]), bytes_of(&[point(999)]));
        assert_eq!(t.last_detail().unwrap().t, detail(996).t);
        let (p, d) = t.all();
        assert_eq!((bytes_of(&p), bytes_of(&d)), (bytes_of(&plain.points), bytes_of(&plain.details)));
        // the same trace restored from a file reads the same
        let r = Trace::restored(&plain.points, 5);
        assert_eq!((bytes_of(&r.all().0), r.written, r.points.len()), (bytes_of(&plain.points), 5, 0));
        // the aircraft returns: new points follow the packed ones
        t.points.push(point(1000));
        assert_eq!((t.len(), t.all().0.len()), (1001, 1001));
        // the day moves on: old chunks go whole, with their details, and
        // nothing newer than the cut goes
        t.written = 700;
        let cut = point(300).t();
        let went = t.trim_before(cut);
        assert!(went > 100 && went <= 300, "{went}");
        assert_eq!((t.len(), t.written, t.first_t()), (1001 - went, 700 - went, Some(point(went as u32).t())));
        let (p, d) = t.all();
        assert_eq!(bytes_of(&p), bytes_of(&plain.points[went..].iter().copied().chain([point(1000)]).collect::<Vec<_>>()));
        assert!(d[0].t >= p[0].t() && d.len() == plain.details.iter().filter(|x| x.t >= p[0].t()).count());
        // and when all of it is old, all of it goes
        assert_eq!(t.trim_before(point(2000).t()), 1001 - went);
        assert!(t.is_empty() && t.last_point().is_none() && t.written == 0);
        // nothing to pack: nothing happens
        let mut e = Trace::default();
        e.compact(0);
        assert!(e.is_empty() && e.first_t().is_none() && e.last_point().is_none() && e.trim_before(5) == 0);
    }
}
