#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
mod auth;
mod config;
mod policy;
mod proxy;
mod routing;
pub mod throttle;

pub use auth::{AuthClient, AuthClientError};
pub use config::{GatewayConfig, GatewayConfigError, GatewayUpstreamAddrs, RouteKind, Upstream};
pub use policy::{
    AccessDecision, AccessRequirement, AuthorizationState, PrincipalRole, authorization_request,
    stamp_trusted_identity_headers,
};
pub use proxy::{GatewayContext, GatewayProxy};
