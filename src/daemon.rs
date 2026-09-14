use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::events::DaemonEvent;
use crate::ipc::{self, ClientMessage, ServerMessage};
use crate::monitor::ChannelMonitor;
use crate::platform::{ChannelEntry, Platform, PlatformKind};
use crate::recording::job::RecordingJob;
use crate::recording::RecordingCommand;

/// Cap on retained terminal (Finished/Failed) recordings. Active jobs are
/// never evicted; this only bounds the completed-history tail so the map
/// can't grow without limit over a long-running daemon (roadmap item 8).
const MAX_TERMINAL_RECORDINGS: usize = 200;

/// Cap on concurrently-handled client connections (TUI + webui share the
/// daemon over the Unix socket). Excess connections are dropped rather than
/// queued so the accept loop can't be starved (roadmap item 9).
const MAX_CLIENT_TASKS: usize = 64;

/// Daemon state — maintained from internal events.
struct DaemonState {
    channels: Vec<ChannelEntry>,
    recordings: HashMap<Uuid, RecordingJob>,
    twitch_connected: bool,
    youtube_connected: bool,
    patreon_connected: bool,
    pending_auth: Option<(PlatformKind, String, String)>,
    auth_queue: std::collections::VecDeque<(PlatformKind, String, String)>,
    // Latest Patreon snapshot, cached so a client connecting between polls
    // sees Patreon immediately (not after up to a full poll interval).
    patreon_creators: Vec<ChannelEntry>,
    patreon_posts: Vec<crate::platform::patreon::PatreonPost>,
    /// Platforms whose credentials currently need human attention (Tier 1
    /// auth signal). Additive to the IPC snapshot — no protocol bump.
    auth_issues: Vec<crate::platform::AuthIssue>,
}

impl DaemonState {
    fn snapshot(&self) -> ServerMessage {
        ServerMessage::StateSnapshot {
            version: crate::ipc::IPC_PROTOCOL_VERSION,
            channels: self.channels.clone(),
            recordings: self.recordings.clone(),
            twitch_connected: self.twitch_connected,
            youtube_connected: self.youtube_connected,
            patreon_connected: self.patreon_connected,
            pending_auth: self.pending_auth.clone(),
            patreon_creators: self.patreon_creators.clone(),
            patreon_posts: self.patreon_posts.clone(),
            auth_issues: self.auth_issues.clone(),
        }
    }

    /// Drop the oldest terminal recordings beyond [`MAX_TERMINAL_RECORDINGS`].
    /// Active jobs (ResolvingUrl/Recording/Stopping) are always kept.
    fn evict_old_terminal(&mut self) {
        use crate::recording::job::RecordingState;
        let mut terminal: Vec<(Uuid, chrono::DateTime<chrono::Utc>)> = self
            .recordings
            .iter()
            .filter(|(_, j)| matches!(j.state, RecordingState::Finished | RecordingState::Failed))
            .map(|(id, j)| (*id, j.started_at))
            .collect();
        if terminal.len() <= MAX_TERMINAL_RECORDINGS {
            return;
        }
        // Oldest first; remove everything beyond the cap.
        terminal.sort_by_key(|(_, started)| *started);
        let remove = terminal.len() - MAX_TERMINAL_RECORDINGS;
        for (id, _) in terminal.into_iter().take(remove) {
            self.recordings.remove(&id);
        }
    }

    /// Insert or update an [`crate::platform::AuthIssue`] for `(kind,
    /// source)`, keeping the original `since` timestamp if one already
    /// exists for that pair — the issue's age is "how long has this been
    /// broken," not "when did the most recent poll notice it."
    fn upsert_auth_issue(
        &mut self,
        kind: PlatformKind,
        source: crate::platform::AuthSource,
        reason: String,
    ) {
        if let Some(existing) = self
            .auth_issues
            .iter_mut()
            .find(|i| i.kind == kind && i.source == source)
        {
            existing.reason = reason;
        } else {
            self.auth_issues.push(crate::platform::AuthIssue {
                kind,
                source,
                reason,
                since: chrono::Utc::now(),
            });
        }
    }

    fn apply(&mut self, event: &DaemonEvent) {
        match event {
            DaemonEvent::ChannelsUpdated(channels) => {
                self.channels = channels.clone();
            }
            DaemonEvent::PatreonState { creators, posts } => {
                self.patreon_creators = creators.clone();
                self.patreon_posts = posts.clone();
            }
            DaemonEvent::RecordingStarted { job } => {
                self.recordings.insert(job.id, job.clone());
            }
            DaemonEvent::RecordingProgress {
                job_id,
                bytes_written,
                duration_secs,
                ..
            } => {
                if let Some(job) = self.recordings.get_mut(job_id) {
                    job.bytes_written = *bytes_written;
                    job.duration_secs = *duration_secs;
                    job.state = crate::recording::job::RecordingState::Recording;
                    // Bytes on disk are the evidence that this platform's
                    // cookie jar (if any) works again. `RecordingStarted` is
                    // too early: it fires at process spawn, so a failing
                    // retry would clear the issue and re-raise it on every
                    // attempt, resetting `since` each time.
                    if *bytes_written > 0 {
                        let platform = job.platform;
                        self.auth_issues.retain(|i| {
                            !(i.kind == platform
                                && i.source == crate::platform::AuthSource::Cookies)
                        });
                    }
                }
            }
            DaemonEvent::RecordingFinished {
                job_id,
                final_state,
                error,
                new_path,
            } => {
                if let Some(job) = self.recordings.get_mut(job_id) {
                    job.state = *final_state;
                    job.error = error.clone();
                    // Finalisation may have corrected the extension to match
                    // the container that was actually written. Adopt the new
                    // path before anything persists, or the journal keeps
                    // pointing at a name that no longer exists.
                    if let Some(p) = new_path {
                        job.output_path = p.clone();
                    }
                }
                self.evict_old_terminal();
            }
            DaemonEvent::RecordingsPruned { job_ids } => {
                for id in job_ids {
                    self.recordings.remove(id);
                }
            }
            DaemonEvent::DeviceCodeRequired {
                kind,
                verification_uri,
                user_code,
            } => {
                let entry = (*kind, verification_uri.clone(), user_code.clone());
                if matches!(&self.pending_auth, Some((p, _, _)) if *p == entry.0) {
                    self.pending_auth = Some(entry);
                } else {
                    self.auth_queue.retain(|(p, _, _)| *p != entry.0);
                    if self.pending_auth.is_none() {
                        self.pending_auth = Some(entry);
                    } else {
                        self.auth_queue.push_back(entry);
                    }
                }
            }
            DaemonEvent::PlatformAuthenticated { kind } => {
                match kind {
                    PlatformKind::Twitch => self.twitch_connected = true,
                    PlatformKind::YouTube => self.youtube_connected = true,
                    PlatformKind::Patreon => self.patreon_connected = true,
                }
                if matches!(&self.pending_auth, Some((pending, _, _)) if pending == kind) {
                    self.pending_auth = self.auth_queue.pop_front();
                }
                self.auth_queue.retain(|(p, _, _)| p != kind);
                // Successful auth clears any outstanding OAuth issue for
                // this platform (a stale Cookies issue, if any, is a
                // separate credential and is left alone).
                self.auth_issues.retain(|i| {
                    !(i.kind == *kind && i.source == crate::platform::AuthSource::OAuth)
                });
            }
            DaemonEvent::PlatformAuthenticationRequired { kind, reason } => {
                match kind {
                    PlatformKind::Twitch => self.twitch_connected = false,
                    PlatformKind::YouTube => self.youtube_connected = false,
                    PlatformKind::Patreon => self.patreon_connected = false,
                }
                self.upsert_auth_issue(*kind, crate::platform::AuthSource::OAuth, reason.clone());
            }
            DaemonEvent::CookieSessionRejected { kind, reason } => {
                self.upsert_auth_issue(*kind, crate::platform::AuthSource::Cookies, reason.clone());
            }
            _ => {}
        }
    }
}

/// Daemon-side plugin host. (W2 phase 2.)
///
/// The daemon used to ignore plugins entirely — they were a TUI / bin
/// concern. With the webui's PluginRpc surface, plugins need to be
/// alive inside the daemon process too so their DB hooks, status_line
/// contributions, and (eventually) verb dispatchers have somewhere to
/// run.
///
/// Verb dispatch over IPC is a phase-3 follow-up — it requires a
/// minimal "DaemonAppState" wrapper for plugins to read recordings
/// from, since the full AppState is TUI-scoped. Today the daemon
/// loads + initializes plugins, runs their event hooks, and logs any
/// PluginRpc requests. The wire contract is stable; only the
/// dispatcher body changes.
pub struct DaemonPluginHost {
    pub registry: crate::plugin::registry::PluginRegistry,
    /// `(extensions section name, marker filename)` pairs a plugin wants
    /// [`crate::config::AppConfig::post_pull_markers`] to touch in each
    /// landed episode directory when that section's `enabled = true`. Core
    /// names neither the section nor the marker itself — whoever composes
    /// plugins into the running app (`register_first_party_plugins` in the
    /// Creator Edition binary, today) populates this before starting the
    /// daemon. Empty in the pure-PVR build, since nothing registers into it.
    pub post_pull_markers: Vec<(String, String)>,
}

impl DaemonPluginHost {
    pub fn new() -> Self {
        Self {
            registry: crate::plugin::registry::PluginRegistry::new(),
            post_pull_markers: Vec::new(),
        }
    }
}

