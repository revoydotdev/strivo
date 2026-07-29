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
//! Tokens are verified as ES256 JWTs before persistence. Verification
//! covers issuer, machine binding, expiry, licence expiry, and signed tier.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strivo_core::licence::{cache::Licence, gate, machine_id, LicenceCache, Tier};

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
    crate::problem::Problem::unavailable(
        "licence backend not configured; a server-issued token is required",
    )
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
    let claims = match strivo_core::licence::verify::verify_token(&parsed.token) {
        Ok(claims) => claims,
        Err(e) => {
            return crate::problem::Problem::unavailable(format!(
                "licence server returned an unverifiable token: {e}"
            ))
            .into_response()
        }
    };
    if claims.tier != parsed.tier {
        return crate::problem::Problem::unavailable(
            "licence server tier does not match signed claims",
        )
        .into_response();
    }
    let lic = Licence {
        tier,
        machine_hash: machine_id::hashed_machine_id(),
        expires_at: claims.licence_exp.clone(),
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
        "expires_at": claims.licence_exp,
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
