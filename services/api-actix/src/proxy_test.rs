//! Does this proxy actually work?
//!
//! Ports `lib/network/proxyTest.js`: dial a test URL through the configured proxy with a HEAD
//! request and report whether it answered, with what status, and how long it took.
//!
//! The value is in the error text. A user pasting a corporate proxy URL needs to know whether
//! it refused the connection, timed out, rejected their credentials, or resolved to nothing —
//! "proxy test failed" sends them guessing.
//!
//! # Why the two URLs are treated differently
//!
//! A local *proxy* is allowed; a local *test URL* is refused. That asymmetry is deliberate.
//!
//! Local proxies are the common legitimate case — a corporate proxy bound to loopback, a SOCKS
//! port from an SSH tunnel, mitmproxy on 127.0.0.1:8080 — so refusing them would break the
//! feature for most of the people who need it. A local test URL has no such case: the point of
//! the test is what the *proxy* can reach, and a loopback or private target answers a different
//! question, namely which internal ports this host can open. That is the shape that turns an
//! authenticated dashboard button into a network scanner, so it is a 400 rather than a test
//! result.
//!
//! This does leave a narrow local-port oracle through the proxy field: a refused connection and
//! an open port that speaks HTTP are distinguishable. It is inherent to testing a proxy at all,
//! it is what upstream does, and the route is behind dashboard auth.

use std::time::{Duration, Instant};

use serde::Serialize;

/// Where to dial when the caller names no URL (upstream `DEFAULT_TEST_URL`).
pub(crate) const DEFAULT_TEST_URL: &str = "https://google.com/";

/// Upstream's default and its ceiling.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// The outcome of one proxy dial.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyTestOutcome {
    pub ok: bool,
    /// The HTTP status the test URL answered with, when it answered at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub latency_ms: u64,
    /// The proxy's or the transport's own words. Absent on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Which URL was dialled, so a caller can see the default was used.
    pub test_url: String,
    pub timeout_ms: u64,
}

/// Why a request was refused before any dial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    MissingProxyUrl,
    InvalidProxyUrl(String),
    InvalidTestUrl(String),
    /// A test URL that would make this route a local-network scanner.
    LocalTestUrl(String),
}

impl Refusal {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::MissingProxyUrl => "proxyUrl is required".to_owned(),
            Self::InvalidProxyUrl(detail) => format!("Invalid proxy URL: {detail}"),
            Self::InvalidTestUrl(detail) => format!("Invalid test URL: {detail}"),
            Self::LocalTestUrl(host) => format!(
                "Refusing to dial {host}: a proxy test must target a host reachable through the \
                 proxy, and a loopback or private address only reveals what this machine can \
                 reach. Use a public URL, or the default {DEFAULT_TEST_URL}."
            ),
        }
    }
}

/// Clamp a caller-supplied timeout the way upstream does.
pub(crate) fn normalise_timeout(timeout_ms: Option<u64>) -> Duration {
    match timeout_ms {
        Some(value) if value > 0 => Duration::from_millis(value).min(MAX_TIMEOUT),
        // Zero or absent both mean "use the default", matching upstream's
        // `Number.isFinite && > 0` check.
        _ => DEFAULT_TIMEOUT,
    }
}

/// Is this host one that a *proxy* test has no business dialling?
///
/// Not a general SSRF filter — the point of this route is to make an outbound request the user
/// asked for. But the request goes out from the server, and a loopback or private target does
/// not test the proxy at all: it reports what this machine can reach, which is how the route
/// would be used to map an internal network from a dashboard session.
///
/// A hostname that is not an IP literal is allowed through: resolving it here to check would
/// be a DNS round trip that the proxy is about to do anyway, and a rebinding race besides.
/// `localhost` is caught by name because it is the one that matters in practice.
fn is_local_target(host: &str) -> bool {
    let bare = host.trim().trim_start_matches('[').trim_end_matches(']');
    if bare.eq_ignore_ascii_case("localhost") || bare.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    if let Ok(v4) = bare.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || v4.is_unspecified()
            || v4.is_broadcast()
            || v4.is_documentation();
    }
    if let Ok(v6) = bare.parse::<std::net::Ipv6Addr>() {
        // `is_unique_local` and `is_unicast_link_local` are unstable, so the prefixes are
        // matched directly: fc00::/7 and fe80::/10. Destructured rather than indexed, since
        // `indexing_slicing` is denied workspace-wide.
        let [first, ..] = v6.segments();
        let unique_local = (first & 0xfe00) == 0xfc00;
        let link_local = (first & 0xffc0) == 0xfe80;
        return v6.is_loopback() || v6.is_unspecified() || unique_local || link_local;
    }
    false
}