impl Default for DaemonPluginHost {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn run() -> Result<()> {
    run_with_plugins_at(DaemonPluginHost::new(), None).await
}

pub async fn run_with_plugins(host: DaemonPluginHost) -> Result<()> {
    run_with_plugins_at(host, None).await
}

/// Canonicalise for identity comparison, falling back to the path as given.
///
/// Two records for the same recording can spell its path differently (one
/// from a directory walk, one from the journal), so comparing the raw
/// strings would miss the match that matters.
fn canonical_or_self(p: &std::path::Path) -> std::path::PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Run the daemon with the configuration selected by the process entrypoint.
pub async fn run_with_plugins_at(
    host: DaemonPluginHost,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    // Initialize logging
    let state_dir = AppConfig::state_dir();
    std::fs::create_dir_all(&state_dir)?;

    // Rolling, capped log files (roadmap item 15): daily rotation, keep the
    // last 7 days, so logs never grow unbounded and users never SSH for them
    // (the webui tails the newest file). Files are `strivo.<date>.log`.
    let appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("strivo")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&state_dir)
        .context("build rolling log appender")?;
    let (nb_writer, log_guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(nb_writer)
        .with_ansi(false)
        .init();
    // Keep the non-blocking writer's flush guard alive for the daemon's
    // lifetime; dropping it would stop log flushing.
    let _log_guard = log_guard;

    tracing::info!("StriVo daemon starting");

    // Strivo Pro: kick off the 72h licence-refresh loop — but only when
    // STRIVO_LICENCE_URL is actually configured. The licence backend is on
    // an explicit release hold (README's "Release boundary"; ADR 0002 /
    // CE06), and every edition shares this entrypoint, so a default PVR (or
    // Creator) install with the env var unset spawns no task and makes no
    // network call at all, rather than an idle-but-live one. Handle is
    // intentionally leaked when it does spawn — the task lives for the
    // daemon's lifetime.
    let _licence_refresh =
        crate::licence::spawn_refresh_loop(crate::licence::DEFAULT_REFRESH_INTERVAL);
    if _licence_refresh.is_some() {
        tracing::info!("licence backend configured; refresh loop started");
    }

    // Write PID file
    let pid_path = ipc::pid_path();
    std::fs::write(&pid_path, std::process::id().to_string())?;

    // Validate external tools
    crate::check_external_tools();

    // Load config
    let config = AppConfig::load(config_path)?;
    tracing::info!("Config loaded");
    for w in config.config_warnings() {
        tracing::warn!("config: {w}");
    }

    // W2 phase 2 — init plugins inside the daemon process. Plugins
    // are registered by the caller (strivo-bin's Command::Daemon
    // arm) via DaemonPluginHost.registry; init_all opens their
    // DBs, sets up tandem state, etc. Verb dispatch is still
    // logging-only until the AppState wrapper lands (W2-phase-3).
    let mut host = host;
    if !host.registry.is_empty() {
        match host.registry.init_all(&config) {
            Ok(()) => {
                tracing::info!(
                    plugin_count = host.registry.len(),
                    "daemon: plugin host initialized"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "daemon: some plugins failed to initialize; healthy plugins remain available"
                );
            }
        }
    }
    // Grab the caller-registered post-pull marker list before `host.registry`
    // moves below; the bulk-download manager needs it (see
    // `DaemonPluginHost::post_pull_markers`).
    let post_pull_markers = host.post_pull_markers.clone();

    // W2-phase-3: share the registry with per-connection handlers so
    // PluginRpc over IPC can actually dispatch on_verb (it's idle after
    // init_all otherwise). tokio Mutex — dispatch is sync and brief.
    let registry = std::sync::Arc::new(tokio::sync::Mutex::new(host.registry));
    let pipeline_path = AppConfig::data_dir().join("pipelines.json");
    let pipeline_registry =
        crate::pipeline::PipelineRegistry::open(&pipeline_path).unwrap_or_else(|error| {
            tracing::error!(
                %error,
                path = %pipeline_path.display(),
                "daemon: pipeline recovery failed; starting with an empty registry"
            );
            crate::pipeline::PipelineRegistry::new()
        });
    let pipelines = std::sync::Arc::new(tokio::sync::Mutex::new(pipeline_registry));

    // Open the persistence db (jobs / catalog) and recover any
    // jobs that were marked running when the daemon last died. Recovery is
    // intentionally minimal: we mark orphans as 'interrupted' so the audit log
    // is honest. Catalog-pull resumption is automatic — the catalog dedupe
    // index in §5 already skips already-recorded VODs on the next pull.
    let persist_db =
        match crate::recording::persist::PersistDb::open(&AppConfig::data_dir().join("jobs.db")) {
            Ok(db) => {
                match db.recover_orphaned_running().await {
                    Ok(n) if n > 0 => {
                        tracing::info!("daemon: marked {n} orphan job(s) as interrupted")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("daemon: persist recover failed: {e}"),
                }
                Some(Arc::new(db))
            }
            Err(e) => {
                tracing::warn!("daemon: failed to open jobs.db: {e} — durability disabled");
                None
            }
        };

    let cancel = CancellationToken::new();

    // Internal event channel
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<DaemonEvent>();
    let (pipeline_action_tx, mut pipeline_action_rx) =
        mpsc::unbounded_channel::<crate::plugin::PluginAction>();

    // Recording command channel
    let (recording_tx, recording_rx) = mpsc::unbounded_channel();

    // Broadcast channel for client fan-out
    let (broadcast_tx, _) = broadcast::channel::<DaemonEvent>(256);

    // Shared auth notify
    let auth_notify = Arc::new(tokio::sync::Notify::new());

    // Initialize platforms
    let mut platforms: Vec<Arc<RwLock<dyn Platform>>> = Vec::new();
    const INITIAL_AUTH_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);
    const MAX_AUTH_BACKOFF: std::time::Duration = std::time::Duration::from_secs(300);

    let mut twitch_handle: Option<Arc<RwLock<crate::platform::twitch::TwitchPlatform>>> = None;
    let mut youtube_handle: Option<Arc<RwLock<crate::platform::youtube::YouTubePlatform>>> = None;
    let mut patreon_handle: Option<Arc<crate::platform::patreon::PatreonClient>> = None;

    // Re-authentication signals. `ReloadPlatformCredentials` writes the new
    // client id/secret into the platform and then pokes these; each platform's
    // auth task owns authentication, so a reload never races a second
    // device-code flow against the one already running — it cancels it.
    let twitch_reauth = Arc::new(tokio::sync::Notify::new());
    let youtube_reauth = Arc::new(tokio::sync::Notify::new());
    let patreon_reauth = Arc::new(tokio::sync::Notify::new());
    let reload_config_path = config_path.map(|p| p.to_path_buf());

    if let Some(ref twitch_config) = config.twitch {
        let mut twitch = crate::platform::twitch::TwitchPlatform::new(
            twitch_config.client_id.clone(),
            twitch_config.client_secret.clone(),
        );
        twitch.set_event_tx(event_tx.clone());
        let twitch = Arc::new(RwLock::new(twitch));
        platforms.push(twitch.clone() as Arc<RwLock<dyn Platform>>);
        twitch_handle = Some(twitch.clone());

        let tx = event_tx.clone();
        let notify = auth_notify.clone();
        let reauth = twitch_reauth.clone();
        tokio::spawn(async move {
            // The daemon routinely starts before the network is usable — the
            // user unit waits on the Secret Service, not on DNS — so the very
            // first `/oauth2/validate` can fail at the transport layer. That
            // says nothing about the stored token, but returning here left
            // Twitch unauthenticated for the whole process lifetime and never
            // reached the hourly validation loop below, which already treats
            // an outage as retryable. Back off and keep trying instead.
            //
            // `reauth` cuts any attempt short: new credentials have landed, so
            // an in-flight device-code flow is polling for the wrong
            // application and should be abandoned, not waited out.
            let mut backoff = INITIAL_AUTH_BACKOFF;
            loop {
                let attempt = tokio::select! {
                    biased;
                    _ = reauth.notified() => None,
                    result = async { twitch.read().await.authenticate().await } => Some(result),
                };
                match attempt {
                    None => {
                        tracing::info!("Twitch credentials replaced; re-authenticating");
                        backoff = INITIAL_AUTH_BACKOFF;
                    }
                    Some(Ok(())) => {
                        tracing::info!("Twitch authenticated");
                        let _ = tx.send(DaemonEvent::PlatformAuthenticated {
                            kind: PlatformKind::Twitch,
                        });
                        notify.notify_one();
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(
                            retry_in_secs = backoff.as_secs(),
                            "Twitch auth failed: {e}"
                        );
                        let _ = tx.send(DaemonEvent::Error(format!("Twitch auth: {e}")));
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {
                                backoff = (backoff * 2).min(MAX_AUTH_BACKOFF);
                            }
                            _ = reauth.notified() => {
                                tracing::info!("Twitch credentials replaced; re-authenticating");
                                backoff = INITIAL_AUTH_BACKOFF;
                            }
                        }
                    }
                }
            }

            // Twitch requires hourly token validation. Refresh early enough
            // that a recording never starts on the edge of token expiry.
            let mut auth_check = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
            auth_check.tick().await;
            loop {
                tokio::select! {
                    _ = auth_check.tick() => {}
                    _ = reauth.notified() => {
                        // Credentials were replaced after we were already
                        // authenticated: the stored session belongs to the old
                        // application, so log in again rather than waiting for
                        // the hourly check to notice.
                        tracing::info!("Twitch credentials replaced; re-authenticating");
                        match twitch.read().await.authenticate().await {
                            Ok(()) => {
                                let _ = tx.send(DaemonEvent::PlatformAuthenticated {
                                    kind: PlatformKind::Twitch,
                                });
                            }
                            Err(error) => {
                                tracing::warn!("Twitch re-authentication failed: {error}");
                                let _ = tx.send(DaemonEvent::Error(format!(
                                    "Twitch auth: {error}"
                                )));
                            }
                        }
                        continue;
                    }
                }
                let health = twitch
                    .read()
                    .await
                    .ensure_fresh_token(std::time::Duration::from_secs(15 * 60))
                    .await;
                match health {
                    Ok(crate::platform::twitch::TwitchTokenHealth::Valid { expires_in_secs }) => {
                        tracing::debug!(expires_in_secs, "Twitch token validation succeeded")
                    }
                    Ok(crate::platform::twitch::TwitchTokenHealth::Refreshed) => {
                        tracing::info!("Twitch token refreshed automatically");
                        let _ = tx.send(DaemonEvent::PlatformAuthenticated {
                            kind: PlatformKind::Twitch,
                        });
                    }
                    Ok(crate::platform::twitch::TwitchTokenHealth::LoginRequired { reason }) => {
                        tracing::warn!(%reason, "Twitch login required");
                        let _ = tx.send(DaemonEvent::PlatformAuthenticationRequired {
                            kind: PlatformKind::Twitch,
                            reason,
                        });
                        // authenticate() retries refresh, then launches the
                        // device-code flow when Twitch requires user consent.
                        match twitch.read().await.authenticate().await {
                            Ok(()) => {
                                let _ = tx.send(DaemonEvent::PlatformAuthenticated {
                                    kind: PlatformKind::Twitch,
                                });
                            }
                            Err(error) => {
                                tracing::warn!("Twitch re-authentication failed: {error}");
                            }
                        }
                    }
                    Err(error) => {
                        // A network outage is not proof credentials are stale.
                        tracing::warn!("Twitch token validation unavailable: {error}");
                    }
                }
            }
        });
    }

