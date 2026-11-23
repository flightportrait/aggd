#!/bin/bash
# A long shadow run: readsb (Docker) and aggd on the same live Beast
# stream, compared every ten minutes, with each one's memory and CPU.
# Runs until stopped; uses its own names and ports (HTTP 35080) so the
# other tools can run meanwhile. It leaves a container and two
# background processes running on this machine: stop them with `stop`.
#   tools/soak.sh start HOST PORT DIR | status DIR [LINES] | stop DIR
#   tools/soak.sh run HOST PORT DIR HOURS   (stays in the foreground, then stops everything)
# DB=aircraft.csv.gz in the environment gives both the airframe database.
HERE=$(cd "$(dirname "$0")/.." && pwd); CMD=${1:-}
IMG=ghcr.io/sdr-enthusiasts/docker-adsb-ultrafeeder:latest
case $CMD in
start)
  HOST=$2; PORT=$3; W=$4; mkdir -p $W/ref; chmod 777 $W/ref
  cp $HERE/target/release/aggd $W/aggd-soak; cp $HERE/tools/compare.py $W/compare.py
  [ -n "${DB:-}" ] && cp $DB $W/ref/aircraft.csv.gz
  RDB=""; ADB=""
  if [ -f $W/ref/aircraft.csv.gz ]; then RDB="--db-file /run/readsb/aircraft.csv.gz --db-file-lt"; ADB="--db-file $W/ref/aircraft.csv.gz --db-file-lt"; fi
  sg docker -c "docker rm -f aggd-soak-ref" >/dev/null 2>&1
  sg docker -c "docker run -d --name aggd-soak-ref --network host -v $W/ref:/run/readsb --entrypoint readsb $IMG --net --net-only --quiet --max-range 20000 --net-connector $HOST,$PORT,beast_in --write-json /run/readsb --write-json-every 1 --json-reliable 1 $RDB" >/dev/null
  setsid nohup $W/aggd-soak --net-connector $HOST,$PORT,beast_in --http-port 35080 --write-state $W/state --write-globe-history $W/hist $ADB > $W/aggd.log 2>&1 &
  echo $! > $W/aggd.pid
  setsid nohup bash $HERE/tools/soak.sh log $W > /dev/null 2>&1 &
  echo $! > $W/logger.pid
  echo "soak started in $W (one line every 10 min in $W/soak.log)";;
log)
  W=$2
  rss() { awk '/VmRSS/{printf "%.0f", $2/1024}' /proc/$1/status 2>/dev/null; }
  cpu() { awk '{print $14+$15}' /proc/$1/stat 2>/dev/null; }
  while sleep 600; do
    RP=$(sg docker -c "docker inspect -f '{{.State.Pid}}' aggd-soak-ref" 2>/dev/null); AP=$(cat $W/aggd.pid)
    line=$(python3 $W/compare.py $W/ref/aircraft.json http://127.0.0.1:35080/data/aircraft.json --rounds 3 2>&1 | head -6 | tr '\n' '|')
    alive=NO; kill -0 $AP 2>/dev/null && alive=yes
    echo "$(date -u +%FT%TZ) readsb rss=$(rss $RP)MB cputicks=$(cpu $RP) | aggd rss=$(rss $AP)MB cputicks=$(cpu $AP) alive=$alive | $line" >> $W/soak.log
  done;;
run)
  HOST=$2; PORT=$3; W=$4; HOURS=${5:-6}; mkdir -p $W/ref; chmod 777 $W/ref
  cp $HERE/target/release/aggd $W/aggd-soak; cp $HERE/tools/compare.py $W/compare.py
  [ -n "${DB:-}" ] && cp $DB $W/ref/aircraft.csv.gz
  RDB=""; ADB=""
  if [ -f $W/ref/aircraft.csv.gz ]; then RDB="--db-file /run/readsb/aircraft.csv.gz --db-file-lt"; ADB="--db-file $W/ref/aircraft.csv.gz --db-file-lt"; fi
  sg docker -c "docker rm -f aggd-soak-ref" >/dev/null 2>&1
  sg docker -c "docker run -d --rm --name aggd-soak-ref --network host -v $W/ref:/run/readsb --entrypoint readsb $IMG --net --net-only --quiet --max-range 20000 --net-connector $HOST,$PORT,beast_in --write-json /run/readsb --write-json-every 1 --json-reliable 1 $RDB" >/dev/null
  $W/aggd-soak --net-connector $HOST,$PORT,beast_in --http-port 35080 --write-state $W/state --write-globe-history $W/hist $ADB > $W/aggd.log 2>&1 &
  AP=$!; echo $AP > $W/aggd.pid
  trap 'kill -TERM $AP 2>/dev/null; sg docker -c "docker rm -f aggd-soak-ref" >/dev/null 2>&1' EXIT
  rss() { awk '/VmRSS/{printf "%.0f", $2/1024}' /proc/$1/status 2>/dev/null; }
  cpu() { awk '{print $14+$15}' /proc/$1/stat 2>/dev/null; }
  for i in $(seq $((HOURS * 6))); do
    sleep 600
    RP=$(sg docker -c "docker inspect -f '{{.State.Pid}}' aggd-soak-ref" 2>/dev/null)
    line=$(python3 $W/compare.py $W/ref/aircraft.json http://127.0.0.1:35080/data/aircraft.json --rounds 3 2>&1 | head -6 | tr '\n' '|')
    alive=NO; kill -0 $AP 2>/dev/null && alive=yes
    echo "$(date -u +%FT%TZ) readsb rss=$(rss $RP)MB cputicks=$(cpu $RP) | aggd rss=$(rss $AP)MB cputicks=$(cpu $AP) alive=$alive | $line" | tee -a $W/soak.log | cut -c1-220
  done
  echo "soak finished after $HOURS h";;
status) tail -${3:-6} $2/soak.log | cut -c1-${COLS:-400};;
stop) W=$2; kill $(cat $W/logger.pid) 2>/dev/null; kill -TERM $(cat $W/aggd.pid) 2>/dev/null; sg docker -c "docker rm -f aggd-soak-ref" >/dev/null 2>&1; echo stopped;;
*) sed -n '2,9p' "$0";;
esac
