use async_trait::async_trait;
use bytes::Bytes;
use http::header;
use pingora_core::Result as PingoraResult;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_error::{Error, ErrorType};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{ProxyHttp, Session};
use std::net::IpAddr;

use crate::auth::{AuthClient, AuthClientError};
use crate::policy::{
    AccessDecision, AuthorizationState, authorization_request, stamp_trusted_identity_headers,
};
use crate::{GatewayConfig, RouteKind};

#[derive(Debug, Default)]
pub struct GatewayContext {
    route: Option<RouteKind>,
    client_ip: Option<IpAddr>,
}

#[derive(Debug, Clone)]
pub struct GatewayProxy {
    config: GatewayConfig,
    auth_client: AuthClient,
    /// `None` unless a rate limit was configured. Absent means no throttling at all rather than an
    /// effectively-infinite limit, so the hot path skips the lock entirely when unused.
    ///
    /// `Arc`, because this proxy is cloned per worker and the buckets must be shared: a per-clone
    /// map would multiply the effective allowance by the worker count.
    throttle: Option<std::sync::Arc<crate::throttle::Throttle>>,
}

impl GatewayProxy {
    pub fn new(config: GatewayConfig) -> Result<Self, AuthClientError> {
        let auth_client = AuthClient::new(config.auth_upstream().addr())?;
        let throttle = crate::throttle::ThrottleConfig::from_env()
            .map(|limit| std::sync::Arc::new(crate::throttle::Throttle::new(limit)));
        if let Some(limit) = crate::throttle::ThrottleConfig::from_env() {
            tracing::info!(
                per_second = limit.per_second,
                burst = limit.burst,
                "control-plane rate limit is active for /api/*"
            );
        }
        Ok(Self {
            config,
            auth_client,
            throttle,
        })
    }

    fn route_for_request(&self, session: &Session, ctx: &GatewayContext) -> RouteKind {
        ctx.route
            .unwrap_or_else(|| self.config.route_for_path(session.req_header().uri.path()))
    }
}

#[async_trait]
impl ProxyHttp for GatewayProxy {
    type CTX = GatewayContext;

    fn new_ctx(&self) -> Self::CTX {
        GatewayContext::default()
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> PingoraResult<bool> {
        let path = session.req_header().uri.path().to_owned();
        let peer_ip = session
            .as_downstream()
            .client_addr()
            .and_then(|address| address.as_inet())
            .map(std::net::SocketAddr::ip);
        let method = session.req_header().method.clone();
        let route = self.config.route_for_path(&path);
        let requirement = self.config.access_requirement(&path, &method, peer_ip);
        ctx.route = Some(route);
        ctx.client_ip = peer_ip;

        // Checked before authorization so a flood does not cost an auth service round-trip per
        // request -- which would make the gateway the amplifier for an attack on its own dependency.
        if let (Some(throttle), Some(peer)) = (self.throttle.as_ref(), peer_ip)
            && crate::throttle::Throttle::governs(&path)
            && let crate::throttle::Verdict::Throttle { retry_after } =
                throttle.check(peer, std::time::Instant::now())
        {
            tracing::warn!(
                audit = true,
                event = "gateway.rate_limited",
                %peer,
                path = %path,
                retry_after_seconds = retry_after.as_secs(),
                "control-plane request refused: rate limit exceeded"
            );
            write_throttled_response(session, retry_after).await?;
            return Ok(true);
        }

        let state = match requirement {
            crate::policy::AccessRequirement::Public
            | crate::policy::AccessRequirement::Forbidden => AuthorizationState::Authorized,
            _ => match authorization_request(session.req_header(), requirement) {
                Some(request) => match self.auth_client.authorize(&request).await {
                    Ok(response) if response.authorized => AuthorizationState::Authorized,
                    Ok(_) => AuthorizationState::Denied,
                    Err(_) => AuthorizationState::Unavailable,
                },
                None => AuthorizationState::Unavailable,
            },
        };
        let decision = requirement.decision(state);
        if decision != AccessDecision::Allow {
            write_access_response(session, decision).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> PingoraResult<Box<HttpPeer>> {
        let route = self.route_for_request(session, ctx);
        let target = self.config.target_for(route);
        Ok(Box::new(HttpPeer::new(target.addr(), false, String::new())))
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> PingoraResult<()>
    where
        Self::CTX: Send + Sync,
    {
        let route = self.route_for_request(session, ctx);
        let target = self.config.target_for(route);
        upstream_request.insert_header(header::HOST, target.authority())?;
        let client_ip = ctx
            .client_ip
            .ok_or_else(|| Error::new(ErrorType::InternalError))?;
        stamp_trusted_identity_headers(upstream_request, client_ip)?;
        Ok(())
    }
}

/// A 429 with `Retry-After`, so a client backs off by the interval the limiter computed rather than
/// guessing.
async fn write_throttled_response(
    session: &mut Session,
    retry_after: std::time::Duration,
) -> PingoraResult<()> {
    const BODY: &str = r#"{"error":"rate_limited","message":"Too many control-plane requests. Retry after the interval in the Retry-After header."}"#;

    let mut response = ResponseHeader::build(429_u16, Some(4))?;
    response.insert_header(header::CACHE_CONTROL, "no-store")?;
    response.insert_header(header::RETRY_AFTER, retry_after.as_secs().to_string())?;
    response.insert_header(header::CONTENT_TYPE, "application/json")?;
    response.insert_header(header::CONTENT_LENGTH, BODY.len().to_string())?;
    session
        .write_response_header(Box::new(response), false)
        .await?;
    session
        .write_response_body(Some(Bytes::from_static(BODY.as_bytes())), true)
        .await?;
    Ok(())
}

async fn write_access_response(
    session: &mut Session,
    decision: AccessDecision,
) -> PingoraResult<()> {
    let mut response = ResponseHeader::build(decision.status().as_u16(), Some(4))?;
    response.insert_header(header::CACHE_CONTROL, "no-store")?;
    if let Some(location) = decision.location() {
        response.insert_header(header::LOCATION, location)?;
    }

    if let Some(body) = decision.body() {
        response.insert_header(header::CONTENT_TYPE, "application/json")?;
        response.insert_header(header::CONTENT_LENGTH, body.len().to_string())?;
        session
            .write_response_header(Box::new(response), false)
            .await?;
        session
            .write_response_body(Some(Bytes::from_static(body.as_bytes())), true)
            .await?;
    } else {
        response.insert_header(header::CONTENT_LENGTH, "0")?;
        session
            .write_response_header(Box::new(response), true)
            .await?;
    }
    Ok(())
}
