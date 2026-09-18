use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingState {
    ResolvingUrl,
    Recording,
    Stopping,
    Finished,
    Failed,
}

impl std::fmt::Display for RecordingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordingState::ResolvingUrl => write!(f, "Resolving"),
            RecordingState::Recording => write!(f, "Recording"),
            RecordingState::Stopping => write!(f, "Stopping"),
            RecordingState::Finished => write!(f, "Finished"),
            RecordingState::Failed => write!(f, "Failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingJob {
    pub id: Uuid,
    pub channel_id: String,
    pub channel_name: String,
    pub platform: crate::platform::PlatformKind,
    pub state: RecordingState,
    pub output_path: PathBuf,
    pub started_at: DateTime<Utc>,
    pub bytes_written: u64,
    pub duration_secs: f64,
    pub transcode: bool,
    pub error: Option<String>,
    pub stream_title: Option<String>,
    pub watched: bool,
    /// Optional playlist/group this recording belongs to.
    #[serde(default)]
    pub playlist: Option<String>,
    /// Source stream/VOD thumbnail URL captured at record-start; snapshotted
    /// to a local cache for the webui cover art (upstream URL can expire).
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    /// Source URL the recording was created from (VOD/past-broadcast URL on
    /// `RecordingCommand::DownloadVod`, None for live captures). The webui
    /// reads this to mark a "Past Broadcasts" pill as downloaded — definitive
    /// correlation instead of FIFO-by-channel-name guesswork.
    #[serde(default)]
    pub source_url: Option<String>,
    /// ffprobe `codec_name` of the first video stream (e.g. "av1", "vp9",
    /// "h264", "hevc"). Populated by the finish-time probe; `None` for rows
    /// persisted before this field existed or when the probe failed.
    #[serde(default)]
    pub video_codec: Option<String>,
    /// ffprobe `codec_name` of the first audio stream (e.g. "opus", "aac").
    #[serde(default)]
    pub audio_codec: Option<String>,
}

impl RecordingJob {
    pub fn new(
        channel_id: String,
        channel_name: String,
        platform: crate::platform::PlatformKind,
        output_path: PathBuf,
        transcode: bool,
        stream_title: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            channel_id,
            channel_name,
            platform,
            state: RecordingState::ResolvingUrl,
            output_path,
            started_at: Utc::now(),
            bytes_written: 0,
            duration_secs: 0.0,
            transcode,
            error: None,
            stream_title,
            watched: false,
            playlist: None,
            thumbnail_url: None,
            source_url: None,
            video_codec: None,
            audio_codec: None,
        }
    }

    /// Create a RecordingJob from an existing file on disk (for scan results).
    ///
    /// The id is derived deterministically from the canonical file path via
    /// UUIDv5 so a daemon restart that re-scans the recording directory
    /// produces the *same* id for the same file — keeping permalinks in the
    /// webui stable across restarts. Falls back to the non-canonical path
    /// when canonicalisation fails (e.g. dangling symlink); the underlying
    /// path is what changed, not the recording's identity.
    pub fn from_file(
        path: PathBuf,
        channel_name: String,
        platform: crate::platform::PlatformKind,
        stream_title: Option<String>,
        started_at: DateTime<Utc>,
    ) -> Self {
        let bytes_written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        let id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            canonical.as_os_str().as_encoded_bytes(),
        );
        Self {
            id,
            channel_id: String::new(),
            channel_name,
            platform,
            state: RecordingState::Finished,
            output_path: path,
            started_at,
            bytes_written,
            duration_secs: 0.0,
            transcode: false,
            error: None,
            stream_title,
            watched: false,
            playlist: None,
            thumbnail_url: None,
            source_url: None,
            video_codec: None,
            audio_codec: None,
        }
    }

    pub fn format_duration(&self) -> String {
        let secs = self.duration_secs as u64;
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;
        if hours > 0 {
            format!("{hours}:{mins:02}:{secs:02}")
        } else {
            format!("{mins}:{secs:02}")
        }
    }

    #[allow(dead_code)]
    pub fn format_size(&self) -> String {
        let bytes = self.bytes_written;
        if bytes >= 1_073_741_824 {
            format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
        } else if bytes >= 1_048_576 {
            format!("{:.1} MB", bytes as f64 / 1_048_576.0)
        } else if bytes >= 1024 {
            format!("{:.0} KB", bytes as f64 / 1024.0)
        } else {
            format!("{bytes} B")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PlatformKind;

    /// `from_file` must derive its id deterministically from the file
    /// path — otherwise daemon restarts that re-scan the recording dir
    /// invalidate every webui permalink to an orphaned recording.
    #[test]
    fn from_file_id_is_stable_per_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("falco_clip.mkv");
        std::fs::write(&path, b"x").unwrap();
        let now = Utc::now();

        let a = RecordingJob::from_file(
            path.clone(),
            "falco".into(),
            PlatformKind::Twitch,
            None,
            now,
        );
        let b = RecordingJob::from_file(
            path.clone(),
            "falco".into(),
            PlatformKind::Twitch,
            None,
            now,
        );
        assert_eq!(a.id, b.id, "same file must produce same UUID across calls");

        // Different file → different id.
        let other = dir.path().join("other.mkv");
        std::fs::write(&other, b"y").unwrap();
        let c = RecordingJob::from_file(other, "falco".into(), PlatformKind::Twitch, None, now);
        assert_ne!(a.id, c.id, "different file must produce a different UUID");
    }
}
