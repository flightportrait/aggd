//! A synthetic sky heard by a synthetic network: aircraft fly, receivers
//! within range hear them, and each receiver feeds an aggregator over
//! its own Beast connection the way a readsb feeder does
//! (beast_reduce_plus_out: UUID hello, then rate-limited frames).

use modes::encode as enc;
use std::io::Write;
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
}

pub struct Plane {
    pub addr: u32,
    pub callsign: [u8; 8],
    pub category: u8,
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
    pub gs: f64,
    pub track: f64,
    pub vrate: f64,
    pub squawk: u16,
    pub ground: bool,
    pub sel_alt: u32,
    /// phase offset so the fleet does not transmit in lockstep
    phase: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Clean,
    /// frames that fail their checksum
    Garbage,
    /// real aircraft, positions a few degrees off
    Liar,
    /// valid frames for aircraft that do not exist, far above any real rate
    Flood,
}

pub struct Receiver {
    pub kind: Kind,
    pub uuid: String,
    pub lat: f64,
    pub lon: f64,
    /// degrees a liar adds to every latitude; each lies differently
    offset: f64,
    /// this receiver's path to the aggregator takes this long, ms
    delay_ms: u64,
    /// frames on their way: (time they arrive, bytes)
    in_flight: std::collections::VecDeque<(u64, Vec<u8>)>,
    socks: Vec<Option<TcpStream>>,
    buf: Vec<u8>,
    /// last time (ms) this receiver forwarded each (plane, kind)
    last: Vec<[u64; KINDS]>,
    next_connect: Instant,
}

const KINDS: usize = 8;
/// Minimum interval per message kind on a feeder link, ms: what a
/// reducing feeder lets through.
const INTERVAL: [u64; KINDS] = [500, 500, 5000, 2500, 2500, 5000, 5000, 1000];

pub struct Opts {
    pub targets: Vec<String>,
    pub planes: usize,
    pub receivers: usize,
    pub seed: u64,
    pub box_: (f64, f64, f64, f64),
    pub range_nm: f64,
    pub seconds: u64,
    /// misbehaving receivers, which connect to the first target only
    pub garbage: usize,
    pub liars: usize,
    pub flood: usize,
    /// each receiver's frames arrive up to this many ms late (a fixed
    /// delay per receiver, plus a little per batch)
    /// the liars all tell the same lie
    pub collude: bool,
    pub hostile_from: Option<String>,
    pub jitter_ms: u64,
    /// fraction of frames a receiver fails to hear
    pub loss: f64,
}

fn dist_nm(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1) * 60.0;
    let dlon = (lon2 - lon1) * 60.0 * ((lat1 + lat2) / 2.0).to_radians().cos();
    (dlat * dlat + dlon * dlon).sqrt()
}

pub fn beast(frame: &[u8], ticks: u64, signal: u8, out: &mut Vec<u8>) {
    out.push(0x1a);
    out.push(if frame.len() == 7 { 0x32 } else { 0x33 });
    let t = ticks.to_be_bytes();
    for &b in t[2..8].iter().chain(std::iter::once(&signal)).chain(frame.iter()) {
        out.push(b);
        if b == 0x1a {
            out.push(0x1a);
        }
    }
}

pub fn hello(uuid: &str) -> Vec<u8> {
    let mut v = vec![0x1a, 0xE4];
    v.extend_from_slice(uuid.as_bytes());
    v.extend_from_slice(&[0x1a, b'W', b'O']);
    v
}

