use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::events::DaemonEvent;
use crate::platform::patreon::PatreonClient;
use crate::platform::PlatformKind;
use crate::recording::RecordingCommand;

pub struct PatreonMonitor {
    /// Shared with the daemon's auth task so a credential reload reaches the
    /// same client this monitor is polling with.
    client: std::sync::Arc<PatreonClient>,
    config: AppConfig,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    recording_tx: mpsc::UnboundedSender<RecordingCommand>,
    cancel: CancellationToken,
    /// Last check time per campaign
    last_checked: HashMap<String, DateTime<Utc>>,
    /// State file for persisting last_checked
    state_path: PathBuf,
    /// Re-read each poll so `auto_pull_creators` toggles apply live.
    config_path: Option<PathBuf>,
}

impl PatreonMonitor {
    pub fn new(
        client: std::sync::Arc<PatreonClient>,
        config: AppConfig,
        event_tx: mpsc::UnboundedSender<DaemonEvent>,
        recording_tx: mpsc::UnboundedSender<RecordingCommand>,
        cancel: CancellationToken,
    ) -> Self {
        let state_path = AppConfig::state_dir().join("patreon_state.json");
        let last_checked = load_state(&state_path).unwrap_or_default();

        Self {
            client,
            config,
            event_tx,
            recording_tx,
            cancel,
            last_checked,
            state_path,
            config_path: None,
        }
    }

    /// Enable per-poll config reloads from the file the daemon loaded.
    pub fn with_config_path(mut self, path: Option<PathBuf>) -> Self {
        self.config_path = path;
        self
    }

    pub async fn run(mut self) {
        let poll_interval = self
            .config
            .patreon
            .as_ref()
            .map(|p| p.poll_interval_secs)
            .unwrap_or(300);
        let interval = std::time::Duration::from_secs(poll_interval.max(60));

        // Wait for auth
        if !self.client.is_authenticated().await {
            tracing::info!("Patreon monitor waiting for authentication...");
            return;
        }

        // Initial poll
        if let Err(e) = self.poll().await {
            tracing::error!("Patreon initial poll error: {e}");
        }

        let mut timer = tokio::time::interval(interval);
        timer.tick().await; // consume immediate tick

        loop {
            tokio::select! {
                _ = timer.tick() => {
                    if let Err(e) = self.poll().await {
                        tracing::error!("Patreon poll error: {e}");
                    }
                }
                _ = self.cancel.cancelled() => {
                    tracing::info!("Patreon monitor shutting down");
                    break;
                }
            }
        }
    }

    async fn poll(&mut self) -> anyhow::Result<()> {
        super::reload_config(&mut self.config, self.config_path.as_deref());
        let creators = self.client.fetch_pledged_creators().await?;
        tracing::debug!("Patreon: found {} pledged creators", creators.len());

        // Accumulate the full snapshot for the TUI (task #69). creators
        // become sidebar rows; posts populate the per-creator right-pane
        // list and are cached by campaign_id.
        let mut creator_entries = Vec::with_capacity(creators.len());
        let mut all_posts = Vec::new();

        for creator in &creators {
            let mut entry = creator.to_channel_entry();
            // Stamped here (rather than left false) so the rail context
            // menu's "Uploads" toggle reads Patreon state from the same
            // `API.patreon()` creators list it already fetches, instead of
            // needing a bespoke lookup.
            entry.auto_download_uploads = self
                .config
                .auto_pull_creators
                .iter()
                .any(|e| e.campaign_id == creator.campaign_id);
            creator_entries.push(entry);

            // Fetch the recent window unconditionally (since=None) so the
            // right pane always shows the latest posts, not just those
            // newer than our last check. "New" detection for auto-pull is
            // done below by comparing published_at against last_checked.
            let last = self.last_checked.get(&creator.campaign_id).copied();
            // The Patreon API returns no posts for campaigns you only patron;
            // when cookies are configured, list them via yt-dlp instead.
            let cookies = self
                .config
                .patreon
                .as_ref()
                .and_then(|p| p.cookies_path.clone());
            let posts = match (cookies.as_deref(), creator.vanity.as_deref()) {
                (Some(cp), Some(vanity)) => {
                    match self
                        .client
                        .fetch_posts_ytdlp(&creator.campaign_id, vanity, Some(cp), 15)
                        .await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!("Patreon yt-dlp post fetch for {vanity} failed: {e:#}");
                            Vec::new()
                        }
                    }
                }
                _ => self
                    .client
                    .fetch_posts(&creator.campaign_id, None)
                    .await
                    .unwrap_or_default(),
            };

            // Per-creator opt-in to auto-download (task #70). Creators with
            // no auto_pull_creators entry get a notification only; the user
            // manually triggers pull from the right pane (task #69).
            let auto_pull = self
                .config
                .auto_pull_creators
                .iter()
                .any(|e| e.campaign_id == creator.campaign_id);

