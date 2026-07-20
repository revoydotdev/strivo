//! Pipeline DAG engine — host-side orchestrator for multi-stage plugin jobs.
//!
//! Plugins compose a [`Pipeline`] of typed [`Stage`]s (extract / transcribe /
//! diarize / subtitle / analyze / clip / concat / …) and submit it via
//! `PluginAction::SubmitPipeline`. The host executor walks the DAG: stages
//! with all inputs satisfied are dispatched; each owns a `CancellationToken`
//! and declares resource locks (GPU, per-provider rate limits) so concurrent
//! pipelines don't trample each other.
//!
//! M4 MVP scope:
//!   - in-memory registry (no `~/.local/share/strivo/pipelines/<uuid>.json`
//!     persistence yet — first crash loses queue; tracked for M5).
//!   - cooperative cancellation only (poll the token between awaits).
//!   - resource locks via a shared [`ResourceRegistry`].
//!   - retries: per-stage `max_attempts` with exponential backoff (5/10/30 s).
//!
//! See `docs/PIPELINE.md` and the user-facing UX plan (Part 6, X1) for the
//! design context.

pub mod executor;
pub mod stage;

pub use executor::{PipelineRegistry, ResourceRegistry, StageAdvance, StageDispatch};
pub use stage::{Pipeline, PipelineId, PipelineState, Stage, StageId, StageKind, StageState};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_round_trip() {
        let p = Pipeline::new("test".to_string());
        assert!(matches!(p.state, PipelineState::Pending));
        assert_eq!(p.stages.len(), 0);
    }

    #[test]
    fn stage_topology_acyclic() {
        let mut p = Pipeline::new("x".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract));
        let b = p.add_stage(Stage::new("b", StageKind::Custom("noop".into())).with_inputs(vec![a]));
        assert!(p.assert_acyclic().is_ok());
        // Cycle: introduce a back-edge from a → b → a
        if let Some(s) = p.stages.iter_mut().find(|s| s.id == a) {
            s.inputs.push(b);
        }
        assert!(p.assert_acyclic().is_err());
    }
}
