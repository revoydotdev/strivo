//! Strivo Pro licence routes.
//!
//! status — read entitlement from the local cache + dev override.
//! activate — POST {licence_key} → backend, persist token on 200.
//! trial    — POST {} → backend (machine_hash added server-side),
//!            persist a 3-day token.
//! refresh  — POST {} → backend, re-sign + extend last_refreshed.
//!
//! Backend URL is taken from the `STRIVO_LICENCE_URL` env var (or
//! `[licence].backend_url` in config.toml — Phase 4). When unset the
//! mutating routes return 501 so a self-hosted user without a Pro
//! account sees a clean "backend not configured" rather than a
//! confusing network error.
//!
//! The backend's response token is a signed ES256 JWT (see
//! `licence-backend/src/jwt.ts`). Before any of it is trusted,
//! [`verify_licence_token`] verifies the raw r||s ECDSA signature
//! against the operator-supplied public key, then checks the `sub`
//! (machine hash), `exp`, and — for trials — `licence_exp` claims.
//! This is the real trust gate (VISION AX-7): a token that fails any
//! of those checks is rejected with a typed [`crate::problem::Problem`]
//! and no [`Licence`] is ever constructed from it. There is no silent
//! tier fallback on verification failure.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strivo_core::licence::{cache::Licence, gate, machine_id, LicenceCache, Tier};

use crate::server::AppState;

/// Compile-time embedded production ES256 (P-256) public key, PEM
/// encoded (SPKI `-----BEGIN PUBLIC KEY-----`). Left blank in this
/// tree — the licence backend (`licence-backend/`) is pre-launch and
/// its production keypair is not checked in (see `licence-backend/
/// *.pem` in `.gitignore`). The operator embeds the real key here at
/// launch, or points `STRIVO_LICENCE_PUBKEY` at it for self-hosted /
/// development deployments (checked first — see `resolve_pubkey_pem`).
/// This is deploy-time configuration, not a stub of the verification
/// logic itself: with no key resolvable, [`persist_and_reply`] fails
/// closed rather than trusting an unverified token.
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
/// `licence-backend/src/index.ts` `mintToken`.
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
/// form of the backend response the rest of the module is allowed to
/// trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedClaims {
    pub tier: String,
    pub licence_exp: Option<String>,
}

/// Verify `token`'s ES256 signature against `pubkey_pem`, then check
/// that its claims are safe to trust: `sub` must equal
/// `expected_machine_hash`, `exp` must not have passed (jsonwebtoken
/// validates this as part of `decode`), and `licence_exp` — when
/// present — must not have passed relative to `now_unix`. Pure and
/// side-effect free: never constructs a [`Licence`]; that is left to
/// the caller once this returns `Ok`. Fails closed — any error here
/// is a hard rejection, never a fallback tier.
pub(crate) fn verify_licence_token(
    token: &str,
    pubkey_pem: &str,
    expected_machine_hash: &str,
    now_unix: i64,
) -> Result<VerifiedClaims, crate::problem::Problem> {
    let key = DecodingKey::from_ec_pem(pubkey_pem.as_bytes()).map_err(|e| {
        crate::problem::Problem::internal(format!("invalid licence public key: {e}"))
    })?;
    let mut validation = Validation::new(Algorithm::ES256);
    validation.validate_exp = true;

    let data = decode::<LicenceClaims>(token, &key, &validation).map_err(|e| {
        crate::problem::Problem::unauthorized_detail(format!("licence token invalid: {e}"))
    })?;
    let claims = data.claims;

    if claims.sub != expected_machine_hash {
        return Err(crate::problem::Problem::unauthorized_detail(
            "licence token is bound to a different machine",
        ));
    }

    if let Some(licence_exp) = &claims.licence_exp {
        let expired = match chrono::DateTime::parse_from_rfc3339(licence_exp) {
            Ok(dt) => dt.timestamp() < now_unix,
            // Unparsable licence_exp fails closed rather than being ignored.
            Err(_) => true,
        };
        if expired {
            return Err(crate::problem::Problem::unauthorized_detail(
                "licence token's trial period has expired",
            ));
        }
    }

    Ok(VerifiedClaims {
        tier: claims.tier,
        licence_exp: claims.licence_exp,
    })
}

#[derive(Serialize)]
struct LicenceStatus {
    entitled: bool,
    tier: &'static str,
    trial: Option<serde_json::Value>,
    expires_at: Option<String>,
    machine_id: Option<String>,
    /// True iff the activation backend URL is configured. Lets the SPA
    /// keep the "Activate / Start trial" buttons disabled with a clean
    /// hint when the user hasn't pointed at a backend yet.
    implemented: bool,
}

