//! Cursor's request headers, including the `x-cursor-checksum` it will not answer without.
//!
//! Ports `open-sse/utils/cursorChecksum.js`. The checksum reads like a signature and is not one: it is a
//! coarse timestamp put through a rolling XOR and base64url-encoded, with the machine id appended in the
//! clear. No secret enters it, so it authenticates nothing — the bearer token does that. Porting it is
//! necessary only because the endpoint requires the header to be present and well-formed.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

/// Cursor's own base64 alphabet for the checksum: URL-safe, unpadded.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The rolling XOR key the cipher starts from.
const CIPHER_SEED: u8 = 165;

/// The IDE build this port identifies as.
const CLIENT_VERSION: &str = "3.12.17";

/// The commit that build was cut from. Cursor checks it against the version.
const CLIENT_COMMIT: &str = "0fb762053c34788bb7760d5673f8a6d4c8589d50";

/// Strip the `user_…::` prefix Cursor's stored tokens carry.
///
/// Every derived value — session id, client key, machine id — is computed from the bare token, so a
/// prefixed one produces different values throughout and the request is rejected as inconsistent.
pub(crate) fn clean_token(token: &str) -> &str {
    token.split_once("::").map_or(token, |(_prefix, rest)| rest)
}

/// SHA-256 of `input + salt`, hex-encoded.
fn hashed_hex(input: &str, salt: &str) -> String {
    let digest = Sha256::digest(format!("{input}{salt}").as_bytes());
    digest
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The machine id derived from a token, for a connection that has none stored.
pub(crate) fn derived_machine_id(token: &str) -> String {
    hashed_hex(clean_token(token), "machineId")
}

/// The `x-client-key` header: SHA-256 of the bare token.
pub(crate) fn client_key(token: &str) -> String {
    hashed_hex(clean_token(token), "")
}

/// The `x-session-id`: a UUID v5 of the token in the DNS namespace.
///
/// v5 is SHA-1 based and defined by RFC 4122, so it is computed here rather than pulled in as a
/// dependency for one value. The DNS namespace is the fixed uuid `6ba7b810-9dad-11d1-80b4-00c04fd430c8`.
pub(crate) fn session_id(token: &str) -> String {
    const DNS_NAMESPACE: [u8; 16] = [
        0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30,
        0xc8,
    ];
    let mut hasher = sha1::Sha1::new();
    sha1::Digest::update(&mut hasher, DNS_NAMESPACE);
    sha1::Digest::update(&mut hasher, clean_token(token).as_bytes());
    let digest = sha1::Digest::finalize(hasher);

    let mut bytes = [0_u8; 16];
    for (target, source) in bytes.iter_mut().zip(digest.iter()) {
        *target = *source;
    }
    // Version 5, variant 1.
    if let Some(byte) = bytes.get_mut(6) {
        *byte = (*byte & 0x0F) | 0x50;
    }
    if let Some(byte) = bytes.get_mut(8) {
        *byte = (*byte & 0x3F) | 0x80;
    }
    hyphenate(&bytes)
}

/// The `x-cursor-checksum`: an obfuscated timestamp with the machine id appended.
///
/// The timestamp is `millis / 1_000_000`, so it advances roughly every seventeen minutes — it identifies
/// an era, not a request, and two requests minutes apart normally carry the same one.
///
/// One quirk is reproduced rather than corrected. Upstream computes the six bytes with JavaScript shift
/// operators, which coerce to a signed 32-bit integer, so `>> 40` and `>> 32` do not shift by 40 and 32 —
/// they shift by 8 and 0 against the low word. The result is that the first two bytes repeat the third and
/// sixth. Cursor accepts what its own IDE sends, and the IDE has the same coercion, so computing the
/// mathematically correct bytes here would produce a checksum that differs from every real client's.
pub(crate) fn checksum(machine_id: &str, millis: u128) -> String {
    let timestamp = millis / 1_000_000;
    // Truncating to 32 bits first is the coercion described above, not an accident. The wrap is the
    // behaviour being reproduced, so it is written as one rather than as a lossy cast.
    let word = u32::try_from(timestamp & 0xFFFF_FFFF).unwrap_or(u32::MAX);
    // `word >> 40` and `word >> 32` in JavaScript shift by 8 and 0: the shift count is taken mod 32, so
    // the first two bytes repeat the fifth and sixth.
    let byte_at = |shift: u32| -> u8 { u8::try_from((word >> shift) & 0xFF).unwrap_or(0) };
    let mut bytes: [u8; 6] = [
        byte_at(8),
        byte_at(0),
        byte_at(24),
        byte_at(16),
        byte_at(8),
        byte_at(0),
    ];

    let mut key = CIPHER_SEED;
    for (index, byte) in bytes.iter_mut().enumerate() {
        // Upstream's `i % 256`. Six bytes never reach it, but the wrap is what it specifies.
        let offset = u8::try_from(index % 256).unwrap_or(0);
        *byte = (*byte ^ key).wrapping_add(offset);
        key = *byte;
    }

    format!("{}{machine_id}", base64url(&bytes))
}

/// Cursor's own unpadded base64url encoder.
///
/// Written out rather than taken from a crate because the padding rule differs: a trailing group emits
/// only the characters its bytes justify, with no `=`.
fn base64url(bytes: &[u8]) -> String {
    let symbol = |index: u8| char::from(ALPHABET.get(usize::from(index)).copied().unwrap_or(b'A'));
    let mut out = String::with_capacity(bytes.len().saturating_mul(4).saturating_div(3));
    for chunk in bytes.chunks(3) {
        let first = chunk.first().copied().unwrap_or(0);
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        out.push(symbol(first >> 2));
        out.push(symbol(((first & 0x03) << 4) | (second >> 4)));
        if chunk.len() > 1 {
            out.push(symbol(((second & 0x0F) << 2) | (third >> 6)));
        }
        if chunk.len() > 2 {
            out.push(symbol(third & 0x3F));
        }
    }
    out
}

/// Format sixteen bytes as a hyphenated uuid.
fn hyphenate(bytes: &[u8; 16]) -> String {
    let hex = bytes
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    [
        hex.get(..8),
        hex.get(8..12),
        hex.get(12..16),
        hex.get(16..20),
        hex.get(20..32),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("-")
}

/// The platform triple Cursor expects, as this build reports it.
///
/// Reported honestly rather than pinned to one value: Cursor uses it to pick platform-specific behaviour,
/// and claiming macOS from a Linux host asks for the wrong one.
const fn platform() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    };
    (os, arch)
}

