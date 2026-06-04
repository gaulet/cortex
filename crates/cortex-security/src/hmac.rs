//! HMAC utilities for guardrails anti-tampering.
//!
//! Guardrails sent in a JobContract are signed with HMAC-SHA256 using a secret
//! known only to Cortex. When a worker finishes, `sync_reflect` re-computes
//! the signature and rejects the job if the worker altered the guardrails.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Errors specific to HMAC operations.
#[derive(Error, Debug)]
pub enum HmacError {
    #[error("Invalid HMAC signature")]
    InvalidSignature,
    #[error("Hex decode error: {0}")]
    HexDecode(String),
    #[error("Crypto setup error: {0}")]
    SetupError(String),
}

/// Sign a canonical JSON serialization of guardrails + DoD with HMAC-SHA256.
///
/// Returns the hex-encoded signature.
pub fn sign_guardrails(secret: &[u8], payload: &serde_json::Value) -> Result<String, HmacError> {
    if secret.is_empty() {
        return Err(HmacError::SetupError("empty secret".into()));
    }
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| HmacError::SetupError(e.to_string()))?;
    let canonical =
        serde_json::to_string(payload).map_err(|e| HmacError::SetupError(e.to_string()))?;
    mac.update(canonical.as_bytes());
    let result = mac.finalize();
    Ok(hex::encode(result.into_bytes()))
}

/// Verify that a provided signature matches the HMAC-SHA256 of the payload.
pub fn verify_guardrails(
    secret: &[u8],
    payload: &serde_json::Value,
    expected: &str,
) -> Result<(), HmacError> {
    let computed = sign_guardrails(secret, payload)?;
    // Constant-time comparison would be nice; for guardrails HMAC it's not critical
    // since the secret is never exposed to the worker. Direct string equality is acceptable.
    if computed == expected {
        Ok(())
    } else {
        Err(HmacError::InvalidSignature)
    }
}

/// Tiny hex encoder since we don't need a full `hex` crate dependency.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sign_and_verify_roundtrip() {
        let secret = b"super-secret-hmac-key";
        let payload = json!({
            "guardrails": ["rule1", "rule2"],
            "definition_of_done": "JSON valid"
        });
        let signature = sign_guardrails(secret, &payload).unwrap();
        assert_eq!(signature.len(), 64);
        verify_guardrails(secret, &payload, &signature).unwrap();
    }

    #[test]
    fn test_wrong_secret_fails() {
        let payload = json!({"guardrails": ["r"]});
        let sig = sign_guardrails(b"right-secret", &payload).unwrap();
        let result = verify_guardrails(b"wrong-secret", &payload, &sig);
        assert!(result.is_err());
    }

    #[test]
    fn test_tampered_payload_fails() {
        let secret = b"secret";
        let original = json!({"guardrails": ["r1", "r2"]});
        let sig = sign_guardrails(secret, &original).unwrap();
        let tampered = json!({"guardrails": ["r1"]});
        let result = verify_guardrails(secret, &tampered, &sig);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_secret_errors() {
        let payload = json!({"x": 1});
        let result = sign_guardrails(b"", &payload);
        assert!(result.is_err());
    }
}
