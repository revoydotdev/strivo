//! ES256 JWT signature verification for activation tokens.
//!
//! The activation backend (`licence-backend/src/jwt.ts`) signs tokens with
//! ECDSA on P-256 (ES256). The JOSE signature is the raw 64-byte `r || s`
//! value (not DER-encoded). This module decodes and verifies that signature
//! against the embedded verifying key before the caller trusts any claim in
//! the payload.
//!
//! # Production key
//!
//! [`ACTIVATION_VERIFYING_KEY_B64`] is a **placeholder**. Before shipping a
//! release build:
//!
//! 1. Export the P-256 public key from the Worker's `JWT_PRIVATE_KEY` secret
//!    in SubjectPublicKeyInfo (SPKI) DER format and base64-encode it, e.g.:
//!
//!    ```sh
//!    openssl ec -in signing.pem -pubout -outform DER | base64 -w0
//!    ```
//!
//! 2. Replace the placeholder string below with that output and re-compile.
//!
//! Alternatively, set `STRIVO_LICENCE_PUBKEY` at runtime (base64 DER SPKI)
//! to override the compiled-in key — useful for CI / staged rollouts.
//!
//! **Do NOT commit the private key.** The `*.pem` and `*.der` patterns are
//! gitignored; only the _public_ key in base64 form belongs here.

use anyhow::{bail, Context, Result};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;

/// Base64-encoded DER SubjectPublicKeyInfo (SPKI) of the P-256 public key
/// used by the activation backend to sign JWT tokens.
///
/// **This is a placeholder.** Replace with the real production public key
/// before shipping (see module-level doc). An empty string or the literal
/// `"PLACEHOLDER"` value causes verification to be skipped with a warning —
/// this preserves dev/CI usability while making it obvious the key is not set.
pub const ACTIVATION_VERIFYING_KEY_B64: &str = "PLACEHOLDER";

/// Decoded claims from a verified activation JWT.
#[derive(Debug, Clone)]
pub struct Claims {
    pub tier: String,
    pub machine_hash: String,
    pub expires_at: Option<String>,
    /// Anything else in the payload, kept as a raw JSON value for callers
    /// that want the full claim set.
    pub raw: serde_json::Value,
}

/// Verify an ES256 JWT produced by the activation backend.
///
/// The `token` must be a compact-serialised JWT
/// (`base64url(header).base64url(payload).base64url(signature)`) where the
/// signature is the raw 64-byte JOSE `r || s` encoding.
///
/// # Key resolution
///
/// 1. `STRIVO_LICENCE_PUBKEY` env var (base64 DER SPKI) — runtime override.
/// 2. [`ACTIVATION_VERIFYING_KEY_B64`] compiled-in constant.
///
/// If the resolved key is empty or equals `"PLACEHOLDER"`, verification is
/// **skipped** and `Ok(claims)` is returned with a `WARN`-level log. This
/// keeps dev boxes and CI functional before the key is wired up; production
/// builds must set the real key.
///
/// # Errors
///
/// Returns `Err` if:
/// - the token has the wrong number of segments
/// - base64url decoding fails
/// - the signature bytes are not a valid P-256 `r || s` pair
/// - the ECDSA verification fails (signature doesn't match the header+payload)
/// - the payload is not valid JSON or is missing required fields
pub fn verify_es256(token: &str) -> Result<Claims> {
    // Resolve the verifying key: runtime env override, then compiled-in const.
    let key_b64 = std::env::var("STRIVO_LICENCE_PUBKEY")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| ACTIVATION_VERIFYING_KEY_B64.to_string());

    verify_es256_with_key(token, &key_b64)
}

/// Verify an ES256 JWT against an explicitly supplied verifying key.
///
/// This is the key-agnostic core of [`verify_es256`]: it takes the base64 DER
/// SPKI verifying key directly instead of resolving it from the environment or
/// compiled-in constant. [`verify_es256`] is a thin wrapper that performs that
/// resolution and delegates here.
///
/// Passing an empty string or the literal `"PLACEHOLDER"` as `key_b64` skips
/// verification (dev/CI mode) exactly as [`verify_es256`] does when no key is
/// configured. Errors are identical to [`verify_es256`] for a given key.
pub fn verify_es256_with_key(token: &str, key_b64: &str) -> Result<Claims> {
    let parts: Vec<&str> = token.splitn(4, '.').collect();
    if parts.len() != 3 {
        bail!("JWT must have exactly 3 segments, got {}", parts.len());
    }
    let (header_b64, payload_b64, sig_b64) = (parts[0], parts[1], parts[2]);

    if key_b64.is_empty() || key_b64 == "PLACEHOLDER" {
        tracing::warn!(
            "STRIVO_LICENCE_PUBKEY / ACTIVATION_VERIFYING_KEY_B64 not configured — \
             skipping ES256 JWT signature verification (dev/CI mode)"
        );
        // Parse and return claims without verifying.
        return decode_claims(payload_b64);
    }

    // Decode the DER SPKI public key.
    let key_der = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, key_b64)
        .context("decode verifying key from base64")?;
    let vk = VerifyingKey::from_public_key_der(&key_der).context("parse P-256 SPKI public key")?;

    // The signing input is the ASCII bytes of "header_b64.payload_b64".
    let signing_input = format!("{header_b64}.{payload_b64}");

    // Decode the JOSE-format signature (raw 64-byte r || s, no DER).
    let sig_bytes =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sig_b64)
            .context("base64url-decode JWT signature")?;
    let signature = Signature::from_slice(&sig_bytes)
        .context("parse ES256 signature (expected 64-byte r||s)")?;

    vk.verify(signing_input.as_bytes(), &signature)
        .context("ES256 signature verification failed")?;

    decode_claims(payload_b64)
}

