//! Request and response handling for `kiro`, Amazon `CodeWhisperer` as the Kiro IDE reaches it.
//!
//! Ports `open-sse/executors/kiro.js`. Like `cursor` this is not JSON on the wire in *both* directions: the
//! request body is JSON, but the response is `vnd.amazon.eventstream` — CRC-framed binary carrying JSON
//! payloads. So it takes the binary response path while sending an ordinary JSON body.
//!
//! Its complication is not the wire format but the **auth surface**. One provider has three endpoints that
//! are not interchangeable, and which one works depends on how the credential was obtained:
//!
//! * A Kiro OIDC or social token is what `runtime.*.kiro.dev` accepts, and that gateway rejects the others.
//! * An IAM Identity Center, enterprise (external `IdP`), or API-key credential is an AWS token. The
//!   kiro.dev gateway answers `403 bearer token invalid` for all three, so they must use the
//!   `*.amazonaws.com` surface — and in the region the token was minted in, since the registry's URLs are
//!   hardcoded to `us-east-1`.
//! * An API key specifically must reach `q.*` **first**. The older `codewhisperer.*` endpoint will
//!   authenticate the key and then reject the same valid body with `REQUEST_BODY_INVALID` — and a 400 is
//!   terminal, so trying it first means the working endpoint is never reached at all.
//!
//! That last one is the trap worth naming: the failure is a 400 about the *body*, from an endpoint that
//! accepted the *credential*, for a request that a different endpoint of the same service answers fine.

pub(crate) mod eventstream;
pub(crate) mod request;
pub(crate) mod response;

use serde_json::Value;

use crate::credentials::Credentials;

/// The `X-Amz-Target` the `CodeWhisperer` surface requires.
///
/// Sent only to that surface: the kiro.dev gateway does not expect it.
const CODEWHISPERER_TARGET: &str = "AmazonCodeWhispererStreamingService.GenerateAssistantResponse";

/// Auth methods whose credential is an AWS token rather than a Kiro one.
const AWS_SURFACE: [&str; 3] = ["api_key", "external_idp", "idc"];

/// The request body for a Kiro call.
pub(crate) fn body(body: &Value, credentials: &Credentials, ids: &Ids) -> Value {
    request::build(
        body,
        &request::Context {
            model: body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            auth_method: credentials.setting("authMethod"),
            profile_arn: credentials.setting("profileArn"),
            conversation_id: &ids.conversation_id,
            continuation_id: &ids.continuation_id,
            timestamp: &ids.timestamp,
        },
    )
}

/// Per-request identifiers, held so a test can pin them.
#[derive(Debug, Clone)]
pub(crate) struct Ids {
    /// This conversation's id, stable across a connection's turns.
    pub(crate) conversation_id: String,
    /// The agent continuation id, which ties a multi-turn agent run together.
    pub(crate) continuation_id: String,
    /// An ISO-8601 timestamp for the time context.
    pub(crate) timestamp: String,
}

impl Ids {
    /// Identifiers for one request.
    ///
    /// The conversation and continuation ids are derived from the connection rather than random, so a
    /// multi-turn conversation keeps one identity. Upstream resolves them from a session store; deriving
    /// them reaches the same place without one, and survives a restart.
    pub(crate) fn for_connection(connection_id: &str, millis: u128) -> Self {
        Self {
            conversation_id: super::session::derive(&format!("kiro:conversation:{connection_id}")),
            continuation_id: super::session::derive(&format!("kiro:continuation:{connection_id}")),
            timestamp: super::cursor::iso8601(millis),
        }
    }
}

/// Headers that depend on which of Kiro's three endpoints was chosen.
///
/// `X-Amz-Target` names an RPC method. The `CodeWhisperer` surface serves it; the kiro.dev gateway does not,
/// and naming a method it has no route for is a rejection.
pub(crate) fn url_headers(url: &str) -> Vec<(String, String)> {
    if url.contains("://codewhisperer.") {
        vec![("X-Amz-Target".to_owned(), CODEWHISPERER_TARGET.to_owned())]
    } else {
        Vec::new()
    }
}

