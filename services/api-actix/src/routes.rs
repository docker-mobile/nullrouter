use actix_web::web;

use crate::{
    catalog, cli_tools, combos, handlers, headroom, keys, lifecycle, locale, media_providers,
    migrate, model_tools, oauth, provider_nodes, provider_tools, providers, proxy_pool_tools,
    proxy_pools, settings_defaults, translator, tunnel, usage,
};

pub fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/health")
                .route(web::get().to(handlers::health))
                .route(web::method(actix_web::http::Method::OPTIONS).to(handlers::no_content)),
        )
        .service(web::resource("/api/init").route(web::get().to(handlers::init)))
        .service(web::resource("/api/version").route(web::get().to(handlers::version)))
        .service(web::resource("/api/status").route(web::get().to(handlers::status)))
        .service(web::resource("/api/models").route(web::get().to(handlers::api_models)))
        .service(web::resource("/api/settings").route(web::get().to(handlers::settings)))
        .service(
            web::resource("/api/dashboard/chat/completions")
                .route(web::post().to(handlers::dashboard_chat_completions))
                .route(web::method(actix_web::http::Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/api/keys")
                .route(web::get().to(handlers::keys))
                .route(web::post().to(keys::create)),
        )
        .service(
            web::resource("/api/providers/client").route(web::get().to(handlers::providers_client)),
        )
        .service(
            web::resource("/v1")
                .route(web::get().to(handlers::openai_models))
                .route(web::method(actix_web::http::Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/v1/models")
                .route(web::get().to(handlers::openai_models))
                .route(web::method(actix_web::http::Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/v1/chat/completions")
                .route(web::post().to(handlers::chat_completions))
                .route(web::method(actix_web::http::Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/v1/responses")
                .route(web::post().to(handlers::responses_endpoint))
                .route(web::method(actix_web::http::Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/v1/messages")
                .route(web::post().to(handlers::messages))
                .route(web::method(actix_web::http::Method::OPTIONS).to(handlers::no_content)),
        );
    provider_tools::configure(config);
    providers::configure(config);
    keys::configure(config);
    combos::configure(config);
    proxy_pool_tools::configure(config);
    proxy_pools::configure(config);
    usage::configure(config);
    migrate::configure(config);
    catalog::configure(config);
    model_tools::configure(config);
    locale::configure(config);
    oauth::configure(config);
    cli_tools::configure(config);
    headroom::configure(config);
    tunnel::configure(config);
    translator::configure(config);
    media_providers::configure(config);
    settings_defaults::configure(config);
    provider_nodes::configure(config);
    lifecycle::configure(config);
    config.service(web::resource("/{tail:.*}").route(web::to(handlers::not_found)));
}
