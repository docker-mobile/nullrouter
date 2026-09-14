use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use actix_files::NamedFile;
use actix_web::{
    HttpResponse, Responder,
    http::header::{self, ContentType},
    web::{self, ServiceConfig},
};
use serde_json::json;

mod pages;

const PUBLIC_ASSET_CACHE_CONTROL: &str = "public, max-age=0, must-revalidate";

#[derive(Debug, Clone)]
pub struct DashboardConfig {
    static_root: Arc<PathBuf>,
}

impl DashboardConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            static_root: Arc::new(root.into()),
        }
    }

    pub fn default_static_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
    }

    pub fn into_configurer(self) -> impl FnOnce(&mut ServiceConfig) {
        move |service| configure_dashboard(service, self)
    }

    fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.static_root.join(relative)
    }
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self::new(Self::default_static_root())
    }
}

pub fn configure_dashboard(service: &mut ServiceConfig, config: DashboardConfig) {
    service
        .app_data(web::Data::new(config))
        .route("/health", web::get().to(health))
        // Both spellings: `/readyz` is the Kubernetes convention, `/ready` is what someone types.
        .route("/readyz", web::get().to(readiness))
        .route("/ready", web::get().to(readiness))
        .route("/", web::get().to(root_redirect))
        .route("/landing", web::get().to(landing))
        .route("/login", web::get().to(login))
        .route("/callback", web::get().to(callback))
        .route("/dashboard", web::get().to(dashboard))
        .route("/dashboard/{path:.*}", web::get().to(dashboard))
        .route("/favicon.svg", web::get().to(favicon_asset))
        .route("/pkg/{path:.*}", web::get().to(pkg_asset))
        .route("/providers/{path:.*}", web::get().to(provider_asset))
        .route("/assets/{path:.*}", web::get().to(generic_asset))
        // Runtime i18n: the client fetches a locale map by name. Reuses the
        // same traversal-stripping as assets so a crafted locale cannot escape
        // the static root.
        .route("/i18n/literals/{path:.*}", web::get().to(i18n_literal))
        .default_service(web::route().to(not_found));
}

async fn health() -> impl Responder {
    web::Json(json!({
        "ok": true,
        "service": "nullrouter-dashboard-host"
    }))
}

/// The files without which this host serves pages that load but do not work.
///
/// The bundle is the application: absent it, `/dashboard` still answers 200 with a document that
/// mounts nothing, which is a blank screen rather than an error. That exact failure happened twice
/// while building this dashboard, and neither time did any status code reveal it.
const REQUIRED_ASSETS: &[&str] = &["pkg/dashboard_leptos_bg.wasm", "assets/dashboard/app.css"];

/// Whether this instance can actually serve, as opposed to merely being alive.
///
/// Separate from `/health` because the two answer different questions and an orchestrator uses them
/// differently. Liveness failing means restart me; readiness failing means stop sending traffic,
/// which is the correct response to a deployment whose static assets did not ship. Conflating them
/// turns a missing bundle into a restart loop that never fixes anything.
///
/// `/health` is left exactly as it was: existing deployments and this repo's own CI poll it, and
/// changing what it means would break them silently.
async fn readiness(config: web::Data<DashboardConfig>) -> HttpResponse {
    let missing: Vec<&str> = REQUIRED_ASSETS
        .iter()
        .filter(|relative| !config.path(relative).is_file())
        .copied()
        .collect();

    if missing.is_empty() {
        return HttpResponse::Ok().json(json!({
            "ready": true,
            "service": "nullrouter-dashboard-host"
        }));
    }
    // 503 so a Kubernetes readiness probe fails, and the list so an operator does not have to guess
    // which build step was skipped.
    HttpResponse::ServiceUnavailable().json(json!({
        "ready": false,
        "service": "nullrouter-dashboard-host",
        "missing": missing,
    }))
}

async fn root_redirect() -> HttpResponse {
    HttpResponse::Found()
        .insert_header((header::LOCATION, "/dashboard"))
        .finish()
}

async fn landing() -> HttpResponse {
    html(pages::LANDING_HTML)
}

async fn login() -> HttpResponse {
    html(pages::LOGIN_HTML)
}

async fn callback() -> HttpResponse {
    html(pages::CALLBACK_HTML)
}

async fn dashboard() -> HttpResponse {
    html(DASHBOARD_HTML)
}