/// Headers for a Kiro request, which depend on both the credential and the endpoint.
pub(crate) fn request_headers(
    credentials: &Credentials,
    url: &str,
    invocation_id: &str,
) -> Vec<(String, String)> {
    let mut headers = vec![
        // The AWS SDK's retry-state header. Upstream sends the same fixed value on every call.
        ("Amz-Sdk-Request".to_owned(), "attempt=1; max=3".to_owned()),
        ("Amz-Sdk-Invocation-Id".to_owned(), invocation_id.to_owned()),
    ];

    headers.extend(url_headers(url));

    let auth_method = credentials.setting("authMethod");
    let is_api_key = auth_method == Some("api_key");
    let api_key = credentials.api_key.as_deref().or_else(|| {
        is_api_key
            .then_some(credentials.access_token.as_deref())
            .flatten()
    });

    if let Some(key) = api_key.filter(|_| is_api_key) {
        // An API key travels as a bearer token like any other, and `TokenType` is what tells
        // CodeWhisperer to treat it as a long-lived key rather than an OIDC access token.
        headers.push(("Authorization".to_owned(), format!("Bearer {key}")));
        headers.push(("TokenType".to_owned(), "API_KEY".to_owned()));
    } else if let Some(token) = credentials.access_token.as_deref() {
        headers.push(("Authorization".to_owned(), format!("Bearer {token}")));
        // An enterprise token is an ordinary OAuth access token, but CodeWhisperer needs this to bind it
        // to a profile.
        if auth_method == Some("external_idp") {
            headers.push(("TokenType".to_owned(), "EXTERNAL_IDP".to_owned()));
        }
    }

    headers
}

/// Order this connection's endpoints, most likely to work first.
///
/// Returns `None` when the registry's own order stands, which is the case for a Kiro OIDC or social token.
pub(crate) fn ordered_urls(credentials: &Credentials, urls: &[String]) -> Option<Vec<String>> {
    let auth_method = credentials.setting("authMethod")?;
    if !AWS_SURFACE.contains(&auth_method) {
        return None;
    }
    let region = credentials
        .setting("region")
        .map(str::trim)
        .filter(|region| !region.is_empty())
        .unwrap_or("us-east-1");

    let regionalize = |url: &str| -> String {
        if region == "us-east-1" || !url.contains("amazonaws.com") {
            return url.to_owned();
        }
        // Rewrite the region in `<service>.<region>.amazonaws.com`. An AWS token is only valid in the
        // region it was minted in, and the registry's URLs are hardcoded to us-east-1.
        match url.split_once(".us-east-1.amazonaws.com") {
            Some((head, tail)) => format!("{head}.{region}.amazonaws.com{tail}"),
            None => url.to_owned(),
        }
    };

    let (aws, others): (Vec<String>, Vec<String>) = urls
        .iter()
        .map(|url| (url.contains("amazonaws.com"), url))
        .fold(
            (Vec::new(), Vec::new()),
            |(mut aws, mut others), (is_aws, url)| {
                if is_aws {
                    aws.push(regionalize(url));
                } else {
                    others.push(url.clone());
                }
                (aws, others)
            },
        );
    if aws.is_empty() {
        return None;
    }

    let mut ordered = Vec::with_capacity(urls.len());
    if auth_method == "api_key" {
        // `q.*` must come first. `codewhisperer.*` authenticates the key and then rejects the same valid
        // body with a 400, which is terminal — so trying it first means never reaching the one that works.
        ordered.extend(aws.iter().filter(|url| url.contains("://q.")).cloned());
        ordered.extend(aws.iter().filter(|url| !url.contains("://q.")).cloned());
    } else {
        ordered.extend(aws);
    }
    ordered.extend(others);
    Some(ordered)
}

