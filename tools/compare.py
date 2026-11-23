#!/usr/bin/env python3
"""Compare two aggregators' aircraft.json, field by field.

    compare.py REFERENCE_URL_OR_FILE CANDIDATE_URL_OR_FILE [--rounds N]

Both are read as close together as possible. Numeric fields may differ
by what one refresh interval of flight explains; everything else must
be equal. Prints per-field mismatch counts and a few examples.
"""
import json, sys, time, math, urllib.request, collections

def load(src):
    if src.startswith("http"):
        return json.load(urllib.request.urlopen(src, timeout=10))
    return json.load(open(src))

# field -> absolute tolerance (None: must be equal)
TOL = {"lat": 0.02, "lon": 0.03, "alt_baro": 150, "alt_geom": 150, "gs": 2, "track": 2, "baro_rate": 200,
       "geom_rate": 200, "nav_heading": 2, "r_dst": 1.0, "r_dir": 1.0, "rssi": 6.0, "mag_heading": 2,
       "true_heading": 2, "ias": 5, "tas": 5, "mach": 0.02, "roll": 3, "track_rate": 1, "nav_altitude_mcp": 0,
       "nav_qnh": 0.11,
       # derived from samples that arrive at different moments: the two pair them differently
       "ws": 3, "wd": 15, "oat": 3, "tat": 3, "calc_track": 10}
SKIP = {"seen", "seen_pos", "messages", "dst", "dir"}

def main():
    ref_src, cand_src = sys.argv[1], sys.argv[2]
    rounds = int(sys.argv[sys.argv.index("--rounds") + 1]) if "--rounds" in sys.argv else 1
    bad = collections.Counter(); examples = {}; total = collections.Counter()
    only_ref = only_cand = both = 0
    for r in range(rounds):
        a, b = load(ref_src), load(cand_src)
        A = {x["hex"]: x for x in a["aircraft"]}; B = {x["hex"]: x for x in b["aircraft"]}
        only_ref += len(A.keys() - B.keys()); only_cand += len(B.keys() - A.keys())
        for h in A.keys() & B.keys():
            both += 1
            x, y = A[h], B[h]
            for k in (x.keys() | y.keys()) - SKIP:
                total[k] += 1
                if k not in x or k not in y:
                    bad[k + (" (missing in candidate)" if k in x else " (extra in candidate)")] += 1
                    examples.setdefault(k, (h, x.get(k), y.get(k)))
                    continue
                u, v = x[k], y[k]
                if k == "lastPosition":
                    u = {i: j for i, j in u.items() if i != "seen_pos"}; v = {i: j for i, j in v.items() if i != "seen_pos"}
                if k in TOL and isinstance(u, (int, float)) and isinstance(v, (int, float)):
                    d = abs(u - v)
                    if k in ("track", "nav_heading", "r_dir", "mag_heading", "true_heading", "wd", "calc_track"): d = min(d, 360 - d)
                    ok = d <= TOL[k]
                else:
                    ok = u == v
                if not ok:
                    bad[k] += 1; examples.setdefault(k, (h, u, v))
        if r + 1 < rounds: time.sleep(3)
    nbad = sum(bad.values()); ntot = sum(total.values())
    print(f"aircraft compared: {both}; only in reference: {only_ref}; only in candidate: {only_cand}; fields compared: {ntot}, differing: {nbad} ({100 * nbad / max(ntot, 1):.2f} %)")
    if not bad:
        print(f"all {sum(total.values())} field comparisons agree"); return 0
    for k, n in bad.most_common():
        base = k.split(" (")[0]
        print(f"  {k}: {n} of {total[base]}  e.g. {examples.get(base)}")
    return 1

sys.exit(main())
