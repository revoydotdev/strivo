//! Verification of machine-bound ES256 licence tokens.

use anyhow::{bail, Context, Result};
use base64::Engine;
use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct VerifiedClaims {
    pub iss: String,
    pub sub: String,
    pub tier: String,
    pub exp: i64,
    #[serde(default)]
    pub licence_exp: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    typ: Option<String>,
}

/// Why a token was refused.
///
/// Split out because "these dates have run out" and "this token is not
/// signed for us" are different facts with different consequences: the
/// first is renewable by a refresh, the second never is. The reconnection
/// classifier needs the distinction, and a caller that only has an
/// `anyhow::Error` would have to sniff its message for the word "expired".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The token's own `exp`, or the licence expiry it carries, has passed.
    Expired,
    /// Anything else: bad signature, wrong issuer, wrong machine, unsupported
    /// tier, malformed JWT.
    Untrusted,
}

/// A verification failure, with the reason and the detail kept apart.
#[derive(Debug)]
pub struct VerifyError {
    pub kind: Refusal,
    pub detail: String,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for VerifyError {}

/// Verify an activation token against the configured public key, issuer,
/// machine binding, expiry, and tier.
pub fn verify_token(token: &str) -> Result<VerifiedClaims> {
    let issuer = super::client::backend_url().context("STRIVO_LICENCE_URL unset")?;
    let public_key =
        std::env::var("STRIVO_LICENCE_PUBLIC_KEY").context("STRIVO_LICENCE_PUBLIC_KEY unset")?;
    verify_token_with(
        token,
        &public_key,
        &issuer,
        &super::machine_id::hashed_machine_id(),
    )
}

pub fn verify_token_with(
    token: &str,
    public_key_pem: &str,
    expected_issuer: &str,
    expected_machine: &str,
) -> Result<VerifiedClaims> {
    verify_token_detailed(token, public_key_pem, expected_issuer, expected_machine)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// [`verify_token_with`] with the refusal reason preserved instead of
/// flattened into a message. Every check that fails for a reason other than
/// expiry is [`Refusal::Untrusted`].
pub fn verify_token_detailed(
    token: &str,
    public_key_pem: &str,
    expected_issuer: &str,
    expected_machine: &str,
) -> std::result::Result<VerifiedClaims, VerifyError> {
    let untrusted = |detail: &str| VerifyError {
        kind: Refusal::Untrusted,
        detail: detail.to_string(),
    };
    let expired = |detail: &str| VerifyError {
        kind: Refusal::Expired,
        detail: detail.to_string(),
    };
    let malformed = |e: anyhow::Error| VerifyError {
        kind: Refusal::Untrusted,
        detail: format!("{e:#}"),
    };

    let mut parts = token.split('.');
    let header_part = parts
        .next()
        .ok_or_else(|| untrusted("JWT header missing"))?;
    let claims_part = parts
        .next()
        .ok_or_else(|| untrusted("JWT claims missing"))?;
    let signature_part = parts
        .next()
        .ok_or_else(|| untrusted("JWT signature missing"))?;
    if parts.next().is_some() {
        return Err(untrusted("JWT has too many segments"));
    }

    let decoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header: Header = serde_json::from_slice(
        &decoder
            .decode(header_part)
            .map_err(|e| malformed(anyhow::Error::new(e).context("decode JWT header")))?,
    )
    .map_err(|e| malformed(anyhow::Error::new(e).context("parse JWT header")))?;
    if header.alg != "ES256" || header.typ.as_deref().is_some_and(|typ| typ != "JWT") {
        return Err(untrusted("unsupported JWT header"));
    }

    let key_der = decode_public_key(public_key_pem).map_err(|e| untrusted(&format!("{e:#}")))?;
    let signature = decoder
        .decode(signature_part)
        .map_err(|e| malformed(anyhow::Error::new(e).context("decode JWT signature")))?;
    let signing_input = format!("{header_part}.{claims_part}");
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key_der)
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| untrusted("invalid JWT signature"))?;

    let claims: VerifiedClaims = serde_json::from_slice(
        &decoder
            .decode(claims_part)
            .map_err(|e| malformed(anyhow::Error::new(e).context("decode JWT claims")))?,
    )
    .map_err(|e| malformed(anyhow::Error::new(e).context("parse JWT claims")))?;
    if claims.iss.trim_end_matches('/') != expected_issuer.trim_end_matches('/') {
        return Err(untrusted("JWT issuer mismatch"));
    }
    if claims.sub != expected_machine {
        return Err(untrusted("JWT machine binding mismatch"));
    }
    if claims.exp <= chrono::Utc::now().timestamp() {
        return Err(expired("JWT expired"));
    }
    if !matches!(claims.tier.as_str(), "pro" | "trial") {
        return Err(untrusted("JWT carries unsupported tier"));
    }
    if let Some(expiry) = &claims.licence_exp {
        let expiry = chrono::DateTime::parse_from_rfc3339(expiry)
            .map_err(|e| malformed(anyhow::Error::new(e).context("invalid licence_exp claim")))?;
        if expiry <= chrono::Utc::now() {
            return Err(expired("licence expired"));
        }
    }
    Ok(claims)
}

fn decode_public_key(pem: &str) -> Result<Vec<u8>> {
    let body = pem
        .replace("-----BEGIN PUBLIC KEY-----", "")
        .replace("-----END PUBLIC KEY-----", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    if body.is_empty() {
        bail!("empty licence public key");
    }
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .context("decode licence public key PEM")?;
    // ring's verifier expects the SEC1 uncompressed point. OpenSSL's
    // `-pubout` emits SubjectPublicKeyInfo, whose final 65 bytes are that
    // point for P-256.
    if der.len() >= 65 && der[der.len() - 65] == 0x04 {
        Ok(der[der.len() - 65..].to_vec())
    } else if der.len() == 65 && der[0] == 0x04 {
        Ok(der)
    } else {
        bail!("licence public key is not a P-256 public key")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};

    #[test]
    fn rejects_unsigned_token() {
        assert!(verify_token_with("e30.e30.", "invalid", "https://issuer", "machine").is_err());
    }

    #[test]
    fn rejects_wrong_segment_count() {
        assert!(verify_token_with("one.two", "x", "issuer", "machine").is_err());
    }

    #[test]
    fn verifies_valid_machine_bound_token() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = encoder.encode(br#"{"alg":"ES256","typ":"JWT"}"#);
        let claims = encoder.encode(
            format!(
                r#"{{"iss":"https://issuer","sub":"machine","tier":"pro","exp":{}}}"#,
                chrono::Utc::now().timestamp() + 300
            )
            .as_bytes(),
        );
        let input = format!("{header}.{claims}");
        let signature = pair.sign(&rng, input.as_bytes()).unwrap();
        let token = format!("{input}.{}", encoder.encode(signature.as_ref()));
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
            base64::engine::general_purpose::STANDARD.encode(pair.public_key().as_ref())
        );

        let verified = verify_token_with(&token, &pem, "https://issuer", "machine").unwrap();
        assert_eq!(verified.tier, "pro");
    }
}