async fn status() -> Json<LicenceStatus> {
    let mh = machine_id::hashed_machine_id();
    let entitled = gate::entitled();
    let cache = LicenceCache::load().ok().flatten();

    let (tier, expires_at, trial) = match cache.as_ref() {
        Some(lic) if entitled => {
            let tier = match lic.tier {
                Tier::Pro => "pro",
                Tier::Trial => "trial",
                Tier::Free => "free",
            };
            let trial = if matches!(lic.tier, Tier::Trial) {
                Some(json!({ "expires_at": lic.expires_at }))
            } else {
                None
            };
            (tier, lic.expires_at.clone(), trial)
        }
        _ if entitled => ("pro", None, None),
        _ => ("free", None, None),
    };

    Json(LicenceStatus {
        entitled,
        tier,
        trial,
        expires_at,
        machine_id: Some(mh),
        implemented: backend_url().is_some(),
    })
}

fn backend_url() -> Option<String> {
    std::env::var("STRIVO_LICENCE_URL")
        .ok()
        .filter(|v| !v.is_empty())
}

#[derive(Deserialize)]
struct ActivateRequest {
    /// Lemon Squeezy licence key the user pasted.
    key: String,
}

#[derive(Deserialize)]
struct BackendTokenResponse {
    token: String,
    /// Unsigned echo of the tier from the backend's JSON body. Not
    /// trusted — `verify_licence_token`'s signed `tier` claim is the
    /// source of truth for what actually gets persisted.
    #[serde(default)]
    #[allow(dead_code)]
    tier: String,
    /// Set only for trials.
    #[serde(default)]
    expires_at: Option<String>,
}

async fn activate(
    State(_state): State<AppState>,
    Json(body): Json<ActivateRequest>,
) -> impl IntoResponse {
    let Some(url) = backend_url() else {
        return crate::problem::Problem::unavailable("licence backend not configured")
            .into_response();
    };
    let resp = match post_backend(
        &format!("{url}/activate"),
        json!({
            "licence_key": body.key,
            "machine_hash": machine_id::hashed_machine_id(),
        }),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return crate::problem::Problem::unavailable(e).into_response(),
    };
    persist_and_reply(resp, Tier::Pro, body.key).await
}

async fn trial(State(_state): State<AppState>) -> impl IntoResponse {
    if let Some(url) = backend_url() {
        // Backend live — go through the proper activation server.
        let resp = match post_backend(
            &format!("{url}/trial"),
            json!({ "machine_hash": machine_id::hashed_machine_id() }),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => return crate::problem::Problem::unavailable(e).into_response(),
        };
        return persist_and_reply(resp, Tier::Trial, String::new()).await;
    }
    // No backend configured — fall back to a self-issued local trial
    // so free clones / unmanaged installs can still kick the tyres.
    // 72h validity matches the backend default; machine_hash binding
    // keeps the trial single-machine. Once-only per machine: if a
    // local trial already exists we refuse to mint another.
    if let Ok(Some(existing)) = LicenceCache::load() {
        if matches!(existing.tier, Tier::Trial) {
            return crate::problem::Problem::bad_request(
                "trial already issued for this machine — wait for expiry or activate a key",
            )
            .into_response();
        }
    }
    let expires = (chrono::Utc::now() + chrono::Duration::hours(72)).to_rfc3339();
    let lic = Licence {
        tier: Tier::Trial,
        machine_hash: machine_id::hashed_machine_id(),
        expires_at: Some(expires.clone()),
        last_refreshed: chrono::Utc::now().to_rfc3339(),
        token: format!("local-trial.{}", machine_id::hashed_machine_id()),
        licence_key: String::new(),
    };
    if let Err(e) = LicenceCache::save(&lic) {
        return crate::problem::Problem::internal(format!("save cache: {e}")).into_response();
    }
    Json(json!({
        "ok": true,
        "tier": "trial",
        "expires_at": expires,
        "local_trial": true,
        "note": "STRIVO_LICENCE_URL unset — issued a local 72h trial. Run the Cloudflare Worker in licence-backend/ and set STRIVO_LICENCE_URL to enable hosted activation / Lemon Squeezy purchases.",
    }))
    .into_response()
}

async fn refresh(State(_state): State<AppState>) -> impl IntoResponse {
    let Some(url) = backend_url() else {
        return crate::problem::Problem::unavailable("licence backend not configured")
            .into_response();
    };
    let cache = match LicenceCache::load() {
        Ok(Some(c)) => c,
        _ => {
            return crate::problem::Problem::bad_request("no licence on file to refresh")
                .into_response()
        }
    };
    let resp = match post_backend(
        &format!("{url}/refresh"),
        json!({
            "licence_key": cache.licence_key.clone(),
            "machine_hash": machine_id::hashed_machine_id(),
        }),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return crate::problem::Problem::unavailable(e).into_response(),
    };
    persist_and_reply(resp, cache.tier, cache.licence_key).await
}

