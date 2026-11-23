#!/usr/bin/env python3
"""When does an aggregator derive wind and temperature from Comm-B
replies that arrive apart? Each history runs on its own address, over a
steady background of ADS-B positions and velocities, and what is shown is
printed as it changes.

    wind_probe2.py BEAST_HOST:PORT AIRCRAFT_JSON_URL_OR_FILE
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
B5, B6, VA = "df20_bds50", "df20_bds60", "velocity_air"
H = []
V, PE, PO = "velocity", "pos_even", "pos_odd"
if os.environ.get("SET", "1") == "1":
    for d in (0.3, 1, 1.4, 2, 3, 5, 8, 12, 20, 40):
        H.append((f"5,0 then {d} s then 6,0", 0.5, [B5, d, B6, 75]))
    for d in (0.3, 1, 2, 3, 5, 8, 12, 20, 40):
        H.append((f"6,0 then {d} s then 5,0", 0.5, [B6, d, B5, 75]))
    for bg in (2, 4, 8):
        H.append((f"background every {bg} s; 5,0 6,0 together", bg, [B5, B6, 75]))
    H.append(("airspeed message once", 0.5, [VA, 75]))
    H.append(("5,0 every 4 s, 6,0 once", 0.5, [B6] + [B5, 4] * 20))
    H.append(("6,0 every 4 s, 5,0 once", 0.5, [B5] + [B6, 4] * 20))
    H.append(("both every 10 s, 5 s apart", 0.5, [B5, 5, B6, 5] * 8))
    H.append(("both every 4 s, 2 s apart", 0.5, [B5, 2, B6, 2] * 20))
    H.append(("no background after start; 5,0 6,0 after 5 s", 0, [5, B5, B6, 75]))
    H.append(("no background after start; 5,0 6,0 after 20 s", 0, [20, B5, B6, 75]))
elif os.environ["SET"] == "3":
    for d in (1.05, 1.1, 1.15, 1.2, 1.25, 1.3, 1.4):
        H.append((f"quiet; position, velocity, {d} s, airspeed", 0, [PE, PO, V, d, VA, 4]))
    for g in (9, 10, 11, 12, 13, 14):
        H.append((f"quiet {g} s then altitude reply twice", 0, [g, "df4", 0.5, "df4", 4]))
    H.append(("altitude reply every 8 s for 90 s", 0, ["df4", 8] * 11))
    H.append(("5,0 every 8 s for 90 s", 0, [B5, 8] * 11))
    H.append(("altitude reply every 8 s, all-call reply every 40 s", 0, (["df4", 8] * 4 + ["df11", 8]) * 3))
elif os.environ["SET"] == "4":
    B5 = "+bds50_b"  # the same reply with wings level
    for d in (0.3, 1, 1.5, 2, 2.4, 3, 5, 8):
        H.append((f"6,0 then {d} s then 5,0", 0.5, [B6, d, B5, 6]))
        H.append((f"5,0 then {d} s then 6,0", 0.5, [B5, d, B6, 6]))
        H.append((f"quiet; velocity, {d} s, 6,0 5,0", 0, [V, d, B6, B5, 6]))
        H.append((f"quiet; velocity, {d} s, 5,0 6,0", 0, [V, d, B5, B6, 6]))
    for bg in (2, 4, 8):
        H.append((f"background every {bg} s; 6,0 5,0 together", bg, [B6, B5, 6]))
    H.append(("together once, then background only", 0.5, [B6, B5, 150]))
    H.append(("together once, then 5,0 every 4 s", 0.5, [B6] + [B5, 4] * 38))
    H.append(("together once, then 6,0 every 4 s", 0.5, [B5] + [B6, 4] * 38))
    H.append(("5,0 and 6,0 alternating 4 s apart", 0.5, [B5, 4, B6, 4] * 19))
    H.append(("5,0 with wings banked, 6,0", 0.5, ["df20_bds50", B6, 6]))
elif os.environ["SET"] == "5":
    for r in (0, 2, 5, 6, 8, 8.5, 9, 9.3, 9.6, 9.9, 10, 10.2, 12, -6, -9, -9.9, -10.2):
        H.append((f"roll {r}, track rate 0", 0.5, [B6, B5, 6], ["--roll", str(r), "--track-rate", "0"]))
    for tr in (0.125, 0.25, 0.5, 1, 2, -0.5):
        H.append((f"roll 0, track rate {tr}", 0.5, [B6, B5, 6], ["--roll", "0", "--track-rate", str(tr)]))
elif os.environ["SET"] == "6":
    K = ["df11", 5]
    H.append(("position, 1 s, 6,0", 0, [PE, PO, 1, B6, 6]))
    H.append(("6,0, 2 s, position", 0, [B6, 2, PE, PO, 6]))
    H.append(("6,0, position, 6,0", 0, [B6, 2, PE, PO, 2, B6, 6]))
    for g in (20, 50, 70, 100, 150):
        H.append((f"position, {g} s, 6,0", 0, [PE, PO] + K * (g // 5) + [B6, 6]))
    H.append(("position, 6,0 once, positions go on", 0.5, [B6, 80]))
    H.append(("position, 6,0 once, no more positions", 0, [PE, PO, B6] + K * 16))
    H.append(("no position ever, 6,0", 0, [B6] + K * 3))
elif os.environ["SET"] == "7":
    # how far apart may an even and an odd position message be and still pair?
    for d in (8, 12, 15, 17, 20, 24, 28, 35, 45, 55):
        k = ["df11", 1] * int(d - 1) + [1]
        H.append((f"even, {d} s, odd; no speed known", 0, [PE] + k + [PO, 5]))
        H.append((f"even, {d} s, odd; 440 kt", 0, [V, PE] + k + [PO, 5]))
else:
    for d in (2.2, 2.4, 2.6, 2.8):
        H.append((f"5,0 then {d} s then 6,0", 0.5, [B5, d, B6, 8]))
        H.append((f"6,0 then {d} s then 5,0", 0.5, [B6, d, B5, 8]))
    for d in (0.5, 1, 1.3, 1.6, 2, 2.4, 2.8, 3.5, 5, 10):
        H.append((f"quiet; position, velocity, {d} s, airspeed", 0, [PE, PO, V, d, VA, 6]))
        H.append((f"quiet; position, airspeed, {d} s, velocity", 0, [PE, PO, VA, d, V, 6]))
        H.append((f"quiet; velocity, airspeed, {d} s, position", 0, [V, VA, d, PE, PO, 6]))
        H.append((f"quiet; {d} s, velocity, airspeed", 0, [d, V, VA, 6]))
    for g in (5, 10, 15, 20, 25, 30, 40, 50):
        H.append((f"quiet {g} s then altitude reply twice", 0, [g, "df4", 0.5, "df4", 6]))
        H.append((f"quiet {g} s then 5,0 6,0", 0, [g, B5, B6, 6]))
rows = {}
def play(i, label, bg, steps, extra=()):
    hexa = "%06X" % (int(os.environ.get("BASE","4D0700"),16) + i)
    fr = dict(l.split() for l in subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa, *extra], capture_output=True, text=True, check=True).stdout.split("\n") if l)
    rows[hexa.lower()] = label
    stop = threading.Event()
    for f in (("df11", "df11") if os.environ.get("SET") in ("6", "7") else ("df11", "pos_even", "pos_odd", "velocity", "df11")): send(fr[f]); time.sleep(0.05)
    def back():
        k = 0
        while not stop.wait(bg):
            send(fr["pos_even" if k % 2 == 0 else "pos_odd"]); send(fr["velocity"]); k += 1
    if bg: threading.Thread(target=back, daemon=True).start()
    time.sleep(3)
    for st in steps:
        if isinstance(st, (int, float)): time.sleep(st)
        else: send(fr.get(st) or fr[st.lstrip("+")])
    stop.set()
th = [threading.Thread(target=play, args=(i, *h), daemon=True) for i, h in enumerate(H)]
for t in th: t.start()
last = {}
while any(t.is_alive() for t in th):
    time.sleep(0.5); d = load()
    for hexa, label in sorted(rows.items()):
        a = d.get(hexa, {})
        v = ("wind" if "wd" in a else "-", "temp" if "oat" in a else "-", "tas" if "tas" in a else "-", "mach" if "mach" in a else "-", "true" if "true_heading" in a else "-", "mag" if "mag_heading" in a else "-", "pos" if "lat" in a else "-")
        if last.get(hexa, ("-", "-")) != v:
            last[hexa] = v
            print(f"{time.time()-t0:6.1f}s {label:48s} {' '.join(v)}", flush=True)
for hexa, label in sorted(rows.items()):
    if hexa not in last: print(f"   never {label}")
