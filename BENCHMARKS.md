# Benchmarks

The README claims nullrouter is "vastly faster" and "lighter to run" than 9Router. **Both are now
measured**, and this document is the harness they had to survive — plus the rules that keep the
comparison honest rather than flattering.

## Where it stands

Twelve cells, both legs of each in the same run against the same mock. Full files in
`benches/results/`; the numbers below are `20260830T185309Z-nullrouter-fair` against
`20260830T132648Z-9router`.

| | router overhead | end-to-end |
|---|---|---|
| best cell | 61.2× (S3, c=8) | 17.7× (S5, c=8) |
| median | 9.5× | 2.0× |
| worst cell | 6.41× (S2, c=1) | 1.29× (S1, c=1) |

**Use the `fair` run, not `opt5`, and here is why.** `opt5` reports slightly better figures — 1.23 ms
against 1.38 ms on S1 c=1 — because the runtime was not enforcing API keys during it. `requireApiKey`
could not be persisted at all (`PUT /api/settings` accepted it and silently dropped it; fixed in
510b6f1), so the gateway enforced from its environment variable while the runtime skipped its own
check, saving one state round trip per request. One key check per request on each side, which is
what 9Router does too — fair by accident, and not the configuration the harness claimed. `fair` is
the run where both layers enforce.

**Both figures matter and they answer different questions.** Router overhead is
`p50(through) − p50(direct)` — a fact about the router, and the number to watch when optimising it.
End-to-end is the whole request including the mock's 25.27ms of service time — what a caller
actually waits for. Quoting 66× to a user would be true of the router and misleading about their
request. Quote the end-to-end figure; keep the overhead figure for deciding what to fix next.

Resources, 8 processes idle from a fresh start: 108.8 MiB and 0.19 CPU-seconds over 15s, against
9Router's 228.3 MiB and 0.01 in one process. Half the memory, 19× the idle CPU — small absolutely
(0.95% of a core against 0.05%), and stated because reporting only the memory win would be
selective.

**The first honest run had nullrouter losing three of twelve cells**, every non-streaming cell at
c=1, by 4–6ms. That is recorded in `20260830T125524Z-nullrouter` and was the starting point for
everything in "The overhead is state round trips" below. A document that only showed the final
numbers would hide the fact that the claim was false when first tested.

## What is actually being claimed

Two separable things, which need separate measurement:

1. **Latency the router adds** to a request that would have happened anyway.
2. **Resources the router costs** to keep running: memory, CPU per unit of work, install size.

Neither is "how fast is the provider". Both are measured against a mock provider with a *fixed*
service time, so what varies between runs is only the router.

## The control that makes any of this meaningful

Router overhead is:

```
overhead = latency(client → router → mock) − latency(client → mock)
```

Both legs measured **in the same run, against the same mock**. Without the subtraction the figures are
dominated by the mock's own sleep and every router looks identical.

**The specific trap:** if measured overhead lands on a suspiciously round fraction of the mock's sleep
time, that is the bug, not a result — the harness is timing the mock, not the router.

## Micro-benchmarks (criterion)

Cheap to run, and the only way to attribute a regression to a specific function later. These do **not**
support the comparative claim; they exist so a slow path is findable.

### `crates/translate` — the hot path

| Bench | Body | Why |
|---|---|---|
| `openai_to_claude/small` | 4 turns, ~500 tokens | Typical interactive turn |
| `openai_to_claude/large` | 60 turns, ~100k chars | Where allocation shows up |
| `openai_to_claude/tools` | 12 tool defs + 3 results | The most branch-heavy path |
| `claude_to_openai/*` | Same three, reversed | Both directions, not just one |
| `gemini_to_openai/small` | 4 turns with an image part | Media re-encoding |
| `passthrough/openai_to_openai` | Same format both ends | **Control.** Must be near zero |

That last row is the important one. If same-format passthrough is not close to free, the router is
paying translation cost on requests that need none.

### `crates/translate` — streaming

Per-frame cost is what dominates a long response, so measure per frame, not per response:

