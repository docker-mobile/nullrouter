#!/usr/bin/env bash
# End-to-end router overhead, measured against a fixed-latency mock.
#
# The number this produces is overhead, not latency:
#
#   overhead = p50(client -> router -> mock) - p50(client -> mock)
#
# Both legs in the same run, against the same mock. Without the subtraction the figures
# are dominated by the mock's own sleep and every router looks identical.
#
# It measures whichever router is listening on --router-port. It does NOT start one, and
# it does not know which one it is talking to — that is deliberate: the same script must
# drive nullrouter and 9Router identically, or the comparison is between two harnesses
# rather than two routers.
#
# This script starts and stops the mock itself, once per cell, because the frame count
# differs per scenario. So --mock-port must be FREE when it runs. benches/configure.sh needs
# a mock up to verify the router can reach one, so the order is:
#
#   ./target/release/mock-provider --port 8099 --sleep-ms 25 --frames 200 &
#   benches/configure.sh                      # verifies, prints the key
#   kill %1                                   # release the port
#   benches/run.sh --label ... --api-key ...  # manages its own mocks
#
# Usage:
#   benches/run.sh --label nullrouter --router-port 20128 --api-key sk-...
#   benches/run.sh --label 9router    --router-port 3000  --api-key sk-...
#
# Requires `oha` (or set LOAD_TOOL=wrk). A hand-rolled client becomes the bottleneck and
# ends up measuring itself.

set -euo pipefail

LABEL=""
ROUTER_PORT=""
API_KEY=""
MOCK_PORT=8099
MOCK_SLEEP_MS=25
DURATION=30s
TRIALS=5
WARMUP=200
OUT_DIR="benches/results"

# Model names must resolve on the router under test to a provider pointed at the mock.
# They differ between nullrouter and 9Router configs, so they are flags, not constants.
MODEL_OPENAI="${MODEL_OPENAI:-bench-openai}"
MODEL_CLAUDE="${MODEL_CLAUDE:-bench-claude}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --label)         LABEL="$2"; shift 2 ;;
    --router-port)   ROUTER_PORT="$2"; shift 2 ;;
    --api-key)       API_KEY="$2"; shift 2 ;;
    --mock-port)     MOCK_PORT="$2"; shift 2 ;;
    --mock-sleep-ms) MOCK_SLEEP_MS="$2"; shift 2 ;;
    --duration)      DURATION="$2"; shift 2 ;;
    --trials)        TRIALS="$2"; shift 2 ;;
    --out)           OUT_DIR="$2"; shift 2 ;;
    --model-openai)  MODEL_OPENAI="$2"; shift 2 ;;
    --model-claude)  MODEL_CLAUDE="$2"; shift 2 ;;
    -h|--help)       sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done

[[ -n "$LABEL" ]]       || { echo "--label is required (nullrouter | 9router)" >&2; exit 2; }
[[ -n "$ROUTER_PORT" ]] || { echo "--router-port is required" >&2; exit 2; }

LOAD_TOOL="${LOAD_TOOL:-oha}"
command -v "$LOAD_TOOL" >/dev/null || {
  echo "$LOAD_TOOL not found. Install oha, or set LOAD_TOOL=wrk." >&2
  echo "A hand-rolled client measures itself; this script will not substitute one." >&2
  exit 3
}

mkdir -p "$OUT_DIR"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$LABEL"
RESULT="$OUT_DIR/$RUN_ID.txt"

# Only one router may be up while measuring. Both idle routers still run background timers
# and hold thread pools, and on a shared CPU that lands in the other one's tail. The check
# is here rather than in the protocol notes because a forgotten teardown is invisible in
# the results -- it just makes both sides look slower.
listening() { timeout 1 bash -c "</dev/tcp/127.0.0.1/$1" 2>/dev/null; }
for other in 20128 20127; do
  [[ "$other" == "$ROUTER_PORT" ]] && continue
  if listening "$other"; then
    echo "port $other is also listening: another router is up while measuring :$ROUTER_PORT." >&2
    echo "stop it first -- two routers sharing this CPU puts one in the other's tail." >&2
    exit 4
  fi
