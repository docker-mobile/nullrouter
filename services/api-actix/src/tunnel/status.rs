//! Reading `tailscale status --json` and `funnel status --json`.
//!
//! Only four fields matter, and each is load-bearing in a way that is not obvious from the
//! name, so they are documented rather than assumed:
//!
//! * `BackendState` — `"Running"`, `"NeedsLogin"`, `"Stopped"`, `"NoState"`. This is the
//!   daemon's own summary and the only reliable "is this usable" signal;
//! * `Self.Online` — whether the *control plane* still lists this device. A device removed
//!   from the tailnet keeps a `Running` backend for a while, so upstream requires both, and
//!   so does this. Without it a removed device reports a working tunnel;
//! * `Self.DNSName` — the device's real name, which is what a Funnel URL has to be built
//!   from. It is **not** the hostname that was requested: a name collision makes Tailscale
//!   assign `name-1`, and it arrives with a trailing dot as a fully-qualified name;
//! * `AuthURL` — where a pending login has to be completed. On some platforms it appears
//!   only here and never on stdout, which is why upstream polls status during a login.

use serde::Deserialize;

/// The subset of `tailscale status --json` this service reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct TailscaleStatus {
    /// The daemon's own state summary.
    #[serde(rename = "BackendState", default)]
    pub(crate) backend_state: String,
    /// This device.
    #[serde(rename = "Self", default)]
    pub(crate) node: Option<SelfNode>,
    /// Set while a login is pending.
    #[serde(rename = "AuthURL", default)]
    pub(crate) auth_url: Option<String>,
}

/// This device, as the control plane sees it.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct SelfNode {
    /// Fully-qualified tailnet name, with a trailing dot.
    #[serde(rename = "DNSName", default)]
    pub(crate) dns_name: String,
    /// Whether the control plane still lists this device.
    #[serde(rename = "Online", default)]
    pub(crate) online: bool,
}

impl TailscaleStatus {
    /// Parse, treating anything unreadable as "no information" rather than as a negative.
    ///
    /// The distinction matters: a parse failure must not be reported as a logged-out device,
    /// because that would make the panel offer a login that is not needed.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    /// Whether the daemon is up and this device is still in the tailnet.
    pub(crate) fn is_logged_in(&self) -> bool {
        self.backend_state == "Running" && self.node.as_ref().is_some_and(|node| node.online)
    }

    /// Whether the daemon is running at all, logged in or not.
    pub(crate) fn is_daemon_up(&self) -> bool {
        !self.backend_state.is_empty() && self.backend_state != "NoState"
    }

    /// Whether a login is what is missing.
    pub(crate) fn needs_login(&self) -> bool {
        !self.is_logged_in()
    }

    /// The device's tailnet name, without the trailing dot.
    pub(crate) fn hostname(&self) -> Option<String> {
        let name = self.node.as_ref()?.dns_name.trim_end_matches('.');
        (!name.is_empty()).then(|| name.to_owned())
    }

    /// The public URL Funnel serves for this device.
    ///
    /// Built from `DNSName` rather than from the requested hostname, because a collision
    /// makes Tailscale assign a different one and a URL built from the request would 404.
    pub(crate) fn funnel_url(&self) -> Option<String> {
        self.hostname().map(|name| format!("https://{name}"))
    }
}

/// Whether `funnel status --json` shows anything being served.
///
/// The payload is a map keyed by `host:port`, so a non-empty `AllowFunnel` is the answer.
/// Absent, null and `{}` all mean "nothing", and none of them is an error.
pub(crate) fn funnel_is_serving(text: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    parsed
        .get("AllowFunnel")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|allowed| {
            allowed
                .values()
                .any(|value| value == &serde_json::Value::Bool(true))
        })
}

#[cfg(test)]
mod tests {
    use super::{TailscaleStatus, funnel_is_serving};

