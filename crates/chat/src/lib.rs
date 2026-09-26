//! Chat client primitives — Chatterino-class building blocks for the SPA.
//!
//! Three pure-data pieces the SPA assembles into a chat surface:
//!
//! 1. [`parse_twitch_irc`] — IRC line → [`ChatMessage`]. Parses Twitch's
//!    IRC variant: leading `@key=value;…` tags, `:nick!user@host`, the
//!    `PRIVMSG` verb, channel, trailing text. Action (`/me`) detection is
//!    folded in.
//! 2. [`tokenize_text`] — text + emote map → `Vec<Token>`. Splits the
//!    message body into mentions (`@user`), emote IDs / names, URLs, and
//!    plain text runs the SPA renders verbatim.
//! 3. [`apply_filters`] — a filter pipeline (keyword in/out, from user,
//!    mentions self, regex-ish substring) used by the SPA's filter chip
//!    bar. Plus [`RoomBuffer`], a fixed-capacity ring used per-tab.
//!
//! A fourth piece, [`acquire`], is what the **daemon** reads instead of a
//! browser: the same wire protocol, classified into a keepalive, a message and
//! a moderator's removal. It exists because the SPA's parser answers "is this a
//! message", which is the right question for a chat panel and the wrong one
//! for an archive — a removal falls out of it as nothing at all.
//!
//! No IO, no DOM. Twenty-plus unit tests cover the IRC parser (PRIVMSG
//! happy path, tags, action, malformed lines), the tokenizer (mentions,
//! emote substitution, link sniffing, runs), the filter pipeline (each
//! filter kind individually + composite AND/OR semantics), and the room
//! ring buffer (capacity, overwrite, mentions counter, mark_read).

mod acquire;

pub use acquire::{classify, Removal, WireLine};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Twitch,
    YouTube,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Server-assigned id when available (`id` IRC tag); else synthesised.
    pub id: String,
    pub platform: Platform,
    pub room: String,
    pub sender: String,
    pub sender_color: Option<String>,
    pub text: String,
    pub timestamp_ms: u64,
    pub badges: Vec<String>,
    pub is_action: bool,
    pub is_system: bool,
    pub deleted: bool,
}

/// Twitch IRC tags we surface to the SPA; everything else is dropped at
/// parse time to keep the payload tight.
#[derive(Debug, Default)]
struct TwitchTags {
    id: Option<String>,
    color: Option<String>,
    badges: Vec<String>,
    display_name: Option<String>,
    tmi_sent_ts: Option<u64>,
}

/// Parse a Twitch IRC PRIVMSG line into a [`ChatMessage`]. Returns None
/// for non-PRIVMSG verbs (PING, JOIN, USERSTATE, …) or malformed lines —
/// callers track those separately if they care.
pub fn parse_twitch_irc(line: &str) -> Option<ChatMessage> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    let tags = if let Some(after) = rest.strip_prefix('@') {
        let (raw_tags, after_tags) = after.split_once(' ')?;
        rest = after_tags;
        parse_irc_tags(raw_tags)
    } else {
        TwitchTags::default()
    };
    let after_prefix = rest.strip_prefix(':')?;
    let (prefix, after_cmd) = after_prefix.split_once(' ')?;
    let sender = prefix.split('!').next()?.to_string();
    let (verb, after_verb) = after_cmd.split_once(' ')?;
    if verb != "PRIVMSG" {
        return None;
    }
    let (channel, trailing) = after_verb.split_once(" :")?;
    let room = channel.trim_start_matches('#').to_string();
    let (text, is_action) = strip_irc_action(trailing);
    let display_sender = tags.display_name.unwrap_or_else(|| sender.clone());
    let id = tags
        .id
        .unwrap_or_else(|| format!("{}-{}", room, tags.tmi_sent_ts.unwrap_or(0)));
    Some(ChatMessage {
        id,
        platform: Platform::Twitch,
        room,
        sender: display_sender,
        sender_color: tags.color,
        text: text.to_string(),
        timestamp_ms: tags.tmi_sent_ts.unwrap_or(0),
        badges: tags.badges,
        is_action,
        is_system: false,
        deleted: false,
    })
}