pub fn make(o: &Opts) -> (Vec<Plane>, Vec<Receiver>) {
    let mut r = Rng(o.seed | 1);
    let (la0, la1, lo0, lo1) = o.box_;
    let planes = (0..o.planes)
        .map(|i| {
            let ground = r.f() < 0.05;
            let mut cs = *b"        ";
            let n = format!("SKY{:04}", i % 10000);
            cs[..n.len()].copy_from_slice(n.as_bytes());
            let alt = if ground { 0.0 } else { (r.range(5000.0, 41000.0) / 25.0).round() * 25.0 };
            Plane {
                addr: 0x300000 + (i as u32).wrapping_mul(211) % 0x0F0000,
                callsign: cs,
                category: [0xA1, 0xA2, 0xA3, 0xA5, 0xA7][(r.next() % 5) as usize],
                lat: r.range(la0, la1),
                lon: r.range(lo0, lo1),
                alt,
                gs: if ground { r.range(0.0, 25.0) } else { r.range(220.0, 520.0) },
                track: r.range(0.0, 360.0),
                vrate: if ground || r.f() < 0.6 { 0.0 } else { (r.range(-2500.0, 2500.0) / 64.0).round() * 64.0 },
                squawk: {
                    let d = |r: &mut Rng| (r.next() % 8) as u16;
                    d(&mut r) << 12 | d(&mut r) << 8 | d(&mut r) << 4 | d(&mut r)
                },
                ground,
                sel_alt: ((alt as u32 + 2000) / 1000) * 1000,
                phase: r.next() % 1000,
            }
        })
        .collect();
    let receivers = (0..o.receivers + o.garbage + o.liars + o.flood)
        .map(|i| {
            let (a, b) = (r.next(), r.next());
            Receiver {
                kind: match i {
                    i if i < o.receivers => Kind::Clean,
                    i if i < o.receivers + o.garbage => Kind::Garbage,
                    i if i < o.receivers + o.garbage + o.liars => Kind::Liar,
                    _ => Kind::Flood,
                },
                uuid: format!(
                    "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                    a >> 32,
                    (a >> 16) & 0xffff,
                    a & 0xffff,
                    b >> 48,
                    b & 0xffff_ffff_ffff
                ),
                lat: r.range(la0, la1),
                lon: r.range(lo0, lo1),
                offset: if o.collude { 3.2 } else { 2.5 + 0.7 * (i % 5) as f64 },
                delay_ms: if o.jitter_ms > 0 { r.next() % (o.jitter_ms + 1) } else { 0 },
                in_flight: std::collections::VecDeque::new(),
                socks: o.targets.iter().map(|_| None).collect(),
                buf: Vec::new(),
                last: vec![[0; KINDS]; o.planes],
                next_connect: Instant::now(),
            }
        })
        .collect();
    (planes, receivers)
}

fn step(p: &mut Plane, dt: f64, b: (f64, f64, f64, f64)) {
    let d = p.gs * dt / 3600.0; // nm
    let t = p.track.to_radians();
    p.lat += d * t.cos() / 60.0;
    p.lon += d * t.sin() / (60.0 * p.lat.to_radians().cos());
    if p.lat < b.0 || p.lat > b.1 {
        p.track = (180.0 - p.track).rem_euclid(360.0);
        p.lat = p.lat.clamp(b.0, b.1);
    }
    if p.lon < b.2 || p.lon > b.3 {
        p.track = (360.0 - p.track).rem_euclid(360.0);
        p.lon = p.lon.clamp(b.2, b.3);
    }
    if !p.ground {
        p.alt += p.vrate * dt / 60.0;
        if p.alt > 43000.0 || p.alt < 3000.0 {
            p.vrate = -p.vrate;
            p.alt = p.alt.clamp(3000.0, 43000.0);
        }
    }
}

