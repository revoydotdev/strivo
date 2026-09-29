//! strivo-captions — caption file generation + translation interface.
//!
//! Two surfaces for the DAW-vision:
//!
//!   1. Format conversion: take Crunchr transcript segments
//!      (start_sec, end_sec, speaker, text) and emit publishable
//!      caption files — SRT (universal), VTT (web standard, with
//!      `<v Speaker>` tags), TXT (paste-into-show-notes).
//!   2. Translation: a [`Translator`] trait so backends are pluggable.
//!      The default [`IdentityTranslator`] is a no-op pass-through; a
//!      future iteration plugs in NLLB / Argos / OpenAI as needed.
//!      [`apply_translation`] takes a list of segments and a translator
//!      and returns the translated copy ready to format.
//!
//! Pure data; no IO; runs in tests without fixtures.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use strivo_branding::{BrandVariant, PreconditionError, SafeArea, SafeRect};
use strivo_editor::time::{Coordinate, Interval, IntervalMap, MediaTime, TimeError, Timed};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_sec: f32,
    pub end_sec: f32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// What a translator actually did, as a closed set.
///
/// The alternative is a boolean, and a boolean is what let a pass-through sit
/// behind a button labelled "translate". This is a value every backend must
/// name, so a backend that cannot honestly say `Translated` has to say
/// `Identity` and be labelled as such in the artifact and in the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationKind {
    /// The text is unchanged. Not a translation, and never presented as one.
    Identity,
    /// The text was produced by a real translation backend.
    Translated,
}

impl TranslationKind {
    /// The user-facing phrase. "Identity" alone is a word about the mechanism;
    /// the string a person reads has to say what happened to their text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Identity => "identity (no translation applied)",
            Self::Translated => "translated",
        }
    }
}

/// Which backend ran, and what it is honestly called.
///
/// Carried on the track and on every artifact this crate emits, so "translated"
/// is a claim a reader can check rather than a column nobody sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Translation {
    pub kind: TranslationKind,
    /// The backend's own name. `"identity"` for the pass-through.
    pub backend: String,
    pub source_lang: String,
    pub target_lang: String,
}

impl Translation {
    /// The label for a backend, in one place.
    #[must_use]
    pub fn describe(t: &dyn Translator) -> Self {
        Self {
            kind: t.kind(),
            backend: t.backend().to_string(),
            source_lang: t.source_lang().to_string(),
            target_lang: t.target_lang().to_string(),
        }
    }

    /// What a person reads about this export's language handling.
    ///
    /// Borrows: the identity case is a literal, and only a real backend's name
    /// is worth an allocation.
    #[must_use]
    pub fn label(&self) -> Cow<'_, str> {
        if self.kind == TranslationKind::Identity {
            return Cow::Borrowed(self.kind.label());
        }
        Cow::Owned(format!("{} by {}", self.kind.label(), self.backend))
    }
}

/// Pluggable translation backend. Implementations may be sync or
/// wrap an async runtime — the trait keeps things synchronous so the
/// formatter functions stay testable without tokio.
pub trait Translator: Send + Sync {
    /// Source language hint (e.g. `"en"`). May be empty when unknown.
    fn source_lang(&self) -> &str;
    /// Target language code (e.g. `"es"`, `"pt-BR"`, `"ja"`).
    fn target_lang(&self) -> &str;
    /// What this backend did. No default, on purpose: a defaulted `kind` is a
    /// backend silently presenting a pass-through as a translation.
    fn kind(&self) -> TranslationKind;
    /// The backend's own name, for the artifact's metadata.
    fn backend(&self) -> &str;
    /// Translate a single string. Implementations may chunk and batch
    /// however they like, as long as the contract holds.
    fn translate(&self, text: &str) -> anyhow::Result<String>;
}

/// No-op translator — returns the input verbatim. Useful as a default,
/// and as the test target for the format-conversion helpers.
pub struct IdentityTranslator;

impl Translator for IdentityTranslator {
    fn source_lang(&self) -> &str {
        "en"
    }
    fn target_lang(&self) -> &str {
        "en"
    }
    fn kind(&self) -> TranslationKind {
        TranslationKind::Identity
    }
    fn backend(&self) -> &str {
        "identity"
    }
    fn translate(&self, text: &str) -> anyhow::Result<String> {
        Ok(text.to_string())
    }
}

