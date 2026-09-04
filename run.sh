#!/usr/bin/env bash
# Start every nullrouter service, in the order they depend on each other.
#
# There are eight binaries because the port is a set of microservices, but nobody should need eight
# terminals to try it. This starts them all, waits until the gateway actually answers, and shuts the whole
# set down on Ctrl-C — including on failure, so a half-started run does not leave six processes behind.
#
# The state service goes first and is waited for: the runtime and API read credentials and usage from it,
# and one that starts against an absent state service logs a connection error per request until it is up.
#
# Not a substitute for a process manager in production. This is the "try it" path; `DEPLOY.md` covers the
# other one.
set -euo pipefail

cd "$(dirname "$0")"

# Persistence is opt-in upstream and easy to miss, which costs a user every connection they just added.
# Defaulted here, and the choice is printed below so it is never a surprise.
: "${NULLROUTER_STATE_FILE:=./nullrouter-state.json}"
export NULLROUTER_STATE_FILE

: "${GATEWAY_PORT:=20128}"
: "${STATE_PORT:=20134}"

# Five services read a bare `PORT` before their own `NULLROUTER_*_PORT`, so a `PORT` inherited from the
# shell — normal in a container or a PaaS shim — would send all five at one port and four would fail to
# bind. Cleared for the children only; each service keeps its own default.
unset PORT

# Release by default: a debug gateway adds latency to every request, which is the wrong first impression.
# `./run.sh --debug` skips the optimisation for a faster edit-run cycle.
PROFILE_ARGS=(--release)
PROFILE_DIR=release
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE_ARGS=()
    PROFILE_DIR=debug
    shift
fi

log() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==>\033[0m %s\n' "$*" >&2; }

PIDS=()
cleanup() {
    local status=$?
    # Disarmed first. A Ctrl-C runs this via the INT trap, whose `exit` then fires the EXIT trap and
    # runs it a second time — which prints the shutdown line twice and signals pids already reaped.
    trap - EXIT INT TERM
    # SIGINT, not SIGTERM. Pingora reads SIGTERM as "graceful" and sleeps out its whole grace period
    # even with nothing in flight, so a SIGTERM here would leave the gateway sitting there after
    # everything else had gone. SIGINT is its immediate path, and the actix services stop on either.
    #
    # `kill` on an already-dead pid is an error under `set -e`, and a shutdown path that fails partway
    # leaves exactly the orphans this trap exists to prevent.
    if ((${#PIDS[@]} > 0)); then
        log "stopping ${#PIDS[@]} services"
        kill -INT "${PIDS[@]}" 2>/dev/null || true
        wait "${PIDS[@]}" 2>/dev/null || true
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

# ── The dashboard bundle ────────────────────────────────────────────────────────────────────────────
# Without it the dashboard, the sign-in screen, and the OAuth callback are all empty shells, so this is
# built before anything starts rather than discovered as a blank page.
BUNDLE=services/dashboard-actix/static/pkg/dashboard_leptos_bg.wasm

# Rebuilt when any dashboard source is newer than the bundle, not only when the bundle is missing.
# Presence alone is the wrong test: after an edit, the stale bundle is still a file, so the old UI
# gets served with no indication that what is running is not what is checked out — which reads as
# "my change did nothing" rather than "the bundle was not rebuilt".
bundle_is_stale() {
    [[ ! -f "$BUNDLE" ]] && return 0
    local newer
    newer=$(find apps/dashboard-leptos/src apps/dashboard-leptos/Cargo.toml \
        -newer "$BUNDLE" -print -quit 2>/dev/null)
    [[ -n "$newer" ]]
}

if bundle_is_stale; then
    log "building the dashboard WASM bundle"
    # Version-pinned to the `wasm-bindgen` in `Cargo.lock`: the CLI refuses a bundle built by a
    # different minor version, and the error names schema numbers rather than the mismatch.
    WASM_BINDGEN_VERSION=$(sed -n '/^name = "wasm-bindgen"$/,/^version/{s/^version = "\(.*\)"$/\1/p;}' Cargo.lock | head -1)
    if [[ "$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')" != "$WASM_BINDGEN_VERSION" ]]; then
        log "installing wasm-bindgen-cli $WASM_BINDGEN_VERSION"
        cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION" --locked
    fi
    rustup target add wasm32-unknown-unknown
    cargo build -p nullrouter-dashboard-wasm --lib --target wasm32-unknown-unknown --release
    wasm-bindgen --target web \
        --out-dir services/dashboard-actix/static/pkg \
        --out-name dashboard_leptos \
        target/wasm32-unknown-unknown/release/nullrouter_dashboard_wasm.wasm
fi

# ── Build once, then run the binaries ───────────────────────────────────────────────────────────────
# `cargo run` per service would take the build lock in turn and serialise eight compilations. One build
# up front, then eight execs.
log "building the services"
cargo build "${PROFILE_ARGS[@]}" --workspace --exclude nullrouter-dashboard-wasm

start() {
    local name=$1
    "./target/$PROFILE_DIR/$name" &
    PIDS+=($!)
}

# State first, and waited for: everything else reads credentials and usage from it.
log "starting nullrouter-state on :$STATE_PORT (persisting to $NULLROUTER_STATE_FILE)"
start nullrouter-state
for _ in $(seq 1 50); do
    if curl -fsS "http://127.0.0.1:$STATE_PORT/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.2
done

log "starting the remaining services"
for service in nullrouter-runtime nullrouter-api nullrouter-events \
    nullrouter-catalog nullrouter-auth nullrouter-dashboard-host nullrouter-gateway; do
    start "$service"
done

# ── Wait for the one public port ────────────────────────────────────────────────────────────────────
# Reporting readiness before the gateway answers would send a user to a connection-refused page and make
# them think the whole thing failed.
for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:$GATEWAY_PORT/api/health" >/dev/null 2>&1; then
        printf '\n'
        log "nullrouter is up"
        printf '    Dashboard  http://127.0.0.1:%s/dashboard\n' "$GATEWAY_PORT"
        printf '    API        http://127.0.0.1:%s/v1\n' "$GATEWAY_PORT"
        printf '    State file %s\n\n' "$NULLROUTER_STATE_FILE"
        # Only when the default is actually in force. Printed unconditionally it told an operator who
        # had set a strong password that theirs was `123456`, which is both false and the kind of
        # warning that teaches people to ignore warnings. `NULLROUTER_AUTH_PASSWORD_HASH` wins over
        # `INITIAL_PASSWORD` in the auth service, so either one being set means the default is gone.
        if [[ -z "${INITIAL_PASSWORD:-}" && -z "${NULLROUTER_AUTH_PASSWORD_HASH:-}" ]]; then
            warn "dashboard password is the default 123456 — set INITIAL_PASSWORD before exposing this anywhere"
        fi
        printf '\nCtrl-C to stop all services.\n'
        wait
    fi
    sleep 0.2
done

warn "the gateway did not answer on :$GATEWAY_PORT within 20s — check the output above"
exit 1
