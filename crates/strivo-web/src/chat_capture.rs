//! Daemon-owned chat acquisition: the socket, the supervisor, and the store
//! writes behind them.
//!
//! **This is the move the plan asked for.** Until now the *browser* opened the
//! chat socket (`crates/strivo-web/assets/spa/014-pvr.js:266-284`) and held the
//! messages in a JS array, so closing the tab ended collection — which is why
//! `docs/STRATEGY-NVIVO-RIVERSIDE.md:26-30` had to be corrected. Here the
//! daemon opens the socket and the tab only renders what the daemon holds.
//!
//! **Reconnection is where a durable record is usually lost, so it is recorded
//! in two places and neither is a retry counter.** A dropped socket becomes an
//! interval at the moment it is noticed ([`ChatCapture::note_gap`]), and a
//! process that died is found by
//! [`strivo_research::creator::Store::reap_open_collections`], which reads the
//! collection row the dead process left open. So the hole is in the record
//! before the reconnect finishes: a reader asking "what do I have?" mid-outage
//! gets "everything up to here, and nothing from here to here", rather than a
//! series that quietly restarts.
//!
//! **Redelivery is the platform's business, not the archive's.** After a
//! reconnect Twitch may replay the last line it sent before the drop, so the
//! store deduplicates on message identity
//! ([`strivo_research::creator::ChatIdentity`]). The supervisor keeps no set of
//! what it has seen: that would be a second answer to a question about content,
//! and the store already has the authoritative one.
//!
//! **Nothing here schedules itself.** A [`ChatCapture`] is spawned by a caller
//! that knows the room, the capture session and the clock map — which today
//! means a test, because no route and no `serve` wiring owns that yet. Stated
//! rather than papered over: the durable record exists and is tested, but
//! nothing in the running product starts collecting on its own.

use strivo_chat::{classify, WireLine};
use strivo_editor::time::ClockMap;
use strivo_research::creator::{
    CaptureSessionId, ChatCollection, ChatDeletionTarget, ChatReplay, CollectionEnd, CoverageGap,
    GapReason, Landmark, ProjectId, RawChatDeletion, RawChatMessage, Store,
};

/// Where a capture reads its lines from.
///
/// The trait exists so the supervisor's loop — classify, append, record the gap,
/// reconnect — is drivable without a socket. That is not convenience: it is the
/// only way the reconnection path can be tested at all, because a test cannot
/// make Twitch drop a socket on cue. The production implementation is
/// [`TwitchIrc`].
pub trait ChatTransport: Send {
    /// The next line, or `None` when the connection is gone. Reconnection is
    /// the caller's job, so a transport never retries.
    fn next_line(&mut self) -> impl std::future::Future<Output = Option<String>> + Send;
}

/// The live Twitch IRC socket.
pub struct TwitchIrc {
    room: String,
    reader: tokio::io::Lines<tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl TwitchIrc {
    /// Connect and join `room`, requesting the tags the archive needs.
    ///
    /// Anonymous, like the SPA's reader was: a chat archive is a public record,
    /// and the daemon has no reason to spend a credential receiving one.
    /// `CAP REQ` asks for `twitch.tv/tags`, which is where `id` and
    /// `tmi-sent-ts` live — without it every message falls back to a content
    /// fingerprint, a weaker identity rule than the platform offers.
    pub async fn connect(room: &str) -> std::io::Result<Self> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let stream = tokio::net::TcpStream::connect("irc.chat.twitch.tv:6667").await?;
        stream.set_nodelay(true).ok();
        let (read, write) = stream.into_split();
        let mut socket = Self {
            room: room.trim_start_matches('#').to_ascii_lowercase(),
            reader: BufReader::new(read).lines(),
            writer: write,
        };
        let nick = format!("justinfan{}", 10_000 + (epoch_subsec_nanos() % 89_999));
        for line in [
            "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
            format!("NICK {nick}"),
            format!("JOIN #{}", socket.room),
        ] {
            socket.writer.write_all(line.as_bytes()).await?;
            socket.writer.write_all(b"\r\n").await?;
        }
        socket.writer.flush().await?;
        Ok(socket)
    }

    /// Echo a keepalive back. Skipping this is the classic IRC bug: the platform
    /// closes a silent socket, and a supervisor that reconnects forever then
    /// looks like a working one.
    async fn pong(&mut self, token: &str) {
        use tokio::io::AsyncWriteExt;

        let _ = self
            .writer
            .write_all(format!("PONG :{token}\r\n").as_bytes())
            .await;
    }
}

