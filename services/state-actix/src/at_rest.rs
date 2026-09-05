//! Encryption of the state file at rest.
//!
//! The file holds provider API keys and OAuth refresh tokens. Mode `0o600` keeps other local
//! accounts out, but it does nothing once the bytes leave this host: a filesystem backup, a VM
//! snapshot, a support bundle, or a stolen disk all carry the credentials in the clear. That is the
//! exposure this module closes.
//!
//! **Off unless `NULLROUTER_STATE_KEY` is set.** An operator who has not supplied a key has not
//! agreed to hold one, and encrypting without their involvement would mean either inventing a key
//! they cannot reproduce — losing their state on the next restart — or deriving one from something
//! on the same disk, which protects nothing.
//!
//! The environment variable *is* the integration point. KMS, sealed-secrets, Vault Agent, a systemd
//! credential, or `docker run --env-file` all reduce to populating it, so this needs no plugin
//! interface to work with any of them.
//!
//! Whether to decrypt is decided by the file's own magic prefix, not by whether a key is set. That
//! is what makes adopting this a non-event: an existing cleartext deployment sets a key, and the next
//! save is sealed, with no migration step and no flag day.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroize as _;

/// Marks a sealed file, and pins the format so a future change is distinguishable rather than a
/// silent misparse.
const MAGIC: &[u8] = b"NRSTATE1";

/// Domain separation for the KDF, so the same operator key used elsewhere derives a different
/// subkey here.
const HKDF_INFO: &[u8] = b"nullrouter state file v1";

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// The variable holding the operator's key material.
pub(crate) const KEY_VAR: &str = "NULLROUTER_STATE_KEY";

#[derive(Debug, thiserror::Error)]
pub(crate) enum AtRestError {
    #[error(
        "the state file is encrypted but {KEY_VAR} is not set; set it to the same value used when \
         the file was written, or the stored credentials cannot be read"
    )]
    KeyMissing,
    #[error(
        "the state file could not be decrypted: {KEY_VAR} does not match the key it was sealed \
         with, or the file has been modified"
    )]
    Undecryptable,
    #[error("the state file is truncated: {0} bytes is too short to be a sealed file")]
    Truncated(usize),
    #[error("the state file could not be encrypted")]
    SealFailed,
}

/// A derived subkey that wipes itself on drop.
///
/// Wrapped rather than used as a bare array so it cannot outlive its scope in a core dump or a
/// swapped page.
struct DerivedKey([u8; KEY_LEN]);

impl Drop for DerivedKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// The operator's key material, if configured.
///
/// Empty and whitespace-only values read as unset: `NULLROUTER_STATE_KEY=` in a compose file is an
/// operator who has not set a key, and treating it as one would seal the file under a key nobody can
/// reproduce.
fn configured_key() -> Option<String> {
    std::env::var(KEY_VAR)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Stretch operator key material of any length into the 32 bytes the cipher needs.
///
/// HKDF rather than a plain hash so a short or low-entropy passphrase is at least domain-separated
/// and uniformly distributed. This is deliberately *not* a password-hashing KDF: there is no
/// per-file salt to store and no interactive login to slow down, so Argon2 would add cost without
/// adding resistance here. The security assumption is that the key comes from a secret manager and
/// is high-entropy — which is what every integration path above provides.
fn derive(material: &str) -> DerivedKey {
    let mut key = [0_u8; KEY_LEN];
    let kdf = hkdf::Hkdf::<sha2::Sha256>::new(None, material.as_bytes());
    // Only fails for absurd output lengths; 32 bytes is well inside the limit.
    if kdf.expand(HKDF_INFO, &mut key).is_err() {
        key = [0_u8; KEY_LEN];
    }
    DerivedKey(key)
}

/// Whether these bytes are a sealed state file.
pub(crate) fn is_sealed(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// Encrypt `plain` when a key is configured; pass it through unchanged otherwise.
pub(crate) fn seal(plain: &[u8]) -> Result<Vec<u8>, AtRestError> {
    let Some(material) = configured_key() else {
        return Ok(plain.to_vec());
    };
    let key = derive(&material);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key.0));

    // A fresh random nonce per write. Reuse under one key is what breaks this construction, and
    // saves happen on a timer, so a counter persisted alongside the file would be the thing most
    // likely to be restored from a backup and repeat itself.
    let mut nonce_bytes = [0_u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).map_err(|_| AtRestError::SealFailed)?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plain)
        .map_err(|_| AtRestError::SealFailed)?;

    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt `bytes` when they are sealed; pass them through when they are not.
///
/// Cleartext passes through so a deployment that has never set a key keeps working, and so setting a
/// key for the first time does not require converting the existing file by hand.
pub(crate) fn open(bytes: &[u8]) -> Result<Vec<u8>, AtRestError> {
    if !is_sealed(bytes) {
        return Ok(bytes.to_vec());
    }
    // Sealed but no key: refuse loudly. Starting with an empty state here would present an operator
    // with an empty dashboard and then overwrite their sealed file on the first save, destroying
    // every credential it held.
    let Some(material) = configured_key() else {
        return Err(AtRestError::KeyMissing);
    };

    let body = bytes.get(MAGIC.len()..).unwrap_or_default();
    let (nonce_bytes, ciphertext) = body
        .split_at_checked(NONCE_LEN)
        .ok_or(AtRestError::Truncated(bytes.len()))?;

    let key = derive(&material);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key.0));
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        // Poly1305 fails closed on both a wrong key and a modified file, and the two are not
        // distinguishable from here. Reported as one condition rather than guessing.
        .map_err(|_| AtRestError::Undecryptable)
}

