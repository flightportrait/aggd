#!/usr/bin/env python3
"""How MLAT and JAERO results mix with direct reception: the same
scenarios played to a reference and a candidate, one address each.

    src_probe.py REF_BEAST REF_JSON CAND_BEAST CAND_JSON MLAT_PORT JAERO_PORT

Both aggregators must connect to MLAT_PORT (sbs_in_mlat) and JAERO_PORT
(sbs_in_jaero) on this host; the script waits for two connections on each.
"""
import json, os, socket, subprocess, sys, threading, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

def hp(s):
    h, p = s.rsplit(":", 1); return h, int(p)

def beast(frame, ticks):
    body = ticks.to_bytes(6, "big") + bytes([120]) + frame
    return b"\x1a" + (b"2" if len(frame) == 7 else b"3") + body.replace(b"\x1a", b"\x1a\x1a")

def load(src):
    try:
        d = json.load(urllib.request.urlopen(src, timeout=5)) if src.startswith("http") else json.load(open(src))
        return {a["hex"]: a for a in d["aircraft"]}
    except Exception as e:
        return {}

def frames(hexa):
    out = subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa], capture_output=True, text=True, check=True).stdout
    return {l.split()[0]: bytes.fromhex(l.split()[1]) for l in out.split("\n") if l}

class Fan:
    """Listen and send each line to everyone connected."""
    def __init__(self, port):
        self.conns = []; self.l = socket.socket(); self.l.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.l.bind(("127.0.0.1", port)); self.l.listen(8)
        threading.Thread(target=self.run, daemon=True).start()
    def run(self):
        while True:
            c, _ = self.l.accept(); self.conns.append(c)
    def send(self, line):
        for c in list(self.conns):
            try: c.sendall(line.encode() + b"\r\n")
            except OSError: self.conns.remove(c)

def sbs(hexa, lat, lon, alt=30000):
    t = time.time(); d = time.strftime("%Y/%m/%d", time.gmtime(t)); h = time.strftime("%H:%M:%S", time.gmtime(t)) + ".%03d" % (t % 1 * 1000)
    return f"MSG,3,1,1,{hexa},1,{d},{h},{d},{h},,{alt},,,{lat:.5f},{lon:.5f},,,0,,0,0"

# name: (hex, [(kind, from second, until, every)]); kinds: modes (DF11 and
# DF4), adsb (positions, velocity, DF11), es (velocity and DF11 only),
# mlat and jaero (a BaseStation position line on that port)
E = 999
S = {
    "modes+mlat":                 ("A00001", [("modes", 0, E, 1), ("mlat", 3, E, 2)]),
    "mlat only":                  ("A00002", [("mlat", 3, E, 2)]),
    "jaero only":                 ("A00006", [("jaero", 10, 10, 1)]),
    "jaero only ~":               ("~A00007", [("jaero", 10, 10, 1)]),
    "modes+jaero":                ("A00005", [("modes", 0, E, 1), ("jaero", 10, 10, 1)]),
    "adsb+mlat":                  ("A00008", [("adsb", 0, E, 1), ("mlat", 3, E, 2)]),
    "adsb+jaero":                 ("A00003", [("adsb", 0, E, 1), ("jaero", 10, E, 3)]),
    "adsb all stops@5, mlat":     ("A00010", [("adsb", 0, 5, 1), ("mlat", 3, E, 2)]),
    "adsb all stops@5, jaero":    ("A00011", [("adsb", 0, 5, 1), ("jaero", 6, E, 3)]),
    "adsb pos stops@5, mlat":     ("A00012", [("adsb", 0, 5, 1), ("es", 6, E, 1), ("mlat", 3, E, 2)]),
    "adsb pos stops@5, jaero":    ("A00013", [("adsb", 0, 5, 1), ("es", 6, E, 1), ("jaero", 6, E, 3)]),
    "mlat stops@12, jaero":       ("A00014", [("modes", 0, E, 1), ("mlat", 3, 12, 2), ("jaero", 14, E, 3)]),
    "jaero@10, adsb from 20":     ("A00015", [("modes", 0, 19, 1), ("jaero", 10, 10, 1), ("adsb", 20, E, 1)]),
    "mlat, adsb from 20":         ("A00016", [("modes", 0, 19, 1), ("mlat", 3, E, 2), ("adsb", 20, E, 1)]),
    "jaero@10, mlat from 20":     ("A00017", [("modes", 0, E, 1), ("jaero", 10, 10, 1), ("mlat", 20, E, 2)]),
    "mlat+jaero":                 ("A00009", [("modes", 0, E, 1), ("mlat", 3, E, 2), ("jaero", 20, E, 3)]),
}

