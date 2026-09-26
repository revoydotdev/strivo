//! Publication eligibility, derived from recorded ownership/licence/consent
//! notes — the positive control first, then the gate.
//!
//! ## Why the positive control is the *first* test in this file
//!
//! A gate that refuses everything satisfies "an unlicensed source is
//! ineligible to publish" for free, and that is not a hypothetical: it is
//! exactly what happened to todo 27, where six refusal tests passed on first
//! run against a stub that refused *everything*. So `a_licensed_and_consented_
//! source_is_publishable` is asserted before any refusal, and it is the
//! stronger claim — it fails for a model that has drifted to refuse-all just
//! as loudly as for one that has failed to gate at all.
//!
//! ## Eligibility is derived, never stored
//!
//! There is no `publishable` column, and [`SourceNote::eligibility`] is the
//! only producer of an [`Eligibility`]. `eligibility_is_derived_and_never_
//! stored` reads the table's own column list rather than trusting that claim,
//! and `writing_the_note_is_the_only_way_to_change_eligibility` shows the
//! verdict moving when — and only when — the note moves.
//!
//! ## The note hangs off the identity, not the path
//!
//! `moving_the_file_preserves_the_note`,
//! `a_different_file_at_the_old_path_does_not_inherit_the_note` and
//! `a_reimport_of_the_same_bytes_keeps_the_note` are todo 33's rule applied to
//! licence data: the note is keyed by [`SourceAssetId`], so a move cannot
//! detach it and a new file at the old path cannot inherit it.

use std::path::PathBuf;

use super::super::entities::SourceAsset;
use super::super::ids::SourceAssetId;
use super::super::schema::TABLES;
use super::super::store::Store;
use super::super::{
    AdmittedFile, Axis, CalendarDate, Consent, Eligibility, ImportRequest, Licence, Ownership,
    SourceNote,
};
use super::fixtures;

// ---------------------------------------------------------------- fixtures

fn day(text: &str) -> CalendarDate {
    CalendarDate::parse(text).expect("a calendar date")
}

fn on(day_text: &str) -> CalendarDate {
    day(day_text)
}

/// A note a human could reasonably have written: the creator made it, holds
/// the licence, and nobody else's material is in it.
fn publishable(store: &Store, source: SourceAssetId) -> SourceNote {
    let note = SourceNote {
        source,
        ownership: Ownership::CreatorOwned,
        licence: Licence::Granted {
            grant: "my own capture".into(),
        },
        consent: Consent::NotApplicable,
        disclosure: String::new(),
        recorded_at: "2026-09-07T00:00:00Z".into(),
        recorded_by: "operator".into(),
    }
    .validated()
    .expect("a complete note validates");
    store.record_note(&note).expect("note recorded");
    note
}

fn store_with_source() -> (tempfile::TempDir, Store, SourceAssetId) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("open");
    let project = fixtures::project("streamer");
    store.insert_project(&project).expect("project inserted");
    let mut source = fixtures::source(project.id, None);
    source.media_fingerprint = "sha256:rights-fixture".into();
    store.insert_source_asset(&source).expect("source inserted");
    (dir, store, source.id)
}

// ------------------------------------------------- the positive control

/// Given a source the creator owns, holds a licence grant for, and which
/// contains nobody else's material
/// When eligibility is derived at a date inside the consent window
/// Then it is publishable — the converse of the gate, and the test that makes
/// every refusal below mean something.
#[test]
fn a_licensed_and_consented_source_is_publishable() {
    // Given a note recording ownership, a licence grant and no consent need
    let (_dir, store, source) = store_with_source();
    let note = publishable(&store, source);

    // When the note is read back and eligibility is derived
    let read = store.source_note(source).expect("read").expect("a note");
    assert_eq!(read, note, "the note round-trips through the store");
    let verdict = read.eligibility(&on("2026-09-07"));

    // Then it is publishable, with no blocker and nothing undecided
    assert_eq!(verdict, Eligibility::Publishable, "{verdict}");
    assert!(verdict.is_publishable(), "{verdict}");
    assert!(verdict.require_publishable().is_ok(), "{verdict}");
    assert!(
        verdict.blockers().is_empty() && verdict.undecided().is_empty(),
        "a publishable verdict names no blocker: {verdict}"
    );
}

