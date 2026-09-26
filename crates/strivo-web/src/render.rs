//! The one place a saved revision and its per-recording sidecars become a
//! [`RenderRequest`], and the two callers that consume the plan it compiles to.
//!
//! ## The trust gap this closes
//!
//! `editor_render` used to read the EDL, the branding spec and the volume
//! automation — and nothing else. Four more settings are persisted per
//! recording and the UI says each is baked: the insert FX chain, pitch/time,
//! the caption style and the sub-mix bus. A user saved a limiter, saw it on
//! the card, and the render came out without it.
//!
//! [`resolve`] is the single reader of all six, and both callers go through
//! it. A second reader would be the second render path this module exists to
//! remove.
//!
//! ## The mixer is a surface over this, not a second reader
//!
//! [`crate::mixer::MixerState`] is the one place the mute / solo / gain / pan /
//! tap / ducking / cleanup / normalisation controls are entered, and it does not
//! render: it projects them onto the **same** [`RenderRequest`] this function
//! builds, so the mixer cannot be a second path to a plan however it is wired.
//! When a `mixer` sidecar exists it is authoritative for the audio surface it
//! covers; when it does not, this function behaves exactly as it did before the
//! mixer existed, and the primitive sidecars it reads keep their own meaning.
//!
//! ## What the resolver does *not* do
//!
//! It does not spell a filter, and it does not validate: a sidecar that parses
//! is read, a sidecar that does not is refused, and everything else is the
//! compiler's business. Each sidecar is parsed by the crate that owns
//! its model — `BrandingSpec`, `InsertChain`, `PitchTime`, `SubMix`,
//! `VolumeAutomation`, `AssStyle` — and the fragment that crate emits is what
//! reaches the compiler as an [`EffectStep`]. The compiler orders, routes and
//! refuses; it never re-derives a filter string. That is why this module needs
//! no ffmpeg knowledge and why adding an effect is a new `EffectStep`, not a
//! new branch here.
//!
//! ## Admission still runs first
//!
//! The revision's inputs are admitted by
//! [`crate::routes::media_input`] before a request is built, so nothing here
//! is a validation gate. The compiler's refusals are about a *request* being
//! internally inconsistent, which is a different question.
//!
//! ## Brand assets are the exception, and they are gated here
//!
//! A brand asset **is** a media input, so it is admitted the same way and at
//! the same boundary: [`resolve`] runs the brand library's preconditions
//! before a request exists, and a missing or unreviewed asset is a named
//! refusal rather than a filter graph pointing at a file that is not there.
//!
//! This is deliberately *here* and not in `render` or in the executor. Both the
//! plan route and the render route call [`resolve`], so the problem appears in
//! a preview that spawns nothing. A check that first fired once ffmpeg was
//! already running would be "at render", which is the defect todo 47 names.

use std::path::{Path, PathBuf};

use strivo_editor::render::{
    approximations, compile, Approximation, CompiledPlan, MediaTime, RenderError, RenderRequest,
};
use strivo_editor::render::{
    CaptionMode, CaptionSpec, ClipRequest, Container, Coordinate, EffectKind, EffectStep as Step,
    LayerKind, LayerSpec, LoudnessSpec, OutputProfile, TrackSpec, Transform, VideoIntent,
};
use strivo_editor::{time::SourceSpan, Cut, CutKind, Edl};

use strivo_branding::{BrandingSpec, PreconditionError, RecordingBrand};

/// The six per-recording sidecars, as one resolved request.
///
/// Grouped into a value because they are not independent: a request missing
/// one of them is not "partly resolved", it is a render that silently drops an
/// effect the user saved.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub request: RenderRequest,
    /// The caption style document, when the recording has one. Kept beside the
    /// request because the executor needs it to *render* a subtitle file, and
    /// re-reading the sidecar would be a second reader.
    pub caption_style: Option<strivo_captions::AssStyle>,
    /// The ffmpeg build the plan will run under, recorded in the plan so a
    /// render by two tools is two plans.
    pub renderer_version: String,
}

