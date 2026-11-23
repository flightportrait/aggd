#!/usr/bin/env python3
"""Which BDS 5,0 replies does an aggregator take? Send an aircraft with
ADS-B position and velocity, then one Comm-B reply shaped as BDS 5,0 with
some fields marked absent, and see whether airspeed and roll appear.

    bds50_probe.py BEAST_HOST:PORT AIRCRAFT_JSON_URL_OR_FILE
"""
import json, os, socket, subprocess, sys, time, urllib.request
HERE = os.path.dirname(os.path.abspath(__file__))
host, port = sys.argv[1].rsplit(":", 1); src = sys.argv[2]

def crc(b):
    r = 0
    for x in b:
        r ^= x << 16
        for _ in range(8):
            r = (r << 1) ^ (0xFFF409 if r & 0x800000 else 0)
    return r & 0xFFFFFF

def mb50(roll, track, gs, rate, tas):
    """Each argument is a value or None (status bit 0, field zero)."""
    def f(v, lsb, bits, signed):
        if v is None: return 0
        n = round(v / lsb)
        if signed: n &= (1 << bits) - 1
        return 1 << bits | n
    if track is not None and track > 180: track -= 360
    v = f(roll, 45 / 256, 10, True)
    v = v << 12 | f(track, 90 / 512, 11, True)
    v = v << 11 | f(gs, 2, 10, False)
    v = v << 11 | f(rate, 8 / 256, 10, True)
    v = v << 11 | f(tas, 2, 10, False)
    return v.to_bytes(7, "big")

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

s = socket.create_connection((host, int(port))); t0 = time.time(); n = [0x4D0200]
def send(f): s.sendall(beast(f, time.time() - t0)); time.sleep(0.05)
def run(label, mb, df=20, before=("df11", "pos_even", "pos_odd", "velocity")):
    n[0] += 1; hexa = "%06X" % n[0]
    fr = dict(l.split() for l in subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa], capture_output=True, text=True, check=True).stdout.split("\n") if l)
    for st in before: send(bytes.fromhex(fr[st]))
    time.sleep(0.3)
    head = bytes.fromhex(fr["df20_bds50"])[:4] if df == 20 else bytes.fromhex(fr["df21_bds40"])[:4]
    body = head + mb
    send(body + (crc(body) ^ n[0]).to_bytes(3, "big"))
    time.sleep(1.6)
    a = load(hexa)
    print(f"{label:52s} tas {str(a.get('tas')):5s} roll {str(a.get('roll')):6s} track_rate {str(a.get('track_rate')):5s} gs {a.get('gs')}", flush=True)

# the ADS-B velocity says 440 kt on 250 degrees
run("all five fields", mb50(-1, 250, 440, 0, 450))
run("no track rate", mb50(-1, 250, 440, None, 450))
run("no track rate, roll -7", mb50(-7.4, 250, 440, None, 450))
run("no track rate, in a DF21", mb50(-7.4, 250, 440, None, 450), df=21)
run("no roll", mb50(None, 250, 440, 0, 450))
run("no airspeed", mb50(-1, 250, 440, 0, None))
run("no track rate, nothing known of the aircraft", mb50(-1, 250, 440, None, 450), before=("df11",))
run("all five, nothing known", mb50(-1, 250, 440, 0, 450), before=("df11",))
run("no track rate, speed 30 kt off ADS-B", mb50(-1, 250, 470, None, 480))
run("all five, track rate 0.5", mb50(-1, 250, 440, 0.5, 450))
run("all five, roll -12 rate 0.5", mb50(-12, 250, 440, 0.5, 450))
run("all five, roll -7 rate 0", mb50(-7.4, 250, 440, 0, 450))