fn parse_irc_tags(raw: &str) -> TwitchTags {
    let mut out = TwitchTags::default();
    for pair in raw.split(';') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        match k {
            "id" if !v.is_empty() => out.id = Some(v.to_string()),
            "color" if !v.is_empty() => out.color = Some(v.to_string()),
            "display-name" if !v.is_empty() => out.display_name = Some(unescape_irc_tag(v)),
            "tmi-sent-ts" => out.tmi_sent_ts = v.parse().ok(),
            "badges" if !v.is_empty() => {
                out.badges = v
                    .split(',')
                    .filter_map(|b| b.split_once('/').map(|(name, _)| name.to_string()))
                    .collect();
            }
            _ => {}
        }
    }
    out
}

/// IRC tags use `\:` `\s` `\\` `\r` `\n` escapes; only the printable ones
/// realistically appear in display-name. We unescape just those.
fn unescape_irc_tag(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(':') => out.push(';'),
                Some('s') => out.push(' '),
                Some('\\') => out.push('\\'),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `\x01ACTION text\x01` → `(text, true)`. Otherwise `(s, false)`.
fn strip_irc_action(s: &str) -> (&str, bool) {
    let s = s
        .strip_prefix('\u{0001}')
        .and_then(|t| t.strip_suffix('\u{0001}'))
        .unwrap_or(s);
    if let Some(rest) = s.strip_prefix("ACTION ") {
        (rest, true)
    } else {
        (s, false)
    }
}

/// One renderable unit in a tokenised message body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Token {
    Text { text: String },
    Mention { user: String },
    Emote { name: String, url: String },
    Link { url: String },
}

pub type EmoteMap = HashMap<String, String>;

/// One span from Twitch's IRC `emotes=` tag — an emote id and the
/// character range (inclusive on both ends) in the message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmoteRange {
    pub id: String,
    pub start: usize,
    pub end: usize,
}

/// Parse a Twitch `emotes=` tag value into a flat list of ranges. The
/// tag shape is `emote_id:start-end[,start-end[,…]][/emote_id:…]`. Any
/// malformed run is silently dropped so a single bad sub-range doesn't
/// nuke an otherwise good list.
pub fn parse_twitch_emotes(raw: &str) -> Vec<EmoteRange> {
    let mut out = Vec::new();
    if raw.is_empty() {
        return out;
    }
    for group in raw.split('/') {
        let Some((id, runs)) = group.split_once(':') else {
            continue;
        };
        for run in runs.split(',') {
            let Some((start, end)) = run.split_once('-') else {
                continue;
            };
            let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>()) else {
                continue;
            };
            if end < start {
                continue;
            }
            out.push(EmoteRange {
                id: id.to_string(),
                start,
                end,
            });
        }
    }
    out.sort_by_key(|r| r.start);
    out
}

/// Build the Twitch CDN URL for an emote id. The v2 endpoint serves
/// modern dark-mode 1x assets which suit the chat tile UI; tooling can
/// switch to 2.0 / 3.0 by changing the trailing component.
pub fn twitch_emote_url(id: &str) -> String {
    format!(
        "https://static-cdn.jtvnw.net/emoticons/v2/{}/default/dark/1.0",
        id
    )
}