impl ChatTransport for TwitchIrc {
    async fn next_line(&mut self) -> Option<String> {
        let line = self.reader.next_line().await.ok()??;
        if let WireLine::Ping(token) = classify(&line) {
            self.pong(&token).await;
        }
        Some(line)
    }
}

/// A per-process anonymous nick. Not a secret, but two daemons sharing one
/// looks like a single client reconnecting in a loop.
fn epoch_subsec_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_nanos()))
}

/// What one submitted line did: newly stored, and already held. Both numbers,
/// never one derived by subtraction at the call site.
pub type LineOutcome = (usize, usize);

/// Everything needed to file a room's messages: which capture they belong to,
/// the clock that will place them, and the landmark a replay is measured from.
///
/// A struct rather than seven constructor parameters, because the group *is* the
/// concept — "this room, in this capture, against this clock" — and a caller
/// that got the order wrong would file one capture's chat against another's
/// timeline without a type error.
pub struct ChatRoom {
    pub session: CaptureSessionId,
    pub project: ProjectId,
    pub clock: ClockMap,
    pub window: strivo_research::creator::CaptureWindow,
    pub platform: String,
    pub room: String,
    /// Required rather than defaulted, for the same reason
    /// [`strivo_research::creator::Store::replay_chat`] requires one: a replay
    /// aligned to nothing is not a replay.
    pub landmark: Landmark,
}

/// One room's collection, owned by the daemon.
///
/// Borrows its [`Store`] rather than holding an `Arc`: a rusqlite `Connection` is
/// neither `Send` nor `Sync`, so an `Arc<Store>` cannot cross a thread, and a
/// supervisor that later runs on a tokio worker wants to. Borrowing also makes
/// the ownership obvious — a capture writes into a store its caller owns and
/// can still read.
pub struct ChatCapture<'a> {
    store: &'a Store,
    target: ChatRoom,
    collection: Option<ChatCollection>,
}

impl<'a> ChatCapture<'a> {
    /// A capture for one room.
    #[must_use]
    pub fn new(store: &'a Store, target: ChatRoom) -> Self {
        Self {
            store,
            target,
            collection: None,
        }
    }

    /// Open the collection row.
    ///
    /// Written *before* the socket opens, not after: the absence of an end on
    /// this row is what a later process reads to learn that this one died, so a
    /// crash between the two would otherwise leave no witness at all.
    pub fn begin(&mut self, at_wall_ms: i64) -> Result<ChatCollection, String> {
        let collection = self
            .store
            .open_chat_collection(
                self.target.session,
                self.target.project,
                &self.target.platform,
                &self.target.room,
                at_wall_ms,
            )
            .map_err(|e| e.to_string())?;
        self.collection = Some(collection.clone());
        Ok(collection)
    }

