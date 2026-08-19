pub mod patreon;

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::events::DaemonEvent;
use crate::platform::{ChannelEntry, Platform, PlatformKind};
use crate::recording::RecordingCommand;

pub struct ChannelMonitor {
    platforms: Vec<Arc<RwLock<dyn Platform>>>,
    config: AppConfig,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    recording_tx: mpsc::UnboundedSender<RecordingCommand>,
    cancel: CancellationToken,
    /// Track which channels were previously live for went-live/went-offline detection
    prev_live: HashMap<String, bool>,
    /// Auto-record channels we've already triggered for (avoid duplicate starts)
    auto_recorded: HashMap<String, bool>,
    /// Last successfully-fetched channel list per platform, so a transient
    /// fetch failure (e.g. YouTube 403 quotaExceeded) retains the prior set
    /// instead of blanking the whole platform from the rail.
    last_channels: HashMap<PlatformKind, Vec<ChannelEntry>>,
    /// Notified when a platform authenticates (triggers immediate first poll)
    auth_notify: Arc<tokio::sync::Notify>,
    /// Notified when a client requests an immediate re-poll
    poll_notify: Arc<tokio::sync::Notify>,
    /// Live channel-poll interval in seconds (item 14b) — updated by
    /// `SetPollInterval` and read when the monitor (re)builds its timer.
    poll_interval_secs: Arc<std::sync::atomic::AtomicU64>,
    /// Notified when `poll_interval_secs` changes so the loop rebuilds its timer.
    interval_notify: Arc<tokio::sync::Notify>,
    /// Read-only persistence handle for capture-profile cutoff checks
    /// (roadmap item 21). `None` when the daemon has no DB.
    persist: Option<Arc<crate::recording::persist::PersistDb>>,
    /// Last time each channel was observed live (persisted), for the
    /// "last live: N ago" label on offline rows.
    last_live: HashMap<String, chrono::DateTime<chrono::Utc>>,
    /// Path backing `last_live`.
    last_live_path: PathBuf,
}