def state(a):
    if a is None: return "absent"
    src = "mlat" if "lat" in a.get("mlat", []) else ("none" if a.get("lat") is None else ("ext" if abs(a["lat"] - 45.52) < 0.005 else "own"))
    return f"{a.get('type')}/{src}"

def main():
    rb, rj, cb, cj, mp, jp = sys.argv[1:7]
    length = int(sys.argv[7]) if len(sys.argv) > 7 else 100
    fan = {"mlat": Fan(int(mp)), "jaero": Fan(int(jp))}
    t = time.time()
    while any(len(f.conns) < 2 for f in fan.values()) and time.time() - t < 90: time.sleep(0.5)
    socks = [socket.create_connection(hp(x)) for x in (rb, cb)]
    fr = {n: frames(h.lstrip("~")) for n, (h, _) in S.items()}
    t0 = time.time(); sec = 0; last = {}; log = {n: [] for n in S}
    while sec <= length:
        if time.time() - t0 < sec:
            time.sleep(0.02); continue
        ticks = int(time.time() * 12e6) & 0xFFFFFFFFFFFF; buf = b""
        for n, (h, parts) in S.items():
            f = fr[n]
            for kind, start, until, every in parts:
                if not (start <= sec <= until and (sec - start) % every == 0): continue
                if kind == "modes": buf += beast(f["df11"], ticks) + beast(f["df4"], ticks + 1200)
                elif kind == "adsb": buf += beast(f["pos_odd" if sec % 2 else "pos_even"], ticks) + beast(f["velocity"], ticks + 1200) + beast(f["df11"], ticks + 2400)
                elif kind == "es": buf += beast(f["velocity"], ticks) + beast(f["df11"], ticks + 1200)
                else: fan[kind].send(sbs(h, 45.52, 9.25))
        for s in socks: s.sendall(buf)
        time.sleep(0.6)
        r, c = load(rj), load(cj)
        for n, (h, _) in S.items():
            now = (state(r.get(h.lower())), state(c.get(h.lower())))
            if last.get(n) != now:
                log[n].append((sec, now)); last[n] = now
        sec += 1
    bad = 0
    for n in S:
        ref = [(t, x[0]) for i, (t, x) in enumerate(log[n]) if i == 0 or log[n][i - 1][1][0] != x[0]]
        cand = [(t, x[1]) for i, (t, x) in enumerate(log[n]) if i == 0 or log[n][i - 1][1][1] != x[1]]
        ref, cand = ([x for x in l if x[1] != "absent"] for l in (ref, cand))
        # the same sequence of states, each within two seconds
        same = [x for _, x in ref] == [x for _, x in cand] and all(abs(a[0] - b[0]) <= 2 for a, b in zip(ref, cand))
        bad += not same
        show = lambda l: "  ".join(f"{t}s {x}" for t, x in l)
        print(f"{'same' if same else 'DIFF'} {n}\n      ref:  {show(ref)}" + ("" if same else f"\n      cand: {show(cand)}"))
    print(f"{len(S) - bad} of {len(S)} scenarios agree")
    sys.exit(1 if bad else 0)

main()
