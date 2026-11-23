//! Keeping one bad feeder from hurting the network: limits on
//! connections, on message rate and on new aircraft per receiver, a
//! ban for links that send garbage, and an allowlist for links we run
//! ourselves.

use crate::track::Ms;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub struct Limits {
    pub max_connections: usize,
    pub max_per_ip: u32,
    /// messages per second per connection, averaged over ten seconds
    pub max_rate: f64,
    /// aircraft one connection may introduce per minute, after the
    /// thousand it may bring when it connects
    pub max_new_aircraft: u32,
    pub max_aircraft: usize,
    pub trusted: Vec<Cidr>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cidr {
    addr: u128,
    mask: u128,
}

fn bits(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v) => u128::from(v.to_ipv6_mapped()),
        IpAddr::V6(v) => match v.to_ipv4_mapped() {
            Some(m) => u128::from(m.to_ipv6_mapped()),
            None => u128::from(v),
        },
    }
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Cidr> {
        let (a, len) = match s.split_once('/') {
            Some((a, l)) => (a, Some(l.parse::<u32>().ok()?)),
            None => (s, None),
        };
        let ip: IpAddr = a.trim().parse().ok()?;
        let width = if ip.is_ipv4() { 32 } else { 128 };
        let len = len.unwrap_or(width);
        if len > width {
            return None;
        }
        let host = width - len;
        let mask = if host >= 128 { 0 } else { !0u128 << host };
        Some(Cidr { addr: bits(ip) & mask, mask })
    }
    pub fn contains(&self, ip: IpAddr) -> bool {
        bits(ip) & self.mask == self.addr
    }
}

#[derive(Default)]
pub struct Counters {
    pub refused_banned: AtomicU64,
    pub refused_limit: AtomicU64,
    pub bans: AtomicU64,
    pub rate_dropped: AtomicU64,
    pub garbage: AtomicU64,
    pub aircraft_refused: AtomicU64,
    pub distrusted_positions: AtomicU64,
}

pub struct Guard {
    pub limits: Limits,
    total: AtomicUsize,
    per_ip: Mutex<HashMap<IpAddr, u32>>,
    /// address -> (refused until, why, how many times so far)
    bans: Mutex<HashMap<IpAddr, (Ms, &'static str, u32)>>,
    pub counters: Counters,
}

/// Held for the life of a connection; frees its slot when dropped.
pub struct Slot<'a> {
    guard: &'a Guard,
    ip: IpAddr,
    pub trusted: bool,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.guard.total.fetch_sub(1, Ordering::Relaxed);
        let mut m = self.guard.per_ip.lock();
        if let Some(n) = m.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

impl Guard {
    pub fn new(limits: Limits) -> Self {
        Guard { limits, total: AtomicUsize::new(0), per_ip: Mutex::new(HashMap::new()), bans: Mutex::new(HashMap::new()), counters: Counters::default() }
    }

    pub fn trusted(&self, ip: IpAddr) -> bool {
        self.limits.trusted.iter().any(|c| c.contains(ip))
    }

    /// Admit a connection, or say why not.
    pub fn admit(&self, ip: IpAddr, now: Ms) -> Result<Slot<'_>, &'static str> {
        let trusted = self.trusted(ip);
        if !trusted {
            let bans = self.bans.lock();
            if let Some(&(until, _, _)) = bans.get(&ip) {
                if now < until {
                    self.counters.refused_banned.fetch_add(1, Ordering::Relaxed);
                    return Err("banned");
                }
            }
        }
        if self.total.load(Ordering::Relaxed) >= self.limits.max_connections {
            self.counters.refused_limit.fetch_add(1, Ordering::Relaxed);
            return Err("connection limit");
        }
        {
            let mut m = self.per_ip.lock();
            let n = m.entry(ip).or_insert(0);
            if !trusted && *n >= self.limits.max_per_ip {
                self.counters.refused_limit.fetch_add(1, Ordering::Relaxed);
                return Err("per-address connection limit");
            }
            *n += 1;
        }
        self.total.fetch_add(1, Ordering::Relaxed);
        Ok(Slot { guard: self, ip, trusted })
    }

