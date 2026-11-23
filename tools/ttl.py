#!/usr/bin/env python3
"""Measure how long an aggregator keeps each field of an aircraft that is
still being heard: send one of every message, then only all-call replies,
and note when each field leaves aircraft.json.

    ttl.py BEAST_HOST:PORT AIRCRAFT_JSON_URL_OR_FILE [SECONDS] [--hex ADDR]
"""
import json, os, socket, subprocess, sys, time, urllib.request
HERE = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]; hexa = "4CA2D6"
if "--hex" in args:
    i = args.index("--hex"); hexa = args[i + 1]; del args[i:i + 2]
host, port = args[0].rsplit(":", 1); src = args[1]; secs = int(args[2]) if len(args) > 2 else 900

def beast(frame, t):
    body = int(t * 12e6).to_bytes(6, "big") + bytes([120]) + frame
    return b"\x1a" + (b"2" if len(frame) == 7 else b"3") + body.replace(b"\x1a", b"\x1a\x1a")

def load():
    try:
        d = json.load(urllib.request.urlopen(src, timeout=5)) if src.startswith("http") else json.load(open(src))
        for a in d["aircraft"]:
            if a["hex"] == hexa.lower(): return a
    except Exception:
        pass
    return None

frames = dict(l.split() for l in subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa], capture_output=True, text=True, check=True).stdout.split("\n") if l)
order = ["df11", "ident", "op_status", "pos_even", "pos_odd", "velocity", "velocity_air", "emergency", "emergency", "target_state", "df4", "df5", "df5",
         "df20_bds50", "df20_bds60", "df21_bds40", "pos_even", "pos_odd"]
s = socket.create_connection((host, int(port))); t0 = time.time()
for n in order:
    s.sendall(beast(bytes.fromhex(frames[n.lstrip("+")]), time.time() - t0)); time.sleep(0.15)
sent = time.time(); time.sleep(1.5)
first = load() or {}
alive = {k: first[k] for k in first}
print("after sending:", json.dumps(first)[:900]); gone = {}
last_ping = 0
while time.time() - sent < secs and (alive or time.time() - sent < 5):
    if time.time() - last_ping >= 4:
        s.sendall(beast(bytes.fromhex(frames["df11"]), time.time() - t0)); last_ping = time.time()
    time.sleep(1)
    a = load()
    if a is None:
        print(f"{time.time() - sent:6.0f} s: aircraft no longer listed"); break
    if "type" in first and a.get("type") != first["type"]:
        print(f"{time.time() - sent:6.0f} s: type is now {a.get('type')}", flush=True); first["type"] = a.get("type")
    for k in list(alive):
        if k not in a:
            gone[k] = time.time() - sent; del alive[k]
            print(f"{gone[k]:6.0f} s: {k} gone" + (f"   (now has {[x for x in a if x not in first]})" if any(x not in first for x in a) else ""), flush=True)
print("still present at the end:", sorted(alive))
