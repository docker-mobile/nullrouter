//! The internal credential surface must never be reachable from the public port.
//!
//! `nullrouter-state` exposes `/internal/v1/*` endpoints that return
//! **unredacted** provider credentials so `nullrouter-runtime` can execute
//! calls. Those endpoints are safe only because the gateway refuses the prefix
//! outright. This pins that invariant.

use std::net::{IpAddr, Ipv4Addr};

use nullrouter_gateway::{AccessDecision, AccessRequirement, AuthorizationState, RouteKind};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));

/// Every internal endpoint the runtime relies on.
const INTERNAL_PATHS: &[&str] = &[
    "/internal",
    "/internal/v1/credentials/select",
    "/internal/v1/credentials/unavailable",
    "/internal/v1/credentials/clear-error",
    "/internal/v1/credentials/refresh",
    "/internal/v1/usage",
    "/internal/v1/routing-context",
    "/internal/v1/authorize",
];

#[test]
fn internal_paths_are_forbidden_from_every_peer_and_route() {
    for path in INTERNAL_PATHS {
        // Even from loopback, and even when managed keys are off, the public
        // listener must refuse the prefix: loopback services talk to state
        // directly, never through the gateway.
        for peer in [Some(LOOPBACK), Some(REMOTE), None] {
            for route in [
                RouteKind::State,
                RouteKind::Api,
                RouteKind::Runtime,
                RouteKind::Dashboard,
                RouteKind::Auth,
                RouteKind::Catalog,
                RouteKind::Events,
            ] {
                let requirement = AccessRequirement::for_request(path, route, peer, false);
                assert_eq!(
                    requirement,
                    AccessRequirement::Forbidden,
                    "{path} must be forbidden (peer={peer:?}, route={route:?})"
                );
            }
        }
    }
}

#[test]
fn forbidden_internal_paths_resolve_to_a_forbidden_decision() {
    for path in INTERNAL_PATHS {
        let requirement =
            AccessRequirement::for_request(path, RouteKind::State, Some(REMOTE), false);
        // Regardless of authorization state, the decision stays Forbidden: a
        // valid session must not unlock the credential surface.
        for state in [
            AuthorizationState::Authorized,
            AuthorizationState::Denied,
            AuthorizationState::Unavailable,
        ] {
            assert_eq!(
                requirement.decision(state),
                AccessDecision::Forbidden,
                "{path} must stay forbidden under {state:?}"
            );
        }
    }
}

#[test]
fn credential_lookalike_paths_are_not_accidentally_forbidden() {
    // The guard is prefix-based; make sure it does not over-reach into real
    // public routes that merely contain the word.
    for path in [
        "/api/keys",
        "/api/providers",
        "/dashboard/internal-notes",
        "/v1/chat/completions",
    ] {
        let requirement =
            AccessRequirement::for_request(path, RouteKind::State, Some(REMOTE), false);
        assert_ne!(
            requirement,
            AccessRequirement::Forbidden,
            "{path} must not be caught by the internal-path guard"
        );
    }
}
