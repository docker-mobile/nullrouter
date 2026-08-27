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

fn html(body: &'static str) -> HttpResponse {
    HttpResponse::Ok()
        .content_type(ContentType::html())
        .body(body)
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
  <title>9Router Dashboard</title>
  <meta name="description" content="Local 9Router dashboard">
  <link rel="modulepreload" href="/pkg/dashboard_leptos.js">
  <link rel="preload" href="/pkg/dashboard_leptos_bg.wasm" as="fetch" type="application/wasm" crossorigin>
  <link rel="preload" href="/assets/fonts/inter-latin.woff2" as="font" type="font/woff2" crossorigin>
  <link rel="preload" href="/assets/fonts/material-symbols-g016.woff2" as="font" type="font/woff2" crossorigin>
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
</head>
<body>
  <noscript>Endpoint &amp; Key requires WebAssembly.</noscript>
  <div class="nr-shell" data-dashboard-fallback data-dashboard-host="actix">
    <aside class="nr-sidebar">
      <div class="nr-window-lights"><span></span><span></span><span></span></div>
      <div class="nr-logo">
        <div class="nr-logo-mark">9</div>
        <div><strong>9Router Proxy</strong><br><small>v0.5.20</small></div>
      </div>
      <nav>
        <a class="nr-sidebar-item active" href="/dashboard/endpoint">Endpoint &amp; Key</a>
        <span class="nr-sidebar-item muted">Providers</span>
        <span class="nr-sidebar-item muted">Combos</span>
        <span class="nr-sidebar-item muted">Usage</span>
        <span class="nr-sidebar-item muted">CLI Tools</span>
      </nav>
    </aside>
    <main class="nr-main">
      <header class="nr-header">
        <div>
          <p class="nr-kicker">Dashboard host</p>
          <h1>Endpoint</h1>
        </div>
        <div class="nr-header-actions">
          <span class="nr-pill">/health</span>
          <span class="nr-pill">/pkg/dashboard_leptos.js</span>
        </div>
      </header>
      <section class="nr-grid">
        <article class="nr-card">
          <div class="nr-card-title">
            <h2>API Endpoint</h2>
            <span class="nr-endpoint-badge nr-endpoint-badge-local">local</span>
          </div>
          <div class="nr-endpoint-list">
            <div class="nr-endpoint-row">
              <span class="nr-endpoint-label">Local</span>
              <code>/v1</code>
              <span class="nr-endpoint-badge">API service</span>
            </div>
          </div>
        </article>
        <article class="nr-card">
          <div class="nr-card-title">
            <h2>Provider Assets</h2>
            <span class="nr-endpoint-badge">static</span>
          </div>
          <div class="nr-provider-grid">
            <div class="nr-provider-tile"><img class="nr-provider-logo" src="/providers/openai.png" alt=""><strong>OpenAI</strong><span class="nr-provider-state">served locally</span></div>
            <div class="nr-provider-tile"><img class="nr-provider-logo" src="/providers/anthropic.png" alt=""><strong>Anthropic</strong><span class="nr-provider-state">served locally</span></div>
          </div>
        </article>
        <article class="nr-card nr-card-wide">
          <p class="nr-status-alert">The API remains a separate service behind Pingora; this host only serves dashboard assets and health.</p>
        </article>
      </section>
    </main>
  </div>
  <div id="dashboard-root"></div>
  <script type="module">
    import init from "/pkg/dashboard_leptos.js";

    const fallback = document.querySelector("[data-dashboard-fallback]");
    await init("/pkg/dashboard_leptos_bg.wasm");
    fallback?.remove();
  </script>
</body>
</html>
"#;
