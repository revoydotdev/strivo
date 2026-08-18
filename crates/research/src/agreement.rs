//! Inter-rater reliability (Cohen's kappa) between two human coders.
//!
//! **Unitization rule.** Cohen's kappa needs a fixed set of comparison
//! units where both raters had the same opportunity to assign a category.
//! Codings are free-form time ranges, not pre-segmented units, so this
//! module manufactures units itself: for each `source_id` either coder
//! touched, it tiles the span from the earliest `start_ms` to the latest
//! `end_ms` (across both coders, restricted to `code_id` when given) into
//! fixed 1-second (`WINDOW_MS`) windows. For each window and each coder,
//! the category is the `code_id` of the first coding (ordered by
//! `start_ms`, then `id`) whose range overlaps the window, or `None`
//! ("no code assigned here") if no coding by that coder covers it.
//!
//! A 1-second window was chosen because it is finer than nearly all
//! qualitative-coding excerpts (which run several seconds to minutes) so it
//! rarely bisects a single coding, while still being coarse enough that
//! millisecond-level boundary jitter between two coders marking "the same"
//! moment does not manufacture spurious disagreement. This is the standard
//! interval-tiling approach used by desktop QDA tools (e.g. QualCoder's
//! overlap-based agreement) adapted to a fixed-width grid so units are
//! well-defined even when the two coders' ranges do not align.
//!
//! When `code_id` is `Some`, only that code's codings are considered from
//! either coder, and the per-window category collapses to a two-valued
//! presence/absence judgement (`Some(code_id)` vs `None`) — the standard
//! single-code agreement question ("did both coders flag this moment with
//! this code?"). When `code_id` is `None`, every human-authored code from
//! both coders participates, and the per-window category is whichever code
//! (if any) each coder assigned — a multi-class agreement question across
//! the whole codebook.
//!
//! Only `codings.origin = 'human'` rows count: model-assisted and imported
//! codings are not human judgements and would bias reliability estimates
//! that exist specifically to validate human coder consistency.
//!
//! Po (observed agreement) is the fraction of windows where both coders'
//! categories match. Pe (expected agreement) is computed from each coder's
//! marginal category distribution over the same window set, per Cohen
//! (1960): `Pe = sum_c P_a(c) * P_b(c)`. `kappa = (Po - Pe) / (1 - Pe)`.
//!
//! When `Pe == 1.0` (both coders assigned the identical single category to
//! every window — no variance in either marginal), the standard formula
//! divides by zero. In that case `Po` is necessarily also `1.0` (a coder
//! that always picks one category and agrees with a coder who does the
//! same has by definition never disagreed), so this module defines kappa
//! as `1.0` and documents the guard rather than propagating `NaN`.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{parse_uuid, ResearchError, ResearchStore, Result};

/// Width of one comparison unit, in milliseconds. See module docs.
const WINDOW_MS: u64 = 1_000;

/// Cohen's kappa between two coders' human-authored codings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Agreement {
    pub cohens_kappa: f64,
    pub observed_agreement: f64,
    pub expected_agreement: f64,
    pub n: u64,
    pub author_a: String,
    pub author_b: String,
}

/// A category is the code assigned to a window, or `None` if the coder
/// assigned no (matching) code to it.
type Category = Option<Uuid>;

struct CodingSpan {
    start_ms: u64,
    end_ms: u64,
    code_id: Uuid,
}

