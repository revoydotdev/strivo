//! Caption timing against a composition that has a cut and a speed change.
//!
//! The central claim of this todo: **caption timing is derived from the
//! composition, not from the source.** The fixture is deliberately the hard
//! case — a cut, so a later caption's naive source time is wrong, and a speed
//! change, so carrying the authored duration across is also wrong.
//!
//! Written before the model existed and run red twice; see
//! `.omo/evidence/task-45-RED-before.txt`. Both failures were behavioural (a
//! wrong number, a silently accepted cue), not a missing import.

use strivo_branding::{BrandVariant, SafeArea};
use strivo_captions::{resolve, CaptionCue, CaptionError, CaptionTrack, IdentityTranslator};
use strivo_editor::render::{
    compile, CaptionMode, CaptionSpec, ClipRequest, Container, CueCoord, LayerKind, LayerSpec,
    OutputProfile, RenderRequest, TrackSpec, Transform, VideoIntent,
};
use strivo_editor::time::{
    Coordinate, Interval, IntervalMap, MediaTime, Rate, SourceRef, SourceSpan, TimeBase,
};

fn us(v: i64) -> MediaTime {
    MediaTime::from_micros(v).expect("non-negative")
}

fn span(start: i64, end: i64, rate: (i64, i64)) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator("/recordings/stream-01.mkv"),
        TimeBase::from_rate(1, 1).expect("identity timebase"),
        Interval::new(us(start), us(end)).expect("ordered"),
        Rate::new(rate.0, rate.1).expect("positive rational"),
    )
    .expect("non-empty span")
}

fn clip(cut: usize, start: i64, end: i64, rate: (i64, i64)) -> ClipRequest {
    ClipRequest {
        cut,
        span: span(start, end, rate),
        fade_in: MediaTime::ZERO,
        fade_out: MediaTime::ZERO,
        overlay_at: None,
    }
}

fn variant() -> BrandVariant {
    BrandVariant {
        name: "1080p".into(),
        width: 1920,
        height: 1080,
    }
}

fn safe() -> SafeArea {
    SafeArea { x: 0.05, y: 0.05 }
}

/// Source `[0,1)` at 1x, then source `[2,4)` at 2x speed-up.
///
/// Output is `[0,1)` then `[1,2)`. Two things are true at once and both matter:
/// source `[1,2)` is **deleted**, and the second segment is **half as long as
/// its source**. A naive read leaves the caption at its source time (2.5s); a
/// cue whose duration is carried verbatim reads 0.5s long where the
/// composition gives it 0.25s.
fn map() -> IntervalMap {
    let mut map = IntervalMap::new();
    map.push_source(span(0, 1_000_000, (1, 1)))
        .expect("append the first cut");
    map.push_source(span(2_000_000, 4_000_000, (1, 2)))
        .expect("append the second cut");
    map
}

fn track_with(cues: Vec<CaptionCue>) -> CaptionTrack {
    let mut track = CaptionTrack::new("rev-1", &IdentityTranslator);
    track.cues = cues;
    track
}

fn source_cue(id: &str, start: i64, duration: i64) -> CaptionCue {
    CaptionCue::in_source(id, "hello", Some("Alice"), us(start), us(duration))
        .expect("a cue with a length and a usable id")
}

fn request(cues: Vec<CueCoord>) -> RenderRequest {
    RenderRequest {
        subject: "stream-01".into(),
        revision: "rev-1".into(),
        clips: vec![
            clip(0, 0, 1_000_000, (1, 1)),
            clip(1, 2_000_000, 4_000_000, (1, 2)),
        ],
        layers: vec![LayerSpec {
            id: "programme".into(),
            kind: LayerKind::Video,
            transform: Transform::default(),
            colour: Default::default(),
            clips: vec![0, 1],
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
            clips: vec![0, 1],
        }],
        effects: Vec::new(),
        loudness: None,
        captions: CaptionSpec {
            mode: CaptionMode::Sidecar,
            style_json: String::new(),
            coordinate: Coordinate::Composition,
            safe_area: true,
            cues,
        },
        video: VideoIntent::default(),
        container: Container {
            extension: "mkv".into(),
            video_codec: "libx264".into(),
            audio_codec: "aac".into(),
            stream_copy: false,
        },
        profile: OutputProfile {
            name: "default".into(),
            renderer: "ffmpeg".into(),
            renderer_version: "test".into(),
        },
    }
}

