#!/usr/bin/env python3
"""Decode a binCraft file (as tar1090 reads it) to aircraft.json-shaped
objects, and optionally check it against an aircraft.json taken at the
same moment.

    bincraft.py FILE.binCraft[.zst] [AIRCRAFT.json]
"""
import struct, sys, json, subprocess, math, collections

TYPES = ["adsb_icao", "adsb_icao_nt", "adsr_icao", "tisb_icao", "adsc", "mlat", "other", "mode_s", "adsb_other",
         "adsr_other", "tisb_trackfile", "tisb_other", "mode_ac", "unknown"]
NAV = ["autopilot", "vnav", "althold", "approach", "lnav", "tcas"]
EMERG = ["none", "general", "lifeguard", "minfuel", "nordo", "unlawful", "downed", "reserved"]

def decode(buf):
    u32 = struct.unpack_from("<13I", buf, 0)
    stride = u32[2]
    head = {"now": u32[0] / 1000 + u32[1] * 4294967.296, "stride": stride, "with_pos": u32[3], "index": u32[4],
            "limits": struct.unpack_from("<4h", buf, 20), "messages": u32[7], "version": u32[10],
            "message_rate": u32[11] / 10, "flags": u32[12]}
    out = []
    for off in range(stride, len(buf) - stride + 1, stride):
        r = buf[off:off + stride]
        s32 = struct.unpack_from("<28i", r); u16 = struct.unpack_from("<56H", r); s16 = struct.unpack_from("<56h", r)
        v = lambda byte, bit: bool(r[byte] >> bit & 1)
        a = {"hex": ("~" if s32[0] & 1 << 24 else "") + "%06x" % (s32[0] & 0xFFFFFF), "type": TYPES[r[67] >> 4]}
        if v(73, 3): a["flight"] = r[78:86].split(b"\0")[0].decode()
        ground = (r[68] & 15) == 1
        if ground: a["alt_baro"] = "ground"
        elif v(73, 4): a["alt_baro"] = s16[10] * 25
        if v(73, 5): a["alt_geom"] = s16[11] * 25
        if v(73, 7): a["gs"] = s16[17] / 10
        if v(74, 0): a["ias"] = u16[29]
        if v(74, 1): a["tas"] = u16[28]
        if v(74, 2): a["mach"] = s16[18] / 1000
        if v(74, 3): a["track"] = s16[20] / 90
        if v(74, 4): a["track_rate"] = s16[21] / 100
        if v(74, 5): a["roll"] = s16[19] / 100
        if v(74, 6): a["mag_heading"] = s16[22] / 90
        if v(74, 7): a["true_heading"] = s16[23] / 90
        if v(75, 0): a["baro_rate"] = s16[8] * 8
        if v(75, 1): a["geom_rate"] = s16[9] * 8
        if v(76, 2): a["squawk"] = "%04x" % u16[16]
        if v(76, 3): a["emergency"] = EMERG[r[67] & 15]
        if r[64]: a["category"] = "%02X" % r[64]
        if v(76, 5): a["nav_qnh"] = s16[14] / 10
        if v(76, 6): a["nav_altitude_mcp"] = u16[12] * 4
        if v(76, 7): a["nav_altitude_fms"] = u16[13] * 4
        if v(77, 1): a["nav_heading"] = s16[15] / 90
        if v(77, 2): a["nav_modes"] = [n for i, n in enumerate(NAV) if r[66] >> i & 1]
        if v(73, 6):
            a["lat"] = s32[3] / 1e6; a["lon"] = s32[2] / 1e6; a["nic"] = r[65]; a["rc"] = u16[30]
            a["seen_pos"] = s32[1] / 10
        ver = r[69] >> 4
        if ver != 15: a["version"] = ver
        if v(75, 4): a["nic_baro"] = r[73] & 1
        if v(75, 5): a["nac_p"] = r[71] & 15
        if v(75, 6): a["nac_v"] = r[71] >> 4
        if v(75, 7):
            a["sil"] = r[72] & 3; a["sil_type"] = ["invalid", "unknown", "persample", "perhour"][r[69] & 3]
        if v(76, 0): a["gva"] = r[72] >> 2 & 3
        if v(76, 1): a["sda"] = r[72] >> 4 & 3
        if v(77, 3): a["alert"] = r[73] >> 1 & 1
        if v(76, 4): a["spi"] = r[73] >> 2 & 1
        if v(77, 4): a["wd"] = s16[24]; a["ws"] = s16[25]
        if v(77, 5): a["oat"] = s16[26]; a["tat"] = s16[27]
        if u16[43]: a["dbFlags"] = u16[43]
        t = r[88:92].split(b"\0")[0].decode(); reg = r[92:104].split(b"\0")[0].decode()
        if t: a["t"] = t
        if reg: a["r"] = reg
        a["seen"] = s32[27] / 10
        a["rssi"] = round(r[105] * 50 / 255 - 50, 1)
        out.append(a)
    return head, out

TOL = {"lat": 0.02, "lon": 0.03, "alt_baro": 150, "alt_geom": 150, "gs": 2, "track": 2, "baro_rate": 200, "geom_rate": 200,
       "nav_heading": 2, "rssi": 6, "seen": 1.5, "seen_pos": 1.5, "nav_qnh": 0.11, "nav_altitude_mcp": 4, "nav_altitude_fms": 4,
       "mag_heading": 2, "true_heading": 2, "mach": 0.02, "roll": 3, "track_rate": 1, "ias": 5, "tas": 5,
       "wd": 5, "ws": 3, "oat": 2, "tat": 2}
SKIP = {"messages", "r_dst", "r_dir", "mlat", "tisb", "lastPosition", "desc", "ownOp", "year"}

def main():
    raw = open(sys.argv[1], "rb").read()
    if raw[:4] == b"\x28\xb5\x2f\xfd":
        raw = subprocess.run(["zstd", "-d", "-c"], input=raw, capture_output=True, check=True).stdout
    head, acs = decode(raw)
    print(head, "aircraft:", len(acs))
    if len(sys.argv) < 3:
        print(json.dumps(acs[:2], indent=1)); return 0
    J = {x["hex"]: x for x in json.load(open(sys.argv[2]))["aircraft"]}
    bad = collections.Counter(); ex = {}; n = 0
    for a in acs:
        j = J.get(a["hex"])
        if not j: bad["(not in json)"] += 1; continue
        n += 1
        for k in (a.keys() | j.keys()) - SKIP:
            if k not in a or k not in j:
                bad[k + (" missing in binCraft" if k in j else " extra in binCraft")] += 1; ex.setdefault(k, (a["hex"], j.get(k), a.get(k))); continue
            u, w = j[k], a[k]
            if k in TOL and isinstance(u, (int, float)) and isinstance(w, (int, float)):
                d = abs(u - w)
                if k in ("track", "nav_heading", "mag_heading", "true_heading", "wd"): d = min(d, 360 - d)
                ok = d <= TOL[k]
            else: ok = u == w
            if not ok: bad[k] += 1; ex.setdefault(k, (a["hex"], u, w))
    print(f"compared {n} aircraft against json")
    for k, c in bad.most_common(): print(f"  {k}: {c}  e.g. {ex.get(k.split(' ')[0])}")
    if not bad: print("  binCraft and json agree")
    return 1 if bad else 0

sys.exit(main())