    /// A trimmed but real-shaped `status --json` payload.
    const LOGGED_IN: &str = r#"{
        "BackendState": "Running",
        "AuthURL": "",
        "Self": {
            "DNSName": "r4nd0m.tail1a2b3c.ts.net.",
            "Online": true,
            "HostName": "r4nd0m"
        }
    }"#;

    #[test]
    fn a_logged_in_device_is_recognised() {
        let status = TailscaleStatus::parse(LOGGED_IN).expect("parses");

        assert!(status.is_logged_in());
        assert!(status.is_daemon_up());
        assert!(!status.needs_login());
    }

    #[test]
    fn the_trailing_dot_is_stripped_from_the_hostname() {
        // Left in, every Funnel URL would be `https://host.ts.net./`, which does not resolve.
        let status = TailscaleStatus::parse(LOGGED_IN).expect("parses");

        assert_eq!(
            status.hostname().as_deref(),
            Some("r4nd0m.tail1a2b3c.ts.net")
        );
        assert_eq!(
            status.funnel_url().as_deref(),
            Some("https://r4nd0m.tail1a2b3c.ts.net")
        );
    }

    #[test]
    fn a_device_removed_from_the_tailnet_is_not_logged_in() {
        // The case the `Online` check exists for: the backend still says Running.
        let removed = r#"{
            "BackendState": "Running",
            "Self": { "DNSName": "gone.tail1a2b3c.ts.net.", "Online": false }
        }"#;

        let status = TailscaleStatus::parse(removed).expect("parses");

        assert!(!status.is_logged_in(), "a removed device must need a login");
        assert!(status.needs_login());
        assert!(status.is_daemon_up(), "the daemon itself is still up");
    }

    #[test]
    fn a_daemon_awaiting_login_reports_its_url() {
        let pending = r#"{
            "BackendState": "NeedsLogin",
            "AuthURL": "https://login.tailscale.com/a/abc123",
            "Self": { "DNSName": "", "Online": false }
        }"#;

        let status = TailscaleStatus::parse(pending).expect("parses");

        assert!(status.needs_login());
        assert!(status.is_daemon_up());
        assert_eq!(
            status.auth_url.as_deref(),
            Some("https://login.tailscale.com/a/abc123")
        );
        assert_eq!(status.hostname(), None);
    }

    #[test]
    fn a_daemon_with_no_state_is_not_up() {
        let fresh = r#"{ "BackendState": "NoState" }"#;

        let status = TailscaleStatus::parse(fresh).expect("parses");

        assert!(!status.is_daemon_up());
        assert!(!status.is_logged_in());
    }

    #[test]
    fn unreadable_output_is_no_information_rather_than_a_negative() {
        // A parse failure reported as "logged out" would make the panel offer a login that
        // is not needed, and hide a real problem behind a plausible-looking prompt.
        for bad in ["", "not json", "<html>error</html>", "[]", "null"] {
            let parsed = TailscaleStatus::parse(bad);
            assert!(
                parsed.is_none_or(|status| !status.is_daemon_up()),
                "{bad:?} parsed into something claiming a live daemon"
            );
        }
    }

    #[test]
    fn extra_fields_do_not_break_parsing() {
        // Tailscale adds fields between releases; a strict parser would break on upgrade.
        let verbose = r#"{
            "Version": "1.99.0",
            "TUN": false,
            "BackendState": "Running",
            "Self": { "DNSName": "a.b.ts.net.", "Online": true, "Capabilities": ["x"] },
            "Peer": {},
            "SomethingNew": { "nested": [1, 2, 3] }
        }"#;

        let status = TailscaleStatus::parse(verbose).expect("must tolerate unknown fields");

        assert!(status.is_logged_in());
        assert_eq!(status.hostname().as_deref(), Some("a.b.ts.net"));
    }

    #[test]
    fn funnel_status_reports_whether_anything_is_served() {
        assert!(funnel_is_serving(
            r#"{"AllowFunnel": {"r4nd0m.tail1a2b3c.ts.net:443": true}}"#
        ));
        assert!(!funnel_is_serving(r#"{"AllowFunnel": {}}"#));
        assert!(!funnel_is_serving(r#"{"AllowFunnel": null}"#));
        assert!(!funnel_is_serving(r#"{"TCP": {"443": {}}}"#));
        assert!(!funnel_is_serving(""));
        // A mapping explicitly turned off is not being served.
        assert!(!funnel_is_serving(
            r#"{"AllowFunnel": {"host:443": false}}"#
        ));
    }
}