/// The frames one aircraft transmits in the tick ending at `now_ms`,
/// with the kind index used for feeder-side rate limiting.
fn transmissions(p: &Plane, now_ms: u64, tick_ms: u64, out: &mut Vec<(usize, Vec<u8>)>) {
    let due = |period: u64, offset: u64| {
        let t = now_ms + p.phase + offset;
        t / period != (t - tick_ms) / period
    };
    let alt = (p.alt / 25.0).round() as i32 * 25;
    if due(500, 0) {
        let odd = ((now_ms + p.phase) / 500) % 2 == 1;
        if p.ground {
            out.push((odd as usize, enc::surface_position(p.addr, p.lat, p.lon, p.gs as f32, p.track as f32, odd).to_vec()));
        } else {
            out.push((odd as usize, enc::airborne_position(p.addr, p.lat, p.lon, alt, odd, 11).to_vec()));
        }
    }
    if !p.ground && due(500, 250) {
        out.push((7, enc::velocity(p.addr, p.gs as f32, p.track as f32, p.vrate as i32, Some(175)).to_vec()));
    }
    if due(5000, 100) {
        out.push((2, enc::ident(p.addr, &p.callsign, p.category).to_vec()));
    }
    // (an aircraft on the ground sends the surface form, which the
    // generator does not make)
    if !p.ground && due(2500, 300) {
        out.push((3, enc::op_status(p.addr, 10, 3, 2, 2).to_vec()));
    }
    if !p.ground && due(1250, 400) {
        out.push((4, enc::target_state(p.addr, p.sel_alt, 1013.6, (p.track as f32 / 0.703125).round() * 0.703125, 0x13).to_vec()));
    }
    if due(5000, 700) {
        out.push((5, enc::emergency(p.addr, 0, p.squawk).to_vec()));
    }
    if due(4000, 50) {
        out.push((6, enc::df11(p.addr, p.ground).to_vec()));
    }
    if !p.ground && due(6000, 900) {
        out.push((6, enc::df4(p.addr, alt, false).to_vec()));
        out.push((6, enc::df5(p.addr, p.squawk).to_vec()));
    }
}