fn decode_claims(payload_b64: &str) -> Result<Claims> {
    use base64::Engine;
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .context("base64url-decode JWT payload")?;
    let raw: serde_json::Value =
        serde_json::from_slice(&payload_bytes).context("parse JWT payload JSON")?;

    let tier = raw
        .get("tier")
        .and_then(|v| v.as_str())
        .unwrap_or("free")
        .to_string();
    let machine_hash = raw
        .get("machine_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_at = raw
        .get("expires_at")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(Claims {
        tier,
        machine_hash,
        expires_at,
        raw,
    })
}

// ─── helpers for tests ──────────────────────────────────────────────────────

/// Build a compact JWT signed with the given signing key.
///
/// Used exclusively in unit tests; not part of the public API.
#[cfg(test)]
pub(crate) fn sign_jwt_es256(claims: &serde_json::Value, sk: &p256::ecdsa::SigningKey) -> String {
    use base64::Engine;
    use p256::ecdsa::signature::Signer;

    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"ES256","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(claims).unwrap());
    let signing_input = format!("{header}.{payload}");

    let sig: p256::ecdsa::Signature = sk.sign(signing_input.as_bytes());
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());

    // Expose the verifying key in base64 DER SPKI so tests can feed it to
    // verify_es256_with_key directly.
    let _ = sk; // bind for clarity

    format!("{signing_input}.{sig_b64}")
}

/// Return the base64 DER SPKI encoding of the verifying key, for use in
/// tests that pass it directly to `verify_es256_with_key`.
#[cfg(test)]
pub(crate) fn verifying_key_b64_spki(sk: &p256::ecdsa::SigningKey) -> String {
    use base64::Engine;
    use p256::pkcs8::EncodePublicKey;

    let vk = sk.verifying_key();
    let der = vk.to_public_key_der().unwrap();
    base64::engine::general_purpose::STANDARD.encode(der.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::SigningKey;

    fn fresh_keypair() -> SigningKey {
        SigningKey::random(&mut rand_core::OsRng)
    }

    fn sample_claims() -> serde_json::Value {
        serde_json::json!({
            "sub": "test-licence",
            "tier": "pro",
            "machine_hash": "abc123",
            "expires_at": null,
            "iat": 1_700_000_000u64,
        })
    }

    // Tests pass the verifying key directly to `verify_es256_with_key`, so
    // they never touch the process-global `STRIVO_LICENCE_PUBKEY` env var and
    // are parallel-safe by construction under the default multi-threaded runner.

    #[test]
    fn accepts_valid_token() {
        let sk = fresh_keypair();
        let claims = sample_claims();
        let token = sign_jwt_es256(&claims, &sk);
        let result = verify_es256_with_key(&token, &verifying_key_b64_spki(&sk));
        let verified = result.expect("valid token must verify");
        assert_eq!(verified.tier, "pro");
        assert_eq!(verified.machine_hash, "abc123");
        assert!(verified.expires_at.is_none());
    }

    #[test]
    fn rejects_tampered_payload() {
        let sk = fresh_keypair();
        let claims = sample_claims();
        let token = sign_jwt_es256(&claims, &sk);

        // Flip one character in the payload segment (second dot-separated part).
        let mut parts: Vec<String> = token.splitn(3, '.').map(str::to_string).collect();
        assert_eq!(parts.len(), 3);
        // Decode, mutate, re-encode the payload.
        let mut payload_bytes =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &parts[1])
                .unwrap();
        // Change "pro" -> "fre" in the raw bytes (ASCII, definitely present).
        for w in payload_bytes.windows(3) {
            if w == b"pro" {
                // found; break and mutate via position search
                break;
            }
        }
        if let Some(pos) = payload_bytes.windows(3).position(|w| w == b"pro") {
            payload_bytes[pos] = b'f';
            payload_bytes[pos + 1] = b'r';
            payload_bytes[pos + 2] = b'e';
        }
        parts[1] = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            &payload_bytes,
        );
        let tampered = parts.join(".");

        let result = verify_es256_with_key(&tampered, &verifying_key_b64_spki(&sk));
        assert!(result.is_err(), "tampered payload must be rejected");
    }

    #[test]
    fn rejects_signature_from_different_key() {
        let sk1 = fresh_keypair();
        let sk2 = fresh_keypair(); // different key
        let claims = sample_claims();

        // Sign with sk1, but verify against sk2's public key.
        let token = sign_jwt_es256(&claims, &sk1);
        let result = verify_es256_with_key(&token, &verifying_key_b64_spki(&sk2));
        assert!(result.is_err(), "signature from wrong key must be rejected");
    }

    #[test]
    fn placeholder_key_skips_verification_and_returns_claims() {
        let sk = fresh_keypair();
        let claims = sample_claims();
        let token = sign_jwt_es256(&claims, &sk);

        // A "PLACEHOLDER" key takes the dev-mode skip path → crypto is skipped
        // and the parsed claims are returned. This mirrors the compiled-in
        // default constant without touching any process-global env state.
        let result = verify_es256_with_key(&token, "PLACEHOLDER");
        let verified = result.expect("placeholder key skips verification; must not error");
        assert_eq!(verified.tier, "pro");
    }
}
