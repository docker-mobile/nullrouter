use bcrypt::verify;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::PasswordConfig;

pub(crate) struct PasswordVerifier {
    password: PasswordConfig,
}

impl PasswordVerifier {
    pub(crate) const fn new(password: PasswordConfig) -> Self {
        Self { password }
    }

    pub(crate) fn verify(&self, candidate: &str) -> bool {
        match &self.password {
            PasswordConfig::BcryptHash(hash) => verify(candidate, hash).unwrap_or(false),
            PasswordConfig::Plaintext(expected) => {
                let candidate_digest = Sha256::digest(candidate.as_bytes());
                let expected_digest = Sha256::digest(expected.as_bytes());
                bool::from(candidate_digest.ct_eq(&expected_digest))
            }
        }
    }
}
