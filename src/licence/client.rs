//! HTTP client for the activation backend.
//!
//! The same calls the SPA reaches via `/api/v1/licence/*` also need
//! to be made from inside the daemon — for the scheduled 72h refresh
//! task (Phase 3d) — so the actual HTTP work lives here in the core
//! crate. The web crate's route handlers and the daemon's background
//! task both call into these functions.
//!
//! ES256 JWT signature + claim verification for the SPA's
//! activate/trial/refresh calls lives in `strivo-web`'s
//! `routes::licence` (`verify_licence_token`) — the core crate can't
//! depend on strivo-web to reuse it there. `refresh_now` below (the
//! daemon's own 72h background refresh, see `spawn_refresh_loop`) is a
//! separate call path that can't reach that verifier either, so this
//! module mirrors the same verification logic as [`verify_licence_token`]:
//! it checks the backend token's ES256 signature against the resolved
//! public key, then its `sub`/`exp`/`licence_exp` claims, before a
//! [`Licence`] is ever constructed from it. With no public key
//! resolvable ([`resolve_pubkey_pem`] returns `None`), `refresh_now`
//! fails closed rather than trusting an unverified token.

use anyhow::{Context, Result};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

use super::cache::{Licence, LicenceCache, Tier};
use super::machine_id::hashed_machine_id;

/// Compile-time embedded production ES256 (P-256) public key, PEM
/// encoded (SPKI `-----BEGIN PUBLIC KEY-----`). Left blank in this
/// tree — the licence backend (`licence-backend/`) is pre-launch and
/// its production keypair is not checked in (see `licence-backend/
/// *.pem` in `.gitignore`). The operator embeds the real key here at
/// launch, or points `STRIVO_LICENCE_PUBKEY` at it for self-hosted /
/// development deployments (checked first — see `resolve_pubkey_pem`).
/// This is deploy-time configuration, not a stub of the verification
/// logic itself: with no key resolvable, `refresh_now` fails closed
/// rather than trusting an unverified token.
const LICENCE_PUBKEY_PEM: &str = "";

/// Resolve the ES256 public key used to verify backend licence tokens:
/// `STRIVO_LICENCE_PUBKEY` (PEM, EC public key) if set and non-empty,
/// else the embedded [`LICENCE_PUBKEY_PEM`]. Returns `None` when
/// neither is available so callers can fail closed.
fn resolve_pubkey_pem() -> Option<String> {
    std::env::var("STRIVO_LICENCE_PUBKEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            if LICENCE_PUBKEY_PEM.is_empty() {
                None
            } else {
                Some(LICENCE_PUBKEY_PEM.to_string())
            }
        })
}

/// Claims carried by the backend's licence JWT — see
/// `licence-backend/src/index.ts` `mintToken`. Mirrors
/// `strivo-web::routes::licence::LicenceClaims`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LicenceClaims {
    /// Issuer — the activation backend's public base URL.
    #[allow(dead_code)]
    iss: String,
    /// SHA-256(machine_id) this token is bound to.
    sub: String,
    /// "pro" | "trial".
    tier: String,
    #[allow(dead_code)]
    iat: i64,
    /// Unix seconds; validated by `jsonwebtoken` (`validate_exp`).
    #[allow(dead_code)]
    exp: i64,
    /// RFC3339 trial expiry. Present only for trial tokens.
    #[serde(default)]
    licence_exp: Option<String>,
}

/// Claims surviving signature verification + claim checks — the only
/// form of the backend response [`refresh_now`] is allowed to trust.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedClaims {
    tier: String,
    licence_exp: Option<String>,
}

/// Verify `token`'s ES256 signature against `pubkey_pem`, then check
/// that its claims are safe to trust: `sub` must equal
/// `expected_machine_hash`, `exp` must not have passed (jsonwebtoken
/// validates this as part of `decode`), and `licence_exp` — when
/// present — must not have passed relative to `now_unix`. Pure and
/// side-effect free: never constructs a [`Licence`]; that is left to
/// the caller once this returns `Ok`. Fails closed — any error here
/// is a hard rejection, never a fallback tier.
fn verify_licence_token(
    token: &str,
    pubkey_pem: &str,
    expected_machine_hash: &str,
    now_unix: i64,
) -> Result<VerifiedClaims> {
    let key = DecodingKey::from_ec_pem(pubkey_pem.as_bytes())
        .context("invalid licence public key")?;
    let mut validation = Validation::new(Algorithm::ES256);
    validation.validate_exp = true;

    let data = decode::<LicenceClaims>(token, &key, &validation)
        .context("licence token invalid")?;
    let claims = data.claims;

    if claims.sub != expected_machine_hash {
        anyhow::bail!("licence token is bound to a different machine");
    }

    if let Some(licence_exp) = &claims.licence_exp {
        let expired = match chrono::DateTime::parse_from_rfc3339(licence_exp) {
            Ok(dt) => dt.timestamp() < now_unix,
            // Unparsable licence_exp fails closed rather than being ignored.
            Err(_) => true,
        };
        if expired {
            anyhow::bail!("licence token's trial period has expired");
        }
    }

    Ok(VerifiedClaims {
        tier: claims.tier,
        licence_exp: claims.licence_exp,
    })
}

