//! The deliberately narrow privileged side of nullrouter's MITM feature.
//!
//! The API service is never allowed to turn a dashboard request into a shell command.  This crate
//! holds the only operations that alter host-wide state: it accepts one of four tool identifiers and
//! writes only delimited blocks for their fixed host allowlists.  Its executable refuses to run unless
//! it is already root/admin; it never invokes `sudo`, never reads a password, and never keeps one.
//!
//! The library works on an explicit path so tests can exercise the transactional rewrite without ever
//! touching `/etc/hosts`.  The executable has no such option: production always uses the platform
//! hosts file.

use std::{fs, io::Write as _, path::Path};

use thiserror::Error;

/// The fixed groups the MITM proxy may redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Google Antigravity endpoints.
    Antigravity,
    /// GitHub Copilot endpoint.
    Copilot,
    /// Cursor endpoint.
    Cursor,
    /// Kiro endpoints.
    Kiro,
}

impl Tool {
    /// Parse one fixed, lower-case tool name.
    pub fn parse(value: &str) -> Result<Self, HelperError> {
        match value {
            "antigravity" => Ok(Self::Antigravity),
            "copilot" => Ok(Self::Copilot),
            "cursor" => Ok(Self::Cursor),
            "kiro" => Ok(Self::Kiro),
            _other => Err(HelperError::UnknownTool(value.to_owned())),
        }
    }

    /// Stable external name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::Kiro => "kiro",
        }
    }

    /// The only hostnames this group may redirect.
    pub const fn hosts(self) -> &'static [&'static str] {
        match self {
            Self::Antigravity => &[
                "daily-cloudcode-pa.googleapis.com",
                "cloudcode-pa.googleapis.com",
            ],
            Self::Copilot => &["api.individual.githubcopilot.com"],
            Self::Cursor => &["api2.cursor.sh"],
            Self::Kiro => &[
                "runtime.us-east-1.kiro.dev",
                "q.us-east-1.amazonaws.com",
                "codewhisperer.us-east-1.amazonaws.com",
            ],
        }
    }
}

/// A failure that the privileged helper can state without printing host content.
#[derive(Debug, Error)]
pub enum HelperError {
    /// Caller named an unrecognised tool.
    #[error("{0:?} is not a MITM tool group")]
    UnknownTool(String),
    /// Hosts file could not be read.
    #[error("could not read the hosts file: {0}")]
    Read(#[source] std::io::Error),
    /// Temporary/target write failed.
    #[error("could not replace the hosts file: {0}")]
    Write(#[source] std::io::Error),
    /// The existing marker layout is corrupt; guessing could delete unrelated entries.
    #[error("the hosts file has a corrupt nullrouter MITM marker block")]
    CorruptMarkers,
    /// Helper was not launched elevated.
    #[error("nullrouter-mitm-helper must be launched as root/administrator")]
    NotElevated,
}

const BEGIN: &str = "# nullrouter-mitm begin ";
const END: &str = "# nullrouter-mitm end ";

fn markers(tool: Tool) -> (String, String) {
    (
        format!("{BEGIN}{}", tool.as_str()),
        format!("{END}{}", tool.as_str()),
    )
}

fn eol(input: &str) -> &'static str {
    if input.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Remove exactly this tool's marked block from a hosts document.
///
/// Unmarked lines are deliberately not interpreted, even if they contain an allowlisted hostname: they
/// might predate nullrouter or belong to an operator's unrelated configuration.
fn without_block(input: &str, tool: Tool) -> Result<String, HelperError> {
    let (begin, end) = markers(tool);
    let newline = eol(input);
    let mut output = Vec::new();
    let mut inside = false;
    let mut saw_begin = false;
    let mut saw_end = false;

    for line in input.lines() {
        if line == begin {
            if inside || saw_begin {
                return Err(HelperError::CorruptMarkers);
            }
            inside = true;
            saw_begin = true;
            continue;
        }
        if line == end {
            if !inside || saw_end {
                return Err(HelperError::CorruptMarkers);
            }
            inside = false;
            saw_end = true;
            continue;
        }
        if !inside {
            output.push(line);
        }
    }
    if inside || saw_begin != saw_end {
        return Err(HelperError::CorruptMarkers);
    }

    let mut joined = output.join(newline);
    if !joined.is_empty() {
        joined.push_str(newline);
    }
    Ok(joined)
}

/// Add one exact, owned hosts block, replacing a prior block for the same tool atomically.
pub fn enable_hosts(path: &Path, tool: Tool) -> Result<(), HelperError> {
    let original = fs::read_to_string(path).map_err(HelperError::Read)?;
    let newline = eol(&original);
    let mut next = without_block(&original, tool)?;
    if !next.is_empty() && !next.ends_with(newline) {
        next.push_str(newline);
    }
    let (begin, end) = markers(tool);
    next.push_str(&begin);
    next.push_str(newline);
    for host in tool.hosts() {
        next.push_str("127.0.0.1 ");
        next.push_str(host);
        next.push_str(newline);
    }
    next.push_str(&end);
    next.push_str(newline);
    atomic_replace(path, next.as_bytes())
}

/// Remove only this tool's owned hosts block.
pub fn disable_hosts(path: &Path, tool: Tool) -> Result<(), HelperError> {
    let original = fs::read_to_string(path).map_err(HelperError::Read)?;
    let next = without_block(&original, tool)?;
    atomic_replace(path, next.as_bytes())
}

/// Whether the exact owned block is present and structurally valid.
pub fn hosts_enabled(path: &Path, tool: Tool) -> Result<bool, HelperError> {
    let content = fs::read_to_string(path).map_err(HelperError::Read)?;
    let (begin, end) = markers(tool);
    let starts = content.lines().filter(|line| *line == begin).count();
    let ends = content.lines().filter(|line| *line == end).count();
    if starts > 1 || ends > 1 || starts != ends {
        return Err(HelperError::CorruptMarkers);
    }
    Ok(starts == 1)
}

/// Write a sibling temporary file and rename it over the target.
///
/// A same-directory rename means readers see either the old complete file or the new complete file,
/// never a partially-written hosts file. The executable only invokes this after elevation has been
/// checked; keeping the path explicit here makes the dangerous system path testable with temp files.
fn atomic_replace(path: &Path, contents: &[u8]) -> Result<(), HelperError> {
    let parent = path.parent().ok_or_else(|| {
        HelperError::Write(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "hosts file has no parent directory",
        ))
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            HelperError::Write(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "hosts file has no UTF-8 file name",
            ))
        })?;
    let temporary = parent.join(format!(".{name}.nullrouter-mitm-new"));
    {
        let mut file = fs::File::create(&temporary).map_err(HelperError::Write)?;
        file.write_all(contents).map_err(HelperError::Write)?;
        file.sync_all().map_err(HelperError::Write)?;
    }
    fs::rename(&temporary, path).map_err(HelperError::Write)
}

