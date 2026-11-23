#!/usr/bin/env python3
"""When does an aggregator show a geometric altitude? Plays several small histories at once, each on
its own address, and prints what is shown as they go.

    geom_probe.py BEAST_HOST:PORT AIRCRAFT_JSON_URL_OR_FILE
"""
import json, os, socket, subprocess, sys, threading, time, urllib.request
HERE = os.path.dirname(os.path.abspath(__file__))
host, port = sys.argv[1].rsplit(":", 1); src = sys.argv[2]
def beast(frame, t):
    body = int(t * 12e6).to_bytes(6, "big") + bytes([120]) + frame
    return b"\x1a" + (b"2" if len(frame) == 7 else b"3") + body.replace(b"\x1a", b"\x1a\x1a")
def load():
    for _ in range(3):
        try:
            d = json.load(urllib.request.urlopen(src, timeout=5)) if src.startswith("http") else json.load(open(src))
            return {a["hex"]: a for a in d["aircraft"]}
        except Exception:
            time.sleep(0.2)
    return {}
s = socket.create_connection((host, int(port))); t0 = time.time(); lock = threading.Lock()
def send(f):
    with lock: s.sendall(beast(bytes.fromhex(f), time.time() - t0))
    time.sleep(0.06)
P = ["pos_even", "pos_odd"]
# a history: (label, [steps]); a step is a frame name, a number of seconds
# to wait, or ("every", seconds, total, [frames]) to repeat frames
H = [
  ("A DF4 twice, then velocity with delta", ["df11", "df4", 1, "df4", 3, "velocity", 8]),
  ("B velocity with delta, then DF4 twice", ["df11", "velocity", 3, "df4", 1, "df4", 8]),
  ("C velocity, 40 s, then DF4 twice", ["df11", "velocity", ("every", 5, 40, ["df11"]), "df4", 1, "df4", 8]),
  ("D DF4 x2, velocity once, DF4 every 5 s for 90 s", ["df11", "df4", 1, "df4", 2, "velocity", ("every", 5, 90, ["df4"])]),
  ("E placed, velocity once, positions for 90 s", ["df11", "pos_even", "pos_odd", "velocity", ("every", 5, 90, ["pos_even", "pos_odd"])]),
  ("F placed, velocity every 5 s, no more positions", ["df11", "pos_even", "pos_odd", ("every", 5, 90, ["velocity"])]),
  ("G velocity every 5 s with DF4 every 5 s", ["df11", ("every", 5, 60, ["velocity", "df4"])]),
]
rows = {}
def play(i, label, steps):
    hexa = "%06X" % (0x4D0500 + i)
    fr = dict(l.split() for l in subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa], capture_output=True, text=True, check=True).stdout.split("\n") if l)
    rows[hexa.lower()] = label
    for st in steps:
        if isinstance(st, (int, float)): time.sleep(st)
        elif isinstance(st, tuple):
            end = time.time() + st[2]
            while time.time() < end:
                for f in st[3]: send(bytes.fromhex(fr[f]).hex())
                time.sleep(st[1])
        else: send(fr[st])
th = [threading.Thread(target=play, args=(i, l, st), daemon=True) for i, (l, st) in enumerate(H)]
for t in th: t.start()
last = {}
while any(t.is_alive() for t in th) or time.time() - t0 < 5:
    time.sleep(1); d = load()
    for hexa, label in sorted(rows.items()):
        a = d.get(hexa, {})
        v = (a.get("type"), a.get("alt_baro"), a.get("alt_geom"), a.get("baro_rate"), a.get("geom_rate"), "pos" if "lat" in a else "-")
        if last.get(hexa) != v:
            last[hexa] = v
            print(f"{time.time()-t0:6.0f}s {label[:46]:46s} type={v[0]} alt_baro={v[1]} alt_geom={v[2]} baro_rate={v[3]} geom_rate={v[4]} {v[5]}", flush=True)
