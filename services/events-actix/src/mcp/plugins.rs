//! Which MCP servers may be spawned, and nothing else.
//!
//! Upstream's `LOCAL_STDIO_PLUGINS` carries the same rule with the same reason recorded in its
//! source: *"Only preset stdio plugins may spawn. No user-defined commands (RCE prevention)."*
//! A plugin name arrives in a URL path, so a lookup that fell through to "run whatever was asked
//! for" would turn `GET /api/mcp/{plugin}/sse` into remote command execution.
//!
//! The list is deliberately a compile-time constant rather than configuration. A configurable
//! command list is the same hole with an extra step: anything that can write the config can run
//! arbitrary commands as this service.

/// One spawnable MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plugin {
    /// The `{plugin}` path segment, matched exactly.
    pub(crate) name: &'static str,
    /// For messages a user reads.
    pub(crate) title: &'static str,
    /// Executable, resolved on `PATH` at spawn time.
    pub(crate) command: &'static str,
    pub(crate) args: &'static [&'static str],
    /// What the server is expected to expose, used to build the tool policy a client writes into
    /// its own config. Not enforced here: the server's own `tools/list` is authoritative.
    pub(crate) tool_names: &'static [&'static str],
    /// Something outside this process that must also be present for the server to do anything.
    /// `None` means the spawn alone is sufficient.
    pub(crate) external_requirement: Option<&'static str>,
}

/// Upstream's `LOCAL_STDIO_PLUGINS`, ported entry for entry.
///
/// The names and tool lists here are also in `nullrouter_contracts::BRIDGEABLE_PLUGINS`, which the
/// API service reads when it writes a client config pointing at `/api/mcp/{name}/sse`. The commands
/// are deliberately *not* shared: only this service should be able to turn a name into a process.
/// A test below holds the two tables to the same names, because a disagreement produces a config
/// naming a bridge that never comes up.
pub(crate) const PLUGINS: &[Plugin] = &[Plugin {
    name: "browsermcp",
    title: "Browser MCP",
    command: "npx",
    args: &["-y", "@browsermcp/mcp@latest"],
    tool_names: &[
        "browser_navigate",
        "browser_snapshot",
        "browser_click",
        "browser_type",
        "browser_screenshot",
        "browser_get_console_logs",
        "browser_wait",
        "browser_press_key",
        "browser_go_back",
        "browser_go_forward",
    ],
    // Upstream's own description: "Control your running Chrome (requires Chrome extension)".
    // Reported to the client rather than discovered by failing: the server starts happily and
    // then every tool call fails, which reads as a router bug rather than a missing extension.
    external_requirement: Some(
        "a running Chrome with the Browser MCP extension installed; the server starts without it \
         and then every tool call fails",
    ),
}];

/// Look up a plugin by its path segment.
///
/// Returns `None` for anything not on the list, which is what keeps an arbitrary path segment from
/// reaching a spawn.
pub(crate) fn find(name: &str) -> Option<&'static Plugin> {
    PLUGINS.iter().find(|plugin| plugin.name == name)
}

/// Build a plugin the tests may spawn, without putting it on the production whitelist.
///
/// `#[cfg(test)]` is the whole point: the bridge takes `&'static Plugin`, so a test needs one that
/// outlives it, and the only alternatives are widening [`PLUGINS`] — which would make a test
/// fixture spawnable in production — or making the list runtime-mutable, which is the RCE hole
/// [`find`] exists to close.
///
/// The leak is deliberate and bounded: one `Plugin` per call, in a test binary that then exits.
#[cfg(test)]
pub(crate) fn leak_for_test(
    name: &'static str,
    command: &'static str,
    args: &'static [&'static str],
) -> &'static Plugin {
    Box::leak(Box::new(Plugin {
        name,
        title: "test fixture",
        command,
        args,
        tool_names: &[],
        external_requirement: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::{PLUGINS, find};

    #[test]
    fn only_listed_names_resolve() {
        assert!(find("browsermcp").is_some());
        for hostile in [
            "",
            "sh",
            "npx",
            "browsermcp ",
            " browsermcp",
            "BrowserMcp",
            "../../bin/sh",
            "browsermcp;id",
            "browsermcp/../sh",
        ] {
            assert!(
                find(hostile).is_none(),
                "{hostile:?} must not resolve to a spawnable plugin"
            );
        }
    }

    #[test]
    fn no_plugin_command_takes_a_shell() {
        // A shell as the command would make args a script rather than an argv, so quoting bugs
        // anywhere upstream of here would become command injection.
        for plugin in PLUGINS {
            assert!(
                !matches!(plugin.command, "sh" | "bash" | "zsh" | "cmd" | "powershell"),
                "{} spawns a shell",
                plugin.name
            );
            assert!(
                !plugin.name.is_empty() && plugin.name.chars().all(|c| c.is_ascii_alphanumeric()),
                "{} has a name that is not a plain path segment",
                plugin.name
            );
        }
    }

    #[test]
    fn the_spawn_table_matches_the_shared_bridgeable_list() {
        // The API service writes client configs naming `/api/mcp/{name}/sse` from
        // `BRIDGEABLE_PLUGINS`. If a name is there and not here, that config points at a bridge
        // this service will refuse to spawn: the client shows a server that fails to connect and
        // nothing says why. If a name is here and not there, the plugin is unreachable through the
        // dashboard. Either way the failure is silent, so it is pinned here.
        let mut spawnable: Vec<&str> = super::PLUGINS.iter().map(|plugin| plugin.name).collect();
        let mut bridgeable: Vec<&str> = nullrouter_contracts::BRIDGEABLE_PLUGINS
            .iter()
            .map(|plugin| plugin.name)
            .collect();
        spawnable.sort_unstable();
        bridgeable.sort_unstable();
        assert_eq!(spawnable, bridgeable);

        // And the tool lists agree, because the allow-policy a client writes is built from the
        // shared copy while the server that answers is spawned from this one.
        for plugin in super::PLUGINS {
            let shared = nullrouter_contracts::bridgeable_plugin(plugin.name)
                .expect("just asserted the names match");
            assert_eq!(
                plugin.tool_names, shared.tool_names,
                "{} exposes a different tool list than the one a client is told to allow",
                plugin.name
            );
        }
    }
}
