//! The mixer inside the plan: audibility, bus routing, and chain order.
//!
//! These are the *structural* half of E04. The decoded-media half — that a
//! muted track really is silent and a gain really lands at the promised dB —
//! lives in `strivo-web`'s `mixer.rs`, because it needs ffmpeg and a corpus.
//! What is asserted here is the thing that makes those measurements possible:
//! one compiler decides audibility and order, and nothing downstream re-decides
//! either.

use super::*;

fn two_tracks() -> RenderRequest {
    let mut req = request();
    req.tracks = vec![
        TrackSpec {
            label: "voice".into(),
            bus: "master".into(),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            insert: String::new(),
            clips: vec![0],
        },
        TrackSpec {
            label: "game".into(),
            bus: "master".into(),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            insert: String::new(),
            clips: vec![1],
        },
    ];
    req
}

fn labels(plan: &CompiledPlan) -> Vec<(&str, bool)> {
    plan.tracks
        .iter()
        .map(|t| (t.label.as_str(), t.audible))
        .collect()
}

// ── audibility: resolved once, by the compiler ──────────────────────────

#[test]
fn an_unaudited_track_is_audible() {
    let plan = preview(&two_tracks()).expect("preview compiles").plan;
    assert_eq!(
        labels(&plan),
        vec![("game", true), ("voice", true)],
        "anti-vacuity: the two-track fixture really does carry two audible tracks, \
         so the assertions below are about a change and not about nothing"
    );
}

#[test]
fn a_mute_silences_its_own_track_and_no_other() {
    let mut req = two_tracks();
    req.tracks[0].muted = true;
    let plan = preview(&req).expect("preview compiles").plan;
    assert_eq!(labels(&plan), vec![("game", true), ("voice", false)]);
}

#[test]
fn a_solo_silences_every_track_that_is_not_soloed() {
    let mut req = two_tracks();
    req.tracks[0].solo = true;
    let plan = preview(&req).expect("preview compiles").plan;
    assert_eq!(labels(&plan), vec![("game", false), ("voice", true)]);
}

#[test]
fn a_muted_track_stays_silent_when_soloed() {
    // The rule, stated as a test because it is the one a DAW can differ on:
    // solo is a listening aid, not an unmute. Resolving it here means the
    // executor and the preview cannot each pick a convention.
    let mut req = two_tracks();
    req.tracks[0].muted = true;
    req.tracks[0].solo = true;
    let plan = preview(&req).expect("preview compiles").plan;
    assert_eq!(labels(&plan), vec![("game", false), ("voice", false)]);
    assert!(
        plan.tracks.iter().all(|t| !t.audible),
        "a lone muted-and-soloed track must not be the one thing the plan hears"
    );
}

#[test]
fn audibility_does_not_depend_on_the_order_the_tracks_were_declared_in() {
    let solo_voice = |mut req: RenderRequest| {
        req.tracks
            .iter_mut()
            .find(|t| t.label == "voice")
            .expect("the fixture has a voice track")
            .solo = true;
        req
    };
    let forwards = solo_voice(two_tracks());
    let mut swapped = two_tracks();
    swapped.tracks.swap(0, 1);
    let backwards = solo_voice(swapped);
    assert_eq!(
        preview(&forwards).expect("preview compiles").plan,
        preview(&backwards).expect("preview compiles").plan,
        "a solo is a statement about the whole mixer, not about a position in a list"
    );
}

#[test]
fn the_executor_reads_resolved_audibility_and_not_the_buttons() {
    // The property that makes "preview and render share one plan" structural
    // for a mute: the filter is a function of `audible`, and `audible` has one
    // writer. A second `if track.muted` anywhere in the executor would satisfy
    // the two button tests above and break this one.
    let mut req = two_tracks();
    req.tracks[0].muted = true;
    let plan = preview(&req).expect("preview compiles").plan;
    assert_eq!(
        track_filter(&plan, 0),
        "volume=0",
        "the muted clip is silenced by a linear zero, which is the only spelling the \
         AAC encoder accepts — `-inf` makes ffmpeg's volume filter produce non-finite \
         samples and the encode fails outright"
    );
    assert_eq!(track_filter(&plan, 1), "", "the other clip is untouched");
    assert!(
        !is_passthrough(&plan, None),
        "a plan with an inaudible track can never stream-copy"
    );
}

// ── routing: a pre-master tap is not the master chain ───────────────────

#[test]
fn a_master_effect_runs_on_the_sum_and_a_tap_runs_on_its_own_track() {
    let mut req = two_tracks();
    req.tracks[0].insert = "highpass=f=80".into();
    req.effects = vec![effect(
        "master-chain",
        EffectKind::SubMix,
        "alimiter=limit=0.891",
    )];
    let plan = preview(&req).expect("preview compiles").plan;

    // The tap is on the track, so the per-clip sub-render carries it and the
    // programme chain does not: it cannot, the programme chain has one input
    // and that input is the sum.
    assert_eq!(
        track_filter(&plan, 0),
        "highpass=f=80",
        "the tap runs on its own track"
    );
    assert_eq!(track_filter(&plan, 1), "", "the other track is not tapped");
    assert!(
        audio_chain(&plan).contains("alimiter") && !audio_chain(&plan).contains("highpass"),
        "the master effect runs on the sum; a tap that reached it would condition \
         the game audio with the voice's chain"
    );
    assert!(
        plan.tracks.iter().any(|t| t.insert == "highpass=f=80"),
        "the plan must carry the tap, or the mixer surface has nothing to draw"
    );
}

