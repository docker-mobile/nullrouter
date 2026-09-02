//! A credential that can be handed to a child process without being printable.
//!
//! Upstream passes the tunnel token as `--token <value>` in argv
//! (`src/lib/tunnel/cloudflare/cloudflared.js`). On Linux `/proc/<pid>/cmdline` is
//! world-readable, so every local user can read a token that grants control of the
//! tunnel. This type exists so the value can only leave via the child's environment,
//! and so an accidental `{:?}` in a log line or an error message cannot print it.

use std::fmt;

/// A value that must reach the child process but must never reach a log.
///
/// `Debug` and `Display` both render a fixed placeholder. There is no `AsRef<str>`,
/// no `Deref`, and no getter that returns the value for general use: the only reader
/// is [`Secret::expose_for_child_env`], whose name is the audit marker.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a credential.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Whether the credential is empty, without revealing it.
    ///
    /// Callers use this to reject a blank token before spawning anything.
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Length in bytes, for diagnostics that need to say "a token was supplied".
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Read the credential, to place it in a child's environment block.
    ///
    /// Every call site is a place where the value could escape, so this name is
    /// deliberately long and searchable. Do not use it to build argv, a response
    /// body, or a log line.
    pub fn expose_for_child_env(&self) -> &str {
        &self.0
    }
}

/// Rendered instead of the value, in both `Debug` and `Display`.
const REDACTED: &str = "<redacted>";

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

/// Remove any occurrence of a secret from text that is about to be stored or returned.
///
/// The child's own output is the remaining leak path: `cloudflared` echoes parts of its
/// configuration, and a failing `tailscale` prints the arguments it was given. Log lines
/// pass through here before they enter a ring buffer or an HTTP response.
///
/// Very short secrets are not scrubbed. A two-character needle would match inside
/// ordinary words and turn a readable log into unreadable mush, and a credential that
/// short is not a credential.
pub fn scrub(text: &str, secrets: &[&Secret]) -> String {
    /// Below this length a match is more likely to be a coincidence than the credential.
    const MIN_SCRUBBABLE: usize = 6;

    let mut out = text.to_owned();
    for secret in secrets {
        let value = secret.expose_for_child_env();
        if value.len() >= MIN_SCRUBBABLE && out.contains(value) {
            out = out.replace(value, REDACTED);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Secret, scrub};

    #[test]
    fn debug_and_display_never_render_the_value() {
        let secret = Secret::new("eyJhIjoiYiJ9-very-secret-token");

        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert!(!format!("{secret:?} {secret}").contains("secret-token"));
    }

    #[test]
    fn debug_of_a_containing_struct_also_hides_it() {
        // The realistic leak is not `{:?}` on the secret itself, it is `{:?}` on the
        // request or config struct that holds one.
        #[derive(Debug)]
        #[allow(
            dead_code,
            reason = "both fields are read only through the derived Debug, which is \
                      exactly what this case checks"
        )]
        struct Request {
            name: &'static str,
            token: Secret,
        }

        let rendered = format!(
            "{:?}",
            Request {
                name: "named-tunnel",
                token: Secret::new("tunnel-token-value"),
            }
        );

        assert!(rendered.contains("named-tunnel"), "{rendered}");
        assert!(!rendered.contains("tunnel-token-value"), "{rendered}");
    }

    #[test]
    fn the_value_is_still_reachable_for_the_child_environment() {
        let secret = Secret::new("abc123");

        assert_eq!(secret.expose_for_child_env(), "abc123");
        assert_eq!(secret.len(), 6);
        assert!(!secret.is_empty());
        assert!(Secret::new("").is_empty());
    }

    #[test]
    fn scrub_removes_the_secret_from_child_output() {
        let token = Secret::new("s3cret-token-abc");
        let line = "failed to connect with token s3cret-token-abc, retrying";

        let scrubbed = scrub(line, &[&token]);

        assert_eq!(
            scrubbed,
            "failed to connect with token <redacted>, retrying"
        );
    }

    #[test]
    fn scrub_removes_every_occurrence_and_handles_several_secrets() {
        let first = Secret::new("aaaaaa-first");
        let second = Secret::new("bbbbbb-second");
        let line = "aaaaaa-first then bbbbbb-second then aaaaaa-first again";

        let scrubbed = scrub(line, &[&first, &second]);

        assert_eq!(scrubbed, "<redacted> then <redacted> then <redacted> again");
    }

    #[test]
    fn scrub_leaves_short_needles_alone() {
        // "at" would otherwise gut every log line that contains the word.
        let tiny = Secret::new("at");
        let line = "connection registered at edge";

        assert_eq!(scrub(line, &[&tiny]), line);
    }

    #[test]
    fn scrub_is_a_no_op_when_the_secret_is_absent() {
        let token = Secret::new("never-appears-here");

        assert_eq!(scrub("clean line", &[&token]), "clean line");
    }
}
