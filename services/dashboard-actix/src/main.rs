use actix_web::{App, HttpServer};
use clap::Parser;
use nullrouter_dashboard_host::DashboardConfig;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, env = "NULLROUTER_DASHBOARD_HOST", default_value = "127.0.0.1")]
    host: String,
    #[arg(long, env = "NULLROUTER_DASHBOARD_PORT", default_value_t = 20130)]
    port: u16,
    #[arg(long, env = "NULLROUTER_DASHBOARD_STATIC")]
    static_root: Option<std::path::PathBuf>,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let config = args
        .static_root
        .map_or_else(DashboardConfig::default, DashboardConfig::new);

    HttpServer::new(move || App::new().configure(config.clone().into_configurer()))
        .bind((args.host.as_str(), args.port))?
        .run()
        .await
}