| Bench | Shape |
|---|---|
| `sse_frame/openai_to_claude` | One `chat.completion.chunk` → Claude events |
| `sse_frame/claude_to_openai` | One `content_block_delta` → OpenAI chunk |
| `sse_frame/with_tool_call` | A frame carrying a partial tool call |
| `sse_stream/2000_frames` | End-to-end, steady state |

Report `sse_stream/2000_frames` as **frames/sec at steady state**. That converts directly to
per-1000-token cost without the reader doing arithmetic.

### `crates/providers` — per-request lookups

Called several times per request; must stay in nanoseconds:

- `capability/explicit_row` — a model with its own capability row
- `capability/fallback_by_name` — the model-name fallback path
- `model/alias_resolution` — `ds/deepseek-v4-pro` → canonical
- `transport/format_lookup` — multi-transport selection (new, unmeasured)

### `services/runtime-actix` — combo resolution

Takes a lock on shared rotation state, so contention is the interesting axis:

- `combo/fill_first_5` and `combo/round_robin_5_sticky_3` — uncontended baselines
- `combo/contended_8_threads` — **the one that matters.** Eight threads on one combo's cursor

## First results (micro only)

Run on this sandbox, 2026-08-29. Machine-specific and not a comparison — recorded because the control
already found something.

| Bench | Time |
|---|---|
| `passthrough/clone_only` | 760 ns |
| `passthrough/extract_thinking_absent` | 116 ns |
| `passthrough/apply_thinking_absent` | 631 ns |
| `passthrough/openai_to_openai` | **1.68 µs** |
| `claude_to_openai/small` | 3.58 µs |
| `gemini_to_openai/with_image` | 3.22 µs |
| `openai_to_claude/small` | 6.46 µs |
| `openai_to_claude/tools` | 61.6 µs |
| `openai_to_claude/large` (~100k chars) | 147 µs |
| `sse_frame/claude_to_openai` | 1.34 µs |
| `sse_frame/openai_to_claude` | 3.50 µs |
| `sse_frame/with_tool_call` | 3.72 µs |
| `sse_stream/2000_frames` | 2.27 ms (≈880k frames/s) |
| `lookup/explicit_row` | 348 ns |
| `lookup/capability_miss` | 674 ns |
| `combo/fill_first_5` | 103 ns |
| `combo/fusion_5` | 102 ns |
| `combo/round_robin_5_sticky_3` | 464 ns |
| `combo/round_robin_5_sticky_1` | 468 ns |
| `combo/contended_8_threads` | 4.60 ms / 2000 selections = **2.30 µs each** (435k/s) |

**What the control found.** Same-format passthrough is 1.68 µs, not near-zero, and the three floors
above account for it: 760 + 116 + 631 = 1507 ns. The clone is unavoidable — `model` has to be rewritten
and the caller's body is borrowed. The other 747 ns is the thinking pipeline running on a request that
mentions no reasoning at all, which is the common case.

`thinking::apply` costs 631 ns there because it calls `capabilities_for_model` unconditionally. That
lookup is load-bearing, not waste: a model that cannot reason must have thinking fields *stripped*
even when the client sent none, since the body may still carry a spelling from the source dialect and
an unrecognised field is a 400 on several providers.

There is a reordering available — `extract_thinking_object` costs 116 ns against the lookup's 631, so
checking the body first would skip the lookup when there is provably nothing to strip. **Deliberately
not done yet.** It saves ~0.5 µs on a request that makes a network round trip, and the end-to-end
numbers below should decide whether that is worth touching a correctness-critical path for. Recorded
here so the option is not lost.

**Lookup misses cost more than hits.** `capability_miss` (674 ns) is nearly twice `explicit_row`
(348 ns), because a name with no registry row exhausts every path — explicit table, prefix rules,
family fallback — before returning the default. That is the *common* case, not the exotic one: every
`openai-compatible-*` provider a user adds by hand misses. It is 674 ns against a network round trip,
so it is not urgent, but it is the wrong way round and worth knowing if provider lookup ever appears
in a profile.

**Round-robin is the only per-request lock.** 464 ns versus fill-first's 103 ns is the cost of taking
the rotation mutex and advancing a cursor; a cursor is meaningless without shared state, so this is
inherent rather than sloppy. Sticky-3 and sticky-1 are indistinguishable (464 vs 468 ns) — the counter
increments either way and only the modulo differs, so tightening stickiness costs nothing.

