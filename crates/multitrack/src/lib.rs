//! strivo-multitrack — multi-track audio enumeration + per-track
//! extraction.
//!
//! Streamers running OBS in multi-track recording mode (the standard
//! pro setup) end up with one container holding 4-6 audio tracks:
//! game, mic, Discord, music, browser, etc. The DAW vision wants
//! those surfaced as real tracks the user can mute/solo and export.
//!
//! This iteration ships the enumeration + extraction path:
//!
//!   - [`parse_streams_json`] turns ffprobe's `-show_streams` JSON
//!     into a typed [`AudioTrack`] list, with channel layout, sample
//!     rate, and the conventional OBS naming hint (`title` tag).
//!   - [`probe_audio_tracks`] runs ffprobe and pipes through the
//!     parser.
//!   - [`extract_track`] cuts a single audio stream into a standalone
//!     file with `ffmpeg -map 0:a:N -c copy` — lossless and fast.
//!
//! Demucs-style source separation on a single mixed stereo (the
//! "single-track OBS" case) is the natural follow-up; the
//! [`SourceSplitter`] trait reserves the slot without forcing a heavy
//! dep today.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTrack {
    /// Stream index inside the container.
    pub index: u32,
    /// Codec name reported by ffprobe (aac, opus, flac, ...).
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    /// Optional title tag — OBS multi-track recordings stamp track
    /// labels here ("Mic", "Game", "Discord", ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional language hint (`und`, `eng`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Inferred kind from the title hint — best-effort categorisation
    /// for the SPA so it can colour-code without the user re-labelling.
    pub inferred_kind: TrackKind,
}

/// What a track is *for*, from the name the recorder stamped on it.
///
/// `Mixed` is OBS's own fallback: the single downmixed output a multitrack
/// recording always carries alongside the stems, and the one a listener can
/// trust when they trust nothing else. It is a role like any other — it is kept
/// as a track, not dropped as a duplicate of the sum of the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Mic,
    Game,
    Discord,
    Music,
    Browser,
    Mixed,
    Other,
}

impl TrackKind {
    /// The role's stable lowercase name.
    ///
    /// Defined by a `match` rather than derived from the serde rename, and
    /// pinned to it by a test over every variant, because the same word ends up
    /// on a database row and in a JSON body and two spellings of one role is
    /// the drift this enum is the single answer to.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::Game => "game",
            Self::Discord => "discord",
            Self::Music => "music",
            Self::Browser => "browser",
            Self::Mixed => "mixed",
            Self::Other => "other",
        }
    }
}

/// Future hook for actual source-separation backends (vocal remover /
/// OpenVINO / hosted services). The trait lets the SPA call the same
/// UI for either real tracks (probed) or split tracks (separated) —
/// only the implementation behind this differs.
pub trait SourceSplitter: Send + Sync {
    fn split(&self, input: &Path, out_dir: &Path) -> Result<Vec<AudioTrack>>;
}

/// Parse the relevant fields from `ffprobe -of json -show_streams`.
/// Pure function — the test suite feeds canned JSON.
pub fn parse_streams_json(json: &str) -> Result<Vec<AudioTrack>> {
    #[derive(Deserialize)]
    struct Wrap {
        #[serde(default)]
        streams: Vec<RawStream>,
    }
    #[derive(Deserialize, Default)]
    struct RawStream {
        #[serde(default)]
        index: u32,
        #[serde(default)]
        codec_type: String,
        #[serde(default)]
        codec_name: String,
        #[serde(default)]
        channels: u32,
        #[serde(default)]
        sample_rate: String,
        #[serde(default)]
        tags: Option<RawTags>,
    }
    #[derive(Deserialize, Default)]
    struct RawTags {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        language: Option<String>,
        #[serde(rename = "TITLE", default)]
        title_upper: Option<String>,
        #[serde(rename = "LANGUAGE", default)]
        language_upper: Option<String>,
    }

    let wrap: Wrap = serde_json::from_str(json).context("parse ffprobe json")?;
    Ok(wrap
        .streams
        .into_iter()
        .filter(|s| s.codec_type == "audio")
        .map(|s| {
            let title = s
                .tags
                .as_ref()
                .and_then(|t| t.title.clone().or_else(|| t.title_upper.clone()));
            let language = s
                .tags
                .as_ref()
                .and_then(|t| t.language.clone().or_else(|| t.language_upper.clone()));
            let sample_rate = s.sample_rate.parse().unwrap_or(0);
            let inferred_kind = infer_kind(title.as_deref().unwrap_or(""));
            AudioTrack {
                index: s.index,
                codec: s.codec_name,
                channels: s.channels,
                sample_rate,
                title,
                language,
                inferred_kind,
            }
        })
        .collect())
}

/// Guess the conventional OBS track name → [`TrackKind`] mapping.
/// Substring match (case-insensitive) so "Mic/Aux", "Mic 1",
/// "Microphone" all land on `Mic`.
pub fn infer_kind(title: &str) -> TrackKind {
    let t = title.to_lowercase();
    // The downmixed fallback first: "Mixed Audio" and "Stream Audio" are the
    // names OBS gives it, and a name that also contains "mic" or "game" is still
    // the fallback, so this has to win over the substring rules below.
    if t.contains("mixed") || t.contains("stream audio") || t == "default" {
        return TrackKind::Mixed;
    }
    if t.contains("mic") || t.contains("voice") || t.contains("vocal") {
        TrackKind::Mic
    } else if t.contains("game") || t.contains("system") || t.contains("desktop") {
        TrackKind::Game
    } else if t.contains("discord") || t.contains("voice chat") || t.contains("vc") {
        TrackKind::Discord
    } else if t.contains("music")
        || t.contains("spotify")
        || t.contains("song")
        || t.contains("ost")
    {
        TrackKind::Music
    } else if t.contains("browser") || t.contains("chrome") || t.contains("alert") {
        TrackKind::Browser
    } else {
        TrackKind::Other
    }
}