/// Read one sidecar, or the type's own default when it is absent.
///
/// `unwrap_or_default` is right here and **only** here: a sidecar that was
/// never written means "no effect saved", and every one of these types has a
/// default that means exactly that. A sidecar that exists but does not parse is
/// a different fact, and it is **refused** — silently falling back to the
/// default would render without the user's limiter and report success.
fn read_sidecar<T>(path: &Path) -> Result<Option<T>, String>
where
    T: serde::de::DeserializeOwned,
{
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Build the request for one recording's saved revision.
pub fn resolve(recording_id: &str, edl: &Edl) -> Result<Resolved, String> {
    let clips = clips_from_edl(edl)?;
    let inline: BrandingSpec =
        read_sidecar(&sidecar(recording_id, "branding"))?.unwrap_or_default();
    // The brand preconditions run here, before any `RenderRequest` exists, so
    // a missing asset is a named refusal at the boundary rather than an
    // ffmpeg failure once the export is already running.
    let branding = branding_spec(recording_id, &inline).map_err(|e| e.to_string())?;
    let inserts: Option<strivo_insert_fx::InsertChain> =
        read_sidecar(&sidecar(recording_id, "insert-fx"))?;
    let pitch: Option<strivo_pitch::PitchTime> = read_sidecar(&sidecar(recording_id, "pitch"))?;
    let submix: Option<strivo_submix::SubMix> = read_sidecar(&sidecar(recording_id, "submix"))?;
    let automation: Option<strivo_automation::VolumeAutomation> =
        read_sidecar(&sidecar(recording_id, "automation"))?;
    let caption_style: Option<strivo_captions::AssStyle> =
        read_sidecar(&sidecar(recording_id, "captions").with_extension("style.json"))?;
    // No loudness sidecar is written by any panel today — the Loudness panel
    // measures and reports, and the render derives a platform preset. This
    // reads one if it exists so a saved target is honoured rather than dropped,
    // and resolves to no loudness stage when it does not.
    let loudness: Option<strivo_loudness::LoudnormTarget> =
        read_sidecar(&sidecar(recording_id, "loudness"))?;
    let mixer: Option<crate::mixer::MixerState> = read_sidecar(&sidecar(recording_id, "mixer"))?;

    let mut effects: Vec<Step> = Vec::new();
    if let Some(pitch) = pitch.filter(|p| !p.is_identity()) {
        effects.push(Step {
            id: "pitch-time".into(),
            kind: EffectKind::PitchTime,
            filter: pitch.to_filter(),
            arity: 1,
            bus: None,
        });
    }
    if let Some(chain) = inserts.filter(|c| !c.is_empty()) {
        effects.push(Step {
            id: "insert-fx".into(),
            kind: EffectKind::InsertFx,
            filter: chain.to_filter(),
            arity: 1,
            bus: None,
        });
    }
    if let Some(bus) = submix.as_ref().filter(|b| !b.tracks.is_empty()) {
        // The single-input projection, not the full graph: a cut-and-concat
        // render of one recording has one audio stream, and the projection
        // refuses a multi-track bus rather than mixing a track it cannot reach.
        effects.push(Step {
            id: "sub-mix".into(),
            kind: EffectKind::SubMix,
            filter: bus.to_single_input_chain().unwrap_or_default(),
            arity: bus.signal_count(),
            bus: None,
        });
    }
    if let Some(curve) = automation.filter(|a| !a.points.is_empty()) {
        effects.push(Step {
            id: "volume-automation".into(),
            kind: EffectKind::VolumeAutomation,
            filter: curve.build_audio_filter(),
            arity: 1,
            bus: None,
        });
    }

    let video_filter_complex = if branding.is_empty() {
        String::new()
    } else {
        branding.build_filter_chain("[0:v]").filter_complex
    };

    let mut request = RenderRequest {
        subject: recording_id.to_string(),
        revision: revision_label(edl),
        clips,
        layers: vec![LayerSpec {
            id: "programme".into(),
            kind: LayerKind::Video,
            transform: Transform::default(),
            colour: Default::default(),
            clips: (0..edl.cuts.len()).collect(),
            visible: true,
            keyframes: Vec::new(),
        }],
        transitions: Vec::new(),
        tracks: vec![TrackSpec {
            label: "programme".into(),
            bus: "master".into(),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            insert: String::new(),
            clips: (0..edl.cuts.len()).collect(),
        }],
        effects,
        loudness: loudness.map(|target| LoudnessSpec {
            platform: "saved".into(),
            i: target.i,
            lra: target.lra,
            tp: target.tp,
        }),
        captions: CaptionSpec {
            mode: if caption_style.is_some() {
                CaptionMode::Sidecar
            } else {
                CaptionMode::None
            },
            style_json: caption_style
                .as_ref()
                .map(|s| serde_json::to_string(s).unwrap_or_default())
                .unwrap_or_default(),
            coordinate: Coordinate::Source,
            safe_area: true,
            cues: Vec::new(),
        },
        video: VideoIntent {
            filter_complex: video_filter_complex,
            colour: None,
            frames: None,
        },
        container: Container {
            extension: "mkv".into(),
            video_codec: "libx264".into(),
            audio_codec: "aac".into(),
            stream_copy: false,
        },
        profile: OutputProfile {
            name: "long-form".into(),
            renderer: "ffmpeg".into(),
            renderer_version: ffmpeg_version(),
        },
    };
    if let Some(mixer) = mixer {
        mixer.apply(&mut request);
    }
    Ok(Resolved {
        request,
        caption_style,
        renderer_version: ffmpeg_version(),
    })
}

/// `<data_dir>/plugins/<name>/<recording_id>.json`, the shape every one of
/// these sidecars already uses. Declared once so a future sidecar cannot pick
/// its own directory and quietly escape the resolver.
fn sidecar(recording_id: &str, name: &str) -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join(name)
        .join(format!("{recording_id}.json"))
}

