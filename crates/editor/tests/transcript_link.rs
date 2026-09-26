//! The link between a transcript word and the media timeline is exact and
//! bidirectional.
//!
//! "Exact" is the whole claim, so every expectation here is a hand-written
//! integer microsecond rather than a value recomputed by the code under test,
//! and the comparison is `assert_eq!` on [`MediaTime`] — a seek that lands on
//! the right word but the wrong microsecond is **not** the same instant, and a
//! tolerance would hide exactly the defect this todo exists to prevent.
//!
//! The fixture's composition is a straight copy of the source, so before any
//! edit a composition coordinate *is* a source coordinate. That is what makes
//! the numbers checkable by eye.

mod transcript;

use strivo_editor::time::{IntervalMap, MediaTime};
use strivo_editor::transcript::{Linked, TranscriptError};
use transcript::{composition, id, transcript, us, MEDIA_US, REVISION, SOURCE_ID};

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

/// A transcript with no word timings satisfies "a word seeks to where it says"
/// for free, because there is nowhere it says. The fixture therefore proves
/// its subject is non-empty *before* any exactness claim: five words, each
/// with its own timing, each distinct.
#[test]
fn the_fixture_really_has_word_timings_before_anything_is_asserted_about_them() {
    let link = linked();
    let words: Vec<_> = link.transcript().words().collect();
    assert_eq!(words.len(), 5, "five words, not an empty subject");
    assert!(
        words.iter().all(|w| w.at.is_some()),
        "every fixture word carries a timing"
    );
    let starts: Vec<i64> = words
        .iter()
        .map(|w| w.at.expect("timed").start().micros())
        .collect();
    assert_eq!(
        starts,
        vec![0, 1_000_000, 2_000_000, 3_500_000, 4_000_000],
        "and the timings are the ones the table above names"
    );
    assert_eq!(link.transcript().timing().token(), "word");
    assert!(
        !link.transcript().timing().is_degraded(),
        "a word-timed transcript is not degraded"
    );
}

/// The load-bearing claim, part one: before any edit, a word's seek target is
/// the position the transcript names, to the microsecond.
#[test]
fn an_unedited_word_seeks_to_the_exact_instant_its_transcript_names() {
    let link = linked();
    assert_eq!(link.seek_target(&id("w1")).expect("w1 is shown"), us(0));
    assert_eq!(
        link.seek_target(&id("w2")).expect("w2 is shown"),
        us(1_000_000)
    );
    assert_eq!(
        link.seek_target(&id("w3")).expect("w3 is shown"),
        us(2_000_000)
    );
    assert_eq!(
        link.seek_target(&id("w4")).expect("w4 is shown"),
        us(3_500_000)
    );
    assert_eq!(
        link.seek_target(&id("w5")).expect("w5 is shown"),
        us(4_000_000)
    );
}

/// Exactness is a two-way claim, so the seek target is checked against the
/// canonical map as well: the transcript's own arithmetic and
/// [`IntervalMap::source_at`] must agree to the microsecond, not merely to
/// within a frame.
#[test]
fn the_seek_target_is_an_exact_inverse_of_the_canonical_interval_map() {
    let link = linked();
    for word in ["w1", "w2", "w3", "w4", "w5"] {
        let target = link.seek_target(&id(word)).expect("the word is shown");
        let mapped = link
            .map()
            .source_at(target)
            .expect("the seek target is inside the map");
        let named = link
            .transcript()
            .word(&id(word))
            .expect("a fixture word")
            .at
            .expect("a timed word")
            .start();
        assert_eq!(
            mapped.at, named,
            "{word}: the round trip must be exact, not approximate"
        );
        assert_eq!(
            mapped.uncertainty_micros, 0,
            "{word}: a straight copy carries no uncertainty"
        );
    }
}

/// The reverse direction is a real direction, not a convenience: asking where
/// a composition instant is in the transcript returns the words spoken there.
#[test]
fn a_composition_instant_resolves_back_to_the_words_spoken_there() {
    let link = linked();
    // 2 400 000 is inside w3's source range and inside w3 only.
    let ids = link.words_at(us(2_400_000)).expect("an instant inside w3");
    assert_eq!(ids, vec![id("w3")]);
    // 1 500 000 is inside w2.
    assert_eq!(
        link.words_at(us(1_500_000)).expect("an instant inside w2"),
        vec![id("w2")]
    );
    // 5 000 000 is the first microsecond past the last word: no word is
    // spoken there, and the answer is a refusal rather than the nearest word.
    assert!(matches!(
        link.words_at(us(5_000_000)),
        Err(TranscriptError::NoWordAt { at: 5_000_000, .. })
    ));
}

