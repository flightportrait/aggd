#!/usr/bin/env python3
"""Decode a heat map file (NN.bin.ttf) and summarise it.
    heatmap.py FILE"""
import gzip, struct, sys, time, collections
d = gzip.decompress(open(sys.argv[1], "rb").read()); n = len(d) // 4
a = struct.unpack("<%di" % n, d); u = struct.unpack("<%dI" % n, d)
nidx = a[0]; idx = [a[i * 4] for i in range(nidx)] + [n // 4]
ok = all(u[s * 4] == 0x0e7f7c9d for s in idx[:-1])
print(f"{nidx} samples, {n // 4} records, every sample starts with the marker: {ok}")
tot = collections.Counter(); first = None; hexes = set()
for k in range(nidx):
    i = idx[k] * 4
    t = (u[i + 1] * 4294967296 + u[i + 2]) / 1000
    for j in range(i + 4, idx[k + 1] * 4, 4):
        if a[j + 1] & (1 << 30): tot["info"] += 1
        else:
            tot["pos"] += 1; hexes.add(u[j] & 0xFFFFFF)
            if first is None:
                alt = struct.unpack("<h", struct.pack("<H", u[j + 3] & 0xFFFF))[0]; gs = struct.unpack("<h", struct.pack("<H", u[j + 3] >> 16))[0]
                first = (time.strftime("%H:%M:%S", time.gmtime(t)), "%06x" % (u[j] & 0xFFFFFF), "kind %d" % (u[j] >> 27), a[j + 1] / 1e6, a[j + 2] / 1e6, alt * 25, gs / 10, "interval %d ms" % a[i + 3])
print(dict(tot), "aircraft:", len(hexes)); print("first position:", first)
