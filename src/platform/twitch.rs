use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::credentials;
use crate::events::DaemonEvent;
use crate::platform::{ChannelEntry, Platform, PlatformKind, VodEntry};

const TWITCH_AUTH_URL: &str = "https://id.twitch.tv/oauth2";
const TWITCH_API_URL: &str = "https://api.twitch.tv/helix";

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    status: Option<u16>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenValidationResponse {
    client_id: String,
    expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwitchTokenHealth {
    Valid { expires_in_secs: u64 },
    Refreshed,
    LoginRequired { reason: String },
}

fn token_needs_refresh(expires_in_secs: u64, min_validity: std::time::Duration) -> bool {
    expires_in_secs <= min_validity.as_secs()
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TwitchUser {
    id: String,
    login: String,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct UsersResponse {
    data: Vec<TwitchUser>,
}

#[derive(Debug, Deserialize)]
struct FollowedChannel {
    broadcaster_id: String,
    broadcaster_login: String,
    broadcaster_name: String,
}

#[derive(Debug, Deserialize)]
struct FollowedResponse {
    data: Vec<FollowedChannel>,
    pagination: Pagination,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    cursor: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct StreamData {
    id: String,
    user_id: String,
    user_login: String,
    user_name: String,
    game_name: Option<String>,
    title: Option<String>,
    viewer_count: Option<u64>,
    started_at: Option<String>,
    thumbnail_url: Option<String>,
    #[serde(rename = "type")]
    stream_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamsResponse {
    data: Vec<StreamData>,
}

pub struct TwitchPlatform {
    client: Client,
    client_id: String,
    client_secret: String,
    access_token: Arc<RwLock<Option<String>>>,
    refresh_token_value: Arc<RwLock<Option<String>>>,
    user_id: Arc<RwLock<Option<String>>>,
    /// Set during device code flow for the TUI to display
    pub pending_device_code: Arc<RwLock<Option<DeviceCodeInfo>>>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<DaemonEvent>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DeviceCodeInfo {
    pub user_code: String,
    pub verification_uri: String,
}

impl TwitchPlatform {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client: Client::new(),
            client_id,
            client_secret,
            access_token: Arc::new(RwLock::new(None)),
            refresh_token_value: Arc::new(RwLock::new(None)),
            user_id: Arc::new(RwLock::new(None)),
            pending_device_code: Arc::new(RwLock::new(None)),
            event_tx: None,
        }
    }

    pub fn set_event_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<DaemonEvent>) {
        self.event_tx = Some(tx);
    }

    /// Shared handle to the current access token, for the EventSub client to
    /// authorize subscription creation with the live (refreshed) token.
    pub fn access_token_arc(&self) -> Arc<RwLock<Option<String>>> {
        self.access_token.clone()
    }

    pub async fn load_stored_tokens(&self) -> Result<bool> {
        Ok(matches!(
            self.repair_stored_token(std::time::Duration::ZERO).await?,
            TwitchTokenHealth::Valid { .. } | TwitchTokenHealth::Refreshed
        ))
    }

    /// Load, validate, and where possible repair the keyring token. The
    /// result distinguishes automatic recovery from cases where Twitch needs
    /// a fresh device login or corrected application credentials.
    pub async fn repair_stored_token(
        &self,
        min_validity: std::time::Duration,
    ) -> Result<TwitchTokenHealth> {
        let Some(token) = credentials::get_secret("twitch_access_token")? else {
            return Ok(TwitchTokenHealth::LoginRequired {
                reason: "no access token is stored".into(),
            });
        };
        *self.access_token.write().await = Some(token);
        *self.refresh_token_value.write().await = credentials::get_secret("twitch_refresh_token")?;
        self.ensure_fresh_token(min_validity).await
    }

    async fn validate_token(&self) -> Result<Option<TokenValidationResponse>> {
        let token = self.access_token.read().await;
        let Some(token) = token.as_ref() else {
            return Ok(None);
        };
        let resp = self
            .client
            .get(format!("{TWITCH_AUTH_URL}/validate"))
            .header("Authorization", format!("OAuth {token}"))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let validation: TokenValidationResponse = resp
            .json()
            .await
            .context("decode Twitch token validation response")?;
        if validation.client_id != self.client_id {
            tracing::warn!(
                "stored Twitch token belongs to a different client_id; refresh or login required"
            );
            return Ok(None);
        }
        Ok(Some(validation))
    }

    /// Validate the current user token and refresh it when expired or close to
    /// expiry. Twitch asks applications to validate tokens hourly; callers can
    /// pass a safety window so long-running recordings never begin with a
    /// token that is about to expire.
    pub async fn ensure_fresh_token(
        &self,
        min_validity: std::time::Duration,
    ) -> Result<TwitchTokenHealth> {
        if let Some(validation) = self.validate_token().await? {
            if !token_needs_refresh(validation.expires_in, min_validity) {
                self.fetch_user_id().await?;
                return Ok(TwitchTokenHealth::Valid {
                    expires_in_secs: validation.expires_in,
                });
            }
        }

        if self.refresh_token_value.read().await.is_none() {
            return Ok(TwitchTokenHealth::LoginRequired {
                reason: "access token is stale and no refresh token is stored".into(),
            });
        }
        match self.do_refresh_token().await {
            Ok(()) => {
                self.fetch_user_id().await?;
                Ok(TwitchTokenHealth::Refreshed)
            }
            Err(error) => Ok(TwitchTokenHealth::LoginRequired {
                reason: format!("automatic refresh was rejected: {error:#}"),
            }),
        }
    }

    /// Return a token only after validation/refresh. This is the safe bridge
    /// for Streamlink and GQL paths that cannot use the Helix client directly.
    pub async fn fresh_access_token(&self) -> Result<Option<String>> {
        match self
            .ensure_fresh_token(std::time::Duration::from_secs(15 * 60))
            .await?
        {
            TwitchTokenHealth::Valid { .. } | TwitchTokenHealth::Refreshed => {
                Ok(self.access_token.read().await.clone())
            }
            TwitchTokenHealth::LoginRequired { reason } => {
                tracing::warn!(%reason, "Twitch token needs interactive login");
                Ok(None)
            }
        }
    }

    /// Resolve a channel login (e.g. "xqc") to its numeric user_id via
    /// helix `/users?login=…`. Requires either a stored user OAuth token
    /// or a valid app token in `access_token`.
    /// Resolve a Twitch login to its `(numeric id, display_name)`. The login
    /// is passed as a properly-encoded query parameter (not string-interpolated)
    /// so odd input can't corrupt the URL.
    pub async fn lookup_channel_id_by_login(&self, login: &str) -> Result<(String, String)> {
        let token = self
            .access_token
            .read()
            .await
            .clone()
            .context("no twitch access_token loaded; authenticate first")?;
        let resp: UsersResponse = self
            .client
            .get(format!("{TWITCH_API_URL}/users"))
            .query(&[("login", login)])
            .header("Authorization", format!("Bearer {token}"))
            .header("Client-Id", &self.client_id)
            .send()
            .await?
            .json()
            .await?;
        resp.data
            .into_iter()
            .find(|u| u.login.eq_ignore_ascii_case(login))
            .map(|u| (u.id, u.display_name))
            .with_context(|| format!("no such twitch channel: {login}"))
    }

    async fn fetch_user_id(&self) -> Result<()> {
        let token = self.access_token.read().await.clone();
        let Some(token) = token else {
            bail!("No access token");
        };
        let resp: UsersResponse = self
            .client
            .get(format!("{TWITCH_API_URL}/users"))
            .header("Authorization", format!("Bearer {token}"))
            .header("Client-Id", &self.client_id)
            .send()
            .await?
            .json()
            .await?;
        if let Some(user) = resp.data.into_iter().next() {
            *self.user_id.write().await = Some(user.id);
        }
        Ok(())
    }

    async fn device_code_flow(&self) -> Result<()> {
        // Step 1: Request device code
        let resp: DeviceCodeResponse = self
            .client
            .post(format!("{TWITCH_AUTH_URL}/device"))
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("scopes", "user:read:follows"),
            ])
            .send()
            .await?
            .json()
            .await
            .context("Failed to get device code from Twitch")?;

        // Store device code info for TUI display
        *self.pending_device_code.write().await = Some(DeviceCodeInfo {
            user_code: resp.user_code.clone(),
            verification_uri: resp.verification_uri.clone(),
        });

        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(DaemonEvent::DeviceCodeRequired {
                kind: PlatformKind::Twitch,
                verification_uri: resp.verification_uri.clone(),
                user_code: resp.user_code.clone(),
            });
        }

        tracing::info!(
            "Twitch auth: go to {} and enter code: {}",
            resp.verification_uri,
            resp.user_code
        );

        // Step 2: Poll for token
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
                .post(format!("{TWITCH_AUTH_URL}/token"))
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
                credentials::store_secret("twitch_access_token", &token.access_token)?;
                if let Some(ref refresh) = token.refresh_token {
                    credentials::store_secret("twitch_refresh_token", refresh)?;
                    *self.refresh_token_value.write().await = Some(refresh.clone());
                }
                *self.access_token.write().await = Some(token.access_token);
                *self.pending_device_code.write().await = None;
                self.fetch_user_id().await?;
                return Ok(());
            }

            // Check if still pending (authorization_pending) or an actual error.
            // Twitch returns 400 while the user hasn't authorized yet; any other
            // status (401 revoked, 403, 5xx) is fatal — bail instead of spinning
            // silently until the device code expires.
            match serde_json::from_str::<TokenErrorResponse>(&body) {
                Ok(err) if err.status == Some(400) => continue,
                Ok(err) => bail!(
                    "Twitch device-code auth failed: {} {}",
                    err.status.map(|s| s.to_string()).unwrap_or_default(),
                    err.message.unwrap_or_default()
                ),
                Err(_) => bail!("Twitch device-code auth failed: HTTP {status}: {body}"),
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
            .post(format!("{TWITCH_AUTH_URL}/token"))
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", refresh.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let detail = serde_json::from_str::<TokenErrorResponse>(&body)
                .ok()
                .and_then(|error| error.message)
                .unwrap_or_else(|| "refresh token or app credentials were rejected".into());
            bail!("token refresh failed ({status}): {detail}");
        }

        let token: TokenResponse = resp.json().await?;
        credentials::store_secret("twitch_access_token", &token.access_token)?;
        if let Some(ref new_refresh) = token.refresh_token {
            credentials::store_secret("twitch_refresh_token", new_refresh)?;
            *self.refresh_token_value.write().await = Some(new_refresh.clone());
        }
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
                .header("Client-Id", &self.client_id)
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
                tracing::warn!(url = %url, secs = backoff.as_secs(), "Twitch rate-limited; backing off");
                drop(resp);
                tokio::time::sleep(backoff).await;
                continue;
            }
            return Ok(resp.json().await?);
        }
        bail!("Twitch API exhausted retries for {url}")
    }

    #[allow(dead_code)]
    pub async fn is_authenticated(&self) -> bool {
        self.access_token.read().await.is_some()
    }
}

