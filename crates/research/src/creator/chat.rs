//! Durable chat: a daemon-owned acquisition record that survives a restart,
//! and a replay aligned to a capture landmark.
//!
//! Report item C03 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:107-108,282`).
//!
//! **What was false before this module.** Chat was a browser WebSocket
//! (`crates/strivo-web/src/routes/chat.rs:1-9`): the SPA opened
//! `wss://irc-ws.chat.twitch.tv` itself, so the record existed only while a tab
//! was open and nothing was persisted — which is why
//! `docs/STRATEGY-NVIVO-RIVERSIDE.md:26-30` had to be corrected. Acquisition
//! now lives in the daemon ([`super::store_chat`] for the record,
//! `crates/strivo-web/src/chat_capture.rs` for the supervisor and the socket),
//! so a closed browser stops rendering chat and nothing else.
//!
//! **A message is a point on the source clock, and a message the clock cannot
//! place is refused.** Every row is built by [`ChatMessage::observe`], which
//! takes a [`ClockMap`]; there is no constructor a caller can hand a
//! [`MediaTime`] to. A message in a reconnect hole is
//! [`ObservationError::OutsideClockMap`], the whole batch is abandoned, and no
//! row is half-written — the same gap-refusal todo 36 established for viewer
//! samples, for the same reason: a chat message pinned to the nearest covered
//! edge is a fabricated caption.
//!
//! **A gap is a recorded interval, never a counter.** There is no
//! `dropped: u64` in this model, and [`ChatAppend::is_accounted_for`] is what
//! holds the line instead: a batch's outcome *partitions* its input, so an
//! event that is neither stored nor a known duplicate cannot be represented at
//! all. The three things that can end a line are all **stated** — stored, a
//! duplicate, or a deletion that named no message — and anything else is an
//! error the caller must handle. A number nobody reads is how a gap becomes
//! invisible, and this is the second time this plan has had to say so.
//!
//! **Identity is content, and the database enforces it.** A message the
//! platform gave an id is identified by that id; one it did not is identified
//! by a sha256 over `(room, sender, wall instant, body)`. So a reconnect that
//! redelivers the last line before the drop is recognised, and re-importing
//! the identical log adds nothing. The `UNIQUE(capture_session_id, identity)`
//! constraint is what makes that a property of the store rather than a
//! convention a caller can route around — todo 33's rule for sources and todo
//! 38's for rights notes, applied to the data most likely to arrive twice.
//!
//! **Coverage for chat is about acquisition, never about talkativeness.** A
//! quiet room is not a gap: nobody speaking is a complete record of a room
//! nobody spoke in, so a density-derived figure would report every lull as a
//! hole and make a real lull indistinguishable from a lost connection. So the
//! denominator is the same [`CaptureWindow`] every other figure uses, the
//! numerator is the union of the intervals a collector was actually open for,
//! and the complement is [`Store::chat_gaps`] — facts the daemon wrote down
//! when it noticed, including the one it can only notice after the fact: a
//! collection row still open at the next start ([`GapReason::DaemonDown`]).
//!
//! **A deletion is recorded, never applied.** A moderator removing a message
//! from the live room is a fact about the room, not an instruction to erase the
//! archive. Applying it would leave a hole with no witness — the same failure
//! as a silent drop, one layer down. So the removal is its own row, the message
//! keeps its text, and a removal whose message we do not hold is reported as
//! [`ChatDeletionTarget::Unidentifiable`] rather than dropped.
//!
//! **Replay cannot be asked for without naming a landmark.**
//! [`Store::replay_chat`] takes a [`Landmark`] and every message it returns
//! carries its [`ChatMessage`] position *and* [`ReplayedChat::offset_from_landmark`].
//! This is the same structural move todo 36 made with the coverage denominator
//! — there is no way to obtain a figure without naming what it is a fraction
//! of — applied to the question "aligned to what?".

use std::fmt;

use sha2::{Digest, Sha256};
use strivo_editor::time::{ClockMap, MediaTime, TimeError};

use super::coverage::{Coverage, CoverageGap, GapReason};
use super::ids::{CaptureSessionId, ChatCollectionId, ProjectId};
use super::observation_error::ObservationError;
use super::observations::{place, CaptureWindow, SamplePosition, MAX_LABEL_BYTES};

