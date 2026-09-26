//! Daemon-side chat acquisition: what one line off the wire means.
//!
//! **This is the half that used to live in a browser.** The SPA opened
//! `wss://irc-ws.chat.twitch.tv` itself
//! (`crates/strivo-web/assets/spa/014-pvr.js:266-284`) and kept the messages in
//! a JS array, so the record existed only while a tab was open. The same
//! protocol is spoken here, in the daemon, and the classifier is the piece worth
//! testing: a socket reader has three jobs and the middle one is easy to lose.
//!
//! * keep the connection alive — a `PING` must be answered `PONG` or Twitch
//!   drops the socket, and a reader that ignores it reconnects forever;
//! * recognise a message;
//! * recognise a moderator's removal, which is a *different line* and used to
//!   fall out of the parser as `None` and vanish.
//!
//! The last one is why this module exists rather than a `for line in lines`
//! loop over [`parse_twitch_irc`]. `parse_twitch_irc` returns `None` for
//! anything that is not a `PRIVMSG`, which is correct for the SPA's purposes and
//! silently lossy for an archive: a `CLEARMSG` is evidence that a message was
//! removed, and dropping it is a gap with no witness.
//!
//! No IO, no clock, no clock model. A caller supplies the wall instant it
//! observed the line at, because this crate sits below the daemon and cannot
//! reach the canonical [`strivo_editor::time::ClockMap`] that decides what that
//! instant means on a recording's timeline.

use crate::{parse_twitch_irc, ChatMessage};

/// A moderator's removal of one message, as the platform reports it.
///
/// `target` is the platform's own id for the removed message. A platform that
/// removes a message without naming it produces no such line at all, so there is
/// no "unidentified removal" to represent here — the absence of the line *is*
/// the fact, and inventing a placeholder for it would be a second, wrong answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// The id of the removed message, from the `target-msg-id` tag.
    pub target: String,
    /// The login that performed the removal, from the login field.
    pub by: String,
    /// The platform's own timestamp, when it sent one.
    pub at_ms: Option<u64>,
}

/// One classified line. `PartialEq` only, because [`ChatMessage`] is not
/// `Eq` and widening the message type for a test convenience would be a change
/// to the SPA-facing model this todo does not own.
#[derive(Debug, Clone, PartialEq)]
pub enum WireLine {
    /// A keepalive that must be echoed back verbatim after the `PING`, or the
    /// platform closes the socket. The payload is the token to echo.
    Ping(String),
    /// A chat message.
    Message(Box<ChatMessage>),
    /// A moderator removed a message.
    Removed(Box<Removal>),
    /// Anything else: joins, parts, `NOTICE`, `RECONNECT`, `001`. Real, and
    /// deliberately not surfaced — a reader that reported them would have to
    /// decide what a caller does with each, and nothing in an archive needs
    /// them.
    Ignored,
}

/// Classify one raw line as it arrives on a chat socket.
///
/// The wall instant is **not** read here. The platform's own `tmi-sent-ts` is
/// the instant the message happened and travels in
/// [`ChatMessage::timestamp_ms`]; a caller with no better observation supplies
/// its own receive time. Deciding which of those two sits on a recording's
/// timeline is the clock map's job, and this crate cannot reach it.
#[must_use]
pub fn classify(line: &str) -> WireLine {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    // `PING` before anything else: it is the only line a socket reader must
    // answer rather than store, and a tag-prefixed check would miss it.
    if let Some(token) = trimmed.strip_prefix("PING ") {
        return WireLine::Ping(token.trim_start_matches(':').to_string());
    }
    if let Some(removal) = parse_clearmsg(trimmed) {
        return WireLine::Removed(Box::new(removal));
    }
    match parse_twitch_irc(trimmed) {
        Some(message) => WireLine::Message(Box::new(message)),
        None => WireLine::Ignored,
    }
}

