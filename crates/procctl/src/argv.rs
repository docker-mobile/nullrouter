//! Argument vectors built from validated pieces, never from concatenated text.
//!
//! Upstream builds most of its tunnel commands as shell strings —
//! `` execSync(`"${bin}" ${SOCKET_FLAG.join(" ")} status --json`) `` — so every value in
//! them is one quoting mistake away from being a command. Nothing here goes through a
//! shell, and every caller-influenced piece has to pass a character allowlist before it
//! can become an argument.
//!
//! The second thing this prevents is argument injection. A value of `--config` reaching
//! argv as a bare token would be read by the child as a flag, not as data, so a leading
//! `-` is rejected on every value.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Why a piece could not become an argument.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArgError {
    /// An empty argument is never something one of our operations wants to pass.
    #[error("{field} must not be empty")]
    Empty {
        /// Which piece was rejected.
        field: &'static str,
    },
    /// Over the per-argument length cap.
    #[error("{field} is {len} bytes, over the {max} byte limit")]
    TooLong {
        /// Which piece was rejected.
        field: &'static str,
        /// Its length.
        len: usize,
        /// The cap.
        max: usize,
    },
    /// A character outside the allowlist for this kind of argument.
    #[error("{field} contains {character:?}, which is not allowed in {kind}")]
    BadCharacter {
        /// Which piece was rejected.
        field: &'static str,
        /// The first offending character.
        character: char,
        /// The allowlist it was checked against.
        kind: &'static str,
    },
    /// A value the child would read as a flag rather than as data.
    #[error("{field} starts with '-', which the child would read as a flag")]
    LooksLikeFlag {
        /// Which piece was rejected.
        field: &'static str,
    },
    /// A path that has to be absolute but is not.
    #[error("{field} must be an absolute path")]
    NotAbsolute {
        /// Which piece was rejected.
        field: &'static str,
    },
}

/// The longest any single argument may be.
///
/// Well above every real hostname, tunnel name and path, and far below the point where
/// an argument list starts to threaten `E2BIG`.
const MAX_ARG_LEN: usize = 1024;

/// Which characters an argument may contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Charset {
    /// Hostnames, tunnel names, identifiers: `A-Za-z0-9`, `-`, `_`, `.`.
    Token,
    /// Filesystem paths: token characters plus `/`, `\`, `:`, `+`, `@` and space.
    Path,
    /// A package requirement: token characters plus `[`, `]`, `,`, `=`, `<`, `>`.
    ///
    /// Its own charset because `headroom-ai[proxy,code]` is not an identifier and is not a path,
    /// and giving it a bypass instead would put a hole in the one place every value is checked.
    /// The comparison characters are here so a pinned requirement stays expressible; note that
    /// none of the additions can start a command, open a subshell, or redirect.
    Requirement,
}

impl Charset {
    /// Name used in the error message.
    const fn kind(self) -> &'static str {
        match self {
            Self::Token => "an identifier",
            Self::Path => "a path",
            Self::Requirement => "a package requirement",
        }
    }

    /// Whether one character is acceptable.
    const fn allows(self, character: char) -> bool {
        let token = character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.');
        match self {
            Self::Token => token,
            Self::Path => token || matches!(character, '/' | '\\' | ':' | '+' | '@' | ' '),
            Self::Requirement => token || matches!(character, '[' | ']' | ',' | '=' | '<' | '>'),
        }
    }
}

/// Check one caller-influenced value against a charset.
fn check(field: &'static str, value: &str, charset: Charset) -> Result<(), ArgError> {
    if value.is_empty() {
        return Err(ArgError::Empty { field });
    }
    if value.len() > MAX_ARG_LEN {
        return Err(ArgError::TooLong {
            field,
            len: value.len(),
            max: MAX_ARG_LEN,
        });
    }
    if value.starts_with('-') {
        return Err(ArgError::LooksLikeFlag { field });
    }
    if let Some(character) = value.chars().find(|c| !charset.allows(*c)) {
        return Err(ArgError::BadCharacter {
            field,
            character,
            kind: charset.kind(),
        });
    }
    Ok(())
}

