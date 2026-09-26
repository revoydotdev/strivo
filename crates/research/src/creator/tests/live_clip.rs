//! Live clipping during capture: what the store sees while a capture is
//! still running, and what it refuses to build a clip from.
//!
//! Three claims, in the order the plan states them:
//!
//! 1. **signals are visible during capture, not only after it.** The
//!    session row is created with `finalized = 0`, an extractor signal is
//!    appended, and it is read back *while the row still says the capture is
//!    running*. The assertion is on the session's own flag, because a test
//!    that only checked the event would also pass against a store that
//!    buffered everything and flushed on finalize — which is the opposite of
//!    the promise.
//!
//! 2. **no clip is built from a part still being written.** A request whose
//!    range reaches into an open part is refused, and the refusal is checked
//!    to have written *nothing*: no plan row, no job. A job row for an
//!    impossible render is worse than no row, because it is a durable claim
//!    somebody will retry.
//!
//! 3. **one render path, one lifecycle, one vocabulary.** The submission is
//!    graded on todo 23's plan and todo 24's job, the two constants agree
//!    with the PVR side, and the rejection names are the PVR side's names —
//!    the last two read the PVR source, so a divergence fails here rather
//!    than becoming two answers in production.

use std::path::PathBuf;

use strivo_editor::time::{ClockMap, MediaTime};

use super::super::ids::*;
use super::super::jobs::JobState;
use super::super::render_plan::read_plan;
use super::super::{
    CountedTable, EventKind, LiveCaptureTail, LiveClipRejection, LiveClipRequest, RawStreamEvent,
    Store, TailPart, MIN_CLIP_US,
};
use super::fixtures;

// --- fixtures -------------------------------------------------------------
//
// Local rather than in the shared `fixtures` module: three other lanes edit
// that file, and a fixture this lane needs is not a fixture they do.

/// 0-based wall clock. Stated, never read from the host, so no assertion
/// here can be a wall-clock flake.
const T0: i64 = 1_789_000_000_000;

/// The PVR half, read at compile time. Not a constant copied into this test:
/// the point is that the two crates' numbers cannot drift without a red.
const PVR_LIVE_CLIP: &str = include_str!("../../../../../src/recording/live_clip.rs");

/// Rows in a named Creator table. Used only where the store has no getter,
/// which is stated at each call site rather than hidden in this helper.
fn count(store: &Store, table: &str) -> i64 {
    store
        .raw_connection()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .expect("count rows")
}

fn plan_row(store: &Store, id: RenderPlanId) -> super::super::entities::RenderPlan {
    use super::super::entities::RenderPlan;
    store
        .raw_connection()
        .query_row(
            "SELECT id,project_id,variant_id,renderer,renderer_version,resolved_inputs_json,
                    content_hash,created_at
             FROM creator_render_plans WHERE id=?1",
            [id.to_string()],
            |r| {
                Ok(RenderPlan {
                    id: RenderPlanId::from_uuid(
                        uuid::Uuid::parse_str(&r.get::<_, String>(0)?).expect("a uuid"),
                    ),
                    project_id: ProjectId::from_uuid(
                        uuid::Uuid::parse_str(&r.get::<_, String>(1)?).expect("a uuid"),
                    ),
                    variant_id: VariantId::from_uuid(
                        uuid::Uuid::parse_str(&r.get::<_, String>(2)?).expect("a uuid"),
                    ),
                    renderer: r.get(3)?,
                    renderer_version: r.get(4)?,
                    resolved_inputs_json: r.get(5)?,
                    content_hash: r.get(6)?,
                    created_at: r.get(7)?,
                })
            },
        )
        .expect("a plan row")
}

fn mt(millis: i64) -> MediaTime {
    MediaTime::from_millis(millis).expect("a non-negative media coordinate")
}

