//! Host-owned executable workflow templates.

use uuid::Uuid;

use super::stage::ResourceLock;
use super::{Pipeline, Stage, StageDispatch, StageKind};

/// The first production Creator workflow. Crunchr implements the capability
/// as an idempotent stage while the host supplies orchestration guarantees.
///
/// Keeping this template in core ensures manual web runs, plugin-triggered
/// runs, and future schedule/API triggers produce byte-for-byte equivalent
/// graphs.
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
        ])
        .with_max_attempts(3),
    );
    pipeline
}

/// End-to-end publish preparation. Transcript intelligence and visual scene
/// mining form independent roots, fan out, and converge into publish-ready
/// media, reusable drafts, and a final editorial brief.
pub fn creator_publish(recording_id: Uuid, trigger: impl Into<String>) -> Pipeline {
    let mut pipeline = Pipeline::new("Ultimate creator publish")
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
        .with_requires(vec![ResourceLock::File {
            path: format!("recording:{recording_id}"),
        }]),
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
        .with_requires(vec![ResourceLock::File {
            path: format!("artifact:{recording_id}:{verb}"),
        }])
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