impl ChannelMonitor {
    pub fn new(
        platforms: Vec<Arc<RwLock<dyn Platform>>>,
        config: AppConfig,
        event_tx: mpsc::UnboundedSender<DaemonEvent>,
        recording_tx: mpsc::UnboundedSender<RecordingCommand>,
        cancel: CancellationToken,
    ) -> Self {
        let interval_secs = config.poll_interval_secs;
        let last_live_path = AppConfig::state_dir().join("last_live.json");
        let last_live = std::fs::read_to_string(&last_live_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            platforms,
            config,
            event_tx,
            recording_tx,
            cancel,
            prev_live: HashMap::new(),
            auto_recorded: HashMap::new(),
            last_channels: HashMap::new(),
            auth_notify: Arc::new(tokio::sync::Notify::new()),
            poll_notify: Arc::new(tokio::sync::Notify::new()),
            poll_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(interval_secs)),
            interval_notify: Arc::new(tokio::sync::Notify::new()),
            persist: None,
            last_live,
            last_live_path,
        }
    }

    /// Handles to live-update the poll interval (item 14b): the daemon stores
    /// the new value in the atomic and fires the notify to rebuild the timer.
    pub fn interval_controls(
        &self,
    ) -> (Arc<std::sync::atomic::AtomicU64>, Arc<tokio::sync::Notify>) {
        (
            self.poll_interval_secs.clone(),
            self.interval_notify.clone(),
        )
    }

    /// Set an external auth notify (shared with auth tasks)
    pub fn set_auth_notify(&mut self, notify: Arc<tokio::sync::Notify>) {
        self.auth_notify = notify;
    }

    /// Provide a read-only persistence handle for capture-profile cutoffs.
    pub fn set_persist(&mut self, db: Arc<crate::recording::persist::PersistDb>) {
        self.persist = Some(db);
    }

    /// Get a handle to trigger an immediate re-poll
    pub fn poll_notify(&self) -> Arc<tokio::sync::Notify> {
        self.poll_notify.clone()
    }

    /// Seed `last_live` from the recording-jobs DB. A recording's `started_at`
    /// is a hard "the channel was live then" timestamp, so this fills in
    /// channels we've recorded but never observed transition through
    /// `is_live` during a daemon lifetime (YouTube's RSS-based live detection
    /// frequently misses the window between live and offline, leaving an
    /// otherwise-recorded channel with no rail "last live" label).
    async fn backfill_last_live_from_persist(&mut self) {
        let Some(db) = self.persist.clone() else {
            return;
        };
        let jobs = match db.load_recording_jobs().await {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("monitor: last_live backfill failed: {e}");
                return;
            }
        };
        let mut wrote = false;
        for job in jobs {
            let cur = self.last_live.get(&job.channel_id).copied();
            if cur.map_or(true, |t| job.started_at > t) {
                self.last_live.insert(job.channel_id, job.started_at);
                wrote = true;
            }
        }
        if wrote {
            if let Ok(json) = serde_json::to_string(&self.last_live) {
                let _ = std::fs::write(&self.last_live_path, json);
            }
            tracing::info!(
                count = self.last_live.len(),
                "monitor: seeded last_live from recording history"
            );
        }
    }

    pub async fn run(mut self) {
        // Seed last_live from the recordings DB before the first poll so the
        // rail "last live" labels are populated even for channels whose live
        // edge this daemon never observed directly.
        self.backfill_last_live_from_persist().await;

        // Wait for first platform auth or timeout before initial poll.
        // If the timeout fires before any platform has authenticated we
        // wait again — emitting an unauthenticated poll just produces a
        // user-visible error and burns API budget for no signal.
        loop {
            tokio::select! {
                _ = self.auth_notify.notified() => {
                    tracing::info!("Platform authenticated, starting initial poll");
                    break;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
                    if self.any_platform_authenticated().await {
                        tracing::info!("Timeout fired with credentials present; polling");
                        break;
                    }
                    tracing::warn!(
                        "No platform authenticated in 10s; waiting for auth_notify before first poll"
                    );
                    // loop back into the select; auth_notify is the only path
                    // that can wake us now (plus cancel).
                }
                _ = self.cancel.cancelled() => {
                    tracing::info!("Monitor shutting down before first poll");
                    return;
                }
            }
        }

        // Let any remaining platforms finish authenticating so the first
        // snapshot is complete rather than arriving in two visible steps.
        self.settle_before_first_poll(std::time::Duration::from_secs(8))
            .await;

        // Immediate first poll
        if let Err(e) = self.poll_all().await {
            tracing::error!("Initial poll error: {e}");
            let _ = self
                .event_tx
                .send(DaemonEvent::Error(format!("Poll error: {e}")));
        }

        let interval_atomic = self.poll_interval_secs.clone();
        let cur_secs = || {
            interval_atomic
                .load(std::sync::atomic::Ordering::Relaxed)
                .max(15)
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(cur_secs()));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the first tick (it fires immediately)
        interval.tick().await;

        loop {
            tokio::select! {
                _ = self.interval_notify.notified() => {
                    // poll_interval changed live (item 14b) — rebuild the timer.
                    let secs = cur_secs();
                    tracing::info!("Poll interval updated to {secs}s");
                    interval = tokio::time::interval(std::time::Duration::from_secs(secs));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    interval.tick().await;
                }
                _ = interval.tick() => {
                    if let Err(e) = self.poll_all().await {
                        tracing::error!("Monitor poll error: {e}");
                        let _ = self.event_tx.send(DaemonEvent::Error(format!("Poll error: {e}")));
                    }
                }
                _ = self.poll_notify.notified() => {
                    tracing::info!("On-demand re-poll triggered");
                    if let Err(e) = self.poll_all().await {
                        tracing::error!("Monitor poll error: {e}");
                        let _ = self.event_tx.send(DaemonEvent::Error(format!("Poll error: {e}")));
                    }
                    interval.reset();
                }
                _ = self.auth_notify.notified() => {
                    // A platform finished authenticating after the first
                    // poll. Re-poll immediately so the new platform's
                    // channels appear without waiting for the next 60s
                    // tick — the original cause of "Twitch missing from
                    // the sidebar for the first minute" symptom.
                    tracing::info!("Platform auth event, re-polling");
                    if let Err(e) = self.poll_all().await {
                        tracing::error!("Monitor poll error: {e}");
                        let _ = self.event_tx.send(DaemonEvent::Error(format!("Poll error: {e}")));
                    }
                    interval.reset();
                }
                _ = self.cancel.cancelled() => {
                    tracing::info!("Monitor shutting down");
                    break;
                }
            }
        }
    }

    /// Are all configured platforms ready to serve requests?
    async fn all_platforms_authenticated(&self) -> bool {
        for platform in &self.platforms {
            let plat = platform.read().await;
            if !plat.is_authenticated().await {
                return false;
            }
        }
        true
    }

    /// Give the stragglers a moment before the very first poll.
    ///
    /// Platforms authenticate at different speeds — Twitch has to resolve a
    /// user id after loading its token — and the monitor wakes as soon as the
    /// FIRST one is ready. Polling right then produces a first snapshot that
    /// is missing whole platforms, so the channel list visibly jumps a
    /// moment later. A short bounded wait costs nothing on a healthy start
    /// and avoids that flicker; if a platform is genuinely broken we stop
    /// waiting and poll with whoever is ready.
    async fn settle_before_first_poll(&self, budget: std::time::Duration) {
        let deadline = std::time::Instant::now() + budget;
        while std::time::Instant::now() < deadline {
            if self.all_platforms_authenticated().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        tracing::debug!(
            "monitor: not every platform authenticated within {:?}; polling with those that are",
            budget
        );
    }

    async fn any_platform_authenticated(&self) -> bool {
        for platform in &self.platforms {
            let plat = platform.read().await;
            if plat.is_authenticated().await {
                return true;
            }
        }
        false
    }

    async fn poll_all(&mut self) -> Result<()> {
        let poll_started = std::time::Instant::now();
        // Phase 1 — fan out per-platform channel + live-status fetches
        // concurrently. Previously these ran in series: a slow YouTube call
        // (quota / SSL handshake) blocked Twitch live-detection for the full
        // duration. Now each platform's two network round-trips run as
        // independent async tasks; the loop below waits for all of them at
        // once via `join_all`.
        //
        // State mutations (prev_live, auto_recorded, last_live) happen in
        // Phase 2 so they remain single-threaded.
        type PlatformFetch = (
            PlatformKind,
            Result<Vec<ChannelEntry>>,         // fetch_followed_channels
            Option<Result<Vec<ChannelEntry>>>, // check_live_status (None on fetch failure)
        );

        let futs: Vec<_> = self
            .platforms
            .iter()
            .map(|platform| {
                let platform = platform.clone();
                async move {
                    let (kind, channels_result) = {
                        let plat = platform.read().await;
                        let kind = plat.kind();
                        // A platform that has not finished authenticating yet
                        // is skipped rather than called. The monitor starts
                        // its first poll as soon as ANY platform authenticates,
                        // and holding an access token is not the same as being
                        // able to serve requests — Twitch resolves its user id
                        // a moment later. Calling anyway produced a warning on
                        // every startup that resolved itself one cycle later,
                        // which trains people to ignore warnings.
                        if !plat.is_authenticated().await {
                            tracing::debug!("{kind}: not authenticated yet; skipping this poll");
                            return (kind, Ok(Vec::new()), None);
                        }
                        let result = plat.fetch_followed_channels().await;
                        (kind, result)
                    };
                    let live_result: Option<Result<Vec<ChannelEntry>>> = match &channels_result {
                        Ok(channels) => {
                            let ids: Vec<String> = channels.iter().map(|c| c.id.clone()).collect();
                            let plat = platform.read().await;
                            Some(plat.check_live_status(&ids).await)
                        }
                        Err(_) => None,
                    };
                    (kind, channels_result, live_result)
                }
            })
            .collect();

        let platform_results: Vec<PlatformFetch> = futures_util::future::join_all(futs).await;

        // Phase 2 — process each platform's results sequentially.
        // Ordering is stable (collect() preserves insertion order) so the
        // emitted ChannelsUpdated list is deterministic across polls.
        let mut all_channels: Vec<ChannelEntry> = Vec::new();

        for (kind, channels_result, live_result) in platform_results {
            match channels_result {
                Ok(mut channels) => {
                    match live_result {
                        Some(Ok(live_channels)) => {
                            let live_map: HashMap<String, ChannelEntry> = live_channels
                                .into_iter()
                                .map(|c| (c.id.clone(), c))
                                .collect();

                            for ch in &mut channels {
                                if let Some(live) = live_map.get(&ch.id) {
                                    ch.is_live = true;
                                    ch.stream_title = live.stream_title.clone();
                                    ch.game_or_category = live.game_or_category.clone();
                                    ch.viewer_count = live.viewer_count;
                                    ch.started_at = live.started_at;
                                    ch.thumbnail_url = live.thumbnail_url.clone();
                                    // Only live detection knows which video is
                                    // airing; the followed-channel list never
                                    // carries it. Dropping it here would leave
                                    // the web UI unable to drive a YouTube tile
                                    // through the player API.
                                    ch.live_video_id = live.live_video_id.clone();
                                }
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!("{kind}: live status check failed: {e}");
                        }
                        None => {}
                    }

                    for ch in &mut channels {
                        // Check auto-record from the channel data directly
                        // (reflects fresh config state from TUI saves)
                        ch.auto_record = self
                            .config
                            .auto_record_channels
                            .iter()
                            .any(|a| a.channel_id == ch.id && a.platform == kind.to_string());

                        // Track/stamp last-seen-live for the offline
                        // "last live: N ago" label.
                        if ch.is_live {
                            self.last_live.insert(ch.id.clone(), chrono::Utc::now());
                        }
                        ch.last_live_at = self.last_live.get(&ch.id).copied();
                    }

                    // Detect went-live / went-offline transitions
                    for ch in &channels {
                        let was_live = self.prev_live.get(&ch.id).copied().unwrap_or(false);
                        if ch.is_live && !was_live {
                            let _ = self.event_tx.send(DaemonEvent::ChannelWentLive(ch.clone()));

                            // Auto-record trigger: use ch.auto_record from fresh data
                            if ch.auto_record
                                && !self.auto_recorded.get(&ch.id).copied().unwrap_or(false)
                                && !self.cutoff_reached(ch).await
                                && !self.max_concurrent_reached().await
                                && !self.disk_budget_exhausted().await
                            {
                                self.auto_recorded.insert(ch.id.clone(), true);
                                // Cookies + transcode policy resolved
                                // inside `intents::start_recording` via
                                // `FromConfig` + `effective_transcode`.
                                let spec = crate::intents::StartSpec {
                                    channel_id: ch.id.clone(),
                                    channel_name: ch.name.clone(),
                                    display_name: Some(ch.display_name.clone()),
                                    platform: ch.platform,
                                    stream_title: ch.stream_title.clone(),
                                    thumbnail_url: ch.thumbnail_url.clone(),
                                    from_start: true,
                                    job_id: None,
                                    transcode_override: None,
                                    cookies: crate::intents::CookieSource::FromConfig,
                                };
                                let _ = self
                                    .recording_tx
                                    .send(crate::intents::start_recording(spec, &self.config));
                            }
                        } else if !ch.is_live && was_live {
                            let _ = self
                                .event_tx
                                .send(DaemonEvent::ChannelWentOffline(ch.clone()));
                            self.auto_recorded.remove(&ch.id);
                        }
                        self.prev_live.insert(ch.id.clone(), ch.is_live);
                    }

                    self.last_channels.insert(kind, channels.clone());
                    all_channels.extend(channels);
                }
                Err(e) => {
                    // Retain the last-known channels for this platform so a
                    // transient failure (e.g. YouTube 403 quotaExceeded) doesn't
                    // blank it from the rail. Live status will be stale until the
                    // next successful poll.
                    match self.last_channels.get(&kind) {
                        Some(cached) if !cached.is_empty() => {
                            tracing::warn!(
                                "{kind}: fetch channels failed ({e}); showing {} cached channels",
                                cached.len()
                            );
                            all_channels.extend(cached.iter().cloned());
                        }
                        _ => tracing::warn!("{kind}: fetch channels failed: {e}"),
                    }
                }
            }
        }

        // Sort: live first, then alphabetical
        all_channels.sort_by(|a, b| {
            a.platform
                .to_string()
                .cmp(&b.platform.to_string())
                .then(b.is_live.cmp(&a.is_live))
                .then(
                    a.display_name
                        .to_lowercase()
                        .cmp(&b.display_name.to_lowercase()),
                )
        });

        let _ = self
            .event_tx
            .send(DaemonEvent::ChannelsUpdated(all_channels));
        tracing::info!(
            channel_count = self.prev_live.len(),
            duration_ms = poll_started.elapsed().as_secs_f64() * 1000.0,
            "platform poll completed"
        );

        // Persist last-seen-live so the "last live: N ago" label survives
        // restarts. Best-effort.
        if let Ok(json) = serde_json::to_string(&self.last_live) {
            let _ = std::fs::write(&self.last_live_path, json);
        }

        Ok(())
    }

    /// True if the channel's capture profile has a `cutoff_episodes` and at
    /// least that many finished recordings already exist (roadmap item 21).
    /// Best-effort: a DB error or missing handle never blocks recording.
    async fn cutoff_reached(&self, ch: &ChannelEntry) -> bool {
        let Some(profile) = self
            .config
            .capture_profile_for(&ch.platform.to_string(), &ch.id)
        else {
            return false;
        };
        let Some(cutoff) = profile.cutoff_episodes else {
            return false;
        };
        let Some(db) = &self.persist else {
            return false;
        };
        match db.count_finished_recordings(&ch.id).await {
            Ok(n) if (n as u32) >= cutoff => {
                tracing::info!(
                    "auto-record skipped for {}: profile '{}' cutoff {} reached ({} recorded)",
                    ch.name,
                    profile.name,
                    cutoff,
                    n
                );
                true
            }
            _ => false,
        }
    }

    /// Honour monitor_limits.max_concurrent_recordings. Returns true
    /// (= skip this capture) when the count of jobs currently in the
    /// Recording state has reached the configured ceiling. A ceiling
    /// of 0 (the default) disables the gate so existing setups don't
    /// silently regress.
    async fn max_concurrent_reached(&self) -> bool {
        let cap = self.config.monitor_limits.max_concurrent_recordings;
        if cap == 0 {
            return false;
        }
        let Some(db) = &self.persist else {
            return false;
        };
        match db.load_jobs_in_states(&["Recording"]).await {
            Ok(active) if (active.len() as u32) >= cap => {
                tracing::info!(
                    "auto-record gated: max_concurrent_recordings={cap} already in flight ({} active)",
                    active.len()
                );
                true
            }
            _ => false,
        }
    }

    /// Honour monitor_limits.disk_budget_reserved_gb — defer new
    /// captures when the free space on the recording dir would drop
    /// below the reserved buffer. 0 disables the gate. The check is
    /// best-effort (statvfs); a stat failure falls through to "allow"
    /// so a transient FS hiccup doesn't lose captures.
    async fn disk_budget_exhausted(&self) -> bool {
        let reserved_gb = self.config.monitor_limits.disk_budget_reserved_gb;
        if reserved_gb == 0 {
            return false;
        }
        let dir = self.config.recording_dir.as_path();
        let free_bytes = match free_space_bytes(dir) {
            Some(b) => b,
            None => return false,
        };
        let reserved_bytes = (reserved_gb as u64) * 1_000_000_000;
        if free_bytes < reserved_bytes {
            tracing::info!(
                "auto-record gated: disk_budget_reserved_gb={reserved_gb} would be crossed (free={} bytes)",
                free_bytes
            );
            true
        } else {
            false
        }
    }

    // `get_cookies_path` retired: the live-record dispatch now routes
    // through `crate::intents::start_recording` with
    // `CookieSource::FromConfig`, which centralises the per-platform
    // policy. The reload-on-every-fire pattern was specific to the old
    // hand-rolled lookup; the monitor's `self.config` snapshot is
    // refreshed by the daemon's config-reload path, so the read-through
    // is no longer needed.
}

/// Free-space lookup for the disk-budget enforcement gate. statvfs on
/// Unix; GetDiskFreeSpaceExW on Windows; None on lookup failure so the
/// gate falls through to "allow".
#[cfg(unix)]
fn free_space_bytes(dir: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut buf) };
    if rc != 0 {
        return None;
    }
    Some((buf.f_bavail as u64) * (buf.f_frsize as u64))
}
#[cfg(windows)]
fn free_space_bytes(dir: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);

    // `lpFreeBytesAvailable` is the bytes free on the disk that are
    // available to the user associated with the calling thread — the
    // Windows analogue of statvfs's `f_bavail` (blocks available to an
    // unprivileged user), as opposed to `lpTotalNumberOfFreeBytes`
    // which mirrors `f_bfree` and can include space reserved for
    // higher-privileged callers (e.g. per-user disk quotas).
    let mut free_bytes_available_to_caller: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_bytes_available_to_caller,
            &mut total_bytes,
            &mut total_free_bytes,
        )
    };
    if ok == 0 {
        return None;
    }
    Some(free_bytes_available_to_caller)
}
#[cfg(not(any(unix, windows)))]
fn free_space_bytes(_dir: &std::path::Path) -> Option<u64> {
    None
}

#[cfg(any(unix, windows))]
#[cfg(test)]
mod free_space_tests {
    use super::free_space_bytes;

    #[test]
    fn free_space_bytes_reports_nonzero_for_an_existing_dir() {
        // Exercises the real statvfs/GetDiskFreeSpaceExW call against the
        // system temp dir — the disk-budget gate falls through to "allow"
        // on `None`, so any regression that quietly turns a real lookup
        // into a `None` (wrong path encoding, wrong struct field, wrong
        // Win32 out-param) must fail this rather than only showing up as
        // an unbounded recording on a live machine.
        let dir = std::env::temp_dir();
        let free = free_space_bytes(&dir);
        assert!(
            free.is_some(),
            "expected a free-space reading for {}",
            dir.display()
        );
        // Zero would mean the disk is full or the call silently returned
        // a zeroed-out struct; neither should happen for the temp dir on
        // a working dev/CI machine.
        assert!(free.unwrap() > 0);
    }

    #[test]
    fn free_space_bytes_returns_none_for_a_nonexistent_path() {
        let bogus = std::path::Path::new("/this/path/does/not/exist/strivo-test-9f3c2");
        assert_eq!(free_space_bytes(bogus), None);
    }
}
