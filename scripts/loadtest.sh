#!/usr/bin/env bash
# Claim-3 evidence harness (claims.md claim 3).
#
# Drives sustained concurrent ingest (the Rust loadgen, keep-alive
# connections) while dashboard queries race it, then reports events/s, peak
# RSS and query latency. Run it under a resource cap to emulate the target box:
#
#   scripts/loadtest.sh 400000 32                      # dev box
#   cargo build --release && cargo build --release --example loadgen
#   SKIP_BUILD=1 systemd-run --user --scope -p CPUQuota=100% -p MemoryMax=1G \
#       scripts/loadtest.sh 400000 32                  # ~1 vCPU / 1 GB
#
# Usage: scripts/loadtest.sh [total_events] [connections]
set -euo pipefail

TOTAL="${1:-200000}"
CONNECTIONS="${2:-16}"
PORT="${PORT:-18930}"
DATA="$(mktemp -d)"
BIN="./target/release/hoglet"
BASE="http://127.0.0.1:$PORT"

if [ -z "${SKIP_BUILD:-}" ]; then
    echo "building release binary + loadgen..."
    cargo build --release >/dev/null 2>&1
    cargo build --release --example loadgen >/dev/null 2>&1
fi

HOGLET_ADDR="127.0.0.1:$PORT" HOGLET_DATA="$DATA" HOGLET_MAX_EVENTS_PER_SEC=100000000 \
    "$BIN" >/dev/null 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; rm -rf "$DATA"' EXIT
until curl -sf "$BASE/ready" >/dev/null 2>&1; do sleep 0.1; done

SETUP="$(curl -s -c "$DATA/jar" -X POST "$BASE/api/auth/setup" -H 'content-type: application/json' \
    -d '{"email":"load@example.com","password":"load-test-password","organization_name":"Load","project_name":"Load"}')"
TOKEN="$(echo "$SETUP" | python3 -c 'import sys,json;print(json.load(sys.stdin)["organizations"][0]["projects"][0]["token"])')"
PROJECT="$(echo "$SETUP" | python3 -c 'import sys,json;print(json.load(sys.stdin)["organizations"][0]["projects"][0]["id"])')"

( while kill -0 "$PID" 2>/dev/null; do ps -o rss= -p "$PID" 2>/dev/null | tr -d ' '; sleep 0.2; done ) > "$DATA/rss.log" &
SAMPLER=$!

# Dashboard reads race the ingest: web overview + a trends query, repeatedly.
QUERY='{"query":{"kind":"TrendsQuery","series":[{"event":null,"math":"dau"}],"date_range":{"date_from":"-7d"},"interval":"day"},"refresh":true}'
( while kill -0 "$PID" 2>/dev/null; do
    curl -s -o /dev/null -w '%{time_total}\n' -b "$DATA/jar" "$BASE/api/projects/$PROJECT/web/overview?date_from=-7d" >> "$DATA/q.log" || true
    curl -s -o /dev/null -w '%{time_total}\n' -b "$DATA/jar" -X POST -H 'content-type: application/json' \
        -d "$QUERY" "$BASE/api/projects/$PROJECT/query" >> "$DATA/q.log" || true
    sleep 0.25
  done ) &
QUERIES=$!

RESULT="$(./target/release/examples/loadgen "$PORT" "$TOTAL" "$CONNECTIONS" "$TOKEN")"

kill "$SAMPLER" "$QUERIES" 2>/dev/null || true
PEAK_KB="$(sort -n "$DATA/rss.log" | tail -1)"
PEAK_MB="$(awk -v kb="${PEAK_KB:-0}" 'BEGIN {printf "%.1f", kb / 1024}')"
Q_P50="$(sort -n "$DATA/q.log" | awk '{a[NR]=$1} END {if (NR) print a[int(NR*0.5)+1]; else print "n/a"}')"
Q_P95="$(sort -n "$DATA/q.log" | awk '{a[NR]=$1} END {if (NR) print a[int(NR*0.95)+1]; else print "n/a"}')"
FILES="$(find "$DATA/events" -name '*.parquet' | wc -l)"

echo "----------------------------------------"
echo "$RESULT"
echo "peak RSS:        ${PEAK_MB} MB"
echo "racing queries:  p50 ${Q_P50}s  p95 ${Q_P95}s  ($(wc -l < "$DATA/q.log") requests)"
echo "event files:     ${FILES}"
echo "----------------------------------------"
echo "targets: >= 5000 events/s, RSS < 400 MB on 1 vCPU / 1 GB"
