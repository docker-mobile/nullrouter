#!/usr/bin/env bash
# Point nullrouter at the mock provider, the same way 9Router is pointed at it.
#
# It imports 9Router's own config rather than building an equivalent one by hand. That is
# the point: hand-writing "the same" two provider definitions is how a comparison quietly
# stops being one. `/api/migrate/9router` reads ~/.9router directly, so both routers end up
# serving the same nodes with the same baseUrls.
#
# API keys are the one thing migration cannot carry: nullrouter stores a digest, so a
# plaintext key cannot be recreated from an import. This mints a fresh one and prints it.
#
#   benches/configure.sh            # import, mint a key, enable enforcement
#   benches/configure.sh --dry-run  # show what would be imported, write nothing
#
# Prints the key on the last line so a harness can capture it.

set -euo pipefail

GATEWAY="${GATEWAY:-http://127.0.0.1:20128}"
PASSWORD="${NULLROUTER_PASSWORD:-123456}"
COOKIES="$(mktemp)"
trap 'rm -f "$COOKIES"' EXIT

DRY_RUN=false
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=true

say() { echo "==> $*"; }
fail() { echo "error: $*" >&2; exit 1; }

# ── sign in ──────────────────────────────────────────────────────────────────
# Checked first, and by name, because every later failure looks like a router problem
# otherwise: the import succeeds, the key mints, and only the final dispatch fails with a
# bad_gateway that reads as nullrouter's fault.
MOCK_PORT="${MOCK_PORT:-8099}"
timeout 1 bash -c "</dev/tcp/127.0.0.1/$MOCK_PORT" 2>/dev/null || fail \
  "no mock provider on :$MOCK_PORT. Start it first:
  cargo build -p mock-provider --release
  ./target/release/mock-provider --port $MOCK_PORT --sleep-ms 25 --frames 200 --frame-chars 12 &"

say "signing in at $GATEWAY"
login="$(curl -s -c "$COOKIES" -X POST "$GATEWAY/api/auth/login" \
  -H 'content-type: application/json' -d "{\"password\":\"$PASSWORD\"}")"
grep -q auth_token "$COOKIES" || fail "login failed: $login"

# ── import 9Router's configuration ───────────────────────────────────────────
if $DRY_RUN; then
  say "dry run: what would be imported"
  curl -s -b "$COOKIES" -X POST "$GATEWAY/api/migrate/9router" \
    -H 'content-type: application/json' -d '{"dryRun":true}'
  echo
  exit 0
fi

say "importing ~/.9router"
report="$(curl -s -b "$COOKIES" -X POST "$GATEWAY/api/migrate/9router" \
  -H 'content-type: application/json' -d '{}')"
echo "$report"

# The benchmark needs the router to *have* connections; how they got there is not the
# point. Asserting on `connectionsImported` would be wrong twice over: the import is
# idempotent, so a correct second run reports 0 and skips both, and a first run reporting a
# nonzero count still would not prove the router can serve them. Ask the router instead.
count="$(curl -s -b "$COOKIES" "$GATEWAY/api/providers" \
  | grep -o '"provider"' | wc -l | tr -d ' ')"
[[ "$count" -gt 0 ]] || fail "router has no provider connections after import: $report"
say "router has $count connection(s)"

# ── mint a client key ────────────────────────────────────────────────────────
say "minting an API key"
key_json="$(curl -s -b "$COOKIES" -X POST "$GATEWAY/api/keys" \
  -H 'content-type: application/json' -d '{"name":"bench"}')"
KEY="$(echo "$key_json" | sed -n 's/.*"key":"\([^"]*\)".*/\1/p')"
[[ -n "$KEY" ]] || fail "no key in response: $key_json"

# ── enforce it, so both routers validate a key on /v1 ────────────────────────
# The gateway flag alone is not enough: the runtime re-checks the persisted setting at the
# last hop before a provider call, by design, so a dashboard toggle is never silently
# ignored. Both have to be on or the two sides are not doing the same work.
say "enabling requireApiKey"
curl -s -b "$COOKIES" -X PUT "$GATEWAY/api/settings" \
  -H 'content-type: application/json' -d '{"requireApiKey":true}' > /dev/null

# ── prove it end to end before the benchmark trusts it ───────────────────────
say "checking the key is required"
unauth="$(curl -s -o /dev/null -w '%{http_code}' -X POST "$GATEWAY/v1/chat/completions" \
  -H 'content-type: application/json' \
  -d '{"model":"benchopenai/bench-model","messages":[{"role":"user","content":"hi"}]}')"
[[ "$unauth" == "401" || "$unauth" == "403" ]] \
  || echo "warning: unauthenticated request returned $unauth, expected 401/403 -- enforcement may be off" >&2

say "checking a request reaches the mock"
body="$(curl -s -m 20 -X POST "$GATEWAY/v1/chat/completions" \
  -H "Authorization: Bearer $KEY" -H 'content-type: application/json' \
  -d '{"model":"benchopenai/bench-model","messages":[{"role":"user","content":"hi"}]}')"
echo "$body" | grep -q 'chatcmpl-bench' \
  || fail "request did not reach the mock (is it running on :8099?): $body"

say "ready"
echo "$KEY"