    /// Close the collection, and record the tail of the window from `at` on as
    /// uncovered. A stream nobody watched for its last three seconds has a gap
    /// in it, and leaving the tail implicit is how a 40%-covered capture gets
    /// reported as complete.
    pub fn finish(&mut self, at_wall_ms: i64, reason: CollectionEnd) -> Result<(), String> {
        let Some(collection) = self.collection.take() else {
            return Ok(());
        };
        self.store
            .close_chat_collection(&collection, at_wall_ms, reason)
            .map_err(|e| e.to_string())?;
        if at_wall_ms < self.target.window.end_wall_ms {
            let gap = CoverageGap::new(
                at_wall_ms,
                self.target.window.end_wall_ms,
                GapReason::NotConnected,
            )
            .map_err(|e| e.to_string())?;
            self.store
                .record_chat_gap(&collection, &gap)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Record an interval nothing was collected in, from `from_wall_ms` to the
    /// end of the window. The end is the window rather than a second argument
    /// because a caller that only knows "the socket died here" does not know
    /// when the hole closed, and a guess would understate it.
    pub fn note_gap(&self, from_wall_ms: i64, why: GapReason) -> Result<(), String> {
        let gap = CoverageGap::new(from_wall_ms, self.target.window.end_wall_ms, why)
            .map_err(|e| e.to_string())?;
        self.store
            .record_chat_gap(self.open_collection()?, &gap)
            .map_err(|e| e.to_string())
    }

    /// Persist one classified line, or `None` when it carried nothing to store.
    ///
    /// The returned pair is the line's own accounting. A caller that ignores it
    /// is ignoring the only statement of what became of that line — which is why
    /// the type is a pair and not a `()`.
    pub fn submit(
        &mut self,
        line: &str,
        observed_at_ms: i64,
    ) -> Result<Option<LineOutcome>, String> {
        let collection = self.open_collection()?.clone();
        match classify(line) {
            WireLine::Message(message) => {
                let raw = RawChatMessage {
                    // The platform's own instant when it sent one, because that
                    // is when the message happened. The receive instant is the
                    // fallback for a provider that sent none; the two are
                    // different facts, not a precision to choose between.
                    wall_ms: i64::try_from(message.timestamp_ms)
                        .ok()
                        .filter(|ms| *ms > 0)
                        .unwrap_or(observed_at_ms),
                    room: message.room.clone(),
                    sender: message.sender.clone(),
                    text: message.text.clone(),
                    is_action: message.is_action,
                    provider_id: Some(message.id.clone()).filter(|id| !id.is_empty()),
                };
                let outcome = self
                    .store
                    .append_chat(
                        &collection,
                        std::slice::from_ref(&raw),
                        &self.target.window,
                        &self.target.clock,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(Some((outcome.stored, outcome.already_present)))
            }
            WireLine::Removed(removal) => {
                let raw = RawChatDeletion {
                    wall_ms: removal
                        .at_ms
                        .and_then(|ms| i64::try_from(ms).ok())
                        .filter(|ms| *ms > 0)
                        .unwrap_or(observed_at_ms),
                    by: removal.by.clone(),
                    target: ChatDeletionTarget::ProviderId(removal.target.clone()),
                };
                let outcome = self
                    .store
                    .apply_chat_deletions(self.target.session, std::slice::from_ref(&raw))
                    .map_err(|e| e.to_string())?;
                Ok(Some((outcome.marked, outcome.unresolved)))
            }
            WireLine::Ping(_) | WireLine::Ignored => Ok(None),
        }
    }

    /// The record as a replay sees it.
    pub fn replay(&self) -> Result<ChatReplay, String> {
        self.store
            .replay_chat(
                self.target.session,
                &self.target.window,
                &self.target.landmark,
            )
            .map_err(|e| e.to_string())
    }

    /// The room this capture is collecting.
    #[must_use]
    pub fn room(&self) -> &str {
        &self.target.room
    }

    fn open_collection(&self) -> Result<&ChatCollection, String> {
        self.collection
            .as_ref()
            .ok_or_else(|| "no chat collection is open".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strivo_editor::time::MediaTime;
    use strivo_research::creator::{CaptureSession, CaptureWindow, Project};

    const T0: i64 = 1_789_000_000_000;

    /// A transport that hands over a scripted list of lines and then reports the
    /// socket gone, so the supervisor's loop is drivable without a network.
    struct Scripted {
        lines: Vec<String>,
        at: usize,
    }

    impl ChatTransport for Scripted {
        async fn next_line(&mut self) -> Option<String> {
            let line = self.lines.get(self.at).cloned();
            self.at += 1;
            line
        }
    }

    fn window() -> CaptureWindow {
        CaptureWindow::new(T0, T0 + 8_000).expect("forward")
    }

    fn clock() -> ClockMap {
        let mut map = ClockMap::new();
        map.push(strivo_editor::time::ClockSegment::exact(
            T0,
            MediaTime::ZERO,
            MediaTime::from_millis(8_000).expect("8 s"),
        ))
        .expect("one segment");
        map
    }

    fn project_and_session(store: &Store) -> (ProjectId, CaptureSessionId) {
        let project = Project {
            id: ProjectId::generate(),
            name: "stream".into(),
            description: String::new(),
            channel: "twitch".into(),
            series: String::new(),
            settings_json: "{}".into(),
            created_at: "2026-09-07T00:00:00Z".into(),
            updated_at: "2026-09-07T00:00:00Z".into(),
        };
        let project_id = project.id;
        store.insert_project(&project).expect("project");
        let session = CaptureSessionId::generate();
        store
            .insert_capture_session(&CaptureSession {
                id: session,
                project_id,
                source_asset_id: None,
                started_at: "2026-09-07T00:00:00Z".into(),
                ended_at: None,
                finalized: false,
                attributes_json: "{}".into(),
                created_at: "2026-09-07T00:00:00Z".into(),
            })
            .expect("session");
        (project_id, session)
    }

    fn capture_for<'a>(store: &'a Store) -> (ChatCapture<'a>, CaptureSessionId) {
        let (project, session) = project_and_session(store);
        let chat = ChatCapture::new(
            store,
            ChatRoom {
                session,
                project,
                clock: clock(),
                window: window(),
                platform: "twitch".into(),
                room: "streamer".into(),
                landmark: Landmark::new("go live", MediaTime::ZERO),
            },
        );
        (chat, session)
    }

    #[tokio::test]
    async fn the_daemon_stores_chat_with_no_browser_involved() {
        let store = Store::open_in_memory().expect("store");
        let (mut chat, _session) = capture_for(&store);
        chat.begin(T0).expect("begin");

        let mut transport = Scripted {
            lines: vec![
                "@id=m1;tmi-sent-ts=1789000001000 :ann!ann@a.tmi.twitch.tv PRIVMSG #streamer :hi"
                    .into(),
                "PING :tmi.twitch.tv".into(),
                "@id=m2;tmi-sent-ts=1789000002000 :bo!bo@b.tmi.twitch.tv PRIVMSG #streamer :there"
                    .into(),
            ],
            at: 0,
        };
        let mut stored = 0;
        while let Some(line) = transport.next_line().await {
            if let Some((s, _)) = chat.submit(&line, T0).expect("submit") {
                stored += s;
            }
        }

        assert_eq!(stored, 2, "two messages, and the PING stored none");
        let replay = chat.replay().expect("replay");
        assert_eq!(replay.messages.len(), 2);
        assert_eq!(replay.messages[1].offset_from_landmark.micros(), 2_000_000);
    }

    #[tokio::test]
    async fn a_dropped_socket_is_recorded_before_the_reconnect_finishes() {
        let store = Store::open_in_memory().expect("store");
        let (mut chat, session) = capture_for(&store);
        chat.begin(T0).expect("begin");
        chat.submit(
            "@id=m1;tmi-sent-ts=1789000001000 :ann!ann@a.tmi.twitch.tv PRIVMSG #streamer :hi",
            T0,
        )
        .expect("submit");
        // The socket dies and the daemon notices, before it reconnects.
        chat.note_gap(T0 + 1_000, GapReason::Disconnected)
            .expect("gap");

        // Then it reconnects and the platform replays the same line.
        let again = chat
            .submit(
                "@id=m1;tmi-sent-ts=1789000001000 :ann!ann@a.tmi.twitch.tv PRIVMSG #streamer :hi",
                T0 + 3_000,
            )
            .expect("submit")
            .expect("a message");

        assert_eq!(again, (0, 1), "stored zero, already present one");
        let gaps = store.chat_gaps(session).expect("gaps");
        assert_eq!(gaps.len(), 1, "and the hole is in the record");
        assert_eq!(gaps[0].reason, GapReason::Disconnected);
        assert_eq!(
            (gaps[0].start_wall_ms, gaps[0].end_wall_ms),
            (T0 + 1_000, T0 + 8_000),
            "recorded from where the hole opened to the end of the window"
        );
    }

    #[tokio::test]
    async fn a_removal_off_the_wire_is_recorded_rather_than_lost() {
        let store = Store::open_in_memory().expect("store");
        let (mut chat, _session) = capture_for(&store);
        chat.begin(T0).expect("begin");
        chat.submit(
            "@id=m1;tmi-sent-ts=1789000001000 :ann!ann@a.tmi.twitch.tv PRIVMSG #streamer :oops",
            T0,
        )
        .expect("submit");
        let marked = chat
            .submit(
                "@target-msg-id=m1;login=mod;tmi-sent-ts=1789000001500 \
                 :mod!mod@mod.tmi.twitch.tv CLEARMSG #streamer :removed",
                T0,
            )
            .expect("submit")
            .expect("a removal");

        assert_eq!(marked, (1, 0), "it marked the message we held");
        let replay = chat.replay().expect("replay");
        assert!(replay.messages[0].is_deleted());
        assert_eq!(
            replay.messages[0].message.text, "oops",
            "and the text is still here: a removal is recorded, not applied"
        );
    }

    #[tokio::test]
    async fn a_line_submitted_with_no_collection_open_is_refused_rather_than_lost() {
        // The anti-silent-loss direction at this seam: before `begin`, a line
        // has nowhere to go, and the answer must be an error a caller can see —
        // not a `None` that reads exactly like "nothing to store".
        let store = Store::open_in_memory().expect("store");
        let (mut chat, _session) = capture_for(&store);
        let outcome = chat.submit(
            "@id=m1;tmi-sent-ts=1789000001000 :ann!ann@a.tmi.twitch.tv PRIVMSG #streamer :hi",
            T0,
        );
        assert!(outcome.is_err(), "refused, and the refusal is visible");
    }
}
