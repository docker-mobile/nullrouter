//! Which `cloudflared` and `tailscale` operations may run, and nothing else.
//!
//! This table is the security boundary. `nullrouter-procctl` will run whatever argv it is
//! handed, so the question "what can the panel make these binaries do" is answered here and
//! only here. It is a `const` table of function pointers: there is no path by which a
//! request body becomes a subcommand, and no runtime registration, for the same reason
//! `events-actix`'s MCP plugin list has none.
//!
//! It is also the extension point. Both binaries do much more than tunnels —
//! `cloudflared access`, `tailscale cert`, `tailscale ip`, `tailscale serve` — and adding one
//! is a row here plus its argv builder. What a new row cannot do is widen the surface:
//! every value still goes through [`nullrouter_procctl::argv`], every one-shot still gets a
//! deadline, and a row that needs a credential still receives it through the child's
//! environment.

use std::time::Duration;

use nullrouter_procctl::argv::{ArgError, Argv};

use super::{cloudflared, tailscale};

/// Which binary an operation drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// `cloudflared`.
    Cloudflared,
    /// The `tailscale` CLI, which talks to `tailscaled` over a socket.
    Tailscale,
}

impl Tool {
    /// The name used in paths and payloads.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Cloudflared => "cloudflared",
            Self::Tailscale => "tailscale",
        }
    }
}

/// Whether an operation only reads, or changes something.
///
/// Mutations are gated to loopback callers by the gateway; this field is what a status
/// payload uses to tell a panel which rows are safe to poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Reads state. Safe to call repeatedly.
    Read,
    /// Changes state on this machine or in the account.
    Mutate,
}

impl Effect {
    /// The name used in payloads.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mutate => "mutate",
        }
    }
}

/// Builds the environment entries for one operation.
///
/// Named because it is the only channel a credential has to a child, and a name makes that
/// searchable.
pub(crate) type EnvBuilder = fn(&Args) -> Vec<(String, String)>;

/// A parameter an operation accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Param {
    /// Name in the request body.
    pub(crate) name: &'static str,
    /// What it is, for the panel.
    pub(crate) about: &'static str,
    /// Whether the operation cannot run without it.
    pub(crate) required: bool,
    /// Whether it is a credential: withheld from logs and from every response.
    pub(crate) secret: bool,
}

impl Param {
    /// A required plain value.
    const fn required(name: &'static str, about: &'static str) -> Self {
        Self {
            name,
            about,
            required: true,
            secret: false,
        }
    }

    /// An optional plain value.
    const fn optional(name: &'static str, about: &'static str) -> Self {
        Self {
            name,
            about,
            required: false,
            secret: false,
        }
    }

    /// A required credential.
    const fn credential(name: &'static str, about: &'static str) -> Self {
        Self {
            name,
            about,
            required: true,
            secret: true,
        }
    }
}

/// The values a caller supplied, already checked for presence.
///
/// Values are still unvalidated text here; they become arguments only through
/// [`nullrouter_procctl::argv`], which is what applies the charset.
#[derive(Debug, Default, Clone)]
pub(crate) struct Args {
    entries: Vec<(String, String)>,
}

impl Args {
    /// Build from name/value pairs.
    pub(crate) const fn from_pairs(entries: Vec<(String, String)>) -> Self {
        Self { entries }
    }

    /// One value, if present and non-empty.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value.as_str())
            .filter(|value| !value.trim().is_empty())
    }

    /// A required value.
    pub(crate) fn require(&self, name: &'static str) -> Result<&str, ArgError> {
        self.get(name).ok_or(ArgError::Empty { field: name })
    }

    /// A required port.
    pub(crate) fn require_port(&self, name: &'static str) -> Result<u16, ArgError> {
        self.require(name)?
            .parse()
            .map_err(|_bad| ArgError::BadCharacter {
                field: name,
                character: '?',
                kind: "a port number",
            })
    }
}

/// How an operation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Runs to completion under a deadline, and its output is the answer.
    OneShot {
        /// Hard deadline.
        timeout: Duration,
    },
    /// Becomes the supervised long-running child for its tool.
    ///
    /// Only one exists per tool at a time, which is what makes "one tunnel, and we know its
    /// pid" true rather than aspirational.
    Supervised,
}

