//! Every input the compiler must refuse, and the lazy answer each refusal
//! replaces.
//!
//! Separate from the convergence test because the failure mode differs: there,
//! passing means the two callers agree; here, passing means a specific bad
//! input is *stopped*. A suite that quietly stopped refusing would still be
//! green on the agreement half.

use super::fixture::*;
use super::*;
use crate::time::{Rate, SourceRef, SourceSpan, TimeBase};

#[test]
fn an_empty_revision_is_refused_rather_than_compiled_to_an_empty_plan() {
    let mut req = request();
    req.clips.clear();
    req.layers[0].clips.clear();
    req.tracks[0].clips.clear();
    req.transitions.clear();
    assert_eq!(
        preview(&req).expect_err("no cuts").to_string(),
        "the revision has no cuts to render"
    );
}

#[test]
fn a_clip_on_no_track_is_refused_rather_than_rendered_silent() {
    let mut req = request();
    req.tracks[0].clips = vec![0];
    assert!(
        preview(&req).is_err(),
        "a cut no track claims would render into silence"
    );
}

#[test]
fn a_non_integral_speed_output_is_refused_not_rounded() {
    let mut req = request();
    // 1_000_001 us at 1/2 is 500_000.5 us — not a whole microsecond, so the
    // mapping would be a rounding rather than a map.
    req.clips[1] = ClipRequest {
        cut: 1,
        span: SourceSpan::new(
            SourceRef::from_locator(SOURCE),
            TimeBase::from_rate(1, 1).expect("identity timebase"),
            Interval::new(us(2_000_000), us(3_000_001)).expect("ordered"),
            Rate::new(1, 2).expect("positive rational"),
        )
        .expect("non-empty"),
        fade_in: MediaTime::ZERO,
        fade_out: MediaTime::ZERO,
        overlay_at: None,
    };
    let refusal = preview(&req).expect_err("a fractional microsecond is refused");
    assert!(
        refusal.to_string().contains("fractional microsecond"),
        "todo 22's own refusal must be carried, not re-derived: {refusal}"
    );
}

#[test]
fn a_cue_the_composition_cut_away_is_refused_not_clamped() {
    let mut req = request();
    // The composition reads 0..1s and 2..3.5s of the source, so 4s is a
    // coordinate no cut reaches.
    req.captions.cues = vec![CueCoord {
        at: us(4_000_000),
        duration: us(400_000),
    }];
    let refusal = preview(&req).expect_err("an unmappable cue is refused");
    assert!(
        refusal.to_string().contains("caption cue 0"),
        "the refusal must name the cue: {refusal}"
    );
}

#[test]
fn a_burned_in_caption_with_no_style_is_refused() {
    let mut req = request();
    req.captions.mode = CaptionMode::BurnedIn;
    req.captions.style_json = String::new();
    assert!(
        preview(&req).is_err(),
        "burning the default style is a silent substitution"
    );
}

#[test]
fn a_transition_past_the_end_of_the_output_is_refused_not_shortened() {
    let mut req = request();
    req.transitions[0].at = us(2_400_000);
    req.transitions[0].duration = us(400_000);
    assert!(
        preview(&req).is_err(),
        "a transition running past the output must be refused, not trimmed to fit"
    );
}

#[test]
fn a_zero_length_or_inverted_segment_is_unrepresentable_not_merely_refused() {
    // Stronger than a refusal: a request cannot *carry* either shape, because
    // `SourceSpan` and `Interval` refuse them at construction. A compiler that
    // had to reject them at run time would be one step closer to accepting a
    // clamped one.
    let empty = SourceSpan::new(
        SourceRef::from_locator(SOURCE),
        TimeBase::from_rate(1, 1).expect("identity timebase"),
        Interval::new(us(2_000_000), us(2_000_000)).expect("a zero-length interval is legal"),
        Rate::ONE,
    );
    assert!(
        matches!(empty, Err(TimeError::Empty { at: 2_000_000 })),
        "a source span with no frames has no reverse map and must be refused"
    );
    let inverted = Interval::new(us(3_000_000), us(2_000_000));
    assert!(
        matches!(inverted, Err(TimeError::Inverted { .. })),
        "an end before its start is refused, never clamped to the start"
    );
}

#[test]
fn a_gap_in_source_time_is_not_a_gap_on_the_output_timeline() {
    let plan = preview(&request()).expect("preview compiles");
    assert_eq!(
        plan.plan.output_duration,
        us(2_500_000),
        "a gap in source time is not a gap on the output timeline"
    );
}

#[test]
fn the_audio_chain_runs_in_the_compilers_order_not_the_callers() {
    let mut req = request();
    req.effects = vec![
        effect("automation", EffectKind::VolumeAutomation, "volume=1.20dB"),
        effect("master", EffectKind::SubMix, "volume=-1.50dB"),
        effect("pitch", EffectKind::PitchTime, "rubberband=tempo=1.10"),
        effect("inserts", EffectKind::InsertFx, "highpass=f=80"),
    ];
    let plan = preview(&req).expect("preview compiles");
    assert_eq!(
        plan.plan.audio_stages(),
        vec![
            EffectKind::PitchTime,
            EffectKind::InsertFx,
            EffectKind::SubMix,
            EffectKind::VolumeAutomation
        ],
        "pitch changes the length everything downstream is measured against, so it \
         runs first; the caller's order must not decide"
    );
}

#[test]
fn an_effect_that_emits_nothing_does_not_reach_the_plan() {
    let mut req = request();
    req.effects
        .push(effect("blank", EffectKind::InsertFx, "   "));
    let plan = preview(&req).expect("preview compiles");
    assert_eq!(
        plan.plan.audio_chain.len(),
        4,
        "the blank chain is not work the render should do"
    );
    assert!(
        !audio_chain(&plan.plan).contains("   ,"),
        "a blank step must not leave a dangling separator in the chain"
    );
}

#[test]
fn a_bus_mixing_more_signals_than_the_plan_has_tracks_is_refused() {
    let mut req = request();
    req.effects.push(EffectStep {
        id: "two-bus".into(),
        kind: EffectKind::SubMix,
        filter: "[0:a][1:a]amix=inputs=2[out]".into(),
        arity: 2,
        bus: None,
    });
    assert!(
        preview(&req).is_err(),
        "a two-input graph over a one-track plan would drop a track silently"
    );
}

#[test]
fn the_compiler_is_stable_across_repeated_runs() {
    // Fixed inputs, no clock and no filesystem: a flake here would be a
    // determinism defect, not a timing one.
    let build = || -> Vec<String> {
        (0..8)
            .map(|i| {
                let mut req = request();
                req.subject = format!("stream-{i}");
                serde_json::to_string(&preview(&req).expect("preview compiles").plan)
                    .expect("plan serialises")
            })
            .collect()
    };
    let first = build();
    for round in 0..3 {
        assert_eq!(first, build(), "round {round} produced a different plan");
    }
}