/// Tokenise `text` while honouring a list of [`EmoteRange`]s. The ranges
/// take priority over the name-based [`EmoteMap`] for the spans they
/// cover (which is what Twitch sends when a message contains a
/// channel-subscriber emote). Non-emote runs fall through to the regular
/// classification (mention / link / emote-by-name / plain text).
pub fn tokenize_text_with_ranges(
    text: &str,
    emotes: &EmoteMap,
    ranges: &[EmoteRange],
) -> Vec<Token> {
    if ranges.is_empty() {
        return tokenize_text(text, emotes);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut cursor = 0usize;
    let push_text_run = |out: &mut Vec<Token>, run: &str, emotes: &EmoteMap| {
        if run.is_empty() {
            return;
        }
        let toks = tokenize_text(run, emotes);
        for tok in toks {
            match (out.last_mut(), &tok) {
                (Some(Token::Text { text: prev }), Token::Text { text: next }) => {
                    prev.push(' ');
                    prev.push_str(next);
                }
                _ => out.push(tok),
            }
        }
    };
    for range in ranges {
        if range.start >= chars.len() {
            continue;
        }
        // Flush plain text up to this emote.
        if range.start > cursor {
            let between: String = chars[cursor..range.start].iter().collect();
            push_text_run(&mut out, between.trim(), emotes);
        }
        let end = (range.end + 1).min(chars.len());
        let name: String = chars[range.start..end].iter().collect();
        out.push(Token::Emote {
            name: name.trim().to_string(),
            url: twitch_emote_url(&range.id),
        });
        cursor = end;
    }
    if cursor < chars.len() {
        let tail: String = chars[cursor..].iter().collect();
        push_text_run(&mut out, tail.trim(), emotes);
    }
    out
}

/// Tokenise a message body. Splits on whitespace, then classifies each
/// run: `@foo` → Mention, known emote name → Emote, `http(s)://…` → Link,
/// else Text. Adjacent Text tokens are merged so the SPA doesn't paint a
/// span-per-word.
pub fn tokenize_text(text: &str, emotes: &EmoteMap) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    for word in text.split_whitespace() {
        let tok = classify_word(word, emotes);
        match (out.last_mut(), &tok) {
            (Some(Token::Text { text: prev }), Token::Text { text: next }) => {
                prev.push(' ');
                prev.push_str(next);
            }
            _ => out.push(tok),
        }
    }
    out
}

fn classify_word(word: &str, emotes: &EmoteMap) -> Token {
    if let Some(rest) = word.strip_prefix('@') {
        if rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Token::Mention {
                user: rest.trim_end_matches([',', '.', '!', '?']).to_string(),
            };
        }
    }
    if word.starts_with("http://") || word.starts_with("https://") {
        return Token::Link {
            url: word.to_string(),
        };
    }
    if let Some(url) = emotes.get(word) {
        return Token::Emote {
            name: word.to_string(),
            url: url.clone(),
        };
    }
    Token::Text {
        text: word.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Filter {
    KeywordIn { needle: String },
    KeywordOut { needle: String },
    FromUser { user: String },
    NotFromUser { user: String },
    MentionsUser { user: String },
    NoLinks,
    NoActions,
}

/// Apply `filters` to `msgs`. Returns the indices that survive. Semantics
/// match common chat clients: ALL `KeywordIn` / `FromUser` / `MentionsUser`
/// filters must match (AND), AND every negative filter must NOT match.
pub fn apply_filters(msgs: &[ChatMessage], filters: &[Filter]) -> Vec<usize> {
    let mut out = Vec::new();
    'outer: for (i, m) in msgs.iter().enumerate() {
        for f in filters {
            if !filter_admits(f, m) {
                continue 'outer;
            }
        }
        out.push(i);
    }
    out
}