/// Run ffprobe against `input` and return the audio-stream list.
pub fn probe_audio_tracks(input: &Path) -> Result<Vec<AudioTrack>> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_streams"])
        .arg(input)
        .output()
        .context("spawn ffprobe")?;
    if !out.status.success() {
        anyhow::bail!(
            "ffprobe exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let s = String::from_utf8(out.stdout).context("ffprobe utf-8 output")?;
    parse_streams_json(&s)
}

/// Cut a single audio stream out of `input` into `output`. Uses
/// `-c copy` — the source is whatever codec OBS recorded (usually AAC
/// or Opus) and we keep it.
pub fn extract_track(input: &Path, track_index: u32, output: &Path) -> Result<u64> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let map = format!("0:{track_index}");
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(input)
        .args(["-map", &map, "-c", "copy"])
        .arg(output)
        .status()
        .context("spawn ffmpeg")?;
    if !status.success() {
        anyhow::bail!("ffmpeg exited {status}");
    }
    let meta = std::fs::metadata(output).context("stat output")?;
    Ok(meta.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_audio_streams_skipping_video() {
        let json = r#"{
            "streams": [
                {"index":0,"codec_type":"video","codec_name":"h264"},
                {"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000",
                  "tags":{"title":"Mic","language":"eng"}},
                {"index":2,"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000",
                  "tags":{"title":"Game"}}
            ]
        }"#;
        let tracks = parse_streams_json(json).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].index, 1);
        assert_eq!(tracks[0].codec, "aac");
        assert_eq!(tracks[0].channels, 2);
        assert_eq!(tracks[0].sample_rate, 48_000);
        assert_eq!(tracks[0].title.as_deref(), Some("Mic"));
        assert_eq!(tracks[0].language.as_deref(), Some("eng"));
        assert_eq!(tracks[0].inferred_kind, TrackKind::Mic);
        assert_eq!(tracks[1].inferred_kind, TrackKind::Game);
    }

    #[test]
    fn handles_uppercase_tag_keys() {
        // Matroska tooling sometimes uppercases TITLE / LANGUAGE.
        let json = r#"{
            "streams": [
                {"index":1,"codec_type":"audio","codec_name":"opus","channels":1,"sample_rate":"48000",
                  "tags":{"TITLE":"Discord","LANGUAGE":"und"}}
            ]
        }"#;
        let tracks = parse_streams_json(json).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title.as_deref(), Some("Discord"));
        assert_eq!(tracks[0].language.as_deref(), Some("und"));
        assert_eq!(tracks[0].inferred_kind, TrackKind::Discord);
    }

    #[test]
    fn empty_streams_yields_empty_tracks() {
        let tracks = parse_streams_json(r#"{"streams":[]}"#).unwrap();
        assert!(tracks.is_empty());
    }

    #[test]
    fn malformed_json_returns_error() {
        let r = parse_streams_json("not json");
        assert!(r.is_err());
    }

    #[test]
    fn inferred_kind_mic_synonyms() {
        assert_eq!(infer_kind("Mic 1"), TrackKind::Mic);
        assert_eq!(infer_kind("Microphone"), TrackKind::Mic);
        assert_eq!(infer_kind("Voice"), TrackKind::Mic);
        assert_eq!(infer_kind("Vocal"), TrackKind::Mic);
    }

    #[test]
    fn inferred_kind_game_synonyms() {
        assert_eq!(infer_kind("Game"), TrackKind::Game);
        assert_eq!(infer_kind("System Audio"), TrackKind::Game);
        assert_eq!(infer_kind("Desktop"), TrackKind::Game);
    }

    #[test]
    fn inferred_kind_other_for_unknown() {
        assert_eq!(infer_kind("Random Label"), TrackKind::Other);
        assert_eq!(infer_kind(""), TrackKind::Other);
    }

    #[test]
    fn inferred_kind_priority_mic_over_other_matches() {
        // 'mic' substring wins even if "music" would also match —
        // since 'music' starts with 'mu', not 'mic'. Sanity check.
        assert_eq!(infer_kind("Mic"), TrackKind::Mic);
        assert_eq!(infer_kind("Music Spotify"), TrackKind::Music);
    }

    #[test]
    fn inferred_kind_mixed_wins_over_the_others() {
        assert_eq!(infer_kind("Mixed"), TrackKind::Mixed);
        assert_eq!(infer_kind("Mixed Audio"), TrackKind::Mixed);
        assert_eq!(infer_kind("Stream Audio"), TrackKind::Mixed);
        assert_eq!(infer_kind("default"), TrackKind::Mixed);
        assert_eq!(
            infer_kind("Mixed: Game + Mic"),
            TrackKind::Mixed,
            "a downmix name that mentions the stems is still the downmix"
        );
    }

    /// `as_str` and the serde rename are two spellings of one role unless a
    /// test holds them together, and the row a role is written to and the JSON a
    /// UI reads both use them.
    #[test]
    fn as_str_agrees_with_the_serde_name_for_every_role() {
        for kind in [
            TrackKind::Mic,
            TrackKind::Game,
            TrackKind::Discord,
            TrackKind::Music,
            TrackKind::Browser,
            TrackKind::Mixed,
            TrackKind::Other,
        ] {
            let json = serde_json::to_string(&kind).expect("a unit variant serialises");
            assert_eq!(
                json,
                format!("\"{}\"", kind.as_str()),
                "{kind:?} spells itself differently in serde and in as_str"
            );
        }
    }
}
