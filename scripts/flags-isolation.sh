#!/usr/bin/env bash
# Claim: flag evaluation stays fast while analytics queries and ingest run flat out
# (PostHog's flags outages were load-shared with analytics; Hoglet's must not be).
# Usage: scripts/flags-isolation.sh [seconds]   (needs target/release/{hoglet,examples/loadgen})
set -euo pipefail
SECS="${1:-20}"
PORT="${PORT:-18931}"
DATA="$(mktemp -d)"
BASE="http://127.0.0.1:$PORT"
HOGLET_ADDR="127.0.0.1:$PORT" HOGLET_DATA="$DATA" HOGLET_DEMO=1 HOGLET_MAX_EVENTS_PER_SEC=100000000 \
    ./target/release/hoglet >/dev/null 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; rm -rf "$DATA"' EXIT
until curl -sf "$BASE/ready" >/dev/null 2>&1; do sleep 0.1; done
curl -s -c "$DATA/jar" -X POST "$BASE/api/auth/login" -H 'content-type: application/json' \
    -d '{"email":"demo@hoglet.dev","password":"hoglet-demo-1"}' >/dev/null
ORG="$(curl -s -b "$DATA/jar" "$BASE/api/organizations")"
PROJECT="$(echo "$ORG" | python3 -c 'import sys,json;print(json.load(sys.stdin)[0]["projects"][0]["id"])')"
TOKEN="$(echo "$ORG" | python3 -c 'import sys,json;print(json.load(sys.stdin)[0]["projects"][0]["token"])')"
curl -s -b "$DATA/jar" -X POST "$BASE/api/projects/$PROJECT/feature_flags" -H 'content-type: application/json' \
    -d '{"key":"iso","active":true,"filters":{"groups":[{"properties":[],"rollout_percentage":50}]}}' >/dev/null
sleep 4   # demo data published

FUNNEL='{"query":{"kind":"FunnelsQuery","series":[{"event":"$pageview"},{"event":"signed_up"},{"event":"note_created"}],"date_range":{"date_from":"-90d"}},"refresh":true}'
WEB="$BASE/api/projects/$PROJECT/web/overview?date_from=-90d"
end=$((SECONDS + SECS))
( while [ $SECONDS -lt $end ]; do
    curl -s -o /dev/null -b "$DATA/jar" -X POST -H 'content-type: application/json' -d "$FUNNEL" "$BASE/api/projects/$PROJECT/query" || true
    curl -s -o /dev/null -b "$DATA/jar" "$WEB" || true
  done ) &
Q1=$!
( while [ $SECONDS -lt $end ]; do
    curl -s -o /dev/null -b "$DATA/jar" -X POST -H 'content-type: application/json' -d "$FUNNEL" "$BASE/api/projects/$PROJECT/query" || true
  done ) &
Q2=$!
./target/release/examples/loadgen "$PORT" 2000000 16 "$TOKEN" >/dev/null 2>&1 &
LG=$!

: > "$DATA/flags.log"
i=0
while [ $SECONDS -lt $end ]; do
    i=$((i + 1))
    curl -s -o /dev/null -w '%{time_total}\n' -X POST "$BASE/flags/?v=2" \
        -d "{\"api_key\":\"$TOKEN\",\"distinct_id\":\"user-$i\"}" >> "$DATA/flags.log"
    sleep 0.01
done
kill $Q1 $Q2 $LG 2>/dev/null || true
wait $Q1 $Q2 $LG 2>/dev/null || true
sort -n "$DATA/flags.log" | awk '{a[NR]=$1} END {printf "flags requests: %d  p50 %.1f ms  p99 %.1f ms  max %.1f ms\n", NR, a[int(NR*0.5)+1]*1000, a[int(NR*0.99)+1]*1000, a[NR]*1000}'
