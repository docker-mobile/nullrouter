//! Deploying a relay worker to a user's own Cloudflare, Deno or Vercel account.
//!
//! A relay is a tiny reverse proxy: it reads `x-relay-target` and `x-relay-path`, forwards the
//! request there, and returns the response. Putting one on a platform with many egress IPs is what
//! makes a proxy pool useful, and these routes create one from the dashboard rather than making the
//! user deploy it by hand.
//!
//! # Whose credentials these are
//!
//! The token arrives in the request and belongs to the person making it. Nothing here needs an
//! account of its own, which is why this is implemented rather than refused — an earlier note in
//! this port said the opposite and was wrong.
//!
//! What follows from that is where the care goes: the token is a credential for a third-party
//! account with, in every case, permission to deploy code. It is used for the calls of one request
//! and never stored, never logged, and never echoed in a response. The pool record that outlives the
//! request holds only the resulting URL.
//!
//! # Deploying code to a user's account is not a small action
//!
//! Each of these creates something billable and publicly reachable under the user's name. So the
//! route validates first and does not deploy on a malformed request, reports the platform's own
//! error rather than a generic failure, and — where the platform's API allows it — cleans up after a
//! deploy that starts and then fails, so a failed attempt does not leave a half-built project behind.
//!
//! # The worker forwards to a caller-named target, by design
//!
//! `x-relay-target` is read from the request, which is the whole point of a relay. It is also why
//! these deploys are held to the same host-only boundary as the rest of the CLI-tool and tunnel
//! routes: whoever can deploy one can also point it anywhere.

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;
use serde_json::Value;

use crate::{json_body, responses};

/// Cloudflare's Workers API.
const CLOUDFLARE_API: &str = "https://api.cloudflare.com/client/v4";
/// Deno's Deploy API.
const DENO_API: &str = "https://api.deno.com/v2";
/// Vercel's API.
const VERCEL_API: &str = "https://api.vercel.com";

/// Overrides for the three API bases, so the deploy flows can be tested against a stub.
///
/// Read from the process environment, which only whoever starts the service controls: not reachable
/// from a request, so not a way to redirect a user's token somewhere else. Without them the only way
/// to cover a multi-step deploy is to really deploy something to somebody's account.
const CLOUDFLARE_API_VAR: &str = "NULLROUTER_CLOUDFLARE_API";
const DENO_API_VAR: &str = "NULLROUTER_DENO_API";
const VERCEL_API_VAR: &str = "NULLROUTER_VERCEL_API";

/// How long any one platform call may take.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The relay, as a Cloudflare module worker.
///
/// `duplex: "half"` on a streaming body is required by the Workers runtime and is not optional
/// decoration: without it a request with a body is rejected before it is forwarded.
const CLOUDFLARE_WORKER: &str = r#"export default {
  async fetch(request, env, ctx) {
    const target = request.headers.get("x-relay-target");
    const relayPath = request.headers.get("x-relay-path") || "/";

    if (!target) {
      return new Response(JSON.stringify({ error: "Missing x-relay-target header" }), {
        status: 400,
        headers: { "content-type": "application/json" },
      });
    }

    const targetUrl = target.replace(/\/$/, "") + relayPath;
    const newRequestInit = {
      method: request.method,
      headers: new Headers(request.headers),
    };

    if (request.method !== "GET" && request.method !== "HEAD") {
      newRequestInit.body = request.body;
      newRequestInit.duplex = "half";
    }

    newRequestInit.headers.delete("x-relay-target");
    newRequestInit.headers.delete("x-relay-path");
    newRequestInit.headers.delete("host");

    try {
      const response = await fetch(targetUrl, newRequestInit);
      return new Response(response.body, {
        status: response.status,
        headers: response.headers,
      });
    } catch (error) {
      return new Response(JSON.stringify({ error: error.message }), {
        status: 502,
        headers: { "content-type": "application/json" },
      });
    }
  },
};
"#;