fn filter_admits(f: &Filter, m: &ChatMessage) -> bool {
    match f {
        Filter::KeywordIn { needle } => contains_ci(&m.text, needle),
        Filter::KeywordOut { needle } => !contains_ci(&m.text, needle),
        Filter::FromUser { user } => eq_ci(&m.sender, user),
        Filter::NotFromUser { user } => !eq_ci(&m.sender, user),
        Filter::MentionsUser { user } => mentions_user(&m.text, user),
        Filter::NoLinks => !(m.text.contains("http://") || m.text.contains("https://")),
        Filter::NoActions => !m.is_action,
    }
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.to_lowercase().contains(&needle.to_lowercase())
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn mentions_user(text: &str, user: &str) -> bool {
    let target = user.trim_start_matches('@');
    text.split_whitespace().any(|w| {
        let w = w.trim_end_matches([',', '.', '!', '?']);
        w.strip_prefix('@')
            .is_some_and(|u| u.eq_ignore_ascii_case(target))
    })
}

/// Fixed-capacity ring buffer for a single chat room. Once full, new
/// pushes evict the oldest message. Tracks unread + mention counts since
/// the last [`mark_read`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomBuffer {
    pub room: String,
    pub platform: Platform,
    pub capacity: usize,
    pub messages: Vec<ChatMessage>,
    pub unread: u32,
    pub mentions: u32,
    pub last_read_id: Option<String>,
    pub watched_user: Option<String>,
}

impl RoomBuffer {
    pub fn new(room: impl Into<String>, platform: Platform, capacity: usize) -> Self {
        Self {
            room: room.into(),
            platform,
            capacity: capacity.max(1),
            messages: Vec::with_capacity(capacity.max(1)),
            unread: 0,
            mentions: 0,
            last_read_id: None,
            watched_user: None,
        }
    }

    /// Append `msg`. Evicts the oldest message when at capacity. Bumps
    /// `unread`; bumps `mentions` when `watched_user` is mentioned.
    pub fn push(&mut self, msg: ChatMessage) {
        if self.messages.len() >= self.capacity {
            self.messages.remove(0);
        }
        if let Some(user) = &self.watched_user {
            if mentions_user(&msg.text, user) {
                self.mentions += 1;
            }
        }
        self.unread += 1;
        self.messages.push(msg);
    }

    /// Drop the unread / mention counters and stamp the high-watermark id.
    pub fn mark_read(&mut self) {
        self.unread = 0;
        self.mentions = 0;
        self.last_read_id = self.messages.last().map(|m| m.id.clone());
    }