/// An argument vector under construction.
///
/// Flags are `&'static str` because they are written in this repository, never received
/// over the wire. Values go through [`check`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Argv(Vec<String>);

impl Argv {
    /// Start an empty vector.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Append a literal flag such as `--json`.
    ///
    /// Debug-asserts the leading `-` so a value cannot be smuggled in through this door
    /// during development; in release the flag is simply appended, since it is a literal
    /// from this repository either way.
    #[must_use]
    pub fn flag(mut self, flag: &'static str) -> Self {
        debug_assert!(flag.starts_with('-'), "{flag:?} is not a flag");
        self.0.push(flag.to_owned());
        self
    }

    /// Append a literal subcommand such as `tunnel` or `status`.
    #[must_use]
    pub fn word(mut self, word: &'static str) -> Self {
        debug_assert!(!word.starts_with('-'), "{word:?} is a flag, not a word");
        self.0.push(word.to_owned());
        self
    }

    /// Append a validated identifier: hostname, tunnel name, device name.
    pub fn token(mut self, field: &'static str, value: &str) -> Result<Self, ArgError> {
        check(field, value, Charset::Token)?;
        self.0.push(value.to_owned());
        Ok(self)
    }

    /// Append `flag=value` with a validated identifier, the form `tailscale up` wants.
    pub fn token_eq(
        mut self,
        flag: &'static str,
        field: &'static str,
        value: &str,
    ) -> Result<Self, ArgError> {
        debug_assert!(flag.starts_with('-'), "{flag:?} is not a flag");
        check(field, value, Charset::Token)?;
        self.0.push(format!("{flag}={value}"));
        Ok(self)
    }

    /// Append a validated package requirement, such as `headroom-ai[proxy,code]`.
    ///
    /// Separate from [`Argv::token`] because a requirement carries brackets and commas that an
    /// identifier must not. It is still checked, and still refuses a leading `-`: a requirement
    /// that reads as a flag would change what the package manager does rather than what it
    /// installs.
    pub fn requirement(mut self, field: &'static str, value: &str) -> Result<Self, ArgError> {
        check(field, value, Charset::Requirement)?;
        self.0.push(value.to_owned());
        Ok(self)
    }

    /// Append a validated absolute path.
    pub fn abs_path(mut self, field: &'static str, value: &Path) -> Result<Self, ArgError> {
        let text = value.to_string_lossy();
        check(field, text.as_ref(), Charset::Path)?;
        if !value.is_absolute() {
            return Err(ArgError::NotAbsolute { field });
        }
        self.0.push(text.into_owned());
        Ok(self)
    }

    /// Append `flag=<absolute path>`, the form `tailscaled` wants.
    pub fn abs_path_eq(
        mut self,
        flag: &'static str,
        field: &'static str,
        value: &Path,
    ) -> Result<Self, ArgError> {
        debug_assert!(flag.starts_with('-'), "{flag:?} is not a flag");
        let text = value.to_string_lossy();
        check(field, text.as_ref(), Charset::Path)?;
        if !value.is_absolute() {
            return Err(ArgError::NotAbsolute { field });
        }
        self.0.push(format!("{flag}={text}"));
        Ok(self)
    }

    /// Append a port. Always safe: the type cannot hold anything but digits.
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.0.push(port.to_string());
        self
    }

    /// Append a loopback HTTP origin for the given port.
    ///
    /// Built here rather than by the caller so no caller can choose the host: a tunnel
    /// that points anywhere but this machine's loopback is a proxy for someone else's
    /// service, which is not what any of these operations are for.
    #[must_use]
    pub fn loopback_origin(mut self, port: u16) -> Self {
        self.0.push(format!("http://127.0.0.1:{port}"));
        self
    }

    /// The finished vector.
    #[must_use]
    pub fn into_vec(self) -> Vec<String> {
        self.0
    }

    /// Borrow the arguments, for logging or assertions.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

/// A directory this crate may create and hand to a child as state or socket storage.
///
/// Kept as a distinct type so an operation cannot be given an arbitrary path to write
/// into: the only constructor validates the path the same way an argument is validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir(PathBuf);