/// What one Cursor request's headers are built from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Request<'a> {
    /// The access token, with or without its `user_…::` prefix.
    pub(crate) token: &'a str,
    /// The device identity, stored on the connection or derived from the token.
    pub(crate) machine_id: &'a str,
    /// Cursor's privacy switch. **On** unless a connection explicitly turns it off: with it off Cursor
    /// may retain the conversation for training, and a router relaying someone else's prompts is the
    /// wrong place to opt them into that. Upstream defaults it on too (`!== false`).
    pub(crate) ghost_mode: bool,
    /// The per-request identifiers.
    pub(crate) nonces: &'a super::Nonces,
    /// Milliseconds since the epoch, for the checksum's coarse timestamp.
    pub(crate) millis: u128,
}

/// Every header a Cursor request carries.
pub(crate) fn build(request: &Request<'_>) -> Vec<(String, String)> {
    let &Request {
        token,
        machine_id,
        ghost_mode,
        nonces,
        millis,
    } = request;
    let (request_id, config_version, trace_id) =
        (&nonces.request_id, &nonces.config_version, &nonces.trace_id);
    let bare = clean_token(token);
    let (os, arch) = platform();
    vec![
        ("authorization".to_owned(), format!("Bearer {bare}")),
        ("connect-accept-encoding".to_owned(), "gzip".to_owned()),
        ("connect-protocol-version".to_owned(), "1".to_owned()),
        (
            "content-type".to_owned(),
            "application/connect+proto".to_owned(),
        ),
        ("user-agent".to_owned(), "connect-es/1.6.1".to_owned()),
        ("x-amzn-trace-id".to_owned(), format!("Root={trace_id}")),
        ("x-client-key".to_owned(), client_key(bare)),
        ("x-cursor-checksum".to_owned(), checksum(machine_id, millis)),
        (
            "x-cursor-client-version".to_owned(),
            CLIENT_VERSION.to_owned(),
        ),
        (
            "x-cursor-client-commit".to_owned(),
            CLIENT_COMMIT.to_owned(),
        ),
        ("x-cursor-client-type".to_owned(), "ide".to_owned()),
        ("x-cursor-client-os".to_owned(), os.to_owned()),
        ("x-cursor-client-arch".to_owned(), arch.to_owned()),
        (
            "x-cursor-client-device-type".to_owned(),
            "desktop".to_owned(),
        ),
        (
            "x-cursor-config-version".to_owned(),
            config_version.to_owned(),
        ),
        // Fixed rather than read from the host clock's zone. The timezone is a fingerprint, and a router
        // has no business reporting the operator's location on a user's behalf.
        ("x-cursor-timezone".to_owned(), "UTC".to_owned()),
        (
            "x-ghost-mode".to_owned(),
            if ghost_mode { "true" } else { "false" }.to_owned(),
        ),
        ("x-request-id".to_owned(), request_id.to_owned()),
        ("x-session-id".to_owned(), session_id(bare)),
    ]
}