/// The headers every HTML response carries, and why each one is shaped the way it is.
///
/// Applied to the documents rather than to assets: a stylesheet or a `.wasm` file has no scripting
/// context to constrain, and a policy on one is noise in a review.
///
/// `script-src` needs both relaxations, and neither is incidental:
///
/// - `'wasm-unsafe-eval'` is what permits `WebAssembly.instantiate`. Without it the dashboard does
///   not run at all -- the bundle is the application.
/// - `'unsafe-inline'` covers the theme bootstrap, which must execute before first paint to avoid a
///   flash of the wrong scheme and therefore cannot be moved to a fetched file. A nonce would be
///   the stricter answer and is not available: these documents are `&'static str` constants, so
///   there is no per-response value to inject.
///
/// `connect-src 'self'` is the load-bearing one for this product: the dashboard holds provider
/// credentials, and this is what stops injected script from posting them to another origin.
///
/// No HSTS. It is meaningless over the plaintext HTTP this service speaks, and a deployment that
/// terminates TLS at a proxy should set it there, where the certificate lives.
const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "Content-Security-Policy",
        "default-src 'self'; \
         script-src 'self' 'wasm-unsafe-eval' 'unsafe-inline'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; \
         font-src 'self'; \
         connect-src 'self'; \
         frame-ancestors 'none'; \
         base-uri 'none'; \
         form-action 'self'; \
         object-src 'none'",
    ),
    // Redundant with `frame-ancestors` for current browsers, kept for older ones that read only
    // this. Enterprise fleets are exactly where those still appear.
    ("X-Frame-Options", "DENY"),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    // Nothing here uses a camera, microphone or location, so the page declines them outright.
    (
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=(), payment=()",
    ),
];

fn html(body: &'static str) -> HttpResponse {
    let mut response = HttpResponse::Ok();
    response.content_type(ContentType::html());
    for (name, value) in SECURITY_HEADERS {
        response.insert_header((*name, *value));
    }
    response.body(body)
}

async fn pkg_asset(
    config: web::Data<DashboardConfig>,
    path: web::Path<String>,
) -> actix_web::Result<impl Responder> {
    let file = config.path(Path::new("pkg").join(clean_asset_path(&path)));
    static_asset(file)
}

async fn provider_asset(
    config: web::Data<DashboardConfig>,
    path: web::Path<String>,
) -> actix_web::Result<impl Responder> {
    let file = config.path(Path::new("providers").join(clean_asset_path(&path)));
    static_asset(file)
}

async fn generic_asset(
    config: web::Data<DashboardConfig>,
    path: web::Path<String>,
) -> actix_web::Result<impl Responder> {
    let file = config.path(Path::new("assets").join(clean_asset_path(&path)));
    static_asset(file)
}

/// Serve one locale's literal map.
///
/// An unknown locale yields the standard 404 from `static_asset` rather than a
/// filesystem error, so a missing translation file is never a 500.
async fn i18n_literal(
    config: web::Data<DashboardConfig>,
    path: web::Path<String>,
) -> actix_web::Result<impl Responder> {
    let file = config.path(
        Path::new("i18n")
            .join("literals")
            .join(clean_asset_path(&path)),
    );
    static_asset(file)
}

async fn favicon_asset(config: web::Data<DashboardConfig>) -> actix_web::Result<impl Responder> {
    static_asset(config.path("assets/favicon.svg"))
}

fn static_asset(file: PathBuf) -> actix_web::Result<impl Responder> {
    Ok(NamedFile::open(file)?
        .use_last_modified(true)
        .customize()
        .insert_header((header::CACHE_CONTROL, PUBLIC_ASSET_CACHE_CONTROL)))
}

async fn not_found() -> HttpResponse {
    HttpResponse::NotFound().finish()
}

fn clean_asset_path(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("/")
}

const DASHBOARD_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>nullrouter</title>
  <meta name="description" content="Self-hosted AI model router and gateway.">
  <meta name="color-scheme" content="light dark">
  <link rel="modulepreload" href="/pkg/dashboard_leptos.js">
  <link rel="preload" href="/pkg/dashboard_leptos_bg.wasm" as="fetch" type="application/wasm" crossorigin>
  <link rel="preload" href="/assets/fonts/inter-latin.woff2" as="font" type="font/woff2" crossorigin>
  <link rel="preload" href="/assets/fonts/material-symbols-g016.woff2" as="font" type="font/woff2" crossorigin>
  <link rel="stylesheet" href="/assets/dashboard/app.css">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
  <script>
    (function () {
      var dark;
      try {
        var stored = window.localStorage.getItem("nullrouter.theme");
        if (stored === "light" || stored === "dark") {
          dark = stored === "dark";
        }
      } catch (_) {}
      if (dark === undefined) {
        try {
          dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
        } catch (_) {
          dark = false;
        }
      }
      if (dark) {
        document.documentElement.classList.add("dark");
      }
    })();
  </script>
</head>
<body>
  <noscript>This dashboard needs WebAssembly and JavaScript enabled.</noscript>
  <div id="dashboard-root"></div>
  <script type="module">
    import init from "/pkg/dashboard_leptos.js";
    await init("/pkg/dashboard_leptos_bg.wasm");
  </script>
</body>
</html>
"#;