    pub fn ban(&self, ip: IpAddr, now: Ms, for_ms: Ms, reason: &'static str) {
        if self.trusted(ip) {
            return;
        }
        self.counters.bans.fetch_add(1, Ordering::Relaxed);
        let mut bans = self.bans.lock();
        // an address is remembered for an hour after its ban ends, and
        // each repeat doubles the next one, up to an hour
        bans.retain(|_, b| b.0 + 3_600_000 > now);
        let strikes = bans.get(&ip).map_or(0, |b| b.2 + 1);
        let for_ms = (for_ms << strikes.min(6)).min(3_600_000);
        bans.insert(ip, (now + for_ms, reason, strikes));
        eprintln!("aggd: {ip} refused for {} s: {reason}", for_ms / 1000);
    }

    pub fn connections(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn json(&self, now: Ms) -> String {
        let c = &self.counters;
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let bans = self.bans.lock();
        let active: Vec<String> = bans
            .iter()
            .filter(|(_, b)| b.0 > now)
            .map(|(ip, b)| format!("{{\"address\":\"{ip}\",\"seconds_left\":{},\"reason\":\"{}\"}}", (b.0 - now) / 1000, b.1))
            .collect();
        format!(
            "{{\"connections\":{},\"refused_banned\":{},\"refused_limit\":{},\"bans\":{},\"messages_over_rate\":{},\"garbage_frames\":{},\"aircraft_refused\":{},\"positions_from_distrusted_receivers\":{},\"banned\":[{}]}}",
            self.connections(),
            g(&c.refused_banned),
            g(&c.refused_limit),
            g(&c.bans),
            g(&c.rate_dropped),
            g(&c.garbage),
            g(&c.aircraft_refused),
            g(&c.distrusted_positions),
            active.join(",")
        )
    }
}

/// Per-connection accounting, owned by the connection's task.
pub struct Meter {
    tokens: f64,
    last: Ms,
    /// seconds in a row in which messages were dropped for rate
    over_secs: u32,
    over_this_sec: bool,
    sec: Ms,
    good: u32,
    garbage: u32,
    window_start: Ms,
    new_tokens: f64,
}

/// Aircraft a link may introduce at once: what a busy receiver sees
/// when it connects.
const NEW_BURST: f64 = 1000.0;

pub enum Verdict {
    Ok,
    /// Disconnect and refuse the address for this long.
    Ban(Ms, &'static str),
}

impl Meter {
    pub fn new(now: Ms, rate: f64) -> Self {
        Meter { tokens: rate * 10.0, last: now, over_secs: 0, over_this_sec: false, sec: now / 1000, good: 0, garbage: 0, window_start: now, new_tokens: NEW_BURST }
    }

    /// Refill for the time passed; call once per read.
    pub fn tick(&mut self, now: Ms, rate: f64, new_per_min: u32) -> Verdict {
        let dt = now.saturating_sub(self.last) as f64 / 1000.0;
        self.tokens = (self.tokens + rate * dt).min(rate * 10.0);
        self.new_tokens = (self.new_tokens + new_per_min as f64 * dt / 60.0).min(NEW_BURST);
        self.last = now;
        if now / 1000 != self.sec {
            self.over_secs = if self.over_this_sec { self.over_secs + 1 } else { 0 };
            self.over_this_sec = false;
            self.sec = now / 1000;
            if self.over_secs >= 20 {
                return Verdict::Ban(60_000, "message rate far above what one receiver can hear");
            }
        }
        if now.saturating_sub(self.window_start) >= 10_000 {
            let total = self.good + self.garbage;
            let bad = total >= 200 && self.garbage * 2 > total;
            self.good = 0;
            self.garbage = 0;
            self.window_start = now;
            if bad {
                return Verdict::Ban(300_000, "mostly frames that fail their checksum");
            }
        }
        Verdict::Ok
    }