/// Statuses that mean "try the next endpoint" rather than "this request failed".
///
/// A 401/403/404 from one surface says the credential does not belong to it, which the next surface may
/// answer fine. A 400 is deliberately **not** here: it is about the body, and sending the same body
/// elsewhere cannot repair it.
pub(crate) const fn is_endpoint_fallback_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 404)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Ids, is_endpoint_fallback_status, ordered_urls, request_headers};
    use crate::credentials::Credentials;

    fn credentials(auth_method: Option<&str>) -> Credentials {
        let mut credentials = Credentials {
            access_token: Some("aws-token".to_owned()),
            connection_id: "conn_kiro".to_owned(),
            ..Credentials::default()
        };
        if let Some(method) = auth_method {
            credentials
                .provider_specific_data
                .insert("authMethod".to_owned(), json!(method));
        }
        credentials
    }

    fn registry_urls() -> Vec<String> {
        vec![
            "https://runtime.us-east-1.kiro.dev/generateAssistantResponse".to_owned(),
            "https://codewhisperer.us-east-1.amazonaws.com/generateAssistantResponse".to_owned(),
            "https://q.us-east-1.amazonaws.com/generateAssistantResponse".to_owned(),
        ]
    }

    fn read(headers: &[(String, String)], name: &str) -> Option<String> {
        headers
            .iter()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value.clone())
    }

    #[test]
    fn the_amz_target_goes_only_to_the_codewhisperer_surface() {
        // It names an RPC method that the kiro.dev gateway does not serve.
        let credentials = credentials(None);
        let codewhisperer = request_headers(
            &credentials,
            "https://codewhisperer.us-east-1.amazonaws.com/generateAssistantResponse",
            "inv-1",
        );
        assert_eq!(
            read(&codewhisperer, "X-Amz-Target").as_deref(),
            Some("AmazonCodeWhispererStreamingService.GenerateAssistantResponse")
        );

        for other in [
            "https://runtime.us-east-1.kiro.dev/generateAssistantResponse",
            "https://q.us-east-1.amazonaws.com/generateAssistantResponse",
        ] {
            assert!(
                read(
                    &request_headers(&credentials, other, "inv-1"),
                    "X-Amz-Target"
                )
                .is_none(),
                "{other} must not receive the target header"
            );
        }
    }

    #[test]
    fn an_api_key_is_marked_as_one_so_it_is_not_read_as_an_oidc_token() {
        let mut api_key = credentials(Some("api_key"));
        api_key.api_key = Some("kiro-api-key".to_owned());
        let headers = request_headers(&api_key, "https://q.us-east-1.amazonaws.com/x", "inv-1");
        assert_eq!(
            read(&headers, "Authorization").as_deref(),
            Some("Bearer kiro-api-key")
        );
        assert_eq!(read(&headers, "TokenType").as_deref(), Some("API_KEY"));
    }

    #[test]
    fn an_api_key_stored_as_an_access_token_is_still_marked() {
        // Some imports put the key in the access-token field; without the marker CodeWhisperer reads it as
        // an OIDC token and refuses it.
        let headers = request_headers(
            &credentials(Some("api_key")),
            "https://q.us-east-1.amazonaws.com/x",
            "inv-1",
        );
        assert_eq!(
            read(&headers, "Authorization").as_deref(),
            Some("Bearer aws-token")
        );
        assert_eq!(read(&headers, "TokenType").as_deref(), Some("API_KEY"));
    }

    #[test]
    fn an_enterprise_token_is_bound_to_its_profile_and_others_are_not_marked() {
        let enterprise = request_headers(
            &credentials(Some("external_idp")),
            "https://codewhisperer.us-east-1.amazonaws.com/x",
            "inv-1",
        );
        assert_eq!(
            read(&enterprise, "TokenType").as_deref(),
            Some("EXTERNAL_IDP")
        );

        // A social or OIDC token carries no TokenType at all.
        for method in [None, Some("social"), Some("oauth"), Some("idc")] {
            let headers = request_headers(
                &credentials(method),
                "https://runtime.us-east-1.kiro.dev/x",
                "inv-1",
            );
            assert!(
                read(&headers, "TokenType").is_none(),
                "{method:?} should carry no TokenType"
            );
        }
    }

    #[test]
    fn an_api_key_reaches_the_q_surface_before_codewhisperer() {
        // `codewhisperer.*` authenticates the key and then rejects the same valid body with a terminal 400,
        // so putting it first means the working endpoint is never tried.
        let ordered = ordered_urls(&credentials(Some("api_key")), &registry_urls())
            .expect("an api-key connection is reordered");
        assert!(
            ordered.first().is_some_and(|url| url.contains("://q.")),
            "{ordered:?}"
        );
        assert!(
            ordered
                .get(1)
                .is_some_and(|url| url.contains("://codewhisperer.")),
            "{ordered:?}"
        );
        // The kiro.dev gateway goes last: it rejects an API key outright.
        assert!(
            ordered.last().is_some_and(|url| url.contains("kiro.dev")),
            "{ordered:?}"
        );
    }

    #[test]
    fn an_aws_credential_skips_the_kiro_gateway_that_would_refuse_it() {
        // IAM Identity Center and enterprise tokens both draw `403 bearer token invalid` there.
        for method in ["idc", "external_idp"] {
            let ordered = ordered_urls(&credentials(Some(method)), &registry_urls())
                .unwrap_or_else(|| panic!("{method} should be reordered"));
            assert!(
                ordered
                    .first()
                    .is_some_and(|url| url.contains("amazonaws.com")),
                "{method}: {ordered:?}"
            );
            assert!(
                ordered.last().is_some_and(|url| url.contains("kiro.dev")),
                "{method}: {ordered:?}"
            );
        }
    }

    #[test]
    fn a_kiro_token_keeps_the_registrys_own_order() {
        // Its gateway is the one that accepts it, and the registry already lists that first.
        for method in [None, Some("social"), Some("oauth")] {
            assert!(
                ordered_urls(&credentials(method), &registry_urls()).is_none(),
                "{method:?} should not be reordered"
            );
        }
    }

    #[test]
    fn an_aws_credential_from_another_region_has_its_endpoints_rewritten() {
        // An AWS token is only valid in the region it was minted in, and the registry's URLs are hardcoded
        // to us-east-1.
        let mut frankfurt = credentials(Some("idc"));
        frankfurt
            .provider_specific_data
            .insert("region".to_owned(), json!("eu-central-1"));
        let ordered =
            ordered_urls(&frankfurt, &registry_urls()).expect("an idc connection is reordered");
        assert!(
            ordered.iter().any(|url| url
                == "https://codewhisperer.eu-central-1.amazonaws.com/generateAssistantResponse"),
            "{ordered:?}"
        );
        // The path survives the rewrite.
        assert!(
            ordered
                .iter()
                .all(|url| !url.contains("amazonaws.com")
                    || url.ends_with("/generateAssistantResponse")),
            "{ordered:?}"
        );
        // The kiro.dev gateway is not regionalised, since it is not an AWS host.
        assert!(
            ordered
                .iter()
                .any(|url| url.contains("runtime.us-east-1.kiro.dev")),
            "{ordered:?}"
        );
    }

    #[test]
    fn a_blank_region_falls_back_rather_than_producing_a_malformed_host() {
        let mut blank = credentials(Some("idc"));
        blank
            .provider_specific_data
            .insert("region".to_owned(), json!("   "));
        let ordered = ordered_urls(&blank, &registry_urls()).expect("reordered");
        assert!(
            ordered
                .iter()
                .any(|url| url.contains("codewhisperer.us-east-1.amazonaws.com")),
            "{ordered:?}"
        );
    }

    #[test]
    fn only_auth_surface_failures_advance_to_the_next_endpoint() {
        // A 400 is about the body. Sending the same body to another surface cannot repair it, and doing so
        // burns the endpoint that would have worked.
        for status in [401_u16, 403, 404] {
            assert!(
                is_endpoint_fallback_status(status),
                "{status} should advance"
            );
        }
        for status in [400_u16, 429, 500, 200] {
            assert!(
                !is_endpoint_fallback_status(status),
                "{status} must not advance"
            );
        }
    }

    #[test]
    fn a_conversation_keeps_one_identity_across_its_turns() {
        // Kiro reads these as an agent run. A fresh pair per request makes every turn a new one.
        let first = Ids::for_connection("conn_kiro", 1_700_000_000_000);
        let second = Ids::for_connection("conn_kiro", 1_700_000_060_000);
        assert_eq!(first.conversation_id, second.conversation_id);
        assert_eq!(first.continuation_id, second.continuation_id);
        // The two ids are distinct from each other, and another connection gets its own.
        assert_ne!(first.conversation_id, first.continuation_id);
        assert_ne!(
            first.conversation_id,
            Ids::for_connection("conn_other", 1_700_000_000_000).conversation_id
        );
        // The timestamp does advance, since it is the request's own time.
        assert_ne!(first.timestamp, second.timestamp);
        assert!(first.timestamp.ends_with('Z'), "{}", first.timestamp);
    }
}
