//! Transcript **edits**, and the two things todo 41 left for this todo: which
//! transcript revision a project is editing, and the `unsaved_json` document
//! shape.
//!
//! Every claim here is a *refusal* or a *degradation* as much as it is a
//! success: a transcript-editing surface that refused nothing would pass the
//! happy-path tests in this file. The happy paths are the control.

mod transcript;

use strivo_editor::transcript::{
    Correction, Linked, Timing, TranscriptDraft, TranscriptError, UnsavedWork, WordRange,
};
use transcript::{
    composition, id, segment_timed_transcript, transcript, untimed_transcript, us, MEDIA_US,
    REVISION,
};

fn linked() -> Linked {
    Linked::new(
        transcript(),
        composition(),
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a word-timed transcript over the whole source is a usable link")
}

fn linked_with(doc: &str) -> Linked {
    let t = match doc {
        "segment" => segment_timed_transcript(),
        _ => untimed_transcript(),
    };
    Linked::new(
        t,
        composition(),
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a link is refused at construction, so reaching here means the link built")
}

/// **Correct-text mode leaves the media intact** — and the type makes that so:
/// `correct_text` takes `&self` and returns a new transcript, so there is no
/// path through it that can reach the composition at all. The byte-level proof
/// that the *decoded media* is untouched is in `transcript_media.rs`; what is
/// asserted here is that the composition map is byte-identical afterwards and
/// that every word's seek target is unchanged to the microsecond.
#[test]
fn correcting_a_word_leaves_the_composition_byte_identical() {
    let link = linked();
    let before_map = serde_json::to_string(link.map()).expect("a map serialises");
    let before_targets: Vec<i64> = ["w1", "w2", "w3", "w4", "w5"]
        .iter()
        .map(|w| {
            link.seek_target(&id(w))
                .expect("the word is shown")
                .micros()
        })
        .collect();

    let corrected = link
        .correct_text(&id("w3"), "roughly")
        .expect("w3 exists and the new text is not empty");

    let after = Linked::new(
        corrected,
        link.map().clone(),
        link.source().clone(),
        link.revision().map(str::to_string),
        link.media_duration(),
    )
    .expect("the corrected document is still a usable link");

    assert_eq!(
        serde_json::to_string(after.map()).expect("a map serialises"),
        before_map,
        "a text correction must not change one byte of the composition"
    );
    let after_targets: Vec<i64> = ["w1", "w2", "w3", "w4", "w5"]
        .iter()
        .map(|w| after.seek_target(&id(w)).expect("shown").micros())
        .collect();
    assert_eq!(
        after_targets, before_targets,
        "and must not move one word by one microsecond"
    );
    // The word keeps its identity: a correction names a word, it does not
    // replace one, so nothing downstream can mistake it for a different word.
    assert_eq!(
        after.transcript().word(&id("w3")).expect("w3").text,
        "roughly"
    );
    assert_eq!(after.transcript().word(&id("w3")).expect("w3").id, id("w3"));
    // And its timing is untouched too — correcting a word must not move it.
    assert_eq!(
        after
            .transcript()
            .word(&id("w3"))
            .expect("w3")
            .at
            .expect("timed"),
        transcript::span(2_000_000, 3_500_000)
    );
}

/// Correcting a word to empty text is refused, because an empty word is not a
/// correction — it is a deletion wearing the wrong interface, and a deletion
/// has to go through the composition.
#[test]
fn correcting_a_word_to_nothing_is_refused_rather_than_deleting_it() {
    assert!(matches!(
        linked().correct_text(&id("w3"), "   "),
        Err(TranscriptError::EmptyText { word }) if word == "w3"
    ));
    assert!(matches!(
        linked().correct_text(&id("w3"), ""),
        Err(TranscriptError::EmptyText { word }) if word == "w3"
    ));
}

/// **Deleting a word range produces the expected composition interval** — the
/// plan's second acceptance clause, with the expected values written out.
#[test]
fn deleting_a_word_range_produces_the_expected_composition_intervals() {
    let edited = linked()
        .delete(&id("w2"), &id("w3"))
        .expect("a word-timed transcript can be cut exactly");

    // Two spans survive, and their output intervals are exact.
    let entries = edited.map().entries();
    assert_eq!(entries.len(), 2, "the cut splits one span into two");
    match (&entries[0], &entries[1]) {
        (
            strivo_editor::time::Entry::Source { span, output },
            strivo_editor::time::Entry::Source {
                span: after,
                output: out2,
            },
        ) => {
            assert_eq!(span.range, transcript::span(0, 1_000_000));
            assert_eq!(*output, transcript::span(0, 1_000_000));
            assert_eq!(after.range, transcript::span(3_500_000, MEDIA_US));
            assert_eq!(*out2, transcript::span(1_000_000, 7_500_000));
        }
        other => panic!("both survivors must be source spans, got {other:?}"),
    }
    // The removed words are gone from the text, by id.
    let remaining: Vec<String> = edited
        .transcript()
        .words()
        .map(|w| w.id.as_str().to_string())
        .collect();
    assert_eq!(remaining, vec!["w1", "w4", "w5"]);
    // And the cut is reported as a source range, which is what a render plan
    // needs in order to become a composition revision.
    let cut = edited.cut().expect("a deletion records what it removed");
    assert_eq!(cut.source.as_str(), transcript::SOURCE_ID);
    assert_eq!(cut.range, transcript::span(1_000_000, 3_500_000));
    assert_eq!(cut.output_removed.micros(), 2_500_000);
}

/// **Undo restores both text and composition** — both, not the one that was
/// easy.
#[test]
fn undo_restores_both_the_text_and_the_composition() {
    let link = linked();
    let before_text = serde_json::to_string(link.transcript()).expect("serialises");
    let before_map = serde_json::to_string(link.map()).expect("serialises");

    let edited = link
        .delete(&id("w2"), &id("w3"))
        .expect("a word-timed transcript can be cut exactly");
    assert_ne!(
        serde_json::to_string(edited.transcript()).expect("serialises"),
        before_text,
        "the control: the edit really did change the text"
    );
    assert_ne!(
        serde_json::to_string(edited.map()).expect("serialises"),
        before_map,
        "the control: the edit really did change the composition"
    );

    let undone = edited.undo();
    assert_eq!(
        serde_json::to_string(&undone.transcript).expect("serialises"),
        before_text,
        "undo restores the text"
    );
    assert_eq!(
        serde_json::to_string(&undone.map).expect("serialises"),
        before_map,
        "undo restores the composition"
    );
}

/// A **provider without word timing is explicitly degraded, not faked.** The
/// label is a stable token so a UI can render it and a test can assert it, and
/// interpolating a word's timing from its segment's would be the fabrication
/// this refuses.
#[test]
fn a_provider_without_word_timing_is_labelled_degraded_with_a_stable_token() {
    let link = linked_with("segment");
    let timing = link.transcript().timing();
    assert_eq!(timing, Timing::SegmentLevel);
    assert!(timing.is_degraded());
    assert_eq!(timing.token(), "segment", "one spelling, in both forms");
    assert_eq!(timing.degraded_reason(), Some("transcript/no-word-timing"));
    // And the wire form a UI renders carries both, so "degraded" and "why" are
    // one value rather than two things to keep in step.
    let wire = serde_json::to_value(timing.wire()).expect("serialises");
    assert_eq!(wire["timing"], "segment");
    assert_eq!(wire["degraded"], true);
    assert_eq!(wire["degraded_reason"], "transcript/no-word-timing");
}

#[test]
fn a_transcript_with_no_timings_at_all_is_a_named_state_rather_than_zero() {
    let link = linked_with("none");
    let timing = link.transcript().timing();
    assert_eq!(timing, Timing::Untimed);
    assert_eq!(timing.token(), "untimed");
    assert_eq!(timing.degraded_reason(), Some("transcript/no-timing"));
    // The measured position is absent, not zero. A zero-length segment is a
    // legal thing somebody said, and reading it as "no timing" is the defect
    // class this todo inherits.
    let word = link.transcript().word(&id("w1")).expect("w1");
    assert_eq!(word.at, None, "absent, not a measured zero");
    assert!(
        matches!(
            link.seek_target(&id("w1")),
            Err(TranscriptError::NoWordTiming { word }) if word == "w1"
        ),
        "and seeking it is refused rather than answered with 0"
    );
}

#[test]
fn a_word_timed_transcript_is_not_labelled_degraded() {
    let timing = linked().transcript().timing();
    assert_eq!(timing, Timing::WordLevel);
    assert!(!timing.is_degraded());
    assert_eq!(timing.degraded_reason(), None);
    let wire = serde_json::to_value(timing.wire()).expect("serialises");
    assert_eq!(wire["timing"], "word");
    assert_eq!(wire["degraded"], false);
    assert_eq!(wire["degraded_reason"], serde_json::Value::Null);
}

/// **A word with a zero duration is legitimate** and must not be treated as
/// absent. A measured zero is a real measurement; a `||` guard that turns it
/// into a default is the defect class this todo inherits from todo 29.
#[test]
fn a_zero_duration_word_is_measured_not_missing() {
    let mut doc = transcript();
    // w1 ends exactly where it starts.
    doc.segments[0].words[0].at = Some(transcript::span(0, 0));
    let link = Linked::new(
        doc,
        composition(),
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a zero-length word is a legal measurement");
    let word = link.transcript().word(&id("w1")).expect("w1");
    assert!(word.at.is_some(), "present, not absent");
    assert_eq!(
        word.at.expect("timed").duration().expect("length").micros(),
        0
    );
    assert_eq!(link.transcript().timing(), Timing::WordLevel);
    assert_eq!(link.seek_target(&id("w1")).expect("shown"), us(0));
}

/// A degraded transcript may be **corrected** — text-only work is always
/// safe — but not **cut**, because a cut at a resolution the provider never
/// reported would be a guess about where the media is.
#[test]
fn a_degraded_transcript_can_be_corrected_but_refuses_to_be_cut() {
    let link = linked_with("segment");
    let corrected = link
        .correct_text(&id("w2"), "actually")
        .expect("a text correction needs no word timing");
    assert_eq!(
        corrected.word(&id("w2")).expect("w2").text.as_str(),
        "actually"
    );
    assert!(matches!(
        link.delete(&id("w2"), &id("w3")),
        Err(TranscriptError::NotExactEnough { timing, .. })
            if timing == Timing::SegmentLevel
    ));
    // And the whole composition is untouched by the attempt.
    assert_eq!(link.map().output_duration(), us(MEDIA_US));
}

// ---------------------------------------------------------------------------
// The two things todo 41 left: which revision, and the draft's shape.
// ---------------------------------------------------------------------------

/// **The transcript-revision selection.** There is no second selection store
/// and no selection column: the transcript revision a project is editing is the
/// one the link was built from, and a draft that names any other revision is
/// reported as superseded rather than silently re-pointed at the newer one.
#[test]
fn an_edit_against_a_superseded_revision_is_reported_not_re_pointed() {
    let link = linked();
    // The link was built from REVISION. A draft naming a different revision is
    // a re-transcription's descendant and is refused.
    let draft = TranscriptDraft {
        base_revision: "9a2b7d10-0000-4000-8000-0000000000ff".to_string(),
        corrected: vec![Correction {
            word: id("w3"),
            text: "roughly".to_string(),
        }],
        deleted: vec![],
    };
    assert!(matches!(
        link.apply(&draft),
        Err(TranscriptError::RevisionSuperseded { base, current })
            if base == "9a2b7d10-0000-4000-8000-0000000000ff" && current == REVISION
    ));
    // The same revision applies.
    let draft = TranscriptDraft {
        base_revision: REVISION.to_string(),
        ..draft
    };
    assert!(link.apply(&draft).is_ok());
}

/// A link built against an older revision, used after a re-transcription, is
/// the case this exists for: the re-transcription is a *new immutable
/// revision*, so the old one is still readable and the edit is still anchored
/// to it — and the fact that a newer one exists is what is reported.
#[test]
fn a_link_built_from_an_older_revision_says_so_when_a_newer_one_is_named() {
    let old = Linked::new(
        transcript(),
        composition(),
        transcript::source(),
        Some("9a2b7d10-0000-4000-8000-0000000000aa".to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("an older link is still a link");
    assert_eq!(old.revision(), Some("9a2b7d10-0000-4000-8000-0000000000aa"));
    let draft = TranscriptDraft {
        base_revision: REVISION.to_string(),
        corrected: vec![],
        deleted: vec![],
    };
    assert!(matches!(
        old.apply(&draft),
        Err(TranscriptError::RevisionSuperseded { base, current })
            if base == REVISION && current == "9a2b7d10-0000-4000-8000-0000000000aa"
    ));
}

/// **The `unsaved_json` document shape**, round-tripped: one JSON object, one
/// `transcript` member, and every field named. This is the contract a
/// downstream todo reads, so it is asserted rather than described.
#[test]
fn the_unsaved_work_document_has_exactly_the_declared_shape() {
    let draft = TranscriptDraft {
        base_revision: REVISION.to_string(),
        corrected: vec![Correction {
            word: id("w3"),
            text: "roughly".to_string(),
        }],
        deleted: vec![WordRange {
            from_word: id("w2"),
            to_word: id("w3"),
        }],
    };
    let work = UnsavedWork {
        transcript: Some(draft.clone()),
    };
    let wire = serde_json::to_string(&work).expect("serialises");
    assert_eq!(
        wire,
        format!(
            r#"{{"transcript":{{"base_revision":"{REVISION}","corrected":[{{"word":"w3","text":"roughly"}}],"deleted":[{{"from_word":"w2","to_word":"w3"}}]}}}}"#
        ),
        "the wire shape is the contract; a downstream todo reads exactly this"
    );
    assert_eq!(
        serde_json::from_str::<UnsavedWork>(&wire).expect("round-trips"),
        work
    );
    // The empty form is a valid object, which is what the workspace stores
    // before anything is unsaved.
    assert_eq!(
        serde_json::to_string(&UnsavedWork::default()).expect("serialises"),
        "{}"
    );
    // And an empty transcript member is a discard, not an edit with no work.
    assert_eq!(
        serde_json::from_str::<UnsavedWork>(r#"{"transcript":null}"#)
            .expect("parses")
            .transcript,
        None
    );
}

/// The draft's own refusals: a correction or a deletion naming a word the
/// base revision does not contain is refused **before** anything is applied,
/// so a bad draft leaves the document and the composition as they were.
#[test]
fn a_draft_naming_a_word_that_does_not_exist_is_refused_and_changes_nothing() {
    let link = linked();
    let before = serde_json::to_string(link.map()).expect("serialises");
    for draft in [
        TranscriptDraft {
            base_revision: REVISION.to_string(),
            corrected: vec![Correction {
                word: id("nope"),
                text: "x".to_string(),
            }],
            deleted: vec![],
        },
        TranscriptDraft {
            base_revision: REVISION.to_string(),
            corrected: vec![],
            deleted: vec![WordRange {
                from_word: id("nope"),
                to_word: id("w3"),
            }],
        },
        TranscriptDraft {
            base_revision: REVISION.to_string(),
            corrected: vec![],
            deleted: vec![WordRange {
                from_word: id("w3"),
                to_word: id("w2"),
            }],
        },
    ] {
        assert!(
            matches!(
                link.apply(&draft),
                Err(TranscriptError::WordNotFound { .. }) | Err(TranscriptError::NotForward { .. })
            ),
            "a draft naming an absent or reversed word must be refused, got {:?}",
            link.apply(&draft).err()
        );
    }
    assert_eq!(
        serde_json::to_string(link.map()).expect("serialises"),
        before,
        "and a refused draft changes nothing"
    );
    assert_eq!(link.transcript().words().count(), 5, "no word was removed");
}

/// Applying a draft is one operation: a correction *and* a deletion together
/// land as a single new pair, and a draft is applied against the base rather
/// than accumulated onto itself.
#[test]
fn a_draft_carrying_a_correction_and_a_deletion_lands_as_one_pair() {
    let edited = linked()
        .apply(&TranscriptDraft {
            base_revision: REVISION.to_string(),
            corrected: vec![Correction {
                word: id("w5"),
                text: "yes".to_string(),
            }],
            deleted: vec![WordRange {
                from_word: id("w1"),
                to_word: id("w1"),
            }],
        })
        .expect("a well-formed draft applies");
    let remaining: Vec<String> = edited
        .transcript()
        .words()
        .map(|w| w.id.as_str().to_string())
        .collect();
    assert_eq!(remaining, vec!["w2", "w3", "w4", "w5"]);
    assert_eq!(
        edited.transcript().word(&id("w5")).expect("w5").text,
        "yes",
        "and the correction landed on the surviving word"
    );
    assert_eq!(
        edited.cut().expect("a deletion was requested").range,
        transcript::span(0, 1_000_000)
    );
}

// ---------------------------------------------------------------------------
// Prompt injection: a transcript is model-produced text that reaches a UI.
// ---------------------------------------------------------------------------

/// Identity is a [`WordId`](strivo_editor::transcript::WordId), never the text.
/// A word whose text is a script tag, a premature closing tag, or ten
/// megabytes of markup is still one word with one id, and correcting it or
/// seeking to it is an ordinary operation on data.
#[test]
fn model_produced_text_is_data_and_identity_is_never_derived_from_it() {
    let payloads: Vec<String> = [
        "<script>alert(1)</script>".to_string(),
        "</div><div onclick=\"steal()\">".to_string(),
        "a".repeat(10 * 1024 * 1024),
        "\u{202e}gnp.exe".to_string(),
        "\u{0}null".to_string(),
    ]
    .to_vec();
    for payload in payloads {
        let mut doc = transcript();
        doc.segments[0].words[0].text = payload.clone();
        let link = Linked::new(
            doc,
            composition(),
            transcript::source(),
            Some(REVISION.to_string()),
            Some(us(MEDIA_US)),
        )
        .expect("a word's text is data, whatever it contains");

        // It is seekable, at the position its timing names.
        assert_eq!(link.seek_target(&id("w1")).expect("shown"), us(0));
        // It keeps its identity, which is not its text.
        assert_eq!(link.transcript().word(&id("w1")).expect("w1").id, id("w1"));
        assert_eq!(
            link.transcript().word(&id("w1")).expect("w1").text,
            payload,
            "and the text round-trips byte-for-byte"
        );
        // Correcting it works and does not touch anything else.
        let corrected = link
            .correct_text(&id("w1"), "safe")
            .expect("a correction is data too");
        assert_eq!(corrected.word(&id("w1")).expect("w1").text, "safe");
        // And it survives the wire unchanged apart from the correction.
        let mut wire = corrected;
        wire.segments[0].words[0].text = payload.clone();
        let round: strivo_editor::transcript::Transcript =
            serde_json::from_str(&serde_json::to_string(&wire).expect("serialises"))
                .expect("round-trips");
        assert_eq!(round.segments[0].words[0].text, payload);
    }
}

/// Two words with the same text stay two words. A link that matched on text
/// would collapse them, and a word's identity is what every edit names.
#[test]
fn two_words_with_the_same_text_stay_two_words() {
    let mut doc = transcript();
    doc.segments[0].words[0].text = "the".to_string();
    doc.segments[0].words[1].text = "the".to_string();
    let link = Linked::new(
        doc,
        composition(),
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a repeated word is legal");
    assert_eq!(link.transcript().words().count(), 5);
    let corrected = link.correct_text(&id("w2"), "THE").expect("w2 exists");
    assert_eq!(corrected.word(&id("w1")).expect("w1").text, "the");
    assert_eq!(corrected.word(&id("w2")).expect("w2").text, "THE");
    // And the two keep their distinct seek targets.
    assert_eq!(link.seek_target(&id("w1")).expect("w1"), us(0));
    assert_eq!(link.seek_target(&id("w2")).expect("w2"), us(1_000_000));
}

/// Overlapping segments are a malformed document, and the refusal names the
/// two segments rather than reporting a number.
#[test]
fn overlapping_segments_are_refused_by_name() {
    let mut doc = transcript();
    doc.segments[0].at = Some(transcript::span(0, 4_000_000));
    doc.segments[1].at = Some(transcript::span(3_500_000, 5_000_000));
    assert!(matches!(
        Linked::new(
            doc,
            composition(),
            transcript::source(),
            Some(REVISION.to_string()),
            Some(us(MEDIA_US)),
        ),
        Err(TranscriptError::OverlappingSegments {
            first: 0,
            second: 1
        })
    ));
}

/// A word whose timing is not inside its own segment is a malformed document:
/// the two disagree about when the word was said, and picking either would be
/// an interpolation.
#[test]
fn a_word_timed_outside_its_own_segment_is_refused() {
    let mut doc = transcript();
    // w5 is declared at 4..5s inside a segment that ends at 5s — fine. Move it
    // into a segment that does not contain it.
    doc.segments[1].words[0].at = Some(transcript::span(0, 100));
    assert!(matches!(
        Linked::new(
            doc,
            composition(),
            transcript::source(),
            Some(REVISION.to_string()),
            Some(us(MEDIA_US)),
        ),
        Err(TranscriptError::WordOutsideSegment { word, .. }) if word == "w4"
    ));
}

/// An inverted segment is **unrepresentable in Rust** and refused on the way
/// in from stored bytes.
///
/// `Interval::new` refuses one and no literal can build one — but `Interval`
/// derives `Deserialize`, so a document read back from `body_json` can arrive
/// inverted without ever passing the constructor. That is a real hole in the
/// type, and the link is the one place that knows the document came from
/// outside, so the check lives here.
#[test]
fn an_inverted_segment_read_from_stored_bytes_is_refused() {
    let raw = r#"{"segments":[
        {"at":{"start":0,"end":3500000},"words":[]},
        {"at":{"start":9000000,"end":1000000},"words":[]}
    ]}"#;
    let doc: strivo_editor::transcript::Transcript =
        serde_json::from_str(raw).expect("the bytes parse into a document");
    assert!(
        matches!(
            Linked::new(
                doc,
                composition(),
                transcript::source(),
                Some(REVISION.to_string()),
                Some(us(MEDIA_US)),
            ),
            Err(TranscriptError::Time(
                strivo_editor::time::TimeError::Inverted {
                    start: 9_000_000,
                    end: 1_000_000
                }
            ))
        ),
        "an inverted span is refused rather than left to make a word \
         silently unseekable"
    );
}
