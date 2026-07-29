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
    let mut parts = token.split('.');
    let header_part = parts.next().context("JWT header missing")?;
    let claims_part = parts.next().context("JWT claims missing")?;
    let signature_part = parts.next().context("JWT signature missing")?;
    if parts.next().is_some() {
        bail!("JWT has too many segments");
    }

    let decoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header: Header =
        serde_json::from_slice(&decoder.decode(header_part).context("decode JWT header")?)
            .context("parse JWT header")?;
    if header.alg != "ES256" || header.typ.as_deref().is_some_and(|typ| typ != "JWT") {
        bail!("unsupported JWT header");
    }

    let key_der = decode_public_key(public_key_pem)?;
    let signature = decoder
        .decode(signature_part)
        .context("decode JWT signature")?;
    let signing_input = format!("{header_part}.{claims_part}");
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key_der)
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| anyhow::anyhow!("invalid JWT signature"))?;

    let claims: VerifiedClaims =
        serde_json::from_slice(&decoder.decode(claims_part).context("decode JWT claims")?)
            .context("parse JWT claims")?;
    if claims.iss.trim_end_matches('/') != expected_issuer.trim_end_matches('/') {
        bail!("JWT issuer mismatch");
    }
    if claims.sub != expected_machine {
        bail!("JWT machine binding mismatch");
    }
    if claims.exp <= chrono::Utc::now().timestamp() {
        bail!("JWT expired");
    }
    if !matches!(claims.tier.as_str(), "pro" | "trial") {
        bail!("JWT carries unsupported tier");
    }
    if let Some(expiry) = &claims.licence_exp {
        let expiry =
            chrono::DateTime::parse_from_rfc3339(expiry).context("invalid licence_exp claim")?;
        if expiry <= chrono::Utc::now() {
            bail!("licence expired");
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