#[test]
fn a_cut_and_a_speed_change_move_a_caption_onto_the_composition() {
    let track = track_with(vec![source_cue("c1", 2_500_000, 500_000)]);
    let resolved = resolve(&track, &map(), safe(), &variant()).expect("the cue survives the edit");

    assert_eq!(resolved.segments.len(), 1);
    assert_eq!(
        resolved.segments[0].start_sec, 1.25,
        "source 2.5s inside a segment that starts at output 1.0s and runs at 2x"
    );
    assert_eq!(
        resolved.segments[0].end_sec, 1.5,
        "0.5s of source under a 2x speed-up is 0.25s of output"
    );
}

#[test]
fn the_fixture_contains_both_a_cut_and_a_speed_change() {
    // Guards the fixture itself. Without this, "the caption moved" could be
    // satisfied by a cut alone or by a speed change alone, and the hard case —
    // the one where a naive source read and a carried duration are both wrong
    // — would go untested.
    let map = map();
    let resolved = resolve(
        &track_with(vec![source_cue("c1", 2_500_000, 500_000)]),
        &map,
        safe(),
        &variant(),
    )
    .expect("the cue survives");

    let start = resolved.segments[0].start_sec;
    let length = resolved.segments[0].end_sec - start;

    assert_ne!(start, 2.5, "the cut must move the cue off its source time");
    assert_ne!(
        length, 0.5,
        "the speed change must shorten the cue; a carried duration is 0.5s"
    );
    // A whole-timeline linear rescale (4s source onto 2s output) coincidentally
    // puts the start at 1.25s, so the *duration* is the only thing that
    // distinguishes this from a linear remap. Asserted rather than assumed.
    assert_ne!(
        (start, length),
        (1.25, 0.5),
        "a linear remap of the whole timeline would produce exactly this pair"
    );
    // And the cut is real: source [1,2) has no composition position at all.
    assert!(
        map.composition_times_of(us(1_500_000)).is_err(),
        "the fixture's cut is not actually deleting anything: source 1.5s still has an output"
    );
}

#[test]
fn a_cue_the_cut_deleted_is_refused_and_nothing_is_resolved() {
    let track = track_with(vec![
        source_cue("ok", 100_000, 200_000),
        source_cue("gone", 1_500_000, 200_000),
    ]);
    assert_eq!(
        resolve(&track, &map(), safe(), &variant()).err(),
        Some(CaptionError::CueDeleted {
            id: "gone".into(),
            at: 1_500_000,
        }),
        "a cue inside deleted source is refused by name, not clamped to a cut edge"
    );
}

#[test]
fn a_cue_running_from_kept_source_into_deleted_source_is_refused() {
    // Source 0.9s is kept; source 1.3s is not. Answering the two ends
    // separately would pair an answer from the first segment with one from the
    // second and draw a continuous caption across the cut.
    let track = track_with(vec![source_cue("straddle", 900_000, 400_000)]);
    let outcome = resolve(&track, &map(), safe(), &variant());
    assert!(
        matches!(outcome, Err(CaptionError::CueDeleted { .. })),
        "a cue that leaves its segment has no composition span; got {outcome:?}"
    );
}

#[test]
fn a_composition_time_cue_is_not_moved_by_any_cut() {
    let cue =
        CaptionCue::in_composition("pinned", "pinned", None, us(200_000), us(300_000)).unwrap();
    let resolved = resolve(&track_with(vec![cue]), &map(), safe(), &variant()).unwrap();
    assert_eq!(resolved.segments[0].start_sec, 0.2);
    assert_eq!(resolved.segments[0].end_sec, 0.5);
}

#[test]
fn a_composition_time_cue_longer_than_the_programme_is_refused() {
    let cue = CaptionCue::in_composition("over", "over", None, us(1_900_000), us(500_000)).unwrap();
    let outcome = resolve(&track_with(vec![cue]), &map(), safe(), &variant());
    assert!(
        matches!(outcome, Err(CaptionError::CueDeleted { .. })),
        "the output is 2s long, so a cue ending at 2.4s is not on the programme"
    );
}

#[test]
fn two_cues_under_one_id_refuse_the_batch() {
    let track = track_with(vec![
        source_cue("same", 100_000, 200_000),
        source_cue("same", 400_000, 200_000),
    ]);
    assert_eq!(
        resolve(&track, &map(), safe(), &variant()).err(),
        Some(CaptionError::DuplicateCue { id: "same".into() })
    );
}