/// Parse a `CLEARMSG` line: `@target-msg-id=… :mod!mod@mod.tmi.twitch.tv CLEARMSG #room :…`
///
/// Returns `None` for the line shapes that are not a removal, and for a removal
/// naming nothing — the latter is dropped, which is honest here: a `CLEARMSG`
/// with no `target-msg-id` identifies no message, and recording an event that
/// names nothing would be a row no reader could act on. The *observable*
/// consequence of a removal is that a message is absent, and absence is
/// recorded by the caller's own coverage rather than by an event.
fn parse_clearmsg(line: &str) -> Option<Removal> {
    let (tags, rest) = line.split_once(' ')?;
    let tags = tags.strip_prefix('@')?;
    if !rest.contains(" CLEARMSG ") {
        return None;
    }
    let target = tag_value(tags, "target-msg-id")?;
    if target.is_empty() {
        return None;
    }
    let actor = rest.strip_prefix(':')?.split('!').next()?;
    Some(Removal {
        target: target.to_string(),
        by: actor.to_string(),
        at_ms: tag_value(tags, "tmi-sent-ts").and_then(|v| v.parse().ok()),
    })
}

fn tag_value<'a>(tags: &'a str, key: &str) -> Option<&'a str> {
    tags.split(';').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIVMSG: &str =
        "@badge-info=subscriber/1;id=abc-123;display-name=Ann;tmi-sent-ts=1789000001000;\
        user-type= :ann!ann@ann.tmi.twitch.tv PRIVMSG #streamer :hello there";

    #[test]
    fn a_privmsg_is_a_message_carrying_the_platforms_own_instant() {
        let WireLine::Message(message) = classify(PRIVMSG) else {
            panic!("a PRIVMSG is a message, got {:?}", classify(PRIVMSG));
        };
        assert_eq!(message.id, "abc-123");
        assert_eq!(message.sender, "Ann");
        assert_eq!(message.room, "streamer");
        assert_eq!(message.text, "hello there");
        // The provider's own timestamp, not a receive time: it is when the
        // message happened, and the clock map is what turns it into a position.
        assert_eq!(message.timestamp_ms, 1_789_000_001_000);
    }

    #[test]
    fn a_ping_is_a_ping_and_nothing_else() {
        // The reason this classifier exists as its own function: a reader that
        // loses this reconnects forever, and a test that only checked messages
        // would not notice.
        assert_eq!(
            classify("PING :tmi.twitch.tv"),
            WireLine::Ping("tmi.twitch.tv".into())
        );
        assert_eq!(
            classify("PING tmi.twitch.tv\r\n"),
            WireLine::Ping("tmi.twitch.tv".into())
        );
    }

    #[test]
    fn a_clearmsg_is_a_removal_rather_than_nothing() {
        let line = "@target-msg-id=abc-123;login=mod;tmi-sent-ts=1789000001500 \
                    :mod!mod@mod.tmi.twitch.tv CLEARMSG #streamer :removed";
        let WireLine::Removed(removal) = classify(line) else {
            panic!("a CLEARMSG is a removal, got {:?}", classify(line));
        };
        assert_eq!(removal.target, "abc-123");
        assert_eq!(removal.by, "mod");
        assert_eq!(removal.at_ms, Some(1_789_000_001_500));
    }

    #[test]
    fn a_clearmsg_naming_nothing_is_not_a_removal() {
        // And the negative, because a removal that identified no message would
        // be a row no reader could act on.
        let line = "@login=mod :mod!mod@mod.tmi.twitch.tv CLEARMSG #streamer :removed";
        assert_eq!(classify(line), WireLine::Ignored);
    }

    #[test]
    fn the_control_verbs_are_ignored_rather_than_stored() {
        for line in [
            ":ann!ann@ann.tmi.twitch.tv JOIN #streamer",
            ":ann!ann@ann.tmi.twitch.tv PART #streamer",
            "@msg-id=1 :tmi.twitch.tv NOTICE #streamer :now hosting someone",
            ":justinfan1234!justinfan1234@justinfan1234.tmi.twitch.tv 001 justinfan1234",
        ] {
            assert_eq!(
                classify(line),
                WireLine::Ignored,
                "{line:?} is real traffic and must not become a message"
            );
        }
    }

    #[test]
    fn the_providers_id_is_what_distinguishes_two_identical_messages() {
        // Two people saying the same thing are two messages. The identity rule
        // lives in the research store; this is only the input it depends on, so
        // the assertion is that the id survives the parse intact.
        let first = classify(PRIVMSG);
        let second = classify(&PRIVMSG.replace("abc-123", "def-456"));
        let (WireLine::Message(a), WireLine::Message(b)) = (&first, &second) else {
            panic!("both are messages");
        };
        assert_ne!(a.id, b.id, "so the store can tell them apart");
    }
}