/// The store's own result type, so a refusal stays typed to the caller.
type Result<T> = std::result::Result<T, ObservationError>;

/// The largest batch one append will take, matching
/// [`super::observations::MAX_BATCH`] rather than inventing a second bound for
/// the same one-transaction write.
pub use super::observations::MAX_BATCH as MAX_CHAT_BATCH;

/// A message as the daemon received it, before it has a media coordinate.
///
/// `wall_ms` is the **provider's** own `tmi-sent-ts` when it sent one, because
/// that is when the message happened; the daemon's receive instant otherwise.
/// A caller cannot pass a position — only an instant — so there is no way in
/// here to fabricate a caption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawChatMessage {
    pub wall_ms: i64,
    pub room: String,
    pub sender: String,
    pub text: String,
    pub is_action: bool,
    /// The platform's own message id, when it supplied one. Its presence
    /// changes the identity rule, and that is the only reason it exists.
    pub provider_id: Option<String>,
}

/// Which message a moderator removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatDeletionTarget {
    /// The platform's own id for the removed message. Exact when we hold it.
    ProviderId(String),
    /// The removal named no message this daemon could identify — the platform
    /// sent a `CLEARMSG` for a message whose id we never saw, or sent a
    /// timeout notice with no id at all. **Not** a reason to discard the
    /// removal: it is recorded with a NULL target and reported as unresolved.
    Unidentifiable,
}

/// A moderator's removal of a message, as the daemon received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawChatDeletion {
    pub wall_ms: i64,
    pub by: String,
    pub target: ChatDeletionTarget,
}

/// What a removal did, with both numbers stated rather than one of them being
/// derived by subtraction at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionOutcome {
    /// Messages we hold that this removal marked.
    pub marked: usize,
    /// Removals naming no message we hold, or no message at all. Persisted
    /// with `unresolved = 1` so a replay can report them.
    pub unresolved: usize,
}

impl DeletionOutcome {
    /// Every removal is accounted for one way or the other. A third outcome
    /// is not representable, which is the property the plan asked for in place
    /// of a dropped-message counter.
    #[must_use]
    pub fn is_accounted_for(self, removals: usize) -> bool {
        self.marked + self.unresolved == removals
    }
}

/// A message's identity: what makes two observations of it the same message.
///
/// Two spellings, kept apart deliberately. A platform id is a name the platform
/// minted and does not re-mint, so it survives a reconnect that redelivers. A
/// content fingerprint covers the providers that send no id at all, at the cost
/// of being exact only for a byte-identical redelivery — which is what a
/// reconnect produces.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChatIdentity {
    ProviderId(String),
    Fingerprint(String),
}

impl ChatIdentity {
    /// The identity of a message, by the rule stated once and used everywhere.
    ///
    /// A platform id wins when there is one. Otherwise the fingerprint is over
    /// exactly the four facts that make a chat message unique in a room —
    /// never over the sequence number, the collection, or the arrival order,
    /// because all three differ between a first delivery and a redelivery,
    /// which is precisely the case deduplication has to catch.
    #[must_use]
    pub fn of(
        room: &str,
        sender: &str,
        wall_ms: i64,
        text: &str,
        provider_id: Option<&str>,
    ) -> Self {
        match provider_id {
            Some(id) if !id.is_empty() => Self::ProviderId(id.to_string()),
            _ => Self::fingerprint(room, sender, wall_ms, text),
        }
    }

    /// The content fingerprint, for a provider that supplied no id.
    #[must_use]
    pub fn fingerprint(room: &str, sender: &str, wall_ms: i64, text: &str) -> Self {
        let mut hasher = Sha256::new();
        // NUL-separated, so no combination of the parts can be read as a
        // different combination of the parts: ("a\0b", "c") and ("a", "b\0c")
        // must not collide.
        for part in [room, sender, &wall_ms.to_string(), text] {
            hasher.update(part.as_bytes());
            hasher.update([0u8]);
        }
        Self::Fingerprint(format!("{:x}", hasher.finalize()))
    }

    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::ProviderId(id) => format!("id:{id}"),
            Self::Fingerprint(fp) => format!("fp:{fp}"),
        }
    }

    /// Refusing an unknown spelling rather than defaulting: a row whose
    /// identity this build cannot read is a row this build must not place on a
    /// timeline, and a default would place it.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        if let Some(id) = raw.strip_prefix("id:") {
            (!id.is_empty()).then(|| Self::ProviderId(id.to_string()))
        } else {
            raw.strip_prefix("fp:")
                .filter(|f| !f.is_empty())
                .map(|f| Self::Fingerprint(f.to_string()))
        }
    }

    #[must_use]
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::ProviderId(_) => "provider_id",
            Self::Fingerprint(_) => "fingerprint",
        }
    }
}