fn clock() -> ClockMap {
    let mut clock = ClockMap::new();
    clock
        .push(strivo_editor::time::ClockSegment::exact(
            T0,
            MediaTime::ZERO,
            mt(600_000),
        ))
        .expect("one segment");
    clock
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    project: ProjectId,
    session: CaptureSessionId,
    source: SourceAssetId,
    scratch: PathBuf,
    capture: PathBuf,
    variant: super::super::entities::Variant,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("creator.db")).expect("open store");
        let project = fixtures::project(name);
        let project_id = project.id;
        store.insert_project(&project).expect("insert project");

        let source = fixtures::source(project_id, None);
        let source_id = source.id;
        store.insert_source_asset(&source).expect("insert source");

        // `finalized: false` — the row a capture in progress actually has.
        let session = super::super::entities::CaptureSession {
            id: CaptureSessionId::generate(),
            project_id,
            source_asset_id: Some(source_id),
            started_at: fixtures::at("2026-09-26T00:00:00Z"),
            ended_at: None,
            finalized: false,
            attributes_json: "{}".into(),
            created_at: fixtures::at("2026-09-26T00:00:00Z"),
        };
        let session_id = session.id;
        store
            .insert_capture_session(&session)
            .expect("insert capture session");

        // A variant is a foreign key from `creator_render_plans`, and it in
        // turn names a composition revision, so the whole chain is inserted.
        // A plan that could be stored without one would be a plan whose
        // destination is undefined.
        let revision = fixtures::revision(project_id, 1, None);
        let revision_id = revision.id;
        store
            .insert_composition_revision(&revision)
            .expect("insert composition revision");
        let variant = super::super::entities::Variant {
            id: VariantId::generate(),
            project_id,
            composition_revision_id: revision_id,
            name: "clip".into(),
            format_overrides_json: "{}".into(),
            created_at: fixtures::at("2026-09-26T00:00:05Z"),
        };
        store.insert_variant(&variant).expect("insert variant");

        let scratch = dir.path().join("scratch");
        let capture = dir.path().join("recordings").join("stream.mkv");
        std::fs::create_dir_all(capture.parent().expect("a parent"))
            .expect("create recordings dir");
        Self {
            _dir: dir,
            store,
            project: project_id,
            session: session_id,
            source: source_id,
            scratch,
            capture,
            variant,
        }
    }

    /// A tail with `finalized_ms` of finished capture and one part open.
    fn tail(&self, finalized_ms: i64) -> LiveCaptureTail {
        LiveCaptureTail {
            running: true,
            parts: vec![
                TailPart {
                    index: 1,
                    path: self.capture.clone(),
                    start: MediaTime::ZERO,
                    duration: mt(finalized_ms),
                    finalized: true,
                },
                TailPart {
                    index: 2,
                    path: PathBuf::from(
                        self.capture
                            .display()
                            .to_string()
                            .replace(".mkv", "_part2.mkv"),
                    ),
                    start: mt(finalized_ms),
                    duration: MediaTime::ZERO,
                    finalized: false,
                },
            ],
        }
    }

    fn request(&self, tail: LiveCaptureTail, start_ms: i64, end_ms: i64) -> LiveClipRequest {
        LiveClipRequest {
            project: self.project,
            session: self.session,
            source_asset: self.source,
            variant: self.variant.clone(),
            tail,
            start: mt(start_ms),
            end: mt(end_ms),
            settings: "container=mkv;copy".into(),
            tool_versions: vec![("ffmpeg".into(), "n9.0.2".into())],
            scratch_root: self.scratch.clone(),
            now: fixtures::at("2026-09-26T00:00:05Z"),
        }
    }
}

// --- claim 1: signals are visible during capture -------------------------