#[cfg(test)]
mod tests {
    use super::{AtRestError, KEY_VAR, MAGIC, is_sealed, open, seal};

    /// Sets the key for one case and restores whatever was there.
    struct Key(Option<std::ffi::OsString>);

    impl Key {
        fn set(value: Option<&str>) -> Self {
            let saved = std::env::var_os(KEY_VAR);
            match value {
                // SAFETY: this module's cases run in one thread and restore the variable on drop.
                Some(key) => unsafe { std::env::set_var(KEY_VAR, key) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var(KEY_VAR) },
            }
            Self(saved)
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            match &self.0 {
                // SAFETY: as above.
                Some(previous) => unsafe { std::env::set_var(KEY_VAR, previous) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var(KEY_VAR) },
            }
        }
    }

    const SECRET: &[u8] = br#"{"apiKeys":[{"key":"sk-live-do-not-leak-this"}]}"#;

    #[test]
    fn a_sealed_file_does_not_contain_the_plaintext() {
        let _key = Key::set(Some("operator-supplied-key"));
        let sealed = seal(SECRET).expect("seal");

        // The assertion that matters: the credential must not appear in the bytes on disk.
        let haystack = String::from_utf8_lossy(&sealed);
        assert!(
            !haystack.contains("sk-live-do-not-leak-this"),
            "the sealed file leaked the credential"
        );
        assert!(is_sealed(&sealed));
        assert_eq!(open(&sealed).expect("open"), SECRET);
    }

    #[test]
    fn without_a_key_nothing_is_encrypted() {
        // An operator who set no key must keep working exactly as before.
        let _key = Key::set(None);
        let passed_through = seal(SECRET).expect("seal");
        assert_eq!(passed_through, SECRET);
        assert!(!is_sealed(&passed_through));
        assert_eq!(open(&passed_through).expect("open"), SECRET);
    }

    #[test]
    fn cleartext_still_loads_after_a_key_is_set() {
        // Adopting encryption must not require converting the existing file by hand: the decision to
        // decrypt is the file's magic prefix, not the presence of a key.
        let _key = Key::set(Some("newly-added-key"));
        assert_eq!(open(SECRET).expect("cleartext must pass through"), SECRET);
    }

    #[test]
    fn a_sealed_file_without_the_key_refuses_rather_than_reading_empty() {
        let sealed = {
            let _key = Key::set(Some("the-original-key"));
            seal(SECRET).expect("seal")
        };
        let _key = Key::set(None);
        // Refusing is the whole point. Returning an empty state would show an empty dashboard and
        // then overwrite the sealed file on the next save, destroying every credential in it.
        assert!(matches!(open(&sealed), Err(AtRestError::KeyMissing)));
    }

    #[test]
    fn the_wrong_key_is_refused() {
        let sealed = {
            let _key = Key::set(Some("the-right-key"));
            seal(SECRET).expect("seal")
        };
        let _key = Key::set(Some("a-different-key"));
        assert!(matches!(open(&sealed), Err(AtRestError::Undecryptable)));
    }

    #[test]
    fn tampering_is_detected() {
        let _key = Key::set(Some("operator-supplied-key"));
        let mut sealed = seal(SECRET).expect("seal");
        // Flip a bit in the ciphertext body. Poly1305 must reject it rather than return altered
        // plaintext -- otherwise someone with write access could edit stored credentials in place.
        let last = sealed.len() - 1;
        if let Some(byte) = sealed.get_mut(last) {
            *byte ^= 0x01;
        }
        assert!(matches!(open(&sealed), Err(AtRestError::Undecryptable)));
    }

    #[test]
    fn a_truncated_sealed_file_is_reported_as_truncated() {
        let _key = Key::set(Some("operator-supplied-key"));
        let mut sealed = MAGIC.to_vec();
        sealed.extend_from_slice(b"short");
        assert!(matches!(open(&sealed), Err(AtRestError::Truncated(_))));
    }

    #[test]
    fn two_writes_of_the_same_state_differ() {
        // A fresh nonce per write. Identical ciphertext for identical input would leak that nothing
        // changed between two backups, and nonce reuse is what breaks this construction outright.
        let _key = Key::set(Some("operator-supplied-key"));
        let first = seal(SECRET).expect("first");
        let second = seal(SECRET).expect("second");
        assert_ne!(first, second, "nonce appears to be reused across writes");
        assert_eq!(open(&first).expect("open first"), SECRET);
        assert_eq!(open(&second).expect("open second"), SECRET);
    }

    #[test]
    fn an_empty_key_variable_reads_as_unset() {
        // `NULLROUTER_STATE_KEY=` in a compose file is an operator who has not set a key. Treating
        // it as one would seal the file under a key nobody can reproduce.
        let _key = Key::set(Some("   "));
        let passed_through = seal(SECRET).expect("seal");
        assert_eq!(passed_through, SECRET, "whitespace must read as unset");
    }
}