done

# ── environment, recorded before anything is measured ────────────────────────
{
  echo "run: $RUN_ID"
  echo "label: $LABEL"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host: $(uname -srm)"
  echo "cpus: $(nproc)"
  echo "mem: $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "load-tool: $LOAD_TOOL $("$LOAD_TOOL" --version 2>&1 | head -1)"
  echo "node: $(node --version 2>/dev/null || echo absent)"
  echo "rustc: $(rustc --version 2>/dev/null || echo absent)"
  echo "mock: port=$MOCK_PORT sleep_ms=$MOCK_SLEEP_MS"
  echo "router-port: $ROUTER_PORT"
  echo "duration-per-cell: $DURATION  trials: $TRIALS  warmup: $WARMUP"
  echo
} | tee "$RESULT"

# ── the mock, which is the control ───────────────────────────────────────────
MOCK_BIN="target/release/mock-provider"
[[ -x "$MOCK_BIN" ]] || { echo "build it first: cargo build -p mock-provider --release" >&2; exit 3; }

start_mock() {
  local frames="$1"

  # A mock left over from the previous cell still holds the port, so the new one fails to
  # bind and the *old* frame count serves the cell -- a 200-frame scenario measured against
  # a 1-frame mock, which reads as a fast result rather than as an error. This produced a
  # bimodal S3/S5 (~14 ms on one trial, ~58 ms on the next) before it was caught.
  local waited=0
  while timeout 1 bash -c "</dev/tcp/127.0.0.1/$MOCK_PORT" 2>/dev/null; do
    waited=$((waited + 1))
    if [[ $waited -gt 50 ]]; then
      echo "port $MOCK_PORT still held after 5s; cannot start a clean mock" >&2
      return 1
    fi
    sleep 0.1
  done

  "$MOCK_BIN" --port "$MOCK_PORT" --sleep-ms "$MOCK_SLEEP_MS" --frames "$frames" \
    --frame-chars 12 >/dev/null 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 50); do
    local health
    health="$(curl -sf "http://127.0.0.1:$MOCK_PORT/health" 2>/dev/null)" || { sleep 0.1; continue; }
    # Confirm it is the mock this cell asked for, not merely *a* mock. /health reports the
    # configuration it is actually running with for exactly this check.
    local got_frames got_sleep
    got_frames="$(echo "$health" | sed -n 's/.*"frames":\([0-9]*\).*/\1/p')"
    got_sleep="$(echo "$health" | sed -n 's/.*"sleepMs":\([0-9]*\).*/\1/p')"
    if [[ "$got_frames" == "$frames" && "$got_sleep" == "$MOCK_SLEEP_MS" ]]; then
      return 0
    fi
    echo "mock on :$MOCK_PORT reports frames=$got_frames sleepMs=$got_sleep, wanted frames=$frames sleepMs=$MOCK_SLEEP_MS" >&2
    echo "  a stale mock is serving this cell; refusing to measure against it" >&2
    return 1
  done
  echo "mock did not come up" >&2
  return 1
}
stop_mock() {
  [[ -n "${MOCK_PID:-}" ]] && kill "$MOCK_PID" 2>/dev/null
  # Wait for the port to be released, so the next cell's bind does not race this kill.
  local waited=0
  while [[ -n "${MOCK_PID:-}" ]] && timeout 1 bash -c "</dev/tcp/127.0.0.1/$MOCK_PORT" 2>/dev/null; do
    waited=$((waited + 1))
    [[ $waited -gt 50 ]] && break
    sleep 0.1
  done
  MOCK_PID=""
  return 0
}
trap stop_mock EXIT

# ── request bodies ───────────────────────────────────────────────────────────
BODY_DIR="$(mktemp -d)"
trap 'stop_mock; rm -rf "$BODY_DIR"' EXIT