/// The subject of the todo, and the test that would pass for free against a
/// refuse-everything stub if the positive control above did not exist.
#[test]
fn an_unlicensed_source_is_ineligible_to_publish() {
    // Given a source with no licence grant recorded
    let (_dir, store, source) = store_with_source();
    store
        .record_note(
            &SourceNote {
                source,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Absent,
                consent: Consent::NotApplicable,
                disclosure: String::new(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note recorded");

    // When eligibility is derived
    let verdict = store
        .source_note(source)
        .expect("read")
        .expect("a note")
        .eligibility(&on("2026-09-07"));

    // Then it is refused, naming the licence axis
    assert!(!verdict.is_publishable(), "{verdict}");
    let refusal = verdict
        .require_publishable()
        .expect_err("an unlicensed source must not publish");
    assert_eq!(refusal.blockers[0].axis, Axis::Licence, "{refusal}");
    assert!(
        refusal.blockers[0].reason.contains("licence"),
        "the reason names what is wrong: {refusal}"
    );
}

/// The plan's own case: a chat-overlay clip whose participants never gave
/// consent. Consent is a separate axis from the licence — this source is
/// perfectly licensed and still not publishable.
#[test]
fn a_licensed_source_with_no_consent_record_is_ineligible() {
    // Given a licensed source whose consent nobody has determined
    let (_dir, store, source) = store_with_source();
    store
        .record_note(
            &SourceNote {
                source,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Granted {
                    grant: "Twitch ToS, my own capture".into(),
                },
                consent: Consent::Undetermined,
                disclosure: String::new(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note recorded");

    // When eligibility is derived
    let verdict = store
        .source_note(source)
        .expect("read")
        .expect("a note")
        .eligibility(&on("2026-09-07"));

    // Then it is undetermined on the consent axis alone, and not publishable
    assert!(!verdict.is_publishable(), "{verdict}");
    assert_eq!(verdict.undecided(), &[Axis::Consent], "{verdict}");
    assert!(
        verdict.blockers().is_empty(),
        "nobody refused it, so nothing is blocked — it is simply undecided: {verdict}"
    );
}

/// The other direction of "separate axes": consent on file, licence unknown.
/// A participant's permission is not a grant of the copyright.
#[test]
fn a_consented_source_with_an_unknown_licence_is_ineligible() {
    // Given consent granted but the licence undetermined
    let (_dir, store, source) = store_with_source();
    store
        .record_note(
            &SourceNote {
                source,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Undetermined,
                consent: Consent::Granted {
                    scope: "chat overlay, all participants".into(),
                    valid_until: None,
                },
                disclosure: String::new(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note recorded");

    // When eligibility is derived
    let verdict = store
        .source_note(source)
        .expect("read")
        .expect("a note")
        .eligibility(&on("2026-09-07"));

    // Then only the licence axis is undecided
    assert_eq!(verdict.undecided(), &[Axis::Licence], "{verdict}");
    assert!(!verdict.is_publishable(), "{verdict}");
}

/// The coexistence claim: a note that says "licensed" alongside a verdict that
/// says "not eligible". If these were the same fact they could not disagree.
#[test]
fn a_note_that_says_licensed_coexists_with_a_verdict_that_says_not_eligible() {
    // Given a source with a licence grant AND a refused consent
    let (_dir, store, source) = store_with_source();
    store
        .record_note(
            &SourceNote {
                source,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Granted {
                    grant: "CC-BY-4.0, publisher's own recording".into(),
                },
                consent: Consent::Refused {
                    reason: "one participant declined chat overlay".into(),
                },
                disclosure: "one chat participant declined".into(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note recorded");

    // When the note is read
    let note = store.source_note(source).expect("read").expect("a note");

    // Then the note says the licence is granted
    assert!(
        matches!(&note.licence, Licence::Granted { .. }),
        "the recorded licence is a grant"
    );
    // And the derived verdict still refuses, on consent
    let verdict = note.eligibility(&on("2026-09-07"));
    assert!(!verdict.is_publishable(), "{verdict}");
    assert_eq!(verdict.blockers()[0].axis, Axis::Consent, "{verdict}");
    assert!(
        verdict.blockers()[0].reason.contains("declined"),
        "the refusal carries the human's own reason: {verdict}"
    );
}

/// Personal-archive material: the plan's "an asset is either archive-only or
/// publishable". Ownership is a third axis, not a flavour of the licence.
#[test]
fn archive_only_material_is_never_publishable_however_well_licensed() {
    // Given a perfectly licensed, consented note whose ownership is archive-only
    let (_dir, store, source) = store_with_source();
    store
        .record_note(
            &SourceNote {
                source,
                ownership: Ownership::ArchiveOnly,
                licence: Licence::Granted {
                    grant: "somebody let me keep it".into(),
                },
                consent: Consent::NotApplicable,
                disclosure: String::new(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note recorded");

    // When eligibility is derived
    let verdict = store
        .source_note(source)
        .expect("read")
        .expect("a note")
        .eligibility(&on("2026-09-07"));

    // Then the ownership axis blocks it and nothing else is at issue
    assert_eq!(verdict.blockers()[0].axis, Axis::Ownership, "{verdict}");
    assert!(verdict.undecided().is_empty(), "{verdict}");
    assert!(!verdict.is_publishable(), "{verdict}");
}

/// The fail-open direction, stated as a test: a brand-new source nobody has
/// written a note about is **not** publishable. This is the default that
/// matters, and it is the one todo 27's design note warns about.
#[test]
fn a_source_with_no_note_at_all_is_not_publishable() {
    // Given a source that exists and has never been annotated
    let (_dir, store, source) = store_with_source();

    // When its note is read
    let read = store.source_note(source).expect("read");

    // Then there is no note, and the missing note is itself the verdict
    assert_eq!(read, None, "nothing has been recorded about it");
    let verdict = super::super::rights::eligibility_of(read.as_ref(), &on("2026-09-07"));
    assert!(!verdict.is_publishable(), "{verdict}");
    assert_eq!(
        verdict.undecided(),
        &[Axis::Ownership, Axis::Licence, Axis::Consent],
        "an unannotated source is undecided on every axis, not publishable: {verdict}"
    );
}

// ---------------------------------------------- derived, never a flag

/// Read the table's own columns rather than trusting the claim: if a
/// `publishable` column existed, a caller could write it and the gate would be
/// a suggestion.
#[test]
fn eligibility_is_derived_and_never_stored() {
    // Given a Creator store
    let (_dir, store, _source) = store_with_source();

    // Then the notes table has no column any caller could set eligibility with
    let mut stmt = store
        .raw_connection()
        .prepare("SELECT name FROM pragma_table_info('creator_source_notes')")
        .expect("prepare");
    let columns: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<std::result::Result<_, _>>()
        .expect("rows");
    assert!(!columns.is_empty(), "the notes table really exists");
    for banned in [
        "publishable",
        "eligible",
        "eligibility",
        "consent_ok",
        "cleared",
    ] {
        assert!(
            !columns.iter().any(|c| c == banned),
            "a `{banned}` column would be a stored eligibility flag: {columns:?}"
        );
    }
    // And the table is the one this todo added, not an unrelated one
    assert!(
        TABLES.contains(&"creator_source_notes"),
        "the notes table is declared in TABLES"
    );
}

/// The gate is closed, then the only thing that opens it is a note — and a
/// second note is what closes it again.
#[test]
fn writing_the_note_is_the_only_way_to_change_eligibility() {
    // Given a source whose note says it may be published
    let (_dir, store, source) = store_with_source();
    let note = publishable(&store, source);
    assert!(note.eligibility(&on("2026-09-07")).is_publishable());

    // When the note is replaced with one recording no licence
    store
        .record_note(
            &SourceNote {
                source,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Absent,
                consent: Consent::NotApplicable,
                disclosure: String::new(),
                recorded_at: "2026-09-08T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note replaced");

    // Then eligibility follows the note, and there is exactly one note row
    let verdict = store
        .source_note(source)
        .expect("read")
        .expect("a note")
        .eligibility(&on("2026-09-07"));
    assert!(!verdict.is_publishable(), "{verdict}");
    let count: i64 = store
        .raw_connection()
        .query_row(
            "SELECT COUNT(*) FROM creator_source_notes WHERE source_asset_id=?1",
            [source.to_string()],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(
        count, 1,
        "a second decision replaces the first, it does not accumulate"
    );
}

// ------------------------------------------------ the identity rule

/// Todo 33's rule applied to licence data: a move rewrites the locator and
/// nothing else, so the note cannot fall off.
#[test]
fn moving_the_file_preserves_the_note() {
    // Given a noted source whose bytes are at one path
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("clip.mkv");
    std::fs::write(&first, b"bytes").expect("write");
    let store = Store::open(dir.path().join("research.db")).expect("open");
    let project = fixtures::project("streamer");
    store.insert_project(&project).expect("project");
    let mut asset = fixtures::source(project.id, Some(first.clone()));
    asset.media_fingerprint = "sha256:movable".into();
    store.insert_source_asset(&asset).expect("source");
    let before = publishable(&store, asset.id);

    // When the file moves and the locator is rewritten
    let second = dir.path().join("archive/2026/clip.mkv");
    std::fs::create_dir_all(second.parent().expect("parent")).expect("mkdir");
    std::fs::rename(&first, &second).expect("rename");
    store
        .set_source_locator(asset.id, Some(second.as_path()))
        .expect("locator rewritten");

    // Then the note is unchanged and the asset is still resolvable
    let after = store
        .source_note(asset.id)
        .expect("read")
        .expect("the note survived the move");
    assert_eq!(after, before, "a move must not touch a note");
    assert_eq!(after.fingerprint(), before.fingerprint());
    let reread = store.source_asset(asset.id).expect("read").expect("asset");
    assert_eq!(
        reread.availability(),
        super::super::Availability::Resolved(second.clone()),
        "and the asset still resolves at its new locator"
    );
    assert!(after.eligibility(&on("2026-09-07")).is_publishable());
}

/// A different file at the old path must not inherit the note that hung off
/// the old occupant's identity. This is the defect class todo 33 exists to
/// prevent, and keying a note on a path would reintroduce it for licence data.
#[test]
fn a_different_file_at_the_old_path_does_not_inherit_the_note() {
    // Given a noted source, and then a *different* file imported over its path
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clip.mkv");
    std::fs::write(&path, b"first bytes").expect("write");
    let store = Store::open(dir.path().join("research.db")).expect("open");
    let project = fixtures::project("streamer");
    store.insert_project(&project).expect("project");
    let mut first = fixtures::source(project.id, Some(path.clone()));
    first.media_fingerprint = "sha256:first".into();
    store.insert_source_asset(&first).expect("first source");
    publishable(&store, first.id);

    std::fs::write(&path, b"entirely different bytes").expect("overwrite");
    let mut second = fixtures::source(project.id, Some(path.clone()));
    second.media_fingerprint = "sha256:second".into();
    store.insert_source_asset(&second).expect("second source");

    // Then the new source has no note, and the old one kept its own
    assert_ne!(first.id, second.id, "two identities");
    assert_eq!(
        store.source_note(second.id).expect("read"),
        None,
        "a note must not re-attach to different bytes at the same path"
    );
    assert_eq!(
        store.source_note(first.id).expect("read").expect("note"),
        publishable(&store, first.id),
        "and the original note is untouched"
    );
}

/// Content identity, not path identity: the same bytes arriving again is the
/// same asset (todo 33's dedup), so the note follows them. The fingerprint is
/// read off the file with the real digest function, not written by hand — a
/// hand-written one would not match what the import computes, and the test
/// would then be proving a mismatch rather than a dedup.
#[test]
fn a_reimport_of_the_same_bytes_keeps_the_note() {
    // Given a project with one noted source
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("clip.mkv");
    std::fs::write(&path, b"the same bytes").expect("write");
    let store = Store::open(dir.path().join("research.db")).expect("open");
    let project = fixtures::project("streamer");
    store.insert_project(&project).expect("project");
    let before = publishable(
        &store,
        first_asset(
            &store,
            project.id,
            &path,
            &super::super::fingerprint(&path).expect("digest"),
        ),
    );

    // When the same content is imported again from a different path
    let moved = dir.path().join("elsewhere/clip.mkv");
    std::fs::create_dir_all(moved.parent().expect("parent")).expect("mkdir");
    std::fs::copy(&path, &moved).expect("copy");
    let files = [AdmittedFile::admitted(&moved)];
    let manifest = super::super::import_files(
        &store,
        &ImportRequest {
            project: project.id,
            files: &files,
            scratch_root: &dir.path().join("scratch"),
            at: "2026-09-08T00:00:00Z",
        },
    )
    .expect("import runs");

    // Then it deduplicates onto the same identity and keeps the note
    assert_eq!(
        manifest.duplicates(),
        1,
        "the same bytes are recognised as a duplicate"
    );
    let duplicated = manifest.entries[0].outcome.outcome_asset().id;
    assert_eq!(duplicated, before.source, "same identity, not a second row");
    assert_eq!(
        store.source_note(duplicated).expect("read").expect("note"),
        before,
        "and the note is still attached to it"
    );
}

fn first_asset(
    store: &Store,
    project: super::super::ids::ProjectId,
    path: &std::path::Path,
    fingerprint: &str,
) -> SourceAssetId {
    let mut asset: SourceAsset = fixtures::source(project, Some(PathBuf::from(path)));
    asset.media_fingerprint = fingerprint.into();
    store.insert_source_asset(&asset).expect("source inserted");
    asset.id
}

// -------------------------------------------------- time, staleness

/// A consent with an expiry is evaluated at a date the caller supplies, never
/// at wall-clock now — so this test is deterministic and the "lapsed" state is
/// a real derived state rather than a race.
#[test]
fn a_consent_that_lapses_becomes_undetermined_again() {
    // Given a consent valid until 2026-10-01
    let (_dir, store, source) = store_with_source();
    let note = SourceNote {
        source,
        ownership: Ownership::CreatorOwned,
        licence: Licence::Granted {
            grant: "my own capture".into(),
        },
        consent: Consent::Granted {
            scope: "chat overlay".into(),
            valid_until: Some(on("2026-10-01")),
        },
        disclosure: String::new(),
        recorded_at: "2026-09-07T00:00:00Z".into(),
        recorded_by: "operator".into(),
    }
    .validated()
    .expect("validates");
    store.record_note(&note).expect("note recorded");

    // When it is read back and derived at three dates
    let read = store.source_note(source).expect("read").expect("a note");
    let before = read.eligibility(&on("2026-09-30"));
    let on_the_day = read.eligibility(&on("2026-10-01"));
    let after = read.eligibility(&on("2026-10-02"));

    // Then it is publishable up to and including the stated day, and
    // undetermined afterwards — a lapsed grant is nobody having decided *now*,
    // not a refusal
    assert!(before.is_publishable(), "{before}");
    assert!(on_the_day.is_publishable(), "{on_the_day}");
    assert!(!after.is_publishable(), "{after}");
    assert_eq!(after.undecided(), &[Axis::Consent], "{after}");
    assert!(
        after.blockers().is_empty(),
        "a lapsed consent was never refused: {after}"
    );
}

/// A note edited after something was reviewed against it is detected, on the
/// same content-fingerprint rule as the workspace and the render plan.
#[test]
fn a_note_edited_after_it_was_reviewed_is_detected_as_stale() {
    // Given a note whose fingerprint a caller recorded at review time
    let (_dir, store, source) = store_with_source();
    let note = publishable(&store, source);
    let recorded = note.fingerprint();
    assert!(!super::super::rights::is_stale(&recorded, &note), "fresh");

    // When the note is edited
    store
        .record_note(
            &SourceNote {
                source,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Absent,
                consent: Consent::NotApplicable,
                disclosure: String::new(),
                recorded_at: "2026-09-09T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note replaced");

    // Then the recorded fingerprint is stale against what is stored now
    let now = store.source_note(source).expect("read").expect("a note");
    assert!(
        super::super::rights::is_stale(&recorded, &now),
        "an edited note must be reported stale, not trusted"
    );
    assert!(!super::super::rights::is_stale(&now.fingerprint(), &now));
}

// ------------------------------------------------------ the gate

/// A release over several sources is refused naming *each* blocking source —
/// the acceptance criterion's "blocked with the reason shown".
#[test]
fn a_release_over_several_sources_names_every_blocking_source() {
    // Given two sources, one publishable and one licensed-but-unconsented
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("open");
    let project = fixtures::project("streamer");
    store.insert_project(&project).expect("project");
    let good = first_asset(&store, project.id, &dir.path().join("a.mkv"), "sha256:a");
    let bad = first_asset(&store, project.id, &dir.path().join("b.mkv"), "sha256:b");
    publishable(&store, good);
    store
        .record_note(
            &SourceNote {
                source: bad,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Granted {
                    grant: "my own capture".into(),
                },
                consent: Consent::Undetermined,
                disclosure: String::new(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note recorded");

    // When the release is checked
    let report = store
        .publication_report(&[good, bad], &on("2026-09-07"))
        .expect("report");

    // Then it is not publishable, and the good source is still reported good
    assert!(!report.is_publishable(), "{report}");
    let bad_verdict = report
        .verdict_for(bad)
        .expect("the blocking source is named");
    assert!(!bad_verdict.is_publishable(), "{bad_verdict}");
    let good_verdict = report.verdict_for(good).expect("every source is reported");
    assert!(good_verdict.is_publishable(), "{good_verdict}");
    assert!(
        report.blocking_sources().contains(&bad) && !report.blocking_sources().contains(&good),
        "one publishable source does not carry an unlicensed one: {report}"
    );
}

/// The same check with only good sources is a clean pass — the gate is not a
/// ritual that always says no.
#[test]
fn a_release_over_publishable_sources_passes() {
    // Given two publishable sources
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("open");
    let project = fixtures::project("streamer");
    store.insert_project(&project).expect("project");
    let a = first_asset(&store, project.id, &dir.path().join("a.mkv"), "sha256:a");
    let b = first_asset(&store, project.id, &dir.path().join("b.mkv"), "sha256:b");
    publishable(&store, a);
    publishable(&store, b);

    // When the release is checked
    let report = store
        .publication_report(&[a, b], &on("2026-09-07"))
        .expect("report");

    // Then it passes with nothing to say
    assert!(report.is_publishable(), "{report}");
    assert!(report.blocking_sources().is_empty(), "{report}");
}

/// A note naming a source that does not exist is refused by the foreign key,
/// so a note cannot be filed against an identity nothing else can reach.
#[test]
fn a_note_naming_a_source_that_does_not_exist_is_refused() {
    // Given a store and a source id nothing was ever inserted under
    let (_dir, store, _source) = store_with_source();
    let phantom = SourceAssetId::generate();

    // When a note is filed against it
    let attempt = store.record_note(
        &SourceNote {
            source: phantom,
            ownership: Ownership::CreatorOwned,
            licence: Licence::Granted { grant: "x".into() },
            consent: Consent::NotApplicable,
            disclosure: String::new(),
            recorded_at: "2026-09-07T00:00:00Z".into(),
            recorded_by: "operator".into(),
        }
        .validated()
        .expect("validates"),
    );

    // Then it is refused by the database
    assert!(attempt.is_err(), "the foreign key must refuse it");
}

/// Deleting a source takes its note with it, so a note cannot outlive its
/// identity and later re-attach to whatever lands on that id.
#[test]
fn deleting_a_source_takes_its_note_with_it() {
    // Given a noted source
    let (_dir, store, source) = store_with_source();
    publishable(&store, source);

    // When the source row is deleted
    store
        .raw_execute(
            "DELETE FROM creator_source_assets WHERE id=?1",
            &[source.to_string()],
        )
        .expect("delete");

    // Then the note is gone with it
    let orphans: i64 = store
        .raw_connection()
        .query_row("SELECT COUNT(*) FROM creator_source_notes", [], |r| {
            r.get(0)
        })
        .expect("count");
    assert_eq!(orphans, 0, "ON DELETE CASCADE must not leave a note behind");
}

// ------------------------------- the store validates, not just the caller

/// `record_note` refuses an unvalidated note. Asserted against the **store**,
/// not against `SourceNote::validated`, because a caller-side check the store
/// also performs is the only kind that survives a caller who forgets.
#[test]
fn the_store_refuses_a_note_the_caller_never_validated() {
    // Given a source that exists
    let (_dir, store, source) = store_with_source();

    // When a note with an empty licence grant is handed to the store directly,
    // bypassing `SourceNote::validated`
    let attempt = store.record_note(&SourceNote {
        source,
        ownership: Ownership::CreatorOwned,
        licence: Licence::Granted {
            grant: String::new(),
        },
        consent: Consent::NotApplicable,
        disclosure: String::new(),
        recorded_at: "2026-09-07T00:00:00Z".into(),
        recorded_by: "operator".into(),
    });

    // Then it is refused with the typed reason, and nothing was written
    assert!(
        matches!(attempt, Err(super::super::RightsError::EmptyLicenceGrant)),
        "the store must refuse what the caller never validated: {attempt:?}"
    );
    assert_eq!(store.source_note(source).expect("read"), None);
}

// ------------------------------- the fingerprint covers the decision

/// The fingerprint is over the *decision*, so an edit that changes only the
/// grant text is detected. Asserted through the store's read path, because a
/// fingerprint that covered nothing would be indistinguishable from one that
/// covered the grant until a real edit arrived.
#[test]
fn a_change_to_the_grant_text_alone_is_detected_as_stale() {
    // Given a note whose fingerprint a caller recorded
    let (_dir, store, source) = store_with_source();
    let before = publishable(&store, source);
    let recorded = before.fingerprint();

    // When only the wording of the grant changes
    store
        .record_note(
            &SourceNote {
                licence: Licence::Granted {
                    grant: "my own capture (restated)".into(),
                },
                ..before.clone()
            }
            .validated()
            .expect("validates"),
        )
        .expect("note replaced");

    // Then the recorded fingerprint is stale
    let now = store.source_note(source).expect("read").expect("a note");
    assert_ne!(
        now.fingerprint(),
        recorded,
        "the grant text is part of the decision"
    );
    assert!(
        super::super::rights_is_stale(&recorded, &now),
        "a restated grant is a changed decision"
    );
}

// ------------------------------- the schema, not the Rust, enforces

/// These four are written through `raw_execute` on purpose: they assert the
/// **database** refuses a decision with nothing attached, not that the Rust
/// constructor does. A test that only exercised `SourceNote::validated` would
/// still pass if the `CHECK`s were deleted, and a row written by a tool this
/// crate does not own would then carry a decision nobody made.
///
/// Each mutation removing the matching `CHECK` in `schema_v6.sql` turns this
/// one red, and no other test notices.
#[test]
fn the_schema_itself_refuses_a_decision_with_nothing_attached() {
    // Given a source and a store whose Rust validation is bypassed entirely
    let (_dir, store, source) = store_with_source();
    let id = source.to_string();
    let fingerprint = "0".repeat(64);
    let cases: [(&str, Vec<String>); 4] = [
        (
            "INSERT INTO creator_source_notes (source_asset_id,ownership,holder,licence,\
             licence_grant,consent,consent_scope,disclosure,fingerprint,recorded_by,recorded_at) \
             VALUES(?1,'creator_owned','','granted','','not_required','','',?2,'op','2026-09-07')",
            vec![id.clone(), fingerprint.clone()],
        ),
        (
            "INSERT INTO creator_source_notes (source_asset_id,ownership,holder,licence,\
             licence_grant,consent,consent_scope,disclosure,fingerprint,recorded_by,recorded_at) \
             VALUES(?1,'creator_owned','','absent','','granted','','',?2,'op','2026-09-07')",
            vec![id.clone(), fingerprint.clone()],
        ),
        (
            "INSERT INTO creator_source_notes (source_asset_id,ownership,holder,licence,\
             licence_grant,consent,consent_scope,disclosure,fingerprint,recorded_by,recorded_at) \
             VALUES(?1,'creator_owned','','absent','','refused','','',?2,'op','2026-09-07')",
            vec![id.clone(), fingerprint.clone()],
        ),
        (
            "INSERT INTO creator_source_notes (source_asset_id,ownership,holder,licence,\
             licence_grant,consent,consent_scope,disclosure,fingerprint,recorded_by,recorded_at) \
             VALUES(?1,'third_party','','absent','','not_required','','',?2,'op','2026-09-07')",
            vec![id, fingerprint],
        ),
    ];

    // Then every one is refused by the database
    for (sql, args) in cases {
        assert!(
            store.raw_execute(sql, &args).is_err(),
            "the database must refuse a decision with nothing attached: {sql}"
        );
    }
    let written: i64 = store
        .raw_connection()
        .query_row("SELECT COUNT(*) FROM creator_source_notes", [], |r| {
            r.get(0)
        })
        .expect("count");
    assert_eq!(written, 0, "and none of them was written");
}