#[derive(Debug, Deserialize)]
struct BackendTokenResponse {
    token: String,
    /// Deliberately not trusted directly — the tier used to build the
    /// `Licence` comes from `verify_licence_token`'s verified claims,
    /// not this raw response field. Kept here only for backend schema
    /// completeness; serde ignores unknown fields, so it could be
    /// dropped, but documents that the field is real, not missing.
    #[serde(default)]
    #[allow(dead_code)]
    tier: String,
    #[serde(default)]
    expires_at: Option<String>,
}

/// Read the activation backend URL from `STRIVO_LICENCE_URL`. Returns
/// None when unset (or empty) so callers can short-circuit.
pub fn backend_url() -> Option<String> {
    std::env::var("STRIVO_LICENCE_URL")
        .ok()
        .filter(|v| !v.is_empty())
}

/// Call `POST {backend}/refresh` for the currently cached licence and
/// rewrite the local cache with the new token + last_refreshed. Returns
/// the persisted Licence on success.
///
/// `Err` reasons the caller should distinguish:
///   - **No backend URL** → user hasn't pointed at one yet; this is a
///     no-op, not a failure.
///   - **No cache** → free tier, nothing to refresh.
///   - **HTTP 403 revoked** → server says the licence is dead; we
///     surface that and the cache is cleared.
///   - **Network error** → keep the cache as-is; we'll try again next
///     interval. (The "no internet kill" rule.)
pub async fn refresh_now() -> Result<Licence> {
    let Some(url) = backend_url() else {
        anyhow::bail!("STRIVO_LICENCE_URL unset");
    };
    let Some(cache) = LicenceCache::load()? else {
        anyhow::bail!("no licence on file");
    };
    if cache.licence_key.is_empty() && matches!(cache.tier, Tier::Trial) {
        // Trials refresh by machine_hash alone (server keyed on that).
    }
    let body = serde_json::json!({
        "licence_key": cache.licence_key,
        "machine_hash": hashed_machine_id(),
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let resp = client
        .post(format!("{url}/refresh"))
        .json(&body)
        .send()
        .await
        .context("backend unreachable")?;
    let status = resp.status();
    let raw = resp.text().await.context("read response")?;

    if status.as_u16() == 403 {
        // Server denied — licence revoked / refunded. Clear cache so
        // the gate fails closed on the next read.
        LicenceCache::clear().ok();
        anyhow::bail!("licence revoked: {raw}");
    }
    if !status.is_success() {
        anyhow::bail!("backend {status}: {raw}");
    }
    let parsed: BackendTokenResponse =
        serde_json::from_str(&raw).context("parse backend response")?;

    let Some(pubkey_pem) = resolve_pubkey_pem() else {
        anyhow::bail!(
            "licence verification key not configured — set STRIVO_LICENCE_PUBKEY or embed LICENCE_PUBKEY_PEM at build time"
        );
    };
    let verified = verify_licence_token(
        &parsed.token,
        &pubkey_pem,
        &hashed_machine_id(),
        chrono::Utc::now().timestamp(),
    )?;

    let tier = match verified.tier.as_str() {
        "pro" => Tier::Pro,
        "trial" => Tier::Trial,
        _ => cache.tier,
    };
    let lic = Licence {
        tier,
        machine_hash: hashed_machine_id(),
        expires_at: verified.licence_exp.clone().or(parsed.expires_at),
        last_refreshed: chrono::Utc::now().to_rfc3339(),
        token: parsed.token,
        licence_key: cache.licence_key.clone(),
    };
    LicenceCache::save(&lic)?;
    Ok(lic)
}

/// Spawn the periodic refresh task. Runs forever:
///   - wait `interval`
///   - if there's a cache AND a backend URL, try a refresh
///   - swallow errors (the "no internet kill" rule keeps the cache
///     valid until the server explicitly revokes)
///
/// Returns the JoinHandle so the daemon can shut it down on exit.
pub fn spawn_refresh_loop(interval: std::time::Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // First tick after one interval — at startup we want to come
        // up immediately even if the network is down, then catch up
        // on the first scheduled fire.
        let mut next = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        loop {
            next.tick().await;
            if backend_url().is_none() {
                continue; // nothing configured, nothing to do
            }
            match refresh_now().await {
                Ok(lic) => tracing::info!(
                    tier = ?lic.tier,
                    "licence refreshed (next attempt in {:?})",
                    interval,
                ),
                Err(e) => {
                    // INFO not WARN — an offline daemon is the
                    // expected case and shouldn't pollute logs.
                    tracing::info!("licence refresh skipped: {e}");
                }
            }
        }
    })
}