Under 8 threads on one combo's cursor it degrades to 2.30 µs per selection, 4.95× the uncontended
figure, saturating at ~435k selections/s. That ceiling is far above any real router's request rate, and
against a 25 ms upstream call 2.30 µs is 0.009% — so the mutex is not worth replacing. The number is
recorded because it is the one place where contention exists at all, and a future regression that made
`fill_first` or `fusion` start locking would show up as their cost climbing toward this row.

## The bug this whole exercise paid for: streamed responses cost twice their latency

Found while chasing a bimodal streaming cell. Fixed in `services/{runtime,events,api}-actix/src/main.rs`.

Every streamed response cost a client using keep-alive **roughly double** its real latency:

| | keep-alive on | keep-alive off |
|---|---|---|
| nullrouter, streamed, before | **79.97 ms** | 40.35 ms |
| nullrouter, streamed, after | **39.16 ms** | 39.98 ms |
| 9Router, streamed | 49.83 ms | 49.03 ms |

The accept socket had no `TCP_NODELAY`. A response leaves as a small header write followed by
the body; once a connection has settled out of Linux's initial quickack mode, the client's ACK is
delayed, Nagle holds the body behind the unacknowledged header segment, and the body lands when the
client's ~40 ms timer fires. Timed on loopback with a raw socket:

```
request 1: recv 349B @48.2ms, recv 11008B @49.1ms, ... @49.9ms     ← fresh connection, fine
request 2: recv 349B @46.5ms, recv 34574B @87.8ms                  ← 41 ms of nothing
request 3: recv 349B @44.3ms, recv 34574B @88.0ms
```

**Every way of looking at it except one said the router was fine.** Functional tests pass — the
bytes are all correct, just late. The first request on any connection is unaffected, so anything
that opens a connection per request sees nothing. The non-streaming cells see nothing. `curl` one
request at a time sees nothing. It needed a streamed response, on a reused connection, measured
end to end. 9Router never had it: Node sets nodelay on HTTP sockets by default.

Pingora already sets nodelay on both accept and connect, so the gateway was clean and was merely
inheriting the runtime's defect through its upstream connection.

`run.sh` now measures every streaming cell both with and without keep-alive and prints the ratio,
because that divergence is the only signal that showed this. A ratio much above 1 means reused
connections are paying for something.

## Where the overhead was, and where it went

The first thing the harness found, and it is worth more than every micro-benchmark above: nullrouter
added **~14 ms to a 25 ms upstream call**, and translation was a rounding error inside it.

This section is kept in the order it was discovered rather than rewritten to the conclusion, because
the wrong diagnosis in the middle of it is the instructive part: the round trips were counted
correctly, blamed correctly for being numerous, and were not the cost.

Measured on this sandbox, `oha`, concurrency 1, non-streaming, mock at `--sleep-ms 25`, **before any
of the fixes below**:

| Path | p50 | Attributable to |
|---|---|---|
| Client → mock (control) | 25.36 ms | the mock's own fixed sleep |
| Client → **runtime** (`:20132`) → mock | 38.96 ms | **13.60 ms** of runtime |
| Client → gateway (`:20128`) → runtime → mock | 40.51 ms | **1.55 ms** of gateway |
| One `GET /internal/v1/routing-context` | 1.92 ms | one state round trip |

A chat request makes five state calls before the provider is dialled:

| Caller | What it needs | Cost |
|---|---|---|
| `enforce_api_key` | `settings.require_api_key` — one bool | one `routing-context` |
| `enforce_api_key` | the key itself | one `validate-api-key` |
| `pxpipe_settings` | three pxpipe numbers | one `routing-context` |
| `resolve_targets` | combos, or a user-defined node prefix | one `routing-context` |
| credential selection | the connection to use | one `credentials/select` |

Five at ~1.9 ms is ~9.6 ms of the 13.60 ms. Three of the five fetch the *same* `routing-context`
payload — every connection and every combo — to read a bool, three numbers, and a prefix map.