impl StateDir {
    /// Accept a directory, requiring it to be absolute and free of surprising characters.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ArgError> {
        let path = path.into();
        let text = path.to_string_lossy();
        check("state directory", text.as_ref(), Charset::Path)?;
        if !path.is_absolute() {
            return Err(ArgError::NotAbsolute {
                field: "state directory",
            });
        }
        Ok(Self(path))
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A file inside the directory, with a validated name.
    pub fn join(&self, name: &str) -> Result<PathBuf, ArgError> {
        check("state file name", name, Charset::Token)?;
        Ok(self.0.join(name))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ArgError, Argv, MAX_ARG_LEN, StateDir};

    #[test]
    fn a_plain_command_assembles_in_order() {
        let argv = Argv::new()
            .word("tunnel")
            .word("run")
            .flag("--no-autoupdate")
            .token("tunnel name", "my-tunnel")
            .expect("a plain name is valid");

        assert_eq!(
            argv.into_vec(),
            vec!["tunnel", "run", "--no-autoupdate", "my-tunnel"]
        );
    }

    #[test]
    fn a_value_that_looks_like_a_flag_is_refused() {
        // The whole point: `--config` as a "hostname" would be read by the child as a
        // flag, and the next real argument would become its value.
        let error = Argv::new()
            .token("hostname", "--config")
            .expect_err("a leading dash must be refused");

        assert_eq!(error, ArgError::LooksLikeFlag { field: "hostname" });
    }

    #[test]
    fn shell_metacharacters_are_refused_even_though_no_shell_runs() {
        for hostile in [
            "a;rm -rf /",
            "a$(id)",
            "a`id`",
            "a|b",
            "a&b",
            "a>b",
            "a\nb",
            "a\0b",
            "a'b",
            "a\"b",
            "a*b",
            "a\tb",
        ] {
            let error = Argv::new()
                .token("hostname", hostile)
                .expect_err("must refuse");

            assert!(
                matches!(error, ArgError::BadCharacter { .. }),
                "{hostile:?} gave {error:?}"
            );
        }
    }

    #[test]
    fn an_empty_value_is_refused() {
        let error = Argv::new().token("hostname", "").expect_err("must refuse");

        assert_eq!(error, ArgError::Empty { field: "hostname" });
    }

    #[test]
    fn an_overlong_value_is_refused() {
        let long = "a".repeat(MAX_ARG_LEN + 1);

        let error = Argv::new()
            .token("hostname", &long)
            .expect_err("must refuse");

        assert_eq!(
            error,
            ArgError::TooLong {
                field: "hostname",
                len: MAX_ARG_LEN + 1,
                max: MAX_ARG_LEN,
            }
        );
    }

    #[test]
    fn a_value_at_the_length_limit_is_accepted() {
        let at_limit = "a".repeat(MAX_ARG_LEN);

        let argv = Argv::new()
            .token("hostname", &at_limit)
            .expect("exactly at the cap is inside it");

        assert_eq!(argv.as_slice(), [at_limit]);
    }

    #[test]
    fn token_eq_joins_with_an_equals_sign() {
        let argv = Argv::new()
            .word("up")
            .token_eq("--hostname", "hostname", "r4nd0m-id")
            .expect("valid");

        assert_eq!(argv.into_vec(), vec!["up", "--hostname=r4nd0m-id"]);
    }

    #[test]
    fn token_eq_validates_its_value_too() {
        // Without this, `--hostname=$(id)` would sail past the leading-dash check
        // because the dash belongs to the flag.
        let error = Argv::new()
            .token_eq("--hostname", "hostname", "$(id)")
            .expect_err("must refuse");

        assert!(matches!(error, ArgError::BadCharacter { .. }), "{error:?}");
    }

    #[test]
    fn paths_accept_separators_that_identifiers_do_not() {
        let argv = Argv::new()
            .flag("--socket")
            .abs_path("socket", Path::new("/var/run/tailscale/tailscaled.sock"))
            .expect("a real socket path is valid");

        assert_eq!(
            argv.into_vec(),
            vec!["--socket", "/var/run/tailscale/tailscaled.sock"]
        );
    }