    /// Whether one more message is within the rate.
    #[inline]
    pub fn take(&mut self) -> bool {
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            self.over_this_sec = true;
            false
        }
    }
    #[inline]
    pub fn good(&mut self) {
        self.good += 1;
    }
    #[inline]
    pub fn garbage(&mut self) {
        self.garbage += 1;
    }
    /// Whether this connection may introduce another aircraft.
    #[inline]
    pub fn may_create(&mut self) -> bool {
        if self.new_tokens >= 1.0 {
            self.new_tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits { max_connections: 3, max_per_ip: 2, max_rate: 100.0, max_new_aircraft: 5, max_aircraft: 10, trusted: vec![Cidr::parse("10.0.0.0/8").unwrap()] }
    }

    #[test]
    fn cidr() {
        let c = Cidr::parse("192.168.4.0/22").unwrap();
        assert!(c.contains("192.168.7.255".parse().unwrap()));
        assert!(!c.contains("192.168.8.0".parse().unwrap()));
        assert!(Cidr::parse("2001:db8::/32").unwrap().contains("2001:db8:1::5".parse().unwrap()));
        assert!(Cidr::parse("1.2.3.4").unwrap().contains("::ffff:1.2.3.4".parse().unwrap()));
        assert_eq!(Cidr::parse("1.2.3.4/40"), None);
    }

    #[test]
    fn connection_limits_and_bans() {
        let g = Guard::new(limits());
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let a = g.admit(ip, 0).unwrap();
        let _b = g.admit(ip, 0).unwrap();
        assert_eq!(g.admit(ip, 0).err(), Some("per-address connection limit"));
        drop(a);
        let _c = g.admit(ip, 0).unwrap();
        let _d = g.admit("203.0.113.10".parse().unwrap(), 0).unwrap();
        assert_eq!(g.admit("203.0.113.11".parse().unwrap(), 0).err(), Some("connection limit"));
        g.ban(ip, 1000, 60_000, "test");
        assert_eq!(g.admit(ip, 2000).err(), Some("banned"));
        // the second offence costs twice as long
        g.ban(ip, 70_000, 60_000, "test");
        assert_eq!(g.admit(ip, 70_000 + 119_000).err(), Some("banned"));
        // our own addresses are never banned
        let own: IpAddr = "10.1.2.3".parse().unwrap();
        g.ban(own, 1000, 60_000, "test");
        assert!(g.bans.lock().get(&own).is_none());
    }

    #[test]
    fn rate_and_garbage() {
        let mut m = Meter::new(0, 100.0);
        // a burst of ten seconds' worth passes, then the rate holds
        let passed = (0..5000).filter(|_| m.take()).count();
        assert_eq!(passed, 1000);
        assert!(matches!(m.tick(1000, 100.0, 60), Verdict::Ok));
        assert_eq!((0..500).filter(|_| m.take()).count(), 100);
        // twenty seconds over the rate end the connection
        let mut banned = false;
        for s in 2..40u64 {
            for _ in 0..500 {
                m.take();
            }
            if let Verdict::Ban(..) = m.tick(s * 1000, 100.0, 60) {
                banned = true;
                break;
            }
        }
        assert!(banned);
        let mut m = Meter::new(0, 1e9);
        for _ in 0..300 {
            m.garbage();
        }
        for _ in 0..100 {
            m.good();
        }
        assert!(matches!(m.tick(10_000, 1e9, 60), Verdict::Ban(300_000, _)));
        // a thousand aircraft at once, then one a second at 60 a minute
        let mut m = Meter::new(0, 1e9);
        assert_eq!((0..5000).filter(|_| m.may_create()).count(), 1000);
        let _ = m.tick(5000, 1e9, 60);
        assert_eq!((0..100).filter(|_| m.may_create()).count(), 5);
    }
}
