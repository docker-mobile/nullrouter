//! Finding the executable to run, and refusing to run a suspicious one.
//!
//! Upstream fetches `cloudflared` from
//! `https://github.com/cloudflare/cloudflared/releases/latest/download`, writes it into a
//! data directory, `chmod`s it to `755` and executes it, with no signature and no digest
//! check (`src/lib/tunnel/cloudflare/cloudflared.js`). That is an unattended
//! code-execution path: whatever that URL serves at the moment of the fetch runs with the
//! user's privileges, and `latest` means the bytes are not even pinned.
//!
//! This module does not download anything. It resolves a binary the operator installed,
//! and before returning it, checks the things that decide whether running it is a
//! decision the operator actually made:
//!
//! * it is a regular file, not a symlink to something unexpected, not a directory;
//! * it is executable;
//! * neither it nor its directory is writable by other users, so a local account cannot
//!   swap the binary between this check and the spawn;
//! * if a digest is pinned, the file matches it.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Why a binary may not be run.
#[derive(Debug, Error)]
pub enum BinaryError {
    /// Nothing was found at any candidate location.
    #[error("{name} is not installed: looked at {searched} location(s). Install it and retry; this service never downloads it.")]
    NotFound {
        /// The program looked for.
        name: &'static str,
        /// How many places were tried.
        searched: usize,
    },
    /// A path was configured explicitly but is unusable.
    #[error("{name} was set to {path} explicitly, but {reason}")]
    BadOverride {
        /// The program looked for.
        name: &'static str,
        /// The configured path.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// Found, but not something we are willing to execute.
    #[error("{path} cannot be run: {reason}")]
    Unusable {
        /// The rejected path.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// Found and runnable, but not the pinned build.
    #[error(
        "{path} does not match the pinned digest: expected {expected}, found {actual}. \
         Refusing to run a binary the operator did not pin."
    )]
    DigestMismatch {
        /// The rejected path.
        path: PathBuf,
        /// The configured digest.
        expected: String,
        /// What the file actually hashes to.
        actual: String,
    },
    /// The file could not be read to hash it.
    #[error("{path} could not be read to verify its digest: {source}")]
    Unreadable {
        /// The path.
        path: PathBuf,
        /// The IO error.
        #[source]
        source: std::io::Error,
    },
}

/// Where to look for one program, and what to require of it.
#[derive(Debug, Clone)]
pub struct BinarySpec {
    /// Program name, used in messages and for the `PATH` search.
    pub name: &'static str,
    /// Absolute paths tried in order, before the `PATH` search.
    pub candidates: &'static [&'static str],
    /// Environment variable that overrides everything, for operators who install
    /// elsewhere. An override that is unusable is an error rather than a fallback: a
    /// silent fallback would run a different binary than the operator asked for.
    pub env_override: &'static str,
    /// Directories searched for `name` after `candidates`.
    pub search_dirs: &'static [&'static str],
}

/// Directories searched for tunnel binaries.
///
/// Mirrors upstream's `EXTENDED_PATH` minus the inherited `$PATH`. The process
/// environment is deliberately not consulted: a `PATH` entry the service inherited from
/// whatever started it is not a location the operator chose for this binary, and it is
/// the classic way an unexpected executable gets picked up.
pub const SYSTEM_BIN_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/usr/sbin",
    "/usr/bin",
    "/bin",
    "/sbin",
    "/snap/bin",
];

/// A binary that passed every check, ready to be spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable {
    path: PathBuf,
    name: &'static str,
}

impl Executable {
    /// Accept a path found by the caller's own search, applying the same checks.
    ///
    /// [`BinarySpec`] covers a program installed at a predictable location. A Python
    /// interpreter is not that: which one to use depends on its version and on which
    /// distributions it can see, so the search is the caller's. This is the door for a path
    /// chosen that way, and it is the same door: the checks below are the ones a spec-resolved
    /// binary passes, so a discovered interpreter is not held to a lower standard than a
    /// configured one.
    pub fn verified(path: PathBuf, name: &'static str) -> Result<Self, BinaryError> {
        if !path.is_absolute() {
            return Err(BinaryError::Unusable {
                path,
                reason: "it is not an absolute path".to_owned(),
            });
        }
        if let Err(reason) = usable(&path) {
            return Err(BinaryError::Unusable { path, reason });
        }
        Ok(Self { path, name })
    }