#[test]
fn the_plan_carries_the_resolved_cue_style_and_count() {
    let track = track_with(vec![source_cue("c1", 2_500_000, 500_000)]);
    let resolved = resolve(&track, &map(), safe(), &variant()).expect("the cue survives");

    let mut req = request(
        resolved
            .segments
            .iter()
            .map(|s| CueCoord {
                at: us((f64::from(s.start_sec) * 1_000_000.0).round() as i64),
                duration: us(
                    ((f64::from(s.end_sec) - f64::from(s.start_sec)) * 1_000_000.0).round() as i64,
                ),
            })
            .collect(),
    );
    req.captions.mode = CaptionMode::BurnedIn;
    req.captions.style_json = serde_json::to_string(&resolved.style).expect("the style serialises");

    let plan = compile(&req).expect("the request compiles");
    assert_eq!(plan.caption_mode, CaptionMode::BurnedIn);
    assert!(plan.caption_safe_area);
    assert_eq!(plan.caption_coordinate, Coordinate::Composition);
    assert_eq!(
        plan.cues.len(),
        resolved.segments.len(),
        "the plan and the export must agree on how many cues there are"
    );
    assert_eq!(
        plan.caption_style_json,
        serde_json::to_string(&resolved.style).expect("the style serialises"),
        "the plan carries the resolved, safe-area-fitted style verbatim"
    );
    // 1080p with a 5% inset needs 54px, and the default asks for 80, so the
    // authored margin already clears it. The raise is exercised where the
    // safe area is the binding constraint — see `safe_area.rs`.
    assert_eq!(resolved.style.margin_v_px, 80);
}

#[test]
fn the_plans_own_cue_field_is_not_where_the_export_reads_its_times() {
    // A recorded disagreement rather than a silent one.
    //
    // `CompiledPlan::cues[i].at` is documented as "where the cue lands on the
    // output timeline", but `compile_cues` stores whatever `IntervalMap::resolve`
    // *answers* — so for a Composition-coordinate query it stores the **source**
    // time, and `duration` is the authored length either way. Both fields live
    // in `crates/editor/src/render/`, which this todo is forbidden to touch, so
    // the authoritative resolved timing is `ResolvedCaptions::segments` and the
    // burn reads that. This test fails if the two ever start agreeing for the
    // wrong reason, and names the disagreement if they do.
    let track = track_with(vec![source_cue("c1", 2_500_000, 500_000)]);
    let resolved = resolve(&track, &map(), safe(), &variant()).expect("the cue survives");
    let mut req = request(vec![CueCoord {
        at: us(1_250_000),
        duration: us(250_000),
    }]);
    req.captions.coordinate = Coordinate::Composition;
    let plan = compile(&req).expect("the request compiles");
    let plan_at = plan.cues[0].at;

    assert_eq!(
        plan_at,
        us(2_500_000),
        "the plan echoes the map's answer to a composition query, which is source time"
    );
    assert_ne!(
        plan_at,
        us(1_250_000),
        "if this ever changes, `crates/editor/src/render/compose.rs` was fixed and this \
         test should be rewritten to read the plan as authoritative"
    );
    assert_eq!(
        resolved.segments[0].start_sec, 1.25,
        "the export's own answer is the composition position"
    );
}

#[test]
fn a_cue_whose_clock_does_not_survive_a_round_trip_is_a_typed_error() {
    // The declaration is in the coordinate's type, so the only way to get it
    // wrong is to hand the deserialiser a clock that does not exist — and that
    // is a typed error rather than a silent default.
    let raw = r#"{"base_revision":"rev-1","cues":[{"id":"c1","text":"hi",
        "speaker":null,"start":{"at":2500000,"coordinate":"wall_clock"},
        "duration":500000}],"style":{"title":"t","font":"Inter","font_size":56,
        "outline_px":2.0,"shadow_px":1.5,"alignment":2,"margin_v_px":80},
        "translation":{"kind":"identity","backend":"identity",
        "source_lang":"en","target_lang":"en"}}"#;
    assert!(
        serde_json::from_str::<CaptionTrack>(raw).is_err(),
        "an unknown clock must not round-trip as some other clock"
    );
}

#[test]
fn a_track_round_trips_through_json_with_its_clock_intact() {
    let track = track_with(vec![source_cue("c1", 2_500_000, 500_000)]);
    let body = serde_json::to_string(&track).expect("the track serialises");
    let back: CaptionTrack = serde_json::from_str(&body).expect("the track reads back");
    assert_eq!(back, track);
    assert!(
        body.contains(r#""coordinate":"source""#),
        "the declared clock is in the document, not implied: {body}"
    );
}

#[test]
fn a_cue_with_no_length_is_refused_rather_than_drawn_for_no_time() {
    let outcome = CaptionCue::in_source("zero", "hi", None, us(1_000_000), MediaTime::ZERO);
    assert!(matches!(outcome, Err(CaptionError::EmptyCue { .. })));
}