Checked rather than assumed: turning `requireApiKey` off removed **1.29 ms**. That also corrected the
count. I predicted two round trips would disappear and only one did, because `enforce_api_key` fetches
`routing-context` to read the flag *before* it can know the flag is off — so the fetch happens either
way and only `validate-api-key` is skipped. One round trip, ~1.3 ms measured against ~1.9 ms
predicted, which is the right order at this precision.

Against these figures the 0.5 µs of thinking-pipeline reordering discussed above is 0.004% of the
overhead. The micro-benchmarks were measuring three orders of magnitude below where the cost lives.
That is the argument for building the end-to-end harness before optimising anything.

### What was actually fixed, and where the prediction above went wrong

The paragraph that used to sit here said the fix was "one fetch per request threaded through the call
chain, **not** a TTL cache", on the grounds that a cache would trade a stated guarantee for latency.
That reasoning was sound and the diagnosis under it was wrong: **the round trips were not the main
cost.** The state service's own handler was.

Every read went through `read_snapshot()`, which clones the entire `StateSnapshot` — api keys,
connections, combos, proxy pools, settings, translator panes, and a usage log that sits at 350 KB of a
490 KB state because it retains 1000 records. `routing-context` called three such accessors, so it
cloned ~1.5 MB per request and read none of the usage log. Worse, every mutation serialised and wrote
the whole 490 KB state file inline, and two mutations happen per request: round-robin credential
selection advances a cursor, and usage recording appends a row.

| Change | Effect |
|---|---|
| `with_snapshot` projection instead of cloning | `routing-context` 1.71 ms → 0.085 ms (572 → 11197 req/s) |
| Deferred persistence with a 250 ms background flush | S1 c=1 overhead 7.74 ms → 1.48 ms |
| Spawn the usage POST instead of awaiting it | removes ~1.7 ms after the response already exists |
| 250 ms `routing-context` cache in the runtime's client | collapses three identical reads to one |
| Quadratic `LineBuffer::push` rewritten | S5 c=1 32.81 ms → 12.56 ms |
| Coalesce queued SSE frames into one write | S3 c=8 3.25 ms → 2.75 ms |
| 250 ms key-validation cache in auth | authorize 0.230 ms → 0.074 ms |

So a TTL cache *was* used, in two places, having first removed the cost that made the round trips look
like the problem. The guarantee it appeared to trade is intact in both cases:

- The **routing-context** cache bounds a dashboard change at 250 ms before it takes effect. That is
  below the point a user notices a toggle, and 9Router re-reads its own SQLite config per request too,
  so this is not a divergence in kind.
- The **key-validation** cache in auth does not weaken enforcement at all, because the runtime
  validates every key against state independently and uncached. A key revoked inside the TTL still
  fails: the gateway forwards it on a stale `authorized: true` and the runtime rejects it. The cost of
  a stale hit is one wasted hop, not an accepted request.

Two things were *not* done, and for the same reason in both cases. The runtime's own
`validate_api_key` is not cached — there is no second check behind it, so caching it would genuinely
delay revocation. And the runtime does not trust a "gateway already checked this" header, because
`NULLROUTER_RUNTIME_HOST` is configurable: on a non-loopback binding, that header would let anyone
bypass key enforcement entirely.

### What the remaining overhead is, measured rather than reasoned about

1.38 ms at c=1 non-streaming, attributed with `benches/hop-probe` and per-endpoint `oha` runs.

**Measure the hops against a zero-latency mock, not the 25 ms one.** Subtracting two numbers that
both contain a 25 ms sleep leaves the difference dominated by scheduling and wake-up latency, which
inflates it: the runtime hop reads 0.772 ms that way and 0.425 ms with `--sleep-ms 0`. The first
version of this table used the inflated figure and attributed 0.575 ms to the actix cycle. A
`/health` request through the same actix stack takes 0.073 ms, which was the clue that the
attribution was wrong rather than the framework being slow.

Against `--sleep-ms 0`:

| Piece | Cost |
|---|---|
| mock control | 0.061 ms |
| runtime hop | **0.425 ms** |
|  ├ actix request/response cycle (`/health` floor) | 0.073 ms |
|  ├ `state keys/validate` | 0.079 ms |
|  ├ `state credentials/select` | 0.084 ms |
|  ├ `reqwest` over a minimal client | 0.038 ms |
|  ├ translation, both directions | 0.004 ms |
|  ├ serde parse + serialise | 0.001 ms |
|  └ **unattributed** | **~0.15 ms** — pipeline control flow between those calls |
| gateway hop | **0.353 ms** |
|  ├ `auth authorize` | 0.079 ms (was 0.230 before the cache) |
|  └ **unattributed** | **~0.27 ms** — Pingora's own proxying |

