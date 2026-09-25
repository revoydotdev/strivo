//! EDL — Edit Decision List format.
//!
//! Universal artifact for batch jobs, transcript edits, and saved pipeline
//! presets. Stored as JSON under `~/.local/share/strivo/edls/<name>.edl.json`
//! or as a sidecar next to a recording (`<recording>.edl.json`).
//!
//! Three kinds:
//! - `batch` — list of vods + pipeline ops applied to each.
//! - `edit` — clip/concat ops on one or more vods.
//! - `preset` — reusable op chain with no inputs bound; turned into a
//!   runnable graph by `render::pipeline_from_edl` / `render::pipeline_from_preset_stages`.
//!
//! See plan Part 6 X2.

pub mod render;
pub mod schema;

pub use schema::{EdlDoc, EdlInput, EdlKind, EdlOp, EDL_VERSION};

use std::path::Path;

use anyhow::{Context, Result};

/// Read an EDL from disk. Validates the schema version + topology.
pub fn load(path: &Path) -> Result<EdlDoc> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read EDL at {}", path.display()))?;
    let doc: EdlDoc =
        serde_json::from_str(&text).with_context(|| format!("parse EDL at {}", path.display()))?;
    doc.validate()
        .with_context(|| format!("validate EDL at {}", path.display()))?;
    Ok(doc)
}

/// Atomic write: serialize, write to `.tmp`, rename. Survives a SIGKILL
/// mid-write without leaving a half-EDL on disk.
pub fn save(path: &Path, doc: &EdlDoc) -> Result<()> {
    doc.validate()
        .context("EDL failed validation before save")?;
    let tmp = path.with_extension("tmp");
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let text = serde_json::to_string_pretty(doc).context("serialize EDL")?;
    std::fs::write(&tmp, text).context("write EDL .tmp")?;
    std::fs::rename(&tmp, path).context("rename EDL .tmp → final")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use schema::*;

    #[test]
    fn round_trip_preset_edl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.edl.json");

        let doc = EdlDoc {
            version: EDL_VERSION,
            kind: EdlKind::Preset,
            name: "fast-cheap".into(),
            inputs: vec![],
            ops: vec![
                EdlOp::Transcribe {
                    provider: "whisper-cli".into(),
                    params: Default::default(),
                },
                EdlOp::Subtitle,
            ],
            created_at: "2026-05-23T00:00:00Z".into(),
            created_by: "strivo-test".into(),
        };

        save(&path, &doc).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.name, "fast-cheap");
        assert_eq!(loaded.ops.len(), 2);
        assert!(matches!(loaded.kind, EdlKind::Preset));
    }

    #[test]
    fn version_mismatch_rejected() {
        let mut doc = EdlDoc::new(EdlKind::Preset, "x");
        doc.version = 999;
        assert!(doc.validate().is_err());
    }

    #[test]
    fn concat_indices_validated() {
        let mut doc = EdlDoc::new(EdlKind::Edit, "bad");
        doc.ops = vec![
            EdlOp::Clip {
                in_word: 0,
                out_word: 10,
                label: "a".into(),
            },
            // Concat references clip index 5 which doesn't exist.
            EdlOp::Concat {
                clips: vec![0, 5],
                output: "out.mkv".into(),
            },
        ];
        assert!(doc.validate().is_err());
    }
}
