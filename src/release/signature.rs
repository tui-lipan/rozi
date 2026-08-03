//! Detached Ed25519 signatures for the exact published manifest bytes.

use super::{MAX_METADATA_SIZE, ReleaseError, Result};
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const SIGNATURE_SCHEMA_VERSION: u32 = 1;
pub const ED25519_ALGORITHM: &str = "ed25519";

/// A detached-signature envelope. Verification is performed over the exact manifest bytes passed
/// to [`verify_manifest_with_keys`], never over a parsed or reserialized JSON value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEnvelope {
    pub schema_version: u32,
    pub signatures: Vec<SignatureEntry>,
}

impl SignatureEnvelope {
    pub fn new(signatures: Vec<SignatureEntry>) -> Self {
        Self {
            schema_version: SIGNATURE_SCHEMA_VERSION,
            signatures,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_METADATA_SIZE {
            return Err(ReleaseError::invalid(format!(
                "release signature envelope exceeds {} bytes",
                MAX_METADATA_SIZE
            )));
        }
        let envelope: Self = serde_json::from_slice(bytes)?;
        if envelope.schema_version != SIGNATURE_SCHEMA_VERSION {
            return Err(ReleaseError::invalid(format!(
                "unsupported release signature schema_version {}",
                envelope.schema_version
            )));
        }
        if envelope.signatures.is_empty() {
            return Err(ReleaseError::invalid(
                "release signature envelope contains no signatures",
            ));
        }
        let mut key_ids = std::collections::HashSet::with_capacity(envelope.signatures.len());
        for entry in &envelope.signatures {
            if entry.key_id.is_empty()
                || entry.algorithm.is_empty()
                || !key_ids.insert(entry.key_id.as_str())
            {
                return Err(ReleaseError::invalid(
                    "signature entries must have unique nonempty key ids and values",
                ));
            }
        }
        Ok(envelope)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        Self::from_bytes(text.as_bytes())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.schema_version != SIGNATURE_SCHEMA_VERSION || self.signatures.is_empty() {
            return Err(ReleaseError::invalid("invalid release signature envelope"));
        }
        Ok(serde_json::to_vec(self)?)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(String::from_utf8(self.to_bytes()?)
            .expect("serde_json emits UTF-8 for a Rust string model"))
    }
}

/// One signature and its claimed key/algorithm identifier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEntry {
    pub key_id: String,
    pub algorithm: String,
    pub signature: String,
}

impl SignatureEntry {
    pub fn new(
        key_id: impl Into<String>,
        algorithm: impl Into<String>,
        signature: impl Into<String>,
    ) -> Self {
        Self {
            key_id: key_id.into(),
            algorithm: algorithm.into(),
            signature: signature.into(),
        }
    }
}

/// An embedded or injected trusted release key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedKey {
    pub id: String,
    pub algorithm: String,
    pub public_key: String,
}

impl TrustedKey {
    pub fn ed25519(id: impl Into<String>, public_key: [u8; 32]) -> Self {
        Self {
            id: id.into(),
            algorithm: ED25519_ALGORITHM.to_string(),
            public_key: base64::engine::general_purpose::STANDARD.encode(public_key),
        }
    }

    pub fn from_public_key_bytes(id: impl Into<String>, public_key: &[u8]) -> Result<Self> {
        let public_key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| ReleaseError::invalid("Ed25519 public keys must contain 32 bytes"))?;
        Ok(Self::ed25519(id, public_key))
    }

    pub fn key_id(&self) -> &str {
        &self.id
    }
}

/// The root shape compiled from `release-keys.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedKeySet {
    pub schema_version: u32,
    pub keys: Vec<TrustedKey>,
}

impl TrustedKeySet {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let key_set: Self = serde_json::from_slice(bytes)?;
        if key_set.schema_version != SIGNATURE_SCHEMA_VERSION {
            return Err(ReleaseError::invalid(format!(
                "unsupported trusted-key schema_version {}",
                key_set.schema_version
            )));
        }
        let mut ids = HashSet::with_capacity(key_set.keys.len());
        for key in &key_set.keys {
            if key.id.is_empty() || !ids.insert(key.id.as_str()) {
                return Err(ReleaseError::invalid(
                    "trusted release key ids must be nonempty and unique",
                ));
            }
            if key.algorithm == ED25519_ALGORITHM {
                decode_public_key(key)?;
            }
        }
        Ok(key_set)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        Self::from_bytes(text.as_bytes())
    }

    pub fn has_ed25519_key(&self) -> bool {
        self.keys
            .iter()
            .any(|key| key.algorithm == ED25519_ALGORITHM)
    }
}

/// The result of a successful signature check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSignature {
    pub key_id: String,
}

/// Parse the release repository's compiled trust anchor.
pub fn compiled_trusted_keys() -> Result<TrustedKeySet> {
    TrustedKeySet::from_bytes(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/release-keys.json"
    )))
}

/// Verify using the compiled trust anchor. An empty `release-keys.json` fails closed.
pub fn verify_manifest(manifest_bytes: &[u8], signature_bytes: &[u8]) -> Result<VerifiedSignature> {
    let keys = compiled_trusted_keys()?;
    verify_manifest_with_keys(manifest_bytes, signature_bytes, &keys.keys)
}

