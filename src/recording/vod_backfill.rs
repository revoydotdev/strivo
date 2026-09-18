//! Twitch VOD backfill.
//!
//! When a Twitch live recording ends, the live HLS pull will have missed
//! the first ~5 minutes of broadcast (DVR window limit) and any black
//! frames left behind by streamlink's ad suppression. Twitch publishes a
//! full archive VOD a few minutes after the stream ends — backfilling
//! from it gives us the complete broadcast.
//!
//! Flow:
//! 1. Wait `delay_secs` seconds for the VOD to finalize on Twitch's side.
//! 2. Query helix `/videos?user_id=X&type=archive&first=5`.
//! 3. Pick the most recent archive whose `published_at` lands within ±2h
//!    of the live recording's start (Twitch's published_at is the broadcast
//!    start, not finalize time).
//! 4. Send `RecordingCommand::DownloadVod` with `<base>_vod.<ext>` as the
//!    output path — the live capture is preserved alongside.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::platform::twitch::TwitchPlatform;
use crate::platform::{Platform, PlatformKind};
use crate::recording::RecordingCommand;

#[derive(Debug, Clone)]
pub struct BackfillRequest {
    pub channel_id: String,
    pub channel_name: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub live_output_path: PathBuf,
    pub stream_title: Option<String>,
    pub delay_secs: u64,
}

/// Spawn-and-forget task that waits for Twitch to finalize the VOD, then
/// queues its download via the recording manager. Failure is logged at
/// warn level and otherwise silent — the live capture is the user's
/// safety net.
///
/// The `cancel` token is threaded from the daemon's global
/// [`CancellationToken`] so that a clean shutdown cancels any pending
/// backfill delay instead of hanging for up to `delay_secs`.
pub fn spawn(
    req: BackfillRequest,
    twitch: Arc<RwLock<TwitchPlatform>>,
    recording_tx: mpsc::UnboundedSender<RecordingCommand>,
    config: AppConfig,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        if let Err(e) = run(req, twitch, recording_tx, config, cancel).await {
            tracing::warn!(error = %e, "vod backfill failed");
        }
    });
}

async fn run(
    req: BackfillRequest,
    twitch: Arc<RwLock<TwitchPlatform>>,
    recording_tx: mpsc::UnboundedSender<RecordingCommand>,
    config: AppConfig,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    tracing::info!(
        channel = %req.channel_name,
        delay_secs = req.delay_secs,
        "vod backfill: scheduled"
    );
    // Race the delay against the daemon shutdown token so the task
    // participates in graceful shutdown instead of blocking exit for up
    // to `delay_secs`.
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(req.delay_secs)) => {}
        _ = cancel.cancelled() => {
            tracing::info!(
                channel = %req.channel_name,
                "vod backfill: cancelled (daemon shutdown)"
            );
            return Ok(());
        }
    }

    let twitch_guard = twitch.read().await;
    // Search the last 7 days; helix returns newest-first, so the first
    // matching archive is what we want.
    let since = req.started_at - chrono::Duration::days(7);
    let vods = twitch_guard
        .fetch_channel_vods(&req.channel_id, Some(since), Some(5))
        .await
        .map_err(|e| anyhow::anyhow!("fetch_channel_vods: {e}"))?;
    drop(twitch_guard);

    let match_window = chrono::Duration::hours(2);
    let chosen = vods.into_iter().find(|v| {
        v.published_at
            .map(|p| (p - req.started_at).num_seconds().abs() <= match_window.num_seconds())
            .unwrap_or(false)
    });

    let Some(vod) = chosen else {
        tracing::info!(
            channel = %req.channel_name,
            started_at = %req.started_at,
            "vod backfill: no matching archive within ±2h — channel may have \"Store past broadcasts\" disabled"
        );
        return Ok(());
    };

    tracing::info!(
        channel = %req.channel_name,
        vod_id = %vod.id,
        url = %vod.url,
        "vod backfill: starting download"
    );

    // `AdjacentTo` co-locates the VOD under `<base>_vod.<ext>` next to
    // the live capture, replacing the bespoke local helper. `Inherit`
    // for cookies — Twitch past broadcasts are public and use the same
    // anonymous yt-dlp path as the live capture.
    let spec = crate::intents::DownloadVodSpec {
        url: vod.url,
        channel_name: req.channel_name,
        platform: PlatformKind::Twitch,
        post_title: req.stream_title.or(Some(vod.title)),
        cookies: crate::intents::CookieSource::Inherit,
        output_policy: crate::intents::OutputPathPolicy::AdjacentTo(req.live_output_path),
        channel_id: Some(req.channel_id.clone()),
    };
    recording_tx
        .send(crate::intents::download_vod(spec, &config))
        .map_err(|e| anyhow::anyhow!("recording_tx send: {e}"))?;

    Ok(())
}

// The path-suffix policy moved to `crate::intents::download_vod` and is
// tested there (`adjacent_policy_appends_vod_suffix`,
// `adjacent_policy_handles_missing_extension`). The backfill module no
// longer needs its own test fixtures for path computation.

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    /// A pre-cancelled token must cause the `select!` guard to take the
    /// cancellation branch immediately, not wait out the full `delay_secs`.
    #[tokio::test]
    async fn delay_exits_immediately_when_pre_cancelled() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let start = std::time::Instant::now();
        let cancelled = tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3600)) => false,
            _ = cancel.cancelled() => true,
        };
        assert!(
            cancelled,
            "pre-cancelled token must win the select, not the sleep"
        );
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "select should resolve within 200 ms, not after the 3600 s delay"
        );
    }

    /// A live token that is cancelled during an in-progress delay must also
    /// unblock promptly.
    #[tokio::test]
    async fn delay_exits_on_mid_flight_cancel() {
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Trigger cancellation from a separate task after a short pause.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_clone.cancel();
        });

        let start = std::time::Instant::now();
        let cancelled = tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3600)) => false,
            _ = cancel.cancelled() => true,
        };
        assert!(
            cancelled,
            "delayed cancel must still win over the 3600 s sleep"
        );
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "should unblock well before the 3600 s delay"
        );
    }
}
