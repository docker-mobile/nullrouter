use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::policy::AccessRequirement;
use crate::routing::route_for_path;

const DEFAULT_LISTEN_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20128);
const DEFAULT_API_UPSTREAM_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20129);
const DEFAULT_DASHBOARD_UPSTREAM_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20130);
const DEFAULT_CATALOG_UPSTREAM_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20131);
const DEFAULT_RUNTIME_UPSTREAM_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20132);
const DEFAULT_EVENTS_UPSTREAM_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20133);
const DEFAULT_STATE_UPSTREAM_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20134);
const DEFAULT_AUTH_UPSTREAM_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20135);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    Api,
    Catalog,
    Runtime,
    Events,
    State,
    Dashboard,
    Auth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayConfigError {
    NonLoopbackUpstream { addr: SocketAddr },
}

impl fmt::Display for GatewayConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonLoopbackUpstream { addr } => {
                write!(
                    formatter,
                    "gateway upstream {addr} must use a loopback address"
                )
            }
        }
    }
}

impl std::error::Error for GatewayConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    addr: SocketAddr,
    authority: String,
}

impl Upstream {
    fn new(addr: SocketAddr) -> Result<Self, GatewayConfigError> {
        if addr.ip().is_loopback() {
            Ok(Self::trusted_loopback(addr))
        } else {
            Err(GatewayConfigError::NonLoopbackUpstream { addr })
        }
    }

    fn trusted_loopback(addr: SocketAddr) -> Self {
        Self {
            addr,
            authority: addr.to_string(),
        }
    }

    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayUpstreamAddrs {
    pub api: SocketAddr,
    pub dashboard: SocketAddr,
    pub catalog: SocketAddr,
    pub runtime: SocketAddr,
    pub events: SocketAddr,
    pub state: SocketAddr,
    pub auth: SocketAddr,
}

impl Default for GatewayUpstreamAddrs {
    fn default() -> Self {
        Self {
            api: DEFAULT_API_UPSTREAM_ADDR,
            dashboard: DEFAULT_DASHBOARD_UPSTREAM_ADDR,
            catalog: DEFAULT_CATALOG_UPSTREAM_ADDR,
            runtime: DEFAULT_RUNTIME_UPSTREAM_ADDR,
            events: DEFAULT_EVENTS_UPSTREAM_ADDR,
            state: DEFAULT_STATE_UPSTREAM_ADDR,
            auth: DEFAULT_AUTH_UPSTREAM_ADDR,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    listen_addr: SocketAddr,
    api_upstream: Upstream,
    dashboard_upstream: Upstream,
    catalog_upstream: Upstream,
    runtime_upstream: Upstream,
    events_upstream: Upstream,
    state_upstream: Upstream,
    auth_upstream: Upstream,
    enforce_managed_api_keys: bool,
}

impl GatewayConfig {
    pub fn new(
        listen_addr: SocketAddr,
        upstream_addrs: GatewayUpstreamAddrs,
    ) -> Result<Self, GatewayConfigError> {
        Ok(Self {
            listen_addr,
            api_upstream: Upstream::new(upstream_addrs.api)?,
            dashboard_upstream: Upstream::new(upstream_addrs.dashboard)?,
            catalog_upstream: Upstream::new(upstream_addrs.catalog)?,
            runtime_upstream: Upstream::new(upstream_addrs.runtime)?,
            events_upstream: Upstream::new(upstream_addrs.events)?,
            state_upstream: Upstream::new(upstream_addrs.state)?,
            auth_upstream: Upstream::new(upstream_addrs.auth)?,
            enforce_managed_api_keys: false,
        })
    }

    pub const fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    pub const fn api_upstream(&self) -> &Upstream {
        &self.api_upstream
    }

    pub const fn dashboard_upstream(&self) -> &Upstream {
        &self.dashboard_upstream
    }

    pub const fn catalog_upstream(&self) -> &Upstream {
        &self.catalog_upstream
    }

    pub const fn runtime_upstream(&self) -> &Upstream {
        &self.runtime_upstream
    }

    pub const fn events_upstream(&self) -> &Upstream {
        &self.events_upstream
    }

    pub const fn state_upstream(&self) -> &Upstream {
        &self.state_upstream
    }

    pub const fn auth_upstream(&self) -> &Upstream {
        &self.auth_upstream
    }

    pub const fn managed_api_keys_required(&self) -> bool {
        self.enforce_managed_api_keys
    }

    #[must_use]
    pub fn with_managed_api_key_enforcement(self, enabled: bool) -> Self {
        Self {
            enforce_managed_api_keys: enabled,
            ..self
        }
    }

    pub fn route_for_path(&self, path: &str) -> RouteKind {
        route_for_path(path)
    }

    /// The access rule for a request.
    ///
    /// The method is part of the input, not decoration: some paths are readable with a session
    /// from anywhere but writable only from this host, so a rule derived from the path alone
    /// would have to pick one answer for both and get one of them wrong.
    pub fn access_requirement(
        &self,
        path: &str,
        method: &http::Method,
        peer_ip: Option<IpAddr>,
    ) -> AccessRequirement {
        AccessRequirement::for_request(
            path,
            method,
            self.route_for_path(path),
            peer_ip,
            self.enforce_managed_api_keys,
        )
    }

    pub const fn target_for(&self, route: RouteKind) -> &Upstream {
        match route {
            RouteKind::Api => &self.api_upstream,
            RouteKind::Catalog => &self.catalog_upstream,
            RouteKind::Runtime => &self.runtime_upstream,
            RouteKind::Events => &self.events_upstream,
            RouteKind::State => &self.state_upstream,
            RouteKind::Dashboard => &self.dashboard_upstream,
            RouteKind::Auth => &self.auth_upstream,
        }
    }
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            listen_addr: DEFAULT_LISTEN_ADDR,
            api_upstream: Upstream::trusted_loopback(DEFAULT_API_UPSTREAM_ADDR),
            dashboard_upstream: Upstream::trusted_loopback(DEFAULT_DASHBOARD_UPSTREAM_ADDR),
            catalog_upstream: Upstream::trusted_loopback(DEFAULT_CATALOG_UPSTREAM_ADDR),
            runtime_upstream: Upstream::trusted_loopback(DEFAULT_RUNTIME_UPSTREAM_ADDR),
            events_upstream: Upstream::trusted_loopback(DEFAULT_EVENTS_UPSTREAM_ADDR),
            state_upstream: Upstream::trusted_loopback(DEFAULT_STATE_UPSTREAM_ADDR),
            auth_upstream: Upstream::trusted_loopback(DEFAULT_AUTH_UPSTREAM_ADDR),
            enforce_managed_api_keys: false,
        }
    }
}
