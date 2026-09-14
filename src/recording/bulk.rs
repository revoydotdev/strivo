//! Per-channel bulk back-catalog downloader (task #71).
//!
//! Wraps the catalog pull engine (`catalog::run_pull`) in a daemon-side
//! manager that the TUI/webui can start and stop per channel. Each running
//! pull owns a `CancellationToken`; `Stop` cancels it and the engine breaks
//! between VODs. Progress is forwarded to the UI as `DaemonEvent::BulkProgress`.

use std::collections::HashMap;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{AppConfig, RecordingFormat};
use crate::events::DaemonEvent;
use crate::platform::{Platform, PlatformKind, VodEntry};
use crate::recording::catalog::{self, CatalogProgress, CatalogPullOptions};
use crate::recording::persist::PersistDb;

/// Commands the manager accepts. The TUI sends these over IPC; the daemon
/// forwards them to the manager's channel.
#[derive(Debug, Clone)]
pub enum BulkCommand {
    Start {
        operation_id: Uuid,
        channel_id: String,
        channel_name: String,
        platform: PlatformKind,
        /// Optional YouTube playlist scope (task #73). When set, only that
        /// playlist's items are pulled instead of the whole channel.
        playlist_id: Option<String>,
        /// Optional explicit VOD selection. None means all items in scope.
        vod_ids: Option<Vec<String>>,
    },
    Stop {
        channel_id: String,
        /// If supplied, only this operation may be cancelled.  `None` keeps
        /// compatibility with older IPC clients that only knew channel ids.
        operation_id: Option<Uuid>,
    },
    /// Fetch a YouTube channel's playlists and emit them as
    /// DaemonEvent::PlaylistList for the scope picker (task #73).
    ListPlaylists {
        channel_id: String,
    },
    /// Fetch playlist items for display.  This command is deliberately
    /// read-only and never invokes the catalog downloader.
    ListPlaylistItems {
        channel_id: String,
        playlist_id: String,
    },
    /// Fetch a channel's recent VODs (live + uploads) and emit them as
    /// DaemonEvent::ChannelVods for the webui channel-detail pane.
    FetchVods {
        channel_id: String,
        platform: PlatformKind,
    },
    /// Resolve a human identifier to a channel id and emit
    /// DaemonEvent::ChannelResolved for the Add-Channel wizard (task #19).
    ResolveChannel {
        platform: PlatformKind,
        query: String,
    },
}

