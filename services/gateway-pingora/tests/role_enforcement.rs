//! Roles are enforced at the gateway, which is the only place they can be.
//!
//! It is the one component that sees a request's route and its session's role at the same moment. A
//! check inside each service would be seven implementations to keep in agreement, and the one that
//! drifted would be the hole.
//!
//! The cases here are about the three rules and the two principals that carry no role. That second
//! part matters more than it looks: an absent role has to read as full access, because `/v1` API keys
//! and every session minted before managed users existed both have none, and reducing them would break
//! every un-migrated install at once.

use http::Method;
use nullrouter_gateway::{
    AccessDecision, AccessRequirement, AuthorizationState, GatewayConfig, PrincipalRole,
};

/// A loopback peer. Constructed rather than parsed so the helper cannot fail.
const LOOPBACK: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// The requirement for one request, as the gateway computes it.
fn requirement(path: &str, method: &Method) -> AccessRequirement {
    GatewayConfig::default().access_requirement(path, method, Some(LOOPBACK))
}

/// The decision for one request made by a principal holding `role`.
fn decision(path: &str, method: &Method, role: PrincipalRole) -> AccessDecision {
    requirement(path, method).decision(AuthorizationState::Authorized { role: Some(role) })
}

#[test]
fn user_management_is_admin_only() {
    // Managing who can sign in is the one capability that can grant every other, so it is the one
    // place where operator is not enough.
    for path in ["/api/users", "/api/users/user_1"] {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            assert_eq!(
                decision(path, &method, PrincipalRole::Admin),
                AccessDecision::Allow,
                "admin was refused {method} {path}"
            );
            for role in [PrincipalRole::Operator, PrincipalRole::Viewer] {
                assert_eq!(
                    decision(path, &method, role),
                    AccessDecision::Forbidden,
                    "{} reached {method} {path}",
                    role.as_str()
                );
            }
        }
    }
}

#[test]
fn a_viewer_can_read_the_router_but_not_change_it() {
    // Given: a viewer is a read-only principal.
    for path in [
        "/api/providers",
        "/api/models",
        "/api/keys",
        "/api/settings",
    ] {
        // A read is allowed.
        assert_eq!(
            decision(path, &Method::GET, PrincipalRole::Viewer),
            AccessDecision::Allow,
            "a viewer could not read {path}"
        );
        // Every write is not. A viewer that could change routing is not a viewer.
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert_eq!(
                decision(path, &method, PrincipalRole::Viewer),
                AccessDecision::Forbidden,
                "a viewer reached {method} {path}"
            );
        }
    }
}

#[test]
fn an_operator_can_change_routing_but_not_users() {
    for path in ["/api/providers", "/api/combos", "/api/models/disabled"] {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            assert_eq!(
                decision(path, &method, PrincipalRole::Operator),
                AccessDecision::Allow,
                "an operator was refused {method} {path}"
            );
        }
    }
    assert_eq!(
        decision("/api/users", &Method::POST, PrincipalRole::Operator),
        AccessDecision::Forbidden
    );
}

#[test]
fn a_preflight_is_treated_as_a_read() {
    // A CORS preflight carries no body and changes nothing. Refusing it makes the browser report a
    // network error instead of the 403 the real request would return, which is strictly less
    // informative to whoever has to debug it.
    assert_eq!(
        decision("/api/providers", &Method::OPTIONS, PrincipalRole::Viewer),
        AccessDecision::Allow
    );
}

#[test]
fn a_principal_with_no_role_keeps_full_access() {
    // Two of these exist and both must keep working: a `/v1` API key, which has no dashboard role at
    // all, and a dashboard session minted before managed users existed. An install that upgrades has
    // only the second kind until someone signs in again.
    let state = AuthorizationState::Authorized { role: None };
    for (path, method) in [
        ("/api/providers", Method::POST),
        ("/api/users", Method::POST),
        ("/api/settings", Method::PUT),
    ] {
        assert_eq!(
            requirement(path, &method).decision(state),
            AccessDecision::Allow,
            "a legacy session was refused {method} {path}"
        );
    }
}

#[test]
fn an_insufficient_role_is_forbidden_rather_than_unauthorized() {
    // The distinction is not cosmetic. A 401 sends the dashboard to the login screen, and signing in
    // again as the same account produces the same 401 -- a loop the user cannot escape. A 403 says the
    // session is fine and this account cannot do this.
    assert_eq!(
        decision("/api/users", &Method::GET, PrincipalRole::Viewer),
        AccessDecision::Forbidden
    );
    // Whereas no valid session at all is still a 401.
    assert_eq!(
        requirement("/api/users", &Method::GET).decision(AuthorizationState::Denied),
        AccessDecision::Unauthorized
    );
}

#[test]
fn an_unknown_role_reads_as_the_least_privilege() {
    // A token minted by a newer build could name a role this one has never heard of. The safe reading
    // of an unknown claim is the least privilege: one that read as admin would be an escalation
    // delivered by a version skew.
    assert_eq!(PrincipalRole::parse("admin"), PrincipalRole::Admin);
    assert_eq!(PrincipalRole::parse("operator"), PrincipalRole::Operator);
    assert_eq!(PrincipalRole::parse("viewer"), PrincipalRole::Viewer);
    assert_eq!(PrincipalRole::parse("superuser"), PrincipalRole::Viewer);
    assert_eq!(PrincipalRole::parse("root"), PrincipalRole::Viewer);
    assert_eq!(PrincipalRole::parse(""), PrincipalRole::Viewer);
}

#[test]
fn the_runtime_path_is_not_role_gated() {
    // `/v1` is inference traffic authenticated by API key. It must not acquire a role requirement: an
    // API key carries no role, and a rule that demanded one would refuse every inference request.
    let requirement = requirement("/v1/chat/completions", &Method::POST);
    assert!(
        !matches!(requirement, AccessRequirement::ApiSession { .. }),
        "the runtime path acquired a session role requirement: {requirement:?}"
    );
}
