//! `NULLROUTER_ENV=production` refuses the two settings that are safe on a laptop and
//! indefensible on a deployed instance.
//!
//! Serialised through one mutex and run in one test: `from_env` reads process-wide state, so two
//! cases mutating it concurrently would see each other's variables. Every variable is restored
//! afterwards, because a leaked one changes the answer for whatever runs next.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "helpers in an integration test are not covered by clippy.toml's allow-expect-in-tests"
)]

use nullrouter_auth::{AuthConfig, AuthConfigError};

const VARS: &[&str] = &[
    "NULLROUTER_ENV",
    "NULLROUTER_AUTH_SESSION_SECRET",
    "NULLROUTER_AUTH_PASSWORD_HASH",
    "INITIAL_PASSWORD",
    "AUTH_COOKIE_SECURE",
];

/// Save every variable, clear them, and restore on drop.
struct Env {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Env {
    fn new() -> Self {
        let saved = VARS
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for name in VARS {
            // SAFETY: this test binary runs these cases in one thread, in one test function.
            unsafe { std::env::remove_var(name) };
        }
        Self { saved }
    }

    fn set(&self, name: &str, value: &str) {
        // SAFETY: as above.
        unsafe { std::env::set_var(name, value) };
    }

    fn clear(&self, name: &str) {
        // SAFETY: as above.
        unsafe { std::env::remove_var(name) };
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            match value {
                // SAFETY: as above.
                Some(previous) => unsafe { std::env::set_var(name, previous) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

#[test]
fn production_refuses_what_development_tolerates() {
    let env = Env::new();

    // Given: no production assertion. Both weak settings are accepted, because this is a laptop.
    let dev =
        AuthConfig::from_env().expect("a development instance must start with no configuration");
    assert!(
        !dev.secure_cookie(),
        "a dev instance serves over plaintext HTTP, so a secure-only cookie would never arrive"
    );

    // When: the operator asserts production but sets no session secret.
    env.set("NULLROUTER_ENV", "production");
    env.set("INITIAL_PASSWORD", "a-real-password");
    assert!(
        matches!(
            AuthConfig::from_env(),
            Err(AuthConfigError::EphemeralSecretInProduction)
        ),
        "a random per-boot secret signs every operator out on restart"
    );

    // And when a secret is set but the password is the built-in default.
    env.set(
        "NULLROUTER_AUTH_SESSION_SECRET",
        "0123456789abcdef0123456789abcdef",
    );
    env.set("INITIAL_PASSWORD", "123456");
    assert!(
        matches!(
            AuthConfig::from_env(),
            Err(AuthConfigError::DefaultPasswordInProduction)
        ),
        "pasting the documented example is not choosing a password"
    );

    // Nor does clearing it help: absent means the default is in force.
    env.clear("INITIAL_PASSWORD");
    assert!(matches!(
        AuthConfig::from_env(),
        Err(AuthConfigError::DefaultPasswordInProduction)
    ));

    // Then: with both set properly it starts, and the cookie is secure without being asked.
    env.set("INITIAL_PASSWORD", "a-real-password");
    let prod = AuthConfig::from_env().expect("a configured production instance must start");
    assert!(
        prod.secure_cookie(),
        "production must not send a session cookie over plaintext HTTP by default"
    );

    // And an operator terminating TLS at a proxy can still turn it off deliberately.
    env.set("AUTH_COOKIE_SECURE", "false");
    let behind_proxy = AuthConfig::from_env().expect("an explicit override must be honoured");
    assert!(!behind_proxy.secure_cookie());
}
