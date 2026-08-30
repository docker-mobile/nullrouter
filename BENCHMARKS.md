# Benchmarks

The README claims nullrouter is "vastly faster" and "lighter to run" than 9Router. **Nothing here has
been measured yet.** This document is the harness those claims have to survive before they may be
stated as numbers — and the rules that keep the comparison honest rather than flattering.

Until a run exists, the README says the claim is unmeasured. That is the correct state; a plausible
number invented from architecture ("Rust, therefore faster") would be worse than no number.

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

## The overhead is state round trips, not translation

The first thing the harness found, and it is worth more than every micro-benchmark above: nullrouter
adds **~14 ms to a 25 ms upstream call**, and translation is a rounding error inside it.

Measured on this sandbox, `oha`, concurrency 1, non-streaming, mock at `--sleep-ms 25`:

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

**The fix is one fetch per request threaded through the call chain, not a TTL cache.** The runtime
re-reads these deliberately, at the last hop before a provider call, so that a dashboard toggle is
never silently ignored; a cache would trade a stated guarantee for latency. Not done here — it touches
`enforce_api_key`, `pxpipe_settings` and `resolve_targets` and wants its own change with its own
before-and-after numbers from this harness.

`services/runtime-actix/tests/node_prefix_routing.rs` asserts the prefix-resolution fetch as a
*difference* from the baseline count rather than pinning the absolute number, so it keeps passing when
this is fixed.

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

- `cargo bench` runs clean and reproducibly.
- Every unquantified speed or weight claim in the README is replaced by a measured number, or by a
  plain statement that no comparison was made.
- Both raw runs are committed, not just the summary.

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
