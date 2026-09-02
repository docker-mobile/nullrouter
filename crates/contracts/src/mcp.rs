//! Which local MCP plugins this router will bridge, shared by the two services that must agree.
//!
//! Two services hold half of this each. The events service spawns the plugin's process and serves
//! it over SSE at `/api/mcp/{name}/sse`; the API service writes a client config pointing at that
//! URL. If their lists disagree, an apply writes a config naming a bridge that will never come up —
//! the client shows a server that fails to connect, with nothing saying why.
//!
//! So the identity and the tool list live here, and **the command does not**. That asymmetry is
//! deliberate: only the events service should be able to turn a name into a process, and a config
//! writer has no reason to know how. The events service keeps its own table with the command in it
//! and is held to these names by a test.

/// One plugin that can be bridged, minus anything about how it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeablePlugin {
    /// The `{name}` in `/api/mcp/{name}/sse`, and the key a client config lists it under.
    pub name: &'static str,
    /// For a UI listing what is available.
    pub title: &'static str,
    pub description: &'static str,
    /// The tools it exposes, used to build an allow-policy so a client does not prompt per call.
    pub tool_names: &'static [&'static str],
    /// Something outside this router that has to be true for the plugin to work, or `None`.
    ///
    /// Reported rather than assumed: Browser MCP drives a Chrome that must already be running with
    /// its extension installed, and without that the bridge comes up and every call times out.
    pub external_requirement: Option<&'static str>,
}

/// Every bridgeable plugin.
///
/// One entry. The list is short because each addition is a process this router will spawn on
/// request, so it is a deliberate decision rather than a configuration option.
pub const BRIDGEABLE_PLUGINS: &[BridgeablePlugin] = &[BridgeablePlugin {
    name: "browsermcp",
    title: "Browser MCP",
    description: "Control your running Chrome (requires Chrome extension)",
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
    external_requirement: Some(
        "a running Chrome with the Browser MCP extension installed and connected",
    ),
}];

/// Look a plugin up by name.
///
/// Exact match against this table, never a substring or a path join: the name reaches a URL and,
/// on the events side, a process table.
#[must_use]
pub fn bridgeable_plugin(name: &str) -> Option<&'static BridgeablePlugin> {
    BRIDGEABLE_PLUGINS.iter().find(|plugin| plugin.name == name)
}

#[cfg(test)]
mod tests {
    use super::{BRIDGEABLE_PLUGINS, bridgeable_plugin};

    #[test]
    fn names_are_unique_and_url_safe() {
        // The name is interpolated into `/api/mcp/{name}/sse` and matched against a spawn table, so
        // a separator or a duplicate would be a routing bug at best.
        let mut names: Vec<&str> = BRIDGEABLE_PLUGINS
            .iter()
            .map(|plugin| plugin.name)
            .collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate plugin name");
        for name in names {
            assert!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-'),
                "{name:?} is not safe in a URL path segment"
            );
        }
    }

    #[test]
    fn lookup_is_exact() {
        assert!(bridgeable_plugin("browsermcp").is_some());
        for name in [
            "",
            "browser",
            "browsermcp/../x",
            "BrowserMCP",
            "browsermcp ",
        ] {
            assert!(
                bridgeable_plugin(name).is_none(),
                "{name:?} should not resolve"
            );
        }
    }

    #[test]
    fn every_plugin_lists_the_tools_a_policy_is_built_from() {
        // An empty list would mean a client config with no allow-policy, so every call prompts.
        for plugin in BRIDGEABLE_PLUGINS {
            assert!(
                !plugin.tool_names.is_empty(),
                "{} lists no tools",
                plugin.name
            );
        }
    }
}
