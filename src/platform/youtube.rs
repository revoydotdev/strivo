use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::credentials;
use crate::events::DaemonEvent;
use crate::platform::{ChannelEntry, Platform, PlatformKind, VodEntry};

const YOUTUBE_API_URL: &str = "https://www.googleapis.com/youtube/v3";
const GOOGLE_AUTH_URL: &str = "https://oauth2.googleapis.com";
const GOOGLE_DEVICE_URL: &str = "https://oauth2.googleapis.com/device/code";

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u64,
    interval: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionListResponse {
    items: Option<Vec<SubscriptionItem>>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionItem {
    snippet: Option<SubscriptionSnippet>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionSnippet {
    #[serde(rename = "resourceId")]
    resource_id: Option<ResourceId>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResourceId {
    #[serde(rename = "channelId")]
    channel_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VideoListResponse {
    items: Option<Vec<VideoItem>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct VideoItem {
    id: Option<String>,
    snippet: Option<VideoSnippet>,
    #[serde(rename = "liveStreamingDetails")]
    live_streaming_details: Option<LiveStreamingDetails>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct VideoSnippet {
    title: Option<String>,
    #[serde(rename = "channelId")]
    channel_id: Option<String>,
    #[serde(rename = "channelTitle")]
    channel_title: Option<String>,
    #[serde(rename = "categoryId")]
    category_id: Option<String>,
    thumbnails: Option<Thumbnails>,
    /// "live", "upcoming", or "none". This is the authoritative liveness
    /// signal; `liveStreamingDetails.activeLiveChatId` is not, because a
    /// stream with chat disabled is live without one.
    #[serde(rename = "liveBroadcastContent")]
    live_broadcast_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Thumbnails {
    medium: Option<ThumbnailInfo>,
    high: Option<ThumbnailInfo>,
}

#[derive(Debug, Deserialize)]
struct ThumbnailInfo {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LiveStreamingDetails {
    #[serde(rename = "actualStartTime")]
    actual_start_time: Option<String>,
    /// Set once the broadcast has finished. Its absence alongside a start
    /// time is the corroborating signal that a stream is still running.
    #[serde(rename = "actualEndTime")]
    actual_end_time: Option<String>,
    #[serde(rename = "concurrentViewers")]
    concurrent_viewers: Option<String>,
}

/// Broadcast state of a candidate video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Liveness {
    /// Streaming right now.
    Live,
    /// Scheduled but not started. Must stay eligible for re-checking.
    Upcoming,
    /// A finished broadcast or an ordinary upload. Safe to stop re-checking.
    Settled,
}

#[allow(dead_code)]
pub struct YouTubePlatform {
    client: Client,
    client_id: String,
    client_secret: String,
    cookies_path: Option<std::path::PathBuf>,
    access_token: Arc<RwLock<Option<String>>>,
    refresh_token_value: Arc<RwLock<Option<String>>>,
    pub pending_device_code: Arc<RwLock<Option<DeviceCodeInfo>>>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<DaemonEvent>>,
    /// Cached subscription list + when it was fetched. Subs barely change, so
    /// we refresh on a long TTL instead of every poll (quota: 1 unit/page).
    subs_cache: Arc<RwLock<Option<(std::time::Instant, Vec<ChannelEntry>)>>>,
    /// Video IDs confirmed NOT live (ended/VOD). The RSS-first live check
    /// skips these so `videos.list` only fires for new or still-live videos —
    /// the fix for the 10k/day quota exhaustion.
    dead_videos: Arc<RwLock<std::collections::HashSet<String>>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DeviceCodeInfo {
    pub user_code: String,
    pub verification_url: String,
}

impl YouTubePlatform {
    pub fn new(
        client_id: String,
        client_secret: String,
        cookies_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            client: Client::new(),
            client_id,
            client_secret,
            cookies_path,
            access_token: Arc::new(RwLock::new(None)),
            refresh_token_value: Arc::new(RwLock::new(None)),
            pending_device_code: Arc::new(RwLock::new(None)),
            event_tx: None,
            subs_cache: Arc::new(RwLock::new(None)),
            dead_videos: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    pub fn set_event_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<DaemonEvent>) {
        self.event_tx = Some(tx);
    }

    pub async fn load_stored_tokens(&self) -> Result<bool> {
        if let Some(token) = credentials::get_secret("youtube_access_token")? {
            *self.access_token.write().await = Some(token);
            if let Some(refresh) = credentials::get_secret("youtube_refresh_token")? {
                *self.refresh_token_value.write().await = Some(refresh);
            }
            if self.validate_token().await? {
                return Ok(true);
            }
            if self.refresh_token_value.read().await.is_some()
                && self.do_refresh_token().await.is_ok()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn validate_token(&self) -> Result<bool> {
        let token = self.access_token.read().await;
        let Some(token) = token.as_ref() else {
            return Ok(false);
        };
        let resp = self
            .client
            .get("https://www.googleapis.com/oauth2/v1/tokeninfo")
            .query(&[("access_token", token.as_str())])
            .send()
            .await?;
        Ok(resp.status().is_success())
    }

    async fn device_code_flow(&self) -> Result<()> {
        let resp: DeviceCodeResponse = self
            .client
            .post(GOOGLE_DEVICE_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("scope", "https://www.googleapis.com/auth/youtube.readonly"),
            ])
            .send()
            .await?
            .json()
            .await
            .context("Failed to get device code from Google")?;

        *self.pending_device_code.write().await = Some(DeviceCodeInfo {
            user_code: resp.user_code.clone(),
            verification_url: resp.verification_url.clone(),
        });

        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(DaemonEvent::DeviceCodeRequired {
                kind: PlatformKind::YouTube,
                verification_uri: resp.verification_url.clone(),
                user_code: resp.user_code.clone(),
            });
        }

        tracing::info!(
            "YouTube auth: go to {} and enter code: {}",
            resp.verification_url,
            resp.user_code
        );

        let interval = std::time::Duration::from_secs(resp.interval.max(5));
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(resp.expires_in);

        loop {
            tokio::time::sleep(interval).await;

            if tokio::time::Instant::now() > deadline {
                *self.pending_device_code.write().await = None;
                bail!("Device code expired");
            }

            let token_resp = self
                .client
                .post(format!("{GOOGLE_AUTH_URL}/token"))
                .form(&[
                    ("client_id", self.client_id.as_str()),
                    ("client_secret", self.client_secret.as_str()),
                    ("device_code", resp.device_code.as_str()),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await?;

            let status = token_resp.status();
            let body = token_resp.text().await?;

            if status.is_success() {
                let token: TokenResponse = serde_json::from_str(&body)?;
                credentials::store_secret("youtube_access_token", &token.access_token)?;
                if let Some(ref refresh) = token.refresh_token {
                    credentials::store_secret("youtube_refresh_token", refresh)?;
                    *self.refresh_token_value.write().await = Some(refresh.clone());
                }
                *self.access_token.write().await = Some(token.access_token);
                *self.pending_device_code.write().await = None;
                return Ok(());
            }

            if let Ok(err) = serde_json::from_str::<TokenErrorResponse>(&body) {
                match err.error.as_deref() {
                    Some("authorization_pending") | Some("slow_down") => continue,
                    Some(other) => bail!("OAuth error: {other}"),
                    None => continue,
                }
            }
        }
    }

    async fn do_refresh_token(&self) -> Result<()> {
        let refresh = self.refresh_token_value.read().await.clone();
        let Some(refresh) = refresh else {
            bail!("No refresh token available");
        };

        let resp = self
            .client
            .post(format!("{GOOGLE_AUTH_URL}/token"))
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", refresh.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            bail!("YouTube token refresh failed: {}", resp.status());
        }

        let token: TokenResponse = resp.json().await?;
        credentials::store_secret("youtube_access_token", &token.access_token)?;
        *self.access_token.write().await = Some(token.access_token);
        Ok(())
    }

    async fn api_get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        for attempt in 0..3 {
            let token = self.access_token.read().await.clone();
            let Some(token) = token else {
                bail!("Not authenticated");
            };
            let resp = self
                .client
                .get(url)
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await?;
            let status = resp.status().as_u16();
            if status == 401 && attempt == 0 {
                drop(resp);
                self.do_refresh_token().await?;
                continue;
            }
            if status == 429 || status == 503 {
                let backoff = crate::platform::parse_retry_after(&resp)
                    .unwrap_or_else(|| std::time::Duration::from_secs(5 * (1 << attempt)));
                tracing::warn!(url = %url, secs = backoff.as_secs(), "YouTube rate-limited; backing off");
                drop(resp);
                tokio::time::sleep(backoff).await;
                continue;
            }
            // Fail on any other non-success status. Without this an error body
            // (e.g. 403 quotaExceeded) deserializes into a struct of all-Option
            // fields as an empty result — silently dropping every channel with
            // no logged error (the "YouTube vanished from the side rail" bug).
            if !(200..300).contains(&status) {
                let body = resp.text().await.unwrap_or_default();
                bail!("YouTube API {status} for {url}: {}", body.trim());
            }
            return Ok(resp.json().await?);
        }
        bail!("YouTube API exhausted retries for {url}")
    }

    /// Check RSS feed for recent videos from a channel (free, no quota)
    async fn check_rss_for_live(&self, channel_id: &str) -> Result<Vec<String>> {
        let url = format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}");
        let resp = self.client.get(&url).send().await?;
        let body = resp.text().await?;

        // Simple XML parsing for video IDs — extract <yt:videoId>...</yt:videoId>
        let mut video_ids = Vec::new();
        for segment in body.split("<yt:videoId>") {
            if let Some(id) = segment.split("</yt:videoId>").next() {
                if id.len() == 11
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    video_ids.push(id.to_string());
                }
            }
        }

        // Only check recent videos (first 5)
        video_ids.truncate(5);
        Ok(video_ids)
    }

    /// Check if specific videos are currently live (1 API unit per call)
    /// Returns `(video_id, ChannelEntry)` for each currently-live video.
    /// How the API describes a video's broadcast state.
    fn classify(
        broadcast: Option<&str>,
        actual_start: Option<&str>,
        actual_end: Option<&str>,
    ) -> Liveness {
        // `liveBroadcastContent` is authoritative when present.
        match broadcast {
            Some("live") => return Liveness::Live,
            Some("upcoming") => return Liveness::Upcoming,
            _ => {}
        }
        // Corroborating fallback for responses that omit the snippet: a
        // broadcast that started and has not ended is still running.
        if actual_start.is_some() && actual_end.is_none() {
            return Liveness::Live;
        }
        Liveness::Settled
    }

    /// Returns the currently-live entries, plus the ids that are settled —
    /// finished broadcasts and ordinary uploads that will never become live.
    /// Scheduled ("upcoming") streams are in neither list: they are not live
    /// yet and must stay eligible for re-checking.
    #[allow(clippy::type_complexity)]
    async fn check_videos_live(
        &self,
        video_ids: &[String],
    ) -> Result<(Vec<(String, ChannelEntry)>, Vec<String>)> {
        if video_ids.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut settled: Vec<String> = Vec::new();

        let ids = video_ids.join(",");
        let url = format!("{YOUTUBE_API_URL}/videos?part=snippet,liveStreamingDetails&id={ids}");
        let resp: VideoListResponse = self.api_get(&url).await?;

        let mut live_channels = Vec::new();

        if let Some(items) = resp.items {
            for item in items {
                let video_id = item.id.clone().unwrap_or_default();
                let details = item.live_streaming_details.as_ref();
                let snippet = item.snippet.as_ref();

                // `liveBroadcastContent` is the authoritative signal. The old
                // test — "has an activeLiveChatId" — silently missed every
                // stream with live chat disabled; measured against three
                // confirmed-live streams it caught one of three.
                // "upcoming" is a scheduled stream that has not started. It
                // is not live now, but it must NOT be written off: it becomes
                // live later, and caching it as dead is what made scheduled
                // streams permanently undetectable.
                match Self::classify(
                    snippet.and_then(|s| s.live_broadcast_content.as_deref()),
                    details.and_then(|d| d.actual_start_time.as_deref()),
                    details.and_then(|d| d.actual_end_time.as_deref()),
                ) {
                    Liveness::Live => {}
                    Liveness::Upcoming => continue,
                    Liveness::Settled => {
                        settled.push(video_id);
                        continue;
                    }
                }
                let started_at = details
                    .and_then(|d| d.actual_start_time.as_deref())
                    .and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(s)
                            .ok()
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                    });

                let viewer_count = details
                    .and_then(|d| d.concurrent_viewers.as_deref())
                    .and_then(|v| v.parse().ok());

                let thumbnail = snippet
                    .and_then(|s| s.thumbnails.as_ref())
                    .and_then(|t| t.high.as_ref().or(t.medium.as_ref()))
                    .and_then(|t| t.url.clone());

                let channel_id = snippet
                    .and_then(|s| s.channel_id.clone())
                    .unwrap_or_default();
                let channel_title = snippet
                    .and_then(|s| s.channel_title.clone())
                    .unwrap_or_default();

                live_channels.push((
                    video_id.clone(),
                    ChannelEntry {
                        id: channel_id.clone(),
                        platform: PlatformKind::YouTube,
                        name: channel_id,
                        display_name: channel_title,
                        is_live: true,
                        stream_title: snippet.and_then(|s| s.title.clone()),
                        game_or_category: None,
                        viewer_count,
                        started_at,
                        thumbnail_url: thumbnail,
                        auto_record: false,
                        last_live_at: None,
                        // The id of the broadcast actually airing. YouTube's
                        // player API addresses a video, not a channel, so the
                        // web UI cannot drive a YouTube tile without it.
                        live_video_id: Some(video_id),
                    },
                ));
            }
        }

        // Ids the API did not return at all are deleted, private, or bogus.
        // They will never go live, so they count as settled — without this the
        // quota guard would re-check them forever.
        let seen: std::collections::HashSet<&str> = live_channels
            .iter()
            .map(|(id, _)| id.as_str())
            .chain(settled.iter().map(|s| s.as_str()))
            .collect();
        let missing: Vec<String> = video_ids
            .iter()
            .filter(|id| !seen.contains(id.as_str()))
            .cloned()
            .collect();
        settled.extend(missing);

        Ok((live_channels, settled))
    }

    /// Enumerate uploads via the channel's auto-generated `UU…` playlist.
    /// Requires authenticated access. Pages 50 items at a time.
    async fn fetch_uploads_playlist(
        &self,
        channel_id: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<VodEntry>> {
        // YouTube's "uploads" playlist for any UC… channel is UU + the last chars after "UC".
        let uploads_id = if let Some(rest) = channel_id.strip_prefix("UC") {
            format!("UU{rest}")
        } else {
            bail!("channel_id must start with 'UC' for YouTube uploads enumeration: {channel_id}");
        };
        self.fetch_playlist_items(&uploads_id, channel_id, since, limit)
            .await
    }

    /// Enumerate a specific playlist's items as VodEntries (task #73).
    /// `channel_id` is recorded on each VodEntry for provenance/dedupe.
    /// Reverse-chrono, so `since` short-circuits once items age out.
    pub async fn fetch_playlist_items(
        &self,
        playlist_id: &str,
        channel_id: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<VodEntry>> {
        let uploads_id = playlist_id.to_string();

        let mut out = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut url = format!(
                "{YOUTUBE_API_URL}/playlistItems?part=snippet,contentDetails&maxResults=50&playlistId={uploads_id}"
            );
            if let Some(ref t) = page_token {
                url.push_str(&format!("&pageToken={t}"));
            }

            let resp: serde_json::Value = self.api_get(&url).await?;
            let items = resp
                .get("items")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for item in items {
                let snippet = item.get("snippet");
                let content_details = item.get("contentDetails");

                let video_id = content_details
                    .and_then(|c| c.get("videoId"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_default();
                if video_id.is_empty() {
                    continue;
                }

                let title = snippet
                    .and_then(|s| s.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled")
                    .to_string();

                let published_at = content_details
                    .and_then(|c| c.get("videoPublishedAt"))
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        snippet
                            .and_then(|s| s.get("publishedAt"))
                            .and_then(|v| v.as_str())
                    })
                    .and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(s)
                            .ok()
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                    });

                if let (Some(after), Some(pub_at)) = (since, published_at) {
                    if pub_at < after {
                        // playlistItems is reverse-chrono — once we drop below the cutoff
                        // every subsequent item will too.
                        return Ok(out);
                    }
                }

                let thumbnail = snippet
                    .and_then(|s| s.get("thumbnails"))
                    .and_then(|t| t.get("high").or_else(|| t.get("medium")))
                    .and_then(|t| t.get("url"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                out.push(VodEntry {
                    id: video_id.clone(),
                    platform: PlatformKind::YouTube,
                    channel_id: channel_id.to_string(),
                    title,
                    published_at,
                    duration: None,
                    url: format!("https://www.youtube.com/watch?v={video_id}"),
                    thumbnail_url: thumbnail,
                    // Uploads-playlist enumeration (bulk DL) doesn't probe
                    // live status; fetch_recent_videos annotates kind for
                    // the webui channel detail.
                    kind: crate::platform::VodKind::Upload,
                });

                if let Some(cap) = limit {
                    if out.len() >= cap {
                        return Ok(out);
                    }
                }
            }

            page_token = resp
                .get("nextPageToken")
                .and_then(|v| v.as_str())
                .filter(|t| !t.is_empty())
                .map(String::from);
            if page_token.is_none() {
                break;
            }
        }

        Ok(out)
    }

    /// Fetch a channel's most recent videos, annotated as live broadcast vs
    /// upload (webui channel detail, task: TUI-style redesign). Two cheap
    /// calls: the uploads playlist for the recent N, then one videos.list
    /// with `liveStreamingDetails` — items carrying `actualStartTime` were
    /// live broadcasts; the rest are uploads.
    pub async fn fetch_recent_videos(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<VodEntry>> {
        let mut vods = self
            .fetch_uploads_playlist(channel_id, None, Some(limit))
            .await?;
        if vods.is_empty() {
            return Ok(vods);
        }

        // videos.list caps at 50 ids per call; recent-N is small so one call.
        let ids = vods
            .iter()
            .take(50)
            .map(|v| v.id.clone())
            .collect::<Vec<_>>()
            .join(",");
        let url = format!("{YOUTUBE_API_URL}/videos?part=liveStreamingDetails&id={ids}");
        match self.api_get::<VideoListResponse>(&url).await {
            Ok(resp) => {
                let live_ids: std::collections::HashSet<String> = resp
                    .items
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|it| {
                        it.live_streaming_details
                            .as_ref()
                            .is_some_and(|d| d.actual_start_time.is_some())
                    })
                    .filter_map(|it| it.id)
                    .collect();
                annotate_live(&mut vods, &live_ids);
            }
            Err(e) => {
                // Non-fatal: fall back to all-uploads if the annotation call
                // fails (quota, transient). The list is still useful.
                tracing::warn!("youtube: liveStreamingDetails annotate failed: {e}");
            }
        }
        Ok(vods)
    }

    /// List a channel's playlists for the bulk-download scope picker
    /// (task #73). Paginates `playlists?channelId=…`. The uploads
    /// playlist is implicit (whole-channel scope handles it) so it's
    /// not returned here.
    pub async fn fetch_playlists(
        &self,
        channel_id: &str,
    ) -> Result<Vec<crate::platform::PlaylistInfo>> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut url = format!(
                "{YOUTUBE_API_URL}/playlists?part=snippet,contentDetails&maxResults=50&channelId={channel_id}"
            );
            if let Some(ref t) = page_token {
                url.push_str(&format!("&pageToken={t}"));
            }
            let resp: serde_json::Value = self.api_get(&url).await?;
            let items = resp
                .get("items")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for item in items {
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                let title = item
                    .get("snippet")
                    .and_then(|s| s.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled playlist")
                    .to_string();
                let item_count = item
                    .get("contentDetails")
                    .and_then(|c| c.get("itemCount"))
                    .and_then(|v| v.as_u64());
                out.push(crate::platform::PlaylistInfo {
                    id,
                    title,
                    item_count,
                });
            }
            page_token = resp
                .get("nextPageToken")
                .and_then(|v| v.as_str())
                .filter(|t| !t.is_empty())
                .map(String::from);
            if page_token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn cookies_path(&self) -> Option<&std::path::Path> {
        self.cookies_path.as_deref()
    }

    #[allow(dead_code)]
    pub async fn is_authenticated(&self) -> bool {
        self.access_token.read().await.is_some()
    }
}

#[async_trait::async_trait]
impl Platform for YouTubePlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::YouTube
    }

    async fn authenticate(&self) -> Result<()> {
        if self.load_stored_tokens().await? {
            tracing::info!("YouTube: authenticated from stored tokens");
            return Ok(());
        }
        self.device_code_flow().await
    }

    async fn fetch_followed_channels(&self) -> Result<Vec<ChannelEntry>> {
        // Subscriptions change rarely — serve from cache for 6h instead of
        // re-fetching every poll (saves quota + survives transient 403s).
        const SUBS_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
        {
            let guard = self.subs_cache.read().await;
            if let Some((at, cached)) = guard.as_ref() {
                if at.elapsed() < SUBS_TTL {
                    return Ok(cached.clone());
                }
            }
        }

        let mut channels = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut url =
                format!("{YOUTUBE_API_URL}/subscriptions?part=snippet&mine=true&maxResults=50");
            if let Some(ref token) = page_token {
                url.push_str(&format!("&pageToken={token}"));
            }

            let resp: SubscriptionListResponse = self.api_get(&url).await?;

            if let Some(items) = resp.items {
                for item in items {
                    let snippet = item.snippet;
                    let Some(snippet) = snippet else { continue };
                    let channel_id = snippet
                        .resource_id
                        .and_then(|r| r.channel_id)
                        .unwrap_or_default();
                    let title = snippet.title.unwrap_or_default();

                    if channel_id.is_empty() {
                        continue;
                    }

                    channels.push(ChannelEntry {
                        id: channel_id.clone(),
                        platform: PlatformKind::YouTube,
                        name: channel_id,
                        display_name: title,
                        is_live: false,
                        stream_title: None,
                        game_or_category: None,
                        viewer_count: None,
                        started_at: None,
                        thumbnail_url: None,
                        auto_record: false,
                        last_live_at: None,
                        live_video_id: None,
                    });
                }
            }

            match resp.next_page_token {
                Some(t) if !t.is_empty() => page_token = Some(t),
                _ => break,
            }
        }

        *self.subs_cache.write().await = Some((std::time::Instant::now(), channels.clone()));
        Ok(channels)
    }

    async fn check_live_status(&self, channel_ids: &[String]) -> Result<Vec<ChannelEntry>> {
        // RSS-first (free): collect recent video ids across all channels, then
        // confirm live state in ONE batched videos.list per 50 ids — and skip
        // ids already confirmed dead (ended/VOD) so quota is spent only on new
        // or still-live videos. This is the fix for the 10k/day exhaustion:
        // previously this made one videos.list call PER channel, every poll.
        // RSS feeds are fetched concurrently (bounded) instead of one at a
        // time: each is an independent network round-trip to
        // youtube.com/feeds, so a sequential loop paid the full per-channel
        // latency N times over — a poll cycle over 50 followed channels at
        // ~200ms/request serialized to ~10s before this. Capped at 10
        // in-flight requests so a large subscription list doesn't open
        // dozens of simultaneous sockets to youtube.com at once.
        const RSS_CONCURRENCY: usize = 10;
        let rss_results: Vec<(String, Result<Vec<String>>)> =
            futures_util::stream::iter(channel_ids.iter().cloned())
                .map(|channel_id| async move {
                    let result = self.check_rss_for_live(&channel_id).await;
                    (channel_id, result)
                })
                .buffer_unordered(RSS_CONCURRENCY)
                .collect()
                .await;

        let mut candidates: Vec<String> = Vec::new();
        {
            let dead = self.dead_videos.read().await;
            for (channel_id, result) in rss_results {
                match result {
                    Ok(video_ids) => {
                        for v in video_ids {
                            if !dead.contains(&v) {
                                candidates.push(v);
                            }
                        }
                    }
                    Err(e) => tracing::warn!("Failed to check RSS for {channel_id}: {e}"),
                }
            }
        }
        candidates.sort();
        candidates.dedup();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_live = Vec::new();
        let mut settled_ids: Vec<String> = Vec::new();
        for chunk in candidates.chunks(50) {
            match self.check_videos_live(chunk).await {
                Ok((pairs, settled)) => {
                    for (_vid, entry) in pairs {
                        all_live.push(entry);
                    }
                    settled_ids.extend(settled);
                }
                Err(e) => tracing::warn!("youtube: videos.list live check failed: {e}"),
            }
        }

        // Cache only ids the API settled: finished broadcasts, plain uploads,
        // and videos it no longer serves. Scheduled ("upcoming") streams are
        // deliberately left out — caching those was what made a stream that
        // was announced before it started permanently undetectable.
        {
            let mut dead = self.dead_videos.write().await;
            for id in settled_ids {
                dead.insert(id);
            }
            if dead.len() > 20_000 {
                dead.clear(); // bound memory; warms back up via RSS
            }
        }

        Ok(all_live)
    }

    async fn refresh_token(&self) -> Result<()> {
        self.do_refresh_token().await
    }

    async fn is_authenticated(&self) -> bool {
        Self::is_authenticated(self).await
    }

    async fn fetch_channel_vods(
        &self,
        channel_id: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<VodEntry>> {
        self.fetch_uploads_playlist(channel_id, since, limit).await
    }
}

/// Flip `kind` to LiveBroadcast for any VOD whose id is in `live_ids`
/// (those carried `liveStreamingDetails.actualStartTime`). Pure so the
/// live/upload partition is unit-testable without the network.
fn annotate_live(vods: &mut [VodEntry], live_ids: &std::collections::HashSet<String>) {
    for v in vods.iter_mut() {
        if live_ids.contains(&v.id) {
            v.kind = crate::platform::VodKind::LiveBroadcast;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{PlatformKind, VodEntry, VodKind};

    fn vod(id: &str) -> VodEntry {
        VodEntry {
            id: id.into(),
            platform: PlatformKind::YouTube,
            channel_id: "UC".into(),
            title: id.into(),
            published_at: None,
            duration: None,
            url: format!("https://youtu.be/{id}"),
            thumbnail_url: None,
            kind: VodKind::Upload,
        }
    }

    #[test]
    fn partitions_live_from_uploads() {
        let mut vods = vec![vod("stream1"), vod("upload1"), vod("stream2")];
        let live: std::collections::HashSet<String> =
            ["stream1".to_string(), "stream2".to_string()]
                .into_iter()
                .collect();
        annotate_live(&mut vods, &live);

        let kinds: Vec<_> = vods.iter().map(|v| (v.id.as_str(), v.kind)).collect();
        assert_eq!(kinds[0], ("stream1", VodKind::LiveBroadcast));
        assert_eq!(kinds[1], ("upload1", VodKind::Upload));
        assert_eq!(kinds[2], ("stream2", VodKind::LiveBroadcast));
        // No id appears in both partitions.
        let streams = vods
            .iter()
            .filter(|v| v.kind == VodKind::LiveBroadcast)
            .count();
        let uploads = vods.iter().filter(|v| v.kind == VodKind::Upload).count();
        assert_eq!(streams + uploads, vods.len());
    }
}

#[cfg(test)]
mod live_detection_tests {
    use super::*;

    /// Regression: the previous test was `activeLiveChatId.is_some()`, which
    /// misses any live stream with chat disabled. These three cases are real
    /// API responses captured from confirmed-live broadcasts — Lofi Girl had
    /// a chat id, Sky News and NASA ISS did not, and all three were live.
    #[test]
    fn live_streams_are_detected_with_or_without_live_chat() {
        // Lofi Girl: chat enabled.
        assert_eq!(
            YouTubePlatform::classify(Some("live"), Some("2026-08-19T00:00:00Z"), None),
            Liveness::Live
        );
        // Sky News / NASA ISS: chat disabled, still unambiguously live.
        assert_eq!(
            YouTubePlatform::classify(Some("live"), None, None),
            Liveness::Live
        );
    }

    /// A scheduled stream must never be written off, or it stays invisible
    /// once it actually starts.
    #[test]
    fn upcoming_streams_are_not_settled() {
        assert_eq!(
            YouTubePlatform::classify(Some("upcoming"), None, None),
            Liveness::Upcoming
        );
    }

    #[test]
    fn finished_broadcasts_and_uploads_are_settled() {
        // Ended broadcast: has both a start and an end time.
        assert_eq!(
            YouTubePlatform::classify(
                Some("none"),
                Some("2022-07-12T15:59:30Z"),
                Some("2026-05-20T02:11:23Z")
            ),
            Liveness::Settled
        );
        // Ordinary upload: no streaming details at all.
        assert_eq!(
            YouTubePlatform::classify(Some("none"), None, None),
            Liveness::Settled
        );
    }

    /// If the snippet is missing, fall back to the streaming details rather
    /// than defaulting a running broadcast to "settled".
    #[test]
    fn missing_snippet_falls_back_to_start_and_end_times() {
        assert_eq!(
            YouTubePlatform::classify(None, Some("2026-08-19T00:00:00Z"), None),
            Liveness::Live
        );
        assert_eq!(
            YouTubePlatform::classify(
                None,
                Some("2026-08-19T00:00:00Z"),
                Some("2026-08-19T02:00:00Z")
            ),
            Liveness::Settled
        );
        assert_eq!(
            YouTubePlatform::classify(None, None, None),
            Liveness::Settled
        );
    }
}
