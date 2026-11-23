#!/usr/bin/env python3
"""Find which ICAO address blocks a reference readsb flags as military
when the airframe database has no entry: show it two all-call replies
from one address in every block of 256 and read dbFlags back.

    milranges.py BEAST_HOST:PORT AIRCRAFT_JSON_FILE_OR_URL [STEP]
"""
import json, socket, sys, time, urllib.request

def crc(data):
    c = 0
    for b in data:
        c ^= b << 16
        for _ in range(8):
            c = ((c << 1) ^ 0xFFF409) if c & 0x800000 else (c << 1)
        c &= 0xFFFFFF
    return c

def df11(addr):
    f = bytes([0x5D]) + addr.to_bytes(3, "big")
    return f + crc(f).to_bytes(3, "big")

def beast(frame):
    body = b"\0\0\0\0\0\1\x50" + frame
    return b"\x1a2" + body.replace(b"\x1a", b"\x1a\x1a")

def dbhit_set(l, cache={}):
    if id(l) not in cache: cache[id(l)] = set(l)
    return cache[id(l)]

def main():
    host, port = sys.argv[1].rsplit(":", 1); src = sys.argv[2]
    step = int(sys.argv[3]) if len(sys.argv) > 3 else 256
    probe = list(range(step // 2, 1 << 24, step))
    s = socket.create_connection((host, int(port)))
    flagged = set(); seen = set(); indb = set()
    todo = probe
    for attempt in range(8):
        CH = 5000
        for i in range(0, len(todo), CH):
            part = todo[i:i + CH]
            for _ in range(3):
                s.sendall(b"".join(beast(df11(a)) for a in part)); time.sleep(0.7)
            time.sleep(1.6)
            d = json.load(urllib.request.urlopen(src)) if src.startswith("http") else json.load(open(src))
            for a in d["aircraft"]:
                h = a["hex"]
                if h.startswith("~"): continue
                v = int(h, 16); seen.add(v)
                if "r" in a or "t" in a: indb.add(v)
                elif a.get("dbFlags", 0) & 1: flagged.add(v)
            print(f"\rpass {attempt}: {i + len(part)}/{len(todo)}, {len(seen)} listed, {len(flagged)} flagged   ", end="", file=sys.stderr)
        todo = [a for a in probe if a not in seen]
        if not todo: break
    print(file=sys.stderr)
    # addresses in the database are flagged from their own entry, not the range
    dbhit = [a for a in probe if a in indb]
    json.dump({"step": step, "flagged": sorted(flagged), "indb": sorted(indb)}, open("milranges.raw.json", "w"))
    # F flagged by range, D in the database (the range rule is hidden), N not flagged
    state = {a // step: ("F" if a in flagged else "D" if a in dbhit_set(dbhit) else "N") for a in probe}
    runs = []; cur = None
    for b in sorted(state):
        st = state[b]
        if st == "F":
            if cur is None: cur = [b, b]
            cur[1] = b
        elif st == "N" and cur is not None:
            runs.append(cur); cur = None
    if cur: runs.append(cur)
    for lo, hi in runs: print("%06x %06x" % (lo * step, hi * step + step - 1))
    print(f"{len(seen)} of {len(probe)} probes were listed", file=sys.stderr)

main()
