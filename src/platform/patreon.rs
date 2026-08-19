use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::credentials;
use crate::events::DaemonEvent;
use crate::platform::{PlatformKind, VodEntry};

const PATREON_API_URL: &str = "https://www.patreon.com/api/oauth2/v2";
const PATREON_AUTH_URL: &str = "https://www.patreon.com/oauth2/authorize";
const PATREON_TOKEN_URL: &str = "https://www.patreon.com/api/oauth2/token";

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PatreonCreator {
    pub campaign_id: String,
    pub name: String,
    pub vanity: Option<String>,
    pub url: String,
    /// Subscription tier name (e.g., "Tier 1", "Premium", etc.), if pledged.
    pub tier: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatreonPost {
    pub id: String,
    pub campaign_id: String,
    pub title: String,
    pub url: String,
    pub published_at: String,
    pub embed_url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

impl PatreonCreator {
    /// Represent a pledged creator as a sidebar ChannelEntry so it can
    /// live in the same list the Twitch/YT channels do. Patreon has no
    /// live concept, so is_live is always false and the row sorts into
    /// the dedicated Patreon section (sidebar.rs groups by platform).
    pub fn to_channel_entry(&self) -> crate::platform::ChannelEntry {
        crate::platform::ChannelEntry {
            id: self.campaign_id.clone(),
            platform: PlatformKind::Patreon,
            name: self
                .vanity
                .clone()
                .unwrap_or_else(|| self.campaign_id.clone()),
            display_name: self.name.clone(),
            is_live: false,
            stream_title: self.tier.clone(),
            game_or_category: None,
            viewer_count: None,
            started_at: None,
            thumbnail_url: None,
            auto_record: false,
            last_live_at: None,
            live_video_id: None,
        }
    }
}

pub struct PatreonClient {
    client: Client,
    client_id: String,
    client_secret: String,
    access_token: Arc<RwLock<Option<String>>>,
    refresh_token_value: Arc<RwLock<Option<String>>>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<DaemonEvent>>,
}

impl PatreonClient {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client: Client::new(),
            client_id,
            client_secret,
            access_token: Arc::new(RwLock::new(None)),
            refresh_token_value: Arc::new(RwLock::new(None)),
            event_tx: None,
        }
    }

    pub fn set_event_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<DaemonEvent>) {
        self.event_tx = Some(tx);
    }

    /// Try to load and validate stored tokens, returning true if successful.
    pub async fn load_stored_tokens(&self) -> Result<bool> {
        if let Some(token) = credentials::get_secret("patreon_access_token")? {
            *self.access_token.write().await = Some(token);
            if let Some(refresh) = credentials::get_secret("patreon_refresh_token")? {
                *self.refresh_token_value.write().await = Some(refresh);
            }
            // Validate
            if self.validate_token().await? {
                return Ok(true);
            }
            // Try refresh
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
            .get(format!("{PATREON_API_URL}/identity"))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        Ok(resp.status().is_success())
    }

    /// OAuth2 authorization code flow via local HTTP listener.
    pub async fn authorize(&self) -> Result<()> {
        if self.load_stored_tokens().await? {
            tracing::info!("Patreon: authenticated from stored tokens");
            return Ok(());
        }

        // Fixed port so the redirect URI can be pre-registered in the Patreon client.
        const PATREON_CALLBACK_PORT: u16 = 47823;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", PATREON_CALLBACK_PORT))
            .await
            .with_context(|| format!(
                "Failed to bind Patreon OAuth callback on 127.0.0.1:{PATREON_CALLBACK_PORT} (port in use?)"
            ))?;
        let redirect_uri = format!("http://127.0.0.1:{PATREON_CALLBACK_PORT}/callback");

        let auth_url = format!(
            "{PATREON_AUTH_URL}?response_type=code&client_id={}&redirect_uri={}&scope=identity%20identity.memberships",
            self.client_id,
            urlencoding(&redirect_uri),
        );

        tracing::info!("Patreon auth: open {auth_url}");

        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(DaemonEvent::DeviceCodeRequired {
                kind: PlatformKind::Patreon,
                verification_uri: auth_url.clone(),
                user_code: "Open URL in browser".to_string(),
            });
        }

        // Wait for callback (with timeout)
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            wait_for_auth_code(listener),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Patreon auth timed out"))??;

        // Exchange code for tokens
        let resp = self
            .client
            .post(PATREON_TOKEN_URL)
            .form(&[
                ("code", code.as_str()),
                ("grant_type", "authorization_code"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await?;
            bail!("Patreon token exchange failed: {body}");
        }

        let token: TokenResponse = resp.json().await?;
        credentials::store_secret("patreon_access_token", &token.access_token)?;
        if let Some(ref refresh) = token.refresh_token {
            credentials::store_secret("patreon_refresh_token", refresh)?;
            *self.refresh_token_value.write().await = Some(refresh.clone());
        }
        *self.access_token.write().await = Some(token.access_token);

        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(DaemonEvent::PlatformAuthenticated {
                kind: PlatformKind::Patreon,
            });
        }

        Ok(())
    }

    async fn do_refresh_token(&self) -> Result<()> {
        let refresh = self.refresh_token_value.read().await.clone();
        let Some(refresh) = refresh else {
            bail!("No Patreon refresh token");
        };

        let resp = self
            .client
            .post(PATREON_TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            bail!("Patreon token refresh failed: {}", resp.status());
        }

        let token: TokenResponse = resp.json().await?;
        credentials::store_secret("patreon_access_token", &token.access_token)?;
        if let Some(ref new_refresh) = token.refresh_token {
            credentials::store_secret("patreon_refresh_token", new_refresh)?;
            *self.refresh_token_value.write().await = Some(new_refresh.clone());
        }
        *self.access_token.write().await = Some(token.access_token);
        Ok(())
    }

    async fn api_get(&self, url: &str) -> Result<serde_json::Value> {
        for attempt in 0..3 {
            let token = self.access_token.read().await.clone();
            let Some(token) = token else {
                bail!("Patreon not authenticated");
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
                tracing::warn!(url = %url, secs = backoff.as_secs(), "Patreon rate-limited; backing off");
                drop(resp);
                tokio::time::sleep(backoff).await;
                continue;
            }
            return Ok(resp.json().await?);
        }
        bail!("Patreon API exhausted retries for {url}")
    }

    /// Fetch campaigns the user supports (pledged creators), including tier info.
    pub async fn fetch_pledged_creators(&self) -> Result<Vec<PatreonCreator>> {
        let url = format!(
            "{PATREON_API_URL}/identity?include=memberships.campaign,memberships.campaign.creator,memberships.currently_entitled_tiers\
             &fields%5Bcampaign%5D=vanity,url,creation_name\
             &fields%5Buser%5D=full_name\
             &fields%5Btier%5D=title\
             &fields%5Bmember%5D=patron_status"
        );
        let data = self.api_get(&url).await?;

        let included = data.get("included").and_then(|v| v.as_array());

        // Build tier title lookup: tier_id -> title
        let mut tier_titles: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Some(items) = included {
            for item in items {
                if item.get("type").and_then(|t| t.as_str()) == Some("tier") {
                    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let title = item
                        .get("attributes")
                        .and_then(|a| a.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown Tier");
                    if !id.is_empty() {
                        tier_titles.insert(id.to_string(), title.to_string());
                    }
                }
            }
        }

        // Build membership -> (campaign_id, tier_name) mapping
        // Each "member" item has relationships.campaign.data.id and relationships.currently_entitled_tiers.data[].id
        let mut campaign_tier: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Some(items) = included {
            for item in items {
                if item.get("type").and_then(|t| t.as_str()) == Some("member") {
                    let campaign_id = item
                        .get("relationships")
                        .and_then(|r| r.get("campaign"))
                        .and_then(|c| c.get("data"))
                        .and_then(|d| d.get("id"))
                        .and_then(|v| v.as_str());

                    let tier_id = item
                        .get("relationships")
                        .and_then(|r| r.get("currently_entitled_tiers"))
                        .and_then(|t| t.get("data"))
                        .and_then(|d| d.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|t| t.get("id"))
                        .and_then(|v| v.as_str());

                    if let (Some(cid), Some(tid)) = (campaign_id, tier_id) {
                        if let Some(title) = tier_titles.get(tid) {
                            campaign_tier.insert(cid.to_string(), title.clone());
                        }
                    }
                }
            }
        }

        // Build user_id -> full_name lookup (the campaign creator's display
        // name, e.g. "Fear &" / "the yard" — the proper title, vs the
        // creation_name "is creating …" blurb).
        let mut user_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Some(items) = included {
            for item in items {
                if item.get("type").and_then(|t| t.as_str()) == Some("user") {
                    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let full = item
                        .get("attributes")
                        .and_then(|a| a.get("full_name"))
                        .and_then(|v| v.as_str());
                    if let (false, Some(full)) = (id.is_empty(), full) {
                        user_names.insert(id.to_string(), full.to_string());
                    }
                }
            }
        }

        // Build creator list from campaign items
        let mut creators = Vec::new();
        if let Some(items) = included {
            for item in items {
                if item.get("type").and_then(|t| t.as_str()) == Some("campaign") {
                    let id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let attrs = item.get("attributes");
                    // Prefer the creator user's full_name; fall back to the
                    // campaign's creation_name, then vanity.
                    let creator_user_id = item
                        .get("relationships")
                        .and_then(|r| r.get("creator"))
                        .and_then(|c| c.get("data"))
                        .and_then(|d| d.get("id"))
                        .and_then(|v| v.as_str());
                    let creation_name = attrs
                        .and_then(|a| a.get("creation_name"))
                        .and_then(|v| v.as_str());
                    let name = creator_user_id
                        .and_then(|uid| user_names.get(uid).cloned())
                        .or_else(|| creation_name.map(|s| s.to_string()))
                        .unwrap_or_else(|| "Unknown".to_string());
                    let vanity = attrs
                        .and_then(|a| a.get("vanity"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let url = attrs
                        .and_then(|a| a.get("url"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !id.is_empty() {
                        let tier = campaign_tier.get(&id).cloned();
                        creators.push(PatreonCreator {
                            campaign_id: id,
                            name,
                            vanity,
                            url,
                            tier,
                        });
                    }
                }
            }
        }

        Ok(creators)
    }

    /// Fetch recent video posts from a campaign.
    pub async fn fetch_posts(
        &self,
        campaign_id: &str,
        since: Option<&chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<PatreonPost>> {
        let mut url = format!(
            "{PATREON_API_URL}/campaigns/{campaign_id}/posts?fields%5Bpost%5D=title,url,published_at,embed_url&filter%5Bis_by_creator%5D=true"
        );
        if let Some(since) = since {
            url.push_str(&format!(
                "&filter%5Bpublished_at%5D%5Bgte%5D={}",
                since.to_rfc3339()
            ));
        }

        let data = self.api_get(&url).await?;
        let mut posts = Vec::new();

        if let Some(items) = data.get("data").and_then(|v| v.as_array()) {
            for item in items {
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let attrs = item.get("attributes");
                let title = attrs
                    .and_then(|a| a.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled")
                    .to_string();
                let post_url = attrs
                    .and_then(|a| a.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let published_at = attrs
                    .and_then(|a| a.get("published_at"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let embed_url = attrs
                    .and_then(|a| a.get("embed_url"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                // Only include posts that have video embeds
                if embed_url.is_some() && !id.is_empty() {
                    posts.push(PatreonPost {
                        id,
                        campaign_id: campaign_id.to_string(),
                        title,
                        url: post_url,
                        published_at,
                        embed_url,
                        thumbnail_url: None,
                    });
                }
            }
        }

        Ok(posts)
    }

    /// List a campaign's recent video posts via yt-dlp + the patron's Patreon
    /// cookies. The Patreon API does not expose a creator's posts to patrons
    /// (it only serves campaigns you own), so this scrapes the campaign page
    /// the way the website does. `cookies_path` is required to see paid posts.
    pub async fn fetch_posts_ytdlp(
        &self,
        campaign_id: &str,
        vanity: &str,
        cookies_path: Option<&std::path::Path>,
        limit: usize,
    ) -> Result<Vec<PatreonPost>> {
        // The campaign page lists each post under its vanity-prefixed URL
        // (e.g. patreon.com/theyard/posts/<slug>-<id>), but yt-dlp's PatreonIE
        // only accepts the canonical patreon.com/posts/<id> form. A single
        // full extraction therefore fails per-post ("Unsupported URL") and
        // yields only null entries — no posts at all. So enumerate the post
        // URLs flat (cheap, reliable), rewrite them to the canonical form, then
        // extract metadata from those.
        let page = format!("https://www.patreon.com/c/{vanity}/posts");

        // Pass 1: flat enumeration of the most recent post URLs.
        let mut list = tokio::process::Command::new("yt-dlp");
        list.arg("-J")
            .arg("--flat-playlist")
            .arg("--playlist-end")
            .arg(limit.to_string())
            .arg("--ignore-errors")
            .arg("--no-warnings");
        if let Some(cp) = cookies_path {
            list.arg("--cookies").arg(cp);
        }
        list.arg(&page);

        let listing = tokio::time::timeout(std::time::Duration::from_secs(60), list.output())
            .await
            .context("yt-dlp post listing timed out")?
            .context("failed to spawn yt-dlp")?;
        let listing_out = String::from_utf8_lossy(&listing.stdout);
        let listing_json: serde_json::Value = serde_json::from_str(listing_out.trim())
            .with_context(|| format!("parse yt-dlp listing for {vanity}"))?;

        let mut canonical_urls = Vec::new();
        if let Some(entries) = listing_json.get("entries").and_then(|v| v.as_array()) {
            for e in entries {
                let raw = e
                    .get("url")
                    .and_then(|v| v.as_str())
                    .or_else(|| e.get("webpage_url").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if let Some(c) = canonical_post_url(raw) {
                    canonical_urls.push(c);
                }
            }
        }

        if canonical_urls.is_empty() {
            return Ok(Vec::new());
        }

        // Pass 2: full metadata extraction against the canonical URLs. yt-dlp
        // does a per-post extraction, so bound the call: a hung request must
        // not stall the monitor poll.
        let mut meta = tokio::process::Command::new("yt-dlp");
        meta.arg("-J").arg("--ignore-errors").arg("--no-warnings");
        if let Some(cp) = cookies_path {
            meta.arg("--cookies").arg(cp);
        }
        for u in &canonical_urls {
            meta.arg(u);
        }

        let output = tokio::time::timeout(std::time::Duration::from_secs(120), meta.output())
            .await
            .context("yt-dlp timed out")?
            .context("failed to spawn yt-dlp")?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        // With multiple URLs, `-J` prints one compact JSON object per post,
        // one per line. Skip lines that fail to parse or carry no id.
        let mut posts = Vec::new();
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(e) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let id = e
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let title = e
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();
            let thumbnail_url = e
                .get("thumbnail")
                .and_then(|v| v.as_str())
                .map(String::from);
            // The canonical post page URL is the stable handle; the pull path
            // re-runs yt-dlp on it (with cookies) to download.
            let url = e
                .get("webpage_url")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| format!("https://www.patreon.com/posts/{id}"));
            // upload_date is YYYYMMDD; normalise to YYYY-MM-DD for display.
            let published_at = e
                .get("upload_date")
                .and_then(|v| v.as_str())
                .map(|d| {
                    if d.len() == 8 {
                        format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8])
                    } else {
                        d.to_string()
                    }
                })
                .unwrap_or_default();
            posts.push(PatreonPost {
                id,
                campaign_id: campaign_id.to_string(),
                title,
                url: url.clone(),
                published_at,
                embed_url: Some(url),
                thumbnail_url,
            });
        }
        Ok(posts)
    }

    pub async fn is_authenticated(&self) -> bool {
        self.access_token.read().await.is_some()
    }

    /// Enumerate a campaign's full back catalog of video/audio posts. Pages until exhausted
    /// or `limit` is hit. `since` filters server-side via Patreon's `published_at[gte]` filter.
    pub async fn fetch_channel_vods(
        &self,
        campaign_id: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<VodEntry>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!(
                "{PATREON_API_URL}/campaigns/{campaign_id}/posts\
                 ?fields%5Bpost%5D=title,url,published_at,embed_url\
                 &filter%5Bis_by_creator%5D=true&page%5Bcount%5D=20&sort=-published_at"
            );
            if let Some(ref s) = since {
                url.push_str(&format!(
                    "&filter%5Bpublished_at%5D%5Bgte%5D={}",
                    s.to_rfc3339()
                ));
            }
            if let Some(ref c) = cursor {
                url.push_str(&format!("&page%5Bcursor%5D={c}"));
            }

            let data = self.api_get(&url).await?;

            if let Some(items) = data.get("data").and_then(|v| v.as_array()) {
                for item in items {
                    let id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let attrs = item.get("attributes");
                    let title = attrs
                        .and_then(|a| a.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Untitled")
                        .to_string();
                    let post_url = attrs
                        .and_then(|a| a.get("url"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let published_at = attrs
                        .and_then(|a| a.get("published_at"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| {
                            chrono::DateTime::parse_from_rfc3339(s)
                                .ok()
                                .map(|dt| dt.with_timezone(&chrono::Utc))
                        });
                    let embed_url = attrs
                        .and_then(|a| a.get("embed_url"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from);

                    // A post with an embed_url is a video/audio embed. Prefer
                    // the embed; fall back to the post page (yt-dlp's Patreon
                    // extractor handles it with the user's cookies).
                    if embed_url.is_none() {
                        continue;
                    }
                    let download_url = embed_url.unwrap_or_else(|| post_url.clone());

                    out.push(VodEntry {
                        id,
                        platform: PlatformKind::Patreon,
                        channel_id: campaign_id.to_string(),
                        title,
                        published_at,
                        duration: None,
                        url: download_url,
                        thumbnail_url: None,
                        kind: crate::platform::VodKind::Upload,
                    });

                    if let Some(cap) = limit {
                        if out.len() >= cap {
                            return Ok(out);
                        }
                    }
                }
            }

            cursor = data
                .get("meta")
                .and_then(|m| m.get("pagination"))
                .and_then(|p| p.get("cursors"))
                .and_then(|c| c.get("next"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
            if cursor.is_none() {
                break;
            }
        }

        Ok(out)
    }
}

/// Rewrite a Patreon post URL to the canonical `patreon.com/posts/<id>` form
/// that yt-dlp's PatreonIE accepts. The campaign listing emits vanity-prefixed
/// URLs (`patreon.com/<vanity>/posts/<slug>-<id>`) that the post extractor
/// rejects, so keep only the trailing numeric post id. Returns None if no
/// numeric id can be found.
fn canonical_post_url(url: &str) -> Option<String> {
    let clean = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let id = clean.rsplit(['-', '/']).next()?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("https://www.patreon.com/posts/{id}"))
}

/// Simple URL encoding for OAuth redirect URI
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

/// Wait for the OAuth callback on the local listener, extract the auth code.
async fn wait_for_auth_code(listener: tokio::net::TcpListener) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut stream, _) = listener.accept().await?;
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the code from "GET /callback?code=xxx HTTP/1.1"
    let code = request
        .lines()
        .next()
        .and_then(|line| {
            let path = line.split_whitespace().nth(1)?;
            let query = path.split('?').nth(1)?;
            for param in query.split('&') {
                if let Some(("code", value)) = param.split_once('=') {
                    return Some(value.to_string());
                }
            }
            None
        })
        .ok_or_else(|| anyhow::anyhow!("No auth code in callback"))?;

    // Send response
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h2>StriVo: Patreon authorized!</h2><p>You can close this tab.</p></body></html>";
    stream.write_all(response.as_bytes()).await?;

    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::canonical_post_url;

    #[test]
    fn canonicalizes_vanity_prefixed_post_urls() {
        // The campaign listing emits vanity-prefixed URLs that PatreonIE rejects.
        assert_eq!(
            canonical_post_url("https://www.patreon.com/theyard/posts/ep-256-supertf-162040009")
                .as_deref(),
            Some("https://www.patreon.com/posts/162040009")
        );
        // Already-canonical slug form.
        assert_eq!(
            canonical_post_url("https://www.patreon.com/posts/why-are-you-to-d-161901821")
                .as_deref(),
            Some("https://www.patreon.com/posts/161901821")
        );
        // Bare numeric id, with a trailing slash and a query string.
        assert_eq!(
            canonical_post_url("https://www.patreon.com/posts/114721679/?foo=bar").as_deref(),
            Some("https://www.patreon.com/posts/114721679")
        );
    }

    #[test]
    fn rejects_non_post_urls() {
        assert_eq!(canonical_post_url("https://www.patreon.com/theyard"), None);
        assert_eq!(canonical_post_url(""), None);
    }
}