/// The same relay for Deno's runtime.
const DENO_WORKER: &str = r#"Deno.serve(async (request) => {
  const target = request.headers.get("x-relay-target");
  const relayPath = request.headers.get("x-relay-path") || "/";

  if (!target) {
    return new Response(JSON.stringify({ error: "Missing x-relay-target header" }), {
      status: 400,
      headers: { "content-type": "application/json" },
    });
  }

  const targetUrl = target.replace(/\/$/, "") + relayPath;
  const newHeaders = new Headers(request.headers);
  newHeaders.delete("x-relay-target");
  newHeaders.delete("x-relay-path");
  newHeaders.delete("host");

  try {
    const response = await fetch(targetUrl, {
      method: request.method,
      headers: newHeaders,
      body: request.method === "GET" || request.method === "HEAD" ? undefined : request.body,
    });
    return new Response(response.body, {
      status: response.status,
      headers: response.headers,
    });
  } catch (error) {
    return new Response(JSON.stringify({ error: error.message }), {
      status: 502,
      headers: { "content-type": "application/json" },
    });
  }
});
"#;

/// The same relay as a Vercel serverless function.
const VERCEL_WORKER: &str = r#"export default async function handler(request) {
  const target = request.headers.get("x-relay-target");
  const relayPath = request.headers.get("x-relay-path") || "/";

  if (!target) {
    return new Response(JSON.stringify({ error: "Missing x-relay-target header" }), {
      status: 400,
      headers: { "content-type": "application/json" },
    });
  }

  const targetUrl = target.replace(/\/$/, "") + relayPath;
  const newHeaders = new Headers(request.headers);
  newHeaders.delete("x-relay-target");
  newHeaders.delete("x-relay-path");
  newHeaders.delete("host");

  try {
    const response = await fetch(targetUrl, {
      method: request.method,
      headers: newHeaders,
      body: request.method === "GET" || request.method === "HEAD" ? undefined : request.body,
    });
    return new Response(response.body, {
      status: response.status,
      headers: response.headers,
    });
  } catch (error) {
    return new Response(JSON.stringify({ error: error.message }), {
      status: 502,
      headers: { "content-type": "application/json" },
    });
  }
}

export const config = { runtime: "edge" };
"#;

fn api_base(variable: &str, default: &str) -> String {
    std::env::var(variable)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| default.to_owned())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())
}

/// A generated project name, when the caller does not supply one.
///
/// Upstream's `relay-${Date.now().toString(36)}`, which is the same shape and the same collision
/// behaviour — two deploys in the same millisecond clash, and the platform reports it.
fn default_project_name() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    format!("relay-{}", base36(millis))
}

fn base36(mut value: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_owned();
    }
    let mut out = Vec::new();
    while value > 0 {
        let index = usize::try_from(value % 36).unwrap_or(0);
        if let Some(digit) = DIGITS.get(index) {
            out.push(*digit);
        }
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// A project name that is safe in a URL path segment and as a platform project id.
///
/// The name reaches a platform API path, so it is checked rather than trusted: a `..` or a slash in
/// it would address a different resource in the user's account than the one the dashboard named.
fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// The project name from a request, or a generated one, or a refusal.
fn project_name(supplied: Option<&str>) -> Result<String, String> {
    match supplied.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) if valid_project_name(name) => Ok(name.to_owned()),
        Some(name) => Err(format!(
            "Project name {name:?} is not usable: lowercase letters, digits and inner hyphens only, \
             at most 63 characters."
        )),
        None => Ok(default_project_name()),
    }
}

/// What every deploy ends with: a pool record pointing at the new relay.
///
/// Written through the state service rather than held here, so the pool appears in the same list the
/// dashboard's proxy-pool pane reads and the runtime selects from.
async fn record_pool(
    state: &crate::StateClient,
    name: &str,
    url: &str,
    kind: &str,
) -> Result<Value, String> {
    state
        .create_proxy_pool(&serde_json::json!({
            "name": name,
            "proxyUrl": url,
            "type": kind,
            "noProxy": "",
            "isActive": true,
            "strictProxy": false,
        }))
        .await
        .ok_or_else(|| {
            format!(
                "The relay deployed to {url}, but the proxy pool could not be recorded because \
                 nullrouter-state did not answer. Add it by hand rather than deploying again."
            )
        })
}

fn deployed(pool: Value, url: &str) -> HttpResponse {
    responses::json(
        StatusCode::CREATED,
        &serde_json::json!({ "proxyPool": pool, "deployUrl": url }),
    )
}

fn refuse(status: StatusCode, error: impl Into<String>) -> HttpResponse {
    responses::json(
        status,
        &serde_json::json!({ "success": false, "error": error.into() }),
    )
}

