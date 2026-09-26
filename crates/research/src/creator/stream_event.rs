//! The stream event: a marker, a category or a provider report, placed on
//! the source clock.
//!
//! An event is a **point**, not a window, so it has no poll interval and its
//! uncertainty bound is the clock's own — a measured zero on an exact
//! segment, which is the contrast that shows the bound is derived rather
//! than a constant. Zero there is a measurement; the same zero used as a
//! fallback would be a fabrication.
//!
//! `label` and `category` are untrusted external text. They are stored and
//! returned verbatim and nothing in this crate interprets them; the only
//! thing done to them here is a length bound, and over it is a refusal
//! rather than a truncation, because a shortened label reads exactly like a
//! whole one.

use strivo_editor::time::{ClockMap, MediaTime};

use super::ids::{CaptureSessionId, ProjectId};
use super::observation_error::ObservationError;
use super::observations::{place, CaptureWindow, SamplePosition, MAX_LABEL_BYTES};

/// The store's own result type, so a refusal stays typed to the caller.
type Result<T> = std::result::Result<T, ObservationError>;

/// One row as SQLite hands it back, in column order.
pub(super) type EventRow = (
    i64,
    String,
    Option<i64>,
    i64,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// What a stream event is. Markers and categories are the creator's own
/// structure; a provider event is something the platform reported. Kept as
/// three names rather than a free string so a consumer can tell the
/// creator's own annotation from the platform's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Marker,
    Category,
    ProviderEvent,
}

impl EventKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Marker => "marker",
            Self::Category => "category",
            Self::ProviderEvent => "provider_event",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "marker" => Some(Self::Marker),
            "category" => Some(Self::Category),
            "provider_event" => Some(Self::ProviderEvent),
            _ => None,
        }
    }
}

/// One event as it arrived, with its untrusted text carried as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawStreamEvent {
    pub wall_ms: i64,
    pub kind: EventKind,
    /// Untrusted external text — a provider's title, a creator's own note.
    /// Stored and returned verbatim; nothing in this crate interprets it.
    pub label: String,
    pub category: Option<String>,
    pub payload_json: String,
}

/// One persisted stream event, placed on the source clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    pub capture_session_id: CaptureSessionId,
    pub project_id: ProjectId,
    pub seq: u32,
    pub kind: EventKind,
    pub wall_ms: i64,
    pub position: SamplePosition,
    pub label: String,
    pub category: Option<String>,
    pub payload_json: String,
}

impl StreamEvent {
    /// Place an event on the source clock.
    ///
    /// An event is a point, so there is no poll window and the bound is the
    /// clock's own — a measured zero on an exact segment.
    pub fn observe(window: &CaptureWindow, clock: &ClockMap, raw: RawStreamEvent) -> Result<Self> {
        if raw.label.len() > MAX_LABEL_BYTES {
            return Err(ObservationError::LabelTooLong {
                bytes: raw.label.len(),
                limit: MAX_LABEL_BYTES,
            });
        }
        if let Some(category) = &raw.category {
            if category.len() > MAX_LABEL_BYTES {
                return Err(ObservationError::LabelTooLong {
                    bytes: category.len(),
                    limit: MAX_LABEL_BYTES,
                });
            }
        }
        let position = place(window, clock, raw.wall_ms, 0)?;
        Ok(Self {
            capture_session_id: CaptureSessionId::generate(),
            project_id: ProjectId::generate(),
            seq: 0,
            kind: raw.kind,
            wall_ms: raw.wall_ms,
            position,
            label: raw.label,
            category: raw.category,
            payload_json: raw.payload_json,
        })
    }

    /// Rebuild an event from its stored columns, refusing a row whose `kind`
    /// is not one this model names rather than defaulting it to a marker —
    /// a default would turn an unknown provider event into the creator's own
    /// annotation, which is a different claim about provenance.
    pub(super) fn from_row(session: CaptureSessionId, row: EventRow) -> Result<Self> {
        const TABLE: &str = "creator_stream_events";
        let missing = |column: &str| ObservationError::UnreadableRow {
            table: TABLE,
            detail: format!("{column} is NULL: the row was written by an interrupted writer"),
        };
        let (seq, kind_raw, wall_ms, media_us, uncertainty_us, label, category, payload_json) = row;
        Ok(Self {
            capture_session_id: session,
            project_id: ProjectId::generate(),
            seq: u32::try_from(seq).map_err(|_| ObservationError::UnreadableRow {
                table: TABLE,
                detail: format!("seq {seq} is negative"),
            })?,
            kind: EventKind::parse(&kind_raw).ok_or_else(|| ObservationError::UnreadableRow {
                table: TABLE,
                detail: format!("kind {kind_raw:?} is not a kind this model names"),
            })?,
            wall_ms: wall_ms.ok_or_else(|| missing("wall_ms"))?,
            position: SamplePosition {
                media: MediaTime::from_micros(media_us).map_err(|e| {
                    ObservationError::UnreadableRow {
                        table: TABLE,
                        detail: format!("media_us: {e}"),
                    }
                })?,
                uncertainty_micros: uncertainty_us.ok_or_else(|| missing("uncertainty_us"))?,
            },
            label: label.ok_or_else(|| missing("label"))?,
            category,
            payload_json: payload_json.unwrap_or_else(|| "{}".to_string()),
        })
    }
}