/// One thing the panel may ask a binary to do.
pub(crate) struct Operation {
    /// Stable id, used in the route and in payloads.
    pub(crate) id: &'static str,
    /// One line for the panel.
    pub(crate) about: &'static str,
    /// Which binary.
    pub(crate) tool: Tool,
    /// Read or mutate.
    pub(crate) effect: Effect,
    /// How it runs.
    pub(crate) mode: Mode,
    /// Accepted parameters.
    pub(crate) params: &'static [Param],
    /// Build the argument vector. The only place a value becomes an argument.
    pub(crate) build: fn(&Args) -> Result<Argv, ArgError>,
    /// Build the child's environment, for an operation that needs a credential.
    ///
    /// Separate from `build` because this is the only route a secret takes to the child, and
    /// keeping it out of `build` makes it impossible for a credential to reach argv by
    /// accident.
    pub(crate) env: Option<EnvBuilder>,
}

impl std::fmt::Debug for Operation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Operation")
            .field("id", &self.id)
            .field("tool", &self.tool)
            .field("effect", &self.effect)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

/// Timeout for a probe that talks to a local socket.
const PROBE: Mode = Mode::OneShot {
    timeout: Duration::from_secs(5),
};

/// Timeout for an operation that talks to a remote API.
const REMOTE: Mode = Mode::OneShot {
    timeout: Duration::from_secs(30),
};

