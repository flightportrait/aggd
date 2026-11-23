//! Record a Beast stream to a file with arrival times, and play it back
//! to one or more aggregators as it arrived (or faster).
//!
//! File: repeated records of u32 milliseconds since the start, u32
//! length, then the bytes as they came off the socket.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub fn record(source: &str, path: &str, seconds: u64) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(source)?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    let start = Instant::now();
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    while start.elapsed().as_secs() < seconds {
        let n = s.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&(start.elapsed().as_millis() as u32).to_le_bytes())?;
        out.write_all(&(n as u32).to_le_bytes())?;
        out.write_all(&buf[..n])?;
        total += n as u64;
    }
    out.flush()?;
    eprintln!("recorded {total} bytes over {} s to {path}", start.elapsed().as_secs());
    Ok(())
}

/// Play a recording to every target. `speed` 1.0 is real time. With
/// `only`, frames for other ICAO addresses are dropped (long frames and
/// all-call replies by their address field; the rest are kept, since
/// their address is in the parity).
pub fn replay(path: &str, targets: &[String], speed: f64, only: Option<u32>) -> anyhow::Result<()> {
    let data = std::fs::read(path)?;
    let mut socks: Vec<TcpStream> = Vec::new();
    for t in targets {
        let s = TcpStream::connect(t)?;
        s.set_nodelay(true)?;
        socks.push(s);
    }
    let start = Instant::now();
    let mut i = 0;
    let mut sent = 0u64;
    let mut carry: Vec<u8> = Vec::new();
    while i + 8 <= data.len() {
        let t = u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as f64;
        let n = u32::from_le_bytes(data[i + 4..i + 8].try_into().unwrap()) as usize;
        i += 8;
        if i + n > data.len() {
            break;
        }
        let chunk = &data[i..i + n];
        i += n;
        let due = Duration::from_secs_f64(t / 1000.0 / speed);
        if let Some(d) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(d);
        }
        let payload: Vec<u8> = match only {
            None => chunk.to_vec(),
            Some(addr) => {
                carry.extend_from_slice(chunk);
                let (kept, used) = filter(&carry, addr);
                carry.drain(..used);
                kept
            }
        };
        for s in socks.iter_mut() {
            s.write_all(&payload)?;
        }
        sent += payload.len() as u64;
    }
    eprintln!("replayed {sent} bytes in {:.1} s", start.elapsed().as_secs_f64());
    // let the receivers drain before the sockets close
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}

/// Whole Beast messages from the front of `b` that concern `addr`:
/// (bytes to send, bytes consumed).
fn filter(b: &[u8], addr: u32) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        let Some(off) = b[pos..].iter().position(|&x| x == 0x1a) else { return (out, b.len()) };
        pos += off;
        let Some(&t) = b.get(pos + 1) else { return (out, pos) };
        let body = match t {
            0x31 => 9,
            0x32 => 14,
            0x33 => 21,
            _ => {
                pos += 1;
                continue;
            }
        };
        let mut raw = [0u8; 21];
        let (mut i, mut n) = (pos + 2, 0);
        let mut broken = false;
        while n < body {
            let Some(&x) = b.get(i) else { return (out, pos) };
            if x == 0x1a {
                match b.get(i + 1) {
                    None => return (out, pos),
                    Some(0x1a) => i += 1,
                    Some(_) => {
                        broken = true;
                        break;
                    }
                }
            }
            raw[n] = x;
            n += 1;
            i += 1;
        }
        if !broken && t != 0x31 {
            let f = &raw[7..body];
            let df = f[0] >> 3;
            let keep = match df {
                11 | 17 | 18 => ((f[1] as u32) << 16 | (f[2] as u32) << 8 | f[3] as u32) == addr,
                _ => modes::crc_residual(f) == addr,
            };
            if keep {
                out.extend_from_slice(&b[pos..i]);
            }
        }
        pos = i;
    }
}

/// Print every frame of one aircraft in a recording, decoded.
pub fn dump(path: &str, addr: u32) -> anyhow::Result<()> {
    let data = std::fs::read(path)?;
    let mut i = 0;
    let mut carry: Vec<u8> = Vec::new();
    while i + 8 <= data.len() {
        let t = u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let n = u32::from_le_bytes(data[i + 4..i + 8].try_into().unwrap()) as usize;
        i += 8;
        if i + n > data.len() {
            break;
        }
        carry.extend_from_slice(&data[i..i + n]);
        i += n;
        let (kept, used) = filter(&carry, addr);
        carry.drain(..used);
        // kept holds whole messages; unescape each and decode
        let mut p = 0;
        while p + 2 < kept.len() {
            let body = if kept[p + 1] == 0x32 { 14 } else { 21 };
            let mut raw = Vec::with_capacity(body);
            let mut q = p + 2;
            while raw.len() < body && q < kept.len() {
                if kept[q] == 0x1a {
                    q += 1;
                }
                raw.push(kept[q]);
                q += 1;
            }
            p = q;
            let f = &raw[7..];
            let hex: String = f.iter().map(|b| format!("{b:02X}")).collect();
            let m = modes::decode(f, |a| a == addr);
            let mut d = format!("{m:?}");
            // drop the fields that say nothing
            for junk in ["Msg { ", " }"] {
                d = d.replace(junk, "");
            }
            let d: Vec<&str> = d.split(", ").filter(|x| !x.ends_with(": None") && !x.starts_with("op: OpStatus") && !x.contains("addr:")).collect();
            println!("{:8.2} {hex} {}", t as f64 / 1000.0, d.join(" "));
        }
    }
    Ok(())
}