The two unattributed figures are the limit of what this sandbox can resolve: `perf`, `valgrind` and
`flamegraph` are all absent, so the cost is locatable to the pipeline's control flow and Pingora's
proxying but not to a function inside them. Recorded as unattributed rather than guessed at.

### A change that measured well and was still wrong: gateway worker threads

Pingora's `threads` defaults to 1, and the gateway is the only public port — so every request on the
box goes through one worker. Against a **zero-latency** mock that is plainly a ceiling: the gateway
saturates a core at 98% and plateaus at 7013 req/s while the runtime behind it still has headroom at
11605. Raising it to the core count lifted that to 8788 req/s, +25%.

Then the same load against a **250 ms** mock — a realistic provider — at c=64: the gateway sits at
**3% of one core**. The ceiling is two orders of magnitude away from anything a real deployment
reaches, because provider latency dominates.

And the threads were not free. S1 and S2 at c=8 went from 1.61 and 1.66 ms to 1.83 and 1.81 ms, with
trial ranges that do not overlap — a real cost on every request, not noise.

So the default stayed at 1 and the change became `NULLROUTER_GATEWAY_THREADS`, for the case where
the upstream really is that fast (a local llama.cpp on the same box) and the tradeoff inverts.

The general point, which cost two benchmark runs to learn twice: **a saturation measurement taken
against an unrealistically fast dependency measures the harness, not the system.** The same error
inflated the hop attribution above, in the opposite direction — there the mock was too *slow* and
scheduling noise landed in the difference.

One cheap win is visible without a profiler: `chat_entrypoint` parses the request body **twice**,
once as `serde_json::Value` and once as a typed `ChatPayload`, from the same bytes. At 0.001 ms per
parse that is not where the 0.15 ms is, which is exactly why it is recorded here rather than fixed in
a hurry — it would be a change with no measurable effect, and this document already contains one of
those.

The `translate` row is the point worth keeping: 0.004 ms, against 0.5 µs of thinking-pipeline
reordering in the micro-benchmarks above. Both are three orders of magnitude below where the cost
lives. That is the argument for building the end-to-end harness before optimising anything.

`services/runtime-actix/tests/node_prefix_routing.rs` asserted the prefix-resolution fetch as a
*difference* from the baseline count rather than pinning the absolute number, specifically so it would
survive this fix. It did not quite: with the context cached, prefix resolution costs *no* extra fetch,
so the assertion became equality plus "the context is still fetched at least once" — the stronger form
of the same property.

## End-to-end overhead

Where the comparative claim lives.

### The mock provider

One binary, used identically by both routers — this is the control that makes comparison possible:

- Serves `/v1/chat/completions` and `/v1/messages`, streaming and not
- **Fixed** sleep for service time, not a random one: variance here becomes variance in the result
- Configurable response size and frame count
- No disk I/O on the hot path

A bench-only crate, not the test helpers: those hold mutexes and run assertions that would show up in
the measurement.

### Scenarios

| # | Shape |
|---|---|
| S1 | OpenAI client → OpenAI provider, non-streaming (pure proxy, no translation) |
| S2 | OpenAI client → Claude provider, non-streaming (translation both ways) |
| S3 | OpenAI client → OpenAI provider, streamed, 200 frames |
| S4 | OpenAI client → Claude provider, streamed, 200 frames |
| S5 | OpenAI client → Claude provider, streamed, 2000 frames, 12 tools |
| S6 | Claude client → Claude provider, non-streaming (**no** translation — the control for S2) |

**S5 is the headline.** It is what a coding agent produces all day, and where per-frame cost
multiplies. A per-request-only benchmark understates the thing users feel.

S6 is S2 with the translation removed and nothing else changed: same provider, same mock, same body
size, only the client dialect differs. S2 − S6 is therefore the translation cost in situ, and the
micro-benchmarks predict it. If the difference is far off that prediction, one of the two numbers is
wrong.