pub fn run(o: Opts) -> anyhow::Result<()> {
    let (mut planes, mut receivers) = make(&o);
    let start = Instant::now();
    let tick = Duration::from_millis(100);
    let mut now_ms: u64 = 1000;
    let mut tx = Vec::new();
    let (mut sent_frames, mut sent_bytes, mut last_report) = (0u64, 0u64, Instant::now());
    let mut noise = Rng(o.seed ^ 0x5DEECE66D);
    eprintln!("sky: {} aircraft, {} receivers -> {}", o.planes, o.receivers, o.targets.join(", "));
    for r in receivers.iter().filter(|r| r.kind == Kind::Liar) {
        // the receiver id an aggregator derives: the first 16 hex digits
        eprintln!("sky: liar {}", &r.uuid[..18]);
    }
    loop {
        let wake = start + Duration::from_millis(now_ms - 1000);
        if let Some(d) = wake.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        if o.seconds > 0 && now_ms > o.seconds * 1000 {
            return Ok(());
        }
        for (pi, p) in planes.iter_mut().enumerate() {
            step(p, 0.1, o.box_);
            tx.clear();
            transmissions(p, now_ms, 100, &mut tx);
            if tx.is_empty() {
                continue;
            }
            let reach = if p.ground { o.range_nm / 12.0 } else { o.range_nm };
            for r in receivers.iter_mut() {
                if matches!(r.kind, Kind::Garbage | Kind::Flood) {
                    continue;
                }
                let d = dist_nm(p.lat, p.lon, r.lat, r.lon);
                if d > reach {
                    continue;
                }
                // signal falls with distance; 12 MHz clock since start
                let signal = (200.0 / (1.0 + d / 20.0)) as u8 + 10;
                for (kind, f) in &tx {
                    if now_ms - r.last[pi][*kind] < INTERVAL[*kind] && r.last[pi][*kind] != 0 {
                        continue;
                    }
                    if o.loss > 0.0 && noise.f() < o.loss {
                        continue;
                    }
                    r.last[pi][*kind] = now_ms;
                    if r.kind == Kind::Liar && *kind < 2 && !p.ground {
                        let lie = enc::airborne_position(p.addr, p.lat + r.offset, p.lon, (p.alt / 25.0).round() as i32 * 25, *kind == 1, 11);
                        beast(&lie, now_ms * 12000, signal, &mut r.buf);
                        sent_frames += 1;
                        continue;
                    }
                    beast(f, now_ms * 12000, signal, &mut r.buf);
                    sent_frames += 1;
                }
            }
        }
        for r in receivers.iter_mut() {
            match r.kind {
                Kind::Garbage => {
                    for _ in 0..300 {
                        let mut f = [0u8; 14];
                        for b in f.iter_mut() {
                            *b = noise.next() as u8;
                        }
                        f[0] = 0x8D;
                        beast(&f, now_ms * 12000, 50, &mut r.buf);
                    }
                }
                Kind::Flood => {
                    for _ in 0..2000 {
                        let addr = 0x700000 + (noise.next() % 0x80000) as u32;
                        beast(&enc::ident(addr, b"FLOOD   ", 0xA1), now_ms * 12000, 50, &mut r.buf);
                    }
                }
                _ => {}
            }
            if o.jitter_ms > 0 {
                // what was heard this tick sets off now and arrives later
                if !r.buf.is_empty() {
                    let arrive = now_ms + r.delay_ms + noise.next() % 60;
                    r.in_flight.push_back((arrive, std::mem::take(&mut r.buf)));
                }
                while r.in_flight.front().map_or(false, |f| f.0 <= now_ms) {
                    let (_, bytes) = r.in_flight.pop_front().unwrap();
                    r.buf.extend_from_slice(&bytes);
                }
            }
            let retry = Instant::now() >= r.next_connect;
            for (ti, target) in o.targets.iter().enumerate() {
                if r.kind != Kind::Clean && ti > 0 {
                    continue;
                }
                if r.socks[ti].is_none() && retry {
                    // a misbehaving receiver comes from the address given,
                    // so the server can tell it from the honest ones
                    let from = o.hostile_from.as_ref().filter(|_| r.kind != Kind::Clean);
                    match connect_from(target, from.map(|s| s.as_str())) {
                        Ok(mut s) => {
                            let _ = s.set_nodelay(true);
                            let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                            if s.write_all(&hello(&r.uuid)).is_ok() {
                                r.socks[ti] = Some(s);
                            }
                        }
                        Err(_) => r.next_connect = Instant::now() + Duration::from_secs(1),
                    }
                }
                if let Some(s) = r.socks[ti].as_mut() {
                    if !r.buf.is_empty() && s.write_all(&r.buf).is_err() {
                        r.socks[ti] = None;
                        r.next_connect = Instant::now() + Duration::from_secs(1);
                    }
                }
            }
            sent_bytes += r.buf.len() as u64;
            r.buf.clear();
        }
        if last_report.elapsed() >= Duration::from_secs(10) {
            let s = last_report.elapsed().as_secs_f64();
            eprintln!(
                "sky: {:.0} msg/s, {:.2} Mbit/s, {} links up",
                sent_frames as f64 / s,
                sent_bytes as f64 * 8.0 / s / 1e6,
                receivers.iter().map(|r| r.socks.iter().filter(|s| s.is_some()).count()).sum::<usize>()
            );
            sent_frames = 0;
            sent_bytes = 0;
            last_report = Instant::now();
        }
        now_ms += tick.as_millis() as u64;
    }
}

/// Connect, from a given local address if one is asked for.
fn connect_from(target: &str, from: Option<&str>) -> std::io::Result<TcpStream> {
    let Some(from) = from else { return TcpStream::connect(target) };
    let bad = |_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad address");
    let to: std::net::SocketAddr = target.parse().map_err(bad)?;
    let ip: std::net::IpAddr = from.parse().map_err(bad)?;
    let s = socket2::Socket::new(socket2::Domain::for_address(to), socket2::Type::STREAM, None)?;
    s.bind(&std::net::SocketAddr::new(ip, 0).into())?;
    s.connect(&to.into())?;
    Ok(s.into())
}
