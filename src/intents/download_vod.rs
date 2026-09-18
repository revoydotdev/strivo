//! Translate a [`DownloadVodSpec`] into a [`RecordingCommand::DownloadVod`].

use std::path::{Path, PathBuf};

use crate::config::AppConfig;
use crate::intents::spec::{DownloadVodSpec, OutputPathPolicy};
use crate::recording::{self, RecordingCommand};

/// Build the engine-ready `DownloadVod` command.
///
/// Applies the spec's `OutputPathPolicy` and `CookieSource` against
/// `config`. Pure: no IO, no channels, no async.
pub fn download_vod(spec: DownloadVodSpec, config: &AppConfig) -> RecordingCommand {
    let cookies_path = spec.cookies.resolve(config, spec.platform);
    let output_path = match &spec.output_policy {
        OutputPathPolicy::Fresh => recording::build_output_path(
            config,
            &spec.channel_name,
            spec.platform,
            spec.post_title.as_deref(),
        ),
        OutputPathPolicy::AdjacentTo(live) => vod_path_adjacent_to(live),
    };

    RecordingCommand::DownloadVod {
        url: spec.url,
        channel_name: spec.channel_name,
        platform: spec.platform,
        output_path,
        cookies_path,
        post_title: spec.post_title,
        channel_id: spec.channel_id,
    }
}

/// `<base>.<ext>` → `<base>_vod.<ext>`.
///
/// Promoted from the private helper that used to live in
/// `recording/vod_backfill.rs` so the policy is reachable from the
/// intent layer.
fn vod_path_adjacent_to(live: &Path) -> PathBuf {
    let parent = live.parent().unwrap_or(Path::new("."));
    let stem = live
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let ext = live.extension().and_then(|s| s.to_str()).unwrap_or("mkv");
    parent.join(format!("{stem}_vod.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intents::cookies::CookieSource;
    use crate::platform::PlatformKind;
    use std::path::PathBuf;

    fn base_spec(platform: PlatformKind, policy: OutputPathPolicy) -> DownloadVodSpec {
        DownloadVodSpec {
            url: "https://example/v".into(),
            channel_name: "creator".into(),
            platform,
            post_title: Some("post".into()),
            cookies: CookieSource::Inherit,
            output_policy: policy,
            channel_id: None,
        }
    }

    #[test]
    fn adjacent_policy_appends_vod_suffix() {
        let cfg = AppConfig::default();
        let live = PathBuf::from("/r/falco_2026-05-22.mkv");
        let spec = base_spec(
            PlatformKind::Twitch,
            OutputPathPolicy::AdjacentTo(live.clone()),
        );
        match download_vod(spec, &cfg) {
            RecordingCommand::DownloadVod { output_path, .. } => {
                assert_eq!(output_path, PathBuf::from("/r/falco_2026-05-22_vod.mkv"));
            }
            _ => panic!("expected DownloadVod"),
        }
    }

    #[test]
    fn adjacent_policy_handles_missing_extension() {
        let cfg = AppConfig::default();
        let live = PathBuf::from("/r/falco");
        let spec = base_spec(
            PlatformKind::Twitch,
            OutputPathPolicy::AdjacentTo(live.clone()),
        );
        match download_vod(spec, &cfg) {
            RecordingCommand::DownloadVod { output_path, .. } => {
                assert_eq!(output_path, PathBuf::from("/r/falco_vod.mkv"));
            }
            _ => panic!("expected DownloadVod"),
        }
    }

    #[test]
    fn fresh_policy_routes_through_build_output_path() {
        // We can't assert the exact path (it embeds wall-clock); we
        // just assert it lands under `config.recording_dir`.
        let mut cfg = AppConfig::default();
        cfg.recording_dir = PathBuf::from("/tmp/strivo-test");
        let spec = base_spec(PlatformKind::Twitch, OutputPathPolicy::Fresh);
        match download_vod(spec, &cfg) {
            RecordingCommand::DownloadVod { output_path, .. } => {
                assert!(
                    output_path.starts_with("/tmp/strivo-test"),
                    "expected output under recording_dir, got {output_path:?}"
                );
            }
            _ => panic!("expected DownloadVod"),
        }
    }

    #[test]
    fn cookies_from_config_picks_youtube_path() {
        let mut cfg = AppConfig::default();
        cfg.youtube = Some(crate::config::YouTubeConfig {
            client_id: String::new(),
            client_secret: String::new(),
            cookies_path: Some(PathBuf::from("/y.txt")),
            websub_callback_url: None,
        });
        let mut spec = base_spec(PlatformKind::YouTube, OutputPathPolicy::Fresh);
        spec.cookies = CookieSource::FromConfig;
        match download_vod(spec, &cfg) {
            RecordingCommand::DownloadVod { cookies_path, .. } => {
                assert_eq!(cookies_path, Some(PathBuf::from("/y.txt")));
            }
            _ => panic!("expected DownloadVod"),
        }
    }
}
