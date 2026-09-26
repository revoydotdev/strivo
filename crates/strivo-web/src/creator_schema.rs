//! The Creator-edition schema bootstrap.
//!
//! This module is `#[cfg(feature = "creator")]`-gated in `lib.rs`, so a PVR
//! build does not compile it, does not link `strivo-research`, and has no
//! statement from `strivo_research::creator::schema` anywhere in the binary.
//! That is the boundary F04 asks for: a PVR install never creates these
//! tables, because a PVR install has no code that could.
//!
//! Declared here rather than in `strivo-research` because the feature that
//! gates it has to be the one that actually removes code from the PVR binary,
//! and `strivo-research` has no such feature — it is an optional dependency
//! of this crate that only `creator` activates.

use std::path::Path;

/// The database the Creator schema shares with the research kernel:
/// `{data_dir}/research.db`, beside `jobs.db`.
pub fn creator_db_path() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir().join("research.db")
}

/// Apply any pending Creator migrations. Called once from `serve`, before
/// the first request. Idempotent, and a failure is logged rather than fatal:
/// a broken Creator schema must not stop the PVR-grade parts of the web UI
/// from serving.
pub fn ensure_creator_schema() {
    if let Err(e) = strivo_research::creator::ensure_schema(&creator_db_path()) {
        tracing::warn!("creator schema not applied: {e}");
    }
}

/// The Creator tables this build's store would create. Named so the edition
/// tests and the PVR proof can assert on the list rather than on prose.
pub fn creator_tables() -> &'static [&'static str] {
    strivo_research::creator::TABLES
}

/// Path of the file a Creator install writes. Split out for the edition
/// tests, which must not touch a real data directory.
pub fn ensure_schema_at(path: &Path) -> Result<i64, String> {
    strivo_research::creator::ensure_schema(path).map_err(|e| e.to_string())
}

/// The only place the Creator job vocabulary and the daemon's are tied
/// together.
///
/// `strivo-research` sits **below** `strivo-core` in the dependency graph, so
/// `creator::jobs` cannot `use` `StageState` — it can only restate the seven
/// names it persists. A restatement with nothing checking it is a second
/// vocabulary, which is the thing this module exists to prevent. This module
/// is the seam: it is `#[cfg(feature = "creator")]` on the `strivo-web` side,
/// so the check runs in a Creator build and a PVR binary contains no statement
/// from either enum.
///
/// The check drives the real `PipelineRegistry` through its own transitions and
/// compares, so it fails on a *divergence*, not on a difference in spelling
/// conventions. Both `creator::jobs` and `schema_v3.sql` refer to this test;
/// before it existed both were citing a safeguard that did not run.
#[cfg(all(test, feature = "creator"))]
mod edition_tests {
    use strivo_core::pipeline::stage::{Pipeline, Stage, StageKind};
    use strivo_core::pipeline::{PipelineRegistry, StageState};
    use strivo_research::creator::{is_legal, JobState, TRANSITIONS};