#[cfg(test)]
mod tests {
    use super::{base64url, checksum, clean_token, client_key, derived_machine_id, session_id};

    #[test]
    fn a_prefixed_token_is_reduced_to_the_bare_one() {
        // Every derived value is computed from the bare token, so a prefix left on produces a request that
        // is inconsistent with itself.
        assert_eq!(clean_token("user_01ABC::eyJhbGci"), "eyJhbGci");
        assert_eq!(clean_token("eyJhbGci"), "eyJhbGci");
    }

    #[test]
    fn derived_values_are_deterministic_and_distinct() {
        let token = "eyJhbGciOiJIUzI1NiJ9.payload.sig";
        assert_eq!(client_key(token), client_key(token));
        assert_eq!(client_key(token).len(), 64);
        assert_eq!(derived_machine_id(token).len(), 64);
        // Salted differently, so the machine id is not the client key.
        assert_ne!(client_key(token), derived_machine_id(token));
        // And a prefixed token derives the same values as its bare form.
        assert_eq!(
            client_key(&format!("user_01ABC::{token}")),
            client_key(token)
        );
    }

    #[test]
    fn the_session_id_is_a_v5_uuid_of_the_token() {
        let id = session_id("some-token");
        let fields: Vec<&str> = id.split('-').collect();
        assert_eq!(
            fields.iter().map(|field| field.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "got {id}"
        );
        // Version nibble is 5.
        assert!(
            fields.get(2).is_some_and(|field| field.starts_with('5')),
            "got {id}"
        );
        // Variant nibble.
        assert!(
            fields.get(3).is_some_and(|field| matches!(
                field.as_bytes().first(),
                Some(b'8' | b'9' | b'a' | b'b')
            )),
            "got {id}"
        );
        // Deterministic: the same token is the same session.
        assert_eq!(id, session_id("some-token"));
        assert_ne!(id, session_id("another-token"));
    }

    #[test]
    fn the_session_id_matches_the_published_v5_vector() {
        // RFC 4122's DNS namespace with the name "python.org" is a widely published v5 vector. Asserting
        // it pins the namespace bytes and the version/variant patching, which a self-consistent test
        // would not catch.
        assert_eq!(
            session_id("python.org"),
            "886313e1-3b8a-5372-9b90-0c9aee199e5d"
        );
    }

    #[test]
    fn the_checksum_ends_with_the_machine_id_in_the_clear() {
        // No secret enters the checksum. It authenticates nothing; the bearer token does that.
        let machine = "a".repeat(64);
        let value = checksum(&machine, 1_700_000_000_000);
        assert!(value.ends_with(&machine), "got {value}");
        // Six cipher bytes encode to eight base64 characters, unpadded.
        assert_eq!(value.len(), 8 + machine.len());
        assert!(!value.contains('='), "the alphabet is unpadded: {value}");
    }

    #[test]
    fn the_checksum_identifies_an_era_rather_than_a_request() {
        // The timestamp is divided by a million, so it advances about every seventeen minutes. Two
        // requests a minute apart normally carry the same checksum, and that is expected rather than a bug.
        let machine = "m".repeat(64);
        let base = 1_700_000_000_000_u128;
        assert_eq!(
            checksum(&machine, base),
            checksum(&machine, base + 60_000),
            "a minute apart must not change it"
        );
        assert_ne!(
            checksum(&machine, base),
            checksum(&machine, base + 2_000_000),
            "crossing an era must change it"
        );
    }

    #[test]
    fn the_checksum_reproduces_javascripts_int32_coercion() {
        // Upstream builds the six bytes with JS shift operators, whose shift count is taken mod 32, so
        // `>> 40` shifts by 8 and `>> 32` by 0. Byte 0 therefore equals byte 4, and byte 1 equals byte 5.
        // Computing the mathematically correct bytes instead would produce a checksum unlike every real
        // client's, so the quirk is reproduced deliberately.
        //
        // Reversing the cipher: b[0] = (t0 ^ 165) + 0, and t0 == t4, t1 == t5.
        let machine = "x";
        let value = checksum(machine, 1_700_000_000_000);
        let encoded = value.strip_suffix(machine).expect("the machine id suffix");
        assert_eq!(encoded.len(), 8);

        // Recompute the expected bytes independently, mod-32 shifts included.
        let word = u32::try_from(1_700_000_000_000_u128 / 1_000_000).expect("an era fits a word");
        let byte_at = |shift: u32| u8::try_from((word >> shift) & 0xFF).expect("a masked byte");
        let mut expected: [u8; 6] = [
            byte_at(8),
            byte_at(0),
            byte_at(24),
            byte_at(16),
            byte_at(8),
            byte_at(0),
        ];
        assert_eq!(
            expected.first(),
            expected.get(4),
            "the coercion makes these equal"
        );
        let mut key = 165_u8;
        for (index, byte) in expected.iter_mut().enumerate() {
            *byte = (*byte ^ key).wrapping_add(u8::try_from(index % 256).expect("a byte"));
            key = *byte;
        }
        assert_eq!(encoded, base64url(&expected));
    }

    #[test]
    fn the_base64_alphabet_is_url_safe() {
        // `+` and `/` would be rejected; the alphabet's last two symbols are `-` and `_`.
        let encoded = base64url(&[0xFF, 0xFF, 0xFF]);
        assert_eq!(encoded, "____");
        assert_eq!(base64url(&[0xFB, 0xFF, 0xBF]), "-_-_");
        // A trailing group emits only what its bytes justify.
        assert_eq!(base64url(&[0x00]).len(), 2);
        assert_eq!(base64url(&[0x00, 0x00]).len(), 3);
        assert_eq!(base64url(&[0x00, 0x00, 0x00]).len(), 4);
    }

    #[test]
    fn ghost_mode_and_the_fixed_timezone_are_both_deliberate() {
        let nonces = super::super::Nonces {
            request_id: "req-1".to_owned(),
            config_version: "cfg-1".to_owned(),
            trace_id: "trace-1".to_owned(),
        };
        let build = |ghost_mode: bool| {
            super::build(&super::Request {
                token: "tok",
                machine_id: "machine",
                ghost_mode,
                nonces: &nonces,
                millis: 0,
            })
        };
        let headers = build(true);
        let read = |name: &str| {
            headers
                .iter()
                .find(|(key, _value)| key == name)
                .map(|(_key, value)| value.clone())
        };
        // Ghost mode on: with it off, Cursor may retain the conversation. A router relaying someone
        // else's prompts must not opt them into that.
        assert_eq!(read("x-ghost-mode").as_deref(), Some("true"));
        // The timezone is a fingerprint, so it is fixed rather than read from the host.
        assert_eq!(read("x-cursor-timezone").as_deref(), Some("UTC"));
        assert_eq!(
            read("content-type").as_deref(),
            Some("application/connect+proto")
        );
        assert_eq!(read("authorization").as_deref(), Some("Bearer tok"));
        assert_eq!(read("x-amzn-trace-id").as_deref(), Some("Root=trace-1"));

        let off = build(false);
        assert!(
            off.iter()
                .any(|(key, value)| key == "x-ghost-mode" && value == "false")
        );
    }
}
