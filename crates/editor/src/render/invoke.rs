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

/// The declared output picture shape as an ffmpeg conform chain.
///
/// Every layer goes through this, not just the base: `overlay` needs both
/// inputs at the same size, rate and timebase, and a second source at a
/// different one fails the graph rather than being conformed.
#[must_use]
pub fn frames_filter(frames: &super::request::Frames) -> String {
    format!(
        "scale={}:{}:flags=bicubic,fps={}/{},format={},settb={}/{}",
        frames.width,
        frames.height,
        frames.fps_num,
        frames.fps_den,
        frames.pixel_format,
        frames.timebase_num,
        frames.timebase_den
    )
}

/// One ramp or hold between two keyframes, in output microseconds.
struct Segment {
    at: i64,
    /// Where this segment starts, so the ramp is scaled by the segment's own
    /// length rather than by a second of wall clock.
    from_at: i64,
    from: f64,
    to: f64,
    hold: bool,
    /// The value held from `at` onwards, which for a stepped keyframe is *not*
    /// `from`: the jump lands on the keyframe.
    after: f64,
}

fn segments(
    layer: &super::plan::PlanLayer,
    field: fn(&super::request::Transform) -> f64,
) -> Vec<Segment> {
    let mut points: Vec<(i64, f64, bool)> = Vec::new();
    if layer.keyframes.first().is_none_or(|k| k.at.micros() > 0) {
        points.push((
            0,
            field(&layer.transform_at(crate::time::MediaTime::ZERO)),
            true,
        ));
    }
    for key in &layer.keyframes {
        points.push((
            key.at.micros(),
            field(&key.transform),
            key.interpolate == super::request::Interpolation::Stepped,
        ));
    }
    points
        .windows(2)
        .filter(|w| w[1].0 > w[0].0)
        .map(|w| Segment {
            at: w[1].0,
            from_at: w[0].0,
            from: w[0].1,
            to: w[1].1,
            hold: w[1].2 || w[0].1 == w[1].1,
            after: w[1].1,
        })
        .collect()
}

/// A piecewise ffmpeg expression over `t` (seconds) from a segment list.
///
/// The commas inside `if(...)` are escaped because an unescaped `,` ends the
/// filter: half an expression evaluates to nothing and a zoom silently stops
/// moving.
///
/// Mirrors [`super::plan::PlanLayer::transform_at`] — same hold before the
/// first keyframe, same hold after the last, same stepping. The model answers a
/// preview handle and this answers the pixels, and the test that reads both is
/// what keeps them the same number.
fn piecewise(list: &[Segment]) -> String {
    let Some(last) = list.last() else {
        return "0.0000".to_string();
    };
    let mut expr = format!("{:.4}", last.after);
    for segment in list.iter().rev() {
        let body = if segment.hold {
            format!("{:.4}", segment.from)
        } else {
            format!(
                "{:.4}+({:.4}-{:.4})*(t-{:.6})/{:.6}",
                segment.from,
                segment.to,
                segment.from,
                segment.from_at as f64 / 1e6,
                (segment.at - segment.from_at) as f64 / 1e6
            )
        };
        expr = format!(
            "if(lt(t\\,{:.6})\\,{body}\\,{expr})",
            segment.at as f64 / 1e6
        );
    }
    expr
}

/// The scale a layer's clips are conformed to, or `None` when it never scales.
#[must_use]
pub fn layer_scale_filter(layer: &super::plan::PlanLayer) -> Option<String> {
    let scaled = layer.transform.scale.is_some()
        || layer.keyframes.iter().any(|k| k.transform.scale.is_some());
    if !scaled {
        return None;
    }
    if layer.keyframes.is_empty() {
        let (w, h) = layer.transform.scale?;
        return Some(format!("scale={w}:{h}"));
    }
    let width = piecewise(&segments(layer, |t| {
        t.scale.map_or(0.0, |(w, _)| f64::from(w))
    }));
    let height = piecewise(&segments(layer, |t| {
        t.scale.map_or(0.0, |(_, h)| f64::from(h))
    }));
    Some(format!("scale=w='{width}':h='{height}':eval=frame"))
}

/// A layer's top-left offset as two ffmpeg expressions.
#[must_use]
pub fn layer_offset_exprs(layer: &super::plan::PlanLayer) -> (String, String) {
    let x = |t: &super::request::Transform| t.offset_px.0 as f64;
    let y = |t: &super::request::Transform| t.offset_px.1 as f64;
    (
        piecewise(&segments(layer, x)),
        piecewise(&segments(layer, y)),
    )
}

/// The layers this plan draws over the programme, in paint order. Everything
/// except the base video lane; an invisible layer is not rendered at all.
#[must_use]
pub fn overlay_layers(plan: &CompiledPlan) -> Vec<&super::plan::PlanLayer> {
    plan.layers
        .iter()
        .filter(|l| l.visible && l.kind != super::request::LayerKind::Video)
        .collect()
}