/// Validate the inputs, returning the URL to dial.
pub(crate) fn validate(
    proxy_url: Option<&str>,
    test_url: Option<&str>,
) -> Result<(String, String), Refusal> {
    let proxy = proxy_url.map(str::trim).unwrap_or_default();
    if proxy.is_empty() {
        return Err(Refusal::MissingProxyUrl);
    }
    // Parsed here rather than left to the HTTP client, so a typo is a 400 naming the problem
    // instead of a generic dial failure.
    let parsed_proxy = reqwest::Url::parse(proxy)
        .map_err(|error| Refusal::InvalidProxyUrl(error.to_string()))?;
    if !matches!(
        parsed_proxy.scheme(),
        "http" | "https" | "socks5" | "socks5h"
    ) {
        return Err(Refusal::InvalidProxyUrl(format!(
            "unsupported scheme {:?}; expected http, https, socks5 or socks5h",
            parsed_proxy.scheme()
        )));
    }

    let target = test_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_TEST_URL);
    let parsed_target =
        reqwest::Url::parse(target).map_err(|error| Refusal::InvalidTestUrl(error.to_string()))?;
    if !matches!(parsed_target.scheme(), "http" | "https") {
        return Err(Refusal::InvalidTestUrl(format!(
            "unsupported scheme {:?}; expected http or https",
            parsed_target.scheme()
        )));
    }
    let host = parsed_target
        .host_str()
        .ok_or_else(|| Refusal::InvalidTestUrl("no host".to_owned()))?;
    if is_local_target(host) {
        return Err(Refusal::LocalTestUrl(host.to_owned()));
    }

    Ok((proxy.to_owned(), target.to_owned()))
}

/// Dial the test URL through the proxy.
pub(crate) async fn run(proxy: &str, target: &str, timeout: Duration) -> ProxyTestOutcome {
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let started = Instant::now();

    let proxy_config = match reqwest::Proxy::all(proxy) {
        Ok(proxy) => proxy,
        Err(error) => {
            return ProxyTestOutcome {
                ok: false,
                status: None,
                latency_ms: 0,
                error: Some(format!("Invalid proxy URL: {error}")),
                test_url: target.to_owned(),
                timeout_ms,
            };
        }
    };
    // A fresh client per test: reusing a pooled one would reuse a connection established
    // through a *previous* proxy, and report the old proxy's reachability as the new one's.
    let client = match reqwest::Client::builder()
        .proxy(proxy_config)
        .timeout(timeout)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ProxyTestOutcome {
                ok: false,
                status: None,
                latency_ms: 0,
                error: Some(format!("Could not build a client for that proxy: {error}")),
                test_url: target.to_owned(),
                timeout_ms,
            };
        }
    };

    // HEAD, as upstream does: the body is irrelevant and downloading one through someone's
    // metered proxy to answer "does it work" would be rude.
    match client
        .head(target)
        .header(reqwest::header::USER_AGENT, "nullrouter")
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            ProxyTestOutcome {
                ok: status.is_success(),
                status: Some(status.as_u16()),
                latency_ms: elapsed_ms(started),
                // A non-2xx still proves the proxy carried the request, which is the thing
                // being tested — so the status is reported and the error says what it was.
                error: if status.is_success() {
                    None
                } else {
                    Some(format!("Test URL answered {status} through the proxy"))
                },
                test_url: target.to_owned(),
                timeout_ms,
            }
        }
        Err(error) => ProxyTestOutcome {
            ok: false,
            status: None,
            latency_ms: elapsed_ms(started),
            error: Some(describe(&error, timeout_ms)),
            test_url: target.to_owned(),
            timeout_ms,
        },
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A transport failure in words a user can act on.
///
/// `reqwest`'s `Display` is often just "error sending request", with the useful part in the
/// source chain — so the chain is walked, the way upstream reads `err.cause`.
fn describe(error: &reqwest::Error, timeout_ms: u64) -> String {
    if error.is_timeout() {
        return format!("Proxy test timed out after {timeout_ms}ms");
    }
    if error.is_connect() {
        let detail = source_chain(error);
        return format!("Could not connect through the proxy: {detail}");
    }
    let detail = source_chain(error);
    if detail == error.to_string() {
        detail
    } else {
        format!("{error}: {detail}")
    }
}

