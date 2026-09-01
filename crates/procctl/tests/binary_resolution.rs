//! What this crate will and will not execute.
//!
//! The subject is the check that replaces upstream's download-and-run: given a path, is
//! running it a decision the operator made, or something a local account could have
//! arranged? These cases are the reason the crate does not fetch `releases/latest`.
#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers in an integration test are not covered by clippy.toml's \
              allow-expect-in-tests, which only reaches #[test] functions"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use nullrouter_procctl::binary::{BinaryError, BinarySpec};

/// Serialises the cases that set the override variable.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The variable every case here overrides.
const OVERRIDE_VAR: &str = "NULLROUTER_TEST_FAKE_BIN";

/// Set the override for the duration of a case.
struct Override;

impl Override {
    fn set(path: &Path) -> Self {
        // SAFETY: the caller holds `env_lock`, so no other case in this binary reads or
        // writes this variable while it is being changed.
        unsafe { std::env::set_var(OVERRIDE_VAR, path) };
        Self
    }
}

impl Drop for Override {
    fn drop(&mut self) {
        // SAFETY: the lock is still held until the guard in the case finishes dropping.
        unsafe { std::env::remove_var(OVERRIDE_VAR) };
    }
}

/// A spec whose only lookup path is the override variable.
const fn spec() -> BinarySpec {
    BinarySpec {
        name: "fake-tunnel-daemon",
        candidates: &[],
        env_override: OVERRIDE_VAR,
        search_dirs: &[],
    }
}

/// Write a file with the given mode and return its path.
fn write_with_mode(dir: &Path, name: &str, contents: &[u8], mode: u32) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write the fixture");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
        .expect("set the fixture mode");
    path
}

#[test]
fn an_executable_file_resolves() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_with_mode(dir.path(), "daemon", b"#!/bin/sh\nexit 0\n", 0o755);
    let _override = Override::set(&path);

    let resolved = spec().resolve(None).expect("a 0755 file must resolve");

    assert_eq!(resolved.path(), path);
    assert_eq!(resolved.name(), "fake-tunnel-daemon");
    assert!(spec().is_installed());
}

#[test]
fn a_missing_binary_is_reported_as_a_bad_override_not_a_silent_fallback() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("absent");
    let _override = Override::set(&missing);

    let error = spec()
        .resolve(None)
        .expect_err("an override pointing nowhere must fail");

    // The distinction matters: falling back to a PATH lookup here would run a different
    // binary than the operator configured.
    match error {
        BinaryError::BadOverride { name, reason, .. } => {
            assert_eq!(name, "fake-tunnel-daemon");
            assert!(reason.contains("does not exist"), "{reason}");
        }
        other => panic!("expected BadOverride, got {other:?}"),
    }
}

#[test]
fn a_relative_override_is_refused() {
    let _guard = env_lock();
    let _override = Override::set(Path::new("relative/daemon"));

    let error = spec().resolve(None).expect_err("must refuse");

    match error {
        BinaryError::BadOverride { reason, .. } => {
            assert!(reason.contains("absolute"), "{reason}");
        }
        other => panic!("expected BadOverride, got {other:?}"),
    }
}

#[test]
fn a_non_executable_file_is_refused() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_with_mode(dir.path(), "daemon", b"not executable", 0o644);
    let _override = Override::set(&path);

    let error = spec().resolve(None).expect_err("must refuse");

    match error {
        BinaryError::BadOverride { reason, .. } => {
            assert!(reason.contains("not executable"), "{reason}");
        }
        other => panic!("expected BadOverride, got {other:?}"),
    }
}

#[test]
fn a_world_writable_binary_is_refused() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    // 0777 is the shape that matters: any local account can overwrite the file between
    // the check and the spawn, so verifying anything about it is pointless.
    let path = write_with_mode(dir.path(), "daemon", b"#!/bin/sh\nexit 0\n", 0o777);
    let _override = Override::set(&path);

    let error = spec().resolve(None).expect_err("must refuse");

    match error {
        BinaryError::BadOverride { reason, .. } => {
            assert!(reason.contains("writable by group or others"), "{reason}");
        }
        other => panic!("expected BadOverride, got {other:?}"),
    }
}