/// Every operation the panel may run.
///
/// Ordered by tool, then by how destructive it is, so the table reads as a permission list.
pub(crate) const OPERATIONS: &[Operation] = &[
    // ── cloudflared ──────────────────────────────────────────────────────────────────
    Operation {
        id: "cloudflared.version",
        about: "Report the installed cloudflared version",
        tool: Tool::Cloudflared,
        effect: Effect::Read,
        mode: PROBE,
        params: &[],
        build: |_args| Ok(Argv::new().flag("--version")),
        env: None,
    },
    Operation {
        id: "cloudflared.tunnel.list",
        about: "List the named tunnels this credential can see",
        tool: Tool::Cloudflared,
        effect: Effect::Read,
        mode: REMOTE,
        params: &[Param::credential(
            "token",
            "Cloudflare tunnel token, sent through the child's environment",
        )],
        build: |_args| {
            Ok(Argv::new()
                .word("tunnel")
                .flag("--output")
                .word("json")
                .word("list"))
        },
        env: Some(cloudflared::token_env),
    },
    Operation {
        id: "cloudflared.tunnel.quick",
        about: "Open a quick tunnel to the gateway and return its trycloudflare.com URL",
        tool: Tool::Cloudflared,
        effect: Effect::Mutate,
        mode: Mode::Supervised,
        params: &[Param::optional(
            "port",
            "Loopback port to expose; defaults to the gateway",
        )],
        build: cloudflared::quick_tunnel_argv,
        env: None,
    },
    Operation {
        id: "cloudflared.tunnel.run",
        about: "Run a named, remotely-managed tunnel",
        tool: Tool::Cloudflared,
        effect: Effect::Mutate,
        mode: Mode::Supervised,
        params: &[Param::credential(
            "token",
            "Cloudflare tunnel token, sent through the child's environment",
        )],
        build: cloudflared::named_tunnel_argv,
        env: Some(cloudflared::token_env),
    },
    // ── tailscale ────────────────────────────────────────────────────────────────────
    Operation {
        id: "tailscale.version",
        about: "Report the installed tailscale version",
        tool: Tool::Tailscale,
        effect: Effect::Read,
        mode: PROBE,
        params: &[],
        build: |_args| Ok(Argv::new().flag("--version")),
        env: None,
    },
    Operation {
        id: "tailscale.status",
        about: "Report tailscaled's backend state and this device's identity",
        tool: Tool::Tailscale,
        effect: Effect::Read,
        mode: PROBE,
        params: &[],
        build: |_args| Ok(tailscale::socket_argv()?.word("status").flag("--json")),
        env: None,
    },
    Operation {
        id: "tailscale.ip",
        about: "Report this device's tailnet addresses",
        tool: Tool::Tailscale,
        effect: Effect::Read,
        mode: PROBE,
        params: &[],
        build: |_args| Ok(tailscale::socket_argv()?.word("ip")),
        env: None,
    },
    Operation {
        id: "tailscale.funnel.status",
        about: "Report which ports Funnel is currently serving",
        tool: Tool::Tailscale,
        effect: Effect::Read,
        mode: PROBE,
        params: &[],
        build: |_args| {
            Ok(tailscale::socket_argv()?
                .word("funnel")
                .word("status")
                .flag("--json"))
        },
        env: None,
    },
    Operation {
        id: "tailscale.serve.status",
        about: "Report the tailnet-only Serve configuration",
        tool: Tool::Tailscale,
        effect: Effect::Read,
        mode: PROBE,
        params: &[],
        build: |_args| {
            Ok(tailscale::socket_argv()?
                .word("serve")
                .word("status")
                .flag("--json"))
        },
        env: None,
    },
    Operation {
        id: "tailscale.cert",
        about: "Provision a TLS certificate for this device's tailnet hostname",
        tool: Tool::Tailscale,
        effect: Effect::Mutate,
        mode: REMOTE,
        params: &[Param::required("hostname", "The device's full tailnet DNS name")],
        build: tailscale::cert_argv,
        env: None,
    },
    Operation {
        id: "tailscale.funnel.start",
        about: "Expose a loopback port to the public internet over Funnel",
        tool: Tool::Tailscale,
        effect: Effect::Mutate,
        mode: REMOTE,
        params: &[Param::optional(
            "port",
            "Loopback port to expose; defaults to the gateway",
        )],
        build: tailscale::funnel_start_argv,
        env: None,
    },
    Operation {
        id: "tailscale.funnel.reset",
        about: "Withdraw every Funnel mapping",
        tool: Tool::Tailscale,
        effect: Effect::Mutate,
        mode: REMOTE,
        params: &[],
        build: |_args| {
            Ok(tailscale::socket_argv()?
                .word("funnel")
                .flag("--bg")
                .word("reset"))
        },
        env: None,
    },
    Operation {
        id: "tailscale.up",
        about: "Log this device into a tailnet, returning the browser URL to finish it",
        tool: Tool::Tailscale,
        effect: Effect::Mutate,
        mode: REMOTE,
        params: &[Param::optional(
            "hostname",
            "Device name to claim in the tailnet",
        )],
        build: tailscale::up_argv,
        env: None,
    },
    Operation {
        id: "tailscale.logout",
        about: "Log this device out of its tailnet",
        tool: Tool::Tailscale,
        effect: Effect::Mutate,
        mode: REMOTE,
        params: &[],
        build: |_args| Ok(tailscale::socket_argv()?.word("logout")),
        env: None,
    },
];

/// Look one operation up by id.
pub(crate) fn operation(id: &str) -> Option<&'static Operation> {
    OPERATIONS.iter().find(|operation| operation.id == id)
}

#[cfg(test)]
mod tests {
    use super::{Args, Mode, OPERATIONS, Tool, operation};

