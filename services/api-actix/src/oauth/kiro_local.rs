//! Reading a Kiro refresh token out of this machine's AWS SSO cache.
//!
//! `GET /api/oauth/kiro/auto-import` finds what a Kiro IDE login left in `~/.aws/sso/cache` and hands it
//! to the panel, which then submits it to `kiro/import`. Pure file reading — no subprocess and no
//! network call — so unlike the Cursor counterpart there is nothing here to sandbox, only paths to be
//! careful about.
//!
//! # The two files
//!
//! A token file holds the refresh token. For an organisation (IDC) login it also carries a
//! `clientIdHash` naming a second file in the same directory, which holds the client credentials that
//! the refresh cannot happen without. Finding the first and missing the second yields a token that
//! looks importable and cannot be renewed, so both are resolved here.
//!
//! # What is deliberately different from upstream
//!
//! Upstream reads every `.json` file in the cache directory and returns the first with a matching
//! token. That directory belongs to the AWS CLI, not to Kiro: it also holds SSO sessions for whatever
//! else the user has logged into. This module keeps the same search — the marker prefix is what makes
//! a file Kiro's, and there is no other way to identify one — but bounds it: files are read in a
//! defined order, a file larger than a plausible token document is skipped rather than slurped, and
//! only the fields needed are ever read out of a matching document.
//!
//! `profileArn` is normalised to `us-east-1` regardless of the login's own region, because Kiro's
//! runtime gateway requires that region in the ARN. Upstream does the same and its comment says why;
//! this is one of those cases where the surprising behaviour is the correct one.

use std::path::{Path, PathBuf};

use actix_web::{HttpResponse, http::StatusCode};

use crate::responses;

/// Overrides the home directory the cache is looked for under, so the search can be tested.
const HOME_OVERRIDE_VAR: &str = "NULLROUTER_KIRO_HOME";

/// The prefix every Kiro refresh token carries.
///
/// This is what distinguishes Kiro's entry from the other SSO sessions in the same directory. It is
/// upstream's marker, and it is load-bearing: without it there is no way to tell which file belongs to
/// Kiro, and importing the wrong one would attach somebody's unrelated AWS session as a Kiro provider.
const TOKEN_MARKER: &str = "aorAAAAAG";

/// The file Kiro writes, tried before the directory sweep.
const KIRO_TOKEN_FILE: &str = "kiro-auth-token.json";

/// A token document is a few kilobytes. Anything larger is not one, and reading it would be pointless
/// work on a directory this service does not own.
const MAX_DOCUMENT: u64 = 256 * 1024;

/// Where the AWS CLI keeps SSO tokens.
fn cache_dir(home: &Path) -> PathBuf {
    home.join(".aws/sso/cache")
}