impl fmt::Display for ChatIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// How a collection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionEnd {
    /// The collector was shut down on purpose.
    Closed,
    /// The socket dropped and the collector gave up on the room.
    Disconnected,
    /// The process is gone. Only ever derived by reaping, never chosen.
    DaemonDown,
}

impl CollectionEnd {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Disconnected => "disconnected",
            Self::DaemonDown => "daemon_down",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "closed" => Some(Self::Closed),
            "disconnected" => Some(Self::Disconnected),
            "daemon_down" => Some(Self::DaemonDown),
            _ => None,
        }
    }

    /// The gap this ending implies, if any. A clean close mid-window means the
    /// rest of the window is uncovered, and `chat_coverage` derives that from
    /// the interval; the reason is here so the two ends cannot disagree about
    /// what a close means.
    #[must_use]
    pub fn gap_reason(self) -> GapReason {
        match self {
            Self::Closed | Self::Disconnected => GapReason::Disconnected,
            Self::DaemonDown => GapReason::DaemonDown,
        }
    }
}

/// One stretch of time the daemon was collecting a room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCollection {
    pub id: ChatCollectionId,
    pub project_id: ProjectId,
    pub capture_session_id: CaptureSessionId,
    pub platform: String,
    pub room: String,
    pub opened_at_wall_ms: i64,
    pub ended_wall_ms: Option<i64>,
    pub end_reason: Option<CollectionEnd>,
}

impl ChatCollection {
    /// Whether this collection is still open. `None` end is the whole restart
    /// mechanism, so it is asked about rather than inferred from a field the
    /// caller set.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.ended_wall_ms.is_none()
    }
}

/// A recorded removal, as it came back off disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatDeletion {
    pub wall_ms: i64,
    pub by: String,
    pub target: Option<ChatIdentity>,
    /// True when the removal named nothing this daemon could identify. Kept as
    /// a field rather than derivable from `target`, because "named a message
    /// we do not hold" and "named nothing" are different facts and a replay
    /// reports them differently.
    pub unresolved: bool,
}

/// A persisted message, placed on the source clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub capture_session_id: CaptureSessionId,
    pub project_id: ProjectId,
    pub collection_id: ChatCollectionId,
    pub seq: u32,
    pub identity: ChatIdentity,
    pub wall_ms: i64,
    pub position: SamplePosition,
    pub sender: String,
    pub text: String,
    pub is_action: bool,
    /// Filled on read by joining the removal record. `None` means no removal
    /// has been seen for this message, which is not the same as a removal that
    /// arrived before the message and is therefore not yet joined — that one is
    /// marked at append time so the two cannot be confused.
    pub deletion: Option<ChatDeletion>,
}

impl ChatMessage {
    /// Place a message on the source clock.
    ///
    /// A message is a point, so like a [`super::stream_event::StreamEvent`] it
    /// has no poll window and its bound is the clock's own — a measured zero on
    /// an exact segment. The untrusted body is bounded, never truncated: a
    /// shortened body reads exactly like a whole one.
    pub fn observe(
        collection: &ChatCollection,
        window: &CaptureWindow,
        clock: &ClockMap,
        raw: &RawChatMessage,
    ) -> Result<Self> {
        if raw.text.len() > MAX_LABEL_BYTES {
            return Err(ObservationError::LabelTooLong {
                bytes: raw.text.len(),
                limit: MAX_LABEL_BYTES,
            });
        }
        if raw.sender.is_empty() || raw.sender.len() > MAX_LABEL_BYTES {
            return Err(ObservationError::LabelTooLong {
                bytes: raw.sender.len(),
                limit: MAX_LABEL_BYTES,
            });
        }
        let position = place(window, clock, raw.wall_ms, 0)?;
        Ok(Self {
            capture_session_id: collection.capture_session_id,
            project_id: collection.project_id,
            collection_id: collection.id,
            seq: 0,
            identity: ChatIdentity::of(
                &raw.room,
                &raw.sender,
                raw.wall_ms,
                &raw.text,
                raw.provider_id.as_deref(),
            ),
            wall_ms: raw.wall_ms,
            position,
            sender: raw.sender.clone(),
            text: raw.text.clone(),
            is_action: raw.is_action,
            deletion: None,
        })
    }

    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.deletion.is_some()
    }
}