/// The composition interval a word range occupies is measured, not assumed:
/// two words that are adjacent in the text and in the source occupy one
/// contiguous output interval.
#[test]
fn a_word_range_occupies_the_expected_composition_interval() {
    let link = linked();
    let occupied = link
        .composition_span(&id("w2"), &id("w3"))
        .expect("w2..w3 is a forward range");
    assert_eq!(occupied.start().micros(), 1_000_000);
    assert_eq!(occupied.end().micros(), 3_500_000);
    assert_eq!(occupied.duration().expect("a length").micros(), 2_500_000);
}

/// A word range that runs backwards in document order has no interval, and is
/// reported rather than reordered.
#[test]
fn a_backwards_word_range_is_refused_rather_than_reordered() {
    assert!(matches!(
        linked().composition_span(&id("w3"), &id("w1")),
        Err(TranscriptError::NotForward { .. })
    ));
}

/// **The link is to a source identity, not a path.** A locator is a resolvable
/// location that moves; an id does not. The fixture's source is an id, and
/// this asserts the id survives the shape of the link rather than being
/// re-derived from anything on disk.
#[test]
fn the_link_names_a_source_identity_so_a_moved_file_cannot_move_a_word() {
    let link = linked();
    assert_eq!(link.source().as_str(), SOURCE_ID);
    // Every source span in the composition names the same identity.
    for entry in link.map().entries() {
        if let strivo_editor::time::Entry::Source { span, .. } = entry {
            assert_eq!(
                span.source.as_str(),
                SOURCE_ID,
                "a span naming a locator instead of an id would make the link \
                 resolve against a path, and a moved file would take the word \
                 with it"
            );
            assert!(
                !span.source.as_str().contains('/'),
                "the link must not contain a filesystem separator"
            );
        }
    }
}

/// A word the transcript does not contain is reported by name, not answered
/// with the nearest word that happens to exist.
#[test]
fn a_word_that_does_not_exist_is_refused_by_name() {
    assert!(matches!(
        linked().seek_target(&id("w99")),
        Err(TranscriptError::WordNotFound { word }) if word == "w99"
    ));
}

