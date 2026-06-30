//! nullrouter-mitm — MITM HTTPS proxy.
//!
//! Ports `inspire/src/mitm/` (17 files: CA cert handling, on-the-fly leaf
//! cert minting via `rcgen`, connection lifecycle, request capture).
//! Uses Pingora for the proxy core.

#![forbid(unsafe_code)]
