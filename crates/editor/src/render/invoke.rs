//! Turning a compiled plan into ffmpeg arguments.
//!
//! Split out of [`super::execute`] so this file is the whole of "what command
//! does this plan run" and can be asserted on without a media file, a media
//! tool, or a scratch directory. The rules it follows:
//!
//! * **a bus gain belongs to the clip, not to the programme.** A track is a
//!   property of the clips routed to it, so its gain and pan go on the
//!   per-clip sub-render. Applying them after the concat would apply one
//!   track's gain to everything.
//! * **the audio chain runs in the order the compiler fixed** and nowhere
//!   else, so the plan's `audio_chain` order is the executed order.
//! * **`-c copy` only when there is genuinely nothing to do.** Asked-for
//!   rather than assumed: a plan with any audio stage, any fade, a video
//!   filter or a loudness pass always transcodes, so a filter can never be
//!   silently dropped onto the copy path.

use std::path::Path;

use super::plan::{Clip, CompiledPlan};
use super::RenderError;
use crate::time::MediaTime;

/// ffmpeg's argument parser takes decimal seconds. The **only** place a
/// [`MediaTime`] becomes a float, and it does not leave this module — todo 22's
/// integer is the model, this is the boundary.
fn secs(time: MediaTime) -> String {
    format!("{:.6}", time.micros() as f64 / 1_000_000.0)
}

fn io(detail: impl std::fmt::Display) -> RenderError {
    RenderError::Execute {
        detail: detail.to_string(),
    }
}

/// The video fade filter for one clip, or empty when it has no fades.
#[must_use]
pub fn fade_filter(clip: &Clip) -> String {
    let (Ok(length), zero, nonzero) = (
        clip.output.duration(),
        clip.fade_in.micros() == 0,
        clip.fade_out.micros() == 0,
    ) else {
        return String::new();
    };
    if zero && nonzero {
        return String::new();
    }
    let mut parts = Vec::new();
    if !zero {
        parts.push(format!("fade=t=in:st=0:d={}", secs(clip.fade_in)));
    }
    if !nonzero {
        let start = (length.micros() - clip.fade_out.micros()).max(0);
        parts.push(format!(
            "fade=t=out:st={:.6}:d={}",
            start as f64 / 1_000_000.0,
            secs(clip.fade_out)
        ));
    }
    parts.join(",")
}

/// The audio filter for the track a clip is routed to, or empty when the track
/// is at unity. An inaudible track is a hard silence, stated rather than
/// skipped.
///
/// Reads [`Track::audible`](super::plan::Track::audible) and not `muted`/`solo`
/// separately: the compiler already resolved the interaction, so a solo button
/// cannot be honoured here and ignored there. `volume=-inf` rather than a
/// dropped chain, because a dropped chain is indistinguishable from a track
/// that was never routed.
#[must_use]
pub fn track_filter(plan: &CompiledPlan, clip: usize) -> String {
    let Some(track) = plan.tracks.iter().find(|t| t.clips.contains(&clip)) else {
        return String::new();
    };
    if !track.audible {
        // `volume=0`, not `volume=-inf`. ffmpeg's `volume` filter evaluates
        // `-inf` dB to a non-finite multiplier, and the AAC encoder then refuses
        // the stream outright — `Input contains (near) NaN/+-Inf`, exit 234 — so
        // a muted track could not be rendered at all. A linear zero is the same
        // silence and encodes to exact zeros. Found by decoding the artifact: an
        // exit-code or file-size check would have reported a mute as working.
        return "volume=0".to_string();
    }
    let mut parts = Vec::new();
    // The tap first: it is this track's own signal being conditioned, so it
    // runs before the trim that sets the track's place in the mix.
    if !track.insert.trim().is_empty() {
        parts.push(track.insert.clone());
    }
    if track.gain_db.abs() > 1e-6 {
        parts.push(format!("volume={:.2}dB", track.gain_db));
    }
    if track.pan.abs() > 1e-6 {
        let pan = track.pan.clamp(-1.0, 1.0);
        parts.push(format!(
            "pan=stereo|c0={:.4}*c0|c1={:.4}*c1",
            1.0 - pan,
            1.0 + pan
        ));
    }
    parts.join(",")
}