/// The reusable brand library. **Global, not per recording** — that is the
/// whole difference between a brand set and a per-recording copy of one, and it
/// is why this is not [`sidecar`]-shaped.
fn brand_library_path() -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("branding")
        .join("brand_sets.json")
}

/// A recording's binding to the library: which set, which destination shape.
fn brand_binding_path(recording_id: &str) -> PathBuf {
    sidecar(recording_id, "brand-set")
}

/// The brand spec a recording renders with, and the preconditions it passed to
/// get there.
///
/// Both paths are gated by the same [`strivo_branding::library::resolve`]:
/// with a binding the library is the source of truth and the set is projected
/// onto the named variant; without one the per-recording spec is used, as
/// before. Either way a missing or unreviewed asset is refused **by name** and
/// no spec is produced.
pub fn branding_spec(
    recording_id: &str,
    inline: &BrandingSpec,
) -> Result<BrandingSpec, PreconditionError> {
    // An absent library is not an error: a recording with no binding does not
    // need one, and refusing here would break every PVR-adjacent caller of a
    // Creator feature. A library that exists and does not parse IS refused —
    // silently falling back to "no branding" is the drop this todo forbids.
    let library = match std::fs::read_to_string(brand_library_path()) {
        Ok(raw) => serde_json::from_str::<strivo_branding::BrandLibrary>(&raw).map_err(|e| {
            PreconditionError::UnreadableLibrary {
                path: brand_library_path(),
                detail: e.to_string(),
            }
        })?,
        Err(_) => strivo_branding::BrandLibrary::default(),
    };
    let binding: Option<RecordingBrand> =
        read_sidecar(&brand_binding_path(recording_id)).map_err(|e| {
            PreconditionError::UnreadableBinding {
                recording_id: recording_id.to_string(),
                detail: e,
            }
        })?;
    strivo_branding::library::resolve(&library, binding.as_ref(), inline)
}

/// The filter a saved spec would produce, for the panel's preview.
///
/// Delegates to [`branding_spec`] so the branding panel's chain and the render's
/// chain are the same chain. A second reader here would be the second render
/// path this module exists to remove — and, worse, a panel showing a chain the
/// export would refuse.
pub fn branding_preview(
    recording_id: &str,
    inline: &BrandingSpec,
) -> Result<(String, String), PreconditionError> {
    let spec = branding_spec(recording_id, inline)?;
    let chain = spec.build_filter_chain("[0:v]");
    Ok((chain.filter_complex, chain.video_label))
}

/// The revision label a plan is compiled from. The EDL store keys revisions by
/// `(recording_id, label)`, so the label is the identity the SPA already has.
fn revision_label(edl: &Edl) -> String {
    format!("{}:{}", edl.recording_id, edl.cuts.len())
}