    #[test]
    fn a_requirement_may_carry_brackets_and_commas() {
        let argv = Argv::new()
            .word("install")
            .requirement("requirement", "headroom-ai[proxy,code,ml]")
            .expect("a real extras requirement is valid");

        assert_eq!(
            argv.into_vec(),
            vec!["install", "headroom-ai[proxy,code,ml]"]
        );
        assert!(
            Argv::new()
                .requirement("requirement", "headroom-ai==1.2.3")
                .is_ok(),
            "a pinned requirement must stay expressible"
        );
    }

    #[test]
    fn a_requirement_cannot_carry_a_url_a_path_or_a_metacharacter() {
        // The shapes that would turn an install into something else entirely: a package fetched
        // from an attacker's index, a local file, a second command.
        for hostile in [
            "https://evil.example.com/x.whl",
            "/tmp/evil.whl",
            "a;id",
            "a$(id)",
            "a b",
            "a|b",
            "--index-url=https://evil.example.com",
            "-r requirements.txt",
            "a`id`",
            "",
        ] {
            assert!(
                Argv::new().requirement("requirement", hostile).is_err(),
                "{hostile:?} was accepted as a requirement"
            );
        }
    }

    #[test]
    fn a_relative_path_is_refused() {
        let error = Argv::new()
            .abs_path("socket", Path::new("relative/path.sock"))
            .expect_err("must refuse");

        assert_eq!(error, ArgError::NotAbsolute { field: "socket" });
    }

    #[test]
    fn a_path_with_a_shell_metacharacter_is_refused() {
        let error = Argv::new()
            .abs_path("socket", Path::new("/tmp/$(id)/x.sock"))
            .expect_err("must refuse");

        assert!(matches!(error, ArgError::BadCharacter { .. }), "{error:?}");
    }

    #[test]
    fn abs_path_eq_joins_and_validates() {
        let argv = Argv::new()
            .abs_path_eq("--statedir", "state directory", Path::new("/var/lib/ts"))
            .expect("valid");

        assert_eq!(argv.into_vec(), vec!["--statedir=/var/lib/ts"]);

        let error = Argv::new()
            .abs_path_eq("--statedir", "state directory", Path::new("rel"))
            .expect_err("must refuse");
        assert_eq!(
            error,
            ArgError::NotAbsolute {
                field: "state directory"
            }
        );
    }

    #[test]
    fn a_port_cannot_be_anything_but_digits() {
        let argv = Argv::new().word("funnel").flag("--bg").port(20128);

        assert_eq!(argv.into_vec(), vec!["funnel", "--bg", "20128"]);
    }

    #[test]
    fn the_origin_host_is_fixed_to_loopback() {
        // A caller must not be able to point a tunnel at another host.
        let argv = Argv::new().flag("--url").loopback_origin(20128);

        assert_eq!(argv.into_vec(), vec!["--url", "http://127.0.0.1:20128"]);
    }

    #[test]
    fn a_state_directory_must_be_absolute_and_clean() {
        let good = StateDir::new(PathBuf::from("/var/lib/nullrouter/tailscale"))
            .expect("absolute and clean");
        assert_eq!(good.path(), Path::new("/var/lib/nullrouter/tailscale"));

        assert_eq!(
            StateDir::new("relative").expect_err("must refuse"),
            ArgError::NotAbsolute {
                field: "state directory"
            }
        );
        assert!(matches!(
            StateDir::new("/tmp/`id`").expect_err("must refuse"),
            ArgError::BadCharacter { .. }
        ));
    }

    #[test]
    fn a_state_file_name_cannot_escape_its_directory() {
        let dir = StateDir::new("/var/lib/nullrouter").expect("valid");

        // `..` is refused not by a traversal check but because `/` is not in the
        // identifier charset, so no name can contain a separator at all.
        assert!(matches!(
            dir.join("../../etc/passwd").expect_err("must refuse"),
            ArgError::BadCharacter { .. }
        ));
        assert_eq!(
            dir.join("tailscaled.sock").expect("a plain name is fine"),
            Path::new("/var/lib/nullrouter/tailscaled.sock")
        );
    }
}