/// `true` only when the process was started elevated.
#[must_use]
pub fn elevated() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` takes no arguments, has no failure mode, and reads a kernel-owned scalar.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(windows)]
    {
        false
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, reason = "test fixtures")]

    use std::fs;

    use tempfile::tempdir;

    use super::{HelperError, Tool, ca, disable_hosts, enable_hosts, hosts_enabled};

    #[test]
    fn enable_is_idempotent_and_preserves_unrelated_lines() {
        let directory = tempdir().expect("temp directory");
        let hosts = directory.path().join("hosts");
        fs::write(
            &hosts,
            "127.0.0.1 localhost\n10.0.0.5 api2.cursor.sh # operator entry\n",
        )
        .expect("fixture");

        enable_hosts(&hosts, Tool::Cursor).expect("enable once");
        enable_hosts(&hosts, Tool::Cursor).expect("enable twice");
        let content = fs::read_to_string(&hosts).expect("result");

        assert!(content.contains("127.0.0.1 localhost"));
        assert!(content.contains("10.0.0.5 api2.cursor.sh # operator entry"));
        assert_eq!(content.matches("# nullrouter-mitm begin cursor").count(), 1);
        assert!(hosts_enabled(&hosts, Tool::Cursor).expect("status"));
    }

    #[test]
    fn disable_removes_only_the_owned_block() {
        let directory = tempdir().expect("temp directory");
        let hosts = directory.path().join("hosts");
        fs::write(&hosts, "127.0.0.1 localhost\n").expect("fixture");
        enable_hosts(&hosts, Tool::Kiro).expect("enable");
        disable_hosts(&hosts, Tool::Kiro).expect("disable");
        let content = fs::read_to_string(&hosts).expect("result");

        assert_eq!(content, "127.0.0.1 localhost\n");
        assert!(!hosts_enabled(&hosts, Tool::Kiro).expect("status"));
    }

    #[test]
    fn a_corrupt_marker_is_refused_without_rewriting() {
        let directory = tempdir().expect("temp directory");
        let hosts = directory.path().join("hosts");
        let original = "# nullrouter-mitm begin cursor\n127.0.0.1 api2.cursor.sh\n";
        fs::write(&hosts, original).expect("fixture");

        let error = disable_hosts(&hosts, Tool::Cursor).expect_err("corrupt markers fail");
        assert!(matches!(error, HelperError::CorruptMarkers));
        assert_eq!(fs::read_to_string(&hosts).expect("unchanged"), original);
    }

    #[test]
    fn unrecognised_tool_is_refused() {
        assert!(matches!(
            Tool::parse("cursor; rm -rf /"),
            Err(HelperError::UnknownTool(_))
        ));
    }

    #[test]
    fn authority_is_stable_and_private() {
        let directory = tempdir().expect("temp directory");
        let first = ca::ensure(directory.path()).expect("first authority");
        let second = ca::ensure(directory.path()).expect("same authority");

        assert_eq!(
            first, second,
            "re-running must not silently rotate a trusted CA"
        );
        assert!(first.certificate.is_file());
        assert!(first.key.is_file());
        assert_eq!(first.fingerprint.len(), 64, "SHA-256 as lower hex");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&first.key)
                    .expect("key metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(directory.path())
                    .expect("directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }
}

/// Certificate authority material kept under an owner-only directory.
pub mod ca {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use sha2::{Digest as _, Sha256};

    use super::HelperError;

    /// The root private-key filename.
    const KEY_FILE: &str = "root-ca.key";
    /// The root certificate filename.
    const CERT_FILE: &str = "root-ca.crt";
    /// Stable identity, used rather than a display name when a system adapter removes trust.
    const FINGERPRINT_FILE: &str = "root-ca.sha256";

    /// Generated CA paths plus its SHA-256 DER fingerprint.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Authority {
        /// PEM private key, never returned through the API.
        pub key: PathBuf,
        /// PEM certificate, which a privileged platform adapter may install.
        pub certificate: PathBuf,
        /// Hex-encoded SHA-256 over the certificate DER.
        pub fingerprint: String,
    }

    /// Generate once, or read the existing authority's recorded fingerprint.
    ///
    /// The directory must already be owner-only in production. This function makes the key/certificate
    /// files owner-readable only on Unix, then writes a public fingerprint next to them. It never
    /// rotates an existing CA implicitly: rotation while the old CA remains trusted strands intercepted
    /// clients and is an administrator's explicit operation.
    pub fn ensure(directory: &Path) -> Result<Authority, HelperError> {
        fs::create_dir_all(directory).map_err(HelperError::Write)?;
        restrict_directory(directory)?;
        let key = directory.join(KEY_FILE);
        let certificate = directory.join(CERT_FILE);
        let fingerprint_file = directory.join(FINGERPRINT_FILE);

        if key.is_file() && certificate.is_file() && fingerprint_file.is_file() {
            return Ok(Authority {
                key,
                certificate,
                fingerprint: fs::read_to_string(fingerprint_file)
                    .map_err(HelperError::Read)?
                    .trim()
                    .to_owned(),
            });
        }
        if key.exists() || certificate.exists() || fingerprint_file.exists() {
            return Err(HelperError::CorruptMarkers);
        }

        let mut parameters = CertificateParams::default();
        parameters.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        parameters
            .distinguished_name
            .push(DnType::CommonName, "nullrouter MITM Root CA");
        parameters
            .distinguished_name
            .push(DnType::OrganizationName, "nullrouter");
        let key_pair = KeyPair::generate().map_err(|error| io_error(error.to_string()))?;
        let cert = parameters
            .self_signed(&key_pair)
            .map_err(|error| io_error(error.to_string()))?;
        let certificate_pem = cert.pem();
        let private_key_pem = key_pair.serialize_pem();
        let fingerprint = hex::encode(Sha256::digest(cert.der()));

        write_private(&key, private_key_pem.as_bytes())?;
        write_private(&certificate, certificate_pem.as_bytes())?;
        write_private(&fingerprint_file, fingerprint.as_bytes())?;
        Ok(Authority {
            key,
            certificate,
            fingerprint,
        })
    }

    fn io_error(message: String) -> HelperError {
        HelperError::Write(std::io::Error::other(message))
    }

    fn write_private(path: &Path, bytes: &[u8]) -> Result<(), HelperError> {
        use std::io::Write as _;
        let mut file = fs::File::create(path).map_err(HelperError::Write)?;
        file.write_all(bytes).map_err(HelperError::Write)?;
        file.sync_all().map_err(HelperError::Write)?;
        restrict_file(path)
    }

    #[cfg(unix)]
    fn restrict_directory(path: &Path) -> Result<(), HelperError> {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(HelperError::Write)
    }

    #[cfg(not(unix))]
    fn restrict_directory(_path: &Path) -> Result<(), HelperError> {
        Ok(())
    }

    #[cfg(unix)]
    fn restrict_file(path: &Path) -> Result<(), HelperError> {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(HelperError::Write)
    }

    #[cfg(not(unix))]
    fn restrict_file(_path: &Path) -> Result<(), HelperError> {
        Ok(())
    }
}