    #[test]
    fn every_id_is_unique_and_namespaced_by_its_tool() {
        let mut ids: Vec<&str> = OPERATIONS.iter().map(|entry| entry.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate operation id");

        for entry in OPERATIONS {
            assert!(
                entry.id.starts_with(entry.tool.id()),
                "{} is not namespaced by {}",
                entry.id,
                entry.tool.id()
            );
        }
    }

    #[test]
    fn no_operation_can_be_invented_by_a_caller() {
        // The table is the whole surface. These are the shapes a request body would try.
        for hostile in [
            "cloudflared.tunnel.run; rm -rf /",
            "../../../../bin/sh",
            "tailscale.status\nteleport",
            "TAILSCALE.STATUS",
            "",
            "cloudflared.access.ssh",
        ] {
            assert!(
                operation(hostile).is_none(),
                "{hostile:?} resolved to an operation"
            );
        }
    }

    #[test]
    fn only_supervised_operations_are_long_running() {
        // A one-shot with no deadline is the upstream failure this table avoids.
        for entry in OPERATIONS {
            match entry.mode {
                Mode::OneShot { timeout } => assert!(
                    !timeout.is_zero() && timeout.as_secs() <= 120,
                    "{} has an unusable timeout {timeout:?}",
                    entry.id
                ),
                Mode::Supervised => assert!(
                    entry.id.contains("tunnel"),
                    "{} is supervised but is not a tunnel",
                    entry.id
                ),
            }
        }
    }

    #[test]
    fn a_credential_is_only_ever_declared_as_an_environment_parameter() {
        for entry in OPERATIONS {
            let secrets: Vec<&str> = entry
                .params
                .iter()
                .filter(|param| param.secret)
                .map(|param| param.name)
                .collect();
            if secrets.is_empty() {
                continue;
            }
            // An operation with a credential must have somewhere to put it that is not
            // argv. Without this, a future row could quietly reintroduce `--token`.
            assert!(
                entry.env.is_some(),
                "{} declares {secrets:?} but has no env builder, so the value could \
                 only reach the child through argv",
                entry.id
            );
        }
    }

    #[test]
    fn no_secret_parameter_reaches_the_argument_vector() {
        // The check that would have caught upstream's `--token <value>`.
        let args = Args::from_pairs(vec![
            ("token".to_owned(), "SECRET-TOKEN-VALUE".to_owned()),
            ("hostname".to_owned(), "device-name".to_owned()),
            ("port".to_owned(), "20128".to_owned()),
        ]);

        for entry in OPERATIONS {
            let Ok(built) = (entry.build)(&args) else {
                continue;
            };
            let rendered = built.as_slice().join(" ");
            assert!(
                !rendered.contains("SECRET-TOKEN-VALUE"),
                "{} put a credential in argv: {rendered}",
                entry.id
            );
        }
    }

    #[test]
    fn an_operation_that_takes_a_credential_puts_it_in_the_environment() {
        let args = Args::from_pairs(vec![("token".to_owned(), "SECRET-TOKEN-VALUE".to_owned())]);

        let named = operation("cloudflared.tunnel.run").expect("the named tunnel row exists");
        let env = (named.env.expect("it declares an env builder"))(&args);

        assert!(
            env.iter()
                .any(|(key, value)| key == "TUNNEL_TOKEN" && value == "SECRET-TOKEN-VALUE"),
            "{env:?}"
        );
    }

    #[test]
    fn every_read_operation_builds_without_any_arguments() {
        // A status poll must not need a body, or the panel cannot poll it.
        let empty = Args::default();
        for entry in OPERATIONS {
            if entry.params.iter().any(|param| param.required) {
                continue;
            }
            assert!(
                (entry.build)(&empty).is_ok(),
                "{} cannot build with no arguments",
                entry.id
            );
        }
    }

    #[test]
    fn a_missing_required_parameter_is_refused_rather_than_defaulted() {
        let empty = Args::default();
        for entry in OPERATIONS {
            if !entry.params.iter().any(|param| param.required && !param.secret) {
                continue;
            }
            assert!(
                (entry.build)(&empty).is_err(),
                "{} built without its required parameter",
                entry.id
            );
        }
    }

    #[test]
    fn a_hostile_parameter_value_is_refused_by_every_builder() {
        for hostile in ["$(id)", "a;b", "--flag", "a b`c`", "a\nb"] {
            let args = Args::from_pairs(vec![
                ("hostname".to_owned(), hostile.to_owned()),
                ("port".to_owned(), hostile.to_owned()),
            ]);
            for entry in OPERATIONS {
                if !entry.params.iter().any(|param| !param.secret) {
                    continue;
                }
                if let Ok(built) = (entry.build)(&args) {
                    let rendered = built.as_slice().join(" ");
                    assert!(
                        !rendered.contains(hostile),
                        "{} passed {hostile:?} through to argv: {rendered}",
                        entry.id
                    );
                }
            }
        }
    }

    #[test]
    fn both_tools_are_represented() {
        for tool in [Tool::Cloudflared, Tool::Tailscale] {
            assert!(
                OPERATIONS.iter().any(|entry| entry.tool == tool),
                "no operations for {}",
                tool.id()
            );
        }
    }
}