#[async_trait::async_trait]
impl Platform for TwitchPlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::Twitch
    }

    async fn authenticate(&self) -> Result<()> {
        if self.load_stored_tokens().await? {
            tracing::info!("Twitch: authenticated from stored tokens");
            return Ok(());
        }
        self.device_code_flow().await
    }

    async fn fetch_followed_channels(&self) -> Result<Vec<ChannelEntry>> {
        let user_id = self.user_id.read().await.clone();
        let Some(user_id) = user_id else {
            bail!("User ID not available - not authenticated");
        };

        let mut channels = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!("{TWITCH_API_URL}/channels/followed?user_id={user_id}&first=100");
            if let Some(ref c) = cursor {
                url.push_str(&format!("&after={c}"));
            }

            let resp: FollowedResponse = self.api_get(&url).await?;

            for ch in resp.data {
                channels.push(ChannelEntry {
                    id: ch.broadcaster_id,
                    platform: PlatformKind::Twitch,
                    name: ch.broadcaster_login,
                    display_name: ch.broadcaster_name,
                    is_live: false,
                    stream_title: None,
                    game_or_category: None,
                    viewer_count: None,
                    started_at: None,
                    thumbnail_url: None,
                    auto_record: false,
                    last_live_at: None,
                });
            }

            match resp.pagination.cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => break,
            }
        }

        Ok(channels)
    }

    async fn check_live_status(&self, channel_ids: &[String]) -> Result<Vec<ChannelEntry>> {
        let mut live_channels = Vec::new();

        // Twitch allows up to 100 user_id params per request
        for chunk in channel_ids.chunks(100) {
            let params: String = chunk
                .iter()
                .map(|id| format!("user_id={id}"))
                .collect::<Vec<_>>()
                .join("&");

            let url = format!("{TWITCH_API_URL}/streams?{params}");
            let resp: StreamsResponse = self.api_get(&url).await?;

            for stream in resp.data {
                let started_at = stream.started_at.as_deref().and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                });

                let thumbnail = stream
                    .thumbnail_url
                    .map(|url| url.replace("{width}", "440").replace("{height}", "248"));

                live_channels.push(ChannelEntry {
                    id: stream.user_id,
                    platform: PlatformKind::Twitch,
                    name: stream.user_login,
                    display_name: stream.user_name,
                    is_live: stream.stream_type.as_deref().map_or(true, |t| t == "live"),
                    stream_title: stream.title,
                    game_or_category: stream.game_name,
                    viewer_count: stream.viewer_count,
                    started_at,
                    thumbnail_url: thumbnail,
                    auto_record: false,
                    last_live_at: None,
                });
            }
        }

        Ok(live_channels)
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
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url =
                format!("{TWITCH_API_URL}/videos?user_id={channel_id}&type=archive&first=100");
            if let Some(ref c) = cursor {
                url.push_str(&format!("&after={c}"));
            }

            let resp: serde_json::Value = self.api_get(&url).await?;
            let items = resp
                .get("data")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                break;
            }

            for item in &items {
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled")
                    .to_string();
                let url_str = item
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| format!("https://www.twitch.tv/videos/{id}"));
                let published_at =
                    item.get("published_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| {
                            chrono::DateTime::parse_from_rfc3339(s)
                                .ok()
                                .map(|dt| dt.with_timezone(&chrono::Utc))
                        });
                let duration = item
                    .get("duration")
                    .and_then(|v| v.as_str())
                    .and_then(parse_twitch_duration);
                let thumbnail = item
                    .get("thumbnail_url")
                    .and_then(|v| v.as_str())
                    .map(|u| u.replace("%{width}", "440").replace("%{height}", "248"));

                if let (Some(after), Some(pub_at)) = (since, published_at) {
                    if pub_at < after {
                        return Ok(out);
                    }
                }

                out.push(VodEntry {
                    id,
                    platform: PlatformKind::Twitch,
                    channel_id: channel_id.to_string(),
                    title,
                    published_at,
                    duration,
                    url: url_str,
                    thumbnail_url: thumbnail,
                    // Twitch `type=archive` videos are past broadcasts.
                    kind: crate::platform::VodKind::LiveBroadcast,
                });

                if let Some(cap) = limit {
                    if out.len() >= cap {
                        return Ok(out);
                    }
                }
            }

            cursor = resp
                .get("pagination")
                .and_then(|p| p.get("cursor"))
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

/// Parse Twitch's `1h2m3s` duration format into a Duration.
fn parse_twitch_duration(s: &str) -> Option<std::time::Duration> {
    let mut total = 0u64;
    let mut acc = 0u64;
    for ch in s.chars() {
        match ch {
            '0'..='9' => acc = acc * 10 + (ch as u64 - '0' as u64),
            'h' => {
                total += acc * 3600;
                acc = 0;
            }
            'm' => {
                total += acc * 60;
                acc = 0;
            }
            's' => {
                total += acc;
                acc = 0;
            }
            _ => return None,
        }
    }
    Some(std::time::Duration::from_secs(total))
}

#[cfg(test)]
mod token_health_tests {
    use super::token_needs_refresh;
    use std::time::Duration;

    #[test]
    fn refreshes_at_safety_window_boundary() {
        assert!(token_needs_refresh(900, Duration::from_secs(900)));
        assert!(token_needs_refresh(899, Duration::from_secs(900)));
        assert!(!token_needs_refresh(901, Duration::from_secs(900)));
    }

    #[test]
    fn zero_window_accepts_any_unexpired_token() {
        assert!(!token_needs_refresh(1, Duration::ZERO));
        assert!(token_needs_refresh(0, Duration::ZERO));
    }
}