/// The home directory to search under.
fn home() -> Option<PathBuf> {
    if let Some(overridden) = std::env::var_os(HOME_OVERRIDE_VAR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Some(overridden);
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Read and parse one cache file, if it is small enough to be a token document.
fn read_document(path: &Path) -> Option<serde_json::Value> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DOCUMENT {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether a document holds a Kiro refresh token, and what it is.
fn kiro_token(document: &serde_json::Value) -> Option<&str> {
    document
        .get("refreshToken")
        .and_then(serde_json::Value::as_str)
        .filter(|token| token.starts_with(TOKEN_MARKER))
}

/// The cache files to examine, Kiro's own name first and the rest in a defined order.
///
/// Sorted rather than left in directory order: an unordered read means which credential gets imported
/// depends on filesystem iteration, which is neither reproducible nor explainable to a user looking at
/// the result.
fn candidate_files(cache: &Path) -> Vec<PathBuf> {
    let mut rest: Vec<PathBuf> = std::fs::read_dir(cache)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path.file_name().is_some_and(|name| name != KIRO_TOKEN_FILE)
        })
        .collect();
    rest.sort();

    let mut files = vec![cache.join(KIRO_TOKEN_FILE)];
    files.extend(rest);
    files
}

/// What the cache held.
#[derive(Debug, Default)]
struct Found {
    refresh_token: String,
    source: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    region: Option<String>,
    auth_method: Option<String>,
}

/// The client credentials an IDC token needs, from the file its document points at.
///
/// The hash names a sibling file, so it is used as a *file name* and nothing else: any value carrying a
/// path separator is refused rather than joined, since this reads a directory the service does not own
/// and a `../` in that field would otherwise escape it.
fn client_credentials(
    cache: &Path,
    document: &serde_json::Value,
) -> (Option<String>, Option<String>) {
    let Some(hash) = document
        .get("clientIdHash")
        .and_then(serde_json::Value::as_str)
        .filter(|hash| {
            !hash.is_empty()
                && hash
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
    else {
        return (None, None);
    };
    let Some(registration) = read_document(&cache.join(format!("{hash}.json"))) else {
        return (None, None);
    };
    let field = |name: &str| {
        registration
            .get(name)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    match (field("clientId"), field("clientSecret")) {
        // Both or neither: `kiro/import` refuses a half pair, so returning one would produce a payload
        // that route rejects.
        (Some(id), Some(secret)) => (Some(id), Some(secret)),
        _incomplete => (None, None),
    }
}

/// Kiro IDE's own profile file, which holds the ARN.
fn profile_arn(home: &Path) -> Option<String> {
    let mut paths = Vec::new();
    if let Some(app_data) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        paths.push(
            PathBuf::from(app_data).join("Kiro/User/globalStorage/kiro.kiroagent/profile.json"),
        );
    }
    paths.push(home.join("AppData/Roaming/Kiro/User/globalStorage/kiro.kiroagent/profile.json"));
    paths.push(home.join(".config/Kiro/User/globalStorage/kiro.kiroagent/profile.json"));

    paths.iter().find_map(|path| {
        read_document(path)?
            .get("arn")
            .and_then(serde_json::Value::as_str)
            .filter(|arn| !arn.is_empty())
            .map(normalise_arn)
    })
}

/// Rewrite a `codewhisperer` ARN's region to `us-east-1`.
///
/// Kiro's runtime gateway requires `us-east-1` in the ARN whatever region the login used, so an ARN
/// passed through unchanged fails at request time with an error that points at the wrong thing. Only
/// the region field of a `codewhisperer` ARN is touched; anything else is returned as found.
fn normalise_arn(arn: &str) -> String {
    const PREFIX: &str = "arn:aws:codewhisperer:";
    let Some(rest) = arn.strip_prefix(PREFIX) else {
        return arn.to_owned();
    };
    match rest.split_once(':') {
        Some((_region, tail)) => format!("{PREFIX}us-east-1:{tail}"),
        None => arn.to_owned(),
    }
}

/// Search the cache for a Kiro token.
fn search(cache: &Path) -> Option<Found> {
    candidate_files(cache).into_iter().find_map(|path| {
        let document = read_document(&path)?;
        let refresh_token = kiro_token(&document)?.to_owned();
        let (client_id, client_secret) = client_credentials(cache, &document);
        let text = |name: &str| {
            document
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        Some(Found {
            refresh_token,
            source: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            client_id,
            client_secret,
            region: text("region"),
            auth_method: text("authMethod"),
        })
    })
}

/// A `found: false` answer. 200, as upstream: this is a question, and "no" answers it.
fn not_found(reason: &str) -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &serde_json::json!({ "found": false, "error": reason }),
    )
}

/// `GET /api/oauth/kiro/auto-import`.
///
/// Host-only at the gateway. Its success answer is a refresh token read off local disk — the longest-
/// lived half of a Kiro credential — so a session cookie stolen from a browser elsewhere must not be
/// able to ask this host for it.
pub(super) async fn kiro_auto_import() -> HttpResponse {
    let Some(home) = home() else {
        return not_found(
            "No home directory is set for this service, so there is no AWS SSO cache to read.",
        );
    };
    let cache = cache_dir(&home);
    if !cache.is_dir() {
        return not_found("AWS SSO cache not found. Please login to Kiro IDE first.");
    }
    let Some(found) = search(&cache) else {
        return not_found("Kiro token not found in AWS SSO cache. Please login to Kiro IDE first.");
    };

    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "found": true,
            "refreshToken": found.refresh_token,
            "source": found.source,
            // Present-and-null rather than absent: the panel decides which refresh protocol to declare
            // from whether these are here, and a missing key reads the same as a null one only if every
            // consumer is careful.
            "clientId": found.client_id,
            "clientSecret": found.client_secret,
            "region": found.region,
            "authMethod": found.auth_method,
            "profileArn": profile_arn(&home),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{TOKEN_MARKER, kiro_token, normalise_arn};

    #[test]
    fn an_arn_region_is_rewritten_to_us_east_1() {
        // Kiro's runtime gateway requires us-east-1 in the ARN whatever region the login used. An ARN
        // passed through unchanged fails at request time, pointing at the wrong cause.
        assert_eq!(
            normalise_arn("arn:aws:codewhisperer:eu-west-1:123456789012:profile/ABCDEF"),
            "arn:aws:codewhisperer:us-east-1:123456789012:profile/ABCDEF"
        );
        // Already correct: unchanged.
        assert_eq!(
            normalise_arn("arn:aws:codewhisperer:us-east-1:1:profile/X"),
            "arn:aws:codewhisperer:us-east-1:1:profile/X"
        );
    }

    #[test]
    fn a_non_codewhisperer_arn_is_left_alone() {
        // Only the one service's ARNs carry this requirement. Rewriting another service's region would
        // corrupt a value this module does not understand.
        let other = "arn:aws:iam::123456789012:role/Example";
        assert_eq!(normalise_arn(other), other);
        assert_eq!(normalise_arn("not-an-arn"), "not-an-arn");
    }

    #[test]
    fn only_a_marked_token_is_kiros() {
        // The cache holds SSO sessions for everything else the user has logged into. Without the marker
        // check, an unrelated AWS session would be imported as a Kiro provider.
        let kiro = serde_json::json!({ "refreshToken": format!("{TOKEN_MARKER}abc") });
        assert_eq!(
            kiro_token(&kiro),
            Some(format!("{TOKEN_MARKER}abc").as_str())
        );

        for foreign in [
            serde_json::json!({ "refreshToken": "some-other-sso-session" }),
            serde_json::json!({ "refreshToken": "" }),
            serde_json::json!({ "accessToken": "no refresh token at all" }),
        ] {
            assert_eq!(kiro_token(&foreign), None, "{foreign}");
        }
    }
}
