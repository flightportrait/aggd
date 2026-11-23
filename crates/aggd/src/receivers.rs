//! Who is feeding: one entry per connection (clients.json) and one per
//! receiver identity (receivers.json, the coverage box each station's
//! positions span).

use crate::track::Ms;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct Client {
    pub id: u64,
    /// The receiver UUID as announced, or empty.
    pub uuid: Mutex<String>,
    pub rid: AtomicU64,
    pub peer: std::net::SocketAddr,
    pub connected: Ms,
    pub bytes: AtomicU64,
    pub messages: AtomicU64,
    pub positions: AtomicU64,
    /// last measured round trip to the feeder, ms; -1 unknown
    pub rtt_ms: std::sync::atomic::AtomicI64,
    /// counters at the last rate sample, and the rates then
    sample: Mutex<(u64, u64, f64, f64)>,
}

/// What one connection saw of a receiver since it last reported in.
pub struct Seen {
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
    pub positions: u64,
    /// positions it alone reported against a confirmed track
    pub bad: u64,
    /// the aircraft those were about
    pub blamed: Vec<u32>,
    /// heard over a link the operator trusts
    pub trusted: bool,
}

impl Default for Seen {
    fn default() -> Self {
        Seen { lat_min: f64::MAX, lat_max: f64::MIN, lon_min: f64::MAX, lon_max: f64::MIN, positions: 0, bad: 0, blamed: Vec::new(), trusted: false }
    }
}

#[derive(Clone)]
pub struct Extent {
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
    pub positions: u64,
    pub last: Ms,
    pub rate: f64,
    sampled: u64,
    /// positions that contradicted an aircraft's track, this minute
    bad: u64,
    blamed: Vec<u32>,
    good_at_window: u64,
    window: Ms,
    /// positions from this receiver are not used until then
    pub distrust_until: Ms,
    /// heard over a link the operator trusts: never distrusted
    trusted: bool,
}

/// A receiver is distrusted only when its contradicted positions are at
/// least one in this many of the positions it had accepted that minute.
const BAD_SHARE: u64 = 10;

#[derive(Default)]
pub struct Registry {
    next: AtomicU64,
    /// bumped whenever the set of distrusted receivers changes
    pub epoch: AtomicU64,
    pub clients: Mutex<HashMap<u64, Arc<Client>>>,
    pub receivers: Mutex<HashMap<u64, Extent>>,
}

/// A receiver id is the first 16 hex digits of its UUID.
pub fn rid_of(uuid: &str) -> u64 {
    let mut v = 0u64;
    let mut n = 0;
    for c in uuid.bytes() {
        if let Some(d) = (c as char).to_digit(16) {
            v = v << 4 | d as u64;
            n += 1;
            if n == 16 {
                break;
            }
        }
    }
    v
}

pub fn rid_str(rid: u64) -> String {
    format!("{:08x}-{:04x}-{:04x}", rid >> 32, (rid >> 16) & 0xffff, rid & 0xffff)
}

