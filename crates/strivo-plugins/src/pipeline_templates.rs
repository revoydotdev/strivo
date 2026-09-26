//! Creator Edition's executable pipeline workflow templates.
//!
//! These build [`Pipeline`] graphs out of concrete Creator plugin names
//! (`crunchr`, `cuepoints`, `chapters`, `clipper`, …) — real, externally
//! visible identifiers that `routes::plugins`' `gate_pro(..)` calls and the
//! plugin registry's `name()` also match on. That vocabulary belongs here,
//! in the Creator plugin host, not in `strivo-core`: core only supplies the
//! generic `Pipeline`/`Stage`/`StageDispatch` types these templates are
//! built from (see ADR 0001, CE01). Moved out of
//! `strivo-core::pipeline::templates` for exactly that reason.
//!
//! Living in the one Creator Edition process (not duplicated per call
//! site) still ensures manual web runs, plugin-triggered runs, and future
//! schedule/API triggers produce byte-for-byte equivalent graphs.

use uuid::Uuid;

use strivo_core::pipeline::stage::ResourceLock;
use strivo_core::pipeline::{Pipeline, Stage, StageDispatch, StageKind};
pub fn creator_intelligence(recording_id: Uuid, trigger: impl Into<String>) -> Pipeline {
    let mut pipeline = Pipeline::new("Creator intelligence")
        .for_recording(recording_id)
        .with_trigger(trigger);
    pipeline.add_stage(
        Stage::new(
            "Transcribe, diarize & analyze",
            StageKind::Custom("crunchr.process".to_string()),
        )
        .with_dispatch(StageDispatch::new("crunchr", "transcribe").for_recording(recording_id))
        .with_requires(vec![
            ResourceLock::File {
                path: format!("recording:{recording_id}"),
            },
            ResourceLock::Gpu,
            ResourceLock::Disk { cap: 2 },
        ])
        .with_max_attempts(3),
    );
    pipeline
}

/// End-to-end **preparation** for publication. Transcript intelligence and
/// visual scene mining form independent roots, fan out, and converge into
/// publish-ready media, reusable drafts, and a final editorial brief.
///
/// **The display name says preparation, and that is the correction
/// `docs/CREATOR-EDITION-REPORT-2026-09-07.md:102` asks for.** This pipeline
/// ends at a rendered artifact and a draft. It does not carry a user's final
/// edit revision, render for a destination, upload, reconcile a provider's
/// processing state, schedule, capture a remote id, or ingest a metric — so
/// calling the result "published" told a creator their video was on YouTube
/// when it was on their disk. The contracts that make the rest real now exist
/// in `strivo_research::creator::publication` (the release package and the
/// fourteen-state publication lifecycle); this template produces the *inputs* to
/// a package and nothing more.
///
/// **The `creator_publish` name stays, and it is a machine id, not a claim.**
/// It is the template key the API, the saved `RecipeChain` files on disk and the
/// browser all address it by, so renaming it would break every stored
/// reference to break a label. What a person reads is the name below.
pub fn creator_publish(recording_id: Uuid, trigger: impl Into<String>) -> Pipeline {
    let mut pipeline = Pipeline::new("Ultimate creator preparation")
        .for_recording(recording_id)
        .with_trigger(trigger);
    let intelligence = pipeline.add_stage(
        Stage::new(
            "Transcribe, diarize & analyze",
            StageKind::Custom("crunchr.process".to_string()),
        )
        .with_dispatch(StageDispatch::new("crunchr", "transcribe").for_recording(recording_id))
        .with_requires(vec![
            ResourceLock::File {
                path: format!("recording:{recording_id}"),
            },
            ResourceLock::Gpu,
            ResourceLock::Disk { cap: 2 },
        ])
        .with_max_attempts(3),
    );
    let cuepoints = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Detect visual cuepoints",
            "cuepoints",
            StageKind::Custom("cuepoints.extract".to_string()),
        )
        .with_requires(vec![
            ResourceLock::File {
                path: format!("recording:{recording_id}"),
            },
            ResourceLock::Cpu { cap: 2 },
            ResourceLock::Disk { cap: 2 },
        ]),
    );
    let chapters = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Generate chapters",
            "chapters",
            StageKind::Custom("chapters.generate".to_string()),
        )
        .with_inputs(vec![intelligence]),
    );
    let captions = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Export caption set",
            "captions",
            StageKind::Subtitle,
        )
        .with_inputs(vec![intelligence]),
    );
    let brandsafe = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Scan brand safety",
            "brandsafe",
            StageKind::Analyze {
                provider: "local-rules".to_string(),
            },
        )
        .with_inputs(vec![intelligence]),
    );
    let highlights = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Score highlight moments",
            "highlights",
            StageKind::Custom("clipper.score".to_string()),
        )
        .with_inputs(vec![cuepoints]),
    );
    let clips = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Export highlight clips",
            "clips",
            StageKind::Custom("clipper.export".to_string()),
        )
        .with_inputs(vec![highlights]),
    );
    let thumbnails = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Generate thumbnail candidates",
            "thumbnails",
            StageKind::Custom("thumbnails.generate".to_string()),
        )
        .with_inputs(vec![highlights]),
    );
    let reuse = pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Compose publish drafts",
            "reuse",
            StageKind::Custom("reuse.compose".to_string()),
        )
        .with_inputs(vec![chapters, captions, clips]),
    );
    pipeline.add_stage(
        artifact_stage(
            recording_id,
            "Assemble Casebook",
            "casebook",
            StageKind::Custom("casebook.compose".to_string()),
        )
        .with_inputs(vec![
            chapters, captions, brandsafe, highlights, clips, thumbnails, reuse,
        ]),
    );
    pipeline
}

