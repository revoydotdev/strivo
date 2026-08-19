//! Outbound webhook dispatcher.
//!
//! When [`WebhookConfig::enabled`](crate::config::WebhookConfig::enabled) is
//! true and a `url` is configured, the daemon POSTs a JSON payload for each
//! notification-worthy [`DaemonEvent`] (channel live, recording finished /
//! failed, generic `Notification`). The POST runs on a spawned task —
//! fire-and-forget, errors are logged and swallowed, the event loop is never
//! blocked on an HTTP call.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::NotificationsConfig;
use crate::events::DaemonEvent;

/// JSON body POSTed to the webhook endpoint.
///
/// Shape mirrors streamerREC's webhook contract (`{event, channel_id, name,
/// platform, recording_id, status, filename, bytes, error}`) so existing
/// Make / n8n / Zapier integrations route both tools without adapter changes.
/// All fields except `event` are optional; unset fields are omitted from the
/// serialised payload (no `null` noise).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebhookPayload {
    /// Event discriminator. One of: `"channel_live"`, `"recording_finished"`,
    /// `"recording_failed"`, `"notification"`.
    pub event: String,
    /// Platform channel ID (e.g. Twitch user-id or YouTube channel-id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    /// Display name of the channel / creator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Platform name (`"Twitch"`, `"YouTube"`, `"Patreon"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// StriVo recording UUID. Set on recording events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<Uuid>,
    /// Terminal recording state (`"finished"`, `"failed"`, `"live"`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Output file path as a string. Set on `RecordingStarted`; not available
    /// on `RecordingFinished` (the job snapshot is not in the event itself).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Bytes written at the time of the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// Error message when the recording failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Title for generic `Notification` events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Body for generic `Notification` events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Build a webhook payload from a `DaemonEvent`.
///
/// Returns `None` for events that don't warrant an outbound POST (channel
/// list updates, progress ticks, auth flows, etc.). This is a pure function
/// with no I/O so it can be unit-tested without a running daemon.
pub fn build_payload(event: &DaemonEvent) -> Option<WebhookPayload> {
    use crate::recording::job::RecordingState;

    match event {
        DaemonEvent::ChannelWentLive(ch) => Some(WebhookPayload {
            event: "channel_live".to_string(),
            channel_id: Some(ch.id.clone()),
            name: Some(ch.display_name.clone()),
            platform: Some(ch.platform.to_string()),
            recording_id: None,
            status: Some("live".to_string()),
            filename: None,
            bytes: None,
            error: None,
            title: None,
            body: None,
        }),

        DaemonEvent::RecordingStarted { job } => Some(WebhookPayload {
            event: "recording_started".to_string(),
            channel_id: Some(job.channel_id.clone()),
            name: Some(job.channel_name.clone()),
            platform: Some(job.platform.to_string()),
            recording_id: Some(job.id),
            status: Some("recording".to_string()),
            filename: Some(job.output_path.display().to_string()),
            bytes: Some(job.bytes_written),
            error: None,
            title: None,
            body: None,
        }),

        DaemonEvent::RecordingFinished {
            job_id,
            final_state,
            error,
        } => {
            let failed = error.is_some() || matches!(final_state, RecordingState::Failed);
            let event_name = if failed {
                "recording_failed"
            } else {
                "recording_finished"
            };
            Some(WebhookPayload {
                event: event_name.to_string(),
                channel_id: None,
                name: None,
                platform: None,
                recording_id: Some(*job_id),
                status: Some(format!("{final_state:?}").to_lowercase()),
                filename: None,
                bytes: None,
                error: error.clone(),
                title: None,
                body: None,
            })
        }

        DaemonEvent::Notification { title, body } => Some(WebhookPayload {
            event: "notification".to_string(),
            channel_id: None,
            name: None,
            platform: None,
            recording_id: None,
            status: None,
            filename: None,
            bytes: None,
            error: None,
            title: Some(title.clone()),
            body: Some(body.clone()),
        }),

        _ => None,
    }
}

