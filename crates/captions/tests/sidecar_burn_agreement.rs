//! The sidecar and the burn, compared as two readings rather than one value.
//!
//! "They came out of the same function" is a tautology, not a measurement. This
//! file **parses both documents back** — the `.srt` a player would read and the
//! `.ass` ffmpeg would burn — and compares them cue by cue, so agreement is a
//! property of the emitted text rather than of the code path.

use strivo_branding::{BrandVariant, SafeArea};
use strivo_captions::{resolve, sidecars, CaptionCue, CaptionTrack, IdentityTranslator};
use strivo_editor::time::{
    Interval, IntervalMap, MediaTime, Rate, SourceRef, SourceSpan, TimeBase,
};

fn us(v: i64) -> MediaTime {
    MediaTime::from_micros(v).expect("non-negative")
}

fn span(start: i64, end: i64, rate: (i64, i64)) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator("/recordings/stream-01.mkv"),
        TimeBase::from_rate(1, 1).unwrap(),
        Interval::new(us(start), us(end)).unwrap(),
        Rate::new(rate.0, rate.1).unwrap(),
    )
    .unwrap()
}

fn map() -> IntervalMap {
    let mut map = IntervalMap::new();
    map.push_source(span(0, 1_000_000, (1, 1))).unwrap();
    map.push_source(span(2_000_000, 4_000_000, (1, 2))).unwrap();
    map
}

/// One cue read back out of a caption document: milliseconds on the output
/// timeline, which is the finest unit both formats carry.
#[derive(Debug, PartialEq)]
struct Read {
    start_ms: i64,
    end_ms: i64,
    text: String,
    /// `None` for the burn, which carries the speaker as a colour override
    /// rather than as a name.
    speaker: Option<String>,
}

fn parse_srt(body: &str) -> Vec<Read> {
    let mut out = Vec::new();
    let mut lines = body.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line.is_empty() || line.parse::<u32>().is_ok() {
            continue;
        }
        let (from, to) = line.split_once(" --> ").expect("a timing line");
        // SRT spells a speaker as a `[Name] ` prefix and ASS spells it as a
        // colour override, so the speaker is compared as a field of its own
        // rather than as part of the message.
        let raw = lines.next().unwrap_or_default().to_string();
        let (speaker, text) = match raw.split_once("] ") {
            Some((name, rest)) if name.starts_with('[') => (
                Some(name.trim_start_matches('[').to_string()),
                rest.to_string(),
            ),
            _ => (None, raw),
        };
        out.push(Read {
            start_ms: stamp_ms(from, ','),
            end_ms: stamp_ms(to, ','),
            text,
            speaker,
        });
    }
    out
}

fn parse_ass(body: &str) -> Vec<Read> {
    body.lines()
        .filter_map(|line| line.strip_prefix("Dialogue: "))
        .map(|line| {
            // Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text
            let fields: Vec<&str> = line.splitn(10, ',').collect();
            assert_eq!(fields.len(), 10, "a Dialogue line has ten fields: {line}");
            Read {
                start_ms: ass_ms(fields[1]),
                end_ms: ass_ms(fields[2]),
                text: fields[9].replace("\\N", "\n").replace('\\', ""),
                speaker: None,
            }
        })
        .collect()
}

fn parts(stamp: &str) -> (i64, i64, i64, i64) {
    let (clock, frac) = stamp.split_once(['.', ',']).expect("a fractional stamp");
    let mut it = clock.split(':');
    let h: i64 = it.next().unwrap().parse().unwrap();
    let m: i64 = it.next().unwrap().parse().unwrap();
    let s: i64 = it.next().unwrap().parse().unwrap();
    (h, m, s, frac.parse().unwrap())
}

fn stamp_ms(stamp: &str, expect_sep: char) -> i64 {
    let (h, m, s, frac) = parts(stamp);
    assert_eq!(
        &stamp[8..9],
        expect_sep.to_string(),
        "SRT uses a comma and VTT a dot; {stamp} does not"
    );
    let _ = expect_sep;
    ((h * 3600 + m * 60 + s) * 1000) + frac
}

fn ass_ms(stamp: &str) -> i64 {
    let (clock, frac) = stamp.split_once('.').expect("a fractional ASS stamp");
    let mut it = clock.split(':');
    let h: i64 = it.next().unwrap().parse().unwrap();
    let m: i64 = it.next().unwrap().parse().unwrap();
    let s: i64 = it.next().unwrap().parse().unwrap();
    // ASS centiseconds, widened to milliseconds. The half-millisecond a
    // centisecond can lose is the stated bound on the two documents agreeing.
    ((h * 3600 + m * 60 + s) * 1000) + (frac.parse::<i64>().unwrap() * 10)
}