fn artifact_stage(recording_id: Uuid, name: &str, verb: &str, kind: StageKind) -> Stage {
    Stage::new(name, kind)
        .with_dispatch(StageDispatch::new("artifacts", verb).for_recording(recording_id))
        .with_requires(vec![
            ResourceLock::File {
                path: format!("artifact:{recording_id}:{verb}"),
            },
            ResourceLock::Cpu { cap: 2 },
            ResourceLock::Disk { cap: 2 },
        ])
        .with_max_attempts(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creator_template_is_executable_and_machine_independent() {
        let recording_id = Uuid::new_v4();
        let pipeline = creator_intelligence(recording_id, "manual");
        assert_eq!(pipeline.subject_id, Some(recording_id));
        assert_eq!(pipeline.trigger, "manual");
        assert_eq!(pipeline.stages.len(), 1);
        let dispatch = pipeline.stages[0].dispatch.as_ref().unwrap();
        assert_eq!(dispatch.plugin, "crunchr");
        assert_eq!(dispatch.selection, vec![recording_id]);
        assert!(pipeline.assert_acyclic().is_ok());
    }

    #[test]
    fn publish_template_fans_out_then_converges() {
        let recording_id = Uuid::new_v4();
        let pipeline = creator_publish(recording_id, "manual");
        // The name a person reads says preparation, because this pipeline ends
        // at a local artifact. `docs/CREATOR-EDITION-REPORT-2026-09-07.md:102`.
        assert_eq!(pipeline.name, "Ultimate creator preparation");
        assert!(
            !pipeline.name.contains("publish"),
            "the display name must not claim the video was published: {}",
            pipeline.name,
        );
        assert_eq!(pipeline.stages.len(), 10);
        assert!(pipeline.assert_acyclic().is_ok());
        let root = pipeline.stages[0].id;
        let fanout = &pipeline.stages[2..5];
        assert!(fanout.iter().all(|stage| stage.inputs == vec![root]));
        let casebook = pipeline.stages.last().unwrap();
        assert_eq!(casebook.inputs.len(), 7);
        assert!(pipeline.stages.iter().all(|stage| stage.dispatch.is_some()));
    }
}