/// The platform's own error message, if it sent one in a shape we recognise.
///
/// Reported rather than replaced with a generic failure: "Authentication error [10000]" tells a user
/// their token is wrong, where "deploy failed" sends them looking for the problem here.
fn platform_error(body: &Value, fallback: &str) -> String {
    // Cloudflare: `{errors: [{message}]}`. Vercel: `{error: {message}}`. Deno: plain text, handled
    // by the caller.
    body.get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.first())
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
        .unwrap_or_else(|| fallback.to_owned())
}

/// A status a platform returned, mapped to one this route can send.
///
/// A platform's 401 is about the user's token, so it is passed through; anything from the 5xx range
/// becomes a 502, because the failure is upstream of here and a 500 would read as this router's bug.
fn passthrough_status(status: reqwest::StatusCode) -> StatusCode {
    let code = status.as_u16();
    if (400..500).contains(&code) {
        StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST)
    } else {
        StatusCode::BAD_GATEWAY
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudflareRequest {
    account_id: Option<String>,
    api_token: Option<String>,
    project_name: Option<String>,
}

/// Upload the worker, turn on its `workers.dev` hostname, then record the pool.
///
/// Three calls, in this order because each depends on the last. The middle one is allowed to fail —
/// upstream ignores it too — because the subdomain may already be enabled, and the third call is what
/// actually establishes whether the relay is reachable.
async fn cloudflare(state: web::Data<crate::StateClient>, body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<CloudflareRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let account = request
        .account_id
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let token = request
        .api_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if account.is_empty() || token.is_empty() {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Cloudflare Account ID and API Token are required",
        );
    }
    // The account id lands in an API path, so it is checked for the same reason the project name is.
    if !account
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        return refuse(
            StatusCode::BAD_REQUEST,
            "Cloudflare Account ID is not valid",
        );
    }
    let name = match project_name(request.project_name.as_deref()) {
        Ok(name) => name,
        Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
    };
    let client = match client() {
        Ok(client) => client,
        Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error),
    };

    let base = api_base(CLOUDFLARE_API_VAR, CLOUDFLARE_API);
    let script_url = format!("{base}/accounts/{account}/workers/scripts/{name}");

    // Cloudflare takes the script as multipart, with the entrypoint named by `main_module` in a
    // metadata part. A plain body upload is rejected.
    let metadata = serde_json::json!({
        "main_module": "index.js",
        "compatibility_date": "2024-03-20",
        "observability": { "enabled": true },
    })
    .to_string();
    let form = reqwest::multipart::Form::new()
        .part(
            "index.js",
            match reqwest::multipart::Part::text(CLOUDFLARE_WORKER)
                .file_name("index.js")
                .mime_str("application/javascript+module")
            {
                Ok(part) => part,
                Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
            },
        )
        .part(
            "metadata",
            match reqwest::multipart::Part::text(metadata)
                .file_name("metadata.json")
                .mime_str("application/json")
            {
                Ok(part) => part,
                Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
            },
        );

    let upload = client
        .put(&script_url)
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await;
    let upload = match upload {
        Ok(response) => response,
        Err(error) => {
            return refuse(
                StatusCode::BAD_GATEWAY,
                format!("Could not reach Cloudflare: {error}"),
            );
        }
    };
    if !upload.status().is_success() {
        let status = passthrough_status(upload.status());
        let detail: Value = upload.json().await.unwrap_or(Value::Null);
        return refuse(
            status,
            platform_error(&detail, "Failed to upload Worker to Cloudflare"),
        );
    }

    // Best-effort: it may already be on, and the next call is what decides reachability.
    let _ = client
        .post(format!("{script_url}/subdomain"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await;

    let subdomain = client
        .get(format!("{base}/accounts/{account}/workers/subdomain"))
        .bearer_auth(token)
        .send()
        .await
        .ok();
    let subdomain = match subdomain {
        Some(response) if response.status().is_success() => {
            response.json::<Value>().await.ok().and_then(|body| {
                body.get("result")
                    .and_then(|result| result.get("subdomain"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        }
        Some(_) | None => None,
    };
    let Some(subdomain) = subdomain.filter(|value| !value.is_empty()) else {
        // The worker exists but has no hostname, so there is nothing to record. Said plainly, with
        // the fix, because the cause is an account setting rather than anything about this request.
        return refuse(
            StatusCode::BAD_REQUEST,
            "The Worker deployed, but this account has no workers.dev subdomain, so the relay has \
             no URL. Set one up in the Cloudflare dashboard and deploy again.",
        );
    };

    let url = format!("https://{name}.{subdomain}.workers.dev");
    match record_pool(&state, &name, &url, "cloudflare").await {
        Ok(pool) => deployed(pool, &url),
        Err(error) => refuse(StatusCode::BAD_GATEWAY, error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DenoRequest {
    org_domain: Option<String>,
    deno_token: Option<String>,
    project_name: Option<String>,
}

/// How long a build is waited on. Upstream's 30 attempts at 2s.
const BUILD_ATTEMPTS: u32 = 30;
const BUILD_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Create the app, deploy the asset, wait for the build, then record the pool.
///
/// The app is deleted if the deploy or the build fails. That cleanup is upstream's and it matters:
/// without it a failed attempt leaves an app in the user's account holding the name, so the obvious
/// next step — try again — fails with "already exists".
async fn deno(state: web::Data<crate::StateClient>, body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<DenoRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let org = request
        .org_domain
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let token = request
        .deno_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    // Checked in upstream's order, so the message a user sees for an empty form matches.
    if org.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "Organization domain is required");
    }
    if token.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "Deno Deploy API token is required");
    }
    let name = match project_name(request.project_name.as_deref()) {
        Ok(name) => name,
        Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
    };
    let client = match client() {
        Ok(client) => client,
        Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error),
    };
    let base = api_base(DENO_API_VAR, DENO_API);

    let created = client
        .post(format!("{base}/apps"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "slug": name,
            "labels": { "custom.kind": "9router-relay" },
            "config": {
                "install": "deno install",
                "runtime": { "type": "dynamic", "entrypoint": "main.ts" },
            },
        }))
        .send()
        .await;
    let created = match created {
        Ok(response) => response,
        Err(error) => {
            return refuse(
                StatusCode::BAD_GATEWAY,
                format!("Could not reach Deno Deploy: {error}"),
            );
        }
    };
    if !created.status().is_success() {
        // 409 is worth its own message: the name is taken, and the fix is to pick another rather
        // than to check the token.
        if created.status().as_u16() == 409 {
            return refuse(
                StatusCode::CONFLICT,
                format!("App \"{name}\" already exists. Choose a different name."),
            );
        }
        let status = passthrough_status(created.status());
        let detail = created.text().await.unwrap_or_default();
        return refuse(status, format!("Failed to create app: {detail}"));
    }
    let app_id = created
        .json::<Value>()
        .await
        .ok()
        .and_then(|app| app.get("id").and_then(Value::as_str).map(str::to_owned));
    let Some(app_id) = app_id else {
        return refuse(
            StatusCode::BAD_GATEWAY,
            "Deno Deploy created the app but did not return its id, so it cannot be deployed to.",
        );
    };

    let deployed_revision = client
        .post(format!("{base}/apps/{app_id}/deploy"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "assets": {
                "main.ts": { "kind": "file", "content": DENO_WORKER, "encoding": "utf-8" },
            },
        }))
        .send()
        .await;
    let deployed_revision = match deployed_revision {
        Ok(response) if response.status().is_success() => response,
        outcome => {
            let detail = match outcome {
                Ok(response) => response.text().await.unwrap_or_default(),
                Err(error) => error.to_string(),
            };
            delete_deno_app(&client, &base, token, &app_id).await;
            return refuse(StatusCode::BAD_GATEWAY, format!("Deploy failed: {detail}"));
        }
    };

    let revision = deployed_revision
        .json::<Value>()
        .await
        .unwrap_or(Value::Null);
    let revision_id = revision
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut status = revision
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let mut attempts = 0_u32;
    while status == "queued" || status == "building" {
        if attempts >= BUILD_ATTEMPTS {
            delete_deno_app(&client, &base, token, &app_id).await;
            return refuse(
                StatusCode::GATEWAY_TIMEOUT,
                "The Deno build did not finish within 60 seconds.",
            );
        }
        actix_web::rt::time::sleep(BUILD_POLL).await;
        let polled = client
            .get(format!("{base}/revisions/{revision_id}"))
            .bearer_auth(token)
            .send()
            .await;
        match polled {
            Ok(response) if response.status().is_success() => {
                status = response
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|body| {
                        body.get("status")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or(status);
            }
            // A failed poll stops the wait rather than retrying forever; the status check below
            // then reports whatever was last known.
            Ok(_) | Err(_) => break,
        }
        attempts += 1;
    }

    if status != "succeeded" {
        delete_deno_app(&client, &base, token, &app_id).await;
        return refuse(
            StatusCode::BAD_GATEWAY,
            format!("The Deno build finished with status {status:?} rather than succeeding."),
        );
    }

    // The hostname is `{app}.{org}.deno.net`, where the org slug is the first label of the domain
    // the caller gave.
    let org_slug = org.split('.').next().unwrap_or(org);
    let url = format!("https://{name}.{org_slug}.deno.net");
    match record_pool(&state, &name, &url, "deno").await {
        Ok(pool) => deployed(pool, &url),
        Err(error) => refuse(StatusCode::BAD_GATEWAY, error),
    }
}

/// Best-effort cleanup, so a failed deploy does not hold the name.
async fn delete_deno_app(client: &reqwest::Client, base: &str, token: &str, app_id: &str) {
    let _ = client
        .delete(format!("{base}/apps/{app_id}"))
        .bearer_auth(token)
        .send()
        .await;
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VercelRequest {
    vercel_token: Option<String>,
    project_name: Option<String>,
}

/// How long a Vercel deployment is waited on. Upstream's 120s at 3s intervals.
const VERCEL_ATTEMPTS: u32 = 40;
const VERCEL_POLL: std::time::Duration = std::time::Duration::from_secs(3);

/// Create the deployment, turn off deployment protection, wait for READY, then record the pool.
///
/// The protection call is the step whose absence would be a silent failure: Vercel puts SSO in front
/// of new deployments by default, so a relay left protected answers every request with a login page
/// and the pool looks configured but never works.
async fn vercel(state: web::Data<crate::StateClient>, body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<VercelRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let token = request
        .vercel_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if token.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "Vercel API token is required");
    }
    let name = match project_name(request.project_name.as_deref()) {
        Ok(name) => name,
        Err(error) => return refuse(StatusCode::BAD_REQUEST, error),
    };
    let client = match client() {
        Ok(client) => client,
        Err(error) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, error),
    };
    let base = api_base(VERCEL_API_VAR, VERCEL_API);

    let created = client
        .post(format!("{base}/v13/deployments"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "name": name,
            "files": [
                { "file": "api/relay.js", "data": VERCEL_WORKER },
                {
                    "file": "package.json",
                    "data": serde_json::json!({ "name": name, "version": "1.0.0" }).to_string(),
                },
                {
                    "file": "vercel.json",
                    // Every path is rewritten to the one function, so the relay answers whatever
                    // path a caller puts in `x-relay-path`.
                    "data": serde_json::json!({
                        "rewrites": [{ "source": "/(.*)", "destination": "/api/relay" }],
                    })
                    .to_string(),
                },
            ],
            "projectSettings": { "framework": Value::Null },
            "target": "production",
        }))
        .send()
        .await;
    let created = match created {
        Ok(response) => response,
        Err(error) => {
            return refuse(
                StatusCode::BAD_GATEWAY,
                format!("Could not reach Vercel: {error}"),
            );
        }
    };
    if !created.status().is_success() {
        let status = passthrough_status(created.status());
        let detail: Value = created.json().await.unwrap_or(Value::Null);
        return refuse(
            status,
            platform_error(&detail, "Failed to create Vercel deployment"),
        );
    }
    let deployment: Value = created.json().await.unwrap_or(Value::Null);
    // `id` on some responses and `uid` on others, so both are accepted.
    let deployment_id = deployment
        .get("id")
        .or_else(|| deployment.get("uid"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if deployment_id.is_empty() {
        return refuse(
            StatusCode::BAD_GATEWAY,
            "Vercel created the deployment but did not return its id, so it cannot be waited on.",
        );
    }
    let project_id = deployment
        .get("projectId")
        .and_then(Value::as_str)
        .unwrap_or(&name)
        .to_owned();

    // Best-effort, as upstream has it: a token without project scope cannot do this, and the
    // deployment is still usable if protection happened to be off already.
    let _ = client
        .patch(format!("{base}/v9/projects/{project_id}"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "ssoProtection": Value::Null }))
        .send()
        .await;

    let mut attempts = 0_u32;
    // The loop yields the URL rather than filling in a variable declared above it: every path out of
    // here either returns or has a URL in hand, so there is no such thing as a not-yet-set value.
    let ready_url = loop {
        let polled = client
            .get(format!("{base}/v13/deployments/{deployment_id}"))
            .bearer_auth(token)
            .send()
            .await;
        let state_body: Value = match polled {
            Ok(response) => response.json().await.unwrap_or(Value::Null),
            Err(error) => {
                return refuse(
                    StatusCode::BAD_GATEWAY,
                    format!("Could not read the Vercel deployment state: {error}"),
                );
            }
        };
        match state_body.get("readyState").and_then(Value::as_str) {
            Some("READY") => {
                break state_body
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
            }
            Some(failed @ ("ERROR" | "CANCELED")) => {
                return refuse(
                    StatusCode::BAD_GATEWAY,
                    format!("The Vercel deployment ended as {failed}."),
                );
            }
            Some(_) | None => {}
        }
        attempts += 1;
        if attempts >= VERCEL_ATTEMPTS {
            return refuse(
                StatusCode::GATEWAY_TIMEOUT,
                "The Vercel deployment did not become ready within 120 seconds.",
            );
        }
        actix_web::rt::time::sleep(VERCEL_POLL).await;
    };

    if ready_url.is_empty() {
        return refuse(
            StatusCode::BAD_GATEWAY,
            "The Vercel deployment reported ready without a URL, so there is nothing to relay to.",
        );
    }
    let url = format!("https://{ready_url}");
    match record_pool(&state, &name, &url, "vercel").await {
        Ok(pool) => deployed(pool, &url),
        Err(error) => refuse(StatusCode::BAD_GATEWAY, error),
    }
}

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/proxy-pools/cloudflare-deploy")
                .route(web::post().to(cloudflare))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/proxy-pools/deno-deploy")
                .route(web::post().to(deno))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/proxy-pools/vercel-deploy")
                .route(web::post().to(vercel))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test assertions read clearer with expect than with error plumbing"
)]
mod tests {
    use super::{base36, default_project_name, platform_error, project_name, valid_project_name};