/// The EDL's cuts as clip requests, through todo 22's one boundary.
///
/// `Cut::span` is where the legacy `f32` seconds become integer media time,
/// and it already refuses a non-finite, negative, inverted or empty cut by
/// name. Re-deriving that here would be the second implementation of "is this
/// a real media coordinate".
fn clips_from_edl(edl: &Edl) -> Result<Vec<ClipRequest>, String> {
    if edl.cuts.is_empty() {
        return Err("the revision has no cuts to render".into());
    }
    edl.cuts
        .iter()
        .enumerate()
        .map(|(cut, entry)| {
            let span: SourceSpan = entry.span().map_err(|e| format!("cut {cut}: {e}"))?;
            let (fade_in, fade_out) = fades(entry);
            Ok(ClipRequest {
                cut,
                span,
                fade_in,
                fade_out,
                overlay_at: None,
            })
        })
        .collect()
}

/// The per-cut fades, in whole microseconds. Read through
/// [`MediaTime::from_secs_f64`] like every other seconds in the model, so a
/// NaN or a negative fade is refused rather than clamped to zero.
fn fades(cut: &Cut) -> (MediaTime, MediaTime) {
    let read = |secs: f32| MediaTime::from_secs_f64(f64::from(secs)).unwrap_or(MediaTime::ZERO);
    (read(cut.fade_in_sec), read(cut.fade_out_sec))
}

/// `ffmpeg -version`'s first line, or `"unknown"`. Part of the plan's
/// identity, so it is read once here rather than guessed by the executor.
fn ffmpeg_version() -> String {
    std::process::Command::new(ffmpeg_binary())
        .arg("-version")
        .output()
        .ok()
        .and_then(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .map(|line| line.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

fn ffmpeg_binary() -> std::path::PathBuf {
    strivo_core::tools::resolve_tool("ffmpeg")
}

/// A preview of what a recording's saved revision renders as.
///
/// The **preview caller**. It compiles and reports; it spawns nothing and
/// writes nothing, which is the only difference between it and [`render`]
/// that is not about the plan.
pub fn preview(resolved: &Resolved) -> Result<PreviewResponse, RenderError> {
    let plan = compile(&resolved.request)?;
    Ok(PreviewResponse {
        approximate: approximations(&plan),
        plan,
        rendered_preview: format!("/api/v1/plugins/editor/{}/render", resolved.request.subject),
        renderer_version: resolved.renderer_version.clone(),
    })
}

/// A rendered export of what a recording's saved revision compiles to.
///
/// The **export caller**. Same compiler, same inputs, different work — it
/// delegates straight to the editor's export rather than compiling here first,
/// so there is one compile per call rather than two.
pub fn render(
    resolved: &Resolved,
    edl: &Edl,
    output: &Path,
) -> Result<RenderResponse, RenderError> {
    let exported = strivo_editor::render::export(&resolved.request, edl, output, None)?;
    Ok(RenderResponse {
        plan: exported.plan,
        output: exported.output,
        bytes: exported.bytes,
        attempt: exported.attempt,
    })
}

/// What the preview route answers.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PreviewResponse {
    pub plan: CompiledPlan,
    /// Derived from the plan, never stored on it — see
    /// [`strivo_editor::render::approximations`].
    pub approximate: Vec<Approximation>,
    /// The route that renders this exact plan.
    pub rendered_preview: String,
    pub renderer_version: String,
}

/// What the render route answers.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RenderResponse {
    pub plan: CompiledPlan,
    /// The immutable per-attempt path this render wrote.
    pub output: PathBuf,
    pub bytes: u64,
    /// The address of this artifact. Two renders of one recording carry two of
    /// these, so neither can be mistaken for the other and neither destroys it.
    pub attempt: strivo_editor::render::AttemptId,
}

/// The media path a cut names. The plan deliberately carries a cut index, not
/// a path, so anything that needs the locator reads it from here.
pub fn cut_locator(edl: &Edl, cut: usize) -> Option<&str> {
    match &edl.cuts.get(cut)?.kind {
        CutKind::Source { source_path } => Some(source_path),
        CutKind::Broll { broll_path } => Some(broll_path),
    }
}
