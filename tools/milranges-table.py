#!/usr/bin/env python3
"""Turn the raw result of milranges.py into the Rust table of address
ranges flagged military.   milranges-table.py milranges.raw.json > mil_ranges.rs"""
import json, sys
d = json.load(open(sys.argv[1])); step = d["step"]
flagged = {a // step for a in d["flagged"]}; indb = {a // step for a in d["indb"]}
runs = []; cur = None
for b in range((1 << 24) // step):
    if b in flagged:
        if cur is None: cur = [b, b]
        cur[1] = b
    elif b in indb:
        continue            # its own database entry hides the range rule
    elif cur is not None:
        runs.append(cur); cur = None
if cur: runs.append(cur)
print("/// ICAO address blocks allocated to military operators, as readsb flags")
print("/// them when the airframe database has no entry: found by showing a")
print(f"/// reference readsb one address in every {step} and reading dbFlags back")
print("/// (tools/milranges.py). Inclusive, sorted.")
print(f"pub const MILITARY: [(u32, u32); {len(runs)}] = [")
for lo, hi in runs: print(f"    (0x{lo * step:06x}, 0x{hi * step + step - 1:06x}),")
print("];")
