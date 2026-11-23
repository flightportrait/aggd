#!/usr/bin/env python3
"""How far apart may an even and an odd position message be and still
give a position, and how does that depend on ground speed?

    PORT=beast_port SRC=aircraft.json_url_or_file [REPEAT_V=1] pair_probe.py SPEED_KT...

Speed 0 sends no velocity message. REPEAT_V=1 repeats the velocity every
5 s, so that the speed is still fresh when the second message comes. The
reference readsb needs --json-reliable 1 to show a position from one pair.
"""
import urllib.request,os,socket,subprocess,time,json,threading,sys
s=socket.create_connection(("127.0.0.1",int(os.environ.get("PORT","39004")))); t0=time.time(); lock=threading.Lock()
def fr(hexa,gs=440): return dict(l.split() for l in subprocess.run([os.path.join(os.path.dirname(os.path.abspath(__file__)), "../target/release/aggbench"),"frames","--hex",hexa,"--gs",str(gs)],capture_output=True,text=True).stdout.split("\n") if l)
def send(f):
    f=bytes.fromhex(f); body=int((time.time()-t0)*12e6).to_bytes(6,"big")+bytes([120])+f
    with lock: s.sendall(b"\x1a"+(b"2" if len(f)==7 else b"3")+body.replace(b"\x1a",b"\x1a\x1a"))
def play(hexa,d,vel):
    f=fr(hexa,vel or 440); send(f["df11"]); time.sleep(.05); send(f["df11"]); time.sleep(.05)
    if vel: send(f["velocity"])
    send(f["pos_even"])
    t=time.time()
    k=0
    while time.time()-t<d-1:
        time.sleep(1); send(f["df11"]); k+=1
        if vel and k%5==0 and os.environ.get("REPEAT_V"): send(f["velocity"])
    time.sleep(max(0,d-(time.time()-t))); send(f["pos_odd"]); time.sleep(2.6)
    for _ in range(3):
        try: RES[hexa.lower()]=any(a["hex"]==hexa.lower() and "lat" in a for a in (json.load(urllib.request.urlopen(os.environ["SRC"])) if os.environ["SRC"].startswith("http") else json.load(open(os.environ["SRC"])))["aircraft"]); break
        except Exception: time.sleep(0.2)
RES={}
D=[9,10.5,12,14,16,18,19,20,21,22,23,24,25,26,28,30,35,40,50] if not os.environ.get('REPEAT_V') else [29,30,30.5,31,31.5,32,32.5,33,34]
G=[int(x) for x in sys.argv[1:]]
import random
B=random.randrange(0x20,0xF0)
th=[threading.Thread(target=play,args=("4D%02X%02X"%(B+j,i),d,v)) for j,v in enumerate(G) for i,d in enumerate(D)]
[t.start() for t in th]; [t.join() for t in th]; time.sleep(2.5)
for j,v in enumerate(G):
    ok=[d for i,d in enumerate(D) if RES.get("4d%02x%02x"%(B+j,i))]; no=[d for i,d in enumerate(D) if not RES.get("4d%02x%02x"%(B+j,i))]
    print(f'gs {v or "unknown"}: pairs up to {max(ok) if ok else None} s, not at {min(no) if no else None} s', 'ODD' if ok and no and max(ok)>min(no) else '')