impl ResearchStore {
    /// Cohen's kappa between `author_a` and `author_b`'s human codings in
    /// `project_id`, optionally restricted to one `code_id`. See the module
    /// doc comment for the unitization rule.
    pub fn agreement(
        &self,
        project_id: Uuid,
        code_id: Option<Uuid>,
        author_a: &str,
        author_b: &str,
    ) -> Result<Agreement> {
        let author_a = author_a.trim();
        let author_b = author_b.trim();
        if author_a.is_empty() || author_b.is_empty() {
            return Err(ResearchError::Validation(
                "agreement requires two non-empty coder authors".into(),
            ));
        }

        let spans_a = fetch_human_codings(&self.conn, project_id, code_id, author_a)?;
        let spans_b = fetch_human_codings(&self.conn, project_id, code_id, author_b)?;

        let mut source_ids: Vec<Uuid> = spans_a
            .keys()
            .chain(spans_b.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        source_ids.sort();

        let mut pairs: Vec<(Category, Category)> = Vec::new();
        for source_id in source_ids {
            let empty = Vec::new();
            let a = spans_a.get(&source_id).unwrap_or(&empty);
            let b = spans_b.get(&source_id).unwrap_or(&empty);
            let Some((min_start, max_end)) = span_bounds(a, b) else {
                continue;
            };
            let mut window_start = min_start;
            while window_start < max_end {
                let window_end = window_start + WINDOW_MS;
                pairs.push((
                    category_for_window(a, window_start, window_end),
                    category_for_window(b, window_start, window_end),
                ));
                window_start = window_end;
            }
        }

        Ok(compute_kappa(pairs, author_a, author_b))
    }
}

fn span_bounds(a: &[CodingSpan], b: &[CodingSpan]) -> Option<(u64, u64)> {
    let min_start = a.iter().chain(b.iter()).map(|span| span.start_ms).min()?;
    let max_end = a.iter().chain(b.iter()).map(|span| span.end_ms).max()?;
    if max_end <= min_start {
        // Degenerate (zero-width) coverage still deserves one unit.
        return Some((min_start, min_start + WINDOW_MS));
    }
    Some((min_start, max_end))
}

fn category_for_window(spans: &[CodingSpan], window_start: u64, window_end: u64) -> Category {
    spans
        .iter()
        .find(|span| span.start_ms < window_end && span.end_ms > window_start)
        .map(|span| span.code_id)
}

fn compute_kappa(pairs: Vec<(Category, Category)>, author_a: &str, author_b: &str) -> Agreement {
    let n = pairs.len() as u64;
    if n == 0 {
        return Agreement {
            cohens_kappa: 0.0,
            observed_agreement: 0.0,
            expected_agreement: 0.0,
            n: 0,
            author_a: author_a.to_string(),
            author_b: author_b.to_string(),
        };
    }

    let matches = pairs.iter().filter(|(a, b)| a == b).count();
    let observed_agreement = matches as f64 / n as f64;

    let mut counts_a: HashMap<Category, u64> = HashMap::new();
    let mut counts_b: HashMap<Category, u64> = HashMap::new();
    for (a, b) in &pairs {
        *counts_a.entry(*a).or_insert(0) += 1;
        *counts_b.entry(*b).or_insert(0) += 1;
    }

    let categories: std::collections::HashSet<Category> =
        counts_a.keys().chain(counts_b.keys()).copied().collect();
    let expected_agreement: f64 = categories
        .into_iter()
        .map(|category| {
            let p_a = *counts_a.get(&category).unwrap_or(&0) as f64 / n as f64;
            let p_b = *counts_b.get(&category).unwrap_or(&0) as f64 / n as f64;
            p_a * p_b
        })
        .sum();

    // Guard: Pe == 1.0 only when both coders assigned the same single
    // category to every window, which forces Po == 1.0 too. See module docs.
    let cohens_kappa = if (1.0 - expected_agreement).abs() < f64::EPSILON {
        1.0
    } else {
        (observed_agreement - expected_agreement) / (1.0 - expected_agreement)
    };

    Agreement {
        cohens_kappa,
        observed_agreement,
        expected_agreement,
        n,
        author_a: author_a.to_string(),
        author_b: author_b.to_string(),
    }
}

fn fetch_human_codings(
    conn: &Connection,
    project_id: Uuid,
    code_id: Option<Uuid>,
    author: &str,
) -> Result<HashMap<Uuid, Vec<CodingSpan>>> {
    let mut stmt = conn.prepare(
        "SELECT source_id, start_ms, end_ms, code_id
         FROM codings
         WHERE project_id = ?1 AND author = ?2 AND origin = 'human'
           AND (?3 IS NULL OR code_id = ?3)
         ORDER BY start_ms, id",
    )?;
    let rows = stmt.query_map(
        params![
            project_id.to_string(),
            author,
            code_id.map(|id| id.to_string())
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;

    let mut by_source: HashMap<Uuid, Vec<CodingSpan>> = HashMap::new();
    for row in rows {
        let (source_id, start_ms, end_ms, code_id) = row?;
        by_source
            .entry(parse_uuid(source_id)?)
            .or_default()
            .push(CodingSpan {
                start_ms: start_ms.max(0) as u64,
                end_ms: end_ms.max(0) as u64,
                code_id: parse_uuid(code_id)?,
            });
    }
    Ok(by_source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CodingOrigin, NewCoding, NewSource, SourceKind};

    fn source(project_id: Uuid) -> NewSource {
        NewSource {
            id: Uuid::new_v4(),
            project_id,
            recording_id: None,
            kind: SourceKind::Recording,
            title: "Episode".into(),
            uri: None,
            duration_ms: None,
            attributes: serde_json::json!({}),
        }
    }

    fn coding(
        project_id: Uuid,
        source_id: Uuid,
        code_id: Uuid,
        author: &str,
        start_ms: u64,
        end_ms: u64,
    ) -> NewCoding {
        NewCoding {
            id: Uuid::new_v4(),
            project_id,
            source_id,
            code_id,
            start_ms,
            end_ms,
            excerpt: String::new(),
            note: String::new(),
            author: author.into(),
            origin: CodingOrigin::Human,
            confidence: None,
            provenance_id: None,
        }
    }

    fn setup() -> (ResearchStore, Uuid, Uuid, crate::Code, crate::Code) {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let src = source(project.id);
        store.upsert_source(&src).unwrap();
        let code_x = crate::Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: None,
            name: "X".into(),
            description: String::new(),
            color: "#ffffff".into(),
        };
        store.create_code(&code_x).unwrap();
        let code_y = crate::Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: None,
            name: "Y".into(),
            description: String::new(),
            color: "#000000".into(),
        };
        store.create_code(&code_y).unwrap();
        (store, project.id, src.id, code_x, code_y)
    }

    #[test]
    fn perfect_agreement_yields_kappa_one() {
        let (mut store, project_id, source_id, code_x, _) = setup();
        for author in ["alice", "bob"] {
            store
                .add_coding(&coding(project_id, source_id, code_x.id, author, 0, 5_000))
                .unwrap();
        }
        let result = store.agreement(project_id, None, "alice", "bob").unwrap();
        assert!((result.cohens_kappa - 1.0).abs() < 1e-9);
        assert!((result.observed_agreement - 1.0).abs() < 1e-9);
        assert!(result.n > 0);
    }

    #[test]
    fn total_disagreement_yields_low_or_negative_kappa() {
        let (mut store, project_id, source_id, code_x, code_y) = setup();
        store
            .add_coding(&coding(project_id, source_id, code_x.id, "alice", 0, 5_000))
            .unwrap();
        store
            .add_coding(&coding(project_id, source_id, code_y.id, "bob", 0, 5_000))
            .unwrap();
        let result = store.agreement(project_id, None, "alice", "bob").unwrap();
        assert!((result.observed_agreement - 0.0).abs() < 1e-9);
        assert!(result.cohens_kappa <= 0.0);
    }

    #[test]
    fn chance_level_agreement_yields_kappa_near_zero() {
        let (mut store, project_id, source_id, code_x, code_y) = setup();
        // Four windows, each coder split 50/50 between X and Y, arranged so
        // observed agreement (2/4 match) equals expected-by-chance
        // agreement (0.5*0.5 + 0.5*0.5 = 0.5) exactly: kappa == 0.
        let plan = [
            (code_x.id, code_x.id), // match
            (code_x.id, code_y.id), // mismatch
            (code_y.id, code_x.id), // mismatch
            (code_y.id, code_y.id), // match
        ];
        for (i, (alice_code, bob_code)) in plan.into_iter().enumerate() {
            let start = i as u64 * 1_000;
            let end = start + 1_000;
            store
                .add_coding(&coding(
                    project_id, source_id, alice_code, "alice", start, end,
                ))
                .unwrap();
            store
                .add_coding(&coding(project_id, source_id, bob_code, "bob", start, end))
                .unwrap();
        }
        let result = store.agreement(project_id, None, "alice", "bob").unwrap();
        assert!((result.observed_agreement - 0.5).abs() < 1e-9);
        assert!((result.expected_agreement - 0.5).abs() < 1e-9);
        assert!(result.cohens_kappa.abs() < 1e-9);
    }

    #[test]
    fn single_code_no_variance_hits_guard_and_returns_one() {
        let (mut store, project_id, source_id, code_x, _) = setup();
        store
            .add_coding(&coding(project_id, source_id, code_x.id, "alice", 0, 1_000))
            .unwrap();
        store
            .add_coding(&coding(project_id, source_id, code_x.id, "bob", 0, 1_000))
            .unwrap();
        let result = store
            .agreement(project_id, Some(code_x.id), "alice", "bob")
            .unwrap();
        assert_eq!(result.expected_agreement, 1.0);
        assert_eq!(result.cohens_kappa, 1.0);
    }

    #[test]
    fn empty_input_returns_zeroed_agreement_without_error() {
        let (store, project_id, _source_id, _code_x, _) = setup();
        let result = store.agreement(project_id, None, "alice", "bob").unwrap();
        assert_eq!(result.n, 0);
        assert_eq!(result.cohens_kappa, 0.0);
        assert_eq!(result.observed_agreement, 0.0);
        assert_eq!(result.expected_agreement, 0.0);
    }

    #[test]
    fn rejects_blank_authors() {
        let (store, project_id, _source_id, _code_x, _) = setup();
        assert!(store.agreement(project_id, None, "", "bob").is_err());
        assert!(store.agreement(project_id, None, "alice", "  ").is_err());
    }
}
