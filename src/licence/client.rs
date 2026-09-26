//! HTTP client for the activation backend.
//!
//! The same calls the SPA reaches via `/api/v1/licence/*` also need
//! to be made from inside the daemon — for the scheduled 72h refresh
//! task (Phase 3d) — so the actual HTTP work lives here in the core
//! crate. The web crate's route handlers and the daemon's background
//! task both call into these functions.
//!
//! Every returned JWT is verified before it reaches the local cache.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::cache::{Licence, LicenceCache, Tier};
use super::reconnect::RefreshOutcome;

#[derive(Debug, Deserialize)]
struct BackendTokenResponse {
    token: String,
    tier: String,
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
/// what the attempt did, so the caller can log "we are still connected"
/// differently from "this licence is dead" — see
/// [`RefreshOutcome`](super::reconnect::RefreshOutcome) for why that is not
/// the same question as the on-disk cache state.
///
/// `Err` reasons the caller should distinguish:
///   - **No backend URL** → user hasn't pointed at one yet; this is a
///     no-op, not a failure.
///   - **No cache** → free tier, nothing to refresh.
///   - **Nothing renewable on disk** → the record is unparsable, unbound, or
///     has no licence key. Presenting it to a server would be sending a
///     payload we cannot attribute, so it is not attempted.
///   - **Unverifiable token** → the server answered, but the answer is not
///     signed for us. The old cache is kept; a forged or misrouted backend
///     must not end a session, and must not grant one either.
///
/// A transient backend failure is `Ok(Offline)` with the cache untouched —
/// the "no internet kill" rule, which is a decision not an accident.
pub async fn refresh_now() -> Result<RefreshOutcome> {
    let Some(url) = backend_url() else {
        anyhow::bail!("STRIVO_LICENCE_URL unset");
    };
    let trust =
        super::reconnect::TokenTrust::from_env().context("STRIVO_LICENCE_PUBLIC_KEY unset")?;
    refresh_with(&url, &LicenceCache::path(), &trust).await
}

/// [`refresh_now`] with the backend, the cache path and the trust supplied.
///
/// The separation exists so the three outcomes — reconnected, offline,
/// revoked — are reachable from a test against a real socket with no
/// environment variable, no shared state directory, and therefore no
/// ordering constraint between tests. `refresh_now` is only the three
/// lookups.
pub async fn refresh_with(
    url: &str,
    path: &std::path::Path,
    trust: &super::reconnect::TokenTrust,
) -> Result<RefreshOutcome> {
    let Some(cache) = LicenceCache::load_at(path) else {
        anyhow::bail!("no licence on file");
    };
    if !super::reconnect::renewable(path, &trust.machine_hash) {
        anyhow::bail!("no renewable licence on file for this machine");
    }
    let body = serde_json::json!({
        "licence_key": cache.licence_key,
        "machine_hash": trust.machine_hash,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let resp = match client
        .post(format!("{url}/refresh"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(RefreshOutcome::Offline {
                detail: format!("backend unreachable: {e}"),
            })
        }
    };
    let status = resp.status();
    let raw = resp.text().await.unwrap_or_default();

    if status.as_u16() == 403 {
        // Server denied — licence revoked / refunded. Clear the cache so the
        // gate fails closed on the next read, and report it as terminal: with
        // the file gone there is nothing left to present, so no later tick
        // retries until a new key is activated.
        super::cache::LicenceCache::clear_at(path).ok();
        return Ok(RefreshOutcome::Revoked);
    }
    if !status.is_success() {
        return Ok(RefreshOutcome::Offline {
            detail: format!("backend {status}"),
        });
    }
    let parsed: BackendTokenResponse = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            return Ok(RefreshOutcome::Offline {
                detail: format!("parse backend response: {e}"),
            })
        }
    };
    let claims = super::verify::verify_token_with(
        &parsed.token,
        &trust.public_key_pem,
        &trust.issuer,
        &trust.machine_hash,
    )
    .context("licence backend returned an unverifiable token")?;
    if claims.tier != parsed.tier {
        anyhow::bail!("licence backend tier does not match signed claims");
    }
    let tier = match claims.tier.as_str() {
        "pro" => Tier::Pro,
        "trial" => Tier::Trial,
        // The session survives an unlabelled re-sign: the tier already on
        // file is the licence's own, so an answer that omits it is not a
        // reason to drop the user to free.
        _ => cache.tier,
    };
    let lic = Licence {
        tier,
        machine_hash: trust.machine_hash.clone(),
        expires_at: claims.licence_exp,
        last_refreshed: chrono::Utc::now().to_rfc3339(),
        token: parsed.token,
        licence_key: cache.licence_key.clone(),
    };
    LicenceCache::save_at(path, &lic)?;
    Ok(RefreshOutcome::Reconnected { tier })
}

/// Spawn the periodic refresh task, but only when the operator has both
/// opted into the licence-cache path *and* pointed at a backend. Runs
/// forever once spawned, on the async runtime — no blocking call, no sleep:
///   - wait `interval`
///   - if there's a cache, try a refresh
///   - report the outcome; a backend that is merely unreachable keeps the
///     cache, a revoked licence loses it (once, terminally)
///
/// `None` unless `[licence] cache_reconnection` is on *and*
/// `STRIVO_LICENCE_URL` is set. Every edition (PVR included) shares this
/// daemon entrypoint, and the licence backend is on an explicit release hold
/// (README's "Release boundary"; ADR 0002 / CE06), so a default install —
/// flag off, env var unset — starts zero background tasks and makes zero
/// network calls, rather than a live-but-idle task that only happens to
/// no-op today. Returns the JoinHandle so the daemon can shut it down on
/// exit when it does run.
pub fn spawn_refresh_loop(interval: std::time::Duration) -> Option<tokio::task::JoinHandle<()>> {
    if !super::reconnect::enabled() {
        return None;
    }
    backend_url()?;
    Some(tokio::spawn(async move {
        // First tick after one interval — at startup we want to come
        // up immediately even if the network is down, then catch up
        // on the first scheduled fire.
        let mut next = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        loop {
            next.tick().await;
            if backend_url().is_none() {
                continue; // config changed under us; nothing to do
            }
            // Bounded by construction: one attempt per tick, a 15s client
            // timeout, and no retry inside. A backend that is down costs one
            // timeout per 72h, not a spin.
            match refresh_now().await {
                Ok(RefreshOutcome::Reconnected { tier }) => {
                    tracing::info!(?tier, "licence reconnected (next attempt in {interval:?})");
                }
                Ok(RefreshOutcome::Revoked) => {
                    tracing::warn!("licence revoked by the backend; cache cleared");
                }
                Ok(RefreshOutcome::Offline { detail }) => {
                    // INFO not WARN — an offline daemon is the expected case
                    // and must not pollute logs.
                    tracing::info!(reason = %detail, "licence reconnect skipped; cache kept");
                }
                Err(e) => {
                    tracing::info!(reason = %e, "nothing to reconnect");
                }
            }
        }
    }))
}

/// Default refresh cadence: 72 hours. The CF Worker's
/// `REFRESH_INTERVAL_HOURS` defaults to the same; keep them in sync.
pub const DEFAULT_REFRESH_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(72 * 60 * 60);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::licence::reconnect::TokenTrust;
    use base64::Engine;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    use std::path::{Path, PathBuf};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const MACHINE: &str = "reconnect-test-machine";

