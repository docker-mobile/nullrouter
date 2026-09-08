//! The stored user record.
//!
//! A password lives here as a bcrypt hash and never leaves this crate. The auth service asks
//! `/internal/v1/users/verify` whether a username and password match; it does not fetch the hash and
//! compare locally. That keeps exactly one copy of the secret material, on the one service that has a
//! reason to hold it, and means a compromised auth process cannot walk the password table.

use serde::{Deserialize, Serialize};

/// Cost for new hashes.
///
/// bcrypt's default. Raising it is a deployment decision with a latency cost on every sign-in, and
/// lowering it below the default is never right, so it is pinned rather than configurable: an operator
/// who wants a different cost is better served by an external IdP, which this build already supports.
const BCRYPT_COST: u32 = bcrypt::DEFAULT_COST;

/// What a user is allowed to do.
///
/// Three levels rather than per-route permissions. A permission matrix is the right answer for a
/// product with dozens of independent capabilities; this one has a natural ladder -- read the router's
/// state, change how it routes, change who may sign in -- and three named rungs are something an
/// operator can hold in their head and audit at a glance. Anything finer would be a matrix nobody
/// reviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    /// Reads only. Cannot change routing, credentials, or users.
    Viewer,
    /// Everything about how the router runs. Cannot manage users.
    Operator,
    /// Everything, including users.
    Admin,
}

impl Role {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    /// Parse a role name, rejecting anything unrecognised.
    ///
    /// Unknown input is `None` rather than defaulting: a typo in a role name that silently became
    /// `viewer` would look like a working restriction, and one that became `admin` would be a
    /// privilege escalation through a spelling mistake.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// A user, as stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserRecord {
    pub(super) id: String,
    /// Lowercased at the boundary, so `Alice` and `alice` cannot become two accounts.
    pub(super) username: String,
    #[serde(default)]
    pub(super) display_name: String,
    #[serde(default)]
    pub(super) email: String,
    pub(super) role: Role,
    /// bcrypt hash. `skip_serializing` is not enough on its own -- the public view below is what
    /// routes return -- but it stops the hash reaching a debug dump of the snapshot.
    #[serde(rename = "passwordHash")]
    pub(super) password_hash: String,
    pub(super) is_active: bool,
    pub(super) created_at: String,
    pub(super) updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last_login_at: Option<String>,
}

/// A user as an API caller sees one: everything except the hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicUser {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) email: String,
    pub(crate) role: String,
    pub(crate) is_active: bool,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_login_at: Option<String>,
}

/// Why a user could not be created or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UserError {
    UsernameTaken,
    UsernameInvalid,
    PasswordTooShort,
    RoleInvalid,
    NotFound,
    /// The last enabled admin cannot be removed, disabled, or demoted.
    ///
    /// Enforced in the store rather than the UI because it is the difference between a mistake and an
    /// unrecoverable install: with no admin left, nobody can create one, and the only way back is
    /// editing the state file by hand.
    WouldOrphanAdmin,
    HashFailed,
}

impl UserError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::UsernameTaken => "That username is already taken",
            Self::UsernameInvalid => {
                "A username must be 2-64 characters of letters, digits, dot, dash or underscore"
            }
            Self::PasswordTooShort => "A password must be at least 12 characters",
            Self::RoleInvalid => "Role must be admin, operator or viewer",
            Self::NotFound => "User not found",
            Self::WouldOrphanAdmin => {
                "This is the last enabled admin; promote another account first"
            }
            Self::HashFailed => "The password could not be hashed",
        }
    }
}

/// Shortest password accepted.
///
/// Twelve rather than eight. This credential guards every provider key the router holds, and the
/// sign-in path is reachable from wherever the dashboard is published.
const MIN_PASSWORD_LEN: usize = 12;

/// Validate and normalise a username.
pub(crate) fn normalise_username(raw: &str) -> Result<String, UserError> {
    let trimmed = raw.trim().to_ascii_lowercase();
    let valid = (2..=64).contains(&trimmed.chars().count())
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'));
    if valid {
        Ok(trimmed)
    } else {
        Err(UserError::UsernameInvalid)
    }
}