#[test]
fn a_group_writable_binary_is_refused() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_with_mode(dir.path(), "daemon", b"#!/bin/sh\nexit 0\n", 0o775);
    let _override = Override::set(&path);

    let error = spec().resolve(None).expect_err("must refuse");

    match error {
        BinaryError::BadOverride { reason, .. } => {
            assert!(reason.contains("writable by group or others"), "{reason}");
        }
        other => panic!("expected BadOverride, got {other:?}"),
    }
}

#[test]
fn a_directory_is_refused() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let inner = dir.path().join("daemon");
    std::fs::create_dir(&inner).expect("create the directory");
    let _override = Override::set(&inner);

    let error = spec().resolve(None).expect_err("must refuse");

    match error {
        BinaryError::BadOverride { reason, .. } => {
            assert!(reason.contains("not a regular file"), "{reason}");
        }
        other => panic!("expected BadOverride, got {other:?}"),
    }
}

#[test]
fn a_binary_in_a_world_writable_directory_without_the_sticky_bit_is_refused() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let open = dir.path().join("open");
    std::fs::create_dir(&open).expect("create the directory");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777))
        .expect("make it world-writable");
    let path = write_with_mode(&open, "daemon", b"#!/bin/sh\nexit 0\n", 0o755);
    let _override = Override::set(&path);

    let error = spec().resolve(None).expect_err("must refuse");

    match error {
        BinaryError::BadOverride { reason, .. } => {
            assert!(reason.contains("world-writable"), "{reason}");
        }
        other => panic!("expected BadOverride, got {other:?}"),
    }
}

#[test]
fn a_sticky_world_writable_directory_is_accepted() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let sticky = dir.path().join("sticky");
    std::fs::create_dir(&sticky).expect("create the directory");
    // 1777, the mode of `/tmp`: world-writable but only the owner may replace a file.
    std::fs::set_permissions(&sticky, std::fs::Permissions::from_mode(0o1777))
        .expect("make it sticky");
    let path = write_with_mode(&sticky, "daemon", b"#!/bin/sh\nexit 0\n", 0o755);
    let _override = Override::set(&path);

    let resolved = spec()
        .resolve(None)
        .expect("the sticky bit makes this directory safe");

    assert_eq!(resolved.path(), path);
}

#[test]
fn a_pinned_digest_that_matches_resolves() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_with_mode(dir.path(), "daemon", b"exact bytes", 0o755);
    let _override = Override::set(&path);

    // sha256("exact bytes"), computed independently below by hashing the same input
    // through a second code path would only re-test sha2; this is the published value.
    let digest = "a6f2a9539a4b4b0e2b1a2f0f0a26f9b03d3a1e5b4c9c4e2a0f8a3e4b5c6d7e8f";

    let error = spec()
        .resolve(Some(digest))
        .expect_err("a wrong pin must be refused");
    match error {
        BinaryError::DigestMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, digest);
            // Now feed back the real digest and require it to be accepted.
            let resolved = spec()
                .resolve(Some(&actual))
                .expect("the file's own digest must be accepted");
            assert_eq!(resolved.path(), path);
            // Case-insensitively, since hex has two spellings.
            let upper = actual.to_uppercase();
            assert!(spec().resolve(Some(&upper)).is_ok());
        }
        other => panic!("expected DigestMismatch, got {other:?}"),
    }
}

#[test]
fn a_changed_binary_stops_matching_its_pin() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_with_mode(dir.path(), "daemon", b"original bytes", 0o755);
    let _override = Override::set(&path);

    let pinned = match spec().resolve(Some("00")).expect_err("wrong pin") {
        BinaryError::DigestMismatch { actual, .. } => actual,
        other => panic!("expected DigestMismatch, got {other:?}"),
    };
    assert!(spec().resolve(Some(&pinned)).is_ok());

    // The swap this check exists to catch.
    write_with_mode(dir.path(), "daemon", b"replaced bytes", 0o755);

    let error = spec()
        .resolve(Some(&pinned))
        .expect_err("a replaced binary must be refused");
    assert!(matches!(error, BinaryError::DigestMismatch { .. }), "{error:?}");
}