# `"stream": false` is explicit, and must stay explicit. 9Router reads an *absent* stream
# field as streaming (`body.stream !== false` in open-sse/handlers/chatCore.js), so a body
# that merely omits it makes 9Router return SSE while nullrouter returns JSON -- different
# work in what is supposed to be the same cell. Omitting it silently invalidates S1, S2 and
# S6 rather than failing them.
cat > "$BODY_DIR/openai.json" <<'JSON'
{"model":"MODEL","stream":false,"messages":[{"role":"user","content":"Explain the difference between a mutex and a channel."}],"max_tokens":256}
JSON
cat > "$BODY_DIR/openai-stream.json" <<'JSON'
{"model":"MODEL","stream":true,"messages":[{"role":"user","content":"Explain the difference between a mutex and a channel."}],"max_tokens":256}
JSON
# Same content and size as openai.json, in Claude's dialect: S6 must differ from S2 in the
# client format and nothing else, or the difference is not the translation cost.
cat > "$BODY_DIR/claude.json" <<'JSON'
{"model":"MODEL","stream":false,"max_tokens":256,"messages":[{"role":"user","content":"Explain the difference between a mutex and a channel."}]}
JSON

body_for() { # scenario -> path, with the model substituted
  local scenario="$1" model="$2" src="$3"
  local out="$BODY_DIR/$scenario.json"
  sed "s|MODEL|$model|" "$BODY_DIR/$src" > "$out"
  echo "$out"
}

# ── measurement ──────────────────────────────────────────────────────────────
# p50 in milliseconds for one URL+body, or "" when the run failed.
#
# The unit is read from the output, not assumed. oha 1.16 prints
# `50.00% in 25.3542 ms`; assuming seconds and multiplying by 1000 produced a first run
# reporting 25366 ms for a 25 ms mock -- a 1000x error that still looked like a plausible
# table because every cell was wrong by the same factor. Hence also `sanity_direct` below.
measure() {
  local url="$1" body="$2" concurrency="$3"
  local args=(-z "$DURATION" -c "$concurrency" -m POST -T application/json
              -D "$body" --no-tui)
  [[ -n "$API_KEY" ]] && args+=(-H "Authorization: Bearer $API_KEY")
  "$LOAD_TOOL" "${args[@]}" "$url" 2>/dev/null | awk '
    /50\.00% in/ {
      value = $3
      unit = $4
      if (unit ~ /^s(ecs?)?$/) value = value * 1000
      else if (unit ~ /^us|^µs/)  value = value / 1000
      printf "%.3f", value
      exit
    }'
}

# The same measurement with connection reuse disabled, for the keep-alive guard below.
measure_no_keepalive() {
  local url="$1" body="$2" concurrency="$3"
  local args=(-z "$DURATION" -c "$concurrency" --disable-keepalive -m POST
              -T application/json -D "$body" --no-tui)
  [[ -n "$API_KEY" ]] && args+=(-H "Authorization: Bearer $API_KEY")
  "$LOAD_TOOL" "${args[@]}" "$url" 2>/dev/null | awk '
    /50\.00% in/ {
      value = $3
      unit = $4
      if (unit ~ /^s(ecs?)?$/) value = value * 1000
      else if (unit ~ /^us|^µs/)  value = value / 1000
      printf "%.3f", value
      exit
    }'
}

# Warm the target: cold-start noise otherwise swamps the cheapest scenario.
warm() {
  local url="$1" body="$2"
  local args=(-n "$WARMUP" -c 4 -m POST -T application/json -D "$body" --no-tui)
  [[ -n "$API_KEY" ]] && args+=(-H "Authorization: Bearer $API_KEY")
  "$LOAD_TOOL" "${args[@]}" "$url" >/dev/null 2>&1 || true
}