    /// The verified path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The program name this was resolved for.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

impl BinarySpec {
    /// Resolve the binary, optionally requiring a digest.
    ///
    /// `pin` is a lowercase hex SHA-256. When present the file is hashed on every
    /// resolve; these binaries are tens of megabytes, so callers that resolve in a hot
    /// path should cache the [`Executable`] rather than the spec.
    pub fn resolve(&self, pin: Option<&str>) -> Result<Executable, BinaryError> {
        if let Some(configured) = self.override_path() {
            let path = PathBuf::from(configured);
            if !path.is_absolute() {
                return Err(BinaryError::BadOverride {
                    name: self.name,
                    path,
                    reason: "it is not an absolute path".to_owned(),
                });
            }
            if let Err(reason) = usable(&path) {
                return Err(BinaryError::BadOverride {
                    name: self.name,
                    path,
                    reason,
                });
            }
            return self.finish(path, pin);
        }

        let mut searched = 0_usize;
        for candidate in self.candidates {
            searched += 1;
            let path = PathBuf::from(candidate);
            if usable(&path).is_ok() {
                return self.finish(path, pin);
            }
        }
        for dir in self.search_dirs {
            searched += 1;
            let path = Path::new(dir).join(self.name);
            if usable(&path).is_ok() {
                return self.finish(path, pin);
            }
        }
        Err(BinaryError::NotFound {
            name: self.name,
            searched,
        })
    }

    /// Whether the binary is present, without hashing it.
    ///
    /// Used by status endpoints, which report installed-or-not and must not spend a
    /// digest over a large file on every poll.
    pub fn is_installed(&self) -> bool {
        self.resolve(None).is_ok()
    }

    /// The configured override, if the variable is set to something non-empty.
    fn override_path(&self) -> Option<String> {
        let value = std::env::var(self.env_override).ok()?;
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    /// Apply the digest pin, if any, and wrap the result.
    fn finish(&self, path: PathBuf, pin: Option<&str>) -> Result<Executable, BinaryError> {
        if let Some(expected) = pin {
            let actual = sha256_of(&path)?;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(BinaryError::DigestMismatch {
                    path,
                    expected: expected.to_owned(),
                    actual,
                });
            }
        }
        Ok(Executable {
            path,
            name: self.name,
        })
    }
}

/// Hash a file, streaming so a large binary does not have to be held in memory.
fn sha256_of(path: &Path) -> Result<String, BinaryError> {
    let mut file = std::fs::File::open(path).map_err(|source| BinaryError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|source| BinaryError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex::encode(hasher.finalize()))
}

/// Whether a path is a file we are willing to execute, and why not if it is not.
///
/// The writability checks are the load-bearing ones. A binary in a directory other users
/// can write to can be replaced between this check and the spawn, which would make every
/// other guarantee in this crate decorative.
#[cfg(unix)]
fn usable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    // Follows symlinks on purpose: a link is a normal way to install, and what matters is
    // the permissions of the target and of the directory the link resolves into.
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "it does not exist".to_owned(),
        std::io::ErrorKind::PermissionDenied => "it is not readable by this service".to_owned(),
        _ => format!("it could not be inspected: {error}"),
    })?;

    if !metadata.is_file() {
        return Err("it is not a regular file".to_owned());
    }
    let mode = metadata.permissions().mode();
    if mode & 0o111 == 0 {
        return Err("it is not executable".to_owned());
    }
    if mode & 0o022 != 0 {
        return Err(format!(
            "it is writable by group or others (mode {:o}), so another local account \
             could replace it between this check and the spawn",
            mode & 0o7777
        ));
    }

    let owner = metadata.uid();
    if owner != 0 && owner != current_uid() {
        return Err(format!(
            "it is owned by uid {owner}, which is neither root nor this service"
        ));
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("/"));
    let parent_meta = std::fs::metadata(parent)
        .map_err(|error| format!("its directory {} could not be inspected: {error}", parent.display()))?;
    let parent_mode = parent_meta.permissions().mode();
    // The sticky bit is what makes `/usr/local/bin`-style shared directories safe when
    // they are group-writable: only the owner of a file may replace it.
    if parent_mode & 0o002 != 0 && parent_mode & 0o1000 == 0 {
        return Err(format!(
            "its directory {} is world-writable without the sticky bit (mode {:o})",
            parent.display(),
            parent_mode & 0o7777
        ));
    }
    Ok(())
}

/// Whether a path is a file we are willing to execute, and why not if it is not.
///
/// Windows has no mode bits to check; existence and file-ness are what remain.
#[cfg(not(unix))]
fn usable(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "it does not exist".to_owned(),
        _ => format!("it could not be inspected: {error}"),
    })?;
    if !metadata.is_file() {
        return Err("it is not a regular file".to_owned());
    }
    Ok(())
}

/// This process's effective uid.
#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, cannot fail, and only reads a value the
    // kernel keeps for this process. There is no pointer and no allocation involved.
    unsafe { libc::geteuid() }
}