/// Dispatch an outbound webhook for a daemon event (fire-and-forget).
///
/// If the webhook is disabled or no URL is configured this is a cheap
/// synchronous no-op. When enabled, the POST is spawned onto a Tokio task and
/// the function returns immediately — the event loop is never blocked.
pub fn dispatch_webhook(cfg: &NotificationsConfig, event: &DaemonEvent) {
    if !cfg.webhook.enabled {
        return;
    }
    let Some(url) = cfg.webhook.url.clone() else {
        return;
    };
    let Some(payload) = build_payload(event) else {
        return;
    };
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        match client.post(&url).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(url = %url, event = %payload.event, "webhook delivered");
            }
            Ok(resp) => {
                tracing::warn!(
                    url = %url,
                    event = %payload.event,
                    status = %resp.status(),
                    "webhook returned non-2xx"
                );
            }
            Err(e) => {
                tracing::warn!(url = %url, event = %payload.event, "webhook POST failed: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{ChannelEntry, PlatformKind};
    use crate::recording::job::{RecordingJob, RecordingState};

    fn make_channel() -> ChannelEntry {
        ChannelEntry {
            id: "123456".to_string(),
            name: "testchan".to_string(),
            display_name: "TestChan".to_string(),
            platform: PlatformKind::Twitch,
            is_live: true,
            auto_record: false,
            stream_title: None,
            game_or_category: None,
            viewer_count: None,
            started_at: None,
            thumbnail_url: None,
            last_live_at: None,
            live_video_id: None,
        }
    }

    #[test]
    fn channel_went_live_produces_payload() {
        let ch = make_channel();
        let event = DaemonEvent::ChannelWentLive(ch);
        let payload = build_payload(&event).expect("should produce a payload");
        assert_eq!(payload.event, "channel_live");
        assert_eq!(payload.channel_id.as_deref(), Some("123456"));
        assert_eq!(payload.name.as_deref(), Some("TestChan"));
        assert_eq!(payload.platform.as_deref(), Some("Twitch"));
        assert_eq!(payload.status.as_deref(), Some("live"));
        assert!(payload.error.is_none());
    }

    #[test]
    fn recording_started_includes_filename() {
        let job = RecordingJob::new(
            "ch1".into(),
            "Chan".into(),
            PlatformKind::Twitch,
            std::path::PathBuf::from("/recordings/test.mkv"),
            false,
            None,
        );
        let event = DaemonEvent::RecordingStarted { job: job.clone() };
        let payload = build_payload(&event).expect("payload");
        assert_eq!(payload.event, "recording_started");
        assert_eq!(payload.recording_id, Some(job.id));
        assert_eq!(payload.filename.as_deref(), Some("/recordings/test.mkv"));
        assert_eq!(payload.bytes, Some(0));
    }

    #[test]
    fn recording_finished_success() {
        let job_id = Uuid::new_v4();
        let event = DaemonEvent::RecordingFinished {
            job_id,
            final_state: RecordingState::Finished,
            error: None,
        };
        let payload = build_payload(&event).expect("payload");
        assert_eq!(payload.event, "recording_finished");
        assert_eq!(payload.recording_id, Some(job_id));
        assert!(payload.error.is_none());
    }

    #[test]
    fn recording_finished_failure_maps_to_failed_event() {
        let job_id = Uuid::new_v4();
        let event = DaemonEvent::RecordingFinished {
            job_id,
            final_state: RecordingState::Failed,
            error: Some("ffmpeg exited 1".to_string()),
        };
        let payload = build_payload(&event).expect("payload");
        assert_eq!(payload.event, "recording_failed");
        assert_eq!(payload.error.as_deref(), Some("ffmpeg exited 1"));
    }

    #[test]
    fn recording_finished_with_error_field_is_failed() {
        let job_id = Uuid::new_v4();
        // Even with Finished state, a non-None error is classified as failed.
        let event = DaemonEvent::RecordingFinished {
            job_id,
            final_state: RecordingState::Finished,
            error: Some("incomplete write".to_string()),
        };
        let payload = build_payload(&event).expect("payload");
        assert_eq!(payload.event, "recording_failed");
    }

    #[test]
    fn notification_event_payload() {
        let event = DaemonEvent::Notification {
            title: "VOD ready".to_string(),
            body: "Download complete".to_string(),
        };
        let payload = build_payload(&event).expect("payload");
        assert_eq!(payload.event, "notification");
        assert_eq!(payload.title.as_deref(), Some("VOD ready"));
        assert_eq!(payload.body.as_deref(), Some("Download complete"));
    }

    #[test]
    fn uninteresting_events_produce_no_payload() {
        assert!(build_payload(&DaemonEvent::ChannelsUpdated(vec![])).is_none());
        assert!(build_payload(&DaemonEvent::AllRecordingsStopped).is_none());
    }

    #[test]
    fn dispatch_webhook_is_noop_when_disabled() {
        use crate::config::NotificationsConfig;
        // Default has webhook.enabled = false — should be a pure no-op.
        let cfg = NotificationsConfig::default();
        assert!(!cfg.webhook.enabled);
        let ch = make_channel();
        let event = DaemonEvent::ChannelWentLive(ch);
        // Must not panic or spawn (no Tokio runtime needed here).
        dispatch_webhook(&cfg, &event);
    }
}