# Does this cell actually measure what it claims? Checked once per cell, before the timed
# runs, because every failure here is invisible in a latency number.
#
# The specific bug this exists for: 9Router treats an absent `stream` field as streaming, so
# a "non-streaming" cell can have one side returning SSE and the other JSON. Both sides
# answer, both look fast, and the comparison is meaningless. Rather than trusting the bodies
# to be right, ask what came back.
verify_shape() {
  local name="$1" router_url="$2" body="$3" mock_url="$4" direct_body="$5" want="$6"

  local args=(-s -o /dev/null -w '%{http_code} %{content_type}' -m 30 -X POST
              -H 'content-type: application/json' --data-binary "@$body")
  [[ -n "$API_KEY" ]] && args+=(-H "Authorization: Bearer $API_KEY")
  local through; through="$(curl "${args[@]}" "$router_url" 2>/dev/null)"

  local direct_args=(-s -o /dev/null -w '%{http_code} %{content_type}' -m 30 -X POST
                     -H 'content-type: application/json' --data-binary "@$direct_body")
  local direct; direct="$(curl "${direct_args[@]}" "$mock_url" 2>/dev/null)"

  local through_code="${through%% *}" through_type="${through#* }"
  local direct_code="${direct%% *}" direct_type="${direct#* }"

  if [[ "$through_code" != 2* ]]; then
    echo "$name: router returned $through_code -- cell will not measure anything useful" | tee -a "$RESULT"
    return 1
  fi
  if [[ "$direct_code" != 2* ]]; then
    echo "$name: mock returned $direct_code on the direct leg" | tee -a "$RESULT"
    return 1
  fi

  # `want` is stream|json. Compare both legs against it, not just against each other: two
  # sides that agree on the wrong shape would still pass a same-shape check.
  local through_kind direct_kind
  case "$through_type" in *event-stream*) through_kind=stream ;; *) through_kind=json ;; esac
  case "$direct_type" in *event-stream*) direct_kind=stream ;; *) direct_kind=json ;; esac

  if [[ "$through_kind" != "$want" || "$direct_kind" != "$want" ]]; then
    echo "$name: shape mismatch -- wanted $want, router gave $through_kind ($through_type), mock gave $direct_kind ($direct_type)" \
      | tee -a "$RESULT"
    echo "  a non-streaming body that omits \"stream\":false reads as streaming to 9Router." \
      | tee -a "$RESULT"
    return 1
  fi
  return 0
}