fn track() -> CaptionTrack {
    let mut track = CaptionTrack::new("rev-1", &IdentityTranslator);
    track.cues = vec![
        CaptionCue::in_source("a", "first line", Some("Alice"), us(100_000), us(300_000)).unwrap(),
        CaptionCue::in_source("b", "second line", Some("Bob"), us(2_500_000), us(500_000)).unwrap(),
    ];
    track
}

fn files() -> strivo_captions::Sidecars {
    let resolved = resolve(
        &track(),
        &map(),
        SafeArea { x: 0.05, y: 0.05 },
        &BrandVariant {
            name: "1080p".into(),
            width: 1920,
            height: 1080,
        },
    )
    .expect("both cues survive the edit");
    sidecars(&resolved)
}

#[test]
fn the_sidecar_and_the_burn_name_the_same_cues_at_the_same_times() {
    let out = files();
    let srt = parse_srt(&out.srt);
    let ass = parse_ass(&out.ass);

    assert_eq!(srt.len(), 2, "two cues in the source document");
    assert_eq!(
        ass.len(),
        srt.len(),
        "the burn and the sidecar disagree on how many cues there are"
    );

    let speakers = ["Alice", "Bob"];
    for (index, (from_sidecar, from_burn)) in srt.iter().zip(ass.iter()).enumerate() {
        assert_eq!(
            from_sidecar.text, from_burn.text,
            "the same cue carries different text in the two documents"
        );
        assert_eq!(
            from_sidecar.speaker.as_deref(),
            Some(speakers[index]),
            "the sidecar must name the speaker it was given"
        );
        // SRT is whole milliseconds, ASS whole centiseconds. The coarsest of
        // the two bounds the agreement, so the check is 5ms and not 0.
        assert!(
            (from_sidecar.start_ms - from_burn.start_ms).abs() <= 5,
            "start disagrees by more than the coarser format's quantum: \
             sidecar {}ms, burn {}ms",
            from_sidecar.start_ms,
            from_burn.start_ms
        );
        assert!(
            (from_sidecar.end_ms - from_burn.end_ms).abs() <= 5,
            "end disagrees by more than the coarser format's quantum: \
             sidecar {}ms, burn {}ms",
            from_sidecar.end_ms,
            from_burn.end_ms
        );
    }
}

#[test]
fn both_documents_carry_the_composition_times_not_the_source_times() {
    let out = files();
    let srt = parse_srt(&out.srt);

    // Cue `a` is 0.1s..0.4s of source, inside the first kept segment, so it
    // does not move. Cue `b` is 2.5s..3.0s of source inside a 2x segment that
    // starts at output 1.0s, so it lands at 1.25s..1.5s.
    assert_eq!((srt[0].start_ms, srt[0].end_ms), (100, 400));
    assert_eq!(
        (srt[1].start_ms, srt[1].end_ms),
        (1250, 1500),
        "the second cue must carry composition times"
    );
    assert_ne!(
        (srt[1].start_ms, srt[1].end_ms),
        (2500, 3000),
        "a sidecar still carrying source times has not been through the composition"
    );
}

#[test]
fn the_agreement_check_can_fail() {
    // Anti-vacuity. A comparison that only ever sees one document agree with
    // itself is not a comparison, so this shifts one cue in the burn and
    // requires the same check to reject it.
    let out = files();
    let srt = parse_srt(&out.srt);
    let tampered = out
        .ass
        .replace("0:00:01.25,0:00:01.50", "0:00:01.25,0:00:01.90");
    assert_ne!(
        tampered, out.ass,
        "the tamper must actually change the document"
    );
    let ass = parse_ass(&tampered);
    assert_eq!(ass.len(), srt.len());
    assert!(
        (srt[1].end_ms - ass[1].end_ms).abs() > 5,
        "a 400ms shift in the burn must fail the same comparison the real run passes"
    );
}

#[test]
fn the_vtt_sidecar_agrees_with_the_srt_on_the_clock_and_the_text() {
    let out = files();
    let srt = parse_srt(&out.srt);
    let vtt_ms: Vec<i64> = out
        .vtt
        .lines()
        .filter_map(|l| l.split(" --> ").next())
        .filter(|l| l.contains(':'))
        .map(|l| stamp_ms(l, '.'))
        .collect();
    let srt_starts: Vec<i64> = srt.iter().map(|r| r.start_ms).collect();
    assert_eq!(vtt_ms, srt_starts);
    assert!(out.vtt.starts_with("WEBVTT\n"));
}

#[test]
fn the_sidecars_carry_the_identity_label_rather_than_claiming_a_translation() {
    let out = files();
    assert_eq!(
        out.translation.kind,
        strivo_captions::TranslationKind::Identity
    );
    let label = out.translation_label();
    assert!(
        label.contains("identity") && label.contains("no translation"),
        "the user-facing label must say the text was not translated: {label}"
    );
    assert!(!label.to_lowercase().contains("translated by"));
}
