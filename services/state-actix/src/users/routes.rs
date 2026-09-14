//! `/api/users` and the internal credential check.
//!
//! The public routes are the admin surface. They are reachable through the gateway like every other
//! `/api/*` path and are therefore behind a dashboard session; the gateway is what restricts them to
//! an admin, because it is the only component that sees the session's role before the request lands.
//!
//! `/internal/v1/users/verify` is the one the auth service calls. It is under `/internal`, which the
//! gateway refuses outright, so it is reachable only on loopback.

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use super::model::{PublicUser, Role, UserError};
use super::store::UserUpdate;
use crate::{StateStore, StoreError, responses};

/// Where the auth service asks whether a credential is good.
pub(crate) const INTERNAL_VERIFY_PATH: &str = "/internal/v1/users/verify";

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/users")
                .route(web::get().to(list_users))
                .route(web::post().to(create_user))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/users/{id}")
                .route(web::get().to(get_user))
                .route(web::put().to(update_user))
                .route(web::delete().to(delete_user))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource(INTERNAL_VERIFY_PATH)
                .route(web::post().to(verify_credentials))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn options() -> HttpResponse {
    responses::no_content()
}

#[derive(Debug, Serialize)]
struct UsersResponse {
    users: Vec<PublicUser>,
    /// Whether the legacy shared password is still accepted.
    ///
    /// Reported so the dashboard can say so rather than leaving an operator to discover it. It is
    /// true exactly while no users exist.
    #[serde(rename = "sharedPasswordActive")]
    shared_password_active: bool,
}

#[derive(Debug, Serialize)]
struct UserResponse {
    user: PublicUser,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUserRequest {
    username: Option<String>,
    password: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateUserRequest {
    display_name: Option<String>,
    email: Option<String>,
    role: Option<String>,
    is_active: Option<bool>,
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyRequest {
    username: Option<String>,
    password: Option<String>,
}

/// The answer to a credential check.
///
/// `authenticated: false` carries no reason. The auth service turns this into one message for a wrong
/// password, an unknown user, and a disabled account, so the sign-in form cannot be used to find out
/// which accounts exist.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyResponse {
    authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    /// True when no users exist, so the caller knows the shared password is still the way in.
    shared_password_active: bool,
}

async fn list_users(store: web::Data<StateStore>) -> HttpResponse {
    let (Ok(users), Ok(has_users)) = (store.list_users(), store.has_users()) else {
        return internal_error();
    };
    responses::json(
        StatusCode::OK,
        &UsersResponse {
            users,
            shared_password_active: !has_users,
        },
    )
}

async fn get_user(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.get_user(&path) {
        Ok(Some(user)) => responses::json(StatusCode::OK, &UserResponse { user }),
        Ok(None) => refuse(StatusCode::NOT_FOUND, UserError::NotFound.message()),
        Err(_) => internal_error(),
    }
}

async fn create_user(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let request = match parse::<CreateUserRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let (Some(username), Some(password)) = (request.username, request.password) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "A username and password are required",
        );
    };
    let Some(role) = request
        .role
        .as_deref()
        .map_or(Some(Role::Operator), Role::parse)
    else {
        return refuse(StatusCode::BAD_REQUEST, UserError::RoleInvalid.message());
    };
    match store.create_user(
        &username,
        &password,
        request.display_name,
        request.email,
        role,
    ) {
        Ok(Ok(user)) => responses::json(StatusCode::CREATED, &UserResponse { user }),
        Ok(Err(error)) => refuse(status_for(error), error.message()),
        Err(_) => internal_error(),
    }
}

async fn update_user(
    store: web::Data<StateStore>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match parse::<UpdateUserRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let role = match request.role.as_deref().map(Role::parse) {
        Some(Some(role)) => Some(role),
        Some(None) => return refuse(StatusCode::BAD_REQUEST, UserError::RoleInvalid.message()),
        None => None,
    };
    let update = UserUpdate {
        display_name: request.display_name,
        email: request.email,
        role,
        is_active: request.is_active,
        password: request.password,
    };
    match store.update_user(&path, update) {
        Ok(Ok(user)) => responses::json(StatusCode::OK, &UserResponse { user }),
        Ok(Err(error)) => refuse(status_for(error), error.message()),
        Err(_) => internal_error(),
    }
}

async fn delete_user(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.delete_user(&path) {
        Ok(Ok(())) => responses::json(StatusCode::OK, &serde_json::json!({ "success": true })),
        Ok(Err(error)) => refuse(status_for(error), error.message()),
        Err(_) => internal_error(),
    }
}

async fn verify_credentials(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let request = match parse::<VerifyRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Ok(has_users) = store.has_users() else {
        return internal_error();
    };
    let denied = VerifyResponse {
        authenticated: false,
        user_id: None,
        username: None,
        display_name: None,
        role: None,
        shared_password_active: !has_users,
    };
    let (Some(username), Some(password)) = (request.username, request.password) else {
        return responses::json(StatusCode::OK, &denied);
    };
    match store.verify_user(&username, &password) {
        Ok(Some(user)) => responses::json(
            StatusCode::OK,
            &VerifyResponse {
                authenticated: true,
                user_id: Some(user.id),
                username: Some(user.username),
                display_name: Some(user.display_name),
                role: Some(user.role),
                shared_password_active: false,
            },
        ),
        Ok(None) => responses::json(StatusCode::OK, &denied),
        Err(_) => internal_error(),
    }
}

/// A refusal an operator can act on maps to the status that describes it.
const fn status_for(error: UserError) -> StatusCode {
    match error {
        UserError::NotFound => StatusCode::NOT_FOUND,
        UserError::UsernameTaken | UserError::WouldOrphanAdmin => StatusCode::CONFLICT,
        UserError::HashFailed => StatusCode::INTERNAL_SERVER_ERROR,
        UserError::UsernameInvalid | UserError::PasswordTooShort | UserError::RoleInvalid => {
            StatusCode::BAD_REQUEST
        }
    }
}

fn parse<T>(body: &[u8]) -> Result<T, HttpResponse>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_slice(body).map_err(|_| refuse(StatusCode::BAD_REQUEST, "Invalid JSON body"))
}

/// `&'static str` rather than `&str`: every message here is a fixed sentence, and the store's error
/// enum returns fixed sentences too. Accepting a runtime string would invite echoing user input back,
/// which is how a refusal becomes a reflection vector.
fn refuse(status: StatusCode, message: &'static str) -> HttpResponse {
    responses::json(status, &responses::error(message))
}

fn internal_error() -> HttpResponse {
    responses::json(
        StatusCode::INTERNAL_SERVER_ERROR,
        &responses::error("State service error"),
    )
}

/// Errors from the store, kept distinct from a refusal the caller caused.
impl From<StoreError> for UserError {
    fn from(_: StoreError) -> Self {
        Self::HashFailed
    }
}
