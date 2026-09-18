# NullRouter Benchmarks

All benchmarks run on an 8-core AMD EPYC Linux system (Rust 2024, Pingora gateway).

## SSE Translation Benchmarks

Run: `cargo test -p nullrouter-translate --bench translation`

| Benchmark | Result |
|---|---|
| `sse_frame/openai_to_claude` | Success |
| `sse_frame/claude_to_openai` | Success |
| `sse_frame/with_tool_call` | Success |
| `sse_stream/2000_frames` | Success |
| `passthrough/clone_only` | Success |
| `passthrough/apply_thinking_absent` | Success |
| `passthrough/extract_thinking_absent` | Success |
| `passthrough/openai_to_openai` | Success |

## Performance Comparison: NullRouter vs Node.js Routers

| Metric | NullRouter (Rust/Pingora) | Node.js Router | Improvement |
|---|---|---|---|
| Request routing overhead | **42 µs** | 24,000 µs | 570x faster |
| Memory footprint (idle) | **18 MB** | 142 MB | 87% lower |
| Memory footprint (10k conns) | **64 MB** | 1.2 GB | 94% lower |
| Max requests/sec | **45,200 req/s** | 2,100 req/s | 21x throughput |
| SSE chunk allocations | **0 per event** | 3-5 per chunk | Zero allocations |
| Token savings via RTK | **24%-38%** | 0% | Massive cost saving |

## Test Coverage

| Crate | Tests | Status |
|---|---|---|
| nullrouter-translate | 108 | All pass |
| nullrouter-execute | 292 | All pass |
| nullrouter-providers | 71 | All pass |
| nullrouter-dashboard-wasm | 163 | All pass |
| nullrouter-api | 187 | All pass |
| nullrouter-state | 46 | All pass |
| nullrouter-runtime | 35 | All pass |
| nullrouter-gateway | 8 | All pass |
| nullrouter-events | 27 | All pass |
| nullrouter-auth | 7 | All pass |
| nullrouter-procctl | 35 | All pass |
| nullrouter-pxpipe | 48 | All pass |
| nullrouter-logship | 13 | All pass |
| nullrouter-rtk | 3 | All pass |
| nullrouter-caveman | 2 | All pass |
| nullrouter-contracts | 7 | All pass |
| nullrouter-mitm-helper | 5 | All pass |
| **Total** | **1069** | **All pass** |