/// Spawn the bulk-download manager. Returns the command sender; the daemon
/// keeps it and forwards `ClientMessage::BulkDownload` onto it.
///
/// `post_pull_markers` is the caller-registered `(extensions section,
/// marker filename)` list (see [`crate::daemon::DaemonPluginHost`]) passed
/// through to [`crate::config::AppConfig::post_pull_markers`] for each
/// pull — core has no plugin loaded at this point to ask directly, so the
/// registration has to arrive as data from daemon startup.
pub fn spawn(
    config: AppConfig,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    post_pull_markers: Vec<(String, String)>,
) -> mpsc::UnboundedSender<BulkCommand> {
    let (tx, mut rx) = mpsc::unbounded_channel::<BulkCommand>();
    // Internal clone so spawned pulls can self-deregister (Stop) without
    // moving the public sender into the manager task.
    let internal_tx = tx.clone();
    tokio::spawn(async move {
        // channel_id -> cancellation handle for the in-flight pull.
        let mut active: HashMap<String, (Uuid, CancellationToken)> = HashMap::new();
        while let Some(cmd) = rx.recv().await {
            match cmd {
                BulkCommand::Start {
                    operation_id,
                    channel_id,
                    channel_name,
                    platform,
                    playlist_id,
                    vod_ids,
                } => {
                    if active.contains_key(&channel_id) {
                        tracing::info!(channel = %channel_name, "bulk-dl already running");
                        continue;
                    }
                    let cancel = CancellationToken::new();
                    active.insert(channel_id.clone(), (operation_id, cancel.clone()));
                    let cfg = config.clone();
                    let etx = event_tx.clone();
                    let done_tx = internal_tx.clone();
                    let markers = post_pull_markers.clone();
                    tokio::spawn(async move {
                        run_channel_pull(
                            &cfg,
                            &channel_id,
                            &channel_name,
                            platform,
                            playlist_id.as_deref(),
                            vod_ids.as_deref(),
                            operation_id,
                            cancel,
                            &etx,
                            &markers,
                        )
                        .await;
                        // Self-deregister so a later Start can re-run.
                        let _ = done_tx.send(BulkCommand::Stop {
                            channel_id: channel_id.clone(),
                            operation_id: Some(operation_id),
                        });
                    });
                }
                BulkCommand::Stop { channel_id, operation_id } => {
                    if let Some((running_id, cancel)) = active.get(&channel_id) {
                        if operation_id.is_none() || operation_id == Some(*running_id) {
                            cancel.cancel();
                            active.remove(&channel_id);
                        } else {
                            tracing::warn!(channel = %channel_id, requested = ?operation_id, running = %running_id, "bulk-dl cancellation id did not match active operation");
                        }
                    }
                }
                BulkCommand::ListPlaylists { channel_id } => {
                    let cfg = config.clone();
                    let etx = event_tx.clone();
                    tokio::spawn(async move {
                        let playlists =
                            fetch_playlists(&cfg, &channel_id)
                                .await
                                .unwrap_or_else(|e| {
                                    tracing::warn!("bulk-dl: fetch_playlists failed: {e:#}");
                                    Vec::new()
                                });
                        let _ = etx.send(DaemonEvent::PlaylistList {
                            channel_id,
                            playlists,
                        });
                    });
                }
                BulkCommand::ListPlaylistItems { channel_id, playlist_id } => {
                    let cfg = config.clone();
                    let etx = event_tx.clone();
                    tokio::spawn(async move {
                        let items = fetch_playlist_items(&cfg, &channel_id, &playlist_id)
                            .await
                            .unwrap_or_else(|e| {
                                tracing::warn!("bulk-dl: fetch_playlist_items failed: {e:#}");
                                Vec::new()
                            });
                        let _ = etx.send(DaemonEvent::PlaylistItems {
                            channel_id,
                            playlist_id,
                            items,
                        });
                    });
                }
                BulkCommand::FetchVods {
                    channel_id,
                    platform,
                } => {
                    let cfg = config.clone();
                    let etx = event_tx.clone();
                    tokio::spawn(async move {
                        let vods = fetch_recent_vods(&cfg, &channel_id, platform)
                            .await
                            .unwrap_or_else(|e| {
                                tracing::warn!("bulk-dl: fetch_recent_vods failed: {e:#}");
                                Vec::new()
                            });
                        let _ = etx.send(DaemonEvent::ChannelVods { channel_id, vods });
                    });
                }
                BulkCommand::ResolveChannel { platform, query } => {
                    let cfg = config.clone();
                    let etx = event_tx.clone();
                    tokio::spawn(async move {
                        let event = match resolve_channel(&cfg, platform, &query).await {
                            Ok((channel_id, display_name)) => DaemonEvent::ChannelResolved {
                                platform,
                                query,
                                channel_id: Some(channel_id),
                                display_name: Some(display_name),
                                error: None,
                            },
                            Err(e) => DaemonEvent::ChannelResolved {
                                platform,
                                query,
                                channel_id: None,
                                display_name: None,
                                error: Some(format!("{e:#}")),
                            },
                        };
                        let _ = etx.send(event);
                    });
                }
            }
        }
    });
    tx
}

/// Fetch a channel's recent VODs (live + uploads) for the webui channel
/// detail. YouTube annotates live-vs-upload; Twitch returns past broadcasts;
/// Patreon has no VOD catalog here (the webui shows its cached posts).
async fn fetch_recent_vods(
    config: &AppConfig,
    channel_id: &str,
    platform: PlatformKind,
) -> anyhow::Result<Vec<VodEntry>> {
    use anyhow::Context;
    match platform {
        PlatformKind::YouTube => {
            let cfg = config
                .youtube
                .clone()
                .context("youtube section missing in config")?;
            let yt = crate::platform::youtube::YouTubePlatform::new(
                cfg.client_id,
                cfg.client_secret,
                cfg.cookies_path.clone(),
            );
            yt.load_stored_tokens().await.context("youtube auth")?;
            yt.fetch_recent_videos(channel_id, 30).await
        }
        PlatformKind::Twitch => {
            let cfg = config
                .twitch
                .clone()
                .context("twitch section missing in config")?;
            let tw = crate::platform::twitch::TwitchPlatform::new(cfg.client_id, cfg.client_secret);
            tw.load_stored_tokens().await.context("twitch auth")?;
            tw.fetch_channel_vods(channel_id, None, Some(30)).await
        }
        PlatformKind::Patreon => Ok(Vec::new()),
    }
}

