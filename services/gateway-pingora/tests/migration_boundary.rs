//! The 9Router import must require a dashboard session.
//!
//! It reads provider credentials out of a local 9Router install and writes them
//! into this router's state. Reachable unauthenticated, it would let anyone on
//! the network trigger a credential import.

use std::net::{IpAddr, Ipv4Addr};

use nullrouter_gateway::{AccessDecision, AccessRequirement, AuthorizationState, GatewayConfig};

const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

const MIGRATE_PATH: &str = "/api/migrate/9router";

#[test]
fn migration_is_routed_to_the_api_service() {
    let config = GatewayConfig::default();
    assert_eq!(
        config.route_for_path(MIGRATE_PATH),
        nullrouter_gateway::RouteKind::Api,
        "the import lives on the API service"
    );
}

#[test]
fn migration_requires_a_session_not_public_access() {
    for peer in [Some(REMOTE), Some(LOOPBACK), None] {
        let requirement = AccessRequirement::for_request(
            MIGRATE_PATH,
            nullrouter_gateway::RouteKind::Api,
            peer,
            false,
        );
        assert_eq!(
            requirement,
            AccessRequirement::ApiSession,
            "import must be session-gated (peer={peer:?})"
        );
        // Without authorization it must not be allowed through.
        assert_eq!(
            requirement.decision(AuthorizationState::Denied),
            AccessDecision::Unauthorized
        );
        assert_eq!(
            requirement.decision(AuthorizationState::Unavailable),
            AccessDecision::Unauthorized,
            "an auth outage must fail closed"
        );
        // With a valid session it proceeds.
        assert_eq!(
            requirement.decision(AuthorizationState::Authorized),
            AccessDecision::Allow
        );
    }
}

#[test]
fn the_internal_import_endpoint_stays_unreachable_publicly() {
    // The API service forwards to this; it must never be callable directly.
    let requirement = AccessRequirement::for_request(
        "/internal/v1/migrate/9router",
        nullrouter_gateway::RouteKind::State,
        Some(REMOTE),
        false,
    );
    assert_eq!(requirement, AccessRequirement::Forbidden);
    assert_eq!(
        requirement.decision(AuthorizationState::Authorized),
        AccessDecision::Forbidden,
        "even an authorized session must not reach the internal endpoint"
    );
}
