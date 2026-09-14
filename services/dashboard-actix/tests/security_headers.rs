//! Every HTML document this host serves carries the same security headers.
//!
//! Asserted per-page rather than once: the three documents are separate constants behind separate
//! handlers, so a fourth page added later without going through `html` would ship unprotected and
//! nothing else would notice.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers in an integration test are not covered by clippy.toml's \
              allow-expect-in-tests, which only reaches #[test] functions"
)]

use actix_web::{App, http::StatusCode, test};
use nullrouter_dashboard_host::DashboardConfig;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The pages a browser loads directly. `/` redirects and serves no document of its own.
const DOCUMENTS: &[&str] = &["/dashboard", "/login", "/callback", "/landing"];

fn fixture() -> Result<(TempDir, DashboardConfig), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    Ok((root, config))
}

#[actix_web::test]
async fn every_document_carries_the_security_headers() -> TestResult {
    let (_root, config) = fixture()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    for path in DOCUMENTS {
        let response =
            test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let headers = response.headers();

        for name in [
            "content-security-policy",
            "x-frame-options",
            "x-content-type-options",
            "referrer-policy",
            "permissions-policy",
        ] {
            // `get_all` rather than `get`: two identical policies is not the same as one, because a
            // browser intersects duplicate CSP headers and the effective policy stops matching what
            // is written here. A duplicate `X-Frame-Options` can make a browser ignore it outright.
            let count = headers.get_all(name).count();
            assert_eq!(count, 1, "{path} sent {name} {count} times, expected once");
        }

        let csp = headers
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();

        // WebAssembly does not run without this. Its absence would not fail a build or a test that
        // only checked the header existed -- it would produce a blank dashboard in a browser.
        assert!(
            csp.contains("'wasm-unsafe-eval'"),
            "{path} CSP would block WebAssembly.instantiate: {csp}"
        );
        // The one that matters for a credential-holding dashboard: script that gets injected must
        // not be able to reach another origin with what it finds.
        assert!(
            csp.contains("connect-src 'self'"),
            "{path} CSP does not confine outbound requests to this origin: {csp}"
        );
        assert!(
            csp.contains("frame-ancestors 'none'"),
            "{path} CSP permits framing: {csp}"
        );
        assert!(
            csp.contains("object-src 'none'"),
            "{path} CSP permits plugin content: {csp}"
        );

        assert_eq!(
            headers
                .get("x-frame-options")
                .and_then(|value| value.to_str().ok()),
            Some("DENY"),
            "{path}"
        );
        assert_eq!(
            headers
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok()),
            Some("nosniff"),
            "{path}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn no_hsts_is_promised_over_plaintext_http() -> TestResult {
    // This service speaks HTTP. Sending HSTS here would be a promise it cannot keep, and a
    // deployment terminating TLS at a proxy must set it there, where the certificate is.
    let (_root, config) = fixture()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;
    let response = test::call_service(
        &app,
        test::TestRequest::get().uri("/dashboard").to_request(),
    )
    .await;
    assert!(
        response
            .headers()
            .get("strict-transport-security")
            .is_none(),
        "HSTS must not be asserted by a plaintext listener"
    );
    Ok(())
}