/// The whole-programme audio chain, in the compiler's order.
#[must_use]
pub fn audio_chain(plan: &CompiledPlan) -> String {
    plan.audio_chain
        .iter()
        .map(|e| e.filter.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// The whole-programme video chain: the caller's own filter graph, then any
/// video effect steps, then the burned-in subtitle track.
///
/// The three join with `;` and each must define the label the next one reads.
/// A subtitle track therefore only appends cleanly when the preceding graph is
/// a passthrough, which is why [`super::RenderError::BurnOverVideoGraph`]
/// exists rather than a guess at a label.
#[must_use]
pub fn video_chain(plan: &CompiledPlan, subtitle: Option<&Path>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !plan.video_filter_complex.trim().is_empty() {
        parts.push(plan.video_filter_complex.clone());
    }
    for effect in &plan.video_chain {
        if !effect.filter.trim().is_empty() {
            parts.push(effect.filter.clone());
        }
    }
    if let Some(path) = subtitle {
        parts.push(format!("subtitles='{}'", path.display()));
    }
    parts.join(";")
}

/// Whether a video graph does any work. A chain of pure `copy` steps — what
/// `BrandingSpec::build_filter_chain` returns for an empty spec — is a
/// passthrough, and treating it as work would push every unbranded render off
/// the copy path for no reason.
#[must_use]
pub fn video_is_passthrough(graph: &str) -> bool {
    graph
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .all(|part| part.ends_with("copy[vout]") || part == "[0:v]copy[vout]")
}

/// Whether the concat step can stream-copy. True only when there is no video
/// work, no audio chain, no loudness pass and no burned-in subtitle.
#[must_use]
pub fn is_passthrough(plan: &CompiledPlan, subtitle: Option<&Path>) -> bool {
    video_is_passthrough(&video_chain(plan, subtitle))
        && audio_chain(plan).is_empty()
        && plan.loudness.is_none()
        && !plan.clips.iter().any(|c| !fade_filter(c).is_empty())
        && !plan.tracks.iter().any(|t| {
            !t.audible
                || t.gain_db.abs() > 1e-6
                || t.pan.abs() > 1e-6
                || !t.insert.trim().is_empty()
        })
}

/// `ffmpeg` arguments for one planned clip's sub-render.
#[must_use]
pub fn clip_args(plan: &CompiledPlan, clip: &Clip, source: &str, out: &Path) -> Vec<String> {
    let video = fade_filter(clip);
    let audio = track_filter(plan, clip_index(plan, clip));
    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-ss".to_string(),
        secs(clip.span.range.start()),
        "-i".to_string(),
        source.to_string(),
        "-t".to_string(),
        clip.output
            .duration()
            .map(secs)
            .unwrap_or_else(|_| "0.000000".to_string()),
    ];
    if !video.is_empty() {
        args.push("-vf".into());
        args.push(video.clone());
    }
    if !audio.is_empty() {
        args.push("-af".into());
        args.push(audio.clone());
    }
    if video.is_empty() && audio.is_empty() {
        args.push("-c".into());
        args.push("copy".into());
    } else {
        args.push("-c:v".into());
        args.push(plan.container.video_codec.clone());
        args.push("-c:a".into());
        args.push(plan.container.audio_codec.clone());
    }
    args.push(out.to_string_lossy().into_owned());
    args
}

fn clip_index(plan: &CompiledPlan, clip: &Clip) -> usize {
    plan.clips
        .iter()
        .position(|c| c.cut == clip.cut)
        .unwrap_or_default()
}

/// `ffmpeg` arguments for the concat step and the plan's whole-programme
/// chains.
#[must_use]
pub fn concat_args(
    plan: &CompiledPlan,
    list: &Path,
    out: &Path,
    subtitle: Option<&Path>,
) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        list.to_string_lossy().into_owned(),
    ];
    if is_passthrough(plan, subtitle) {
        args.push("-c".into());
        args.push("copy".into());
    } else {
        let video = video_chain(plan, subtitle);
        let audio = audio_chain(plan);
        let mut graph = Vec::new();
        if video_is_passthrough(&video) {
            graph.push("[0:v]copy[vout]".to_string());
        } else {
            graph.push(video);
        }
        if audio.is_empty() {
            graph.push("[0:a]anull[aout]".to_string());
        } else {
            graph.push(format!("[0:a]{audio}[aout]"));
        }
        args.push("-filter_complex".into());
        args.push(graph.join(";"));
        args.push("-map".into());
        args.push("[vout]".into());
        args.push("-map".into());
        args.push("[aout]".into());
        args.push("-c:v".into());
        args.push(plan.container.video_codec.clone());
        args.push("-c:a".into());
        args.push(plan.container.audio_codec.clone());
    }
    args.push(out.to_string_lossy().into_owned());
    args
}

/// The loudnorm pass-1 measurement filter. `loudnorm` prints its readings to
/// stderr, so this is a filter with no output rather than an output pass.
#[must_use]
pub fn measure_filter(target: &super::plan::Loudness) -> String {
    format!(
        "loudnorm=I={}:LRA={}:TP={}:print_format=json",
        target.i, target.lra, target.tp
    )
}

/// `ffmpeg` arguments for the loudnorm second pass.
#[must_use]
pub fn normalize_args(
    input: &Path,
    out: &Path,
    target: &super::plan::Loudness,
    measured: &str,
) -> Vec<String> {
    vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-i".to_string(),
        input.to_string_lossy().into_owned(),
        "-af".to_string(),
        normalize_filter(target, measured),
        "-c:v".to_string(),
        "copy".to_string(),
        out.to_string_lossy().into_owned(),
    ]
}

/// The loudnorm pass-2 filter, with pass 1's readings fed back in.
#[must_use]
pub fn normalize_filter(target: &super::plan::Loudness, measured: &str) -> String {
    format!(
        "loudnorm=I={}:LRA={}:TP={}:{measured}:linear=true",
        target.i, target.lra, target.tp
    )
}

/// Pull pass 1's readings out of ffmpeg's stderr block.
pub fn parse_measurements(stderr: &str) -> Result<String, RenderError> {
    let field = |name: &str| -> Option<String> {
        let key = format!("\"{name}\"");
        stderr
            .lines()
            .find_map(|line| line.trim().strip_prefix(&key))
            .and_then(|rest| rest.split(':').nth(1))
            .map(|value| value.trim().trim_matches(',').trim_matches('"').to_string())
    };
    let read = |name: &str| {
        field(name).ok_or_else(|| {
            io(format!(
                "ffmpeg reported no {name}; the loudness pass produced no measurement block"
            ))
        })
    };
    Ok(format!(
        "measured_I={}:measured_LRA={}:measured_TP={}:measured_thresh={}:offset={}",
        read("input_i")?,
        read("input_lra")?,
        read("input_tp")?,
        read("input_thresh")?,
        read("target_offset")?
    ))
}
