use nullrouter_contracts::ValidateApiKeyResponse;

use super::model::{ApiKeyRecord, CreatedApiKey, PublicApiKey, digest_secret, generate_secret};
use crate::{
    StoreError,
    store::{StateStore, next_id, timestamp},
};

impl StateStore {
    pub(crate) fn list_keys(&self) -> Result<Vec<PublicApiKey>, StoreError> {
        Ok(self
            .read_snapshot()?
            .api_keys
            .iter()
            .map(ApiKeyRecord::public)
            .collect())
    }

    pub(crate) fn get_key(&self, id: &str) -> Result<Option<PublicApiKey>, StoreError> {
        Ok(self
            .read_snapshot()?
            .api_keys
            .iter()
            .find(|key| key.id == id)
            .map(ApiKeyRecord::public))
    }

    pub(crate) fn create_key(&self, name: String) -> Result<CreatedApiKey, StoreError> {
        let secret = generate_secret()?;
        self.write_snapshot(|snapshot| {
            let record = ApiKeyRecord::new(
                next_id("key", snapshot.api_keys.len()),
                name,
                timestamp(),
                &secret,
            );
            let created = record.created(secret);
            snapshot.api_keys.push(record);
            created
        })
    }

    pub(crate) fn update_key(
        &self,
        id: &str,
        is_active: Option<bool>,
    ) -> Result<Option<PublicApiKey>, StoreError> {
        self.write_snapshot(|snapshot| {
            let key = snapshot.api_keys.iter_mut().find(|key| key.id == id)?;
            if let Some(is_active) = is_active {
                key.is_active = is_active;
            }
            Some(key.public())
        })
    }

    pub(crate) fn delete_key(&self, id: &str) -> Result<bool, StoreError> {
        self.write_snapshot(|snapshot| {
            let original_len = snapshot.api_keys.len();
            snapshot.api_keys.retain(|key| key.id != id);
            snapshot.api_keys.len() != original_len
        })
    }

    pub(crate) fn validate_managed_key(
        &self,
        secret: &str,
    ) -> Result<ValidateApiKeyResponse, StoreError> {
        let candidate = digest_secret(secret);
        // Projected rather than cloned: this runs on every request when `requireApiKey` is on, and
        // the full snapshot carries a 350KB usage log this has no use for.
        //
        // The loop still visits every key after a match instead of breaking early. That is
        // deliberate — an early exit makes the response time depend on the matched key's position,
        // which leaks it — and it is why the match is recorded rather than returned.
        let matched = self.with_snapshot(|snapshot| {
            let mut matched = None;
            for key in &snapshot.api_keys {
                if key.matches_digest(&candidate) {
                    matched = Some((key.id.clone(), key.is_active));
                }
            }
            matched
        })?;
        Ok(match matched {
            Some((key_id, active)) => ValidateApiKeyResponse {
                valid: true,
                active,
                key_id: Some(key_id),
            },
            None => ValidateApiKeyResponse {
                valid: false,
                active: false,
                key_id: None,
            },
        })
    }
}