    struct Signer {
        pem: String,
        pair: EcdsaKeyPair,
    }

    impl Signer {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            let pem = format!(
                "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
                base64::engine::general_purpose::STANDARD.encode(pair.public_key().as_ref())
            );
            Self { pem, pair }
        }

        fn token(&self, tier: &str, exp_in_secs: i64) -> String {
            let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let header = enc.encode(br#"{"alg":"ES256","typ":"JWT"}"#);
            let claims = enc.encode(
                format!(
                    r#"{{"iss":"{ISSUER}","sub":"{MACHINE}","tier":"{tier}","exp":{}}}"#,
                    chrono::Utc::now().timestamp() + exp_in_secs
                )
                .as_bytes(),
            );
            let input = format!("{header}.{claims}");
            let sig = self
                .pair
                .sign(&SystemRandom::new(), input.as_bytes())
                .unwrap();
            format!("{input}.{}", enc.encode(sig.as_ref()))
        }
    }

    const ISSUER: &str = "https://licence.test.invalid";

    fn cache_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("strivo-licence-client-tests")
            .join(format!("{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("licence.json")
    }

    /// One request, one canned response, then the listener closes — enough
    /// for a single `reqwest` POST and not a mock of our own client. The
    /// transport is real, so an offline-vs-refused distinction cannot pass by
    /// accident.
    async fn serve_once(status_line: &'static str, body: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        });
        format!("http://{addr}")
    }

    fn expired_record() -> Licence {
        Licence {
            tier: Tier::Pro,
            machine_hash: MACHINE.into(),
            expires_at: None,
            last_refreshed: chrono::Utc::now().to_rfc3339(),
            token: "stale".into(),
            licence_key: "lic-key-42".into(),
        }
    }

    fn trust(pem: &str) -> TokenTrust {
        TokenTrust {
            issuer: ISSUER.into(),
            public_key_pem: pem.into(),
            machine_hash: MACHINE.into(),
        }
    }

    fn seed(path: &Path, lic: &Licence) {
        LicenceCache::save_at(path, lic).unwrap();
    }

    /// The session survives: a reconnect replaces the token and the
    /// timestamps, and keeps the licence key, the machine binding and the
    /// tier. Nothing here re-presents a key, and the file the gate reads is
    /// the one the refresh wrote.
    #[tokio::test]
    async fn reconnecting_an_expired_cache_keeps_the_licence_session() {
        let signer = Signer::new();
        let path = cache_path("reconnect");
        seed(&path, &expired_record());
        let fresh = signer.token("pro", 3_600);
        let body = serde_json::json!({ "token": fresh, "tier": "pro" }).to_string();
        let url = serve_once("200 OK", body).await;

        let outcome = refresh_with(&url, &path, &trust(&signer.pem))
            .await
            .unwrap();
        assert_eq!(outcome, RefreshOutcome::Reconnected { tier: Tier::Pro });

        let after = LicenceCache::load_at(&path).expect("cache rewritten");
        assert_eq!(after.licence_key, "lic-key-42", "the key must survive");
        assert_eq!(after.machine_hash, MACHINE, "the binding must survive");
        assert_eq!(after.tier, Tier::Pro);
        assert_eq!(after.token, fresh, "the token must be the new one");
        // And the gate now sees the new token as the entitlement.
        assert!(crate::licence::reconnect::classify(&path, &trust(&signer.pem)).grants());
    }

    /// Revocation is the branch that destroys state, so it is asserted on
    /// both the outcome and the filesystem: the cache is gone, so a later
    /// tick has nothing to present and cannot retry into a grant.
    #[tokio::test]
    async fn a_revoked_licence_is_cleared_and_is_not_retryable() {
        let signer = Signer::new();
        let path = cache_path("revoked");
        seed(&path, &expired_record());
        let url = serve_once("403 Forbidden", r#"{"error":"revoked"}"#.into()).await;

        let outcome = refresh_with(&url, &path, &trust(&signer.pem))
            .await
            .unwrap();
        assert_eq!(outcome, RefreshOutcome::Revoked);
        assert!(!path.exists(), "revocation must delete the cache");

        let again = serve_once("200 OK", "{}".into()).await;
        let second = refresh_with(&again, &path, &trust(&signer.pem)).await;
        assert!(
            second.is_err(),
            "with no cache there is nothing to present, so revocation is terminal"
        );
    }

    /// A backend that is merely down must not end a paid session. The file
    /// is left byte-for-byte as it was.
    #[tokio::test]
    async fn an_unreachable_backend_keeps_the_cache_untouched() {
        let signer = Signer::new();
        let path = cache_path("offline");
        seed(&path, &expired_record());
        let before = std::fs::read(&path).unwrap();

        // A port nothing listens on: a real connection refusal, not a stub.
        let outcome = refresh_with("http://127.0.0.1:9", &path, &trust(&signer.pem))
            .await
            .unwrap();
        assert!(
            matches!(outcome, RefreshOutcome::Offline { .. }),
            "{outcome:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// A 5xx is an outage, not a revocation: the same cache survives.
    #[tokio::test]
    async fn a_server_error_keeps_the_cache_untouched() {
        let signer = Signer::new();
        let path = cache_path("server-error");
        seed(&path, &expired_record());
        let before = std::fs::read(&path).unwrap();
        let url = serve_once("500 Internal Server Error", r#"{"error":"boom"}"#.into()).await;

        let outcome = refresh_with(&url, &path, &trust(&signer.pem))
            .await
            .unwrap();
        assert!(
            matches!(outcome, RefreshOutcome::Offline { .. }),
            "{outcome:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// A backend that answers with a token signed by somebody else must not
    /// be able to end a session or grant one.
    #[tokio::test]
    async fn a_token_we_cannot_verify_is_refused_and_the_cache_is_kept() {
        let signer = Signer::new();
        let impostor = Signer::new();
        let path = cache_path("impostor");
        seed(&path, &expired_record());
        let before = std::fs::read(&path).unwrap();
        let body =
            serde_json::json!({ "token": impostor.token("pro", 3_600), "tier": "pro" }).to_string();
        let url = serve_once("200 OK", body).await;

        let outcome = refresh_with(&url, &path, &trust(&signer.pem)).await;
        assert!(outcome.is_err(), "an unverifiable token must be an error");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// A record for another machine is not renewable, so no request is made
    /// at all — presenting a foreign licence would be sending a payload we
    /// cannot attribute.
    #[tokio::test]
    async fn a_foreign_record_is_not_presented_to_the_backend() {
        let signer = Signer::new();
        let path = cache_path("foreign-refresh");
        let mut lic = expired_record();
        lic.machine_hash = "another-machine".into();
        seed(&path, &lic);
        // Nothing is listening on this port, so a request would surface as
        // Offline rather than an error.
        let outcome = refresh_with("http://127.0.0.1:9", &path, &trust(&signer.pem)).await;
        assert!(outcome.is_err(), "{outcome:?}");
    }

    /// The atomic-write property the reconnection depends on: the file the
    /// gate reads is never a partial one, and no temp file is left behind.
    #[test]
    fn a_saved_cache_leaves_no_temp_file_and_reads_back_whole() {
        let path = cache_path("atomic");
        seed(&path, &expired_record());
        let mut second = expired_record();
        second.token = "replacement".into();
        LicenceCache::save_at(&path, &second).unwrap();

        assert_eq!(
            LicenceCache::load_at(&path).unwrap().token,
            "replacement",
            "a save must replace the record completely"
        );
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }
}