/// Apply a translator across a segment list. Failures on individual
/// segments are surfaced — the caller decides whether to retry or
/// continue with a partial result.
pub fn apply_translation(segments: &[Segment], t: &dyn Translator) -> anyhow::Result<Vec<Segment>> {
    let mut out = Vec::with_capacity(segments.len());
    for s in segments {
        out.push(Segment {
            start_sec: s.start_sec,
            end_sec: s.end_sec,
            text: t.translate(&s.text)?,
            speaker: s.speaker.clone(),
        });
    }
    Ok(out)
}

/// Format a duration as `HH:MM:SS,mmm` (SRT) or `HH:MM:SS.mmm` (VTT).
fn fmt_time(sec: f32, sep: char) -> String {
    let ms = (sec.max(0.0) * 1000.0).round() as u64;
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1000) % 60;
    let r = ms % 1000;
    format!("{h:02}:{m:02}:{s:02}{sep}{r:03}")
}

/// Emit a SubRip (.srt) caption file. Numbered entries, comma-decimal
/// timestamps, speaker label prefixed when present.
pub fn to_srt(segments: &[Segment]) -> String {
    let mut out = String::new();
    for (i, s) in segments.iter().enumerate() {
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            fmt_time(s.start_sec, ','),
            fmt_time(s.end_sec, ',')
        ));
        let line = match &s.speaker {
            Some(spk) if !spk.is_empty() => format!("[{spk}] {}", s.text.trim()),
            _ => s.text.trim().to_string(),
        };
        out.push_str(&line);
        out.push_str("\n\n");
    }
    out
}

/// Emit a WebVTT (.vtt) file. VTT uses `.` for the decimal separator
/// and supports `<v Speaker>text` voice tags which players render with
/// styling. Header is the mandatory `WEBVTT` line.
pub fn to_vtt(segments: &[Segment]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for (i, s) in segments.iter().enumerate() {
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            fmt_time(s.start_sec, '.'),
            fmt_time(s.end_sec, '.')
        ));
        let line = match &s.speaker {
            Some(spk) if !spk.is_empty() => format!("<v {spk}>{}", s.text.trim()),
            _ => s.text.trim().to_string(),
        };
        out.push_str(&line);
        out.push_str("\n\n");
    }
    out
}

/// Styled caption export options — surfaces in the Editor "Styled
/// subtitles" panel and persists with the recording. Designed for the
/// YouTube Shorts / TikTok / Reels cut where styled captions drive
/// watch-time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssStyle {
    /// Title in the ASS header.
    pub title: String,
    /// Default font face. Picks one that the player is likely to have;
    /// system fallback resolves the missing-font case.
    pub font: String,
    /// Default font size in script-resolution points.
    pub font_size: u32,
    /// Outline thickness in pixels. Heavy outline = readable over busy
    /// video; lower outline = aesthetic.
    pub outline_px: f32,
    /// Drop-shadow distance in pixels.
    pub shadow_px: f32,
    /// 1..=9 numpad-style alignment. 2 = bottom-centre (default).
    pub alignment: u8,
    /// Bottom margin in pixels. Shorts / Reels usually want 80-120 to
    /// avoid the UI overlays.
    pub margin_v_px: u32,
    /// Optional per-speaker colour map — speaker label → ASS BGR hex
    /// (e.g. "FFFFFF" for white, "00FFFF" for yellow). When present,
    /// each cue gets the speaker's colour via a `{\c}` override.
    #[serde(default)]
    pub speaker_colors: std::collections::BTreeMap<String, String>,
}

impl Default for AssStyle {
    fn default() -> Self {
        Self {
            title: "StriVo styled captions".into(),
            font: "Inter".into(),
            font_size: 56,
            outline_px: 2.0,
            shadow_px: 1.5,
            alignment: 2,
            margin_v_px: 80,
            speaker_colors: Default::default(),
        }
    }
}

/// One word with its sub-segment timing — drives karaoke-style `\k`
/// highlight in ASS dialogue lines. Optional; absent karaoke = the
/// whole cue text appears at once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordTiming {
    pub text: String,
    pub start_sec: f32,
    pub end_sec: f32,
}

/// A caption cue enriched with optional per-word timing for karaoke
/// rendering. The plain SRT/VTT/TXT paths still work off the simpler
/// [`Segment`] shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KaraokeSegment {
    pub start_sec: f32,
    pub end_sec: f32,
    pub speaker: Option<String>,
    pub words: Vec<WordTiming>,
}

/// Format an ASS time as `H:MM:SS.cc` (centiseconds, single-digit hour).
fn fmt_ass_time(sec: f32) -> String {
    let cs = (sec.max(0.0) * 100.0).round() as u64;
    let h = cs / 360_000;
    let m = (cs / 6000) % 60;
    let s = (cs / 100) % 60;
    let c = cs % 100;
    format!("{h}:{m:02}:{s:02}.{c:02}")
}