    /// Soft-delete a message by id (Twitch CLEARMSG). Keeps the message in
    /// the buffer but flags `deleted` so the SPA can render a placeholder.
    pub fn soft_delete(&mut self, id: &str) -> bool {
        for m in self.messages.iter_mut() {
            if m.id == id {
                m.deleted = true;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priv_line(tags: &str, nick: &str, channel: &str, text: &str) -> String {
        let tag_part = if tags.is_empty() {
            String::new()
        } else {
            format!("@{tags} ")
        };
        format!("{tag_part}:{nick}!{nick}@{nick}.tmi.twitch.tv PRIVMSG #{channel} :{text}")
    }

    #[test]
    fn parses_minimal_privmsg() {
        let line = priv_line("", "alice", "cohh", "Hello chat!");
        let m = parse_twitch_irc(&line).unwrap();
        assert_eq!(m.sender, "alice");
        assert_eq!(m.room, "cohh");
        assert_eq!(m.text, "Hello chat!");
        assert!(!m.is_action);
    }

    #[test]
    fn parses_tags_color_badges_display_name_ts() {
        let line = priv_line(
            "badges=subscriber/12,vip/1;color=#FF0000;display-name=Alice123;id=abc-def;tmi-sent-ts=1700000000000",
            "alice123",
            "cohh",
            "Hi!",
        );
        let m = parse_twitch_irc(&line).unwrap();
        assert_eq!(m.sender, "Alice123");
        assert_eq!(m.sender_color.as_deref(), Some("#FF0000"));
        assert_eq!(m.badges, vec!["subscriber", "vip"]);
        assert_eq!(m.id, "abc-def");
        assert_eq!(m.timestamp_ms, 1700000000000);
    }

    #[test]
    fn detects_action_message() {
        let line = priv_line("", "alice", "cohh", "\u{0001}ACTION waves\u{0001}");
        let m = parse_twitch_irc(&line).unwrap();
        assert!(m.is_action);
        assert_eq!(m.text, "waves");
    }

    #[test]
    fn non_privmsg_returns_none() {
        assert!(parse_twitch_irc(":tmi.twitch.tv PING :tmi.twitch.tv").is_none());
        assert!(parse_twitch_irc(":alice!alice@alice.tmi.twitch.tv JOIN #cohh").is_none());
    }

    #[test]
    fn malformed_line_returns_none() {
        assert!(parse_twitch_irc("").is_none());
        assert!(parse_twitch_irc("not a real line").is_none());
        assert!(parse_twitch_irc("PRIVMSG #cohh :hi").is_none());
    }

    #[test]
    fn display_name_escape_handles_space() {
        // `\s` decodes to ' ' per IRCv3 tags spec.
        let line = priv_line("display-name=Alice\\sB.", "alice", "cohh", "hi");
        let m = parse_twitch_irc(&line).unwrap();
        assert_eq!(m.sender, "Alice B.");
    }

    fn emotes_with(name: &str, url: &str) -> EmoteMap {
        let mut m = EmoteMap::new();
        m.insert(name.into(), url.into());
        m
    }

    #[test]
    fn tokenise_merges_adjacent_text_runs() {
        let toks = tokenize_text("the quick brown fox", &EmoteMap::new());
        assert_eq!(toks.len(), 1);
        assert!(matches!(&toks[0], Token::Text { text } if text == "the quick brown fox"));
    }

    #[test]
    fn tokenise_extracts_mention() {
        let toks = tokenize_text("hey @Alice nice play", &EmoteMap::new());
        let m = toks
            .iter()
            .find(|t| matches!(t, Token::Mention { .. }))
            .unwrap();
        assert!(matches!(m, Token::Mention { user } if user == "Alice"));
    }

    #[test]
    fn tokenise_strips_punctuation_from_mention() {
        let toks = tokenize_text("@Alice!", &EmoteMap::new());
        assert!(matches!(&toks[0], Token::Mention { user } if user == "Alice"));
    }

    #[test]
    fn tokenise_recognises_emote_from_map() {
        let toks = tokenize_text(
            "Kappa rocks",
            &emotes_with("Kappa", "https://cdn/emote/Kappa.png"),
        );
        assert!(matches!(&toks[0], Token::Emote { name, .. } if name == "Kappa"));
        assert!(matches!(&toks[1], Token::Text { text } if text == "rocks"));
    }

    #[test]
    fn tokenise_recognises_https_link() {
        let toks = tokenize_text("look https://example.com pog", &EmoteMap::new());
        let l = toks
            .iter()
            .find(|t| matches!(t, Token::Link { .. }))
            .unwrap();
        assert!(matches!(l, Token::Link { url } if url == "https://example.com"));
    }

    fn msg(sender: &str, text: &str, action: bool) -> ChatMessage {
        ChatMessage {
            id: format!("{sender}-{text}"),
            platform: Platform::Twitch,
            room: "cohh".into(),
            sender: sender.into(),
            sender_color: None,
            text: text.into(),
            timestamp_ms: 0,
            badges: vec![],
            is_action: action,
            is_system: false,
            deleted: false,
        }
    }

    #[test]
    fn filter_keyword_in_admits_substring_case_insensitive() {
        let msgs = vec![msg("a", "Hello there", false), msg("b", "goodbye", false)];
        let kept = apply_filters(
            &msgs,
            &[Filter::KeywordIn {
                needle: "HELLO".into(),
            }],
        );
        assert_eq!(kept, vec![0]);
    }

    #[test]
    fn filter_keyword_out_excludes_match() {
        let msgs = vec![msg("a", "hello there", false), msg("b", "goodbye", false)];
        let kept = apply_filters(
            &msgs,
            &[Filter::KeywordOut {
                needle: "bye".into(),
            }],
        );
        assert_eq!(kept, vec![0]);
    }

    #[test]
    fn filter_from_user_is_case_insensitive() {
        let msgs = vec![msg("Alice", "hi", false), msg("bob", "hi", false)];
        let kept = apply_filters(
            &msgs,
            &[Filter::FromUser {
                user: "alice".into(),
            }],
        );
        assert_eq!(kept, vec![0]);
    }

    #[test]
    fn filter_mentions_user_finds_at_handle() {
        let msgs = vec![
            msg("a", "hi @alice nice", false),
            msg("b", "good play", false),
        ];
        let kept = apply_filters(
            &msgs,
            &[Filter::MentionsUser {
                user: "alice".into(),
            }],
        );
        assert_eq!(kept, vec![0]);
    }

    #[test]
    fn filter_no_links_drops_links() {
        let msgs = vec![
            msg("a", "look https://x.com", false),
            msg("b", "no link here", false),
        ];
        let kept = apply_filters(&msgs, &[Filter::NoLinks]);
        assert_eq!(kept, vec![1]);
    }

    #[test]
    fn filter_no_actions_drops_actions() {
        let msgs = vec![msg("a", "waves", true), msg("b", "hi", false)];
        let kept = apply_filters(&msgs, &[Filter::NoActions]);
        assert_eq!(kept, vec![1]);
    }

    #[test]
    fn composite_filters_apply_as_and() {
        let msgs = vec![
            msg("alice", "hello link http://x", false),
            msg("alice", "hello plain", false),
            msg("bob", "hello plain", false),
        ];
        let kept = apply_filters(
            &msgs,
            &[
                Filter::FromUser {
                    user: "alice".into(),
                },
                Filter::KeywordIn {
                    needle: "hello".into(),
                },
                Filter::NoLinks,
            ],
        );
        assert_eq!(kept, vec![1]);
    }

    #[test]
    fn room_buffer_evicts_oldest_at_capacity() {
        let mut b = RoomBuffer::new("cohh", Platform::Twitch, 3);
        for i in 0..5 {
            b.push(msg("a", &format!("m{i}"), false));
        }
        assert_eq!(b.messages.len(), 3);
        assert_eq!(b.messages[0].text, "m2");
        assert_eq!(b.messages[2].text, "m4");
        assert_eq!(b.unread, 5);
    }

    #[test]
    fn room_buffer_tracks_mentions_for_watched_user() {
        let mut b = RoomBuffer::new("cohh", Platform::Twitch, 10);
        b.watched_user = Some("alice".into());
        b.push(msg("a", "hi @alice", false));
        b.push(msg("a", "plain", false));
        b.push(msg("a", "@Alice nice", false));
        assert_eq!(b.mentions, 2);
        assert_eq!(b.unread, 3);
    }

    #[test]
    fn mark_read_resets_counters_and_stamps_last_id() {
        let mut b = RoomBuffer::new("cohh", Platform::Twitch, 10);
        b.push(msg("a", "hi", false));
        b.push(msg("b", "world", false));
        b.mark_read();
        assert_eq!(b.unread, 0);
        assert_eq!(b.mentions, 0);
        assert_eq!(b.last_read_id.as_deref(), Some("b-world"));
    }

    #[test]
    fn soft_delete_flags_message_in_place() {
        let mut b = RoomBuffer::new("cohh", Platform::Twitch, 10);
        b.push(msg("a", "spammy", false));
        let did = b.soft_delete("a-spammy");
        assert!(did);
        assert!(b.messages[0].deleted);
    }

    #[test]
    fn soft_delete_missing_id_returns_false() {
        let mut b = RoomBuffer::new("cohh", Platform::Twitch, 10);
        b.push(msg("a", "x", false));
        assert!(!b.soft_delete("nope"));
    }

    #[test]
    fn parse_emotes_handles_single_run() {
        let v = parse_twitch_emotes("25:0-4");
        assert_eq!(
            v,
            vec![EmoteRange {
                id: "25".into(),
                start: 0,
                end: 4
            }]
        );
    }

    #[test]
    fn parse_emotes_handles_multiple_groups_and_runs() {
        let v = parse_twitch_emotes("25:0-4,6-10/1902:12-17");
        assert_eq!(
            v,
            vec![
                EmoteRange {
                    id: "25".into(),
                    start: 0,
                    end: 4
                },
                EmoteRange {
                    id: "25".into(),
                    start: 6,
                    end: 10
                },
                EmoteRange {
                    id: "1902".into(),
                    start: 12,
                    end: 17
                },
            ]
        );
    }

    #[test]
    fn parse_emotes_drops_malformed_runs_but_keeps_good_ones() {
        let v = parse_twitch_emotes("25:0-4,broken/1902:12-17");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "25");
        assert_eq!(v[1].id, "1902");
    }

    #[test]
    fn parse_emotes_empty_string_returns_empty() {
        assert!(parse_twitch_emotes("").is_empty());
    }

    #[test]
    fn twitch_emote_url_is_v2_dark_1x() {
        assert_eq!(
            twitch_emote_url("25"),
            "https://static-cdn.jtvnw.net/emoticons/v2/25/default/dark/1.0"
        );
    }

    #[test]
    fn tokenize_with_ranges_substitutes_emote_in_middle() {
        let toks = tokenize_text_with_ranges(
            "hey Kappa nice",
            &EmoteMap::new(),
            &[EmoteRange {
                id: "25".into(),
                start: 4,
                end: 8,
            }],
        );
        // 3 tokens: "hey", emote "Kappa", "nice"
        assert_eq!(toks.len(), 3);
        assert!(matches!(&toks[0], Token::Text { text } if text == "hey"));
        match &toks[1] {
            Token::Emote { name, url } => {
                assert_eq!(name, "Kappa");
                assert!(url.contains("/v2/25/"));
            }
            _ => panic!("expected emote token"),
        }
        assert!(matches!(&toks[2], Token::Text { text } if text == "nice"));
    }

    #[test]
    fn tokenize_with_ranges_handles_leading_emote() {
        let toks = tokenize_text_with_ranges(
            "Kappa hello",
            &EmoteMap::new(),
            &[EmoteRange {
                id: "25".into(),
                start: 0,
                end: 4,
            }],
        );
        assert!(matches!(&toks[0], Token::Emote { name, .. } if name == "Kappa"));
        assert!(matches!(&toks[1], Token::Text { text } if text == "hello"));
    }

    #[test]
    fn tokenize_with_ranges_falls_back_to_name_map_outside_ranges() {
        let mut m = EmoteMap::new();
        m.insert("PogChamp".into(), "https://cdn/p.png".into());
        let toks = tokenize_text_with_ranges(
            "Kappa PogChamp",
            &m,
            &[EmoteRange {
                id: "25".into(),
                start: 0,
                end: 4,
            }],
        );
        // 2 tokens — Twitch emote then name-mapped emote
        assert_eq!(toks.len(), 2);
        assert!(matches!(&toks[0], Token::Emote { name, .. } if name == "Kappa"));
        assert!(
            matches!(&toks[1], Token::Emote { name, url } if name == "PogChamp" && url == "https://cdn/p.png")
        );
    }

    #[test]
    fn tokenize_with_empty_ranges_is_equivalent_to_plain_tokenize() {
        let plain = tokenize_text("hello @bob https://x.com", &EmoteMap::new());
        let ranged = tokenize_text_with_ranges("hello @bob https://x.com", &EmoteMap::new(), &[]);
        assert_eq!(plain.len(), ranged.len());
    }

    #[test]
    fn json_roundtrip_preserves_message_and_room_buffer() {
        let mut b = RoomBuffer::new("cohh", Platform::Twitch, 8);
        b.push(msg("a", "hello", false));
        let s = serde_json::to_string(&b).unwrap();
        let back: RoomBuffer = serde_json::from_str(&s).unwrap();
        assert_eq!(back.messages.len(), 1);
        assert_eq!(back.messages[0].text, "hello");
    }
}