#[test]
fn a_tap_declared_in_the_programme_chain_is_refused() {
    // The silent substitution this closes: a caller who wants a per-bus high-pass
    // writes the filter into the master chain, it runs on the sum, and the
    // render succeeds having applied the voice's chain to the game audio.
    let mut req = two_tracks();
    req.effects = vec![effect("voice-chain", EffectKind::InsertFx, "highpass=f=80")];
    req.effects[0].bus = Some("voice".into());
    let refusal = preview(&req).expect_err("the programme chain has one input: the sum");
    assert!(
        matches!(refusal, RenderError::TapInProgrammeChain { .. }),
        "expected the tap-in-programme-chain refusal, got {refusal:?}"
    );
    assert!(
        refusal.to_string().contains("insert chain"),
        "the refusal must name the field that does express a tap, or it is a dead \
         end for the caller: {refusal}"
    );
}

#[test]
fn a_tapped_track_keeps_its_tap_and_its_place_in_the_signal() {
    // Order inside the per-track chain: the tap conditions the signal, then the
    // trim sets the track's level. Asserted because a chain that ran after the
    // gain would be a different filter for every fader position.
    let mut req = two_tracks();
    req.tracks[0].insert = "highpass=f=80".into();
    req.tracks[0].gain_db = -6.0;
    req.tracks[0].pan = 0.5;
    let plan = preview(&req).expect("preview compiles").plan;
    let filter = track_filter(&plan, 0);
    let tap = filter.find("highpass").expect("the tap");
    let gain = filter.find("volume=").expect("the trim");
    let pan = filter.find("pan=").expect("the pan");
    assert!(
        tap < gain && gain < pan,
        "the order is tap, trim, pan: {filter}"
    );
}

#[test]
fn the_audio_chain_is_ordered_by_signal_flow_not_by_declaration() {
    let mut req = request();
    req.effects = vec![
        effect("loud", EffectKind::VolumeAutomation, "volume=1.0dB"),
        effect("master", EffectKind::SubMix, "volume=-1.5dB"),
        effect("inserts", EffectKind::InsertFx, "highpass=f=80"),
        effect("pitch", EffectKind::PitchTime, "rubberband=tempo=1.1"),
    ];
    let plan = preview(&req).expect("preview compiles").plan;
    assert_eq!(
        plan.audio_chain.iter().map(|e| e.kind).collect::<Vec<_>>(),
        vec![
            EffectKind::PitchTime,
            EffectKind::InsertFx,
            EffectKind::SubMix,
            EffectKind::VolumeAutomation,
        ],
        "the chain's order is the signal flow, and it is asserted rather than \
         trusted: an ordered chain whose order is not checked is not ordered"
    );
    // And it is the order the executor runs, not only the order the plan shows.
    assert!(
        audio_chain(&plan).find("rubberband").unwrap()
            < audio_chain(&plan).find("highpass").unwrap(),
        "the executor must run the stages in the plan's order"
    );
}

// ── normalisation: measure-then-apply, and only there ───────────────────

#[test]
fn a_loudness_stage_in_the_chain_is_refused() {
    // Normalisation is measure-then-apply and lives on `plan.loudness`, where
    // the executor runs pass 1 and then pass 2. As a chain step it would be a
    // *second*, single-pass `loudnorm` applied after the measured one, so the
    // programme would be normalised twice and the ceiling re-applied to a
    // signal already at it. The refusal is how that decision is structural.
    let mut req = request();
    req.effects
        .push(effect("normalise", EffectKind::Loudness, "loudnorm=I=-14"));
    let refusal = preview(&req).expect_err("loudness is not a chain stage");
    assert!(
        matches!(refusal, RenderError::LoudnessAsChainStep { .. }),
        "expected the loudness-as-chain-step refusal, got {refusal:?}"
    );
    assert!(
        refusal.to_string().contains("measure-then-apply"),
        "the refusal must name the decision it enforces, or it reads as arbitrary: {refusal}"
    );
}

#[test]
fn the_plan_records_the_normalisation_target_and_no_loudness_filter() {
    let plan = preview(&request()).expect("preview compiles").plan;
    let target = plan.loudness.as_ref().expect("a loudness target");
    assert_eq!(target.i, -14.0);
    assert!(
        !audio_chain(&plan).contains("loudnorm"),
        "the chain must carry no loudnorm: the measure-then-apply pass runs from \
         `plan.loudness`, and a chain copy would be a second normalisation"
    );
    assert!(
        measure_filter(target).contains("print_format=json"),
        "pass one must measure, which is what makes the two-pass choice real"
    );
    assert!(
        normalize_filter(target, "measured_I=-20.0:measured_LRA=1.0").contains("linear=true"),
        "pass two must apply the readings pass one took"
    );
}

// ── the plan's own mixer view ───────────────────────────────────────────

#[test]
fn the_plan_names_every_track_the_request_declared() {
    let plan = preview(&two_tracks()).expect("preview compiles").plan;
    assert_eq!(plan.tracks.len(), 2);
    assert!(plan.tracks.iter().all(|t| t.bus == "master"));
    // Every cut is still routed: audibility is a mixer decision, not a reason
    // for a clip to leave the plan.
    assert_eq!(
        plan.clips.len(),
        2,
        "an inaudible track must not unroute its clips — that is `UnroutedClip`'s job"
    );
}