/// What one append did, with the input count beside the outcome so the
/// partition can be checked rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatAppend {
    /// Messages newly written.
    pub stored: usize,
    /// Messages the session already held under this identity. Named, not
    /// dropped: a redelivery after a reconnect lands here.
    pub already_present: usize,
}

impl ChatAppend {
    /// Whether every line in a batch of `lines` is accounted for.
    ///
    /// The replacement for a dropped-event counter, and the reason a refusal
    /// can never be silent: the outcome has two fields and a refusal is an
    /// `Err`, so a line is stored, a known duplicate, or an error the caller
    /// must handle. There is no third place for it to go.
    #[must_use]
    pub fn is_accounted_for(self, lines: usize) -> bool {
        self.stored + self.already_present == lines
    }
}

/// A named position on the media timeline that chat is replayed against.
///
/// "Aligned to a capture landmark" is only a claim if the landmark is named, so
/// [`Store::replay_chat`] will not run without one. The usual ones are go-live
/// and the first frame of a resumed capture part, and both are positions on the
/// *media* clock — never on the wall clock, which is the mistake the whole
/// [`ClockMap`] exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landmark {
    pub label: String,
    pub at: MediaTime,
}

impl Landmark {
    #[must_use]
    pub fn new(label: impl Into<String>, at: MediaTime) -> Self {
        Self {
            label: label.into(),
            at,
        }
    }

    /// How far after this landmark a media coordinate sits, or `None` when it
    /// is *before* it — which is a real answer about a replay that starts
    /// mid-stream, and a wrapped positive number would be a lie.
    pub fn offset_of(&self, at: MediaTime) -> Option<MediaTime> {
        MediaTime::from_micros(at.micros().checked_sub(self.at.micros())?).ok()
    }
}

/// One message as a replay hands it back: the record, its position, and its
/// offset from the landmark the caller named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedChat {
    pub message: ChatMessage,
    pub offset_from_landmark: MediaTime,
    pub position: SamplePosition,
}

impl ReplayedChat {
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.message.is_deleted()
    }
}

/// Everything one replay produced, including what it could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatReplay {
    /// The landmark the offsets are measured from. Carried so a reader cannot
    /// see an offset without the name it is relative to.
    pub landmark: Landmark,
    /// In capture order.
    pub messages: Vec<ReplayedChat>,
    /// Removals recorded against this session, in capture order.
    pub deletions: Vec<ChatDeletion>,
    /// Removals naming no message this daemon holds. A count, not a gap: these
    /// are instants, and reporting them as a number is honest because each one
    /// is a durable row a caller can read.
    pub unresolved_deletions: usize,
    /// The recorded acquisition gaps inside the replayed window.
    pub gaps: Vec<CoverageGap>,
    /// The same figure [`Store::chat_coverage`] returns for the same window.
    /// Carried rather than recomputed differently, so a replay cannot report
    /// full coverage while the standalone read does not.
    pub coverage: Coverage,
}

impl ChatReplay {
    /// The messages that are not removed, for a consumer that wants the room as
    /// it currently reads. A convenience over [`Self::messages`], not a
    /// second read: the removed ones are still in `messages` with their text.
    pub fn visible(&self) -> impl Iterator<Item = &ReplayedChat> {
        self.messages.iter().filter(|m| !m.is_deleted())
    }
}

/// Turn a stored position back into a value, refusing a null rather than
/// reading it as the first frame.
pub(super) fn media_of(media_us: i64, table: &'static str) -> Result<MediaTime> {
    MediaTime::from_micros(media_us).map_err(|e: TimeError| ObservationError::UnreadableRow {
        table,
        detail: format!("media_us: {e}"),
    })
}