#[test]
fn a_signal_is_readable_while_the_capture_is_still_running() {
    let f = Fixture::new("signals-live");

    // The row still says the capture is open. If this is false the whole
    // claim is vacuous, so it is asserted before anything else.
    assert!(
        !f.store
            .capture_session(f.session)
            .expect("read session")
            .expect("a session row")
            .finalized,
        "the fixture must be a capture in progress"
    );

    let window = super::super::CaptureWindow::new(T0, T0 + 600_000).expect("a forward window");
    let written = f
        .store
        .append_stream_events(
            f.session,
            f.project,
            &[RawStreamEvent {
                wall_ms: T0 + 12_000,
                kind: EventKind::Category,
                label: "scene_change".into(),
                category: Some("scene".into()),
                payload_json: "{}".into(),
            }],
            &window,
            &clock(),
        )
        .expect("append a signal");
    assert_eq!(written.len(), 1);

    // Readable now, with the capture still running — not flushed at the end.
    let read = f.store.stream_events(f.session).expect("read signals");
    assert_eq!(read.len(), 1, "the signal must be visible during capture");
    assert_eq!(read[0].label, "scene_change");
    assert!(
        !f.store
            .capture_session(f.session)
            .expect("read session")
            .expect("a session row")
            .finalized,
        "reading a signal must not have finalized the capture"
    );
    assert_eq!(
        f.store
            .row_count(CountedTable::StreamEvents)
            .expect("count"),
        1,
        "the row is durable, not held in a buffer"
    );
}

#[test]
fn signals_accumulate_across_a_running_capture_and_keep_their_order() {
    let f = Fixture::new("signals-many");
    let window = super::super::CaptureWindow::new(T0, T0 + 600_000).expect("a forward window");
    for (n, wall_ms) in [5_000i64, 30_000, 55_000].into_iter().enumerate() {
        f.store
            .append_stream_events(
                f.session,
                f.project,
                &[RawStreamEvent {
                    wall_ms: T0 + wall_ms,
                    kind: EventKind::Category,
                    label: format!("signal-{n}"),
                    category: Some("extractor".into()),
                    payload_json: "{}".into(),
                }],
                &window,
                &clock(),
            )
            .expect("append");
    }
    let read = f.store.stream_events(f.session).expect("read");
    assert_eq!(read.len(), 3, "a probe that cannot fail");
    assert_eq!(
        read.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
        vec!["signal-0", "signal-1", "signal-2"],
        "seq is assigned by the store and must not collide"
    );
}

// --- claim 2: no clip from a part still being written --------------------

#[test]
fn a_clip_naming_a_part_still_being_written_is_refused_and_writes_nothing() {
    let f = Fixture::new("open-part");
    // Part 1 holds 60 s; part 2 is open. A range reaching into part 2 must
    // be refused rather than answered with a silent 10 s prefix.
    let err = f
        .store
        .submit_live_clip(&f.request(f.tail(60_000), 50_000, 200_000))
        .expect_err("an open part is not a legal clip input");
    assert_eq!(
        err.rejection(),
        Some(&LiveClipRejection::PartStillOpen {
            index: 2,
            path: PathBuf::from(
                f.capture
                    .display()
                    .to_string()
                    .replace(".mkv", "_part2.mkv")
            ),
            finalized_end: mt(60_000),
        })
    );
    // Nothing durable was written for a render that cannot happen.
    assert!(
        f.store.jobs(f.project).expect("jobs").is_empty(),
        "a refused submission must leave no job row"
    );
    assert_eq!(
        count(&f.store, "creator_render_plans"),
        0,
        "a refused submission must leave no plan row"
    );
}

#[test]
fn a_clip_from_finalized_media_is_admitted_while_the_capture_runs() {
    let f = Fixture::new("finalized-only");
    let submission = f
        .store
        .submit_live_clip(&f.request(f.tail(60_000), 10_000, 40_000))
        .expect("a range wholly inside finalized media is legal");
    assert_eq!(submission.parts.len(), 1);
    assert_eq!(submission.parts[0].index, 1);
    assert!(submission.parts.iter().all(|p| p.finalized));
    // The capture has not stopped, which is the whole feature.
    assert!(
        !f.store
            .capture_session(f.session)
            .expect("session")
            .expect("row")
            .finalized
    );
}

