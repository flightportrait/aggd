#!/usr/bin/env python3
"""Send one frame of each kind to a reference and a candidate aggregator
and show where their outputs differ after each.

    probe.py REF_BEAST REF_HTTP_OR_FILE REF_SBS  CAND_BEAST CAND_HTTP CAND_SBS [--hex ADDR]

Each *_BEAST and *_SBS is host:port; REF_HTTP_OR_FILE is an aircraft.json
URL or file. Frames come from `aggbench frames`.
"""
import json, socket, subprocess, sys, threading, time, urllib.request, os

HERE = os.path.dirname(os.path.abspath(__file__))
SKIP = {"seen", "seen_pos", "messages", "rssi", "r_dst", "r_dir"}

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
        return {"error": str(e)}

class Sbs(threading.Thread):
    def __init__(self, addr):
        super().__init__(daemon=True); self.lines = []; self.s = socket.create_connection(hp(addr)); self.start()
    def run(self):
        buf = b""
        while True:
            d = self.s.recv(65536)
            if not d: return
            buf += d
            *ls, buf = buf.split(b"\n")
            self.lines += [l.decode().strip() for l in ls]
    def take(self):
        l, self.lines = self.lines, []; return l

def main():
    args = sys.argv[1:]
    if "--only" in args:
        i = args.index("--only"); del args[i:i + 2]
    hexa = "4CA2D6"
    if "--hex" in args:
        i = args.index("--hex"); hexa = args[i + 1]; del args[i:i + 2]
    rb, rh, rs, cb, ch, cs = args
    frames = subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa], capture_output=True, text=True, check=True).stdout.split("\n")
    frames = [l.split() for l in frames if l]
    socks = [socket.create_connection(hp(x)) for x in (rb, cb)]
    hello = b"\x1a\xe4" + b"0badc0de-0000-4000-8000-00000000beef" + b"\x1aWO"
    for s in socks: s.sendall(hello)
    sbs = [Sbs(rs), Sbs(cs)]
    time.sleep(0.5)
    t0 = time.time(); bad = 0; known = set()
    only = None
    if "--only" in sys.argv: only = sys.argv[sys.argv.index("--only") + 1].split(",")
    for i, (name, fx) in enumerate(frames):
        if only and name.lstrip("+") not in only: continue
        f = bytes.fromhex(fx)
        msg = beast(f, int((time.time() - t0) * 12e6))
        for s in socks: s.sendall(msg)
        # a name starting with + follows its predecessor at once
        if i + 1 < len(frames) and frames[i + 1][0].startswith("+"):
            time.sleep(0.05); continue
        time.sleep(1.6)   # both refresh their JSON once a second
        A, B = load(rh), load(ch)
        diffs = []
        for h in sorted(set(A) | set(B)):
            x, y = A.get(h), B.get(h)
            if x is None or y is None:
                diffs.append(f"{h}: only in {'reference' if y is None else 'candidate'}"); continue
            if not isinstance(x, dict): diffs.append(f"{h}: {x} / {y}"); continue
            for k in sorted((set(x) | set(y)) - SKIP):
                if x.get(k) != y.get(k): diffs.append(f"{h}.{k}: ref {x.get(k)!r} cand {y.get(k)!r}")
        def strip(l):
            p = l.split(","); return ",".join(p[:6] + p[10:])   # drop the timestamps
        la, lb = [strip(l) for l in sbs[0].take()], [strip(l) for l in sbs[1].take()]
        if la != lb: diffs.append(f"sbs: ref {la} cand {lb}")
        new = [d for d in diffs if d not in known]; known = set(diffs)
        bad += bool(diffs)
        print(f"{'DIFF' if diffs else 'ok  '} {name}" + (f"  ({len(diffs) - len(new)} carried over)" if len(diffs) > len(new) else ""))
        for d in new: print("       " + d)
    print(f"{len(frames) - bad} of {len(frames)} steps agree")
    return 1 if bad else 0

sys.exit(main())
