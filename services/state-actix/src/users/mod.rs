//! Managed users: the accounts that can sign in to the dashboard.
//!
//! This replaces a single shared password. That password is still accepted, but only while no user
//! exists -- otherwise every install would lock itself out on upgrade, and a migration that locks the
//! operator out of their own router is not one. Once an account exists, the shared password stops
//! working, so the switch happens the moment there is another way in and not before.
//!
//! Passwords are bcrypt hashes and never leave this crate: the auth service asks
//! [`routes::INTERNAL_VERIFY_PATH`] whether a credential matches rather than fetching a hash to
//! compare. One copy of the secret material, on the service whose job is holding it.

mod model;
mod routes;
mod store;

pub(crate) use model::UserRecord;
pub(crate) use routes::configure;