#[test]
fn a_request_before_anything_is_finalized_names_the_shortfall() {
    let f = Fixture::new("nothing-yet");
    let empty = LiveCaptureTail {
        running: true,
        parts: vec![TailPart {
            index: 1,
            path: f.capture.clone(),
            start: MediaTime::ZERO,
            duration: MediaTime::ZERO,
            finalized: false,
        }],
    };
    assert_eq!(
        f.store
            .submit_live_clip(&f.request(empty, 0, 5_000))
            .expect_err("nothing is finalized")
            .rejection(),
        Some(&LiveClipRejection::NothingFinalized {
            requested_start: MediaTime::ZERO,
            finalized_end: MediaTime::ZERO,
        })
    );
}

#[test]
fn a_stopped_capture_is_not_a_live_clip() {
    let f = Fixture::new("stopped");
    let mut tail = f.tail(60_000);
    tail.running = false;
    assert_eq!(
        f.store
            .submit_live_clip(&f.request(tail, 0, 30_000))
            .expect_err("stopped")
            .rejection(),
        Some(&LiveClipRejection::CaptureStopped)
    );
}

// --- claim 3: one plan, one lifecycle, one vocabulary ---------------------

#[test]
fn a_live_clip_runs_through_the_existing_plan_and_job_lifecycle() {
    let f = Fixture::new("lifecycle");
    let submission = f
        .store
        .submit_live_clip(&f.request(f.tail(60_000), 10_000, 40_000))
        .expect("submit");

    // Todo 24's state, not a new one: accepted and not started.
    assert_eq!(submission.job.state, JobState::Pending);
    assert_eq!(submission.job.subject_kind, "live_clip");
    assert_eq!(submission.job.attempt, 0);
    assert_eq!(submission.job.max_attempts, 3);

    // Todo 23's row, readable through todo 23's reader. The row is fetched
    // with SQL because `Store` has no `render_plan` getter — a gap in the
    // store, reported rather than papered over by adding one here.
    assert_eq!(count(&f.store, "creator_render_plans"), 1);
    let plan = read_plan(&plan_row(&f.store, submission.plan_id)).expect("the stored plan parses");
    assert_eq!(plan.clips.len(), 1, "one finalized part, one clip");
    assert_eq!(plan.output_duration.micros(), 30_000_000);

    // The log is complete from seq 0, so "what did this job do" needs no
    // second source.
    let log = f.store.transitions(submission.job_id()).expect("log");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].reason, "submit");
    assert_eq!(log[0].from, None);

    // Finish it the ordinary way: bytes on disk, artifact row, validation.
    std::fs::create_dir_all(submission.output.parent().expect("a parent"))
        .expect("create output dir");
    std::fs::write(&submission.output, b"a clip's bytes").expect("write the clip");
    let outcome = f.store.complete_live_clip(
        &submission,
        &submission.output,
        Some(mt(30_000)),
        "2026-09-26T00:00:09Z",
    );

    // The artifact row IS written, and it names the plan, the variant and the
    // bytes — everything but the last step.
    let artifact_id: ArtifactId = f
        .store
        .raw_connection()
        .query_row(
            "SELECT id FROM creator_artifacts WHERE render_plan_id=?1",
            [submission.plan_id.to_string()],
            |r| {
                Ok(ArtifactId::from_uuid(
                    uuid::Uuid::parse_str(&r.get::<_, String>(0)?).expect("a uuid"),
                ))
            },
        )
        .expect("an artifact row for the plan");
    let row = f
        .store
        .artifact(artifact_id)
        .expect("read artifact")
        .expect("an artifact row");
    assert_eq!(row.kind, "clip");
    assert_eq!(row.render_plan_id, Some(submission.plan_id));
    assert_eq!(row.variant_id, Some(submission.variant_id));
    assert_eq!(row.duration, Some(mt(30_000)));
    assert_eq!(row.bytes, Some(14));

    // **Reported, not fixed: todo 24's own two hashers disagree.**
    // `import::fingerprint` (the crate's hasher, which every caller is meant
    // to use) returns `sha256:<hex>`, and `store_jobs::check_output`
    // compares the row against `hash_file`'s bare `<hex>`. So a row written
    // with the crate's hasher can never validate, and `complete` — which is
    // the only road to `done`, a state nothing can leave — is unreachable
    // for every caller that hashes honestly. `import.rs:395` dodges it by
    // storing a `CacheKey` instead of a file digest.
    //
    // This test pins the CURRENT behaviour rather than the intended one, so
    // the day todo 24's owner fixes the comparison this goes red and says so.
    let refusal = outcome.expect_err("check_output compares two hash spellings");
    let text = refusal.to_string();
    assert!(
        text.contains("the artifact claims sha256:"),
        "the refusal must name the hash mismatch, got: {text}"
    );
    assert!(
        !text.contains("is empty") && !text.contains("the attempt recorded"),
        "the refusal must be the hash comparison and not a size or emptiness check: {text}"
    );
    assert_eq!(
        f.store
            .job(submission.job_id())
            .expect("read job")
            .expect("a job row")
            .state,
        JobState::Pending,
        "a job whose output did not validate must not be final"
    );
    // A zero-byte output is refused for the honest reason, which is the
    // check that does work.
    assert!(
        f.store
            .job(submission.job_id())
            .expect("read job")
            .expect("a job row")
            .artifact_id
            .is_none(),
        "an unvalidated job must not be pointed at an artifact"
    );
}

