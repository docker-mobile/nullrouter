#![allow(clippy::future_not_send)]
#![allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    reason = "the fixture setup below runs outside a `#[test]` fn, which is the only place \
              clippy.toml's allow-expect-in-tests applies. A test that cannot create its own temp data \
              directory must fail loudly rather than run against the machine's real one. Indexing a \
              serde_json::Value is the assertion itself: a shape that does not match is a test failure, \
              which is what the panic reports."
)]

mod api_mitm_support;

use actix_web::http::{Method, StatusCode};
use serde_json::{Value, json};

use api_mitm_support::{TestResult, request_json};

/// Point this binary's MITM data directory at a temporary one, once.
///
/// The CA and alias routes write real files under `$DATA_DIR/mitm`. Without this a test run would create
/// a certificate authority in the operator's own home directory, and the tests in this file run
/// concurrently in one process, so the variable is set once for all of them rather than per test.
fn isolated_data_dir() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("nullrouter-api-mitm-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("mitm")).expect("a temp data directory");

        // A hosts file with the redirection markers in place for two tools, so the alias guard — which
        // refuses to save an alias for a tool whose traffic is not redirected — has a real state to read
        // instead of the machine's own /etc/hosts.
        let hosts = dir.join("hosts");
        std::fs::write(
            &hosts,
            "127.0.0.1 localhost\n\
             # nullrouter-mitm begin antigravity\n\
             127.0.0.1 daily-cloudcode-pa.googleapis.com\n\
             # nullrouter-mitm end antigravity\n\
             # nullrouter-mitm begin kiro\n\
             127.0.0.1 runtime.us-east-1.kiro.dev\n\
             # nullrouter-mitm end kiro\n\
             # nullrouter-mitm begin copilot\n\
             127.0.0.1 api.githubcopilot.com\n\
             # nullrouter-mitm end copilot\n",
        )
        .expect("a temp hosts file");

        // SAFETY: inside `Once`, before any test body has spawned a thread that reads the environment.
        // One operation per block: the workspace denies `multiple_unsafe_ops_per_block`, so that a
        // reviewer checking an `unsafe` block's justification has exactly one thing to check.
        unsafe {
            std::env::set_var("DATA_DIR", &dir);
        }
        // SAFETY: as above — inside `Once`, before any test body reads the environment.
        unsafe {
            std::env::set_var("NULLROUTER_HOSTS_PATH", &hosts);
        }
    });
}

const MITM_URI: &str = "/api/cli-tools/antigravity-mitm";
const MITM_UNSUPPORTED: &str = "Antigravity MITM control is not supported by nullrouter-api";
const INVALID_JSON: &str = "Invalid JSON body";
const INVALID_URL: &str = "Invalid MITM router URL";
const INVALID_PROTOCOL: &str = "MITM router URL must use http or https";
const REQUIRED_ACTION: &str = "tool and action required";
const INVALID_ACTION: &str = "action must be enable, disable, or trust-cert";

fn unsupported_json() -> Value {
    json!({
        "success": false,
        "unsupported": true,
        "message": MITM_UNSUPPORTED,
    })
}

fn assert_error(response: &(StatusCode, Value), status: StatusCode, error: &str) {
    assert_eq!(response, &(status, json!({ "error": error })));
}

fn assert_unsupported(response: &(StatusCode, Value)) {
    assert_eq!(response, &(StatusCode::NOT_IMPLEMENTED, unsupported_json()));
}

#[actix_rt::test]
async fn mitm_get_returns_the_safe_status_contract() -> TestResult {
    isolated_data_dir();
    // Given: nullrouter-api runs unprivileged and cannot inspect host trust stores or the hosts file.
    // When: the explicit MITM status endpoint is requested.
    let (status, body) = request_json(Method::GET, MITM_URI, "").await?;
    // Then: the deterministic status contract is returned, plus the CA report a client needs in order to
    // trust an interceptor. Everything this service cannot observe stays false rather than being guessed.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["running"], json!(false));
    assert_eq!(body["pid"], json!(null));
    assert_eq!(
        body["certTrusted"],
        json!(false),
        "trust is a system store's state"
    );
    assert_eq!(body["hasCachedPassword"], json!(false));
    assert_eq!(
        body["needsSudoPassword"],
        json!(false),
        "this service never invokes sudo"
    );
    assert_eq!(body["isAdmin"], json!(false));
    assert_eq!(body["isWin"], json!(cfg!(windows)));
    assert_eq!(body["mitmRouterBaseUrl"], json!("http://localhost:20128"));
    // Real state read from the hosts file, not a placeholder: the fixture redirects two of the four.
    assert_eq!(
        body["dnsStatus"],
        json!({ "antigravity": true, "copilot": true, "cursor": false, "kiro": true })
    );
    // The CA report names where the certificate goes and how to install it, without this service having
    // created one: a GET must not have a side effect.
    let authority = &body["authority"];
    assert!(
        authority["certificatePath"]
            .as_str()
            .is_some_and(|path| path.ends_with("mitm/root-ca.crt")),
        "{authority}"
    );
    assert_eq!(
        authority["canInstall"],
        json!(false),
        "installation needs root"
    );
    assert!(
        authority["installCommand"]
            .as_str()
            .is_some_and(|command| command.contains("nullrouter-mitm-helper install-ca")),
        "{authority}"
    );
    Ok(())
}

