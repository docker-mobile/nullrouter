//! User CRUD against the snapshot.
//!
//! Every mutation goes through `write_snapshot`, so a change is on disk before the request is
//! answered. A user created and then lost to a restart is worse than one that failed to be created:
//! the operator has been told a credential exists.

use super::model::{
    NewUser, PublicUser, Role, UserError, UserRecord, hash_password, normalise_username,
};
use crate::{
    StoreError,
    store::{StateStore, next_id, timestamp},
};

/// A verified sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedUser {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) role: &'static str,
}

/// What to change about a user. `None` leaves a field alone.
#[derive(Debug, Clone, Default)]
pub(crate) struct UserUpdate {
    pub(crate) display_name: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) role: Option<Role>,
    pub(crate) is_active: Option<bool>,
    pub(crate) password: Option<String>,
}

impl StateStore {
    pub(crate) fn list_users(&self) -> Result<Vec<PublicUser>, StoreError> {
        let mut users: Vec<PublicUser> = self
            .read_snapshot()?
            .users
            .iter()
            .map(UserRecord::public)
            .collect();
        users.sort_by(|left, right| left.username.cmp(&right.username));
        Ok(users)
    }

    pub(crate) fn get_user(&self, id: &str) -> Result<Option<PublicUser>, StoreError> {
        Ok(self
            .read_snapshot()?
            .users
            .iter()
            .find(|user| user.id == id)
            .map(UserRecord::public))
    }

    /// Whether any user exists at all.
    ///
    /// This is what decides whether the legacy shared password is still accepted: an install with no
    /// users has no other way in, and one with users has a real account for every person who should
    /// have access.
    pub(crate) fn has_users(&self) -> Result<bool, StoreError> {
        Ok(!self.read_snapshot()?.users.is_empty())
    }

    pub(crate) fn create_user(
        &self,
        username: &str,
        password: &str,
        display_name: Option<String>,
        email: Option<String>,
        role: Role,
    ) -> Result<Result<PublicUser, UserError>, StoreError> {
        let username = match normalise_username(username) {
            Ok(username) => username,
            Err(error) => return Ok(Err(error)),
        };
        // Hashed before taking the write lock: bcrypt at the default cost takes hundreds of
        // milliseconds, and holding the snapshot lock through it would stall every other reader.
        let hash = match hash_password(password) {
            Ok(hash) => hash,
            Err(error) => return Ok(Err(error)),
        };
        self.write_snapshot(|snapshot| {
            if snapshot.users.iter().any(|user| user.username == username) {
                return Err(UserError::UsernameTaken);
            }
            let display = display_name
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| username.clone());
            let record = UserRecord::new(NewUser {
                id: next_id("user", snapshot.users.len()),
                username,
                display_name: display,
                email: email
                    .map(|value| value.trim().to_owned())
                    .unwrap_or_default(),
                role,
                password_hash: hash,
                now: timestamp(),
            });
            let public = record.public();
            snapshot.users.push(record);
            Ok(public)
        })
    }

    pub(crate) fn update_user(
        &self,
        id: &str,
        update: UserUpdate,
    ) -> Result<Result<PublicUser, UserError>, StoreError> {
        // As in `create_user`: hash outside the lock.
        let hash = match update.password.as_deref().map(hash_password) {
            Some(Ok(hash)) => Some(hash),
            Some(Err(error)) => return Ok(Err(error)),
            None => None,
        };
        self.write_snapshot(|snapshot| {
            // Checked before the mutation, because it has to consider the state the change would
            // produce rather than the one it starts from.
            let losing_admin = |snapshot: &crate::store::StateSnapshot| -> bool {
                let target = snapshot.users.iter().find(|user| user.id == id);
                let Some(target) = target else { return false };
                // Only a change to an account that is *currently* an active admin can orphan the
                // install. Without this the guard also fired on disabling an operator, which cannot
                // remove an admin and was refused anyway.
                if target.role != Role::Admin || !target.is_active {
                    return false;
                }
                let still_admin = match (update.role, update.is_active) {
                    (Some(role), Some(active)) => role == Role::Admin && active,
                    (Some(role), None) => role == Role::Admin,
                    (None, Some(active)) => active,
                    (None, None) => true,
                };
                if still_admin {
                    return false;
                }
                // The change removes this account's admin standing. Refuse when it is the only one
                // left, or the install becomes unadministrable.
                !snapshot
                    .users
                    .iter()
                    .any(|user| user.id != id && user.role == Role::Admin && user.is_active)
            };
            if losing_admin(snapshot) {
                return Err(UserError::WouldOrphanAdmin);
            }

            let Some(user) = snapshot.users.iter_mut().find(|user| user.id == id) else {
                return Err(UserError::NotFound);
            };
            if let Some(display_name) = update.display_name {
                let trimmed = display_name.trim();
                if !trimmed.is_empty() {
                    user.display_name = trimmed.to_owned();
                }
            }
            if let Some(email) = update.email {
                user.email = email.trim().to_owned();
            }
            if let Some(role) = update.role {
                user.role = role;
            }
            if let Some(is_active) = update.is_active {
                user.is_active = is_active;
            }
            if let Some(hash) = hash {
                user.password_hash = hash;
            }
            user.updated_at = timestamp();
            Ok(user.public())
        })
    }

    pub(crate) fn delete_user(&self, id: &str) -> Result<Result<(), UserError>, StoreError> {
        self.write_snapshot(|snapshot| {
            let Some(target) = snapshot.users.iter().find(|user| user.id == id) else {
                return Err(UserError::NotFound);
            };
            if target.role == Role::Admin
                && target.is_active
                && !snapshot
                    .users
                    .iter()
                    .any(|user| user.id != id && user.role == Role::Admin && user.is_active)
            {
                return Err(UserError::WouldOrphanAdmin);
            }
            snapshot.users.retain(|user| user.id != id);
            Ok(())
        })
    }

    /// Check a username and password, and record the sign-in when it matches.
    ///
    /// Returns `None` for a wrong password, an unknown username, and a disabled account alike. The
    /// caller gets one answer for all three on purpose: distinguishing them turns the sign-in form
    /// into a way to enumerate which accounts exist.
    ///
    /// A disabled account fails even with the right password. That is what "disabled" has to mean, or
    /// it is a label rather than a control.
    pub(crate) fn verify_user(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<VerifiedUser>, StoreError> {
        let username = username.trim().to_ascii_lowercase();
        // Read first so the bcrypt comparison happens outside the write lock; only a match takes it.
        let candidate = self
            .read_snapshot()?
            .users
            .into_iter()
            .find(|user| user.username == username);
        let Some(candidate) = candidate else {
            return Ok(None);
        };
        if !candidate.is_active || !candidate.verify(password) {
            return Ok(None);
        }
        let verified = VerifiedUser {
            id: candidate.id.clone(),
            username: candidate.username.clone(),
            display_name: candidate.display_name.clone(),
            role: candidate.role.as_str(),
        };
        // Deferred rather than durable: a last-login stamp is not worth a synchronous disk write on
        // the sign-in path, and losing the most recent one to a crash costs nothing.
        self.write_snapshot_deferred(|snapshot| {
            if let Some(user) = snapshot
                .users
                .iter_mut()
                .find(|user| user.id == candidate.id)
            {
                user.last_login_at = Some(timestamp());
            }
        })?;
        Ok(Some(verified))
    }
}
