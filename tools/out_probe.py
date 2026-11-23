#!/usr/bin/env python3
"""Do the output streams hold up under load? Connect consumers to the
Beast ports of a running aggregator (and listen for its outbound
connector), read for a while, and report per consumer: connections it
took, megabytes, frames, bytes that were not a frame.

    out_probe.py SECONDS  name=host:port ...  [name=listen:PORT ...]  [name=slow:host:port]

`slow:` reads 20 kB a second, as a consumer on a bad line would.
"""
import socket, sys, threading, time

LEN = {0x31: 2, 0x32: 7, 0x33: 14, 0x34: 2}

class Consumer(threading.Thread):
    def __init__(self, name, spec, until):
        super().__init__(daemon=True)
        self.name, self.until = name, until
        self.slow = spec.startswith("slow:"); spec = spec.removeprefix("slow:")
        self.listen = spec.startswith("listen:")
        h, p = spec.removeprefix("listen:").rsplit(":", 1) if ":" in spec.removeprefix("listen:") else ("127.0.0.1", spec.removeprefix("listen:"))
        self.addr = (h, int(p)); self.conns = 0; self.bytes = 0; self.frames = 0; self.stray = 0; self.rest = b""; self.err = ""
        if self.listen:
            self.l = socket.socket(); self.l.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); self.l.bind(self.addr); self.l.listen(4); self.l.settimeout(1)
        self.start()
    def open(self):
        if self.listen:
            try: return self.l.accept()[0]
            except OSError: return None
        try: return socket.create_connection(self.addr, timeout=3)
        except OSError as e:
            self.err = str(e); time.sleep(0.5); return None
    def run(self):
        while time.time() < self.until:
            s = self.open()
            if s is None: continue
            self.conns += 1; self.rest = b""; s.settimeout(1)
            if self.slow: s.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 8192)
            while time.time() < self.until:
                try: d = s.recv(2000 if self.slow else 1 << 16)
                except socket.timeout: continue
                except OSError: break
                if not d: break
                self.bytes += len(d); self.parse(d)
                if self.slow: time.sleep(0.1)
            s.close()
    def parse(self, d):
        b = self.rest + d; i = 0; n = len(b)
        while i < n:
            if b[i] != 0x1A:
                self.stray += 1; i += 1; continue
            if i + 1 >= n: break
            t = b[i + 1]
            if t not in LEN:
                self.stray += 2; i += 2; continue
            need, j, got = 7 + LEN[t], i + 2, 0
            while got < need and j < n:
                if b[j] == 0x1A:
                    if j + 1 >= n: break
                    if b[j + 1] != 0x1A: got = -1; break
                    j += 1
                j += 1; got += 1
            if got == -1:
                self.stray += 2; i += 2; continue
            if got < need: break
            self.frames += 1; i = j
        self.rest = b[i:]

def main():
    secs = int(sys.argv[1]); until = time.time() + secs
    cs = [Consumer(*a.split("=", 1), until) for a in sys.argv[2:]]
    time.sleep(secs + 1.5)
    bad = 0
    for c in cs:
        ok = c.conns == 1 and c.frames > 0 and c.stray == 0
        ok = ok or (c.slow and c.frames > 0 and c.stray == 0)   # a slow one may be cut off, never corrupted
        bad += not ok
        print(f"{'ok  ' if ok else 'FAIL'} {c.name:14} connections {c.conns:3}  {c.bytes / 1e6:8.1f} MB  {c.frames:9} frames  {c.frames / secs:8.0f}/s  stray bytes {c.stray} {c.err}")
    sys.exit(1 if bad else 0)

main()
