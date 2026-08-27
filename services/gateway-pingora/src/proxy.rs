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
}

impl GatewayProxy {
    pub fn new(config: GatewayConfig) -> Result<Self, AuthClientError> {
        let auth_client = AuthClient::new(config.auth_upstream().addr())?;
        Ok(Self {
            config,
            auth_client,
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
        let route = self.config.route_for_path(&path);
        let requirement = self.config.access_requirement(&path, peer_ip);
        ctx.route = Some(route);
        ctx.client_ip = peer_ip;

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