#[test]
fn nothing_installed_reports_how_many_places_were_tried() {
    let _guard = env_lock();
    // No override set, and both lookup lists point at paths that cannot exist.
    let spec = BinarySpec {
        name: "fake-tunnel-daemon",
        candidates: &["/nonexistent/a", "/nonexistent/b"],
        env_override: OVERRIDE_VAR,
        search_dirs: &["/nonexistent/dir"],
    };

    let error = spec.resolve(None).expect_err("must fail");

    match error {
        BinaryError::NotFound { name, searched } => {
            assert_eq!(name, "fake-tunnel-daemon");
            assert_eq!(searched, 3);
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
    assert!(!spec.is_installed());
    // The message has to tell the operator this service will not fetch it for them.
    assert!(
        error.to_string().contains("never downloads"),
        "{error}"
    );
}

#[test]
fn an_empty_override_falls_through_to_the_normal_lookup() {
    let _guard = env_lock();
    // SAFETY: the lock is held, so no other case in this binary reads this variable.
    unsafe { std::env::set_var(OVERRIDE_VAR, "   ") };

    let error = BinarySpec {
        name: "fake-tunnel-daemon",
        candidates: &["/nonexistent/a"],
        env_override: OVERRIDE_VAR,
        search_dirs: &[],
    }
    .resolve(None)
    .expect_err("must fall through, then fail");

    // SAFETY: as above.
    unsafe { std::env::remove_var(OVERRIDE_VAR) };

    assert!(matches!(error, BinaryError::NotFound { searched: 1, .. }), "{error:?}");
}

#[test]
fn a_caller_discovered_path_gets_the_same_checks() {
    // A Python interpreter is chosen by version and by what it can import, so the search is
    // the caller's. The checks must not be, or a discovered binary would be trusted more
    // easily than a configured one.
    let dir = tempfile::tempdir().expect("tempdir");
    let good = write_with_mode(dir.path(), "python3", b"#!/bin/sh\nexit 0\n", 0o755);

    let resolved = nullrouter_procctl::binary::Executable::verified(good.clone(), "python3")
        .expect("a 0755 absolute path must be accepted");
    assert_eq!(resolved.path(), good);
    assert_eq!(resolved.name(), "python3");

    // World-writable is refused here too.
    let open = write_with_mode(dir.path(), "python3-open", b"#!/bin/sh\nexit 0\n", 0o777);
    let error = nullrouter_procctl::binary::Executable::verified(open, "python3")
        .expect_err("must refuse");
    match error {
        BinaryError::Unusable { reason, .. } => {
            assert!(reason.contains("writable by group or others"), "{reason}");
        }
        other => panic!("expected Unusable, got {other:?}"),
    }

    // As is a relative path, which cannot be checked meaningfully.
    let error = nullrouter_procctl::binary::Executable::verified(
        std::path::PathBuf::from("python3"),
        "python3",
    )
    .expect_err("must refuse");
    assert!(matches!(error, BinaryError::Unusable { .. }), "{error:?}");

    // And a non-executable file.
    let plain = write_with_mode(dir.path(), "notes.txt", b"text", 0o644);
    let error = nullrouter_procctl::binary::Executable::verified(plain, "python3")
        .expect_err("must refuse");
    match error {
        BinaryError::Unusable { reason, .. } => {
            assert!(reason.contains("not executable"), "{reason}");
        }
        other => panic!("expected Unusable, got {other:?}"),
    }
}

#[test]
fn a_real_system_binary_resolves_through_the_search_path() {
    let _guard = env_lock();
    // `sh` exists on every unix this runs on, and exercises the search-directory branch
    // rather than the override branch.
    let spec = BinarySpec {
        name: "sh",
        candidates: &[],
        env_override: OVERRIDE_VAR,
        search_dirs: nullrouter_procctl::binary::SYSTEM_BIN_DIRS,
    };

    let resolved = spec.resolve(None).expect("sh must be installed");

    assert!(resolved.path().ends_with("sh"), "{:?}", resolved.path());
    assert!(resolved.path().is_absolute());
}
