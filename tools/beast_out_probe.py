#!/usr/bin/env python3
"""Are the Beast outputs the same stream readsb gives? Send the same
frames to a reference and a candidate and compare what comes out of
their full and reduced Beast ports: frame type, 12 MHz timestamp, signal
byte and payload.

    beast_out_probe.py REF_IN REF_FULL REF_REDUCED  CAND_IN CAND_FULL CAND_REDUCED
"""
import os, socket, subprocess, sys, threading, time
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
LEN = {0x31: 2, 0x32: 7, 0x33: 14}

def hp(s):
    h, p = s.rsplit(":", 1); return h, int(p)

def beast(frame, ticks, sig):
    body = ticks.to_bytes(6, "big") + bytes([sig]) + frame
    return b"\x1a" + (b"2" if len(frame) == 7 else b"3") + body.replace(b"\x1a", b"\x1a\x1a")

class Tap(threading.Thread):
    def __init__(self, addr):
        super().__init__(daemon=True); self.buf = b""; self.s = socket.create_connection(hp(addr)); self.start()
    def run(self):
        while True:
            d = self.s.recv(65536)
            if not d: return
            self.buf += d
    def frames(self):
        """[(type, ticks, signal, payload hex)], other = bytes not understood"""
        b, i, out, other = self.buf, 0, [], 0
        while i < len(b):
            if b[i] != 0x1A or i + 1 >= len(b):
                other += 1; i += 1; continue
            t = b[i + 1]; i += 2
            if t not in LEN:
                out.append((t, None, None, "")); continue
            need, body = 7 + LEN[t], bytearray()
            while len(body) < need and i < len(b):
                if b[i] == 0x1A:
                    if i + 1 < len(b) and b[i + 1] == 0x1A: i += 1
                    else: break
                body.append(b[i]); i += 1
            if len(body) == need:
                out.append((t, int.from_bytes(body[:6], "big"), body[6], body[7:].hex()))
            else:
                other += 1
        return out, other

def main():
    ri, rf, rr, ci, cf, cr = sys.argv[1:7]
    taps = {"full": (Tap(rf), Tap(cf)), "reduced": (Tap(rr), Tap(cr))}
    ins = [socket.create_connection(hp(x)) for x in (ri, ci)]
    time.sleep(1)
    sent = 0
    for sec in range(12):
        buf = b""
        for k, hexa in enumerate(("4CA2D6", "A1B2C3", "3C1A1A")):   # the last has 0x1A bytes to escape
            out = subprocess.run([os.path.join(HERE, "../target/release/aggbench"), "frames", "--hex", hexa], capture_output=True, text=True, check=True).stdout
            for j, l in enumerate(x for x in out.split("\n") if x):
                ticks = (sec * 12_000_000 + k * 1_000_000 + j * 26_001 + 0x1A1A) & 0xFFFFFFFFFFFF
                buf += beast(bytes.fromhex(l.split()[1]), ticks, 26 + (sec + j) % 200); sent += 1
        for s in ins: s.sendall(buf)
        time.sleep(1)
    time.sleep(2)
    bad = 0
    for name, (r, c) in taps.items():
        (rf_, ro), (cf_, co) = r.frames(), c.frames()
        data = lambda l: Counter(x for x in l if x[0] in LEN)
        kinds = lambda l: dict(Counter("0x%02x" % x[0] for x in l))
        rd, cd = data(rf_), data(cf_)
        only_r, only_c = rd - cd, cd - rd
        print(f"{name}: sent {sent}; reference gave {sum(rd.values())} frames, candidate {sum(cd.values())}; identical (type, timestamp, signal, payload): {sum((rd & cd).values())}")
        print(f"  message types  reference {kinds(rf_)}  candidate {kinds(cf_)}; stray bytes {ro} / {co}")
        if name == "full":
            bad += bool(only_r or only_c)
            for x in list(only_r)[:5]: print("  only reference:", x)
            for x in list(only_c)[:5]: print("  only candidate:", x)
        else:
            # which messages a reduced stream keeps is each program's choice; what must
            # hold is that every frame in it is one that was sent, unaltered
            payload = lambda d: Counter({(x[0], x[3]): n for x, n in d.items()})
            print(f"  by payload: in both {sum((payload(rd) & payload(cd)).values())}, only reference {sum((payload(rd) - payload(cd)).values())}, only candidate {sum((payload(cd) - payload(rd)).values())}")
            ts = lambda d: Counter(x[1] for x in d)
            print(f"  timestamps the reference uses, top: {ts(rd).most_common(3)}; candidate: {ts(cd).most_common(3)}")
    sys.exit(bad)

main()
