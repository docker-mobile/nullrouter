//! Tool deduplication: strip built-in/duplicate tool definitions when
//! equivalent MCP tools are present, reducing token bloat for Claude clients.
//!
//! Ports `open-sse/utils/toolDeduper.js`.

use serde_json::Value;

/// One deduplication rule: if any trigger tool is present, strip the
/// listed tools.
struct DedupRule {
    /// Tool names that, when present, trigger the strip.
    triggers: &'static [&'static str],
    /// Tool names to strip when triggered.
    strip: &'static [&'static str],
}

const RULES: &[DedupRule] = &[
    // Exa MCP present → drop built-in web tools.
    DedupRule {
        triggers: &["mcp__exa__web_search_exa", "mcp__exa__web_fetch_exa"],
        strip: &["WebSearch", "WebFetch", "mcp__workspace__web_fetch"],
    },
    // Tavily MCP present → drop built-in web tools.
    DedupRule {
        triggers: &["mcp__tavily__tavily_search", "mcp__tavily__tavily_extract"],
        strip: &["WebSearch", "WebFetch", "mcp__workspace__web_fetch"],
    },
];

fn tool_name(tool: &Value) -> &str {
    tool.get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .or_else(|| tool.get("name").and_then(Value::as_str))
        .unwrap_or("")
}

/// Deduplicate tools: strip built-in tools when equivalent MCP tools are present.
///
/// Returns the filtered tool list and the names of stripped tools.
/// Call this on the `tools` array before sending a request to the provider.
#[must_use]
pub fn dedupe_tools(tools: &[Value]) -> (Vec<Value>, Vec<String>) {
    if tools.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let names: Vec<&str> = tools.iter().map(tool_name).collect();
    let mut to_strip: Vec<&str> = Vec::new();

    for rule in RULES {
        let has_trigger = names.iter().any(|n| rule.triggers.contains(n));
        if !has_trigger {
            continue;
        }
        for name in &names {
            if rule.strip.contains(name) {
                to_strip.push(name);
            }
        }
    }

    if to_strip.is_empty() {
        return (tools.to_vec(), Vec::new());
    }

    let stripped: Vec<String> = to_strip
        .iter()
        .map(|s| (*s).to_owned())
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[0] != w[1])
        .map(|w| w[0].clone())
        .chain(to_strip.last().map(std::string::ToString::to_string))
        .collect();

    let filtered: Vec<Value> = tools
        .iter()
        .filter(|t| !to_strip.contains(&tool_name(t)))
        .cloned()
        .collect();

    (filtered, stripped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_websearch_when_exa_present() {
        let tools = json!([
            {"type": "function", "function": {"name": "WebSearch", "parameters": {}}},
            {"type": "function", "function": {"name": "mcp__exa__web_search_exa", "parameters": {}}}
        ]);
        let arr = tools.as_array().unwrap();
        let (filtered, stripped) = dedupe_tools(arr);
        assert_eq!(filtered.len(), 1);
        assert_eq!(stripped, vec!["WebSearch"]);
    }

    #[test]
    fn no_strip_when_no_trigger() {
        let tools = json!([
            {"type": "function", "function": {"name": "WebSearch", "parameters": {}}}
        ]);
        let arr = tools.as_array().unwrap();
        let (filtered, stripped) = dedupe_tools(arr);
        assert_eq!(filtered.len(), 1);
        assert!(stripped.is_empty());
    }

    #[test]
    fn empty_tools_returns_empty() {
        let (filtered, stripped) = dedupe_tools(&[]);
        assert!(filtered.is_empty());
        assert!(stripped.is_empty());
    }
}
