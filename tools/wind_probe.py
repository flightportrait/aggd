#!/usr/bin/env python3
"""Find when an aggregator derives wind: vary how far apart in time the
inputs arrive (position, ground speed and track, airspeed and heading).

    wind_probe.py BEAST_HOST:PORT AIRCRAFT_JSON_URL_OR_FILE
"""
import json, os, socket, subprocess, sys, time, urllib.request
HERE = os.path.dirname(os.path.abspath(__file__))
host, port = sys.argv[1].rsplit(":", 1); src = sys.argv[2]

def beast(frame, t):
    body = int(t * 12e6).to_bytes(6, "big") + bytes([120]) + frame
    return b"\x1a" + (b"2" if len(frame) == 7 else b"3") + body.replace(b"\x1a", b"\x1a\x1a")

def load(hexa):
    for _ in range(3):
        try:
            d = json.load(urllib.request.urlopen(src, timeout=5)) if src.startswith("http") else json.load(open(src))
            for a in d["aircraft"]:
                if a["hex"] == hexa.lower(): return a
            return {}
        except Exception:
            time.sleep(0.2)
    return {}

s = socket.create_connection((host, int(port))); t0 = time.time(); n = [0x4D0100]
def run(label, steps):
    """steps: list of frame names and numbers (seconds to wait)."""
    n[0] += 1; hexa = "%06X" % n[0]
    fr = dict(l.split() for l in subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa], capture_output=True, text=True, check=True).stdout.split("\n") if l)
    for st in steps:
        if isinstance(st, (int, float)): time.sleep(st)
        else: s.sendall(beast(bytes.fromhex(fr[st.lstrip("+")]), time.time() - t0)); time.sleep(0.05)
    time.sleep(1.6)
    a = load(hexa)
    print(f"{label:58s} wind {'yes ' + str((a.get('wd'), a.get('ws'))) if 'ws' in a else 'no'}   temp {'yes' if 'oat' in a else 'no'}", flush=True)

P = ["df11", "pos_even", "pos_odd"]; V = ["velocity"]; A = ["velocity_air"]
for d in (0.2, 1, 2, 3, 5, 8, 12, 20):
    run(f"position, speed+track, wait {d}, airspeed+heading", P + V + [d] + A)
for d in (1, 3, 8, 20):
    run(f"position, airspeed+heading, wait {d}, speed+track", P + A + [d] + V)
for d in (1, 3, 8, 20, 40):
    run(f"position, wait {d}, speed+track, airspeed+heading", P + [d] + V + A)
for d in (1, 3, 8, 20):
    run(f"speed+track, airspeed+heading, wait {d}, position", ["df11"] + V + A + [d] + ["pos_even", "pos_odd"])
run("Comm-B: position, speed+track, BDS 6,0, BDS 5,0", P + V + ["df20_bds60", "df20_bds50"])
run("Comm-B only: position, BDS 5,0, BDS 6,0", P + ["df20_bds50", "df20_bds60"])
for d in (3, 8, 20):
    run(f"position, BDS 5,0, wait {d}, BDS 6,0", P + ["df20_bds50", d, "df20_bds60"])