#[test]
fn a_clip_of_zero_bytes_is_refused_rather_than_completed() {
    let f = Fixture::new("zero-bytes");
    let submission = f
        .store
        .submit_live_clip(&f.request(f.tail(60_000), 10_000, 40_000))
        .expect("submit");
    std::fs::create_dir_all(submission.output.parent().expect("a parent"))
        .expect("create output dir");
    std::fs::write(&submission.output, b"").expect("write nothing");
    assert!(
        f.store
            .complete_live_clip(
                &submission,
                &submission.output,
                None,
                "2026-09-26T00:00:09Z"
            )
            .is_err(),
        "a zero-byte clip must not be completed"
    );
    assert_eq!(
        f.store
            .job(submission.job_id())
            .expect("read job")
            .expect("a job row")
            .state,
        JobState::Pending,
        "the job must not have reached a final state"
    );
}

#[test]
fn the_creator_ceiling_agrees_with_the_pvr_one() {
    // Both are restated rather than shared — the crates cannot see each
    // other — so the restatement is pinned here.
    assert_eq!(MIN_CLIP_US, 1_000_000, "one second, in microseconds");
    assert!(
        PVR_LIVE_CLIP.contains("pub const MIN_CLIP_MS: u64 = 1_000;"),
        "the PVR half no longer states MIN_CLIP_MS = 1_000, so the two ceilings can drift"
    );
}

#[test]
fn the_rejection_vocabulary_mirrors_the_pvr_one() {
    for variant in [
        "CaptureStopped",
        "InvertedRange",
        "ClipTooShort",
        "NothingFinalized",
        "PartStillOpen",
    ] {
        assert!(
            PVR_LIVE_CLIP.contains(&format!("LiveClipError::{variant}")),
            "the PVR half no longer refuses with {variant}, so the two vocabularies have diverged"
        );
    }
}

#[test]
fn the_pvr_half_still_refuses_to_read_an_open_segment() {
    // The Creator half validates a *copy* of the tail, so the claim that no
    // clip is ever cut from a file being written has to be checked on the
    // half that owns the file. Read at compile time; the assertions below
    // are the two branches that make it true.
    assert!(
        PVR_LIVE_CLIP.contains("PartState::Finalized")
            && PVR_LIVE_CLIP.contains("pub fn finalize_part"),
        "the PVR tail no longer distinguishes a finalized part from an open one"
    );
    assert!(
        PVR_LIVE_CLIP.contains("a_clip_naming_an_open_part_is_refused_not_truncated"),
        "the PVR half lost the test that proves an open part is refused"
    );
}
