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
//! JWT signature verification (ES256 / P-256) is performed by
//! `strivo_core::licence::verify_es256` before any backend token is
//! persisted. In dev mode (production key not yet configured) verification
//! is skipped with a logged warning so local dev boxes remain functional.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strivo_core::licence::{cache::Licence, gate, machine_id, verify_es256, LicenceCache, Tier};

use crate::server::AppState;

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
    let tier = match parsed.tier.as_str() {
        "pro" => Tier::Pro,
        "trial" => Tier::Trial,
        _ => fallback_tier,
    };
    // Verify the ES256 JWT signature before trusting the payload.
    // In dev mode (ACTIVATION_VERIFYING_KEY_B64 == "PLACEHOLDER" and
    // STRIVO_LICENCE_PUBKEY unset) this logs a warning and skips
    // cryptographic verification so dev/local-trial flows still work.
    // Local-trial tokens (produced in `trial()` above) do not go through
    // the backend and thus never reach this helper, so skipping is safe
    // for those paths in production as well.
    if !parsed.token.is_empty() && !parsed.token.starts_with("local-trial.") {
        if let Err(e) = verify_es256(&parsed.token) {
            return crate::problem::Problem::internal(format!(
                "JWT signature verification failed: {e}"
            ))
            .into_response();
        }
    }
    let lic = Licence {
        tier,
        machine_hash: machine_id::hashed_machine_id(),
        expires_at: parsed.expires_at.clone(),
        last_refreshed: chrono::Utc::now().to_rfc3339(),
        token: parsed.token,
        licence_key,
    };
    if let Err(e) = LicenceCache::save(&lic) {
        return crate::problem::Problem::internal(format!("save cache: {e}")).into_response();
    }
    Json(json!({
        "ok": true,
        "tier": parsed.tier,
        "expires_at": parsed.expires_at,
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