/// Resolve a human-entered identifier to a channel id (task #19). Twitch
/// resolves a login to its numeric id; YouTube/Patreon have no public search
/// wired, so the entered id is accepted as-is.
async fn resolve_channel(
    config: &AppConfig,
    platform: PlatformKind,
    query: &str,
) -> anyhow::Result<(String, String)> {
    use anyhow::Context;
    let query = query.trim();
    if query.is_empty() {
        anyhow::bail!("empty query");
    }
    match platform {
        PlatformKind::Twitch => {
            let cfg = config
                .twitch
                .clone()
                .context("twitch section missing in config")?;
            let tw = crate::platform::twitch::TwitchPlatform::new(cfg.client_id, cfg.client_secret);
            tw.load_stored_tokens().await.context("twitch auth")?;
            let (id, display_name) = tw
                .lookup_channel_id_by_login(query)
                .await
                .with_context(|| format!("no Twitch channel for login '{query}'"))?;
            Ok((id, display_name))
        }
        PlatformKind::YouTube | PlatformKind::Patreon => Ok((query.to_string(), query.to_string())),
    }
}

/// Resolve a channel's catalog and run the pull, emitting BulkProgress.
#[allow(clippy::too_many_arguments)]
async fn run_channel_pull(
    config: &AppConfig,
    channel_id: &str,
    channel_name: &str,
    platform: PlatformKind,
    playlist_id: Option<&str>,
    vod_ids: Option<&[String]>,
    operation_id: Uuid,
    cancel: CancellationToken,
    event_tx: &mpsc::UnboundedSender<DaemonEvent>,
    post_pull_markers: &[(String, String)],
) {
    let emit = |done: usize, total: usize, percent: Option<f32>, active: bool| {
        let _ = event_tx.send(DaemonEvent::BulkProgress {
            operation_id,
            channel_id: channel_id.to_string(),
            done,
            total,
            percent,
            active,
        });
    };

    emit(0, 0, None, true);

    let vods = match resolve_vods(config, channel_id, platform, playlist_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(channel = %channel_name, "bulk-dl resolve failed: {e:#}");
            let _ = event_tx.send(DaemonEvent::Error(format!("Bulk DL {channel_name}: {e}")));
            emit(0, 0, None, false);
            return;
        }
    };
    let vods = if let Some(ids) = vod_ids {
        vods.into_iter()
            .filter(|vod| ids.iter().any(|id| id == &vod.id))
            .collect::<Vec<_>>()
    } else {
        vods
    };
    let total = vods.len();
    if total == 0 {
        let _ = event_tx.send(DaemonEvent::Notification {
            title: format!("Bulk DL: {channel_name}"),
            body: "No new catalog items".to_string(),
        });
        emit(0, 0, None, false);
        return;
    }

    // Forward catalog progress as a running done/total counter.
    let (ptx, mut prx) = mpsc::unbounded_channel::<CatalogProgress>();
    let etx2 = event_tx.clone();
    let cid = channel_id.to_string();
    tokio::spawn(async move {
        let mut done = 0usize;
        while let Some(ev) = prx.recv().await {
            match ev {
                CatalogProgress::Finished { .. }
                | CatalogProgress::Skipped { .. }
                | CatalogProgress::Failed { .. } => {
                    done += 1;
                    let _ = etx2.send(DaemonEvent::BulkProgress {
                        operation_id,
                        channel_id: cid.clone(),
                        done,
                        total,
                        percent: None,
                        active: true,
                    });
                }
                CatalogProgress::Progress { pct } => {
                    let _ = etx2.send(DaemonEvent::BulkProgress { operation_id, channel_id: cid.clone(), done, total, percent: Some(pct), active: true });
                }
                _ => {}
            }
        }
    });

    let cookies_path = if matches!(platform, PlatformKind::YouTube) {
        config.youtube.as_ref().and_then(|y| y.cookies_path.clone())
    } else {
        None
    };
    let chan_override = config
        .auto_record_channels
        .iter()
        .find(|c| c.channel_id == channel_id && c.platform == platform.to_string())
        .and_then(|c| c.format.clone());
    let resolved = RecordingFormat::resolved(chan_override.as_ref(), &config.recording.format);

    let marker_registrations: Vec<(&str, &str)> = post_pull_markers
        .iter()
        .map(|(section, marker)| (section.as_str(), marker.as_str()))
        .collect();
    let opts = CatalogPullOptions {
        root: config.recording_dir.clone(),
        channel_name: channel_name.to_string(),
        format: resolved,
        cookies_path,
        force: false,
        post_pull_markers: config.post_pull_markers(false, &marker_registrations),
    };

    let db_path = AppConfig::data_dir().join("jobs.db");
    let db = match PersistDb::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            let _ = event_tx.send(DaemonEvent::Error(format!("Bulk DL db: {e}")));
            emit(0, total, None, false);
            return;
        }
    };

    match catalog::run_pull(&db, vods, &opts, Some(ptx), Some(&cancel)).await {
        Ok(report) => {
            let _ = event_tx.send(DaemonEvent::Notification {
                title: format!("Bulk DL: {channel_name}"),
                body: format!(
                    "downloaded {} · skipped {} · failed {}",
                    report.downloaded,
                    report.skipped_existing,
                    report.failed.len()
                ),
            });
        }
        Err(e) => {
            let _ = event_tx.send(DaemonEvent::Error(format!("Bulk DL {channel_name}: {e}")));
        }
    }
    emit(total, total, Some(100.0), false);
}

