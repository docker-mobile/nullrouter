#!/usr/bin/env bash
# Bring nullrouter up from release binaries for benchmarking, and tear it down again.
#
# Not a deployment script. It exists so the benchmark starts the router the same way twice,
# and so the teardown actually kills all eight services — a leftover runtime holding :20132
# makes the next run measure the previous build.
#
#   benches/serve.sh up     # start, wait for health, print the gateway port
#   benches/serve.sh down   # stop everything
#   benches/serve.sh status
#
# State goes to a scratch file, not the user's real one.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release"
RUN="${NULLROUTER_BENCH_RUN:-/tmp/nullrouter-bench}"
STATE_FILE="$RUN/state.json"
LOG_DIR="$RUN/logs"

# state must be first: runtime and api read credentials from it.
SERVICES=(
  "nullrouter-state:20134"
  "nullrouter-runtime:20132"
  "nullrouter-api:20129"
  "nullrouter-events:20133"
  "nullrouter-catalog:20131"
  "nullrouter-auth:20135"
  "nullrouter-dashboard-host:20130"
  "nullrouter-gateway:20128"
)

mkdir -p "$RUN" "$LOG_DIR"

pidfile() { echo "$RUN/$1.pid"; }

is_up() { # a service is up when its port answers, not when its pid exists
  local port="$1"
  timeout 1 bash -c "</dev/tcp/127.0.0.1/$port" 2>/dev/null
}

up() {
  for entry in "${SERVICES[@]}"; do
    local name="${entry%%:*}" port="${entry##*:}"
    if is_up "$port"; then
      echo "$name: already listening on $port — refusing to start a second one" >&2
      echo "run 'benches/serve.sh down' first, or the benchmark measures whichever answers" >&2
      exit 1
    fi
  done

  for entry in "${SERVICES[@]}"; do
    local name="${entry%%:*}" port="${entry##*:}"
    [[ -x "$BIN/$name" ]] || {
      echo "missing $BIN/$name — build first: cargo build --release --workspace" >&2
      exit 3
    }

    # Only state takes the state file; the others reach it over the wire.
    #
    # NULLROUTER_REQUIRE_API_KEY is on for a fairness reason, not a security one: the baseline
    # validates a Bearer key on /v1, and nullrouter does not by default. Leaving it off
    # would hand nullrouter a free pass on work the baseline is doing. The matching
    # `requireApiKey` setting has to be turned on in state too, or the runtime does not
    # enforce it at the last hop -- benches/configure.sh does that.
    if [[ "$name" == "nullrouter-state" ]]; then
      NULLROUTER_STATE_FILE="$STATE_FILE" "$BIN/$name" > "$LOG_DIR/$name.log" 2>&1 &
    else
      NULLROUTER_REQUIRE_API_KEY=true "$BIN/$name" > "$LOG_DIR/$name.log" 2>&1 &
    fi
    echo $! > "$(pidfile "$name")"

    # Wait for this one before starting the next: the runtime reads state at boot, and
    # starting them together makes the first few requests race the credential load.
    local waited=0
    until is_up "$port"; do
      sleep 0.2
      waited=$((waited + 1))
      if [[ $waited -gt 100 ]]; then
        echo "$name did not open $port in 20s. Log:" >&2
        tail -20 "$LOG_DIR/$name.log" >&2
        down
        exit 1
      fi
    done
    echo "$name up on $port"
  done

  echo
  echo "gateway: http://127.0.0.1:20128"
  echo "state:   $STATE_FILE"
  echo "logs:    $LOG_DIR"
}

down() {
  for entry in "${SERVICES[@]}"; do
    local name="${entry%%:*}"
    local pf; pf="$(pidfile "$name")"
    if [[ -f "$pf" ]]; then
      local pid; pid="$(cat "$pf")"
      kill "$pid" 2>/dev/null || true
      rm -f "$pf"
    fi
  done
  # Give them a moment to release ports, then report anything still holding one:
  # a benchmark against a stale process is worse than one that fails to start.
  sleep 1
  for entry in "${SERVICES[@]}"; do
    local name="${entry%%:*}" port="${entry##*:}"
    if is_up "$port"; then
      echo "warning: $port still listening after stopping $name" >&2
    fi
  done
  echo "stopped"
}

status() {
  for entry in "${SERVICES[@]}"; do
    local name="${entry%%:*}" port="${entry##*:}"
    if is_up "$port"; then echo "$name: up ($port)"; else echo "$name: down ($port)"; fi
  done
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  status) status ;;
  *) echo "usage: $0 {up|down|status}" >&2; exit 2 ;;
esac
