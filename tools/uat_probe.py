#!/usr/bin/env python3
"""Build UAT (978 MHz) ADS-B frames from the DO-282B field layout, send
them as dump978 raw lines to one or two aggregators' UAT input ports,
and print what each makes of them.

    uat_probe.py HOST:PORT JSON_URL_OR_FILE [HOST:PORT JSON_URL_OR_FILE]
"""
import json, socket, sys, time, urllib.request, math

B40 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ  .."

def frame(addr, lat, lon, alt_ft, gs, track, vrate, callsign, category=1, aq=0, mdb=1, ground=False, squawk=None, geo_alt=None):
    f = bytearray(34)
    f[0] = mdb << 3 | aq
    f[1:4] = addr.to_bytes(3, "big")
    rlat = int(round((lat if lat >= 0 else lat + 180 + 180) / (360 / 2**24))) & 0x7FFFFF if lat >= 0 else int(round((lat + 360) / (360 / 2**24))) & 0xFFFFFF
    rlat &= 0x7FFFFF
    rlon = int(round((lon % 360) / (360 / 2**24))) & 0xFFFFFF
    f[4] = rlat >> 15; f[5] = rlat >> 7 & 0xFF; f[6] = (rlat & 0x7F) << 1 | rlon >> 23
    f[7] = rlon >> 15 & 0xFF; f[8] = rlon >> 7 & 0xFF; f[9] = (rlon & 0x7F) << 1   # altitude type 0: pressure
    ralt = (alt_ft + 1000) // 25 + 1
    f[10] = ralt >> 4; f[11] = (ralt & 0xF) << 4 | 9          # NIC 9
    if not ground:
        ns = gs * math.cos(math.radians(track)); ew = gs * math.sin(math.radians(track))
        rns = (0x400 if ns < 0 else 0) | (int(round(abs(ns))) + 1); rew = (0x400 if ew < 0 else 0) | (int(round(abs(ew))) + 1)
        f[12] = 0 << 6 | rns >> 6; f[13] = (rns & 0x3F) << 2 | rew >> 9; f[14] = rew >> 1 & 0xFF
        rv = 0x400 | (0x200 if vrate < 0 else 0) | (abs(vrate) // 64 + 1)   # barometric source
        f[15] = (rew & 1) << 7 | rv >> 4; f[16] = (rv & 0xF) << 4
    else:
        rgs = int(gs) + 1; rtrk = 1 << 9 | int(round(track * 512 / 360)) & 0x1FF     # true track
        f[12] = 2 << 6 | rgs >> 6; f[13] = (rgs & 0x3F) << 2 | rtrk >> 9; f[14] = rtrk >> 1 & 0xFF; f[15] = (rtrk & 1) << 7
    # mode status
    cs = (callsign + "        ")[:8] if squawk is None else ("%04o" % squawk if isinstance(squawk, int) else squawk).ljust(8)
    idx = [B40.index(c) for c in cs]
    for k, (a, b, c) in enumerate([(category, idx[0], idx[1]), (idx[2], idx[3], idx[4]), (idx[5], idx[6], idx[7])]):
        v = a * 1600 + b * 40 + c; f[17 + 2 * k] = v >> 8; f[18 + 2 * k] = v & 0xFF
    f[23] = 0 << 5 | 2 << 2 | 3          # no emergency, UAT version 2, SIL 3
    f[24] = 20 << 2                      # transmit MSO
    f[25] = 10 << 4 | 2 << 1 | 1         # NACp 10, NACv 2, NIC baro
    f[26] = 0x02 if squawk is None else 0   # the callsign field holds a callsign (CSID)
    if geo_alt is not None:
        r = (geo_alt + 1000) // 25 + 1; f[29] = r >> 4; f[30] = (r & 0xF) << 4
    return bytes(f)

def load(src, hexes):
    try:
        d = json.load(urllib.request.urlopen(src, timeout=5)) if src.startswith("http") else json.load(open(src))
        return {a["hex"]: a for a in d["aircraft"] if a["hex"].lstrip("~") in hexes}
    except Exception as e:
        return {"error": str(e)}

def main():
    targets = [(sys.argv[i], sys.argv[i + 1]) for i in range(1, len(sys.argv), 2)]
    socks = [socket.create_connection((t[0].rsplit(":", 1)[0], int(t[0].rsplit(":", 1)[1]))) for t in targets]
    cases = [
        ("airborne, ICAO address", dict(addr=0xA1B2C3, lat=40.5, lon=-74.25, alt_ft=5500, gs=140, track=75, vrate=-640, callsign="N123AB", category=1, geo_alt=5700)),
        ("on the ground", dict(addr=0xA1B2C4, lat=40.6, lon=-74.20, alt_ft=25, gs=12, track=270, vrate=0, callsign="N456CD", category=1, ground=True)),
        ("squawk in place of callsign", dict(addr=0xA1B2C5, lat=40.7, lon=-74.10, alt_ft=3000, gs=100, track=180, vrate=0, callsign="", squawk="1200")),
        ("TIS-B target", dict(addr=0xA1B2C6, lat=40.8, lon=-74.00, alt_ft=8000, gs=200, track=300, vrate=1280, callsign="TIS1", aq=2)),
        ("rebroadcast (ADS-R)", dict(addr=0xA1B2C7, lat=40.9, lon=-73.90, alt_ft=9000, gs=220, track=10, vrate=0, callsign="ADSR1", aq=6)),
        ("southern, eastern hemisphere", dict(addr=0xA1B2C8, lat=-33.9, lon=151.2, alt_ft=12000, gs=250, track=45, vrate=0, callsign="VHABC")),
    ]
    for rep in range(4):
        for _, kw in cases:
            line = ("-" + frame(**kw).hex() + ";rs=0;rssi=-12.0;t=%.3f;\n" % time.time()).encode()
            for s in socks: s.sendall(line)
        time.sleep(0.6)
    time.sleep(1.6)
    hexes = {"%06x" % c[1]["addr"] for c in cases}
    outs = [load(t[1], hexes) for t in targets]
    skip = {"seen", "seen_pos", "messages", "rssi", "r_dst", "r_dir"}
    bad = 0
    for name, kw in cases:
        h = "%06x" % kw["addr"]
        rows = [next((v for k, v in o.items() if k.lstrip("~") == h), None) for o in outs]
        print(f"--- {name}")
        print("   ", json.dumps(rows[0])[:520])
        if len(rows) > 1:
            x, y = rows
            if x is None or y is None:
                print("    DIFF: listed by", "neither" if x is None and y is None else ("the first only" if y is None else "the second only")); bad += 1; continue
            d = [f"{k}: {x.get(k)!r} / {y.get(k)!r}" for k in sorted((set(x) | set(y)) - skip) if x.get(k) != y.get(k)]
            bad += bool(d)
            print("    same" if not d else "    DIFF: " + "; ".join(d))
    return 1 if bad else 0

sys.exit(main())
