#!/usr/bin/env python3
"""Throw malformed input at every port of a running aggd and check it
stays up.   fuzz.py HOST BEAST_PORT SBS_PORT HTTP_PORT [SECONDS]"""
import os, random, socket, sys, time, urllib.request

host, beast, sbs, http = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
secs = int(sys.argv[5]) if len(sys.argv) > 5 else 30
rnd = random.Random(7)

def send(port, data, hold=0.0):
    try:
        s = socket.create_connection((host, port), timeout=3)
        s.settimeout(0.25)
        try:
            s.sendall(data)
            if hold: time.sleep(hold)
            try: s.recv(4096)
            except Exception: pass
        finally: s.close()
    except Exception:
        pass   # refused or banned is a fine answer

def beast_cases():
    yield os.urandom(rnd.randrange(1, 70000))
    yield b"\x1a" * rnd.randrange(1, 5000)
    yield b"\x1a3" + os.urandom(5)                      # truncated
    yield b"\x1a\xe4" + os.urandom(rnd.randrange(0, 80))  # bad uuid
    yield b"\x1a\xe3" + os.urandom(3)
    yield (b"\x1a3" + os.urandom(21)) * rnd.randrange(1, 3000)
    yield (b"\x1a2" + b"\x1a\x1a" * 14) * 500
    yield b"\x1aP" + b"\x1aW" + b"\x1a4" + os.urandom(40)
    yield bytes(rnd.choice([0x1a, 0x33, 0x32, 0x8d, 0xff, 0]) for _ in range(20000))

def sbs_cases():
    yield b"MSG," * 100000
    yield b"A" * 2_000_000
    yield b"MSG,3,1,1,ZZZZZZ,1,2026/13/45,99:99:99.999,x,y,,abc,def,ghi,999,-999,,,,,,\r\n" * 200
    yield b"MSG,3,1,1,4CA2D6,1,,,,,,1e400,nan,inf,91.5,181.5,,,,,,\r\n" * 200
    yield "MSG,3,1,1,ééé,1,2026/11/16,10:00:00.000,,,,,,,,,,,,,,\r\n".encode() * 50
    yield os.urandom(50000)
    yield b"MSG,3\r\n" * 5000

def http_cases():
    paths = ["/re-api/?circle=" + "9" * 5000, "/re-api/?box=a,b,c,d", "/re-api/?find_hex=" + ",".join(["zz"] * 1000),
             "/data/traces/../../../../etc/passwd", "/data/traces/zz/trace_full_éé.json", "/globe_history/....-..-../x",
             "/globe_history/2026-11-16/traces/%00/x", "/data/globe_99999999999.binCraft.zst", "/data/globe_-1.binCraft",
             "/../../etc/passwd", "/%2e%2e/%2e%2e/etc/passwd", "/re-api/?all&jv2&binCraft&zstd&closest=1,2,3",
             "/re-api/?circle=nan,inf,-inf", "/re-api/?above_alt_baro=99999999999999999999", "/" + "a" * 20000]
    for p in paths:
        yield ("GET %s HTTP/1.1\r\nHost: x\r\n\r\n" % p).encode("utf-8", "replace")
    yield b"GET / HTTP/1.1\r\n" + b"X: y\r\n" * 100000
    yield b"POST /data/aircraft.json HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n"
    yield os.urandom(30000)
    yield b"GET /data/aircraft.json HTTP/1.1\r\nAccept-Encoding: zstd\r\n\r\n" * 200   # pipelined

end = time.time() + secs; n = 0
while time.time() < end:
    for c in beast_cases(): send(beast, c); n += 1
    for c in sbs_cases(): send(sbs, c); n += 1
    for c in http_cases(): send(http, c, 0.05); n += 1
try:
    ok = urllib.request.urlopen(f"http://{host}:{http}/health", timeout=5).read() == b"ok\n"
except Exception as e:
    ok = False; print("health check failed:", e)
import json
for doc in ("aircraft.json", "stats.json", "status.json", "clients.json", "receivers.json", "receiver.json"):
    try:
        json.load(urllib.request.urlopen(f"http://{host}:{http}/data/{doc}", timeout=5))
    except Exception as e:
        ok = False; print(f"{doc} is not valid JSON any more: {e}")
print(f"{n} malformed inputs sent; server {'still answering' if ok else 'NOT answering'}")
sys.exit(0 if ok else 1)
