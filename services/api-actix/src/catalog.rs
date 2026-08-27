use actix_web::web;

mod models;
mod pricing;

pub(super) fn configure(config: &mut web::ServiceConfig) {
    pricing::configure(config);
    models::configure(config);
}