    #[test]
    fn a_project_name_that_could_address_another_resource_is_refused() {
        // The name goes into a platform API path, so a separator in it would act on something other
        // than the project the dashboard named.
        for name in [
            "../other",
            "a/b",
            "a b",
            "UPPER",
            "-leading",
            "trailing-",
            "with_underscore",
            &"x".repeat(64),
        ] {
            assert!(!valid_project_name(name), "{name:?} should be refused");
            assert!(project_name(Some(name)).is_err(), "{name:?}");
        }
        for name in ["relay-1", "abc", "a-b-c", "relay-mjk4l2"] {
            assert!(valid_project_name(name), "{name:?} should be accepted");
        }
    }

    #[test]
    fn an_absent_name_is_generated_and_usable() {
        let generated = default_project_name();
        assert!(generated.starts_with("relay-"), "{generated}");
        assert!(
            valid_project_name(&generated),
            "a generated name must pass the same check: {generated}"
        );
        assert_eq!(
            project_name(None).map(|name| name.starts_with("relay-")),
            Ok(true)
        );
        // Whitespace is not a name.
        assert!(project_name(Some("   ")).is_ok_and(|name| name.starts_with("relay-")));
    }

    #[test]
    fn base36_matches_the_javascript_encoding_upstream_uses() {
        // Upstream names projects with `Date.now().toString(36)`, so a user's existing relays are
        // named this way and a differing encoding would look like a different tool wrote them.
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        assert_eq!(base36(1_700_000_000_000), "loyw3v28");
    }

    #[test]
    fn a_platform_error_is_reported_rather_than_replaced() {
        // "Authentication error" tells a user their token is wrong; "deploy failed" sends them
        // looking for the problem in this router.
        let cloudflare = serde_json::json!({"errors": [{"message": "Authentication error"}]});
        assert_eq!(
            platform_error(&cloudflare, "fallback"),
            "Authentication error"
        );

        let vercel = serde_json::json!({"error": {"message": "Not authorized"}});
        assert_eq!(platform_error(&vercel, "fallback"), "Not authorized");

        // And an unrecognised shape falls back rather than reporting an empty string.
        assert_eq!(
            platform_error(&serde_json::json!({}), "fallback"),
            "fallback"
        );
        assert_eq!(
            platform_error(&serde_json::Value::Null, "fallback"),
            "fallback"
        );
    }
}