/// Emit an Advanced SubStation Alpha (.ass) caption file. ASS supports
/// per-cue colour, outline, drop shadow, positioning, and the karaoke
/// `\k` highlight tag — everything you need for YouTube Shorts /
/// TikTok / Reels-style "bold readable captions".
///
/// Pass a `karaoke` list when word-level timing is available; the cue
/// for each segment is built from the matching `KaraokeSegment` if its
/// start/end roughly aligns (within 0.05s). Cues with no karaoke fall
/// back to the segment's plain text.
pub fn to_ass(segments: &[Segment], style: &AssStyle, karaoke: &[KaraokeSegment]) -> String {
    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str(&format!("Title: {}\n", ass_escape(&style.title)));
    out.push_str("ScriptType: v4.00+\n");
    out.push_str("PlayResX: 1920\nPlayResY: 1080\n");
    out.push_str("ScaledBorderAndShadow: yes\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    // ASS colours are &HAABBGGRR (alpha + BGR). White text, black
    // outline, 50% black shadow — classic high-contrast preset.
    out.push_str(&format!(
        "Style: Default,{font},{size},&H00FFFFFF,&H000000FF,&H00000000,&H7F000000,1,0,0,0,100,100,0,0,1,{outline:.2},{shadow:.2},{align},20,20,{marginv},1\n\n",
        font = style.font,
        size = style.font_size,
        outline = style.outline_px,
        shadow = style.shadow_px,
        align = style.alignment,
        marginv = style.margin_v_px,
    ));
    out.push_str("[Events]\n");
    out.push_str(
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    // Build a fast lookup so we can match karaoke entries by their
    // (rounded) start time without an O(n*m) scan.
    let kar_by_start: std::collections::HashMap<u32, &KaraokeSegment> = karaoke
        .iter()
        .map(|k| ((k.start_sec * 100.0).round() as u32, k))
        .collect();
    for s in segments {
        let key = (s.start_sec * 100.0).round() as u32;
        let kar = kar_by_start
            .get(&key)
            .filter(|k| (k.end_sec - s.end_sec).abs() < 0.05);
        let speaker_override = match &s.speaker {
            Some(spk) => style
                .speaker_colors
                .get(spk)
                .map(|hex| format!("{{\\c&H00{hex}&}}", hex = hex.to_uppercase())),
            None => None,
        }
        .unwrap_or_default();
        let text = if let Some(k) = kar {
            karaoke_text(k, &speaker_override)
        } else {
            format!("{}{}", speaker_override, ass_escape(s.text.trim()))
        };
        let line = format!(
            "Dialogue: 0,{start},{end},Default,,0,0,0,,{text}\n",
            start = fmt_ass_time(s.start_sec),
            end = fmt_ass_time(s.end_sec),
        );
        out.push_str(&line);
    }
    out
}

fn karaoke_text(seg: &KaraokeSegment, color_prefix: &str) -> String {
    // ASS \k takes centisecond durations.
    let mut out = String::new();
    out.push_str(color_prefix);
    for w in &seg.words {
        let cs = ((w.end_sec - w.start_sec).max(0.0) * 100.0).round() as u32;
        out.push_str(&format!("{{\\k{cs}}}{} ", ass_escape(w.text.trim())));
    }
    out.trim_end().to_string()
}

/// Minimal ASS escape — guard the two characters that break dialogue
/// parsing: `\N` is the line-break sequence (we keep newlines literal
/// by replacing them), and `{` opens override blocks.
fn ass_escape(s: &str) -> String {
    s.replace('\n', "\\N")
        .replace('{', "\\{")
        .replace('}', "\\}")
}

/// Plain-text export — drops timestamps + tags, useful for show-notes
/// drafts or to feed a separate summarisation pipeline. Keeps speaker
/// labels because they read well in markdown.
pub fn to_txt(segments: &[Segment]) -> String {
    let mut out = String::new();
    for s in segments {
        match &s.speaker {
            Some(spk) if !spk.is_empty() => out.push_str(&format!("{spk}: ")),
            _ => {}
        }
        out.push_str(s.text.trim());
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Captions as revisioned timeline content
// ---------------------------------------------------------------------------

/// A cue's identity, assigned by whoever created it and carried unchanged by
/// every later edit.
///
/// Never derived from the text, for the reason `strivo_editor::transcript`'s
/// `WordId` gives: caption text is model-produced and reaches a user interface,
/// so an identity matched on it would collapse two cues that read alike and
/// would let markup in a cue change which cue it is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CueId(String);

impl CueId {
    /// Longest accepted id, in bytes. A cue id arrives from a caller, so it is
    /// a trust boundary; an unbounded one is a denial-of-service vector wearing
    /// an identifier.
    pub const MAX_BYTES: usize = 256;

    pub fn new(raw: &str) -> Result<Self, CaptionError> {
        if raw.is_empty() {
            return Err(CaptionError::CueIdInvalid {
                detail: "empty".into(),
            });
        }
        if raw.trim().is_empty() {
            return Err(CaptionError::CueIdInvalid {
                detail: "whitespace only".into(),
            });
        }
        if raw.len() > Self::MAX_BYTES {
            return Err(CaptionError::CueIdInvalid {
                detail: "longer than 256 bytes".into(),
            });
        }
        if raw.chars().any(char::is_control) {
            return Err(CaptionError::CueIdInvalid {
                detail: "contains a control character".into(),
            });
        }
        Ok(Self(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One caption cue, as stored.
///
/// `start` is a [`Timed`], not a `MediaTime`, so **a cue cannot hold a
/// coordinate without naming the clock it is on** — the declaration report §7.2
/// asks of every timestamped thing. There is no string flag and no default: the
/// clock is part of the coordinate's type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionCue {
    pub id: CueId,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// Where the cue starts, with its clock declared.
    pub start: Timed,
    /// How long it reads for, on the same clock as `start`.
    pub duration: MediaTime,
}

impl CaptionCue {
    /// A cue positioned in **source** time — the ordinary case, because a
    /// transcript times words against the media they were spoken over.
    pub fn in_source(
        id: &str,
        text: &str,
        speaker: Option<&str>,
        start: MediaTime,
        duration: MediaTime,
    ) -> Result<Self, CaptionError> {
        Self::build(id, text, speaker, Timed::source(start), duration)
    }

    /// A cue positioned in **composition** time — a caption the creator placed
    /// by eye against the finished programme, which no cut may move.
    pub fn in_composition(
        id: &str,
        text: &str,
        speaker: Option<&str>,
        start: MediaTime,
        duration: MediaTime,
    ) -> Result<Self, CaptionError> {
        Self::build(id, text, speaker, Timed::composition(start), duration)
    }

    fn build(
        id: &str,
        text: &str,
        speaker: Option<&str>,
        start: Timed,
        duration: MediaTime,
    ) -> Result<Self, CaptionError> {
        if duration.micros() == 0 {
            return Err(CaptionError::EmptyCue {
                id: CueId::new(id)?.to_string(),
            });
        }
        Ok(Self {
            id: CueId::new(id)?,
            text: text.to_string(),
            speaker: speaker.map(str::to_string),
            start,
            duration,
        })
    }
}

/// The caption content of one revision: the editable half.
///
/// One struct, not a document schema and not a second store. It carries
/// `base_revision` for the same reason `TranscriptDraft` does — a caption edit
/// that names no immutable base is an orphan, and todo 49's history stores the
/// track verbatim on the same entry as the composition, so one undo crosses
/// both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionTrack {
    pub base_revision: String,
    pub cues: Vec<CaptionCue>,
    /// The style every cue is drawn with. Style is a property of the track, not
    /// of a cue: an override per cue would be a second style model for a case
    /// no export needs.
    pub style: AssStyle,
    /// What was done to the text, and by what. Never inferred from the langs.
    pub translation: Translation,
}

impl CaptionTrack {
    /// An empty track over a base revision, drawn with the default style and
    /// labelled with what the backend actually was.
    #[must_use]
    pub fn new(base_revision: &str, translator: &dyn Translator) -> Self {
        Self {
            base_revision: base_revision.to_string(),
            cues: Vec::new(),
            style: AssStyle::default(),
            translation: Translation::describe(translator),
        }
    }
}

/// Everything a caption export refuses.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptionError {
    /// The time model's own refusal, passed through rather than restated.
    Time(TimeError),
    /// An id that could not be a stable key.
    CueIdInvalid { detail: String },
    /// A cue with no length. It reads nothing and has no interval to place.
    EmptyCue { id: String },
    /// Two cues under one id: two answers to "which cue", so the batch is
    /// refused rather than deduplicated by position.
    DuplicateCue { id: String },
    /// The composition has no source behind this cue — a cut ran through it.
    ///
    /// **Refused, not clamped.** Todo 36's shape: the whole batch fails, so no
    /// caption file is written at all, and the cue that would have been drawn
    /// across the cut is still there to be fixed on the next read.
    CueDeleted { id: String, at: i64 },
    /// The brand set could not be projected onto the frame.
    Brand(PreconditionError),
}

impl std::fmt::Display for CaptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Time(e) => write!(f, "caption time: {e}"),
            Self::CueIdInvalid { detail } => write!(f, "cue id is not a usable key: {detail}"),
            Self::EmptyCue { id } => write!(f, "caption cue {id} has no length"),
            Self::DuplicateCue { id } => write!(f, "two caption cues are both {id}"),
            Self::CueDeleted { id, at } => write!(
                f,
                "caption cue {id} sits at {at} us of source, which the composition deleted"
            ),
            Self::Brand(e) => write!(f, "caption safe area: {e}"),
        }
    }
}

impl std::error::Error for CaptionError {}

impl From<TimeError> for CaptionError {
    fn from(error: TimeError) -> Self {
        Self::Time(error)
    }
}

impl From<PreconditionError> for CaptionError {
    fn from(error: PreconditionError) -> Self {
        Self::Brand(error)
    }
}

/// A track resolved onto a composition: every cue on the output timeline.
///
/// The whole point of the type. A [`CaptionTrack`] is an *edit* and says nothing
/// about where its cues land; this is the export view, and it is only reachable
/// through [`resolve`], so no caller can read a source-time cue as if it were a
/// composition-time one.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCaptions {
    /// Composition-time segments, in output order. One entry per composition
    /// position the cue reaches, so a repeated clip keeps both.
    pub segments: Vec<Segment>,
    /// The style, already inside the frame's safe rect.
    pub style: AssStyle,
    /// The safe rect the style was fitted to.
    pub safe_rect: SafeRect,
    pub translation: Translation,
}

/// The boundary from integer media time to the `f32` seconds the caption
/// formats are written in.
///
/// Lossy, and the loss is a property of the formats rather than of this
/// function: SRT and VTT quantise to whole milliseconds and ASS to whole
/// centiseconds, so a microsecond coordinate cannot survive either. Rounding
/// here to the millisecond is therefore the *finest* any of them can carry, and
/// the agreement between a sidecar and a burn is bounded by the coarser of the
/// two, not by anything this crate chose.
fn to_secs(at: MediaTime) -> f32 {
    (at.micros() as f64 / 1_000.0).round() as f32 / 1_000.0
}

/// Resolve a track onto a composition, or refuse the whole track.
///
/// The one place a caption's timing is derived, and it derives it from the
/// **composition**: a source-time cue is placed through the interval map, so a
/// cut moves it and a speed change shortens it. A composition-time cue is taken
/// as authored, because a caption the creator placed against the finished
/// programme is the one thing no cut is allowed to move.
///
/// Atomic: the first cue with no composition span fails the call, and the
/// caller therefore writes nothing. There is no partial export.
pub fn resolve(
    track: &CaptionTrack,
    map: &IntervalMap,
    safe: SafeArea,
    variant: &BrandVariant,
) -> Result<ResolvedCaptions, CaptionError> {
    let mut seen = std::collections::BTreeSet::new();
    let mut segments = Vec::with_capacity(track.cues.len());
    for cue in &track.cues {
        if !seen.insert(cue.id.clone()) {
            return Err(CaptionError::DuplicateCue {
                id: cue.id.to_string(),
            });
        }
        let source_range = Interval::new(
            cue.start.at,
            cue.start
                .at
                .checked_add(cue.duration)
                .map_err(CaptionError::Time)?,
        )?;
        match cue.start.coordinate {
            Coordinate::Source => {
                for output in map
                    .output_spans_of(source_range)
                    .map_err(|error| match error {
                        TimeError::Unmapped { .. } => CaptionError::CueDeleted {
                            id: cue.id.to_string(),
                            at: cue.start.at.micros(),
                        },
                        other => CaptionError::Time(other),
                    })?
                {
                    segments.push(Segment {
                        start_sec: to_secs(output.start()),
                        end_sec: to_secs(output.end()),
                        text: cue.text.clone(),
                        speaker: cue.speaker.clone(),
                    });
                }
            }
            Coordinate::Composition => {
                // A composition-time cue's end must still be inside the output;
                // one that runs past it is a cue longer than the programme.
                if cue
                    .start
                    .at
                    .checked_add(cue.duration)
                    .map_err(CaptionError::Time)?
                    > map.output_duration()
                {
                    return Err(CaptionError::CueDeleted {
                        id: cue.id.to_string(),
                        at: cue.start.at.micros(),
                    });
                }
                segments.push(Segment {
                    start_sec: to_secs(cue.start.at),
                    end_sec: to_secs(
                        cue.start
                            .at
                            .checked_add(cue.duration)
                            .map_err(CaptionError::Time)?,
                    ),
                    text: cue.text.clone(),
                    speaker: cue.speaker.clone(),
                });
            }
        }
    }
    segments.sort_by(|a, b| {
        a.start_sec
            .partial_cmp(&b.start_sec)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let safe_rect = safe.rect(variant)?;
    Ok(ResolvedCaptions {
        segments,
        style: style_within_safe_area(&track.style, safe_rect),
        safe_rect,
        translation: track.translation.clone(),
    })
}

/// Raise the style's bottom margin to the safe rect's bottom inset.
///
/// `MarginV` in ASS is measured up from the frame's bottom edge, so the safe
/// rect's own bottom inset is exactly the margin a centred caption needs. The
/// authored margin is kept when it is already larger — a creator who asked for
/// 120px to clear a Shorts UI overlay still gets 120px.
#[must_use]
pub fn style_within_safe_area(style: &AssStyle, safe: SafeRect) -> AssStyle {
    let mut out = style.clone();
    out.margin_v_px = out.margin_v_px.max(safe.y);
    out
}

/// Where a caption's text band lands, in output pixels, for a given alignment.
///
/// **The preview's geometry, measured rather than asserted.** It reads the same
/// two numbers the ASS style carries — `MarginV` and `font_size` — so a preview
/// drawn from it and a render made from it cannot disagree about where the text
/// is. Width is the safe rect's, because that is the widest a centred caption
/// can be without leaving it.
#[must_use]
pub fn preview_band(style: &AssStyle, safe: SafeRect) -> SafeRect {
    // ASS alignment 1..3 is a bottom row, 4..6 a middle row, 7..9 a top row.
    let height = style.font_size.max(1);
    let y = match style.alignment {
        0 => safe.y,
        4..=6 => safe.y + safe.height / 2,
        _ => safe.y + safe.height - height,
    };
    SafeRect {
        x: safe.x,
        y,
        width: safe.width,
        height: height.min(safe.height),
    }
}

/// Whether a band leaves the safe rect. False for a band that fits.
#[must_use]
pub fn clips_safe_area(band: SafeRect, safe: SafeRect) -> bool {
    band.x < safe.x
        || band.y < safe.y
        || band.x.saturating_add(band.width) > safe.x.saturating_add(safe.width)
        || band.y.saturating_add(band.height) > safe.y.saturating_add(safe.height)
}

/// Every caption file for one resolved track.
///
/// The sidecars and the burn come out of the **same** resolved list, so
/// "do the .srt and the burned-in text agree" is a question about two encoders
/// of one list rather than about two pipelines. It is still worth measuring —
/// see `tests/sidecar_burn_agreement.rs` — because the two formats quantise
/// differently and agreement is a property, not an accident.
#[derive(Debug, Clone, PartialEq)]
pub struct Sidecars {
    pub srt: String,
    pub vtt: String,
    /// The burned-in document. Same cues, same clock as the sidecars.
    pub ass: String,
    pub txt: String,
    pub translation: Translation,
    pub safe_rect: SafeRect,
}

impl Sidecars {
    /// The label to record beside the files.
    #[must_use]
    pub fn translation_label(&self) -> String {
        self.translation.label().into_owned()
    }
}

/// Emit the sidecars and the burn from one resolved track.
#[must_use]
pub fn sidecars(resolved: &ResolvedCaptions) -> Sidecars {
    Sidecars {
        srt: to_srt(&resolved.segments),
        vtt: to_vtt(&resolved.segments),
        ass: to_ass(&resolved.segments, &resolved.style, &[]),
        txt: to_txt(&resolved.segments),
        translation: resolved.translation.clone(),
        safe_rect: resolved.safe_rect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f32, end: f32, text: &str, spk: Option<&str>) -> Segment {
        Segment {
            start_sec: start,
            end_sec: end,
            text: text.into(),
            speaker: spk.map(|s| s.into()),
        }
    }

    struct UpperTranslator;
    impl Translator for UpperTranslator {
        fn source_lang(&self) -> &str {
            "en"
        }
        fn target_lang(&self) -> &str {
            "en-shout"
        }
        fn kind(&self) -> TranslationKind {
            TranslationKind::Translated
        }
        fn backend(&self) -> &str {
            "upper"
        }
        fn translate(&self, text: &str) -> anyhow::Result<String> {
            Ok(text.to_uppercase())
        }
    }

    #[test]
    fn srt_emits_numbered_entries_with_comma_decimal() {
        let segs = vec![
            seg(1.0, 5.5, "hello world", None),
            seg(6.0, 10.123, "second line", Some("Alice")),
        ];
        let s = to_srt(&segs);
        assert!(s.contains("00:00:01,000 --> 00:00:05,500"), "got:\n{s}");
        assert!(s.contains("00:00:06,000 --> 00:00:10,123"));
        assert!(s.contains("1\n"));
        assert!(s.contains("2\n"));
        assert!(s.contains("hello world"));
        assert!(s.contains("[Alice] second line"));
    }

    #[test]
    fn vtt_emits_header_dot_decimal_and_voice_tag() {
        let segs = vec![seg(0.0, 3.0, "ok", Some("Bob"))];
        let s = to_vtt(&segs);
        assert!(s.starts_with("WEBVTT\n"));
        assert!(s.contains("00:00:00.000 --> 00:00:03.000"));
        assert!(s.contains("<v Bob>ok"));
    }

    #[test]
    fn txt_drops_timestamps_keeps_speakers() {
        let segs = vec![
            seg(0.0, 1.0, "hi", Some("Alice")),
            seg(1.0, 2.0, "bye", None),
        ];
        let s = to_txt(&segs);
        assert!(s.contains("Alice: hi"));
        assert!(s.contains("bye"));
        assert!(!s.contains("00:00"));
    }

    #[test]
    fn fmt_time_handles_hours() {
        // 1h 2m 3s + 456ms
        let t = 3600.0 + 120.0 + 3.0 + 0.456;
        assert_eq!(fmt_time(t, ','), "01:02:03,456");
    }

    #[test]
    fn fmt_time_clamps_negative_to_zero() {
        assert_eq!(fmt_time(-5.0, ','), "00:00:00,000");
    }

    #[test]
    fn identity_translator_round_trips_text() {
        let segs = vec![seg(0.0, 1.0, "hola", None)];
        let out = apply_translation(&segs, &IdentityTranslator).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "hola");
    }

    #[test]
    fn upper_translator_transforms_each_segment() {
        let segs = vec![
            seg(0.0, 1.0, "hello", None),
            seg(1.0, 2.0, "world", Some("Alice")),
        ];
        let out = apply_translation(&segs, &UpperTranslator).unwrap();
        assert_eq!(out[0].text, "HELLO");
        assert_eq!(out[1].text, "WORLD");
        // Speaker should pass through untranslated.
        assert_eq!(out[1].speaker.as_deref(), Some("Alice"));
    }

    #[test]
    fn empty_segments_yield_empty_outputs() {
        assert_eq!(to_srt(&[]), "");
        assert_eq!(to_vtt(&[]), "WEBVTT\n\n");
        assert_eq!(to_txt(&[]), "");
    }

    fn kar(start: f32, end: f32, spk: Option<&str>, words: &[(&str, f32, f32)]) -> KaraokeSegment {
        KaraokeSegment {
            start_sec: start,
            end_sec: end,
            speaker: spk.map(|s| s.into()),
            words: words
                .iter()
                .map(|(t, s, e)| WordTiming {
                    text: (*t).into(),
                    start_sec: *s,
                    end_sec: *e,
                })
                .collect(),
        }
    }

    #[test]
    fn ass_header_carries_style_knobs() {
        let style = AssStyle {
            font: "Helvetica".into(),
            font_size: 72,
            outline_px: 3.0,
            shadow_px: 2.0,
            margin_v_px: 120,
            ..Default::default()
        };
        let s = to_ass(&[], &style, &[]);
        assert!(s.contains("Title: StriVo styled captions"));
        assert!(s.contains("Helvetica,72,"));
        assert!(s.contains(",3.00,2.00,2,20,20,120,"));
        assert!(s.contains("PlayResX: 1920"));
    }

    #[test]
    fn ass_dialogue_line_emits_centisecond_timestamps() {
        let segs = vec![seg(1.0, 5.5, "hello world", None)];
        let s = to_ass(&segs, &AssStyle::default(), &[]);
        assert!(s.contains("Dialogue: 0,0:00:01.00,0:00:05.50,Default,,0,0,0,,hello world"));
    }

    #[test]
    fn ass_per_speaker_color_overrides_cue() {
        let mut style = AssStyle::default();
        style.speaker_colors.insert("Alice".into(), "00FFFF".into()); // yellow (BGR)
        let segs = vec![seg(0.0, 1.0, "hi", Some("Alice"))];
        let s = to_ass(&segs, &style, &[]);
        // Cue should carry the colour override before the text.
        assert!(s.contains(r"{\c&H0000FFFF&}hi"));
    }

    #[test]
    fn ass_karaoke_emits_per_word_k_tags() {
        let segs = vec![seg(0.0, 2.0, "hello world", None)];
        let k = vec![kar(
            0.0,
            2.0,
            None,
            &[("hello", 0.0, 0.5), ("world", 0.5, 2.0)],
        )];
        let s = to_ass(&segs, &AssStyle::default(), &k);
        // hello = 50cs, world = 150cs.
        assert!(s.contains(r"{\k50}hello"));
        assert!(s.contains(r"{\k150}world"));
    }

    #[test]
    fn ass_karaoke_falls_back_to_plain_text_on_mismatch() {
        let segs = vec![seg(0.0, 2.0, "hello world", None)];
        // Karaoke entry with mismatched end time → not used.
        let k = vec![kar(0.0, 3.0, None, &[("hello", 0.0, 0.5)])];
        let s = to_ass(&segs, &AssStyle::default(), &k);
        assert!(s.contains(",hello world"));
        assert!(!s.contains(r"{\k"));
    }

    #[test]
    fn ass_escape_handles_braces_and_newlines() {
        let segs = vec![seg(0.0, 1.0, "use {0} and\n{1}", None)];
        let s = to_ass(&segs, &AssStyle::default(), &[]);
        // Newline → \N; braces → escaped.
        assert!(s.contains(r"use \{0\} and\N\{1\}"));
    }

    #[test]
    fn ass_empty_segments_still_produce_valid_header() {
        let s = to_ass(&[], &AssStyle::default(), &[]);
        assert!(s.contains("[Script Info]"));
        assert!(s.contains("[V4+ Styles]"));
        assert!(s.contains("[Events]"));
        // No dialogue lines.
        assert!(!s.contains("\nDialogue:"));
    }

    #[test]
    fn ass_combines_color_and_karaoke_on_same_cue() {
        let mut style = AssStyle::default();
        style.speaker_colors.insert("Bob".into(), "0080FF".into());
        let segs = vec![seg(0.0, 1.0, "go go", Some("Bob"))];
        let k = vec![kar(
            0.0,
            1.0,
            Some("Bob"),
            &[("go", 0.0, 0.5), ("go", 0.5, 1.0)],
        )];
        let s = to_ass(&segs, &style, &k);
        assert!(s.contains(r"{\c&H000080FF&}{\k50}go {\k50}go"));
    }

    #[test]
    fn fmt_ass_time_handles_centisecond_rounding() {
        assert_eq!(fmt_ass_time(0.0), "0:00:00.00");
        assert_eq!(fmt_ass_time(3666.789), "1:01:06.79");
    }

    #[test]
    fn trims_whitespace_around_text() {
        let segs = vec![seg(0.0, 1.0, "   hello   ", None)];
        let s = to_srt(&segs);
        // The line should be "hello", not "   hello   "
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines[2], "hello");
    }

    #[test]
    fn the_identity_translator_is_labelled_identity_and_not_as_a_translation() {
        let t = Translation::describe(&IdentityTranslator);
        assert_eq!(t.kind, TranslationKind::Identity);
        assert_eq!(t.backend, "identity");
        assert_eq!(t.target_lang, t.source_lang);
        let label = t.label();
        assert!(label.contains("identity"), "got: {label}");
        assert!(label.contains("no translation"), "got: {label}");
        assert!(!label.contains("translated by"), "got: {label}");
    }

    #[test]
    fn a_track_built_on_the_identity_translator_carries_that_labelling() {
        let track = CaptionTrack::new("rev-1", &IdentityTranslator);
        assert_eq!(track.translation.kind, TranslationKind::Identity);
        let body = serde_json::to_string(&track).expect("the track serialises");
        assert!(
            body.contains(r#""kind":"identity""#),
            "the label travels in the document, not only in the UI: {body}"
        );
    }

    #[test]
    fn a_real_backend_is_labelled_as_translated_and_names_itself() {
        let t = Translation::describe(&UpperTranslator);
        assert_eq!(t.kind, TranslationKind::Translated);
        assert_eq!(t.label(), "translated by upper");
    }

    #[test]
    fn a_track_cannot_be_built_from_a_translator_that_will_not_say_what_it_did() {
        // There is no default for `Translator::kind`, so this is a compile-time
        // property rather than a runtime one. The runtime half of the claim is
        // that the label is a function of the backend, not of the langs: two
        // backends with the same `en`→`en` languages are labelled differently.
        assert_ne!(
            Translation::describe(&IdentityTranslator).label(),
            Translation::describe(&UpperTranslator).label()
        );
    }
}