async fn post_backend(url: &str, body: serde_json::Value) -> Result<BackendResponse, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let r = client
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("backend unreachable: {e}"))?;
    let status = r.status();
    let raw = r.text().await.map_err(|e| e.to_string())?;
    Ok(BackendResponse { status, raw })
}

struct BackendResponse {
    status: reqwest::StatusCode,
    raw: String,
}

async fn persist_and_reply(
    resp: BackendResponse,
    fallback_tier: Tier,
    licence_key: String,
) -> axum::response::Response {
    if !resp.status.is_success() {
        // Pass the backend's status + body through so the SPA gets the
        // real reason ("licence revoked", "trial already claimed", …)
        // instead of a generic 500.
        return (
            resp.status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            resp.raw,
        )
            .into_response();
    }
    let parsed: BackendTokenResponse = match serde_json::from_str(&resp.raw) {
        Ok(p) => p,
        Err(e) => {
            return crate::problem::Problem::internal(format!("malformed backend response: {e}"))
                .into_response()
        }
    };

    // Fail-closed gate (VISION AX-7): verify the ES256 signature and
    // claims before anything from the backend response is trusted. No
    // `Licence` is constructed, and no tier is granted, on any failure
    // here — there is no silent fallback to a default tier.
    let Some(pubkey_pem) = resolve_pubkey_pem() else {
        return crate::problem::Problem::internal(
            "licence verification key not configured — set STRIVO_LICENCE_PUBKEY or embed LICENCE_PUBKEY_PEM at build time",
        )
        .into_response();
    };
    let expected_machine_hash = machine_id::hashed_machine_id();
    let now_unix = chrono::Utc::now().timestamp();
    let verified = match verify_licence_token(
        &parsed.token,
        &pubkey_pem,
        &expected_machine_hash,
        now_unix,
    ) {
        Ok(v) => v,
        Err(problem) => return problem.into_response(),
    };

    let tier = match verified.tier.as_str() {
        "pro" => Tier::Pro,
        "trial" => Tier::Trial,
        _ => fallback_tier,
    };
    // Verified claims are the source of truth for what gets persisted;
    // `licence_exp` (signed) is preferred over the backend body's
    // unsigned `expires_at` echo when both are present.
    let lic = Licence {
        tier,
        machine_hash: expected_machine_hash,
        expires_at: verified.licence_exp.clone().or(parsed.expires_at.clone()),
        last_refreshed: chrono::Utc::now().to_rfc3339(),
        token: parsed.token,
        licence_key,
    };
    if let Err(e) = LicenceCache::save(&lic) {
        return crate::problem::Problem::internal(format!("save cache: {e}")).into_response();
    }
    Json(json!({
        "ok": true,
        "tier": verified.tier,
        "expires_at": lic.expires_at,
    }))
    .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/licence/status", get(status))
        .route("/api/v1/licence/activate", post(activate))
        .route("/api/v1/licence/trial", post(trial))
        .route("/api/v1/licence/refresh", post(refresh))
}

#[cfg(test)]
mod verify_tests {
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
    fn licence_verify() {
        let (priv_pem, pub_pem) = ephemeral_keypair_pem();
        let now = chrono::Utc::now().timestamp();
        let claims = base_claims(now);
        let token = sign(&priv_pem, &claims);

        let verified = verify_licence_token(&token, &pub_pem, MACHINE_HASH, now)
            .expect("well-formed, correctly-signed token must verify");
        assert_eq!(verified.tier, "pro");
        assert_eq!(verified.licence_exp, None);

        // Same claims, signed by a DIFFERENT key: verifying against the
        // original public key must fail — this is the actual signature
        // check, not just a claims check.
        let (other_priv_pem, _other_pub_pem) = ephemeral_keypair_pem();
        let tampered = sign(&other_priv_pem, &claims);
        assert!(
            verify_licence_token(&tampered, &pub_pem, MACHINE_HASH, now).is_err(),
            "token re-signed with a different key must not verify"
        );
    }

    #[test]
    fn licence_reject() {
        let (priv_pem, pub_pem) = ephemeral_keypair_pem();
        let now = chrono::Utc::now().timestamp();

        // Valid signature, but sub != local machine hash.
        let mut wrong_machine = base_claims(now);
        wrong_machine.sub = "some-other-machine-hash".into();
        let token = sign(&priv_pem, &wrong_machine);
        assert!(
            verify_licence_token(&token, &pub_pem, MACHINE_HASH, now).is_err(),
            "token bound to a different machine must be rejected"
        );

        // Valid signature, but exp is in the past.
        let mut expired = base_claims(now);
        expired.iat = now - 7200;
        expired.exp = now - 3600;
        let token = sign(&priv_pem, &expired);
        assert!(
            verify_licence_token(&token, &pub_pem, MACHINE_HASH, now).is_err(),
            "expired token must be rejected"
        );

        // Valid signature, exp in the future, but licence_exp (trial
        // expiry) is in the past.
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
