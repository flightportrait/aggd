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
  ("A never placed: status, velocity", ["df11", "op_status", "velocity", ("every", 5, 60, ["velocity", "df11"])]),
  ("B placed, status, then only velocity for 150 s", ["df11"] + P + ["op_status", ("every", 5, 150, ["velocity", "df11"])]),
  ("C as B, then status again, then position", ["df11"] + P + ["op_status", ("every", 5, 110, ["velocity", "df11"]), "op_status", 10, "velocity", 10] + P + [10, "velocity", 10]),
  ("D as B, then position without status", ["df11"] + P + ["op_status", ("every", 5, 110, ["velocity", "df11"])] + P + [5, "velocity", 10, "op_status", 10]),
  ("E placed, no status ever, velocity", ["df11"] + P + [("every", 5, 40, ["velocity", "df11"])]),
  ("F ident only, never placed", ["df11", "ident", ("every", 5, 70, ["df11"])]),
  ("G DF11 then DF0 altitude twice", ["df11", "df0", 1, "df0", 5, "df11", "df0", 10]),
  ("H DF17 velocity then DF0 twice", ["df11", "velocity", "df0", 1, "df0", 5, "df0", 10]),
  ("I placed, positions keep coming, status once", ["df11"] + P + ["op_status", ("every", 5, 100, P + ["velocity"])]),
]
rows = {}
def play(i, label, steps):
    hexa = "%06X" % (0x4D0300 + i)
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