/// `ffmpeg` arguments for one overlay clip's sub-render: **video only**.
///
/// `-an` is the whole of "the source keeps playing underneath". An overlay is
/// composited over the programme's picture and contributes no signal of its
/// own, so it is decoded without an audio stream to decode and cannot reach the
/// mix even if a caller tried to route it.
#[must_use]
pub fn overlay_clip_args(
    plan: &CompiledPlan,
    clip: &Clip,
    source: &str,
    out: &Path,
) -> Vec<String> {
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
    let video = fade_filter(clip);
    if !video.is_empty() {
        args.push("-vf".into());
        args.push(video);
    }
    args.push("-an".into());
    args.push("-c:v".into());
    args.push(plan.container.video_codec.clone());
    args.push(out.to_string_lossy().into_owned());
    args
}

/// `ffmpeg` arguments that composite the overlays over the concatenated
/// programme.
///
/// `rendered` is every clip's sub-render indexed by clip index, so the order
/// overlays are composited in is the plan's paint order rather than an
/// argument's.
///
/// One command, so the programme's audio reaches the output through exactly one
/// filter chain whether or not overlays exist — the audio graph is the same
/// `[0:a]…[aout]` either way, and no overlay input is ever mapped.
pub fn composite_args(
    plan: &CompiledPlan,
    list: &Path,
    rendered: &[&Path],
    out: &Path,
    subtitle: Option<&Path>,
) -> Result<Vec<String>, RenderError> {
    // A burned-in subtitle is appended to the picture by a label the plan does
    // not own, and the composite now sits before it.
    if subtitle.is_some() {
        return Err(RenderError::BurnOverVideoGraph);
    }
    let layers = overlay_layers(plan);
    let planned: usize = layers.iter().map(|l| l.clips.len()).sum();
    let actual = plan.clips.iter().filter(|c| c.overlay).count();
    if planned != actual || rendered.len() != plan.clips.len() {
        return Err(RenderError::Execute {
            detail: format!(
                "{planned} overlay clips on {} layers, {actual} overlay clips in the plan, \
                 {} rendered sub-clips",
                layers.len(),
                rendered.len()
            ),
        });
    }
    // Video effect steps are joined onto the picture by a label the plan does
    // not own, and guessing one would drop either the overlay or the effect
    // while reporting success. The branding chain in `video_filter_complex` is
    // the supported way to shape the picture and is applied below.
    if !plan.video_chain.is_empty() {
        return Err(RenderError::OverlaysOverVideoEffects);
    }

    let mut graph: Vec<String> = Vec::new();
    let conform = plan.frames.as_ref().map(frames_filter).unwrap_or_default();
    let caller = plan.video_filter_complex.trim();
    if !caller.is_empty() && !caller.contains("[0:v]") {
        return Err(RenderError::UnlabelledVideoGraph);
    }
    if caller.is_empty() {
        graph.push(format!(
            "[0:v]{}[call]",
            if conform.is_empty() { "null" } else { &conform }
        ));
    } else {
        // Relabelled, because ffmpeg forbids reusing a label and the composite
        // now sits between the caller's entry and its exit.
        graph.push(caller.replace("[0:v]", "[v0]").replace("[vout]", "[call]"));
        if !conform.is_empty() {
            graph.push(format!("[v0]{conform}[call]"));
        }
    }
    let audio = audio_chain(plan);
    if audio.is_empty() {
        graph.push("[0:a]anull[aout]".to_string());
    } else {
        graph.push(format!("[0:a]{audio}[aout]"));
    }

    let mut cursor = "call".to_string();
    let mut order: Vec<usize> = Vec::new();
    for layer in &layers {
        let scale = layer_scale_filter(layer);
        let (x_expr, y_expr) = layer_offset_exprs(layer);
        for clip_index in &layer.clips {
            let clip = &plan.clips[*clip_index];
            let input = 1 + order.len();
            let mut chain: Vec<String> = Vec::new();
            if !conform.is_empty() {
                chain.push(conform.clone());
            }
            if let Some(scale) = &scale {
                chain.push(scale.clone());
            }
            let label = format!("l{input}");
            let chain = if chain.is_empty() {
                "null".to_string()
            } else {
                chain.join(",")
            };
            graph.push(format!("[{input}]{chain}[{label}]"));
            let painted = format!("c{input}");
            graph.push(format!(
                "[{cursor}][{label}]overlay=x='{x_expr}':y='{y_expr}':\
                 enable='between(t\\,{}\\,{})'[{painted}]",
                secs(clip.output.start()),
                secs(clip.output.end())
            ));
            cursor = painted;
            order.push(*clip_index);
        }
    }
    graph.push(format!("[{cursor}]null[vout]"));

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
    for clip_index in &order {
        args.push("-i".to_string());
        args.push(rendered[*clip_index].to_string_lossy().into_owned());
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
    args.push(out.to_string_lossy().into_owned());
    Ok(args)
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
