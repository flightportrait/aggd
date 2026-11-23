#!/usr/bin/env python3
"""When does an aggregator show an altitude from a surveillance reply
(DF4) after the ADS-B altitude has gone stale?
    alt_probe.py BEAST_HOST:PORT AIRCRAFT_JSON_URL_OR_FILE"""
import json, os, socket, subprocess, sys, time, urllib.request
HERE = os.path.dirname(os.path.abspath(__file__))
host, port = sys.argv[1].rsplit(":", 1); src = sys.argv[2]
def beast(frame, t):
    body = int(t * 12e6).to_bytes(6, "big") + bytes([120]) + frame
    return b"\x1a" + (b"2" if len(frame) == 7 else b"3") + body.replace(b"\x1a", b"\x1a\x1a")
def load(hexa):
    try:
        d = json.load(urllib.request.urlopen(src, timeout=5)) if src.startswith("http") else json.load(open(src))
        return next((a for a in d["aircraft"] if a["hex"] == hexa.lower()), {})
    except Exception: return {}
def frames(hexa, alt):
    out = subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa, "--df4-alt", str(alt)], capture_output=True, text=True, check=True).stdout
    return dict(l.split() for l in out.split("\n") if l)
s = socket.create_connection((host, int(port))); t0 = time.time()
def send(f): s.sendall(beast(bytes.fromhex(f), time.time() - t0)); time.sleep(0.05)
# (label, hex, wait before the reply, altitude in the reply, how many replies)
cases = [("after 20 s, same altitude", "4D0201", 20, 36000, 1), ("after 20 s, 2000 ft lower", "4D0202", 20, 34000, 1),
         ("after 45 s, same altitude", "4D0203", 45, 36000, 1), ("after 45 s, 2000 ft lower", "4D0204", 45, 34000, 1),
         ("after 80 s, same altitude", "4D0205", 80, 36000, 1), ("after 80 s, 2000 ft lower", "4D0206", 80, 34000, 1),
         ("after 80 s, 2000 ft lower, twice", "4D0207", 80, 34000, 2), ("after 80 s, 300 ft lower", "4D0208", 80, 35700, 1),
         ("no ADS-B altitude before, one reply", "4D0209", 0, 34000, 1)]
fr = {c[1]: frames(c[1], c[3]) for c in cases}
for label, h, wait, alt, n in cases:
    send(fr[h]["df11"])
    if wait:
        for k in ("pos_even", "pos_odd"): send(fr[h][k])
start = time.time(); done = set()
while len(done) < len(cases):
    el = time.time() - start
    for label, h, wait, alt, n in cases:
        send(fr[h]["df11"]) if int(el) % 8 == 0 else None
        if h not in done and el >= wait:
            for _ in range(n): send(fr[h]["df4"]); time.sleep(0.3)
            done.add(h)
            time.sleep(1.8); a = load(h)
            print(f"{label:40s} -> alt_baro {a.get('alt_baro')}", flush=True)
    time.sleep(1)