# One scenario, both legs, N trials. Reports each trial so the spread is visible —
# a single number hides whether the machine was noisy.
cell() {
  local name="$1" frames="$2" model="$3" src="$4" router_path="$5" mock_path="$6" concurrency="$7" want="$8"

  start_mock "$frames" || return 1
  local body; body="$(body_for "$name" "$model" "$src")"
  local direct_body; direct_body="$(body_for "$name-direct" "bench-model" "$src")"

  local router_url="http://127.0.0.1:$ROUTER_PORT$router_path"
  local mock_url="http://127.0.0.1:$MOCK_PORT$mock_path"

  if ! verify_shape "$name" "$router_url" "$body" "$mock_url" "$direct_body" "$want"; then
    stop_mock
    return 0
  fi

  warm "$router_url" "$body"
  warm "$mock_url" "$direct_body"

  local through=() direct=() overhead=()
  for trial in $(seq 1 "$TRIALS"); do
    local t d
    t="$(measure "$router_url" "$body" "$concurrency")"
    d="$(measure "$mock_url" "$direct_body" "$concurrency")"
    if [[ -z "$t" || -z "$d" ]]; then
      echo "$name c=$concurrency trial=$trial: FAILED (no p50 — is the router up and the model routable?)" | tee -a "$RESULT"
      continue
    fi
    through+=("$t"); direct+=("$d")
    overhead+=("$(awk -v a="$t" -v b="$d" 'BEGIN{printf "%.3f", a-b}')")
  done

  # The keep-alive comparison has to happen while the mock is still up. Measuring it after
  # `stop_mock` reported `none=0.000` for every streaming cell -- a value that looks like a
  # measurement rather than like the failure it was.
  local no_keepalive=""
  if [[ "$want" == "stream" ]]; then
    no_keepalive="$(measure_no_keepalive "$router_url" "$body" "$concurrency")"
  fi

  stop_mock

  if [[ ${#overhead[@]} -eq 0 ]]; then
    echo "$name c=$concurrency: NO RESULT" | tee -a "$RESULT"
    return 0
  fi

  # The direct leg is the control, and it has a known expected value: the mock's own fixed
  # sleep. If it does not land near --mock-sleep-ms then the harness is mis-measuring and
  # the overhead column is arithmetic on two wrong numbers. Asserted rather than left as
  # advice in the footer, because the first run of this script reported a 25 ms mock as
  # 25366 ms and every cell was self-consistently wrong.
  local first_direct="${direct[0]}"
  local low high
  low="$(awk -v s="$MOCK_SLEEP_MS" 'BEGIN{print s * 0.5}')"
  high="$(awk -v s="$MOCK_SLEEP_MS" 'BEGIN{print s * 3 + 10}')"
  if awk -v d="$first_direct" -v l="$low" -v h="$high" 'BEGIN{exit !(d < l || d > h)}'; then
    echo "$name c=$concurrency: direct leg measured ${first_direct}ms against a ${MOCK_SLEEP_MS}ms mock" \
      | tee -a "$RESULT"
    echo "  outside [${low}, ${high}] -- the harness is mis-measuring; overhead not reported" \
      | tee -a "$RESULT"
    return 0
  fi

  # Median of the per-trial overheads, plus min/max so the spread is on the record.
  local median
  median="$(printf '%s\n' "${overhead[@]}" | sort -n | awk '{v[NR]=$1} END{print (NR%2)?v[(NR+1)/2]:(v[NR/2]+v[NR/2+1])/2}')"
  local lo hi
  lo="$(printf '%s\n' "${overhead[@]}" | sort -n | head -1)"
  hi="$(printf '%s\n' "${overhead[@]}" | sort -n | tail -1)"

  # All three columns are medians. Printing trial 1's `through` beside a median `overhead`
  # made a bimodal cell look like a stable one with an odd spread.
  local median_through median_direct
  median_through="$(printf '%s\n' "${through[@]}" | sort -n | awk '{v[NR]=$1} END{print (NR%2)?v[(NR+1)/2]:(v[NR/2]+v[NR/2+1])/2}')"
  median_direct="$(printf '%s\n' "${direct[@]}" | sort -n | awk '{v[NR]=$1} END{print (NR%2)?v[(NR+1)/2]:(v[NR/2]+v[NR/2+1])/2}')"

  printf '%-28s c=%-2s  through=%-9s direct=%-9s overhead=%s ms  [%s..%s over %d trials]\n' \
    "$name" "$concurrency" "$median_through" "$median_direct" "$median" "$lo" "$hi" "${#overhead[@]}" \
    | tee -a "$RESULT"

  # Keep-alive guard, streaming cells only. Runs after the medians because it compares
  # against `median_through` — an earlier version referenced it above its assignment, and
  # `set -u` killed the whole run on the first streaming cell.
  #
  # A streamed response used to cost ~2x its real latency to any client using keep-alive,
  # because the accept socket had no TCP_NODELAY: the headers went out as one small write
  # and Nagle then held the body until the client's delayed-ACK timer fired. It was
  # invisible in every functional test, invisible on a freshly-opened connection (Linux
  # quickacks early), and invisible in the non-streaming cells. What made it visible was
  # the same cell measured both ways diverging.
  #
  # So the harness measures both and reports the ratio. Anything much above 1 means a
  # client that reuses connections -- which is every real client -- is paying for it.
  if [[ "$want" == "stream" ]]; then
    # A zero or missing figure means the no-keep-alive leg did not measure anything. Say so
    # rather than printing `ratio=0.00`, which reads like a result.
    if [[ -z "$no_keepalive" ]] || awk -v b="$no_keepalive" 'BEGIN{exit !(b <= 0)}'; then
      printf '%-28s c=%-2s  keep-alive comparison unavailable (no p50 without keep-alive)\n' \
        "  $name" "$concurrency" | tee -a "$RESULT"
    else
      local ratio
      ratio="$(awk -v a="$median_through" -v b="$no_keepalive" 'BEGIN{printf "%.2f", a / b}')"
      printf '%-28s c=%-2s  keep-alive=%-9s none=%-9s ratio=%s%s\n' \
        "  $name" "$concurrency" "$median_through" "$no_keepalive" "$ratio" \
        "$(awk -v r="$ratio" 'BEGIN{print (r > 1.5) ? "  <-- reused connections pay extra on streamed responses; check TCP_NODELAY" : ""}')" \
        | tee -a "$RESULT"
    fi
  fi
}

# A cell that fails must not take the run with it.
#
# `set -e` plus a non-zero return from `cell` killed a 12-cell run after cell 6, silently and
# with no message in the output -- so the surviving file looked like a complete set of c=1
# results rather than like half a run. Reporting the cell and continuing is what a reader
# needs: the missing row is visible, and the rows that did measure are still worth having.
run_cell() {
  if ! cell "$@"; then
    echo "$1 c=$7: cell failed to run (see stderr above); continuing" | tee -a "$RESULT"
  fi
  return 0
}

echo "scenarios (overhead = through - direct, both p50 ms)" | tee -a "$RESULT"
echo | tee -a "$RESULT"

for concurrency in 1 8; do
  # S1: pure proxy, no translation.
  run_cell "S1-proxy-nonstream" 1 "$MODEL_OPENAI" "openai.json" \
    "/v1/chat/completions" "/v1/chat/completions" "$concurrency" json
  # S2: OpenAI client, Claude provider, non-streaming.
  run_cell "S2-translate-nonstream" 1 "$MODEL_CLAUDE" "openai.json" \
    "/v1/chat/completions" "/v1/messages" "$concurrency" json
  # S3: streamed, no translation.
  run_cell "S3-proxy-stream-200" 200 "$MODEL_OPENAI" "openai-stream.json" \
    "/v1/chat/completions" "/v1/chat/completions" "$concurrency" stream
  # S4: streamed, translated.
  run_cell "S4-translate-stream-200" 200 "$MODEL_CLAUDE" "openai-stream.json" \
    "/v1/chat/completions" "/v1/messages" "$concurrency" stream
  # S5: the headline. What a coding agent produces all day.
  run_cell "S5-translate-stream-2000" 2000 "$MODEL_CLAUDE" "openai-stream.json" \
    "/v1/chat/completions" "/v1/messages" "$concurrency" stream
  # S6: S2 with the translation removed and nothing else changed. S2 - S6 is the
  # translation cost in situ, which the micro-benchmarks independently predict.
  run_cell "S6-claude-native-nonstream" 1 "$MODEL_CLAUDE" "claude.json" \
    "/v1/messages" "/v1/messages" "$concurrency" json
done

echo | tee -a "$RESULT"
echo "written to $RESULT" | tee -a "$RESULT"
echo | tee -a "$RESULT"
cat >> "$RESULT" <<'NOTE'
Reading this:
  * `overhead` is the router's cost. `direct` is the mock's own service time and should
    sit near --mock-sleep-ms; if it does not, the mock or the machine is the problem and
    the overhead column is not trustworthy.
  * If overhead lands on a round fraction of --mock-sleep-ms, suspect the harness rather
    than celebrating the result: it is probably timing the mock.
  * A wide [min..max] means a noisy machine, not a fast router. Re-run before quoting.
  * S5 is the headline: per-frame cost multiplies, and it is the shape real agent traffic
    takes.
NOTE
