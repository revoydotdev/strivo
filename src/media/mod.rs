//! Proxies, waveform pyramids, filmstrips and bounded preview caching
//! (plan todo 50, report item R02).
//!
//! The three submodules answer three questions in order: [`proxy`] turns a
//! source into something cheap to navigate and measures how its timeline lines
//! up with the source's; [`pyramid`] and [`preview`] read *that* rather than
//! the source, so a six-hour recording is never decoded whole during an
//! interaction; [`spawn`] records every child process these paths start, which
//! is what makes the "no original-resolution decode" claim observable instead
//! of asserted.
pub mod preview;
pub mod proxy;
pub mod pyramid;
pub mod spawn;

use anyhow::Result;
use std::path::Path;
use tokio::process::Command;

/// Rich media information extracted via ffprobe.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct MediaInfo {
    pub format_name: String,
    pub duration_secs: f64,
    pub size_bytes: u64,
    pub bit_rate: u64,
    pub video_codec: Option<String>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub audio_codec: Option<String>,
    pub audio_sample_rate: Option<u32>,
}

#[allow(dead_code)]
impl MediaInfo {
    pub fn resolution_str(&self) -> String {
        match (self.video_width, self.video_height) {
            (Some(w), Some(h)) => format!("{w}x{h}"),
            _ => "unknown".to_string(),
        }
    }

    pub fn bitrate_str(&self) -> String {
        if self.bit_rate >= 1_000_000 {
            format!("{:.1} Mbps", self.bit_rate as f64 / 1_000_000.0)
        } else if self.bit_rate >= 1_000 {
            format!("{:.0} kbps", self.bit_rate as f64 / 1_000.0)
        } else if self.bit_rate > 0 {
            format!("{} bps", self.bit_rate)
        } else {
            "unknown".to_string()
        }
    }

    pub fn size_str(&self) -> String {
        let bytes = self.size_bytes;
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

    pub fn duration_str(&self) -> String {
        let secs = self.duration_secs as u64;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        }
    }
}

/// Probe a media file using ffprobe and return structured info.
pub async fn probe_file(path: &Path) -> Result<MediaInfo> {
    let output = Command::new(crate::tools::resolve_tool("ffprobe"))
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe failed: {stderr}");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let mut info = MediaInfo::default();

    // Parse format section
    if let Some(format) = json.get("format") {
        info.format_name = format
            .get("format_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        info.duration_secs = format
            .get("duration")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        info.size_bytes = format
            .get("size")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        info.bit_rate = format
            .get("bit_rate")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
    }

    // Parse streams
    if let Some(streams) = json.get("streams").and_then(|v| v.as_array()) {
        for stream in streams {
            let codec_type = stream
                .get("codec_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match codec_type {
                "video" if info.video_codec.is_none() => {
                    info.video_codec = stream
                        .get("codec_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    info.video_width = stream
                        .get("width")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32);
                    info.video_height = stream
                        .get("height")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32);
                }
                "audio" if info.audio_codec.is_none() => {
                    info.audio_codec = stream
                        .get("codec_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    info.audio_sample_rate = stream
                        .get("sample_rate")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok());
                }
                _ => {}
            }
        }
    }

    Ok(info)
}
