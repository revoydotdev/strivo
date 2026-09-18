//! Twitch/YouTube "auto-download new uploads" detection — the core-edition
//! counterpart to `monitor::patreon`'s per-creator auto-pull, but for the
//! two platforms whose channel-list poll already runs in `ChannelMonitor`.
//!
//! Patreon is deliberately absent here: it has its own dedicated poll loop
//! (`monitor::patreon::PatreonMonitor`) with its own watermark file, and
//! reuses `config.auto_pull_creators` as its opt-in list. Duplicating that
//! machinery for Twitch/YouTube would just be a second copy of the same
//! shape; instead `ChannelMonitor` calls into this module from its existing
//! `poll_all` loop.
//!
//! ## Interval
//!
//! The live channel-poll runs every ~90s across every followed channel
//! (~69 in the reference deployment). Re-fetching each opted-in channel's
//! upload list on that same cadence would multiply API cost for no benefit
//! — new uploads land on the order of hours, not seconds. Upload checks
//! instead run on their own `UPLOAD_CHECK_INTERVAL_SECS` (15 minutes)
//! timer, checked from inside `poll_all`, plus immediately whenever
//! `poll_all` is invoked from a YouTube WebSub `PollNow` (`force` = true) —
//! that call is itself already event-driven and cheap (opted-in channels
//! only), so there is no reason to make a real upload land minutes late.
//!
//! ## YouTube quota cost
//!
//! `Platform::fetch_channel_vods` for YouTube (`YouTubeClient::fetch_uploads_playlist`)
//! calls `playlistItems.list` against the channel's uploads playlist, not
//! `search.list`. `playlistItems.list` costs 1 quota unit per call
//! (vs. 100 for `search.list`), so even a dense follow list checked every
//! 15 minutes stays cheap against the default 10,000/day project quota.
//!
//! ## Twitch past-broadcast rule
//!
//! A Twitch channel with `auto_record` on already gets its live broadcasts
//! captured directly (and optionally VOD-backfilled — see
//! `RecordingConfig::auto_vod_backfill`). When that channel's past
//! broadcast later shows up in `fetch_channel_vods` as a `VodKind::LiveBroadcast`,
//! it is not a new upload — it is the same stream StriVo already has (or is
//! already capturing) under a different code path. `select_new_uploads`
//! therefore skips `VodKind::LiveBroadcast` entries for any channel with
//! `auto_record` enabled. `VodKind::Upload` (regular uploads, highlights)
//! is never skipped by this rule. YouTube channels get the same treatment
//! for the same reason — `fetch_channel_vods` flips a video's kind to
//! `LiveBroadcast` there too (`annotate_live` in `platform/youtube.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::platform::{VodEntry, VodKind};

/// How often (seconds) opted-in channels are checked for new uploads.
/// See the module doc for the cost rationale.
pub const UPLOAD_CHECK_INTERVAL_SECS: u64 = 900;

/// Build the watermark map key. Shared by the monitor and the state file.
pub fn watermark_key(platform: crate::platform::PlatformKind, channel_id: &str) -> String {
    format!("{platform}:{channel_id}")
}

/// Decide which of a channel's recently-fetched VODs are genuinely new
/// uploads to auto-download.
///
/// Pure — no IO, no network — so it's unit-testable without a platform
/// client. `watermark` is `None` on a channel's first-ever check: in that
/// case nothing is "new" (a fresh opt-in must not trigger a back-catalog
/// stampede — the caller establishes the watermark from this call and
/// downloads begin on the next check). `auto_record_enabled` implements the
/// Twitch/YouTube past-broadcast rule described in the module doc.
pub fn select_new_uploads(
    vods: &[VodEntry],
    watermark: Option<DateTime<Utc>>,
    auto_record_enabled: bool,
) -> Vec<VodEntry> {
    let Some(watermark) = watermark else {
        return Vec::new();
    };
    vods.iter()
        .filter(|v| v.published_at.is_some_and(|p| p > watermark))
        .filter(|v| !(auto_record_enabled && v.kind == VodKind::LiveBroadcast))
        .cloned()
        .collect()
}

/// The watermark to store after a check: the newest `published_at` among
/// `vods`, never moving backwards. On a first check with no dated VODs the
/// watermark starts at now, so the channel stops being "first check" and
/// uploads published afterwards are picked up. Returns `None` when there is
/// nothing to change.
pub fn next_watermark(vods: &[VodEntry], current: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let newest = vods.iter().filter_map(|v| v.published_at).max();
    match (current, newest) {
        (Some(cur), Some(n)) if n > cur => Some(n),
        (Some(_), _) => None,
        (None, Some(n)) => Some(n),
        (None, None) => Some(Utc::now()),
    }
}

pub type WatermarkMap = HashMap<String, DateTime<Utc>>;

pub fn load_watermarks(path: &Path) -> WatermarkMap {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_watermarks(path: &PathBuf, state: &WatermarkMap) -> anyhow::Result<()> {
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
    use crate::platform::PlatformKind;

    fn vod(id: &str, published_secs_ago: i64, kind: VodKind) -> VodEntry {
        VodEntry {
            id: id.into(),
            platform: PlatformKind::Twitch,
            channel_id: "ch1".into(),
            title: id.into(),
            published_at: Some(Utc::now() - chrono::Duration::seconds(published_secs_ago)),
            duration: None,
            url: format!("https://example/{id}"),
            thumbnail_url: None,
            kind,
        }
    }

    #[test]
    fn watermark_tracks_newest_published_not_wall_clock() {
        let cur = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![
            vod("a", 1800, VodKind::Upload),
            vod("b", 7200, VodKind::Upload),
        ];
        let next = next_watermark(&vods, Some(cur)).unwrap();
        assert_eq!(Some(next), vods[0].published_at);
        // Nothing newer than the current watermark: leave it alone.
        assert_eq!(
            next_watermark(&[vod("old", 7200, VodKind::Upload)], Some(cur)),
            None
        );
        // First check on an empty channel starts the watermark.
        assert!(next_watermark(&[], None).is_some());
    }

    #[test]
    fn first_check_downloads_nothing() {
        let vods = vec![vod("a", 10, VodKind::Upload)];
        let picked = select_new_uploads(&vods, None, false);
        assert!(
            picked.is_empty(),
            "first poll must only establish a watermark"
        );
    }

    #[test]
    fn newer_vod_is_selected_once() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![
            vod("new", 10, VodKind::Upload),
            vod("old", 7200, VodKind::Upload),
        ];
        let picked = select_new_uploads(&vods, Some(watermark), false);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "new");
    }

    #[test]
    fn live_broadcast_skipped_when_auto_record_on() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![vod("stream-vod", 10, VodKind::LiveBroadcast)];
        let picked = select_new_uploads(&vods, Some(watermark), true);
        assert!(picked.is_empty());
    }

    #[test]
    fn live_broadcast_kept_when_auto_record_off() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![vod("stream-vod", 10, VodKind::LiveBroadcast)];
        let picked = select_new_uploads(&vods, Some(watermark), false);
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn upload_kind_never_skipped_by_auto_record_rule() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![vod("clip", 10, VodKind::Upload)];
        let picked = select_new_uploads(&vods, Some(watermark), true);
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn watermark_round_trips_through_state_file() {
        let dir =
            std::env::temp_dir().join(format!("strivo-upload-wm-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("upload_watermarks.json");
        let mut state = WatermarkMap::new();
        state.insert("Twitch:ch1".to_string(), Utc::now());
        save_watermarks(&path, &state).unwrap();
        let loaded = load_watermarks(&path);
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("Twitch:ch1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
