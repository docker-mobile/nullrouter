//! nullrouter-sse — SSE pipeline (request/response translation, streaming,
//! provider executors, rtk, handlers, services, transformer).
//!
//! Ports `inspire/open-sse/` (translator, executors, rtk, handlers, services,
//! transformer, shared). The translator submodule is the bug-for-bug parity
//! gate against `inspire/tests/translator/` fixtures.

#![forbid(unsafe_code)]