/// A word the composition does not show is `Unmapped`. It is never clamped to
/// the nearest shown instant, because a word at the edge of a cut is a word on
/// the wrong frame.
///
/// The construction matters: the word is **still in the transcript**, so this
/// is not the deletion case (a deleted word is gone from the text, and
/// `WordNotFound` is the right answer there). This is a word whose media the
/// composition does not carry — a hole where the source used to be — which is
/// the state a partial import or an earlier ripple-delete leaves behind.
#[test]
fn a_word_whose_media_is_not_in_the_composition_is_reported_and_never_clamped() {
    let mut map = IntervalMap::new();
    map.push_source(transcript::source_span(0, 2_000_000))
        .expect("append");
    map.push_source(transcript::source_span(3_500_000, MEDIA_US))
        .expect("append");
    let link = Linked::new(
        transcript(),
        map,
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a composition with a hole is still a composition");
    // The word is present, so this is not a lookup failure.
    assert!(link
        .transcript()
        .word(&id("w3"))
        .expect("w3 is in the text")
        .at
        .is_some());
    // Its media is entirely inside the hole.
    assert!(matches!(
        link.seek_target(&id("w3")),
        Err(TranscriptError::UnmappedSource { word, at: 2_000_000, .. }) if word == "w3"
    ));
    // Neither end of the hole is an acceptable answer.
    for edge in [2_000_000, 3_500_000] {
        assert!(
            !matches!(link.seek_target(&id("w3")), Ok(t) if t.micros() == edge),
            "clamping to the hole's edge at {edge}us is the defect this refuses"
        );
    }
    // A hole in the **source** is not a hole in the **output**: the removed
    // range shortens the composition rather than leaving a gap in it. The
    // second span is source [3 500 000, 10 000 000) at output
    // [2 000 000, 8 500 000), so output 2 400 000 is source 3 900 000 — inside
    // w4, which is the word that was really said there.
    assert_eq!(
        link.words_at(us(2_400_000))
            .expect("something is shown there"),
        vec![id("w4")],
        "2 400 000 of output is 3 900 000 of source, which is w4"
    );
    assert_eq!(
        link.seek_target(&id("w5")).expect("w5 is shown"),
        us(2_500_000),
        "and w5 is 4 000 000 of source, 500 000 into the second span"
    );
}

/// A real gap in the **output** — a placed `Gap`, which is what a title card
/// or a mixed-out track leaves — has no word behind it, and the reverse
/// direction says so rather than answering with the nearest one.
#[test]
fn a_gap_in_the_output_has_no_word_behind_it() {
    let mut map = IntervalMap::new();
    map.push_source(transcript::source_span(0, 1_000_000))
        .expect("append");
    map.push_gap(us(500_000))
        .expect("a zero-or-more gap is accepted");
    map.push_source(transcript::source_span(1_000_000, MEDIA_US))
        .expect("append");
    let link = Linked::new(
        transcript(),
        map,
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a composition with a gap is a composition");
    // Output [1 000 000, 1 500 000) is the gap.
    assert!(matches!(
        link.words_at(us(1_200_000)),
        Err(TranscriptError::NoWordAt { at: 1_200_000, .. })
    ));
    // The source after the gap is placed after it, so w2 sits 500 000 later
    // than its source position rather than at it.
    assert_eq!(
        link.seek_target(&id("w2")).expect("w2 is shown"),
        us(1_500_000),
        "source 1 000 000 comes after a 500 000 gap, so it plays at 1 500 000"
    );
    // And the round trip agrees.
    assert_eq!(
        link.map().source_at(us(1_500_000)).expect("shown").at,
        us(1_000_000)
    );
}

/// A word that was deleted is gone from the **text** as well as the media, so
/// asking for it is a lookup failure and not an unmapped instant. Reporting
/// the two differently is what lets a caller tell "this word never existed in
/// this revision" from "this word exists and its media is gone".
#[test]
fn a_deleted_word_is_gone_from_the_text_and_reported_as_such() {
    let edited = linked()
        .delete(&id("w2"), &id("w3"))
        .expect("a word-timed transcript can be cut exactly");
    for word in ["w2", "w3"] {
        assert!(matches!(
            edited.transcript().word(&id(word)),
            Err(TranscriptError::WordNotFound { .. })
        ));
    }
    assert_eq!(
        edited.transcript().words().count(),
        3,
        "and the surviving words are untouched"
    );
}

/// After a cut, every surviving word seeks to its new position, exactly.
///
/// Deleting `w2..w3` removes source `[1 000 000, 3 500 000)` — 2 500 000 µs —
/// so a word after the cut must move earlier by exactly that much, and a word
/// before it must not move at all.
#[test]
fn after_a_cut_every_surviving_word_seeks_to_its_exact_new_instant() {
    let link = linked();
    let edited = link
        .delete(&id("w2"), &id("w3"))
        .expect("a word-timed transcript can be cut exactly");

    let after = Linked::new(
        edited.transcript().clone(),
        edited.map().clone(),
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("the edited pair is still a usable link");

    // Before the cut: unmoved.
    assert_eq!(after.seek_target(&id("w1")).expect("w1 survives"), us(0));
    // After the cut: earlier by exactly the deleted source length.
    assert_eq!(
        after.seek_target(&id("w4")).expect("w4 survives"),
        us(1_000_000),
        "3 500 000 minus the 2 500 000 that was removed"
    );
    assert_eq!(
        after.seek_target(&id("w5")).expect("w5 survives"),
        us(1_500_000),
        "4 000 000 minus the 2 500 000 that was removed"
    );
    // The map agrees, to the microsecond.
    assert_eq!(
        after.map().output_duration(),
        us(7_500_000),
        "10 000 000 minus the 2 500 000 that was removed"
    );
    assert_eq!(
        after
            .map()
            .source_at(us(1_000_000))
            .expect("the cut boundary is still inside the map")
            .at,
        us(3_500_000),
        "composition 1 000 000 is the source that follows the cut"
    );
}

/// A word shown twice in one composition has **no** single seek target, and
/// taking the first would drop the repeat.
#[test]
fn a_word_shown_twice_has_no_single_seek_target() {
    // A composition that uses the first five seconds twice.
    let mut map = IntervalMap::new();
    map.push_source(transcript::source_span(0, 5_000_000))
        .expect("append");
    map.push_source(transcript::source_span(0, 5_000_000))
        .expect("append");
    let link = Linked::new(
        transcript(),
        map,
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a repeated source is a usable link");
    assert!(matches!(
        link.seek_target(&id("w3")),
        Err(TranscriptError::Repeated { word, positions: 2, .. }) if word == "w3"
    ));
    // The reverse direction still answers, with every position.
    let hits = link
        .words_at(us(7_000_000))
        .expect("the second showing of w3");
    assert_eq!(hits, vec![id("w3")], "and it is a word either way");
}

/// The reverse direction names *every* showing of a repeated word.
#[test]
fn the_reverse_direction_returns_every_showing_of_a_repeated_word() {
    let mut map = IntervalMap::new();
    map.push_source(transcript::source_span(0, 5_000_000))
        .expect("append");
    map.push_source(transcript::source_span(0, 5_000_000))
        .expect("append");
    let link = Linked::new(
        transcript(),
        map,
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("a repeated source is a usable link");
    // Source 2 000 000 is composition 2 000 000 and composition 7 000 000.
    let at = link
        .words_at(us(7_000_000))
        .expect("w3 in the second showing");
    assert_eq!(at, vec![id("w3")]);
    let both = link.seek_targets(&id("w3")).expect("every showing of w3");
    assert_eq!(
        both,
        vec![us(2_000_000), us(7_000_000)],
        "a repeated word is at two output positions, and taking the first \
         would drop the repeat"
    );
}

/// A transcript whose timings run past the media is a malformed document, and
/// it is reported at the link rather than discovered by a player.
#[test]
fn a_timing_past_the_end_of_the_media_is_refused() {
    let mut past_end = transcript();
    past_end.segments[0].words[0].at = Some(transcript::span(9_500_000, 11_000_000));
    assert!(matches!(
        Linked::new(
            past_end,
            composition(),
            transcript::source(),
            Some(REVISION.to_string()),
            Some(us(MEDIA_US)),
        ),
        Err(TranscriptError::BeyondMedia {
            at: 11_000_000,
            duration: 10_000_000,
            ..
        })
    ));
}

/// A source of unknown length cannot be checked, so the check is skipped
/// rather than refused — and that is stated, not assumed.
#[test]
fn a_source_of_unknown_length_is_accepted_rather_than_assumed_short() {
    let link = Linked::new(
        transcript(),
        composition(),
        transcript::source(),
        Some(REVISION.to_string()),
        None,
    )
    .expect("an unknown media length is not a malformed transcript");
    assert_eq!(link.media_duration(), None);
    assert_eq!(link.seek_target(&id("w1")).expect("w1 is shown"), us(0));
}

/// A word id is provider-supplied and reaches this code from outside, so it
/// is validated where it is minted — including through deserialisation, which
/// is the path the revision store takes.
#[test]
fn a_word_id_is_validated_where_it_is_minted() {
    assert!("".parse::<strivo_editor::transcript::WordId>().is_err());
    assert!("   ".parse::<strivo_editor::transcript::WordId>().is_err());
    assert!("w\u{7}1"
        .parse::<strivo_editor::transcript::WordId>()
        .is_err());
    assert!("w\n1".parse::<strivo_editor::transcript::WordId>().is_err());
    assert!("x"
        .repeat(257)
        .parse::<strivo_editor::transcript::WordId>()
        .is_err());
    assert!("x"
        .repeat(256)
        .parse::<strivo_editor::transcript::WordId>()
        .is_ok());
    // And through the document, which is the path the store takes.
    let raw = r#"{"segments":[{"at":null,"words":[{"id":"","at":null,"text":"x"}]}]}"#;
    assert!(
        serde_json::from_str::<strivo_editor::transcript::Transcript>(raw).is_err(),
        "an empty id must not survive deserialisation into a document"
    );
}

#[test]
fn the_fixture_source_is_not_empty_before_any_claim_about_it() {
    assert_eq!(
        composition().entries().len(),
        1,
        "one span, not an empty map"
    );
    assert!(SOURCE_ID.len() > 1);
    assert_ne!(
        MediaTime::from_micros(MEDIA_US).expect("positive"),
        MediaTime::ZERO
    );
}
