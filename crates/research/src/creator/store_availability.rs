//! Persistence for a metric's availability state.
//!
//! A metric nobody holds a permission for is a **fact with a reason**, and
//! this is where that fact lives. Without it the only honest-looking
//! representation of an ungranted permission is an empty series — and an
//! empty series is indistinguishable from a stream that was polled all night
//! and came back with nothing, which is the exact conflation the plan names.
//!
//! The state is per `(capture_session_id, metric)`, so one session holding a
//! grant says nothing about another that does not, and re-declaring replaces
//! rather than accumulates: two rows would give "is it available?" two
//! answers.
//!
//! The rate-limit retry hint lives in `detail` rather than a second column,
//! so a rate limit's particulars have one home instead of two that can
//! disagree.

use rusqlite::{params, OptionalExtension};

use super::ids::{CaptureSessionId, ProjectId};
use super::observation_error::ObservationError;
use super::observations::{MetricAvailability, MetricName, UnavailableReason};
use super::store::Store;

/// The store's own result type, so a refusal stays typed to the caller.
type Result<T> = std::result::Result<T, ObservationError>;

impl Store {
    /// Record whether a metric yielded a value for this session.
    ///
    /// The `Unavailable` arm is the point of the method: a permission nobody
    /// holds is a *state*, persisted and readable, rather than an empty
    /// series that reads the same as a stream nobody polled.
    pub fn declare_metric(
        &self,
        session: CaptureSessionId,
        project: ProjectId,
        metric: &str,
        state: MetricAvailability,
    ) -> Result<()> {
        let name = MetricName::parse(metric)?;
        let (reason, detail) = match &state {
            MetricAvailability::Observed(value) => ("observed", value.to_string()),
            MetricAvailability::Unavailable { reason, detail } => {
                let detail = match reason {
                    UnavailableReason::RateLimited { retry_after_ms }
                        if !detail.contains("retry_after_ms=") =>
                    {
                        format!("{detail} retry_after_ms={retry_after_ms}")
                    }
                    _ => detail.clone(),
                };
                (reason.as_str(), detail)
            }
        };
        self.conn.execute(
            "INSERT INTO creator_metric_availability
             (capture_session_id,project_id,metric,state,reason,detail,declared_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(capture_session_id,metric) DO UPDATE SET
               state=excluded.state, reason=excluded.reason,
               detail=excluded.detail, declared_at=excluded.declared_at",
            params![
                session.to_string(),
                project.to_string(),
                name.as_str(),
                if state.is_unavailable() {
                    "unavailable"
                } else {
                    "observed"
                },
                reason,
                detail,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// The state declared for a metric, or `None` when this session never
    /// declared one — which is a third state, distinct from both an observed
    /// zero and an unheld permission.
    pub fn metric_state(
        &self,
        session: CaptureSessionId,
        metric: &str,
    ) -> Result<Option<MetricAvailability>> {
        let row: Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT state,reason,detail FROM creator_metric_availability
                 WHERE capture_session_id=?1 AND metric=?2",
                params![session.to_string(), metric],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((state, reason, detail)) = row else {
            return Ok(None);
        };
        if state == "observed" {
            return Ok(Some(MetricAvailability::Observed(detail.parse().map_err(
                |_| ObservationError::UnreadableRow {
                    table: "creator_metric_availability",
                    detail: format!("observed value {detail:?} is not a count"),
                },
            )?)));
        }
        Ok(Some(MetricAvailability::Unavailable {
            reason: UnavailableReason::parse(&reason, &detail).ok_or(
                ObservationError::UnreadableRow {
                    table: "creator_metric_availability",
                    detail: format!("reason {reason:?} is not one this model names"),
                },
            )?,
            detail,
        }))
    }
}
