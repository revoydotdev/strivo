//! HTTP client for the activation backend.
//!
//! The same calls the SPA reaches via `/api/v1/licence/*` also need
//! to be made from inside the daemon — for the scheduled 72h refresh
//! task (Phase 3d) — so the actual HTTP work lives here in the core
//! crate. The web crate's route handlers and the daemon's background
//! task both call into these functions.
//!
//! ES256 JWT signature + claim verification lives in `strivo-web`'s
//! `routes::licence` (`verify_licence_token`) and gates the SPA's
//! activate/trial/refresh calls before a `Licence` is persisted from
//! them — the core crate can't depend on strivo-web to reuse it here.
//! `refresh_now` below (the daemon's own 72h background refresh, see
//! `spawn_refresh_loop`) is a separate call path and does not yet run
//! the same signature check; wiring an equivalent core-crate verifier
//! into this path is tracked as follow-up hardening, not covered by
//! this module today.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::cache::{Licence, LicenceCache, Tier};
use super::machine_id::hashed_machine_id;

#[derive(Debug, Deserialize)]
struct BackendTokenResponse {
    token: String,
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
    let tier = match parsed.tier.as_str() {
        "pro" => Tier::Pro,
        "trial" => Tier::Trial,
        _ => cache.tier,
    };
    let lic = Licence {
        tier,
        machine_hash: hashed_machine_id(),
        expires_at: parsed.expires_at,
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