**S6 was originally specified as `Claude client → deepseek`, to measure multi-transport selection
removing the translation hop. That is not measurable here and the scenario was changed.** A registry
provider's URL comes from its transport table; `base_url()` is only consulted for the
`openai-compatible`, `anthropic-compatible`, and `ollama-local` families, so a connection cannot
point `deepseek` at the mock. Reaching it would need either a `baseUrl` override added to registry
providers — changing the program to suit the benchmark — or a hosts-file redirect plus TLS
termination in the mock, which would put a handshake on one side of the comparison and not the other.

So **multi-transport selection has no end-to-end measurement.** It is covered by unit tests in
`crates/providers/src/format.rs` and `crates/execute/src/credentials.rs`, including one that
enumerates the registry so a new multi-transport entry is exercised the day it lands, but no figure
here shows the hop being skipped against a live provider. Stated rather than quietly dropped, because
the README claims the feature and a reader deserves to know which claims have numbers behind them.

### Protocol

- Warm both routers with ≥200 discarded requests. Cold-start noise otherwise swamps S1.
- Each cell: ≥30s or ≥2000 requests, whichever is longer.
- Report **p50, p95, p99, max**. Not the mean — users feel the tail.
- ≥5 independent trials per cell; report the median of per-run p50s **and the spread**.
- Concurrency 1 and 8.
- Use `oha`/`k6`/`wrk` — a hand-rolled client becomes the bottleneck and measures itself.

### What the committed runs actually used

**20s per leg and 3 trials, not 30s and 5.** The protocol above is the standard; this is the
deviation, stated rather than left for a reader to infer from the file headers.

Each streaming cell is now measured twice over (with and without keep-alive), so a full pass is
18 timed legs per concurrency level. At 30s × 5 trials that is about three hours per router, and
fairness rule 6 means nothing else may run on the machine for the duration — six hours of exclusive
time for two routers.

At 20s and concurrency 1 the streaming cells land near 500 requests, short of the nominal 2000-request
floor. That floor exists to keep a median from being noise; the observed per-trial spread was ±0.5 ms
on the non-streaming cells, so its purpose is met at this length. The c=1 streaming cells are the
weakest rows in the table for this reason, and the `[min..max]` column is there to show it rather than
hide it. Anyone re-running with more budget should use the documented 30s/5.

### The harness

`benches/run.sh` implements the above. It measures whichever router is listening on `--router-port`
and does not start one, nor know which it is talking to — deliberately, so the same script drives both
sides and the comparison is between two routers rather than two harnesses.

```bash
cargo build -p mock-provider --release
benches/run.sh --label nullrouter --router-port 20128 \
  --model-openai bench-openai --model-claude bench-claude --api-key sk-...
benches/run.sh --label 9router --router-port 3000 \
  --model-openai bench-openai --model-claude bench-claude --api-key sk-...
```

Each cell runs both legs in the same run against the same mock and reports
`overhead = p50(through) − p50(direct)` as the median of per-trial differences, with `[min..max]`
beside it. Model names are flags because they resolve differently in each router's config. Results land
in `benches/results/<timestamp>-<label>.txt`, environment header first, and every run records `node`,
`rustc`, CPU count, and memory before it measures anything.

It refuses to run without `oha` on PATH rather than falling back to a curl loop. Two sanity checks are
written into the output rather than left to the reader: `direct` must land near `--mock-sleep-ms`, and
an overhead that is a round fraction of it means the harness is timing the mock. Failed cells print
`FAILED`/`NO RESULT` and are kept in the file.

## Resource cost

At S5, concurrency 8:

- **Steady-state RSS** after warmup, and **peak RSS** sampled ≥10×/sec from `/proc/<pid>/status`.
- **CPU-seconds per 1000 requests** from `/proc/<pid>/stat` — an honest efficiency number, unlike
  instantaneous CPU%.
- **Sum across the whole process tree on both sides.** nullrouter runs 8 services; 9Router runs 1.
  This is where an unfair benchmark is easiest to build by accident. If nullrouter's total is higher,
  publish that: microservice isolation costs memory, and the README must not imply otherwise.