/// Fetch a YouTube channel's playlists for the scope picker (task #73).
async fn fetch_playlists(
    config: &AppConfig,
    channel_id: &str,
) -> anyhow::Result<Vec<crate::platform::PlaylistInfo>> {
    use anyhow::Context;
    let cfg = config
        .youtube
        .clone()
        .context("youtube section missing in config")?;
    let yt = crate::platform::youtube::YouTubePlatform::new(
        cfg.client_id,
        cfg.client_secret,
        cfg.cookies_path.clone(),
    );
    yt.load_stored_tokens().await.context("youtube auth")?;
    yt.fetch_playlists(channel_id).await
}

/// Fetch one playlist's items for a read-only viewer.  Keeping this separate
/// from `resolve_vods` makes it impossible for a viewer request to enqueue a
/// download as a side effect.
async fn fetch_playlist_items(
    config: &AppConfig,
    channel_id: &str,
    playlist_id: &str,
) -> anyhow::Result<Vec<VodEntry>> {
    use anyhow::Context;
    let cfg = config
        .youtube
        .clone()
        .context("youtube section missing in config")?;
    let yt = crate::platform::youtube::YouTubePlatform::new(
        cfg.client_id,
        cfg.client_secret,
        cfg.cookies_path.clone(),
    );
    yt.load_stored_tokens().await.context("youtube auth")?;
    yt.fetch_playlist_items(playlist_id, channel_id, None, None).await
}

/// Resolve a channel's back-catalog VODs. Mirrors the `pull` CLI path.
async fn resolve_vods(
    config: &AppConfig,
    channel_id: &str,
    platform: PlatformKind,
    playlist_id: Option<&str>,
) -> anyhow::Result<Vec<VodEntry>> {
    use anyhow::Context;
    match platform {
        PlatformKind::YouTube => {
            let cfg = config
                .youtube
                .clone()
                .context("youtube section missing in config")?;
            let yt = crate::platform::youtube::YouTubePlatform::new(
                cfg.client_id,
                cfg.client_secret,
                cfg.cookies_path.clone(),
            );
            yt.load_stored_tokens().await.context("youtube auth")?;
            // Playlist scope (task #73): pull only that playlist's items.
            if let Some(pl) = playlist_id {
                yt.fetch_playlist_items(pl, channel_id, None, None).await
            } else {
                yt.fetch_channel_vods(channel_id, None, None).await
            }
        }
        PlatformKind::Twitch => {
            let cfg = config
                .twitch
                .clone()
                .context("twitch section missing in config")?;
            let tw = crate::platform::twitch::TwitchPlatform::new(cfg.client_id, cfg.client_secret);
            tw.load_stored_tokens().await.context("twitch auth")?;
            tw.fetch_channel_vods(channel_id, None, None).await
        }
        PlatformKind::Patreon => {
            let cfg = config
                .patreon
                .clone()
                .context("patreon section missing in config")?;
            let pt = crate::platform::patreon::PatreonClient::new(cfg.client_id, cfg.client_secret);
            pt.load_stored_tokens().await.context("patreon auth")?;
            pt.fetch_channel_vods(channel_id, None, None).await
        }
    }
}
