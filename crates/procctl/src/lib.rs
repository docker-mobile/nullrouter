//! Running external binaries under tight, narrow control.
//!
//! This crate exists to manage `cloudflared` and `tailscaled` — two programs the router has
//! to drive but does not own — without inheriting the way 9Router drives them. Upstream's
//! approach, in `src/lib/tunnel/`, is a set of habits this crate refuses one by one:
//!
//! | upstream | here |
//! |---|---|
//! | downloads `cloudflared` from `releases/latest`, `chmod 755`, runs it | never downloads; runs only an installed binary, and checks its ownership and permissions first ([`binary`]) |
//! | passes the tunnel token as `--token <value>` in argv, visible in `/proc/*/cmdline` | credentials reach the child only through its environment, and cannot be printed ([`secret`]) |
//! | builds commands as shell strings interpolating values | argv arrays assembled from allowlisted characters, no shell ([`argv`]) |
//! | `pkill -f "cloudflared.*:20128"`, `pkill -9 -f tailscaled` | signals exactly the pid this crate spawned, nothing else ([`signal`]) |
//! | `SIGKILL` first | `SIGTERM`, bounded wait, then `SIGKILL` ([`signal`]) |
//! | several probes with no timeout, run with `execSync` | every one-shot has a hard deadline and bounded capture ([`oneshot`]) |
//! | `logTail = (logTail + msg).slice(-4000)` per chunk | a fixed-size ring of split lines ([`logring`]) |
//! | inherits the whole process environment | `env_clear`, then only what the operation declares |
//! | reconnects with no attempt ceiling | counted restarts with backoff, then a terminal failed state ([`supervise`]) |
//!
//! # Shape
//!
//! [`supervise::Supervisor`] owns one long-running child on its own thread. [`oneshot::Run`]
//! runs one command to completion. Neither decides *what* may run: the caller passes an
//! [`binary::Executable`] and an argv built through [`argv::Argv`], so the set of possible
//! operations is a fixed table in the calling service rather than anything this crate can
//! be talked into.
//!
//! Nothing here is specific to tunnels. `cloudflared` and `tailscale` have many other
//! subcommands, and a new one is a new entry in that caller-side table, not a change here.

pub mod argv;
pub mod binary;
pub mod logring;
pub mod oneshot;
pub mod secret;
mod signal;
pub mod supervise;

pub use signal::StopOutcome;