- **Install size**: nullrouter's stripped release binaries vs 9Router's `node_modules` + source.
  Note that nullrouter *also* needs Node whenever pxpipe is enabled — "no Node required" is false for
  that configuration and must not be claimed unqualified.

## Configuring 9Router to its own advantage

A comparison that handicaps the baseline proves nothing:

- Pin the SHA: v0.5.55, `699edac3273e13d4744bc46f6082618f08560702`.
- **`next build && next start`, never `next dev`.** Dev mode is several times slower; using it would
  manufacture the result.
- `NODE_ENV=production`, debug logging off.
- Node LTS ≥20.19, version recorded.
- Same machine, same session, same mock, one API-key connection, every token saver off on both sides.

**If 9Router cannot be made to run in the sandbox, that is a blocker to report — not a licence to
publish one-sided numbers.**

## Fairness rules

1. Same mock, same fixed service time, both directions.
2. Both warmed identically before measurement.
3. Production builds on both sides.
4. No feature enabled on one side and not the other. `benches/serve.sh` therefore sets
   `NULLROUTER_REQUIRE_API_KEY`: 9Router validates a Bearer key on `/v1` and nullrouter does not by
   default, so leaving it off hands nullrouter a free pass on work the baseline is doing.
5. **One router up at a time.** Two idle routers still hold thread pools and run background timers,
   and on a shared CPU that lands in the other one's tail. `run.sh` refuses to start if the other
   port is listening.
6. **Nothing else running on the machine.** No `cargo build`, no test run, no second benchmark. A
   16-core sandbox absorbs a compile without obviously stalling, and the cost shows up as tail
   latency in whichever cell was unlucky — indistinguishable from a real regression.
7. Both configured from the *same* source. `benches/configure.sh` imports 9Router's own config via
   `/api/migrate/9router` rather than hand-building an equivalent, because hand-writing "the same"
   two providers twice is how a comparison quietly stops being one.
8. Publish the full command lines and the mock's source.
9. **Publish losing cells.** A table with one embarrassing row is credible; one with rows missing is
   not.

## Definition of done

- `cargo bench` runs clean and reproducibly. ✔
- Every unquantified speed or weight claim in the README is replaced by a measured number, or by a
  plain statement that no comparison was made. ✔
- Both raw runs are committed, not just the summary. ✔ — and every intermediate run too, so the
  regressions and the flat changes are visible alongside the wins.

### What "no room for further optimisation" can and cannot mean here

It cannot be verified. Proving no faster implementation exists is a claim about everything not yet
tried, and no run establishes it. What *can* be stated, and what this document tracks:

- Every cell has been measured against 9Router, and the worst is 6.95× on overhead.
- Every piece of the remaining overhead is either attributed to a component with a number
  (`benches/hop-probe`) or explicitly recorded as unattributed, with the reason — no profiler on this
  box.
- Every change that was tried is recorded with its measured effect, including the one that turned out
  to be flat on the cell it was aimed at (frame coalescing on S5) and the diagnosis that was wrong
  (round trips rather than the snapshot clone behind them).

The next attempt should start from the two unattributed figures above — ~0.15 ms in the pipeline's
control flow and ~0.27 ms in Pingora's proxying — which together are a little over half of what is
left at c=1 non-streaming, and neither of which can be narrowed further on a box without a profiler.

And it should measure the hops against a zero-latency mock. The first version of that table did not,
and reported a runtime hop of 0.772 ms where the real figure is 0.425 ms: subtracting two numbers
that each contain the same 25 ms sleep leaves scheduling noise in the remainder. That error made
the actix cycle look like 0.575 ms of the cost when it is 0.073 ms.

## Environment notes

This sandbox has 32 GB of disk and `target/` reaches ~25 GB. It has filled twice, and a full disk
presents as *silently empty command output* — not as an error. Check `df -h /` before concluding a
tool is broken, and `rm -rf target` to recover.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only cargo bench
```

Use a `bench` profile inheriting `release` with `debug = "line-tables-only"`, so a profiler can
symbolise without carrying full debug info.

**One pitfall worth stating plainly:** a benchmark is not a test. If a bench body is rejected by the
translator, fix the body — do not add an assertion. A bench that silently measures an error path is
worse than no bench.
