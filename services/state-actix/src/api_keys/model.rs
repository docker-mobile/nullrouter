use std::fmt;

use nullrouter_contracts::SecretString;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::StoreError;

pub(crate) const PUBLIC_KEY_MASK: &str = "nr_nullrouter_state_...redacted";
const SECRET_PREFIX: &str = "nr_nullrouter_state_";
const RANDOM_BYTES: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiKeyRecord {
    pub(super) id: String,
    #[serde(default, rename = "key", skip_serializing)]
    pub(super) legacy_key: Option<SecretString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) verification: Option<VerificationRecord>,
    pub(super) name: String,
    pub(super) machine_id: String,
    pub(super) is_active: bool,
    pub(super) created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VerificationRecord {
    algorithm: VerificationAlgorithm,
    digest: [u8; RANDOM_BYTES],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum VerificationAlgorithm {
    #[serde(rename = "sha256-v1")]
    Sha256V1,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicApiKey {
    id: String,
    key: &'static str,
    name: String,
    machine_id: String,
    is_active: bool,
    created_at: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatedApiKey {
    id: String,
    key: String,
    name: String,
    machine_id: String,
    is_active: bool,
    created_at: String,
}

impl fmt::Debug for CreatedApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreatedApiKey")
            .field("id", &self.id)
            .field("key", &"[REDACTED]")
            .field("name", &self.name)
            .field("machine_id", &self.machine_id)
            .field("is_active", &self.is_active)
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl ApiKeyRecord {
    pub(super) fn new(id: String, name: String, created_at: String, secret: &str) -> Self {
        Self {
            id,
            legacy_key: None,
            verification: Some(VerificationRecord::for_secret(secret)),
            name,
            machine_id: "nullrouter-state".to_owned(),
            is_active: true,
            created_at,
        }
    }

    pub(super) fn public(&self) -> PublicApiKey {
        PublicApiKey {
            id: self.id.clone(),
            key: PUBLIC_KEY_MASK,
            name: self.name.clone(),
            machine_id: self.machine_id.clone(),
            is_active: self.is_active,
            created_at: self.created_at.clone(),
        }
    }

    pub(super) fn created(&self, secret: String) -> CreatedApiKey {
        CreatedApiKey {
            id: self.id.clone(),
            key: secret,
            name: self.name.clone(),
            machine_id: self.machine_id.clone(),
            is_active: self.is_active,
            created_at: self.created_at.clone(),
        }
    }

    pub(super) fn matches_digest(&self, candidate: &[u8; RANDOM_BYTES]) -> bool {
        self.verification
            .as_ref()
            .is_some_and(|verification| verification.matches_digest(candidate))
    }
}

impl VerificationRecord {
    fn for_secret(secret: &str) -> Self {
        Self {
            algorithm: VerificationAlgorithm::Sha256V1,
            digest: digest_secret(secret),
        }
    }

    fn matches_digest(&self, candidate: &[u8; RANDOM_BYTES]) -> bool {
        match self.algorithm {
            VerificationAlgorithm::Sha256V1 => bool::from(self.digest.ct_eq(candidate)),
        }
    }
}

pub(crate) fn migrate_legacy_records(records: &mut [ApiKeyRecord]) -> bool {
    let mut migrated = false;
    for record in records {
        if let Some(secret) = record.legacy_key.take() {
            if record.verification.is_none() {
                record.verification = Some(VerificationRecord::for_secret(secret.expose_secret()));
            }
            migrated = true;
        }
    }
    migrated
}

pub(super) fn generate_secret() -> Result<String, StoreError> {
    let mut random = [0_u8; RANDOM_BYTES];
    getrandom::fill(&mut random).map_err(|_| StoreError::Random)?;
    let mut secret = String::with_capacity(SECRET_PREFIX.len() + (RANDOM_BYTES * 2));
    secret.push_str(SECRET_PREFIX);
    for byte in random {
        secret.push(hex_digit(byte >> 4));
        secret.push(hex_digit(byte & 0x0f));
    }
    random.fill(0);
    Ok(secret)
}

pub(crate) fn digest_secret(secret: &str) -> [u8; RANDOM_BYTES] {
    Sha256::digest(secret.as_bytes()).into()
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'a' + (value - 10)),
        _ => '0',
    }
}