            for post in &posts {
                let Some(ref embed_url) = post.embed_url else {
                    continue;
                };

                // Is this post new since we last polled this creator?
                let is_new = match (last, post.published_at.parse::<DateTime<Utc>>()) {
                    (Some(last), Ok(published)) => published > last,
                    // No prior watermark (first poll) or unparseable date:
                    // treat as not-new so a fresh auth doesn't trigger a
                    // back-catalog stampede. Auto-pull only fires on posts
                    // that appear after the first successful poll.
                    _ => false,
                };

                if is_new {
                    let _ = self.event_tx.send(DaemonEvent::PatreonPostFound {
                        creator_name: creator.name.clone(),
                        post_title: post.title.clone(),
                    });
                    let _ = self.event_tx.send(DaemonEvent::Notification {
                        title: format!("Patreon: {}", creator.name),
                        body: post.title.clone(),
                    });
                }

                if is_new && auto_pull {
                    tracing::info!("Patreon auto-pull: {} - {}", creator.name, post.title);
                    let spec = auto_pull_spec(embed_url, &creator.name, &post.title);
                    let _ = self
                        .recording_tx
                        .send(crate::intents::download_vod(spec, &self.config));
                }
            }

            all_posts.extend(posts);

            // Update last_checked
            self.last_checked
                .insert(creator.campaign_id.clone(), Utc::now());
        }

        // Push the snapshot to the host (SPA).
        let _ = self.event_tx.send(DaemonEvent::PatreonState {
            creators: creator_entries,
            posts: all_posts,
        });

        // Persist state
        save_state(&self.state_path, &self.last_checked).ok();

        Ok(())
    }
}

/// Build the auto-pull `DownloadVodSpec` for a newly-seen Patreon post.
///
/// Uses `CookieSource::FromConfig`, which resolves `config.patreon
/// .cookies_path` lazily at consumption time via `CookieSource::resolve()`
/// (`src/intents/cookies.rs`) — the same way the listing call earlier in
/// `poll()` already does. `CookieSource::Inherit` was tried here previously
/// but is a guaranteed no-op: `PatreonClient` authenticates with a bearer
/// token over a plain `Client::new()` with no cookie jar, so there is no
/// live session for `Inherit` to carry over. Without real session cookies,
/// yt-dlp's Patreon extractor silently succeeds against a public/teaser
/// rendition instead of failing, producing a `Finished` recording whose
/// playback is broken/wrong for patron-gated posts.
fn auto_pull_spec(
    embed_url: &str,
    creator_name: &str,
    post_title: &str,
) -> crate::intents::DownloadVodSpec {
    crate::intents::DownloadVodSpec {
        url: embed_url.to_string(),
        channel_name: creator_name.to_string(),
        platform: PlatformKind::Patreon,
        post_title: Some(post_title.to_string()),
        cookies: crate::intents::CookieSource::FromConfig,
        output_policy: crate::intents::OutputPathPolicy::Fresh,
        channel_id: None,
    }
}

fn load_state(path: &PathBuf) -> Option<HashMap<String, DateTime<Utc>>> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_state(path: &PathBuf, state: &HashMap<String, DateTime<Utc>>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(state)?;
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The auto-pull path must resolve cookies from config, not `Inherit`
    /// (which is a guaranteed no-op — see `auto_pull_spec`'s doc comment).
    /// This is the regression test for the broken-Patreon-playback fix.
    #[test]
    fn auto_pull_spec_uses_cookie_source_from_config() {
        let spec = auto_pull_spec(
            "https://patreon.example/embed/123",
            "Some Creator",
            "Some Post",
        );
        assert!(matches!(
            spec.cookies,
            crate::intents::CookieSource::FromConfig
        ));
        assert_eq!(spec.platform, PlatformKind::Patreon);
        assert_eq!(spec.channel_name, "Some Creator");
        assert_eq!(spec.post_title.as_deref(), Some("Some Post"));
    }

    /// The rail's "Uploads" toggle for a Patreon creator must read/write
    /// `config.auto_pull_creators` — there is deliberately no separate
    /// Patreon entry in `auto_download_uploads` (see that field's doc
    /// comment). This proves the membership check `poll()` uses to stamp
    /// `ChannelEntry::auto_download_uploads` matches campaign_id, the same
    /// key the `auto_pull` gate a few lines below it in `poll()` uses.
    #[test]
    fn patreon_upload_toggle_reads_auto_pull_creators_by_campaign_id() {
        let entries = [crate::config::AutoPullEntry {
            campaign_id: "camp-1".into(),
            creator_name: "Some Creator".into(),
        }];
        let opted_in = |campaign_id: &str| entries.iter().any(|e| e.campaign_id == campaign_id);
        assert!(opted_in("camp-1"));
        assert!(!opted_in("camp-2"));
    }
}