#[actix_rt::test]
async fn mitm_post_validates_json_api_key_and_router_url() -> TestResult {
    // Given: starting MITM is unsupported but retains the upstream boundary contract.
    // When: malformed, incomplete, or invalid requests are posted.
    for body in ["", "{"] {
        assert_error(
            &request_json(Method::POST, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            INVALID_JSON,
        );
    }
    for body in [
        "{}",
        r#"{"apiKey":null}"#,
        r#"{"apiKey":""}"#,
        r#"{"apiKey":"   "}"#,
    ] {
        assert_error(
            &request_json(Method::POST, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            "Missing apiKey",
        );
    }
    for url in ["/relative", "router.example", "http:/missing-authority"] {
        let body = json!({"apiKey": "key", "mitmRouterBaseUrl": url}).to_string();
        assert_error(
            &request_json(Method::POST, MITM_URI, &body).await?,
            StatusCode::BAD_REQUEST,
            INVALID_URL,
        );
    }
    assert_error(
        &request_json(
            Method::POST,
            MITM_URI,
            r#"{"apiKey":"key","mitmRouterBaseUrl":"ftp://router.example"}"#,
        )
        .await?,
        StatusCode::BAD_REQUEST,
        INVALID_PROTOCOL,
    );
    // Then: blank/default and valid absolute HTTP(S) requests reach only the unsupported response.
    for body in [
        r#"{"apiKey":"key"}"#,
        r#"{"apiKey":"key","mitmRouterBaseUrl":""}"#,
        r#"{"apiKey":"key","mitmRouterBaseUrl":"   "}"#,
        r#"{"apiKey":" key ","mitmRouterBaseUrl":"http://localhost:20128"}"#,
        r#"{"apiKey":"key","mitmRouterBaseUrl":"https://router.example/path"}"#,
    ] {
        assert_unsupported(&request_json(Method::POST, MITM_URI, body).await?);
    }
    Ok(())
}

#[actix_rt::test]
async fn mitm_delete_ignores_every_body_shape() -> TestResult {
    // Given: DELETE must not parse or act on caller data.
    // When: it receives empty, malformed, or irrelevant bodies.
    for body in ["", "{", "not-json", r#"{"irrelevant":true}"#] {
        // Then: every body reaches the same side-effect-free unsupported response.
        assert_unsupported(&request_json(Method::DELETE, MITM_URI, body).await?);
    }
    Ok(())
}

#[actix_rt::test]
async fn mitm_patch_validates_required_fields_and_actions() -> TestResult {
    isolated_data_dir();
    // Given: PATCH recognizes only three action envelopes.
    // When: malformed, incomplete, invalid, and valid actions are submitted.
    for body in ["", "{"] {
        assert_error(
            &request_json(Method::PATCH, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            INVALID_JSON,
        );
    }
    for body in [
        "{}",
        r#"{"tool":"antigravity"}"#,
        r#"{"action":"enable"}"#,
        r#"{"tool":"","action":"enable"}"#,
        r#"{"tool":"   ","action":"enable"}"#,
        r#"{"tool":"antigravity","action":"   "}"#,
    ] {
        assert_error(
            &request_json(Method::PATCH, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            REQUIRED_ACTION,
        );
    }
    assert_error(
        &request_json(
            Method::PATCH,
            MITM_URI,
            r#"{"tool":"antigravity","action":"restart"}"#,
        )
        .await?,
        StatusCode::BAD_REQUEST,
        INVALID_ACTION,
    );
    // Then: hosts-file actions are refused with the reason and the privileged command, because editing
    // `/etc/hosts` needs root and this service must stay unprivileged.
    for action in ["enable", "disable"] {
        let (status, body) = request_json(
            Method::PATCH,
            MITM_URI,
            &json!({"tool": "antigravity", "action": action}).to_string(),
        )
        .await?;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
        let error = body["error"].as_str().unwrap_or_default();
        assert!(error.contains("needs root"), "{error}");
        assert!(
            error.contains("nullrouter-mitm-helper enable-hosts"),
            "{error}"
        );
    }

    // But trust-cert generates the CA, which is the part this service *can* do, and reports where it is.
    let (status, body) =
        request_json(Method::PATCH, MITM_URI, r#"{"action":"trust-cert"}"#).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));
    assert_eq!(
        body["trusted"],
        json!(false),
        "installing into a trust store needs root"
    );
    assert_eq!(body["authority"]["exists"], json!(true));
    assert_eq!(
        body["authority"]["fingerprint"]
            .as_str()
            .unwrap_or_default()
            .len(),
        64,
        "a hex sha-256: {body}"
    );
    // Idempotent: a second call returns the same authority rather than rotating it.
    let (again_status, again) =
        request_json(Method::PATCH, MITM_URI, r#"{"action":"trust-cert"}"#).await?;
    assert_eq!(again_status, StatusCode::OK);
    assert_eq!(again["authority"], body["authority"]);
    Ok(())
}

const ALIAS_URI: &str = "/api/cli-tools/antigravity-mitm/alias";

#[actix_rt::test]
async fn the_alias_map_round_trips_through_the_file_the_interceptor_reads() -> TestResult {
    isolated_data_dir();
    // Given: upstream's standalone MITM server has no SQLite binding, so `$DATA_DIR/mitm/aliases.json`
    // *is* the interface between this control surface and any interceptor built against it.
    let (status, body) = request_json(
        Method::PUT,
        ALIAS_URI,
        r#"{"tool":"antigravity","mappings":{"gemini-3-pro":"kr/claude-sonnet-4"}}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["aliases"]["antigravity"]["gemini-3-pro"],
        json!("kr/claude-sonnet-4")
    );

    // When: the map is read back.
    let (status, read) = request_json(Method::GET, ALIAS_URI, "").await?;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        read["aliases"]["antigravity"]["gemini-3-pro"],
        json!("kr/claude-sonnet-4")
    );

    // Then: it is on disk at the path an interceptor looks for, in the shape it expects.
    let data_dir = std::env::var("DATA_DIR").expect("the isolated data dir");
    let file = std::path::Path::new(&data_dir)
        .join("mitm")
        .join("aliases.json");
    let text = std::fs::read_to_string(&file).expect("the alias file");
    let parsed: Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(
        parsed["antigravity"]["gemini-3-pro"],
        json!("kr/claude-sonnet-4"),
        "the file is the contract: {text}"
    );

    // And writing another tool leaves the first alone: a UI saving one must not blank the other.
    let (status, second) = request_json(
        Method::PUT,
        ALIAS_URI,
        r#"{"tool":"kiro","mappings":{"gpt-5":"kr/claude-4.5-opus"}}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["aliases"]["antigravity"]["gemini-3-pro"],
        json!("kr/claude-sonnet-4"),
        "the first tool's mappings must survive: {second}"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_alias_write_for_an_unknown_tool_is_refused() -> TestResult {
    isolated_data_dir();
    // An alias map for a tool the interceptor does not know is a file nothing will ever read, so
    // accepting it would report success for work that cannot happen.
    let (status, body) = request_json(
        Method::PUT,
        ALIAS_URI,
        r#"{"tool":"notatool","mappings":{"a":"b"}}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("notatool"), "{error}");
    assert!(
        error.contains("antigravity"),
        "the known tools should be named: {error}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_null_mapping_clears_an_alias_rather_than_writing_an_empty_model() -> TestResult {
    isolated_data_dir();
    // Upstream's UI deletes an alias by sending null. An alias mapped to `""` would tell the interceptor
    // to rewrite a model name to nothing.
    let (status, body) = request_json(
        Method::PUT,
        ALIAS_URI,
        r#"{"tool":"copilot","mappings":{"keep":"kr/one","drop":null,"blank":"  "}}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    // A tool of its own: these tests share one process and therefore one alias file, so two of them
    // writing the same tool would have each clobber the other's mappings.
    let mappings = &body["aliases"]["copilot"];
    assert_eq!(mappings["keep"], json!("kr/one"));
    assert!(
        mappings.get("drop").is_none(),
        "a null mapping should be dropped: {mappings}"
    );
    assert!(
        mappings.get("blank").is_none(),
        "a whitespace-only model should be dropped: {mappings}"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_alias_write_is_refused_until_that_tool_is_actually_redirected() -> TestResult {
    isolated_data_dir();
    // Upstream's guard, kept because it is right: an alias for a tool whose traffic is not being
    // redirected configures a rewrite nothing will perform, and saving it would look like it took effect.
    // The fixture redirects antigravity and kiro but not cursor.
    let (status, body) = request_json(
        Method::PUT,
        ALIAS_URI,
        r#"{"tool":"cursor","mappings":{"gpt-5":"kr/one"}}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        body["error"],
        json!("DNS must be enabled for cursor before editing model mappings")
    );

    // And nothing was written for it.
    let (status, read) = request_json(Method::GET, ALIAS_URI, "").await?;
    assert_eq!(status, StatusCode::OK);
    assert!(
        read["aliases"].get("cursor").is_none(),
        "a refused write must leave no trace: {read}"
    );
    Ok(())
}