/// Default refresh cadence: 72 hours. The CF Worker's
/// `REFRESH_INTERVAL_HOURS` defaults to the same; keep them in sync.
pub const DEFAULT_REFRESH_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(72 * 60 * 60);

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};

    const MACHINE_HASH: &str = "abc123deadbeef";

    /// Generate an ephemeral P-256 keypair, PEM-encoded (PKCS8 private /
    /// SPKI public) — exactly the forms `jsonwebtoken`'s `from_ec_pem`
    /// helpers accept. Ephemeral because the real production key is not
    /// (and must not be) checked into this repo.
    fn ephemeral_keypair_pem() -> (String, String) {
        let signing_key = SigningKey::random(&mut rand_core::OsRng);
        let priv_pem = signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode pkcs8 private key")
            .to_string();
        let pub_pem = signing_key
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("encode spki public key");
        (priv_pem, pub_pem)
    }

    fn sign(priv_pem: &str, claims: &LicenceClaims) -> String {
        let key = EncodingKey::from_ec_pem(priv_pem.as_bytes()).expect("load ec private key");
        encode(&Header::new(Algorithm::ES256), claims, &key).expect("sign jwt")
    }

    fn base_claims(now: i64) -> LicenceClaims {
        LicenceClaims {
            iss: "https://licence.strivo.test".into(),
            sub: MACHINE_HASH.into(),
            tier: "pro".into(),
            iat: now,
            exp: now + 3600,
            licence_exp: None,
        }
    }

    #[test]
    fn accepts_validly_signed_token() {
        let (priv_pem, pub_pem) = ephemeral_keypair_pem();
        let now = chrono::Utc::now().timestamp();
        let claims = base_claims(now);
        let token = sign(&priv_pem, &claims);

        let verified = verify_licence_token(&token, &pub_pem, MACHINE_HASH, now)
            .expect("well-formed, correctly-signed token must verify");
        assert_eq!(verified.tier, "pro");
        assert_eq!(verified.licence_exp, None);
    }

    #[test]
    fn rejects_tampered_token() {
        let (priv_pem, pub_pem) = ephemeral_keypair_pem();
        let now = chrono::Utc::now().timestamp();
        let claims = base_claims(now);

        // Same claims, signed by a DIFFERENT key: verifying against the
        // original public key must fail — this is the actual signature
        // check, not just a claims check.
        let (other_priv_pem, _other_pub_pem) = ephemeral_keypair_pem();
        let tampered = sign(&other_priv_pem, &claims);
        assert!(
            verify_licence_token(&tampered, &pub_pem, MACHINE_HASH, now).is_err(),
            "token re-signed with a different key must not verify"
        );
        // sanity: the untampered private key still matches this pubkey
        let _ = sign(&priv_pem, &claims);
    }

    #[test]
    fn rejects_wrong_machine() {
        let (priv_pem, pub_pem) = ephemeral_keypair_pem();
        let now = chrono::Utc::now().timestamp();

        let mut wrong_machine = base_claims(now);
        wrong_machine.sub = "some-other-machine-hash".into();
        let token = sign(&priv_pem, &wrong_machine);
        assert!(
            verify_licence_token(&token, &pub_pem, MACHINE_HASH, now).is_err(),
            "token bound to a different machine must be rejected"
        );
    }

    #[test]
    fn rejects_expired_trial() {
        let (priv_pem, pub_pem) = ephemeral_keypair_pem();
        let now = chrono::Utc::now().timestamp();

        let mut trial_expired = base_claims(now);
        trial_expired.tier = "trial".into();
        trial_expired.licence_exp =
            Some((chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339());
        let token = sign(&priv_pem, &trial_expired);
        assert!(
            verify_licence_token(&token, &pub_pem, MACHINE_HASH, now).is_err(),
            "token whose trial licence_exp has passed must be rejected"
        );
    }
}
