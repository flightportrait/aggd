#!/usr/bin/env python3
"""When does an aggregator show an aircraft's ADS-B version, kind of
source and DF0 altitude? Plays several small histories at once, each on
its own address, and prints what is shown as they go.

    version_probe.py BEAST_HOST:PORT AIRCRAFT_JSON_URL_OR_FILE
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
  ("S gap 8: placed+status; velocity at 62 and 70", ["df11"] + P + ["op_status", 61, "velocity", 8, "velocity", 12]),
  ("S gap 12: placed+status; velocity at 58 and 70", ["df11"] + P + ["op_status", 57, "velocity", 12, "velocity", 12]),
  ("S gap 16: placed+status; velocity at 54 and 70", ["df11"] + P + ["op_status", 53, "velocity", 16, "velocity", 12]),
  ("S gap 20: placed+status; velocity at 50 and 70", ["df11"] + P + ["op_status", 49, "velocity", 20, "velocity", 12]),
  ("S gap 25: placed+status; velocity at 45 and 70", ["df11"] + P + ["op_status", 44, "velocity", 25, "velocity", 12]),
  ("S6 placed+status; quiet; velocity at 55", ["df11"] + P + ["op_status", 54, "velocity", 12]),
  ("S7 placed+status; quiet; velocity at 58", ["df11"] + P + ["op_status", 57, "velocity", 12]),
  ("S8 placed+status; quiet; velocity at 62", ["df11"] + P + ["op_status", 61, "velocity", 12]),
  ("S9 placed+status; quiet; velocity at 64", ["df11"] + P + ["op_status", 63, "velocity", 12]),
  ("S10 placed+status; velocity at 30,45,58; then 70", ["df11"] + P + ["op_status", 29, "velocity", 15, "velocity", 13, "velocity", 12, "velocity", 12]),
  ("S11 placed+status; ident at 62 (not velocity)", ["df11"] + P + ["op_status", 61, "ident", 12]),
  ("S12 placed+status; status again at 62", ["df11"] + P + ["op_status", 61, "op_status", 12]),
]
rows = {}
def play(i, label, steps):
    hexa = "%06X" % (0x4D0700 + i)
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
        v = (a.get("type"), a.get("version"), a.get("nac_p"), a.get("sil"), a.get("alt_baro"), "pos" if "lat" in a else ("last" if "lastPosition" in a else "-"))
        if last.get(hexa) != v:
            last[hexa] = v
            print(f"{time.time()-t0:6.0f}s {label[:46]:46s} type={v[0]} version={v[1]} nac_p={v[2]} sil={v[3]} alt={v[4]} {v[5]}", flush=True)
