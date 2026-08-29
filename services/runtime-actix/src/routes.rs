use actix_web::{http::Method, web};

use crate::{handlers, pxpipe};

pub fn configure(config: &mut web::ServiceConfig) {
    register_health_route(config);
    pxpipe::configure(config);
    register_v1_routes(config, "/v1");
    register_v1_routes(config, "/api/v1");
    register_v1beta_routes(config, "/v1beta");
    register_v1beta_routes(config, "/api/v1beta");
}

fn register_health_route(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/health")
            .route(web::get().to(handlers::health))
            .route(web::method(Method::OPTIONS).to(handlers::no_content)),
    );
}

fn register_v1_routes(config: &mut web::ServiceConfig, base: &'static str) {
    config.service(
        web::scope(base)
            .service(
                web::resource("")
                    .route(web::get().to(handlers::openai_models))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/models")
                    .route(web::get().to(handlers::openai_models))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/models/info")
                    .route(web::get().to(handlers::model_info))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/models/{kind}")
                    .route(web::get().to(handlers::openai_models_by_kind))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/chat/completions")
                    .route(web::post().to(handlers::chat_completions))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/responses")
                    .route(web::post().to(handlers::responses_endpoint))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/messages")
                    .route(web::post().to(handlers::messages))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/api/chat")
                    .route(web::post().to(handlers::api_chat))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/responses/compact")
                    .route(web::post().to(handlers::responses_compact))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/messages/count_tokens")
                    .route(web::post().to(handlers::count_tokens))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/embeddings")
                    .route(web::post().to(handlers::embeddings))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/images/generations")
                    .route(web::post().to(handlers::image_generations))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            // Async video jobs. The creation actions are registered before the
            // catch-all poll route so `/v1/videos/generations` is not read as a job
            // id — actix matches in registration order.
            .service(
                web::resource("/videos/{action:generations|edits|extensions}")
                    .route(web::post().to(handlers::video_create))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/videos/{id}")
                    .route(web::get().to(handlers::video_status))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/audio/speech")
                    .route(web::post().to(handlers::audio_speech))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/audio/transcriptions")
                    .route(web::post().to(handlers::audio_transcriptions))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/audio/voices")
                    .route(web::get().to(handlers::audio_voices))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/search")
                    .route(web::post().to(handlers::search))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/web/fetch")
                    .route(web::post().to(handlers::web_fetch))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .default_service(web::to(handlers::not_found)),
    );
}

fn register_v1beta_routes(config: &mut web::ServiceConfig, base: &'static str) {
    config.service(
        web::scope(base)
            .service(
                web::resource("/models")
                    .route(web::get().to(handlers::gemini_models))
                    .route(web::post().to(handlers::gemini_models))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .service(
                web::resource("/models/{tail:.*}")
                    .route(web::post().to(handlers::gemini_generation))
                    .route(web::method(Method::OPTIONS).to(handlers::no_content)),
            )
            .default_service(web::to(handlers::not_found)),
    );
}
