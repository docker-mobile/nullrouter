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
