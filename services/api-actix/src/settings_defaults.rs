use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct DatabaseImportRequest {
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyTestRequest {
    proxy_url: Option<String>,
    test_url: Option<String>,
    timeout_ms: Option<u64>,
}

// `DatabaseExport`, `SettingsSnapshot` and `ProxyTestResponse` lived here. The first two
// described the empty-arrays "backup" that `database_export` no longer returns; the third
// described the 501 that `proxy_test` no longer answers.

#[derive(Debug, Clone, Copy, Serialize)]
struct UnsupportedMutation {
    success: bool,
    unsupported: bool,
    error: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/settings/database")
                .route(web::get().to(database_export))
                .route(web::post().to(database_import)),
        )
        // `GET /api/settings/require-login` used to be served here. It is gone:
        // dashboard login is always required, so nothing is left to report.
        .service(web::resource("/api/settings/proxy-test").route(web::post().to(proxy_test)));
}

/// Refused, rather than answering `success: true` with empty arrays.
///
/// That is what this route used to do, and it is the worst shape available: a user clicking
/// "export my configuration" got a file that looked like a backup, validated as a backup, and
/// contained none of their providers, keys or combos. They would find out when they tried to
/// restore it.
///
/// Not implemented yet because a faithful export is every provider credential in plaintext, and
/// upstream gates it on a password re-authentication (`x-9r-password`) over and above the
/// dashboard session — a gate this port has not ported. Shipping the export before the gate
/// would put every stored credential behind one session cookie.
async fn database_export() -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &serde_json::json!({
            "success": false,
            "unsupported": true,
            "error": "Configuration export is not implemented. It carries every provider \
                      credential in plaintext, and the password re-authentication upstream \
                      requires for it is not ported yet, so this refuses rather than exporting \
                      credentials behind a session cookie alone.",
        }),
    )
}

async fn database_import(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<DatabaseImportRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let _ = request.password;
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &UnsupportedMutation {
            success: false,
            unsupported: true,
            error: "Database import is not supported by nullrouter-api",
        },
    )
}

/// Dial a test URL through the given proxy and report what happened.
///
/// A non-2xx from the test URL is still `ok: false` but carries the status, because the status
/// proves the proxy *carried* the request — which is what a user needs to distinguish "my proxy
/// is broken" from "the site I picked returns 403 to HEAD".
async fn proxy_test(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ProxyTestRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    let (proxy, target) = match crate::proxy_test::validate(
        request.proxy_url.as_deref(),
        request.test_url.as_deref(),
    ) {
        Ok(pair) => pair,
        Err(refusal) => {
            // Upstream answers 400 for a missing or unparseable proxy URL. The local-target
            // refusal is this port's own and shares the status: it is a bad request, not a
            // failed test, and reporting it as a failed test would suggest the proxy is broken.
            return responses::json(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({ "ok": false, "error": refusal.message() }),
            );
        }
    };

    let timeout = crate::proxy_test::normalise_timeout(request.timeout_ms);
    let outcome = crate::proxy_test::run(&proxy, &target, timeout).await;
    // 200 whether or not the proxy worked: the *test* completed and its result is in the body.
    // Upstream returns the failing status here, which makes a working test of a broken proxy
    // indistinguishable from a broken route.
    responses::json(StatusCode::OK, &outcome)
}