impl Registry {
    pub fn connect(&self, peer: std::net::SocketAddr, now: Ms) -> Arc<Client> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let c = Arc::new(Client {
            id,
            uuid: Mutex::new(String::new()),
            // until a UUID arrives the connection is its own receiver
            rid: AtomicU64::new(0xFFFF_0000_0000_0000 | id),
            peer,
            connected: now,
            bytes: AtomicU64::new(0),
            messages: AtomicU64::new(0),
            positions: AtomicU64::new(0),
            rtt_ms: std::sync::atomic::AtomicI64::new(-1),
            sample: Mutex::new((0, 0, 0.0, 0.0)),
        });
        self.clients.lock().insert(id, c.clone());
        c
    }
    pub fn disconnect(&self, c: &Client) {
        self.clients.lock().remove(&c.id);
    }

    /// A receiver's rough location: the middle of the box its airborne
    /// positions span, once there are enough of them.
    pub fn reference(&self, rid: u64) -> Option<(f64, f64)> {
        // a link that did not say which receiver it is may carry a whole
        // network; the middle of everything it hears is nowhere
        if rid >> 48 == 0xFFFF {
            return None;
        }
        let r = self.receivers.lock();
        let e = r.get(&rid)?;
        // The middle of a receiver's coverage stands for where it is only
        // if the coverage is that of one antenna: an id that speaks for
        // a whole network has no place.
        let mid = ((e.lat_min + e.lat_max) / 2.0).to_radians().cos().max(0.05);
        let one_antenna = e.lat_max - e.lat_min <= 10.0 && (e.lon_max - e.lon_min) * mid <= 10.0;
        (e.positions >= 100 && e.distrust_until == 0 && one_antenna).then(|| ((e.lat_min + e.lat_max) / 2.0, (e.lon_min + e.lon_max) / 2.0))
    }

    /// Merge what one connection saw of a receiver: a bounding box
    /// (lat min, lat max, lon min, lon max) and a position count.
    pub fn position_box(&self, rid: u64, b: Seen, now: Ms) {
        let mut r = self.receivers.lock();
        let e = r.entry(rid).or_insert(Extent {
            lat_min: b.lat_min,
            lat_max: b.lat_max,
            lon_min: b.lon_min,
            lon_max: b.lon_max,
            positions: 0,
            last: now,
            rate: 0.0,
            sampled: 0,
            bad: 0,
            blamed: Vec::new(),
            good_at_window: 0,
            window: now,
            distrust_until: 0,
            trusted: false,
        });
        e.trusted |= b.trusted;
        for a in &b.blamed {
            if e.blamed.len() < 64 && !e.blamed.contains(a) {
                e.blamed.push(*a);
            }
        }
        let b = (b.lat_min, b.lat_max, b.lon_min, b.lon_max, b.positions, b.bad);
        if b.4 > 0 {
            // Once a receiver's coverage is known, a position far outside
            // it is an aircraft reporting a false position (GPS
            // interference does that), not new coverage.
            let (mut b0, mut b1, mut b2, mut b3) = (b.0, b.1, b.2, b.3);
            if e.positions >= 200 {
                let (clat, clon) = ((e.lat_min + e.lat_max) / 2.0, (e.lon_min + e.lon_max) / 2.0);
                let wlon = 7.0 / clat.to_radians().cos().max(0.1);
                b0 = b0.max(clat - 7.0);
                b1 = b1.min(clat + 7.0);
                b2 = b2.max(clon - wlon);
                b3 = b3.min(clon + wlon);
            }
            if b0 <= b1 && b2 <= b3 {
                e.lat_min = e.lat_min.min(b0);
                e.lat_max = e.lat_max.max(b1);
                e.lon_min = e.lon_min.min(b2);
                e.lon_max = e.lon_max.max(b3);
            }
        }
        e.positions += b.4;
        e.bad += b.5;
        e.last = now;
    }

    /// Receivers whose positions are currently not used.
    pub fn distrusted(&self, now: Ms) -> Vec<u64> {
        self.receivers.lock().iter().filter(|(_, e)| e.distrust_until > now).map(|(r, _)| *r).collect()
    }

    /// Once a second: refresh rates, forget receivers silent for an hour.
    pub fn tick(&self, now: Ms, dt: f64) {
        for c in self.clients.lock().values() {
            let (m, p) = (c.messages.load(Ordering::Relaxed), c.positions.load(Ordering::Relaxed));
            let mut s = c.sample.lock();
            s.2 = 0.7 * s.2 + 0.3 * (m - s.0) as f64 / dt;
            s.3 = 0.7 * s.3 + 0.3 * (p - s.1) as f64 / dt;
            s.0 = m;
            s.1 = p;
        }
        let mut r = self.receivers.lock();
        r.retain(|_, e| now.saturating_sub(e.last) < 3_600_000);
        let mut changed = false;
        for (rid, e) in r.iter_mut() {
            e.rate = 0.7 * e.rate + 0.3 * (e.positions - e.sampled) as f64 / dt;
            e.sampled = e.positions;
            if e.distrust_until > now {
                continue;
            }
            if e.distrust_until != 0 {
                // the time is served: the links must hear of it, or they
                // would go on ignoring this receiver for good
                e.distrust_until = 0;
                e.bad = 0;
                e.blamed.clear();
                e.good_at_window = e.positions;
                e.window = now;
                changed = true;
                eprintln!("aggd: receiver {} positions used again", rid_str(*rid));
            }
            let mut why = None;
            if now.saturating_sub(e.window) >= 60_000 {
                // One aircraft out of place is that aircraft's business (two
                // airframes sharing an address, a faulty transponder). The
                // same receiver out of step on several is the receiver.
                // And only when it is a real share of what the receiver
                // sent: a relay carrying half the network collects forty
                // contradictions a minute from honest traffic alone.
                let good = e.positions.saturating_sub(e.good_at_window);
                if !e.trusted && e.bad >= 40 && e.blamed.len() >= 5 && e.bad * BAD_SHARE >= good {
                    why = Some("its positions contradict the rest of the network");
                }
                e.bad = 0;
                e.blamed.clear();
                e.good_at_window = e.positions;
                e.window = now;
            }
            if let Some(w) = why {
                e.distrust_until = now + 600_000;
                // start its coverage box afresh when it is trusted again
                e.lat_min = f64::MAX;
                e.lat_max = f64::MIN;
                e.lon_min = f64::MAX;
                e.lon_max = f64::MIN;
                e.positions = 0;
                e.sampled = 0;
                e.good_at_window = 0;
                changed = true;
                eprintln!("aggd: receiver {} positions ignored for 10 min: {w}", rid_str(*rid));
            }
        }
        if changed {
            self.epoch.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn clients_json(&self, now: Ms) -> String {
        use std::fmt::Write;
        let mut o = String::with_capacity(4096);
        let _ = write!(
            o,
            "{{ \"now\" : {:.3},\n  \"format\" : [ \"receiverId\", \"host:port\", \"avg. kbit/s\", \"conn time(s)\", \"messages/s\", \"positions/s\", \"reduce_signal\", \"recent_rtt(ms)\", \"positions\" ],\n  \"clients\" : [\n",
            now as f64 / 1000.0
        );
        let cl = self.clients.lock();
        let mut first = true;
        for c in cl.values() {
            let secs = (now.saturating_sub(c.connected) as f64 / 1000.0).max(0.001);
            let s = c.sample.lock();
            let uuid = c.uuid.lock();
            let name = if uuid.is_empty() { rid_str(c.rid.load(Ordering::Relaxed)) } else { uuid.clone() };
            if !first {
                o.push_str(",\n");
            }
            first = false;
            let _ = write!(
                o,
                "[ \"{}\", \"{:>50}\", {:.2}, {:.0}, {:.3}, {:.3}, 0, {}, {} ]",
                name,
                format!("{} port {}", c.peer.ip(), c.peer.port()),
                c.bytes.load(Ordering::Relaxed) as f64 * 8.0 / 1000.0 / secs,
                secs,
                s.2,
                s.3,
                c.rtt_ms.load(Ordering::Relaxed),
                c.positions.load(Ordering::Relaxed)
            );
        }
        o.push_str("\n  ]\n}\n");
        o
    }

    pub fn receivers_json(&self, now: Ms) -> String {
        use std::fmt::Write;
        let mut o = String::with_capacity(4096);
        let _ = write!(o, "{{ \"now\" : {:.1},\n  \"receivers\" : [\n", now as f64 / 1000.0);
        let r = self.receivers.lock();
        let mut first = true;
        for (rid, e) in r.iter() {
            // a box with nothing in it yet has no corners
            let (lat0, lat1, lon0, lon1) = if e.positions == 0 { (0.0, 0.0, 0.0, 0.0) } else { (e.lat_min, e.lat_max, e.lon_min, e.lon_max) };
            if !first {
                o.push_str(",\n");
            }
            first = false;
            let _ = write!(
                o,
                "[ \"{}\", {:.2}, 0.00, {:.2}, {:.2}, {:.2}, {:.2}, {}, {:.2}, {:.2} ]",
                rid_str(*rid),
                e.rate,
                lat0,
                lat1,
                lon0,
                lon1,
                (e.distrust_until > now) as u8,
                (lat0 + lat1) / 2.0,
                (lon0 + lon1) / 2.0
            );
        }
        o.push_str("\n  ]\n}\n");
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minute(reg: &Registry, rid: u64, good: u64, bad: u64, now: Ms) {
        let s = Seen { lat_min: 45.0, lat_max: 46.0, lon_min: 9.0, lon_max: 10.0, positions: good, bad, blamed: (0..6).collect(), trusted: false };
        reg.position_box(rid, s, now);
        reg.tick(now, 1.0);
    }

    #[test]
    fn a_busy_relay_is_not_distrusted_for_a_few_contradictions() {
        let reg = Registry::default();
        minute(&reg, 7, 0, 0, 1_000);
        minute(&reg, 7, 50_000, 60, 62_000);
        assert!(reg.distrusted(62_000).is_empty());
    }

    #[test]
    fn distrust_ends_and_the_links_are_told() {
        let reg = Registry::default();
        minute(&reg, 7, 0, 0, 1_000);
        minute(&reg, 7, 100, 60, 62_000);
        assert_eq!(reg.distrusted(62_000), vec![7]);
        let told = reg.epoch.load(Ordering::Relaxed);
        reg.tick(62_000 + 599_000, 1.0);
        assert_eq!(reg.epoch.load(Ordering::Relaxed), told, "still serving");
        reg.tick(62_000 + 601_000, 1.0);
        assert!(reg.distrusted(62_000 + 601_000).is_empty());
        assert!(reg.epoch.load(Ordering::Relaxed) > told, "links refresh their list");
    }
}
