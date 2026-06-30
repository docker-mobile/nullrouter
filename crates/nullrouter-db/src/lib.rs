//! nullrouter-db — SQLite storage layer.
//!
//! Ports `inspire/src/db/` (schema, migrations, queries). Uses `rusqlite`
//! bundled with the same sync semantics as upstream `better-sqlite3`.

#![forbid(unsafe_code)]