    if let Some(ref yt_config) = config.youtube {
        let mut youtube = crate::platform::youtube::YouTubePlatform::new(
            yt_config.client_id.clone(),
            yt_config.client_secret.clone(),
            yt_config.cookies_path.clone(),
        );
        youtube.set_event_tx(event_tx.clone());
        let youtube = Arc::new(RwLock::new(youtube));
        platforms.push(youtube.clone() as Arc<RwLock<dyn Platform>>);
        youtube_handle = Some(youtube.clone());

        let tx = event_tx.clone();
        let notify = auth_notify.clone();
        let reauth = youtube_reauth.clone();
        tokio::spawn(async move {
            // Same shape as Twitch above: retry rather than give up on a
            // boot-time network error, and abandon an in-flight device-code
            // flow the moment new credentials replace the ones it is using.
            let mut backoff = INITIAL_AUTH_BACKOFF;
            loop {
                let attempt = tokio::select! {
                    biased;
                    _ = reauth.notified() => None,
                    result = async { youtube.read().await.authenticate().await } => Some(result),
                };
                match attempt {
                    None => {
                        tracing::info!("YouTube credentials replaced; re-authenticating");
                        backoff = INITIAL_AUTH_BACKOFF;
                    }
                    Some(Ok(())) => {
                        tracing::info!("YouTube authenticated");
                        let _ = tx.send(DaemonEvent::PlatformAuthenticated {
                            kind: PlatformKind::YouTube,
                        });
                        notify.notify_one();
                        // Stay alive to answer a later credential change.
                        reauth.notified().await;
                        tracing::info!("YouTube credentials replaced; re-authenticating");
                        backoff = INITIAL_AUTH_BACKOFF;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(
                            retry_in_secs = backoff.as_secs(),
                            "YouTube auth failed: {e}"
                        );
                        let _ = tx.send(DaemonEvent::Error(format!("YouTube auth: {e}")));
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {
                                backoff = (backoff * 2).min(MAX_AUTH_BACKOFF);
                            }
                            _ = reauth.notified() => {
                                tracing::info!("YouTube credentials replaced; re-authenticating");
                                backoff = INITIAL_AUTH_BACKOFF;
                            }
                        }
                    }
                }
            }
        });
    }

    // Spawn Patreon auth + monitor
    if let Some(ref patreon_config) = config.patreon {
        let mut patreon_client = crate::platform::patreon::PatreonClient::new(
            patreon_config.client_id.clone(),
            patreon_config.client_secret.clone(),
        );
        patreon_client.set_event_tx(event_tx.clone());
        let patreon_client = Arc::new(patreon_client);
        patreon_handle = Some(patreon_client.clone());

        let tx = event_tx.clone();
        let rec_tx = recording_tx.clone();
        let cfg = config.clone();
        let cancel_clone = cancel.clone();
        let reauth = patreon_reauth.clone();
        tokio::spawn(async move {
            let mut backoff = INITIAL_AUTH_BACKOFF;
            loop {
                let attempt = tokio::select! {
                    biased;
                    _ = reauth.notified() => None,
                    result = patreon_client.authorize() => Some(result),
                };
                match attempt {
                    None => {
                        tracing::info!("Patreon credentials replaced; re-authenticating");
                        backoff = INITIAL_AUTH_BACKOFF;
                        continue;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(
                            retry_in_secs = backoff.as_secs(),
                            "Patreon auth failed: {e}"
                        );
                        let _ = tx.send(DaemonEvent::Error(format!("Patreon auth: {e}")));
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {
                                backoff = (backoff * 2).min(MAX_AUTH_BACKOFF);
                            }
                            _ = reauth.notified() => {
                                tracing::info!("Patreon credentials replaced; re-authenticating");
                                backoff = INITIAL_AUTH_BACKOFF;
                            }
                        }
                        continue;
                    }
                    Some(Ok(())) => {}
                }

                tracing::info!("Patreon authenticated");
                let _ = tx.send(DaemonEvent::PlatformAuthenticated {
                    kind: PlatformKind::Patreon,
                });
                backoff = INITIAL_AUTH_BACKOFF;

                // The monitor consumes itself when it runs, so it is rebuilt
                // whenever credentials change. `new()` reloads the per-campaign
                // last-checked map from the state file, so the restart doesn't
                // re-pull anything already seen.
                let monitor = crate::monitor::patreon::PatreonMonitor::new(
                    patreon_client.clone(),
                    cfg.clone(),
                    tx.clone(),
                    rec_tx.clone(),
                    cancel_clone.clone(),
                );
                tokio::select! {
                    _ = monitor.run() => break,
                    _ = reauth.notified() => {
                        tracing::info!("Patreon credentials replaced; re-authenticating");
                    }
                }
            }
        });
    }

    // Bundle the live platform handles so a client connection can apply
    // credentials saved by the web UI without restarting the daemon.
    let platform_control = PlatformControl {
        config_path: reload_config_path,
        twitch: twitch_handle.clone(),
        youtube: youtube_handle.clone(),
        patreon: patreon_handle.clone(),
        twitch_reauth: twitch_reauth.clone(),
        youtube_reauth: youtube_reauth.clone(),
        patreon_reauth: patreon_reauth.clone(),
    };

    // Spawn recording manager
    let rec_config = config.clone();
    let rec_tx = event_tx.clone();
    let rec_cancel = cancel.clone();
    let rec_twitch = twitch_handle.clone();
    tokio::spawn(async move {
        crate::recording::run_manager(rec_config, rec_twitch, recording_rx, rec_tx, rec_cancel)
            .await;
    });

    // Spawn the per-channel bulk back-catalog download manager (task #71).
    let bulk_tx =
        crate::recording::bulk::spawn(config.clone(), event_tx.clone(), post_pull_markers);

    // Spawn channel monitor
    let mut interval_ctl: Option<(
        std::sync::Arc<std::sync::atomic::AtomicU64>,
        std::sync::Arc<tokio::sync::Notify>,
    )> = None;
    let poll_notify = if !platforms.is_empty() {
        let mut monitor = ChannelMonitor::new(
            platforms.clone(),
            config.clone(),
            event_tx.clone(),
            recording_tx.clone(),
            cancel.clone(),
        );
        monitor.set_auth_notify(auth_notify.clone());
        if let Some(ref db) = persist_db {
            monitor.set_persist(db.clone());
        }
        let poll_notify = monitor.poll_notify();
        interval_ctl = Some(monitor.interval_controls());
        tokio::spawn(async move {
            monitor.run().await;
        });
        Some(poll_notify)
    } else {
        None
    };

    // Twitch EventSub (real-time stream.online → immediate poll). Reuses the
    // monitor's poll_notify so the proven auto-record path runs within seconds
    // of a broadcast start, instead of waiting up to a poll interval.
    if let (Some(th), Some(tcfg), Some(pn)) = (
        twitch_handle.clone(),
        config.twitch.clone(),
        poll_notify.clone(),
    ) {
        let client_id = tcfg.client_id.clone();
        let auto: std::collections::HashSet<String> = config
            .auto_record_channels
            .iter()
            .filter(|a| a.platform == "Twitch")
            .map(|a| a.channel_id.clone())
            .collect();
        let cancel_es = cancel.clone();
        tokio::spawn(async move {
            // Wait for Twitch auth (token present) before subscribing.
            let token_arc = loop {
                if cancel_es.is_cancelled() {
                    return;
                }
                let arc = th.read().await.access_token_arc();
                if arc.read().await.is_some() {
                    break arc;
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            };
            // Having an access token is not the same as being ready to call
            // the API: the client also has to resolve the user id, and it does
            // that slightly later. Listing follows can therefore fail with
            // "not authenticated" even though the token gate above passed.
            //
            // Retry instead of returning. The previous code abandoned EventSub
            // for the lifetime of the process on a single early failure, so a
            // race lost by a couple of seconds silently downgraded Twitch from
            // sub-second push to 90-second polling until the next restart.
            let all_ids: Vec<String> = {
                const MAX_ATTEMPTS: u32 = 10;
                let mut attempt = 0;
                loop {
                    if cancel_es.is_cancelled() {
                        return;
                    }
                    match th.read().await.fetch_followed_channels().await {
                        Ok(chs) => break chs.into_iter().map(|c| c.id).collect(),
                        Err(e) => {
                            attempt += 1;
                            if attempt >= MAX_ATTEMPTS {
                                tracing::warn!(
                                    "twitch eventsub: could not list follows after {attempt} \
                                     attempts, falling back to polling: {e:#}"
                                );
                                return;
                            }
                            tracing::debug!(
                                "twitch eventsub: follows not ready (attempt {attempt}): {e:#}"
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        }
                    }
                }
            };
            if all_ids.is_empty() {
                return;
            }
            // Twitch caps WebSocket EventSub at a max total cost of 10, and a
            // stream.online subscription for a broadcaster who hasn't authorized
            // our app costs 1 — so we can only push-subscribe to 10 channels.
            // Prioritize the auto-record set (the channels we actually PVR) so
            // they get sub-second detection; ordinary polling backstops the rest.
            let mut ids: Vec<String> = all_ids
                .iter()
                .filter(|id| auto.contains(*id))
                .cloned()
                .collect();
            for id in &all_ids {
                if ids.len() >= crate::platform::twitch_eventsub::MAX_EVENTSUB_SUBS {
                    break;
                }
                if !auto.contains(id) {
                    ids.push(id.clone());
                }
            }
            ids.truncate(crate::platform::twitch_eventsub::MAX_EVENTSUB_SUBS);
            if ids.len() < all_ids.len() {
                tracing::info!(
                    "twitch eventsub: real-time push for {} of {} follows (Twitch caps WebSocket subs at {}); polling backstops the rest",
                    ids.len(),
                    all_ids.len(),
                    crate::platform::twitch_eventsub::MAX_EVENTSUB_SUBS
                );
            }
            crate::platform::twitch_eventsub::EventSubClient {
                client_id,
                token: token_arc,
                channel_ids: ids,
                poll_notify: pn,
                cancel: cancel_es,
            }
            .run()
            .await;
        });
    }

    // YouTube WebSub (PubSubHubbub) — Google's hub pushes new-video / live
    // notifications to `strivo serve`'s public /yt-websub callback, which fires
    // a PollNow over IPC. Combined with the RSS-candidate poll this gives
    // near-real-time YouTube detection without burning Data API quota on a
    // tight interval. Only runs when a public callback URL is configured.
    if let (Some(yt), Some(url)) = (
        youtube_handle.clone(),
        config
            .youtube
            .as_ref()
            .and_then(|c| c.websub_callback_url.clone()),
    ) {
        let cancel_ws = cancel.clone();
        tokio::spawn(async move {
            crate::platform::youtube_websub::WebSubClient {
                callback_url: url,
                youtube: yt,
                cancel: cancel_ws,
            }
            .run()
            .await;
        });
    }

    // Spawn schedule manager
    if !config.schedule.is_empty() {
        let sched_config = config.clone();
        let sched_rec_tx = recording_tx.clone();
        let sched_event_tx = event_tx.clone();
        let sched_cancel = cancel.clone();
        tokio::spawn(async move {
            crate::recording::schedule::run_schedule_manager(
                sched_config,
                sched_rec_tx,
                sched_event_tx,
                sched_cancel,
            )
            .await;
        });
    }

    // Scan existing recordings. This walks the recording directory and
    // stats/canonicalizes every matched file — on a large library (the
    // documented use case) that's thousands of blocking syscalls run
    // directly on this async task before, which stalled the tokio runtime
    // worker thread (and everything else scheduled on it: IPC, HTTP,
    // channel polling) for however long the scan took. Moved to
    // spawn_blocking so it runs on the blocking thread pool instead.
    let scan_config = config.clone();
    let scanned = tokio::task::spawn_blocking(move || {
        crate::recording::scan::scan_existing_recordings(&scan_config)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!("recording scan task panicked: {e}");
        Vec::new()
    });

    // Initialize daemon state
    let mut state = DaemonState {
        channels: Vec::new(),
        recordings: HashMap::new(),
        twitch_connected: false,
        youtube_connected: false,
        patreon_connected: false,
        pending_auth: None,
        auth_queue: std::collections::VecDeque::new(),
        patreon_creators: Vec::new(),
        patreon_posts: Vec::new(),
        auth_issues: Vec::new(),
    };
    // Replay recordings from the journal FIRST so the disk scan can be
    // deduped against it. Journal entries carry the original Uuid, channel
    // link, and progress; the scan is only a backstop for files that
    // pre-date the journal.
    //
    // Dedupe is by output PATH, not by id. The scan derives a deterministic
    // v5 uuid from the file path while the journal holds the v4 uuid minted
    // when the capture started, so the same recording arrives under two
    // different ids and a keyed-by-id insert never collides — which is how
    // one file ended up listed twice in the library.
    let mut journal_paths: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    if let Some(ref db) = persist_db {
        // A journal entry is useful only while its media exists. Prune
        // confirmed deletions before rebuilding the UI snapshot; retain a
        // durable, human-readable audit trail in the rolling daemon log.
        match db.prune_missing_recordings().await {
            Ok(removed) => {
                for (id, path) in removed {
                    let thumb = crate::config::AppConfig::data_dir().join("thumbs").join(format!("{id}.jpg"));
                    let _ = std::fs::remove_file(&thumb);
                    tracing::info!(recording_id = %id, path = %path.display(), "Removed recording entry because its media file was deleted");
                }
            }
            Err(e) => tracing::warn!("Could not reconcile deleted recording files: {e}"),
        }
        match db.load_recording_jobs().await {
            Ok(jobs) => {
                let n = jobs.len();
                for job in jobs {
                    journal_paths.insert(canonical_or_self(&job.output_path));
                    state.recordings.insert(job.id, job);
                }
                if n > 0 {
                    tracing::info!("daemon: replayed {n} recording(s) from journal");
                }
            }
            Err(e) => tracing::warn!("daemon: failed to load recording journal: {e}"),
        }
    }

    // Now fold in the disk scan, skipping anything the journal already owns.
    let mut shadowed = 0usize;
    for job in scanned {
        if journal_paths.contains(&canonical_or_self(&job.output_path)) {
            shadowed += 1;
            continue;
        }
        state.recordings.insert(job.id, job);
    }
    if shadowed > 0 {
        tracing::debug!("daemon: {shadowed} scanned file(s) already known to the journal");
    }

    let shared_recordings = Arc::new(tokio::sync::RwLock::new(state.recordings.clone()));
    let pipeline_runtime = crate::pipeline::PipelineRuntime::spawn(
        pipelines.clone(),
        registry.clone(),
        shared_recordings.clone(),
        config.plugin_toggles.clone(),
        event_tx.clone(),
        pipeline_action_tx,
        cancel.clone(),
    );
    pipeline_runtime.wake();

    // Set up the IPC transport (Unix socket, or a named pipe on Windows).
    let socket_path = ipc::socket_path();
    let endpoint = ipc::Endpoint::current();
    let mut listener = ipc::Listener::bind(&endpoint).await?;
    tracing::info!("Listening on {}", socket_path.display());

    // Signal handler
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Received SIGINT, shutting down");
        cancel_signal.cancel();
    });

    #[cfg(unix)]
    {
        // Register the SIGTERM handler synchronously so a registration failure
        // surfaces as a startup error rather than panicking inside a spawned
        // task and silently losing graceful-shutdown.
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("failed to register SIGTERM handler")?;
        let cancel_term = cancel.clone();
        tokio::spawn(async move {
            sigterm.recv().await;
            tracing::info!("Received SIGTERM, shutting down");
            cancel_term.cancel();
        });
    }

    #[cfg(windows)]
    {
        // Windows has no SIGTERM. The equivalent stop paths are CTRL_CLOSE
        // (console window closed, or a service stop) and CTRL_SHUTDOWN (system
        // going down). Registered synchronously for the same reason as SIGTERM
        // above.
        //
        // Both are on a deadline: Windows gives the handler only a few seconds
        // before terminating the process regardless. Cancelling promptly is
        // what gives the recorders their window to stop ffmpeg gracefully, so
        // a recording is finalised rather than truncated on shutdown.
        let mut ctrl_close = tokio::signal::windows::ctrl_close()
            .context("failed to register CTRL_CLOSE handler")?;
        let mut ctrl_shutdown = tokio::signal::windows::ctrl_shutdown()
            .context("failed to register CTRL_SHUTDOWN handler")?;
        let cancel_win = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = ctrl_close.recv() => {
                    tracing::info!("Received CTRL_CLOSE, shutting down");
                }
                _ = ctrl_shutdown.recv() => {
                    tracing::info!("Received CTRL_SHUTDOWN, shutting down");
                }
            }
            cancel_win.cancel();
        });
    }

    // Cap concurrent client handler tasks so a flood of connections can't
    // spawn unbounded tasks (roadmap item 9). Excess connections are dropped
    // immediately; a TUI/webui reconnects on its own.
    let client_sem = Arc::new(tokio::sync::Semaphore::new(MAX_CLIENT_TASKS));

    // Reconcile deletions while the daemon is running, so stale rows do not
    // wait for a restart to disappear from connected clients.
    if let Some(db) = persist_db.clone() {
        let tx = event_tx.clone();
        let shared = shared_recordings.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tick.tick().await;
                let Ok(removed) = db.prune_missing_recordings().await else { continue };
                if removed.is_empty() { continue; }
                let mut ids = Vec::with_capacity(removed.len());
                for (id, path) in removed {
                    let _ = std::fs::remove_file(crate::config::AppConfig::data_dir().join("thumbs").join(format!("{id}.jpg")));
                    tracing::info!(recording_id = %id, path = %path.display(), "Removed recording entry because its media file was deleted");
                    ids.push(id);
                }
                { let mut snapshot = shared.write().await; for id in &ids { snapshot.remove(id); } }
                let _ = tx.send(crate::events::DaemonEvent::RecordingsPruned { job_ids: ids });
            }
        });
    }

    // Main loop
    loop {
        tokio::select! {
            // Accept new client connections
            result = listener.accept() => {
                match result {
                    Ok(stream) => {
                        let permit = match client_sem.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                tracing::warn!(
                                    "client task limit ({MAX_CLIENT_TASKS}) reached; dropping connection"
                                );
                                drop(stream);
                                continue;
                            }
                        };
                        let snapshot = state.snapshot();
                        let client_broadcast_rx = broadcast_tx.subscribe();
                        let rec_tx = recording_tx.clone();
                        let bulk_tx_ref = Some(bulk_tx.clone());
                        let poll_notify = poll_notify.clone();
                        let interval_ctl = interval_ctl.clone();
                        let cancel_ref = cancel.clone();

                        let client_config = config.clone();
                        let client_registry = registry.clone();
                        let client_pipeline_runtime = pipeline_runtime.clone();
                        let client_recordings = state.recordings.clone();
                        let client_event_tx = event_tx.clone();
                        let client_persist_db = persist_db.clone();
                        let client_platform_control = platform_control.clone();
                        tokio::spawn(async move {
                            // Held for the connection's lifetime; released on drop.
                            let _permit = permit;
                            if let Err(e) = handle_client(
                                stream,
                                snapshot,
                                client_broadcast_rx,
                                rec_tx,
                                bulk_tx_ref,
                                client_config,
                                client_registry,
                                client_pipeline_runtime,
                                client_recordings,
                                client_event_tx,
                                poll_notify,
                                interval_ctl,
                                client_persist_db,
                                client_platform_control,
                                cancel_ref,
                            ).await {
                                tracing::debug!("Client disconnected: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("Accept error: {e}");
                    }
                }
            }
            // Process internal events
            Some(event) = event_rx.recv() => {
                {
                    let de = &event;
                    state.apply(de);
                    *shared_recordings.write().await = state.recordings.clone();
                    // Fan out to all connected clients
                    let _ = broadcast_tx.send(de.clone());

                    // Desktop banners (notify-rust). The NotificationsConfig
                    // flags and DaemonEvent::Notification producers were wired
                    // long before any dispatcher existed; this is it.
                    dispatch_desktop_notification(&config.notifications, de);

                    // Outbound webhook — fire-and-forget POST on a spawned
                    // task; never blocks the event loop. Resolve a channel
                    // key (and whether this is an upload/VOD pull vs a live
                    // capture) for the event types that carry one, so a
                    // per-channel alert override can apply; every other
                    // event type passes `None`, which preserves today's
                    // global-only behaviour exactly.
                    let channel_ctx: Option<(String, bool)> = match de {
                        DaemonEvent::ChannelWentLive(ch) => {
                            Some((format!("{}:{}", ch.platform, ch.id), false))
                        }
                        DaemonEvent::RecordingFinished { job_id, .. } => {
                            state.recordings.get(job_id).map(|job| {
                                (
                                    format!("{}:{}", job.platform, job.channel_id),
                                    job.source_url.is_some(),
                                )
                            })
                        }
                        _ => None,
                    };
                    crate::webhook::dispatch_webhook(
                        &config.notifications,
                        &config.channel_alerts,
                        channel_ctx.as_ref().map(|(k, u)| (k.as_str(), *u)),
                        de,
                    );

                    // Plugins are daemon residents: lifecycle events must reach
                    // them even when no web client is connected.
                    let actions = {
                        let mut plugins = registry.lock().await;
                        let ctx = crate::plugin::VerbContext {
                            recordings: &state.recordings,
                            plugin_toggles: &config.plugin_toggles,
                        };
                        plugins.dispatch_event(de, &ctx)
                    };
                    process_daemon_plugin_actions(
                        actions,
                        &registry,
                        &pipeline_runtime,
                        &event_tx,
                    );

                    // Auto VOD backfill: when a Twitch live recording
                    // ends cleanly, schedule a delayed download of the
                    // archive VOD so we get the first ~5 minutes the
                    // HLS pull missed.
                    if config.recording.auto_vod_backfill {
                        if let DaemonEvent::RecordingFinished {
                            job_id, final_state, ..
                        } = de
                        {
                            if *final_state == crate::recording::job::RecordingState::Finished {
                                if let (Some(job), Some(twitch)) =
                                    (state.recordings.get(job_id), twitch_handle.as_ref())
                                {
                                    if job.platform == PlatformKind::Twitch {
                                        crate::recording::vod_backfill::spawn(
                                            crate::recording::vod_backfill::BackfillRequest {
                                                channel_id: job.channel_id.clone(),
                                                channel_name: job.channel_name.clone(),
                                                started_at: job.started_at,
                                                live_output_path: job.output_path.clone(),
                                                stream_title: job.stream_title.clone(),
                                                delay_secs: config.recording.vod_backfill_delay_secs,
                                            },
                                            twitch.clone(),
                                            recording_tx.clone(),
                                            config.clone(),
                                            cancel.clone(),
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // Persist recording lifecycle for crash-recovery audit.
                    if let Some(ref db) = persist_db {
                        let db = db.clone();
                        let de = de.clone();
                        let recordings = state.recordings.clone();
                        tokio::spawn(async move {
                            persist_event(&db, &de, &recordings).await;
                        });
                    }
                }
            }
            Some(action) = pipeline_action_rx.recv() => {
                process_daemon_plugin_actions(
                    vec![action],
                    &registry,
                    &pipeline_runtime,
                    &event_tx,
                );
            }
            _ = cancel.cancelled() => {
                tracing::info!("Daemon shutting down");
                break;
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&pid_path);
    tracing::info!("StriVo daemon exited");
    Ok(())
}

/// Live platform handles and their re-authentication signals, so a client
/// connection can push saved credentials into the running daemon.
///
/// A struct rather than seven more parameters on `handle_client`, and it keeps
/// the reload logic in one testable place instead of inline in the dispatch.
#[derive(Clone)]
struct PlatformControl {
    config_path: Option<std::path::PathBuf>,
    twitch: Option<Arc<RwLock<crate::platform::twitch::TwitchPlatform>>>,
    youtube: Option<Arc<RwLock<crate::platform::youtube::YouTubePlatform>>>,
    patreon: Option<Arc<crate::platform::patreon::PatreonClient>>,
    twitch_reauth: Arc<tokio::sync::Notify>,
    youtube_reauth: Arc<tokio::sync::Notify>,
    patreon_reauth: Arc<tokio::sync::Notify>,
}

impl PlatformControl {
    /// Re-read `config.toml` and push that platform's client id/secret into
    /// the running platform, then wake its auth task.
    ///
    /// The credentials are read from disk rather than carried in the IPC
    /// message so secrets never cross the socket. Returns `false` when the
    /// platform has no live handle — it wasn't configured when the daemon
    /// started, so there is no task to hand them to.
    async fn apply_saved_credentials(&self, kind: PlatformKind) -> Result<bool> {
        let fresh = AppConfig::load(self.config_path.as_deref())?;
        Ok(match kind {
            PlatformKind::Twitch => match (&fresh.twitch, &self.twitch) {
                (Some(cfg), Some(platform)) => {
                    platform
                        .read()
                        .await
                        .set_credentials(cfg.client_id.clone(), cfg.client_secret.clone())
                        .await;
                    self.twitch_reauth.notify_one();
                    true
                }
                _ => false,
            },
            PlatformKind::YouTube => match (&fresh.youtube, &self.youtube) {
                (Some(cfg), Some(platform)) => {
                    platform
                        .read()
                        .await
                        .set_credentials(cfg.client_id.clone(), cfg.client_secret.clone())
                        .await;
                    self.youtube_reauth.notify_one();
                    true
                }
                _ => false,
            },
            PlatformKind::Patreon => match (&fresh.patreon, &self.patreon) {
                (Some(cfg), Some(platform)) => {
                    platform
                        .set_credentials(cfg.client_id.clone(), cfg.client_secret.clone())
                        .await;
                    self.patreon_reauth.notify_one();
                    true
                }
                _ => false,
            },
        })
    }
}

/// Forwards broadcast `DaemonEvent`s onto one connection's bounded write
/// queue (`write_tx`). Bounded (matches `broadcast::channel(256)` at the
/// call site above): an unbounded queue let a stalled reader (dead client,
/// full TCP send buffer — reachable via one open `/events` SSE tab)
/// accumulate every event forever, since the connection's writer task is
/// the only drain (B-05). On a full queue, warns and cancels `conn_cancel`
/// so `handle_client`'s read loop also unwinds and the connection actually
/// closes (clients re-sync via Hello) instead of the queue growing without
/// bound. Split out of `handle_client` so this behavior is unit-testable
/// (see `tests::stalled_write_queue_cancels_the_connection_instead_of_growing`)
/// without standing up a full daemon.
fn spawn_event_forwarder(
    write_tx: mpsc::Sender<String>,
    mut bcast_rx: broadcast::Receiver<DaemonEvent>,
    cancel: CancellationToken,
    conn_cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = bcast_rx.recv() => {
                    match result {
                        Ok(event) => {
                            let msg = ServerMessage::Event(event);
                            if let Ok(encoded) = ipc::encode_message(&msg) {
                                match write_tx.try_send(encoded) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        tracing::warn!(
                                            "client write queue full (256); dropping connection"
                                        );
                                        conn_cancel.cancel();
                                        break;
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            tracing::warn!("Client lagged, they should re-sync via Hello");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = cancel.cancelled() => break,
            }
        }
    })
}

async fn handle_client(
    stream: ipc::Stream,
    snapshot: ServerMessage,
    broadcast_rx: broadcast::Receiver<DaemonEvent>,
    recording_tx: mpsc::UnboundedSender<RecordingCommand>,
    bulk_tx: Option<mpsc::UnboundedSender<crate::recording::bulk::BulkCommand>>,
    config: AppConfig,
    registry: Arc<tokio::sync::Mutex<crate::plugin::registry::PluginRegistry>>,
    pipeline_runtime: crate::pipeline::PipelineRuntime,
    recordings: HashMap<Uuid, RecordingJob>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    poll_notify: Option<Arc<tokio::sync::Notify>>,
    interval_ctl: Option<(Arc<std::sync::atomic::AtomicU64>, Arc<tokio::sync::Notify>)>,
    persist_db: Option<Arc<crate::recording::persist::PersistDb>>,
    platform_control: PlatformControl,
    cancel: CancellationToken,
) -> Result<()> {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Note: the first message is NOT required to be Hello. The TUI opens
    // a long-lived connection with Hello (→ snapshot) and then streams
    // commands; the webui's send_command opens a short-lived connection
    // and writes a single command with no Hello. Both are handled in the
    // read loop below (Hello → snapshot via the writer task), so a
    // command-first connection is dispatched rather than dropped.

    // Spawn a writer task that sends broadcast events. Bounded (matching the
    // broadcast::channel(256) fan-out above): an unbounded queue let a
    // stalled reader (dead client, full TCP send buffer, one per open
    // /events SSE tab) accumulate every DaemonEvent forever, since
    // writer_task is the only drain (B-05).
    let (write_tx, mut write_rx) = mpsc::channel::<String>(256);

    let writer_task = tokio::spawn(async move {
        while let Some(msg) = write_rx.recv().await {
            if writer.write_all(msg.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    // Cancelled when this connection's write queue fills up, so the read
    // loop below (which may otherwise sit blocked on a stalled client's
    // input) also unwinds and the connection actually closes — separate
    // from `cancel`, which is the whole-daemon shutdown token shared by
    // every connection.
    let conn_cancel = CancellationToken::new();
    let broadcast_task = spawn_event_forwarder(
        write_tx.clone(),
        broadcast_rx,
        cancel.clone(),
        conn_cancel.clone(),
    );

    // Read client messages
    loop {
        line.clear();
        // Race the blocking read against this connection's cancellation so
        // a stalled reader whose write queue just filled up (B-05) actually
        // gets its socket closed instead of sitting here until it next
        // sends something (which a stalled client, by definition, may
        // never do).
        let n = tokio::select! {
            r = buf_reader.read_line(&mut line) => r?,
            _ = conn_cancel.cancelled() => break,
        };
        if n == 0 {
            break; // Client disconnected
        }

        let msg: ClientMessage = match serde_json::from_str(line.trim()) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Invalid client message: {e}");
                continue;
            }
        };

        match msg {
            ClientMessage::Hello { version } => {
                if version != ipc::IPC_PROTOCOL_VERSION {
                    tracing::warn!(
                        client_version = version,
                        daemon_version = ipc::IPC_PROTOCOL_VERSION,
                        "IPC protocol version mismatch — client and daemon may be mismatched"
                    );
                }
                // Send the state snapshot through the writer task. (The
                // snapshot is captured at connect time; good enough — the
                // client also receives live events thereafter.)
                if let Ok(encoded) = ipc::encode_message(&snapshot) {
                    let _ = write_tx.try_send(encoded);
                }
            }
            ClientMessage::Recording(cmd) => {
                let _ = recording_tx.send(cmd);
            }
            ClientMessage::PollNow => {
                if let Some(ref notify) = poll_notify {
                    notify.notify_one();
                }
            }
            ClientMessage::SetPollInterval(secs) => {
                // Live-update the monitor's poll cadence (item 14b). The web
                // endpoint already persisted it to config.toml; here we just
                // apply it to the running monitor.
                let secs = secs.max(15);
                if let Some((ref atomic, ref notify)) = interval_ctl {
                    atomic.store(secs, std::sync::atomic::Ordering::Relaxed);
                    notify.notify_one();
                    tracing::info!("Applied live poll interval: {secs}s");
                }
            }
            ClientMessage::ReloadPlatformCredentials { kind } => {
                match platform_control.apply_saved_credentials(kind).await {
                    Ok(true) => tracing::info!(%kind, "applied new platform credentials"),
                    Ok(false) => tracing::warn!(
                        %kind,
                        "credentials saved, but the platform was not configured when the \
                         daemon started — restart to enable it"
                    ),
                    Err(e) => tracing::warn!(%kind, "applying new credentials failed: {e}"),
                }
            }
            ClientMessage::Shutdown => {
                cancel.cancel();
                break;
            }
            ClientMessage::PatreonPull {
                embed_url,
                creator_name,
                post_title,
            } => {
                // Patreon posts are gated; `CookieSource::FromConfig`
                // pulls the patron's cookies path (same cookies used
                // to list them). Path policy is Fresh — the SPA's
                // pull request has no prior live file to live next to.
                let spec = crate::intents::DownloadVodSpec {
                    url: embed_url,
                    channel_name: creator_name,
                    platform: crate::platform::PlatformKind::Patreon,
                    post_title: Some(post_title),
                    cookies: crate::intents::CookieSource::FromConfig,
                    output_policy: crate::intents::OutputPathPolicy::Fresh,
                };
                let _ = recording_tx.send(crate::intents::download_vod(spec, &config));
            }
            ClientMessage::DownloadVod {
                url,
                channel_name,
                platform,
                post_title,
            } => {
                // Routed through `crate::intents::download_vod` so the
                // cookies/output-path policy is shared with the other
                // VOD-pull sites instead of being hand-rolled here.
                let spec = crate::intents::DownloadVodSpec {
                    url,
                    channel_name,
                    platform,
                    post_title,
                    cookies: crate::intents::CookieSource::FromConfig,
                    output_policy: crate::intents::OutputPathPolicy::Fresh,
                };
                let _ = recording_tx.send(crate::intents::download_vod(spec, &config));
            }
            ClientMessage::Start {
                channel_id,
                channel_name,
                display_name,
                platform,
                stream_title,
                thumbnail_url,
                from_start,
                transcode_override,
            } => {
                // Webui-shaped envelope: minimal payload, daemon
                // resolves cookies/transcode against `config`. Fixes
                // the historical "webui-initiated start of a gated
                // YouTube stream silently failed because the route
                // hardcoded `cookies_path: None`."
                let spec = crate::intents::StartSpec {
                    channel_id,
                    channel_name,
                    display_name,
                    platform,
                    stream_title,
                    thumbnail_url,
                    from_start,
                    job_id: None,
                    transcode_override,
                    cookies: crate::intents::CookieSource::FromConfig,
                };
                let _ = recording_tx.send(crate::intents::start_recording(spec, &config));
            }
            ClientMessage::DeleteRecording { job_id } => {
                let Some(db) = persist_db.as_ref() else {
                    tracing::warn!("delete_recording {job_id}: persist db unavailable");
                    continue;
                };
                // Look up the file path. Prefer the live snapshot (fast)
                // and fall back to the journal so already-finished rows
                // still resolve.
                let path = recordings
                    .get(&job_id)
                    .map(|j| j.output_path.clone())
                    .or_else(|| {
                        tracing::debug!("delete_recording {job_id}: not in snapshot, querying db");
                        None
                    });
                let path = if let Some(p) = path {
                    Some(p)
                } else {
                    match db.load_recording_jobs().await {
                        Ok(jobs) => jobs
                            .into_iter()
                            .find(|j| j.id == job_id)
                            .map(|j| j.output_path),
                        Err(e) => {
                            tracing::warn!("delete_recording {job_id}: db lookup failed: {e}");
                            None
                        }
                    }
                };
                if let Some(p) = path {
                    if p.exists() {
                        if let Err(e) = crate::recording::trash::move_to_trash(&p) {
                            tracing::warn!(
                                "delete_recording {job_id}: trash {} failed: {e}",
                                p.display()
                            );
                        }
                    }
                }
                let pruned = match db.delete_recording_job(job_id).await {
                    Ok(n) => {
                        tracing::info!("delete_recording {job_id}: dropped {n} row(s)");
                        true
                    }
                    Err(e) => {
                        tracing::warn!("delete_recording {job_id}: drop row failed: {e}");
                        false
                    }
                };
                // Emit the precise prune event so every subscriber (DaemonState
                // snapshot, TUI app, webui SPA) drops this row from its own
                // in-memory map. AllRecordingsStopped was the wrong wire signal
                // — it doesn't prune anything, just triggers a stale refetch
                // that still surfaces the deleted row from the daemon snapshot.
                if pruned {
                    let _ = event_tx.send(DaemonEvent::RecordingsPruned {
                        job_ids: vec![job_id],
                    });
                }
            }
            ClientMessage::ClearErroredRecordings => {
                let Some(db) = persist_db.as_ref() else {
                    tracing::warn!("clear_errored: persist db unavailable");
                    continue;
                };
                // Source of truth #1 — the persisted journal. Source of truth
                // #2 — the live in-memory snapshot, in case the journal was
                // truncated or a row was lost. We union them so a Failed entry
                // that's only in memory still gets cleared from memory (and
                // attempted from disk, where the delete is a no-op).
                let mut to_prune: HashMap<Uuid, std::path::PathBuf> = HashMap::new();
                match db.load_errored_recording_jobs().await {
                    Ok(jobs) => {
                        for j in jobs {
                            to_prune.insert(j.id, j.output_path);
                        }
                    }
                    Err(e) => tracing::warn!("clear_errored: db load failed: {e}"),
                }
                // `'interrupted'` in the journal maps to `RecordingState::Failed`
                // in-memory (see `persist::map_journal_state`), so a single
                // Failed check covers both errored buckets.
                for (id, job) in &recordings {
                    if matches!(job.state, crate::recording::job::RecordingState::Failed) {
                        to_prune
                            .entry(*id)
                            .or_insert_with(|| job.output_path.clone());
                    }
                }
                let mut pruned_ids = Vec::with_capacity(to_prune.len());
                let mut dropped = 0u64;
                for (id, path) in to_prune {
                    if path.exists() {
                        if let Err(e) = crate::recording::trash::move_to_trash(&path) {
                            tracing::warn!(
                                "clear_errored {id}: trash {} failed: {e}",
                                path.display()
                            );
                        }
                    }
                    match db.delete_recording_job(id).await {
                        Ok(n) => dropped += n,
                        Err(e) => tracing::warn!("clear_errored {id}: drop row failed: {e}"),
                    }
                    pruned_ids.push(id);
                }
                tracing::info!(
                    "clear_errored: pruned {pruned} (jobs.db dropped {dropped})",
                    pruned = pruned_ids.len()
                );
                if !pruned_ids.is_empty() {
                    let _ = event_tx.send(DaemonEvent::RecordingsPruned {
                        job_ids: pruned_ids,
                    });
                }
            }
            ClientMessage::ListPlaylists { channel_id } => {
                if let Some(ref tx) = bulk_tx {
                    let _ =
                        tx.send(crate::recording::bulk::BulkCommand::ListPlaylists { channel_id });
                }
            }
            ClientMessage::FetchChannelVods {
                channel_id,
                platform,
            } => {
                if let Some(ref tx) = bulk_tx {
                    let _ = tx.send(crate::recording::bulk::BulkCommand::FetchVods {
                        channel_id,
                        platform,
                    });
                }
            }
            ClientMessage::ResolveChannel { platform, query } => {
                if let Some(ref tx) = bulk_tx {
                    let _ = tx.send(crate::recording::bulk::BulkCommand::ResolveChannel {
                        platform,
                        query,
                    });
                }
            }
            ClientMessage::BulkDownload {
                channel_id,
                channel_name,
                platform,
                action,
                playlist_id,
            } => {
                let cmd = match action {
                    crate::ipc::BulkAction::Start => crate::recording::bulk::BulkCommand::Start {
                        channel_id,
                        channel_name,
                        platform,
                        playlist_id,
                    },
                    crate::ipc::BulkAction::Stop => {
                        crate::recording::bulk::BulkCommand::Stop { channel_id }
                    }
                };
                if let Some(ref tx) = bulk_tx {
                    let _ = tx.send(cmd);
                }
            }
            ClientMessage::PluginRpc {
                plugin,
                verb,
                selection,
                payload: _,
            } => {
                // W2-phase-3 — actually dispatch the verb. The registry
                // is shared (Arc<Mutex>); on_verb takes the narrow
                // VerbContext so we don't need a full AppState. The
                // returned PluginActions are processed headless: the
                // SpawnTask futures (the real work — transcription,
                // archive pulls) are spawned, and SetStatus/Notify are
                // surfaced as daemon notifications. TUI-only actions
                // (ActivatePane/NavigateBack) are no-ops here.
                let actions = {
                    let mut reg = registry.lock().await;
                    let ctx = crate::plugin::VerbContext {
                        recordings: &recordings,
                        plugin_toggles: &config.plugin_toggles,
                    };
                    reg.dispatch_verb(&plugin, &verb, &selection, &ctx)
                };
                tracing::info!(
                    plugin = %plugin,
                    verb = %verb,
                    action_count = actions.len(),
                    "daemon: dispatched plugin verb"
                );
                process_daemon_plugin_actions(actions, &registry, &pipeline_runtime, &event_tx);
            }
            ClientMessage::SubmitPipeline(pipeline) => {
                if let Err(error) = pipeline_runtime.submit(pipeline).await {
                    let _ =
                        event_tx.send(DaemonEvent::Error(format!("pipeline rejected: {error}")));
                }
            }
            ClientMessage::CancelPipeline { pipeline_id } => {
                pipeline_runtime.cancel(pipeline_id).await;
            }
            ClientMessage::RetryPipelineStage { stage_id } => {
                if !pipeline_runtime.retry(stage_id).await {
                    let _ = event_tx.send(DaemonEvent::Error(format!(
                        "pipeline stage not found: {stage_id}"
                    )));
                }
            }
            ClientMessage::Unknown => {
                // Forward-compatibility: a message variant added in a newer
                // client is silently ignored rather than crashing the daemon.
                tracing::debug!(
                    "received unknown IPC message variant — ignored (protocol forward-compat)"
                );
            }
        }
    }

    broadcast_task.abort();
    writer_task.abort();
    Ok(())
}

/// Process PluginActions returned by a daemon-side verb dispatch (W2-phase-3).
/// Headless: the SpawnTask futures (the real work) are spawned, and their
/// follow-up plugin events are pumped back through the shared registry so a
/// multi-stage verb runs to completion. SetStatus/Notify become daemon
/// notifications (visible to connected TUI/web clients). TUI-only actions
/// (pane activation, mpv playback) are no-ops in the daemon.
fn process_daemon_plugin_actions(
    actions: Vec<crate::plugin::PluginAction>,
    registry: &Arc<tokio::sync::Mutex<crate::plugin::registry::PluginRegistry>>,
    pipeline_runtime: &crate::pipeline::PipelineRuntime,
    event_tx: &mpsc::UnboundedSender<DaemonEvent>,
) {
    use crate::plugin::PluginAction as PA;
    for action in actions {
        match action {
            PA::SetStatus(s) => {
                let _ = event_tx.send(DaemonEvent::Notification {
                    title: "Plugin".to_string(),
                    body: s,
                });
            }
            PA::Notify { title, body } => {
                let _ = event_tx.send(DaemonEvent::Notification { title, body });
            }
            PA::SpawnTask {
                plugin_name,
                future,
            } => {
                let reg = registry.clone();
                let runtime = pipeline_runtime.clone();
                let etx = event_tx.clone();
                tokio::spawn(async move {
                    let result = future.await;
                    let next = {
                        let mut r = reg.lock().await;
                        r.dispatch_plugin_event(plugin_name, result)
                    };
                    // Recurse: the follow-up actions may spawn further
                    // stages (e.g. transcription pipeline steps).
                    process_daemon_plugin_actions(next, &reg, &runtime, &etx);
                });
            }
            PA::SubmitPipeline(pipeline) => {
                let runtime = pipeline_runtime.clone();
                let etx = event_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = runtime.submit(pipeline).await {
                        let _ =
                            etx.send(DaemonEvent::Error(format!("plugin pipeline rejected: {e}")));
                    }
                });
            }
            PA::UpdateStage {
                stage_id,
                new_state,
            } => {
                let runtime = pipeline_runtime.clone();
                tokio::spawn(async move {
                    runtime.update_stage(stage_id, new_state).await;
                });
            }
            PA::UpdateConfig { plugin_name, .. } => {
                let _ = event_tx.send(DaemonEvent::Error(format!(
                    "plugin {plugin_name} requested a configuration update that the daemon cannot apply"
                )));
            }
            // No daemon equivalent (no TUI panes / mpv / config persistence
            // path here); the TUI handles these when it dispatches verbs.
            _ => {}
        }
    }
}

/// Fire a desktop banner for notification-worthy daemon events, honouring the
/// per-event [`NotificationsConfig`](crate::config::NotificationsConfig)
/// flags. Native notification commands can block on the desktop session, so
/// they run on a blocking thread; missing commands and headless-session
/// failures are intentionally swallowed.
fn dispatch_desktop_notification(cfg: &crate::config::NotificationsConfig, event: &DaemonEvent) {
    if !cfg.desktop_enabled {
        return;
    }
    use crate::recording::job::RecordingState;
    let (summary, body) = match event {
        DaemonEvent::ChannelWentLive(ch) if cfg.on_go_live => (
            "StriVo — channel live".to_string(),
            format!("{} is now live", ch.display_name),
        ),
        DaemonEvent::RecordingFinished {
            final_state, error, ..
        } => {
            let failed = error.is_some() || matches!(final_state, RecordingState::Failed);
            if failed {
                if !cfg.on_recording_failed {
                    return;
                }
                (
                    "StriVo — recording failed".to_string(),
                    error
                        .clone()
                        .unwrap_or_else(|| "recording ended in error".to_string()),
                )
            } else if matches!(final_state, RecordingState::Finished) {
                if !cfg.on_recording_finished {
                    return;
                }
                (
                    "StriVo — recording finished".to_string(),
                    "A recording completed successfully.".to_string(),
                )
            } else {
                return;
            }
        }
        // Generic notifications (bulk pulls, Patreon, schedule) carry their
        // own copy; `on_vod_ready` and friends gate at the producer.
        DaemonEvent::Notification { title, body } => (title.clone(), body.clone()),
        _ => return,
    };
    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("notify-send")
            .args(["--app-name", "StriVo", &summary, &body])
            .status();

        #[cfg(target_os = "macos")]
        {
            let escape = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                escape(&body),
                escape(&summary)
            );
            let _ = std::process::Command::new("osascript")
                .args(["-e", &script])
                .status();
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = (summary, body);
    });
}

/// Persist a recording's lifecycle for crash-recovery. Best-effort — a sqlite
/// hiccup never breaks the live event flow.
async fn persist_event(
    db: &crate::recording::persist::PersistDb,
    event: &DaemonEvent,
    recordings: &HashMap<Uuid, RecordingJob>,
) {
    use crate::recording::job::RecordingState;
    use crate::recording::persist::PersistedJob;

    // Encode a RecordingJob to JSON for the journal. Serialization is not
    // expected to fail for our types, but if it does we record a structured
    // error marker so the persisted row remains diagnostically useful rather
    // than silently empty.
    fn encode_job(job: &RecordingJob) -> String {
        match serde_json::to_string(job) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(job_id = %job.id, "daemon: failed to serialize recording job for journal: {e}");
                format!(
                    r#"{{"_serialize_error":"{}","job_id":"{}"}}"#,
                    e.to_string().replace('"', "'"),
                    job.id
                )
            }
        }
    }

    let snapshot =
        |job_id: &Uuid, state: RecordingState, error: Option<String>| -> Option<PersistedJob> {
            let job = recordings.get(job_id)?;
            Some(PersistedJob {
                id: job.id.to_string(),
                kind: "Recording".to_string(),
                payload: encode_job(job),
                state: format!("{state:?}").to_lowercase(),
                attempts: 0,
                last_error: error,
                episode_dir: job.output_path.parent().map(|p| p.to_path_buf()),
            })
        };

    let result = match event {
        DaemonEvent::RecordingStarted { job } => {
            let pj = PersistedJob {
                id: job.id.to_string(),
                kind: "Recording".to_string(),
                payload: encode_job(job),
                state: "running".to_string(),
                attempts: 0,
                last_error: None,
                episode_dir: job.output_path.parent().map(|p| p.to_path_buf()),
            };
            db.upsert_job(&pj).await
        }
        DaemonEvent::RecordingFinished {
            job_id,
            final_state,
            error,
            // The path fix is already applied to the in-memory job by the
            // state handler above, and `snapshot` serialises that job.
            new_path: _,
        } => {
            let Some(pj) = snapshot(job_id, *final_state, error.clone()) else {
                return;
            };
            db.upsert_job(&pj).await
        }
        _ => return,
    };
    if let Err(e) = result {
        tracing::warn!("daemon: persist event failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PlatformKind;
    use crate::recording::job::{RecordingJob, RecordingState};

    /// B-05 — a stalled reader (never drains `write_rx`, standing in for a
    /// dead client / full TCP send buffer) must not let the write queue
    /// grow past its bound. Drives the real `spawn_event_forwarder` used by
    /// `handle_client`, with real bounded/broadcast channels — no daemon
    /// stood up, since the behavior under test lives entirely in the queue
    /// wiring, not in IPC framing or the rest of handle_client's command
    /// dispatch.
    #[tokio::test]
    async fn stalled_write_queue_cancels_the_connection_instead_of_growing() {
        let (write_tx, write_rx) = mpsc::channel::<String>(256);
        let (bcast_tx, bcast_rx) = broadcast::channel::<DaemonEvent>(512);
        let cancel = CancellationToken::new();
        let conn_cancel = CancellationToken::new();

        let forwarder =
            spawn_event_forwarder(write_tx, bcast_rx, cancel.clone(), conn_cancel.clone());

        // Never drain write_rx — the stalled reader this connection's queue
        // is supposed to survive without growing unboundedly.
        for i in 0..300u32 {
            bcast_tx
                .send(DaemonEvent::Notification {
                    title: format!("t{i}"),
                    body: String::new(),
                })
                .unwrap();
        }

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !conn_cancel.is_cancelled() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect(
            "conn_cancel must fire once the 256-capacity queue fills; \
             it must never grow past that bound",
        );

        forwarder.abort();
        drop(write_rx);
        drop(cancel);
    }

    /// Write a config.toml carrying one platform's credentials and return its
    /// path, so the reload path can be driven through the real
    /// `AppConfig::load` rather than a hand-built struct.
    fn config_with_credentials(
        dir: &std::path::Path,
        section: &str,
        client_id: &str,
        client_secret: &str,
    ) -> std::path::PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            format!(
                "recording_dir = \"{}\"\n\n[{section}]\nclient_id = \"{client_id}\"\nclient_secret = \"{client_secret}\"\n",
                dir.join("recordings").display()
            ),
        )
        .unwrap();
        path
    }

    fn control_for(config_path: std::path::PathBuf) -> PlatformControl {
        PlatformControl {
            config_path: Some(config_path),
            twitch: None,
            youtube: None,
            patreon: None,
            twitch_reauth: Arc::new(tokio::sync::Notify::new()),
            youtube_reauth: Arc::new(tokio::sync::Notify::new()),
            patreon_reauth: Arc::new(tokio::sync::Notify::new()),
        }
    }

    #[tokio::test]
    async fn reload_replaces_twitch_credentials_and_wakes_the_auth_task() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = config_with_credentials(dir.path(), "twitch", "new-id", "new-secret");

        let twitch = Arc::new(RwLock::new(crate::platform::twitch::TwitchPlatform::new(
            "stale-id".into(),
            "stale-secret".into(),
        )));
        let mut control = control_for(path);
        control.twitch = Some(twitch.clone());

        // The auth task waits on this; the reload has to wake it, otherwise
        // the new credentials sit unused until the next hourly check.
        let reauth = control.twitch_reauth.clone();
        let woken = tokio::spawn(async move { reauth.notified().await });
        tokio::task::yield_now().await;

        assert!(control
            .apply_saved_credentials(PlatformKind::Twitch)
            .await
            .unwrap());

        let creds = twitch.read().await.creds().await;
        assert_eq!(creds.client_id, "new-id");
        assert_eq!(creds.client_secret, "new-secret");
        tokio::time::timeout(std::time::Duration::from_secs(5), woken)
            .await
            .expect("reload must wake the auth task")
            .unwrap();
    }

    #[tokio::test]
    async fn reload_replaces_youtube_credentials() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = config_with_credentials(dir.path(), "youtube", "yt-id", "yt-secret");

        let youtube = Arc::new(RwLock::new(crate::platform::youtube::YouTubePlatform::new(
            "stale-id".into(),
            "stale-secret".into(),
            None,
        )));
        let mut control = control_for(path);
        control.youtube = Some(youtube.clone());

        assert!(control
            .apply_saved_credentials(PlatformKind::YouTube)
            .await
            .unwrap());
        assert_eq!(youtube.read().await.creds().await.client_id, "yt-id");
    }

    #[tokio::test]
    async fn reload_replaces_patreon_credentials() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = config_with_credentials(dir.path(), "patreon", "pt-id", "pt-secret");

        let patreon = Arc::new(crate::platform::patreon::PatreonClient::new(
            "stale-id".into(),
            "stale-secret".into(),
        ));
        let mut control = control_for(path);
        control.patreon = Some(patreon.clone());

        assert!(control
            .apply_saved_credentials(PlatformKind::Patreon)
            .await
            .unwrap());
        assert_eq!(patreon.creds().await.client_id, "pt-id");
    }

    #[tokio::test]
    async fn reload_reports_a_platform_the_daemon_never_started() {
        // Credentials for a platform that was absent from config at boot: it
        // has no live handle, so the caller must be told a restart is needed
        // rather than being left to assume it took effect.
        let dir = tempfile::TempDir::new().unwrap();
        let path = config_with_credentials(dir.path(), "twitch", "new-id", "new-secret");
        let control = control_for(path);

        assert!(!control
            .apply_saved_credentials(PlatformKind::Twitch)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn reload_leaves_credentials_alone_when_the_config_is_unreadable() {
        // `AppConfig::load` falls back to defaults on a malformed file rather
        // than failing, so the reload sees a config with no `[twitch]` at all.
        // It must report "nothing applied" and leave the running credentials
        // intact — overwriting them with the fallback's emptiness would take
        // down a working platform on a stray edit to config.toml.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not = valid toml [[[").unwrap();

        let twitch = Arc::new(RwLock::new(crate::platform::twitch::TwitchPlatform::new(
            "live-id".into(),
            "live-secret".into(),
        )));
        let mut control = control_for(path);
        control.twitch = Some(twitch.clone());

        assert!(!control
            .apply_saved_credentials(PlatformKind::Twitch)
            .await
            .unwrap());
        let creds = twitch.read().await.creds().await;
        assert_eq!(creds.client_id, "live-id");
        assert_eq!(creds.client_secret, "live-secret");
    }

    fn empty_state() -> DaemonState {
        DaemonState {
            channels: Vec::new(),
            recordings: HashMap::new(),
            twitch_connected: false,
            youtube_connected: false,
            patreon_connected: false,
            pending_auth: None,
            auth_queue: std::collections::VecDeque::new(),
            patreon_creators: Vec::new(),
            patreon_posts: Vec::new(),
            auth_issues: Vec::new(),
        }
    }

    #[test]
    fn cookie_issue_survives_a_retry_spawn_and_clears_once_bytes_flow() {
        let mut state = empty_state();
        let mut j = job(RecordingState::Recording, 0);
        j.platform = PlatformKind::YouTube;
        let progress = |bytes: u64| DaemonEvent::RecordingProgress {
            job_id: j.id,
            bytes_written: bytes,
            duration_secs: 0.0,
            download_pct: None,
            download_eta_secs: None,
            download_rate_bps: None,
        };

        state.apply(&DaemonEvent::CookieSessionRejected {
            kind: PlatformKind::YouTube,
            reason: "cookies are no longer valid".into(),
        });
        let since = state.auth_issues[0].since;

        // A retry spawning is not evidence the jar works.
        state.apply(&DaemonEvent::RecordingStarted { job: j.clone() });
        assert_eq!(state.auth_issues.len(), 1, "spawn must not clear the issue");

        // The retry failing again updates the reason but keeps the age.
        state.apply(&DaemonEvent::CookieSessionRejected {
            kind: PlatformKind::YouTube,
            reason: "sign in to confirm you're not a bot".into(),
        });
        assert_eq!(state.auth_issues.len(), 1);
        assert_eq!(
            state.auth_issues[0].since, since,
            "since must survive an upsert"
        );
        assert_eq!(
            state.auth_issues[0].reason,
            "sign in to confirm you're not a bot"
        );

        // Zero bytes is still not evidence.
        state.apply(&progress(0));
        assert_eq!(state.auth_issues.len(), 1);

        // Data on disk is.
        state.apply(&progress(4096));
        assert!(
            state.auth_issues.is_empty(),
            "bytes written must clear the cookies issue"
        );
    }

    fn job(state: RecordingState, age_secs: i64) -> RecordingJob {
        let mut j = RecordingJob::new(
            "ch".into(),
            "Chan".into(),
            PlatformKind::Twitch,
            std::path::PathBuf::from("/tmp/x.mkv"),
            false,
            None,
        );
        j.state = state;
        j.started_at = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        j
    }

    /// Finalisation can rename a capture so its extension matches the
    /// container actually written. If the daemon ignored that, the journal
    /// would keep the old name and the library would show a file that is not
    /// there — which is exactly the failure this plumbing exists to prevent.
    #[test]
    fn finishing_adopts_a_corrected_output_path() {
        let mut st = empty_state();
        let mut j = job(RecordingState::Recording, 1);
        j.output_path = std::path::PathBuf::from("/tmp/capture.mkv");
        let id = j.id;
        st.recordings.insert(id, j);

        st.apply(&DaemonEvent::RecordingFinished {
            job_id: id,
            final_state: RecordingState::Finished,
            error: None,
            new_path: Some(std::path::PathBuf::from("/tmp/capture.mp4")),
        });

        let got = st.recordings.get(&id).expect("job survives finishing");
        assert_eq!(got.state, RecordingState::Finished);
        assert_eq!(
            got.output_path,
            std::path::PathBuf::from("/tmp/capture.mp4")
        );
    }

    /// The common case: nothing was renamed, so the path must not move.
    #[test]
    fn finishing_without_a_rename_leaves_the_path_alone() {
        let mut st = empty_state();
        let mut j = job(RecordingState::Recording, 1);
        j.output_path = std::path::PathBuf::from("/tmp/capture.mkv");
        let id = j.id;
        st.recordings.insert(id, j);

        st.apply(&DaemonEvent::RecordingFinished {
            job_id: id,
            final_state: RecordingState::Finished,
            error: None,
            new_path: None,
        });

        let got = st.recordings.get(&id).unwrap();
        assert_eq!(
            got.output_path,
            std::path::PathBuf::from("/tmp/capture.mkv")
        );
    }

    #[test]
    fn evict_caps_terminal_keeps_active() {
        let mut st = empty_state();
        // One active job (must survive) plus a terminal job older than any
        // we add below, to confirm the oldest terminal is the one dropped.
        let active = job(RecordingState::Recording, 1);
        let active_id = active.id;
        st.recordings.insert(active_id, active);
        let oldest = job(RecordingState::Finished, 1_000_000);
        let oldest_id = oldest.id;
        st.recordings.insert(oldest_id, oldest);

        // Push terminal jobs well past the cap.
        for i in 0..MAX_TERMINAL_RECORDINGS + 50 {
            let j = job(RecordingState::Finished, i as i64);
            st.recordings.insert(j.id, j);
        }

        st.evict_old_terminal();

        let terminal = st
            .recordings
            .values()
            .filter(|j| matches!(j.state, RecordingState::Finished | RecordingState::Failed))
            .count();
        assert_eq!(terminal, MAX_TERMINAL_RECORDINGS, "terminal tail capped");
        assert!(st.recordings.contains_key(&active_id), "active job kept");
        assert!(
            !st.recordings.contains_key(&oldest_id),
            "oldest terminal dropped"
        );
    }

    #[test]
    fn evict_noop_under_cap() {
        let mut st = empty_state();
        for i in 0..10 {
            let j = job(RecordingState::Finished, i);
            st.recordings.insert(j.id, j);
        }
        st.evict_old_terminal();
        assert_eq!(st.recordings.len(), 10);
    }
}