fn source_chain(error: &reqwest::Error) -> String {
    let mut parts = Vec::new();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(current) = source {
        let text = current.to_string();
        if !parts.contains(&text) {
            parts.push(text);
        }
        source = current.source();
    }
    if parts.is_empty() {
        error.to_string()
    } else {
        parts.join(": ")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TEST_URL, DEFAULT_TIMEOUT, MAX_TIMEOUT, Refusal, is_local_target,
        normalise_timeout, validate,
    };
    use std::time::Duration;

    #[test]
    fn a_missing_proxy_url_is_refused() {
        assert_eq!(validate(None, None), Err(Refusal::MissingProxyUrl));
        assert_eq!(validate(Some("  "), None), Err(Refusal::MissingProxyUrl));
    }

    #[test]
    fn the_default_test_url_is_used_when_none_is_given() {
        let (proxy, target) = validate(Some("http://proxy.example:8080"), None).expect("valid");
        assert_eq!(proxy, "http://proxy.example:8080");
        assert_eq!(target, DEFAULT_TEST_URL);
    }

    #[test]
    fn the_supported_proxy_schemes_are_accepted() {
        for scheme in ["http", "https", "socks5", "socks5h"] {
            let url = format!("{scheme}://proxy.example:1080");
            assert!(validate(Some(&url), None).is_ok(), "{scheme} should be accepted");
        }
    }

    #[test]
    fn an_unsupported_proxy_scheme_is_named_in_the_refusal() {
        let error = validate(Some("ftp://proxy.example"), None).expect_err("refused");
        let message = error.message();
        assert!(message.contains("ftp"), "{message}");
        assert!(message.contains("expected http"), "{message}");
    }

    #[test]
    fn a_local_test_url_is_refused() {
        // The case that matters: this route makes the server dial on the caller's behalf, and a
        // loopback or private target tests the machine rather than the proxy.
        for target in [
            "http://127.0.0.1:20134/internal/v1/routing-context",
            "http://localhost:20128/api/settings",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fc00::1]/",
        ] {
            let error = validate(Some("http://proxy.example:8080"), Some(target))
                .expect_err(&format!("{target} should be refused"));
            assert!(
                matches!(error, Refusal::LocalTestUrl(_)),
                "{target} gave {error:?}"
            );
        }
    }

    #[test]
    fn a_public_test_url_is_allowed() {
        for target in [
            "https://google.com/",
            "http://example.com/health",
            "https://8.8.8.8/",
        ] {
            assert!(
                validate(Some("http://proxy.example:8080"), Some(target)).is_ok(),
                "{target} should be allowed"
            );
        }
    }

    #[test]
    fn a_non_http_test_url_is_refused() {
        let error = validate(Some("http://proxy.example"), Some("file:///etc/passwd"))
            .expect_err("refused");
        assert!(matches!(error, Refusal::InvalidTestUrl(_)), "{error:?}");
    }

    #[test]
    fn local_detection_covers_the_shapes_that_matter() {
        for host in ["127.0.0.1", "localhost", "LOCALHOST", "app.localhost",
                     "10.1.2.3", "192.168.0.1", "172.31.255.255", "169.254.1.1",
                     "0.0.0.0", "::1", "[::1]", "fe80::1", "fc00::abcd"] {
            assert!(is_local_target(host), "{host} should be local");
        }
        for host in ["google.com", "8.8.8.8", "1.1.1.1", "2606:4700::1111",
                     "notlocalhost.com", "172.32.0.1", "11.0.0.1"] {
            assert!(!is_local_target(host), "{host} should not be local");
        }
    }

    #[test]
    fn a_timeout_is_clamped_and_defaulted_like_upstream() {
        assert_eq!(normalise_timeout(None), DEFAULT_TIMEOUT);
        assert_eq!(normalise_timeout(Some(0)), DEFAULT_TIMEOUT);
        assert_eq!(normalise_timeout(Some(1_500)), Duration::from_millis(1_500));
        // Upstream caps at 30s so one request cannot hold a worker indefinitely.
        assert_eq!(normalise_timeout(Some(60_000)), MAX_TIMEOUT);
        assert_eq!(normalise_timeout(Some(u64::MAX)), MAX_TIMEOUT);
    }
}