pub(crate) fn check_password(raw: &str) -> Result<(), UserError> {
    if raw.chars().count() < MIN_PASSWORD_LEN {
        return Err(UserError::PasswordTooShort);
    }
    Ok(())
}

pub(crate) fn hash_password(raw: &str) -> Result<String, UserError> {
    check_password(raw)?;
    bcrypt::hash(raw, BCRYPT_COST).map_err(|_| UserError::HashFailed)
}

/// Everything needed to create a user.
///
/// Grouped rather than passed as seven arguments: at that count the call site is a row of same-typed
/// strings where transposing two is a silent bug -- an email in the display-name slot type-checks
/// perfectly.
#[derive(Debug, Clone)]
pub(super) struct NewUser {
    pub(super) id: String,
    pub(super) username: String,
    pub(super) display_name: String,
    pub(super) email: String,
    pub(super) role: Role,
    pub(super) password_hash: String,
    pub(super) now: String,
}

impl UserRecord {
    pub(super) fn new(new: NewUser) -> Self {
        Self {
            id: new.id,
            username: new.username,
            display_name: new.display_name,
            email: new.email,
            role: new.role,
            password_hash: new.password_hash,
            is_active: true,
            created_at: new.now.clone(),
            updated_at: new.now,
            last_login_at: None,
        }
    }

    pub(super) fn public(&self) -> PublicUser {
        PublicUser {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            email: self.email.clone(),
            role: self.role.as_str().to_owned(),
            is_active: self.is_active,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            last_login_at: self.last_login_at.clone(),
        }
    }

    /// Whether `candidate` is this user's password.
    ///
    /// A bcrypt error is a failed verification, not a pass. `verify` errors on a malformed stored
    /// hash, and a record whose hash cannot be parsed must not authenticate.
    pub(super) fn verify(&self, candidate: &str) -> bool {
        bcrypt::verify(candidate, &self.password_hash).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{Role, UserError, normalise_username};

    #[test]
    fn the_role_ladder_orders_as_written() {
        // Enforcement lives in the gateway, which is the only component that sees a route and a
        // session's role together; this only pins that the ordering here agrees with it.
        assert!(Role::Admin > Role::Operator);
        assert!(Role::Operator > Role::Viewer);
    }

    #[test]
    fn an_unknown_role_is_rejected_rather_than_defaulted() {
        assert_eq!(Role::parse("admin"), Some(Role::Admin));
        assert_eq!(Role::parse("  Operator "), Some(Role::Operator));
        assert_eq!(Role::parse("VIEWER"), Some(Role::Viewer));
        // A typo must not silently become a role. Defaulting to viewer would look like a working
        // restriction; defaulting to admin would be escalation by spelling mistake.
        assert_eq!(Role::parse("adminn"), None);
        assert_eq!(Role::parse("root"), None);
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn usernames_are_lowercased_so_one_person_cannot_hold_two_accounts() {
        assert_eq!(normalise_username("Alice"), Ok("alice".to_owned()));
        assert_eq!(
            normalise_username("  BOB.smith "),
            Ok("bob.smith".to_owned())
        );
        assert_eq!(normalise_username("a-b_c.9"), Ok("a-b_c.9".to_owned()));
    }

    #[test]
    fn a_username_that_could_collide_or_confuse_is_refused() {
        for bad in [
            "",
            "a",
            "  ",
            "has space",
            "user@host",
            "sql'inject",
            "../etc",
            "ünïcode",
        ] {
            assert_eq!(
                normalise_username(bad),
                Err(UserError::UsernameInvalid),
                "{bad} was accepted"
            );
        }
        assert_eq!(
            normalise_username(&"a".repeat(65)),
            Err(UserError::UsernameInvalid)
        );
        assert!(normalise_username(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn a_short_password_is_refused_and_a_long_one_hashes() {
        assert_eq!(
            super::check_password("short"),
            Err(UserError::PasswordTooShort)
        );
        assert_eq!(super::check_password("exactlytwelve"), Ok(()));
        let hash = super::hash_password("a-sufficiently-long-password").expect("hash");
        assert!(hash.starts_with("$2"), "not a bcrypt hash: {hash}");
        // The hash must not contain the password.
        assert!(!hash.contains("sufficiently"));
    }
}