/// Verify using explicitly supplied trusted keys. This is the injection seam for deterministic
/// tests and release tooling that owns a key set separately from the application binary.
pub fn verify_manifest_with_keys(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trusted_keys: &[TrustedKey],
) -> Result<VerifiedSignature> {
    if trusted_keys.is_empty() {
        return Err(ReleaseError::TrustAnchorNotConfigured);
    }
    let key_set = TrustedKeySet {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        keys: trusted_keys.to_vec(),
    };
    if !key_set.has_ed25519_key() {
        return Err(ReleaseError::TrustAnchorNotConfigured);
    }
    let envelope = SignatureEnvelope::from_bytes(signature_bytes)?;
    for entry in &envelope.signatures {
        if entry.algorithm != ED25519_ALGORITHM {
            continue;
        }
        let Some(key) = trusted_keys
            .iter()
            .find(|key| key.id == entry.key_id && key.algorithm == ED25519_ALGORITHM)
        else {
            continue;
        };

        let Ok(public_key) = decode_public_key(key) else {
            continue;
        };
        let Ok(signature_bytes) =
            base64::engine::general_purpose::STANDARD.decode(&entry.signature)
        else {
            continue;
        };
        let Ok(signature) = Signature::try_from(signature_bytes.as_slice()) else {
            continue;
        };
        if public_key.verify_strict(manifest_bytes, &signature).is_ok() {
            return Ok(VerifiedSignature {
                key_id: entry.key_id.clone(),
            });
        }
    }
    Err(ReleaseError::invalid(
        "no trusted Ed25519 signature validates the release manifest",
    ))
}

/// Sign manifest bytes with one Ed25519 key for release tooling.
pub fn sign_manifest(
    manifest_bytes: &[u8],
    key_id: impl Into<String>,
    signing_key: &SigningKey,
) -> SignatureEnvelope {
    let signature = signing_key.sign(manifest_bytes);
    SignatureEnvelope::new(vec![SignatureEntry::new(
        key_id,
        ED25519_ALGORITHM,
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    )])
}

/// Sign and serialize a detached signature envelope.
pub fn sign_manifest_bytes(
    manifest_bytes: &[u8],
    key_id: impl Into<String>,
    signing_key: &SigningKey,
) -> Result<Vec<u8>> {
    sign_manifest(manifest_bytes, key_id, signing_key).to_bytes()
}

fn decode_public_key(key: &TrustedKey) -> Result<VerifyingKey> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&key.public_key)
        .map_err(|error| {
            ReleaseError::invalid(format!("invalid trusted key {}: {error}", key.id))
        })?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        ReleaseError::invalid(format!("trusted Ed25519 key {} is not 32 bytes", key.id))
    })?;
    VerifyingKey::from_bytes(&bytes).map_err(|error| {
        ReleaseError::invalid(format!("invalid trusted Ed25519 key {}: {error}", key.id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn exact_manifest_bytes_are_signed_and_verified() {
        let signing = key(7);
        let trusted = TrustedKey::ed25519("stable", signing.verifying_key().to_bytes());
        let manifest = br#"{"schema_version":1,"version":"1.2.3"}"#;
        let envelope = sign_manifest_bytes(manifest, "stable", &signing).unwrap();
        assert_eq!(
            verify_manifest_with_keys(manifest, &envelope, &[trusted])
                .unwrap()
                .key_id,
            "stable"
        );
        assert!(
            verify_manifest_with_keys(
                b" ",
                &envelope,
                &[TrustedKey::ed25519(
                    "stable",
                    signing.verifying_key().to_bytes()
                )]
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_entries_are_ignored_when_a_trusted_signature_passes() {
        let signing = key(8);
        let valid = sign_manifest(b"manifest", "new", &signing);
        let mut entries = vec![
            SignatureEntry::new("future", "future-ed25519", "not-base64"),
            SignatureEntry::new("missing", ED25519_ALGORITHM, "not-base64"),
        ];
        entries.extend(valid.signatures);
        let envelope = SignatureEnvelope::new(entries).to_bytes().unwrap();
        let trusted = TrustedKey::ed25519("new", signing.verifying_key().to_bytes());
        assert_eq!(
            verify_manifest_with_keys(b"manifest", &envelope, &[trusted])
                .unwrap()
                .key_id,
            "new"
        );
    }

    #[test]
    fn two_key_rotation_accepts_either_key() {
        let old = key(9);
        let new = key(10);
        let trusted = [
            TrustedKey::ed25519("old", old.verifying_key().to_bytes()),
            TrustedKey::ed25519("new", new.verifying_key().to_bytes()),
        ];
        let envelope = sign_manifest_bytes(b"manifest", "old", &old).unwrap();
        assert_eq!(
            verify_manifest_with_keys(b"manifest", &envelope, &trusted)
                .unwrap()
                .key_id,
            "old"
        );
        let envelope = sign_manifest_bytes(b"manifest", "new", &new).unwrap();
        assert_eq!(
            verify_manifest_with_keys(b"manifest", &envelope, &trusted)
                .unwrap()
                .key_id,
            "new"
        );
    }

    #[test]
    fn empty_trust_anchor_fails_closed() {
        let signing = key(11);
        let envelope = sign_manifest_bytes(b"manifest", "any", &signing).unwrap();
        let error = verify_manifest_with_keys(b"manifest", &envelope, &[]).unwrap_err();
        assert!(error.to_string().contains("trust anchor not configured"));
    }

    #[test]
    fn compiled_release_keys_parse_and_empty_sets_fail_closed() {
        let keys = compiled_trusted_keys().unwrap();
        if keys.keys.is_empty() {
            let signing = key(12);
            let envelope = sign_manifest_bytes(b"manifest", "any", &signing).unwrap();
            let error = verify_manifest(b"manifest", &envelope).unwrap_err();
            assert!(error.to_string().contains("trust anchor not configured"));
        }
    }
}