    /// The variant name, with no payload. `Running` and `Failed` carry fields
    /// in the daemon's enum and none here, so the comparison is on names — the
    /// thing the two vocabularies are actually claimed to share.
    fn variant(state: &StageState) -> &'static str {
        match state {
            StageState::Pending => "pending",
            StageState::Running { .. } => "running",
            StageState::Done => "done",
            StageState::Failed { .. } => "failed",
            StageState::Exhausted { .. } => "exhausted",
            StageState::Cancelled => "cancelled",
            StageState::Skipped => "skipped",
        }
    }

    fn registry_with_one_stage() -> (PipelineRegistry, strivo_core::pipeline::StageId) {
        let mut pipeline = Pipeline::new("edition");
        let stage = pipeline.add_stage(Stage::new("render", StageKind::ExportClip));
        let mut registry = PipelineRegistry::new();
        registry.submit(pipeline).expect("submit");
        (registry, stage)
    }

    /// `mark_stage_running` on a submitted pipeline, which is how a stage
    /// starts. Kept as a helper so the tests read as the transitions they
    /// assert rather than as setup.
    fn started(registry: &mut PipelineRegistry, stage: strivo_core::pipeline::StageId) {
        let id = registry.iter().next().expect("one pipeline").id;
        assert!(registry.mark_stage_running(id, stage), "the stage starts");
    }

    fn state_of(
        registry: &PipelineRegistry,
        stage: strivo_core::pipeline::StageId,
    ) -> Option<&'static str> {
        registry
            .iter()
            .flat_map(|p| p.stages.iter())
            .find(|s| s.id == stage)
            .map(|s| variant(&s.state))
    }

    #[test]
    fn the_registry_admits_every_creator_transition_it_is_credited_with() {
        // Each row names the `PipelineRegistry` method that admits it. This
        // drives those methods against a real registry and checks the row, so
        // a row that stops being true fails rather than quietly documenting
        // behaviour the daemon does not have.
        use strivo_core::pipeline::StageId;

        // running, from pending
        let (mut reg, stage) = registry_with_one_stage();
        started(&mut reg, stage);
        assert_eq!(state_of(&reg, stage), Some("running"));

        // done, from running
        let id = reg.iter().next().unwrap().id;
        assert!(reg.complete_stage(id, stage, Vec::new()));
        assert_eq!(state_of(&reg, stage), Some("done"));

        // failed with retries remaining, from running
        let (mut reg, stage) = registry_with_one_stage();
        started(&mut reg, stage);
        reg.mark_stage_failed(stage, "boom".into());
        assert_eq!(state_of(&reg, stage), Some("failed"));

        // running again, from failed
        let id = reg.iter().next().unwrap().id;
        assert!(
            reg.mark_stage_running(id, stage),
            "a failed stage admits a fresh attempt"
        );
        assert_eq!(state_of(&reg, stage), Some("running"));

        // pending, from failed (retry)
        let mut reg2 = reg;
        reg2.mark_stage_failed(stage, "boom".into());
        assert!(reg2.retry_stage(stage, None).is_some());
        assert_eq!(state_of(&reg2, stage), Some("pending"));

        // exhausted, from running, when the budget is spent
        let (mut reg, stage) = registry_with_one_stage();
        let mut pipeline = reg.iter().next().cloned().expect("pipeline");
        pipeline.stages[0].max_attempts = 1;
        let id = pipeline.id;
        reg = PipelineRegistry::new();
        reg.submit(pipeline).expect("resubmit");
        started(&mut reg, stage);
        reg.mark_stage_failed(stage, "boom".into());
        assert_eq!(state_of(&reg, stage), Some("exhausted"));

        // pending, from exhausted, with the budget restored
        assert!(reg.retry_stage(stage, None).is_some());
        assert_eq!(state_of(&reg, stage), Some("pending"));
        let _ = id;

        // cancelled, from running
        let (mut reg, stage) = registry_with_one_stage();
        started(&mut reg, stage);
        let id = reg.iter().next().unwrap().id;
        reg.cancel_pipeline(id);
        assert_eq!(state_of(&reg, stage), Some("cancelled"));

        // skipped, from pending
        let (mut reg, stage) = registry_with_one_stage();
        assert!(reg.skip_stage(stage).is_some());
        assert_eq!(state_of(&reg, stage), Some("skipped"));

        // skipped, from failed
        let (mut reg, stage) = registry_with_one_stage();
        started(&mut reg, stage);
        reg.mark_stage_failed(stage, "boom".into());
        assert!(reg.skip_stage(stage).is_some());
        assert_eq!(state_of(&reg, stage), Some("skipped"));

        // and the registry really is a typed handle, not a loose one
        let _: StageId = stage;
    }

    #[test]
    fn every_credited_transition_is_in_the_table() {
        // The rows above are the table's rows. Asserted separately so a table
        // that loses an edge fails with a name rather than as a silently
        // shorter list of cases.
        for (from, to) in [
            (JobState::Pending, JobState::Running),
            (JobState::Running, JobState::Done),
            (JobState::Running, JobState::Failed),
            (JobState::Running, JobState::Exhausted),
            (JobState::Running, JobState::Cancelled),
            (JobState::Failed, JobState::Running),
            (JobState::Failed, JobState::Pending),
            (JobState::Exhausted, JobState::Pending),
            (JobState::Pending, JobState::Skipped),
            (JobState::Pending, JobState::Cancelled),
        ] {
            assert!(is_legal(from, to), "TRANSITIONS lost ({from}, {to})");
        }
    }

    #[test]
    fn a_creator_state_the_daemon_cannot_reach_is_refused() {
        // The reverse direction: the daemon's terminal set must be the
        // Creator enum's, or `recover_jobs` would re-queue something the
        // runtime considers finished.
        for name in ["done", "exhausted", "cancelled", "skipped"] {
            let state = JobState::ALL
                .iter()
                .find(|s| s.as_str() == name)
                .unwrap_or_else(|| panic!("no creator state named {name}"));
            assert!(
                state.is_settled(),
                "{name} is settled in the daemon and must be settled here too"
            );
        }
        assert_eq!(
            TRANSITIONS
                .iter()
                .filter(|(from, _)| *from == JobState::Done)
                .count(),
            0,
            "the daemon cannot leave Done, and neither can this"
        );
    }
}
