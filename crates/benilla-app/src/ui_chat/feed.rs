//! The chat add (`0x509dd0`): every chat line a handler, step or verb hands over is built here in
//! its caller's turn, its names read from [`crate::names::NameCache`], held until a missing name's
//! answer lands, and routed by [`super::frames::route`].

use bevy::prelude::*;

use benilla_protocol::messages::{
    channel_notice, ChannelNoticeTail, ChatMessage, MACRO_EXPANDED_TYPES,
};

use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{EmoteLine, EmoteTextCatalog};

use crate::names::{NameCache, NameKey};
use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};

use super::edit::ChannelState;
use super::event::{flag_of_tag, kind_of_wire, ChatEvent, ChatEventKind};
#[cfg(test)]
use super::frames::route;
use super::frames::ChatWindows;
use benilla_ui::script::UiScript;

/// The text-emote sentence tables, read once off the patch chain.
#[derive(Resource)]
pub(crate) struct EmoteTexts(pub(crate) EmoteTextCatalog);

/// The DBC string column: 0, enUS; the reference reads its locale slot.
const LOCALE: usize = 0;

/// Load `EmotesText.dbc` with `EmotesTextData.dbc`; before `AssetSet::Open` it loads nothing.
pub(super) fn load_emote_texts(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_emote_text_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("chat: {} emote sentence tables", cat.len());
            commands.insert_resource(EmoteTexts(cat));
        }
        Err(e) => warn!("chat: emote text catalog failed to load: {e:#}"),
    }
}

/// One chat add (`0x509dd0`) and the client-composed lines that share its path, as a handler or
/// step hands it over; one that waits on a name is held on [`ChatLog`] until the name lands.
pub(crate) enum ChatLine {
    /// A decoded `SMSG_MESSAGECHAT`, addon lines included: a player or pet sender waits on its
    /// name (`0x509f92`), and a player or pet receiver sent without its name on that
    /// (`0x509f98`-`0x509fd2`).
    Wire { msg: ChatMessage },
    /// A channel notice with guids to name (`b_guid` is a kick's or ban's actor, 0 when absent).
    Notice {
        notice: u8,
        channel: String,
        a_guid: u64,
        b_guid: u64,
    },
    /// A `/random` broadcast (`0x6e37af`): composed by the handler when the roller's name is cached,
    /// else by the name answer's callback (`0x6cb990`), the roll its callback's param.
    Roll {
        min: u32,
        max: u32,
        roll: u32,
        guid: u64,
    },
    /// An XP line, its victim's name read in the call (`0x50b20a`).
    XpGain { line: XpLine },
    /// An honor award, its victim's name read in the call (`0x50b7c0`); a non-negative `rank`
    /// becomes a title at the add from the victim's faction template, and a negative one takes the
    /// no-rank form (`0x50b6d0`).
    HonorGain { victim: u64, honor: i32, rank: i32 },
    /// A text emote (`0x504070`): shown at once when its performer's name is cached (`0x503660`),
    /// else queued on the text emotes' own list (`0x504107`) for their callback (`0x503fb0`).
    /// `target` is the wire's name, empty when untargeted, used as sent.
    TextEmote {
        performer: u64,
        text_id: u32,
        target: String,
    },
    /// An area discovery (`SMSG_EXPLORATION_EXPERIENCE`).
    Discovery { area: String, xp: u32 },
    /// One combat-log line, classified at the packet: its unit names read in the call, never held;
    /// an uncached item asks its record with the composer's callback.
    Combat(Box<super::combat::PendingCombat>),
    /// A ready event (client-composed lines; name-carrying notices).
    Event(ChatEvent),
    /// A member's channel flags changing (`MODE_CHANGE`), printed only when the name is cached.
    Mode {
        channel: String,
        guid: u64,
        old_flags: u8,
        new_flags: u8,
    },
}

/// One of this module's callbacks on a name record, run when the record's answer lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NameWait {
    /// The chat add's callback (`0x503c80`) over the held chat lines.
    Chat,
    /// The text emotes' callback (`0x503fb0`) over their own list.
    TextEmote,
    /// A roll's callback (`0x6cb990`), the roll its param (`0x6e3816`).
    Roll { min: u32, max: u32, roll: u32 },
    /// The channel pane's roster callback (`0x4ffe30`), asked by the display list (`0x503bdb`) and
    /// a user-list add (`0x50423b`).
    Roster,
}

impl From<ChatEvent> for ChatLine {
    fn from(event: ChatEvent) -> Self {
        Self::Event(event)
    }
}

impl ChatLine {
    /// A line whose text its caller composes before the add, so a queue holds it as text: the
    /// XP, honor and combat lines, the discovery and the roll (`0x50b1d0`, `0x50b6d0`,
    /// `0x50bd90`, `0x6e3848`, `0x6cb990`).
    fn composed_by_caller(&self) -> bool {
        matches!(
            self,
            ChatLine::XpGain { .. }
                | ChatLine::HonorGain { .. }
                | ChatLine::Combat(_)
                | ChatLine::Discovery { .. }
                | ChatLine::Roll { .. }
        )
    }

    /// Whether the line names `key` as a sender or receiver it waits on (`0x503cc9`-`0x503cf4`).
    fn names_key(&self, key: NameKey) -> bool {
        self.player_guids()
            .into_iter()
            .any(|g| g != 0 && NameKey::of(g, None) == Some(key))
    }

    /// Every name the line waits on is cached (`0x503e19`-`0x503e25`).
    fn names_ready(&self, names: &NameCache) -> bool {
        self.player_guids()
            .into_iter()
            .all(|g| g == 0 || names.peek(g).is_some())
    }

    /// The answered `key`, named by this line, came back denied (`0x503e27`-`0x503e5c`): the line
    /// is dropped.
    fn names_missing(&self, key: NameKey, names: &NameCache) -> bool {
        self.player_guids()
            .into_iter()
            .any(|g| g != 0 && NameKey::of(g, None) == Some(key) && names.chat_denied(g))
    }

    /// The player and pet guids whose names the line waits on, 0 for none.
    fn player_guids(&self) -> [u64; 2] {
        match self {
            ChatLine::Wire { msg } => [
                if sender_waits(msg) {
                    msg.sender_guid
                } else {
                    0
                },
                // A receiver sent without its name, a player or a pet (`0x509f98`-`0x509fd2`).
                if msg.receiver_name.is_none()
                    && (is_player_guid(msg.target_guid) || is_pet_guid(msg.target_guid))
                {
                    msg.target_guid
                } else {
                    0
                },
            ],
            ChatLine::Notice { a_guid, b_guid, .. } => [*a_guid, *b_guid],
            ChatLine::Mode { guid, .. } => [*guid, 0],
            _ => [0, 0],
        }
    }
}

/// Fire `CHAT_MSG_ADDON(prefix, message, distribution, sender)`, not through [`route`]: it is no
/// `ChatTypeInfo` key and takes four arguments, not ten.
#[cfg(test)]
pub(super) fn fire_addon_message(
    script: &mut benilla_ui::script::UiScript,
    prefix: String,
    message: String,
    distribution: String,
    sender: String,
) {
    use benilla_ui::script::ScriptValue::Str;
    script.fire_event(
        "CHAT_MSG_ADDON",
        vec![Str(prefix), Str(message), Str(distribution), Str(sender)],
    );
}

/// A text emote's chat event, or `None` when the sentence is blank, as SIT, STAND and TRAIN are in
/// every locale. `sender` is the performer, seen only by addons as arg2: `TEXT_EMOTE` prints
/// verbatim (`ChatFrame.lua:1395`), and the fire pushes the name record the handler resolved.
pub(super) fn text_emote_event(
    cat: &EmoteTextCatalog,
    text_id: u32,
    line: &EmoteLine,
) -> Option<ChatEvent> {
    Some(ChatEvent {
        kind: Some(ChatEventKind::TextEmote),
        text: cat.compose(text_id, line, LOCALE)?,
        sender: line.performer.to_string(),
        ..Default::default()
    })
}

/// The chat lines that wait: on a name (`0x50ad82`, each answer's callback adds them, `0x503edb`),
/// or, while the UI is outside the world, on its entry (`0x509f48`, flushed at `0x50dfc3`).
#[derive(Resource)]
pub(crate) struct ChatLog {
    /// The held lines in add order, each with its queued mark (`node+0x2c`): set while the UI is
    /// outside the world, cleared by the entry's walk.
    pending: Vec<(ChatLine, bool)>,
    /// The text emotes waiting on their performer's name, in queue order: their own list
    /// (`0x504107`), walked by their own callback (`0x503fb0`).
    emotes: Vec<ChatLine>,
    /// This module's callbacks on the name records still asked, in registration order: the
    /// record runs them head to tail when its answer lands (`0x680d03`-`0x680d2c`, `0x67a2d0`).
    waits: Vec<(NameKey, NameWait)>,
    /// The add queues every line (`[0xac7a38]`): set from the start and at the world exit
    /// (`0x4fb190`), cleared at the UI's world entry (`0x50dfbd`).
    queue_chat: bool,
    /// A channel invite waits on its inviter's name; another is dropped meanwhile (`0x50e79e`).
    invite_waiting: bool,
    /// The guids whose whispers we heard, newest first, 16 deep (`0x4fb710`).
    whisper_sources: Vec<u64>,
    /// The guids our whispers reached, newest first, 128 deep (`0x4fb820`).
    whisper_dests: Vec<u64>,
}

impl Default for ChatLog {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            emotes: Vec::new(),
            waits: Vec::new(),
            queue_chat: true,
            invite_waiting: false,
            whisper_sources: Vec::new(),
            whisper_dests: Vec::new(),
        }
    }
}

/// Move `guid` to the front of `list`, the oldest falling off past `depth`; guid 0 is never entered
/// (`0x4fb71f`, `0x4fb82f`).
fn note_recent(list: &mut Vec<u64>, guid: u64, depth: usize) {
    if guid == 0 {
        return;
    }
    match list.iter().position(|&g| g == guid) {
        Some(i) => {
            list.remove(i);
        }
        None => list.truncate(depth - 1),
    }
    list.insert(0, guid);
}

impl ChatLog {
    /// A whisper's sender enters the source history (`0x4fb710`); both histories start empty with
    /// the game (`0x50f11b`, `0x50f143`), here with the session's chat state.
    pub(crate) fn record_whisper_source(&mut self, guid: u64) {
        note_recent(&mut self.whisper_sources, guid, 16);
    }

    /// A whisper echo's receiver enters the destination history (`0x4fb820`).
    pub(crate) fn record_whisper_dest(&mut self, guid: u64) {
        note_recent(&mut self.whisper_dests, guid, 128);
    }

    /// Whether our whispers reached `guid` lately: the destination history alone (`0x4fb7e0`).
    pub(crate) fn whispered(&self, guid: u64) -> bool {
        guid != 0 && self.whisper_dests.contains(&guid)
    }

    /// The source history, newest first.
    #[cfg(test)]
    pub(crate) fn whisper_sources(&self) -> &[u64] {
        &self.whisper_sources
    }

    /// The UI's world exit (`0x528cc9` -> `0x4fb190`): the add queues every line until the next
    /// entry.
    pub(crate) fn leave_world(&mut self) {
        self.queue_chat = true;
    }

    /// Whether the add queues every line (`[0xac7a38]`).
    #[cfg(test)]
    pub(crate) fn queues(&self) -> bool {
        self.queue_chat
    }

    /// Drop every held line on disconnect.
    pub(crate) fn clear_session(&mut self) {
        self.pending.clear();
        self.emotes.clear();
        self.waits.clear();
    }

    /// Register `wait` on `key`'s record, as a lookup that misses does (`0x67d880`): appended, so
    /// the record runs it after those before it. A second chat callback on the same record is not
    /// kept: the first already added every line naming it, so the second finds none.
    fn register(&mut self, key: NameKey, wait: NameWait) {
        if wait == NameWait::Chat && self.waits.contains(&(key, NameWait::Chat)) {
            return;
        }
        self.waits.push((key, wait));
    }

    /// The held addon lines as `(prefix, message, distribution)`.
    #[cfg(test)]
    pub(crate) fn pending_addons(&self) -> Vec<(String, String, String)> {
        self.pending
            .iter()
            .filter_map(|(p, _)| match p {
                ChatLine::Wire { msg } if msg.is_addon() => Some(addon_split(msg)),
                _ => None,
            })
            .collect()
    }

    /// How many held lines will render in a chat window; addon lines never do.
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending
            .iter()
            .filter(|(p, _)| !matches!(p, ChatLine::Wire { msg } if msg.is_addon()))
            .count()
    }

    /// Every held line, addon lines and text emotes included.
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len() + self.emotes.len()
    }

    /// The kind and text of every held event line, in order.
    #[cfg(test)]
    pub(crate) fn pending_events(&self) -> Vec<(Option<super::ChatEventKind>, String)> {
        self.pending
            .iter()
            .filter_map(|(p, _)| match p {
                ChatLine::Event(e) => Some((e.kind, e.text.clone())),
                _ => None,
            })
            .collect()
    }

    /// The text of every held composed line, in order.
    #[cfg(test)]
    pub(crate) fn pending_lines(&self) -> Vec<String> {
        self.pending_events().into_iter().map(|(_, t)| t).collect()
    }
}

/// Where a handler or step hands its chat lines: each is added in the caller's turn, in order with
/// the events around it (`0x509dd0` runs inside its caller).
pub(crate) trait ChatSink {
    /// Add `line` in turn.
    fn push_line(&mut self, line: ChatLine);

    /// Add a decoded wire line (`SMSG_MESSAGECHAT`).
    fn push_wire(&mut self, msg: ChatMessage) {
        self.push_line(ChatLine::Wire { msg });
    }

    /// Add a client-composed event: loot, quest and system lines.
    fn push_event(&mut self, event: ChatEvent) {
        self.push_line(ChatLine::Event(event));
    }

    /// Add one combat-family line, classified at the packet by [`super::combat`].
    fn push_combat(&mut self, line: super::combat::PendingCombat) {
        self.push_line(ChatLine::Combat(Box::new(line)));
    }

    /// Add a decoded `SMSG_CHANNEL_NOTIFY`'s lines; a guid tail waits for its names.
    fn push_channel_notice(&mut self, notice_byte: u8, channel: String, tail: &ChannelNoticeTail) {
        for line in channel_notice_lines(notice_byte, channel, tail) {
            self.push_line(line);
        }
    }

    /// Add an inbound addon line, which signals once its sender is named ([`addon_line`]).
    #[cfg(test)]
    fn push_addon(&mut self, text: &str, chat_type: u8, guid: u64) {
        self.push_line(addon_line(text, chat_type, guid));
    }

    /// Add a `/random` broadcast (`MSG_RANDOM_ROLL`), which waits on the roller's name.
    fn push_roll(&mut self, min: u32, max: u32, roll: u32, guid: u64) {
        self.push_line(ChatLine::Roll {
            min,
            max,
            roll,
            guid,
        });
    }

    /// Add an XP line (`SMSG_LOG_XPGAIN`).
    fn push_xp_gain(&mut self, line: XpLine) {
        self.push_line(ChatLine::XpGain { line });
    }

    /// Add an honor award's line (`SMSG_PVP_CREDIT`); victim 0 is a bonus or objective award.
    fn push_pvp_credit(&mut self, honor: i32, victim: u64, rank: i32) {
        self.push_line(ChatLine::HonorGain {
            victim,
            honor,
            rank,
        });
    }

    /// Add a text emote (`SMSG_TEXT_EMOTE`), which waits on its performer's name.
    fn push_text_emote(&mut self, performer: u64, text_id: u32, target: String) {
        self.push_line(ChatLine::TextEmote {
            performer,
            text_id,
            target,
        });
    }

    /// Add an area discovery: the `ERR_ZONE_EXPLORED` toast on every packet, never chat, and the
    /// `ERR_ZONE_EXPLORED_XP` system line when `xp > 0`.
    fn push_exploration(&mut self, area_name: &str, xp: u32) {
        self.push_line(ChatLine::Discovery {
            area: area_name.to_string(),
            xp,
        });
    }
}

impl ChatSink for crate::ui_script::live::UiEvents {
    fn push_line(&mut self, line: ChatLine) {
        self.chat(line);
    }
}

/// A system that raises in turn adds its lines behind the raises it made so far.
impl ChatSink for crate::ui_script::VmTurn<'_> {
    fn push_line(&mut self, line: ChatLine) {
        match self.queue() {
            Some(queue) => queue.chat(line),
            None => debug!("chat: no handler queue — line dropped"),
        }
    }
}

/// An inbound addon line on `chat_type` (the shared numbering) from `guid`, as the decoder hands
/// it over.
#[cfg(test)]
pub(crate) fn addon_line(text: &str, chat_type: u8, guid: u64) -> ChatLine {
    ChatLine::Wire {
        msg: ChatMessage {
            chat_type,
            language: benilla_protocol::messages::LANGUAGE_ADDON,
            sender_guid: guid,
            target_guid: 0,
            sender_name: None,
            receiver_name: None,
            channel: None,
            text: text.to_string(),
            chat_tag: 0,
            achievement: 0,
            wire_type: 0,
            sender_word: 0,
            gm: false,
        },
    }
}

/// An addon line's prefix, message and distribution: the text splits on its first tab, and with
/// none it is all prefix (`0x50a012`-`0x50a02a`); the distribution is the type's
/// ([`addon_distribution`]).
fn addon_split(msg: &ChatMessage) -> (String, String, String) {
    const TAB: char = '\u{9}';
    let text = msg.text.as_str();
    let (prefix, message) = match text.find(TAB) {
        Some(i) => (text[..i].to_string(), text[i + 1..].to_string()),
        None => (text.to_string(), String::new()),
    };
    (
        prefix,
        message,
        addon_distribution(msg.chat_type).to_string(),
    )
}

/// `CHAT_MSG_ADDON`'s distribution off the line's type, through the add's jump table
/// (`0x50a09e`, `0x50a0a5`): the whisper types (a foreign whisper arrives as one) and Battle.net
/// read `"WHISPER"`, a type outside the table `"UNKNOWN"`.
fn addon_distribution(chat_type: u8) -> &'static str {
    use benilla_protocol::messages as m;
    match chat_type {
        m::CHAT_MSG_PARTY => "PARTY",
        m::CHAT_MSG_RAID => "RAID",
        m::CHAT_MSG_GUILD => "GUILD",
        m::CHAT_MSG_WHISPER | m::CHAT_MSG_BATTLENET => "WHISPER",
        m::CHAT_MSG_BATTLEGROUND => "BATTLEGROUND",
        _ => "UNKNOWN",
    }
}

/// The addon lines among `lines` as `(prefix, message, distribution)`.
#[cfg(test)]
pub(crate) fn addon_parts(lines: Vec<ChatLine>) -> Vec<(String, String, String)> {
    lines
        .into_iter()
        .filter_map(|p| match p {
            ChatLine::Wire { msg } if msg.is_addon() => Some(addon_split(&msg)),
            _ => None,
        })
        .collect()
}

/// The lines one `SMSG_CHANNEL_NOTIFY` adds: a guid tail waits for its names, a join's trailing
/// text is a second notice line (`0x50eae3`).
fn channel_notice_lines(
    notice_byte: u8,
    channel: String,
    tail: &ChannelNoticeTail,
) -> Vec<ChatLine> {
    // The voice flag's set prints nothing (`0x50ea00`); its record write is the handler's.
    if notice_byte == channel_notice::VOICE_ENABLED {
        return Vec::new();
    }
    let (a_guid, b_guid, name) = match tail {
        ChannelNoticeTail::Guid(g) | ChannelNoticeTail::Actor(g) => (*g, 0, None),
        ChannelNoticeTail::Actors { target, source } => (*target, *source, None),
        ChannelNoticeTail::Name(n) => (0, 0, Some(n.clone())),
        ChannelNoticeTail::YouJoined { .. }
        | ChannelNoticeTail::YouLeft { .. }
        | ChannelNoticeTail::Empty => (0, 0, None),
        ChannelNoticeTail::ModeChange {
            guid,
            old_flags,
            new_flags,
        } => {
            return vec![ChatLine::Mode {
                channel,
                guid: *guid,
                old_flags: *old_flags,
                new_flags: *new_flags,
            }];
        }
    };
    if a_guid != 0 {
        return vec![ChatLine::Notice {
            notice: notice_byte,
            channel,
            a_guid,
            b_guid,
        }];
    }
    let mut lines = Vec::new();
    let second = match tail {
        ChannelNoticeTail::YouJoined {
            text: Some(text), ..
        } if !text.is_empty() => Some(text.clone()),
        _ => None,
    };
    let wire_channel = channel.clone();
    if let Some(mut event) = notice_event(notice_byte, channel, name, None) {
        event.left_constant = matches!(tail, ChannelNoticeTail::YouLeft { constant: true });
        if let ChannelNoticeTail::YouJoined {
            flags, instance, ..
        } = tail
        {
            event.joined_flags = *flags as u8;
            event.joined_instance = *instance;
        }
        lines.push(ChatLine::Event(event));
    }
    if let Some(text) = second {
        lines.push(ChatLine::Event(ChatEvent {
            kind: Some(ChatEventKind::ChannelNotice),
            text,
            channel: wire_channel,
            ..Default::default()
        }));
    }
    lines
}

/// The VM's globals as a string lookup, borrowed per composition so `route` can take the VM.
fn globals(script: &benilla_ui::script::UiScript) -> impl Fn(&str) -> Option<String> + '_ {
    |key: &str| script.lua().globals().get::<String>(key).ok()
}

/// Fill a key's template; a missing or empty string prints no line, as in the reference.
fn keyed(
    get: &dyn Fn(&str) -> Option<String>,
    key: &str,
    args: &[benilla_ui::strings::Arg<'_>],
) -> Option<String> {
    let text = benilla_ui::strings::fill(&get(key)?, args);
    (!text.is_empty()).then_some(text)
}

/// The XP line `SMSG_LOG_XPGAIN` prints, its form chosen at the packet (`0x50c720`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum XpLine {
    /// Our own player as the victim: the loss line with the total, a negative signed amount on
    /// the wire, negated (`0x50b660`).
    Loss { total: u32 },
    /// Refer-a-Friend quest XP: the total, two thirds of it as the bonus, and rest state 6's name
    /// (`0x50b56d`-`0x50b633`).
    ReferAFriendQuest { total: u32, state: String },
    /// `0x50b1d0`: named for a held victim unit (0 unnamed); `_GROUP`/`_RAID` from the rate.
    Plain {
        victim: u64,
        total: u32,
        group_rate: f32,
    },
    /// A kill under a rest state whose factor is neither 0 nor 1 (`0x50b3fb`): the state's key and
    /// name, and the signed bonus over the base.
    Rested {
        victim: u64,
        key: &'static str,
        state: String,
        total: u32,
        base: u32,
        group_rate: f32,
    },
}

impl XpLine {
    /// The victim the line names; 0 for none.
    fn victim(&self) -> u64 {
        match self {
            XpLine::Plain { victim, .. } | XpLine::Rested { victim, .. } => *victim,
            XpLine::Loss { .. } | XpLine::ReferAFriendQuest { .. } => 0,
        }
    }
}

/// `amount` over the group rate, truncated as the reference's float-to-int (`0x88b9c0`).
fn over_rate(amount: i64, group_rate: f32) -> i64 {
    (amount as f64 / f64::from(group_rate)) as i64
}

/// A signed amount as `"%s%d"`, with a `+` for zero and up (`0x50b457`-`0x50b474`).
fn signed(amount: i64) -> String {
    if amount >= 0 {
        format!("+{amount}")
    } else {
        amount.to_string()
    }
}

/// The `_GROUP` suffix when the rate raised the award, `_RAID` when it cut it, none when equal
/// (`0x50b21e`, `0x50b416`).
fn rate_suffix(amount: i64, scaled: i64) -> &'static str {
    match scaled.cmp(&amount) {
        std::cmp::Ordering::Equal => "",
        std::cmp::Ordering::Less => "_GROUP",
        std::cmp::Ordering::Greater => "_RAID",
    }
}

/// The text of one [`XpLine`]; `victim` is the named forms' resolved name.
pub(super) fn xp_gain_line(
    line: &XpLine,
    victim: Option<&str>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::Arg;
    match line {
        XpLine::Loss { total } => keyed(
            get,
            "COMBATLOG_XPLOSS_FIRSTPERSON_UNNAMED",
            &[Arg::D(-i64::from(*total as i32))],
        ),
        XpLine::ReferAFriendQuest { total, state } => {
            let bonus = (*total as f32 * 0.666_666_7).round_ties_even() as i64;
            keyed(
                get,
                "COMBATLOG_XPGAIN_QUEST",
                &[
                    Arg::D(i64::from(*total)),
                    Arg::S(&signed(bonus)),
                    Arg::S(state),
                    Arg::D(bonus),
                ],
            )
        }
        XpLine::Plain {
            total, group_rate, ..
        } => {
            let total = i64::from(*total);
            let scaled = over_rate(total, *group_rate);
            let diff = Arg::D((total - scaled).abs());
            let suffix = rate_suffix(total, scaled);
            match victim {
                Some(name) if suffix.is_empty() => keyed(
                    get,
                    "COMBATLOG_XPGAIN_FIRSTPERSON",
                    &[Arg::S(name), Arg::D(total)],
                ),
                Some(name) => keyed(
                    get,
                    &format!("COMBATLOG_XPGAIN_FIRSTPERSON{suffix}"),
                    &[Arg::S(name), Arg::D(total), diff],
                ),
                None if suffix.is_empty() => keyed(
                    get,
                    "COMBATLOG_XPGAIN_FIRSTPERSON_UNNAMED",
                    &[Arg::D(total)],
                ),
                None => keyed(
                    get,
                    &format!("COMBATLOG_XPGAIN_FIRSTPERSON_UNNAMED{suffix}"),
                    &[Arg::D(total), diff],
                ),
            }
        }
        XpLine::Rested {
            key,
            state,
            total,
            base,
            group_rate,
            ..
        } => {
            let name = victim.unwrap_or_default();
            let (total, base) = (i64::from(*total), i64::from(*base));
            let scaled = over_rate(base, *group_rate);
            let bonus = signed(total - base);
            let suffix = rate_suffix(base, scaled);
            let mut args = vec![Arg::S(name), Arg::D(total), Arg::S(&bonus), Arg::S(state)];
            if !suffix.is_empty() {
                args.push(Arg::D((base - scaled).abs()));
            }
            keyed(get, &format!("{key}{suffix}"), &args)
        }
    }
}

/// The rest-state keys by state (`0xac7fb4`): 0 has none, 3 is the literal `"NULL"`, and 6, the
/// Refer-a-Friend state, shares 1's.
pub(crate) const REST_STATE_KEYS: [Option<&str>; 7] = [
    None,
    Some("COMBATLOG_XPGAIN_EXHAUSTION1"),
    Some("COMBATLOG_XPGAIN_EXHAUSTION2"),
    Some("NULL"),
    Some("COMBATLOG_XPGAIN_EXHAUSTION4"),
    Some("COMBATLOG_XPGAIN_EXHAUSTION5"),
    Some("COMBATLOG_XPGAIN_EXHAUSTION1"),
];

/// The honor line (`0x50b6d0`): `COMBATLOG_HONORAWARD` with no victim; a kill under 1 honor prints
/// nothing (`0x50b71f`); else `COMBATLOG_HONORGAIN` with the rank title, or
/// `COMBATLOG_HONORGAIN_NO_RANK` without one.
pub(super) fn honor_gain_line(
    victim: Option<&str>,
    rank_title: Option<&str>,
    honor: i32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::Arg;
    match (victim, rank_title) {
        (None, _) => keyed(get, "COMBATLOG_HONORAWARD", &[Arg::D(i64::from(honor))]),
        (Some(_), _) if honor < 1 => None,
        (Some(name), None) => keyed(
            get,
            "COMBATLOG_HONORGAIN_NO_RANK",
            &[Arg::S(name), Arg::D(i64::from(honor))],
        ),
        (Some(name), Some(rank_title)) => keyed(
            get,
            "COMBATLOG_HONORGAIN",
            &[Arg::S(name), Arg::S(rank_title), Arg::D(i64::from(honor))],
        ),
    }
}

/// The `ERR_ZONE_EXPLORED` toast (error route 1, the red error message), every packet.
pub(super) fn exploration_toast(
    area_name: &str,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    keyed(
        get,
        "ERR_ZONE_EXPLORED",
        &[benilla_ui::strings::Arg::S(area_name)],
    )
}

/// The `ERR_ZONE_EXPLORED_XP` system line, only when the packet carries XP.
pub(super) fn exploration_line(
    area_name: &str,
    xp: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use benilla_ui::strings::Arg;
    keyed(
        get,
        "ERR_ZONE_EXPLORED_XP",
        &[Arg::S(area_name), Arg::D(i64::from(xp))],
    )
}

/// A member's mode change as its notice tokens, in the reference's order: moderator `0x02`
/// (`0x50d05f`, `0x50d089`), voice `0x04` (`0x50d0be`, `0x50d0e8`), speak `0x10` (`0x50d11e`,
/// `0x50d152`).
pub(super) fn mode_change_tokens(old: u8, new: u8) -> Vec<&'static str> {
    [
        (0x02, "SET_MODERATOR", "UNSET_MODERATOR"),
        (0x04, "SET_VOICE", "UNSET_VOICE"),
        (0x10, "SET_SPEAK", "UNSET_SPEAK"),
    ]
    .into_iter()
    .filter_map(|(bit, set, unset)| match (old & bit != 0, new & bit != 0) {
        (false, true) => Some(set),
        (true, false) => Some(unset),
        _ => None,
    })
    .collect()
}

/// The event a channel notice becomes; [`super::frames::compose_notice`] reads its `notice`.
pub(super) fn notice_event(
    notice_byte: u8,
    channel: String,
    a: Option<String>,
    b: Option<String>,
) -> Option<ChatEvent> {
    // Each arm's `mov edi` in the jump table: MODE_CHANGE fires nothing, a byte past THROTTLED
    // fires SYSTEM, the rest CHANNEL_NOTICE (0x12) or CHANNEL_NOTICE_USER (0x13).
    use channel_notice as n;
    let kind = match notice_byte {
        n::JOINED => ChatEventKind::ChannelJoin,
        n::LEFT => ChatEventKind::ChannelLeave,
        n::MODE_CHANGE => return None,
        n::YOU_JOINED
        | n::YOU_LEFT
        | n::WRONG_PASSWORD
        | n::NOT_MEMBER
        | n::NOT_MODERATOR
        | n::NOT_OWNER
        | n::MUTED
        | n::BANNED
        | n::THROTTLED
        // 3.3.5a's two refusals fire the same event (`esi = 0x15`).
        | n::NOT_IN_AREA
        | n::NOT_IN_LFG => ChatEventKind::ChannelNotice,
        n::PASSWORD_CHANGED
        | n::OWNER_CHANGED
        | n::PLAYER_NOT_FOUND
        | n::CHANNEL_OWNER
        | n::ANNOUNCEMENTS_ON
        | n::ANNOUNCEMENTS_OFF
        | n::MODERATION_ON
        | n::MODERATION_OFF
        | n::PLAYER_KICKED
        | n::PLAYER_BANNED
        | n::PLAYER_UNBANNED
        | n::PLAYER_NOT_BANNED
        | n::PLAYER_ALREADY_MEMBER
        | n::INVITE
        | n::INVITE_WRONG_FACTION
        | n::WRONG_FACTION
        | n::INVALID_NAME
        | n::NOT_MODERATED
        | n::PLAYER_INVITED
        | n::PLAYER_INVITE_BANNED
        // 3.3.5a's voice toggles name their actor (`esi = 0x16`).
        | n::VOICE_ON
        | n::VOICE_OFF => ChatEventKind::ChannelNoticeUser,
        _ => ChatEventKind::System,
    };
    Some(ChatEvent {
        kind: Some(kind),
        sender: a.unwrap_or_default(),
        target: b.unwrap_or_default(),
        channel,
        notice: notice_byte.to_string(),
        ..Default::default()
    })
}

/// Deliver one built event at the channel notice handler's steps (`0x50e120`): a `YOU_JOINED`
/// confirms its record and a `YOU_LEFT` marks it left, each relisting, before the line; a
/// non-suspended leave or a refusal then drops the record after the line, relisting again.
#[cfg(test)]
pub(super) fn deliver(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    channels: &mut ChannelState,
    event: &mut ChatEvent,
) {
    let (password, relist) = deliver_before(channels, event);
    if let Some(relist) = relist {
        if relist == Relist::Left {
            script.channel_left(&event.channel);
        }
        script.channels_updated();
    }
    if password {
        script.fire_event("CHANNEL_PASSWORD_REQUEST", password_request_args(event));
    }
    let after = deliver_mid(channels, event);
    route(script, windows, event);
    if deliver_after(channels, after) {
        script.channels_updated();
    }
}

/// A `WRONG_PASSWORD` notice's `CHANNEL_PASSWORD_REQUEST` argument, ahead of the line with the
/// wire name (`0x50e3aa`).
fn password_request_args(event: &ChatEvent) -> Vec<benilla_ui::script::ScriptValue> {
    vec![benilla_ui::script::ScriptValue::Str(event.channel.clone())]
}

/// Which held-record write ahead of the line relists the channels (`0x505d40`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Relist {
    /// The join's confirm (`0x507880`).
    Joined,
    /// The leave (`0x507900`), which unselects a selected channel of the record's name first.
    Left,
}

/// [`deliver`]'s writes ahead of the line: whether a password request fires before it, and which
/// of the join or the leave relists.
fn deliver_before(channels: &mut ChannelState, event: &mut ChatEvent) -> (bool, Option<Relist>) {
    let notice = event
        .notice_byte()
        .filter(|_| event.kind == Some(ChatEventKind::ChannelNotice));
    // A server join of a new zone name renames the slot of its ChannelID (`0x507880`).
    if notice == Some(channel_notice::YOU_JOINED) {
        if let Some(old) = channels.zone_slot_held_as(&event.channel) {
            if let Some(n) = channels.rename_slot(&old, &event.channel) {
                debug!("chat: server moved slot {n} {old:?} → {:?}", event.channel);
            }
        }
    }
    // Read first: the reference picks the token from `slot+0x9c` before writing it, and the renamed
    // and suspended tokens keep the window's registration.
    event.slot_state = channels.slot_state(&event.channel);
    // A leave keeps its record when the server says suspended and we did not ask (`0x50e31b`,
    // `0x50e324`).
    event.suspended = notice == Some(channel_notice::YOU_LEFT)
        && event.left_constant
        && channels.leave_asked(&event.channel) == Some(false);
    let password = notice == Some(channel_notice::WRONG_PASSWORD);
    if let Some(byte) = notice {
        // The server's half of every join and leave, for a log to tell a request from its answer.
        debug!(
            "chat: channel notice {byte:#04x} for {:?} (slot {:?}, {:?})",
            event.channel,
            channels.number_of(&event.channel),
            event.slot_state
        );
    }
    let held = channels.number_of(&event.channel).is_some();
    // With no record, neither the join nor the leave writes or relists (`0x5078a4`, `0x507926`).
    if !held {
        return (password, None);
    }
    let relist = match notice {
        Some(channel_notice::YOU_JOINED) => {
            // State 0 (`0x5078b3`), the zone bit (`0x5078d2`), the second word and the flags
            // (`0x5078e5`, `0x5078eb`), then the relist (`0x5078f3`).
            channels.confirm_slot(&event.channel);
            channels.note_zone_channel_joined(&event.channel);
            channels.note_custom_channel_joined(&event.channel);
            channels.set_slot_join(&event.channel, event.joined_instance, event.joined_flags);
            Some(Relist::Joined)
        }
        Some(channel_notice::YOU_LEFT) => {
            // The record leaves state 0 (`0x50792c`), so the verbs stop reading it as joined,
            // then the unselect (`0x5079b3`) and the relist (`0x507a18`), all ahead of the line.
            channels.suspend_slot(&event.channel);
            Some(Relist::Left)
        }
        _ => None,
    };
    (password, relist)
}

/// The wire names of the records the handler drops after the line, read before the stamp numbers
/// it.
struct AfterLine {
    dropped: Option<String>,
}

/// [`deliver`]'s reads just before the line: the record the handler drops after it, then the stamp.
fn deliver_mid(channels: &mut ChannelState, event: &mut ChatEvent) -> AfterLine {
    use channel_notice as n;
    let notice = event
        .notice_byte()
        .filter(|_| event.kind == Some(ChatEventKind::ChannelNotice));
    // The handler's drop flag (`[ebp-0x14]`): a held leave not suspended (`0x50e358`), a wrong
    // password (`0x50e3a3`), a ban (`0x50e6df`), a wrong faction (`0x50e85e`) and the two area
    // refusals (`0x50e931`, `0x50e958`). The wire name is kept before the stamp decorates arg4.
    let drops = match notice {
        Some(n::YOU_LEFT) => channels.number_of(&event.channel).is_some() && !event.suspended,
        Some(n::WRONG_PASSWORD | n::BANNED | n::WRONG_FACTION | n::NOT_IN_AREA | n::NOT_IN_LFG) => {
            true
        }
        _ => false,
    };
    let dropped = drops.then(|| event.channel.clone());
    // The channel renders numbered when its slot is known.
    channels.stamp_channel(event);
    AfterLine { dropped }
}

/// [`deliver`]'s write after the line: the flagged record dropped (`0x50eaa5` -> `0x507a30`), which
/// relists whether a record was found or not (`0x507a78`). Cleared in place, never compacted.
fn deliver_after(channels: &mut ChannelState, AfterLine { dropped }: AfterLine) -> bool {
    let Some(name) = dropped else {
        return false;
    };
    let freed = channels.free_slot(&name);
    debug!("chat: channel {name:?} record dropped (slot {freed:?} now free)");
    true
}

/// The channel list's update (`0x505d40`) with the `World` lent: the pane relists the live records
/// and fires `CHANNEL_UI_UPDATE`.
pub(super) fn channels_updated(world: &mut World) {
    crate::ui_script::safe_world::lend(world, |s| s.channels_updated());
}

/// The speaker's gesture, which the add selects ahead of the line (`0x50a1ce`).
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct SpeakerEffects<'w> {
    gestures: ResMut<'w, crate::creature_anim::GestureQueue>,
}

/// `UNIT_FIELD_FLAGS` bit `0x20000000` (`PREVENT_ANIM`: Polymorph, Kidney Shot, Gouge, Sleep).
const UNIT_FLAG_PREVENT_ANIM: u32 = 0x2000_0000;

/// Whether the speaker's flags suppress the chat talk gesture (`0x50a1ce`). The composer holds the
/// one selector, and a line queued for a name re-enters it, so a held line passes the same gate.
pub(super) fn gesture_prevented(speaker_flags: Option<u32>) -> bool {
    speaker_flags.is_some_and(|f| f & UNIT_FLAG_PREVENT_ANIM != 0)
}

/// Everything the add reads to build a line, fetched apart from the VM.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct AddParams<'w, 's> {
    log: ResMut<'w, ChatLog>,
    channels: Res<'w, ChannelState>,
    names: Res<'w, NameCache>,
    speaker: SpeakerEffects<'w>,
    commands: Res<'w, NetCommands>,
    /// The text-emote tables, and our guid for the "are you the performer?" test.
    emote_texts: Option<Res<'w, EmoteTexts>>,
    self_guid: Res<'w, SelfGuid>,
    /// The `$`-macro subject: monster and BG lines expand against their addressee's descriptors.
    guids: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    states: Res<'w, crate::world_state::WorldStates>,
    /// An award line's `$a`: the achievement tables and the game date its link carries.
    award: (
        Option<Res<'w, crate::ui_achievement::Achievements>>,
        Res<'w, crate::net::ServerTime>,
    ),
    /// The language gate: the word pool, and the active player's GM bit and fluency read at the call.
    langs: super::language::Hearer<'w, 's>,
    /// `Languages.dbc`, whose record names arg3 and the log's header (`0x4fdbfd`, `0x50abd6`).
    languages: Option<Res<'w, crate::ui_unit::LanguagesRes>>,
    /// Item names for `TRADESKILL_LOG`, `FEEDPET_LOG`, `ITEMENCHANTMENT*`, `SPELLDURABILITYDAMAGE`.
    items: Res<'w, crate::items::Items>,
    /// The item records' callbacks, which an uncached combat line's item joins.
    record_waits: Option<NonSendMut<'w, crate::items::RecordWaits>>,
    /// The spam and profanity filters, which run in the reference's display function.
    text_filter: crate::text_filter::ChatTextFilter<'w>,
    /// The honor title's side, off the victim's faction template (`0x50b755`).
    factions: Option<Res<'w, crate::target::ring::Factions>>,
}

/// What building a line left: the line back while a name it needs is not cached (`0x509f92` ->
/// `0x50ad82`), else the steps that add it, in order, none for a line that prints nothing.
enum Built {
    Held(ChatLine),
    Steps(Vec<Step>),
}

/// One step of an added line, run with the `World` after the build lets go of it.
enum Step {
    /// A raise with the `World` lent.
    Fire(&'static str, Vec<benilla_ui::script::ScriptValue>),
    /// A built line recorded on its type and its event raised (`0x50a8a5`, `0x50a8d5`), then logged.
    Route(ChatEvent),
    /// A channel notice: its channel writes around the line ([`deliver`]).
    Deliver(ChatEvent),
    /// A text emote's line, outside the add (`0x503660`): its event with line id 0 and no chat
    /// record (`0x5038f3`, `0x503922`), then the sentence written raw to the log (`0x503945`).
    TextEmote(ChatEvent),
    /// A spoken line: its chat record, the line, then its bubble (`0x50a970`).
    Speech {
        event: ChatEvent,
        record: benilla_ui::script::ChatRecord,
        text: String,
    },
}

/// `CHAT_MSG_ADDON`'s four arguments.
fn addon_args(
    prefix: String,
    message: String,
    distribution: String,
    sender: String,
) -> Vec<benilla_ui::script::ScriptValue> {
    use benilla_ui::script::ScriptValue::Str;
    vec![Str(prefix), Str(message), Str(distribution), Str(sender)]
}

/// A unit's name as the combat and XP lines read it in the call: the cached name, asked on a miss,
/// and `UNKNOWNOBJECT` meanwhile (`0x72a000`, `0x74d750` -> `0x74d8ef`).
fn unit_name(
    script: &UiScript,
    guid: u64,
    store: Option<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
) -> String {
    super::combat::object_name(guid, store, names, commands)
        .or_else(|| super::combat::global_string(script, "UNKNOWNOBJECT"))
        .unwrap_or_default()
}

/// Build one line through the add (`0x509dd0`): its names resolved, asked when missing, its text
/// composed; the VM is read, nothing raised. With `passes` a composed
/// line also takes the add's text passes ([`composed_passes`]); a line composed to be queued does
/// not, as 12340 queues it ahead of them (`0x509f48`) and runs them when the queue adds it again.
#[allow(clippy::too_many_lines)] // one add, every line type's arm
fn build(In((item, script, passes)): In<(ChatLine, UiScript, bool)>, p: AddParams) -> Built {
    let AddParams {
        mut log,
        channels,
        names,
        mut speaker,
        commands,
        emote_texts,
        self_guid,
        guids,
        stores,
        states,
        award: (award_db, award_time),
        langs,
        languages,
        items,
        mut record_waits,
        mut text_filter,
        factions,
    } = p;
    let mut steps = Vec::new();
    // The active player's level, which each quest link's colour reads (`0x50a342`-`0x50a356`).
    let player_level = self_guid
        .0
        .and_then(|g| guids.0.get(&g))
        .and_then(|e| stores.get(*e).ok())
        .and_then(|s| s.0.unit_level())
        .map(|l| l as i32);
    match item {
        ChatLine::Event(event) => {
            steps.push(Step::Deliver(event));
        }
        ChatLine::Wire { mut msg } => {
            // A line sent without its sender's name reads it off the player record as its full
            // name, with the record's sex, or off the pet record (`0x509e51`-`0x509ec1`).
            let mut female = false;
            let name = match &msg.sender_name {
                Some(n) => Some(n.clone()),
                None if is_player_guid(msg.sender_guid) => {
                    let guid = msg.sender_guid;
                    names.chat_record(guid, &commands).and_then(|_| {
                        // `[record+0x144] == 1` (`0x509e6f`).
                        female = names.player_traits(guid).is_some_and(|t| t.2 == 1);
                        names.peek_full_name(guid)
                    })
                }
                None if is_pet_guid(msg.sender_guid) => names
                    .chat_record(msg.sender_guid, &commands)
                    .map(str::to_string),
                None => None,
            };
            // The receiver's off its record, bare (`0x509f33`, `0x50a06f`).
            let target = msg.target_guid;
            let target_name = match &msg.receiver_name {
                Some(n) => Some(n.clone()),
                None if is_player_guid(target) || is_pet_guid(target) => {
                    names.chat_record(target, &commands).map(str::to_string)
                }
                None => None,
            };
            // A player or pet sender or receiver still unnamed holds the line, with the names it
            // read so far (`0x509f5d`-`0x509fd2` -> `0x50ad4c`).
            let target_waits = msg.receiver_name.is_none()
                && (is_player_guid(target) || is_pet_guid(target))
                && target_name.is_none();
            if (name.is_none() && sender_waits(&msg)) || target_waits {
                msg.sender_name = msg.sender_name.or(name);
                msg.receiver_name = msg.receiver_name.or(target_name);
                return Built::Held(ChatLine::Wire { msg });
            }
            // An addon line signals `CHAT_MSG_ADDON` with the sender's name or `""`, nothing else
            // (`0x509fd8` -> `0x50a0fa`).
            if msg.is_addon() {
                let (prefix, message, distribution) = addon_split(&msg);
                steps.push(Step::Fire(
                    "CHAT_MSG_ADDON",
                    addon_args(prefix, message, distribution, name.unwrap_or_default()),
                ));
                return Built::Steps(steps);
            }
            let kind = kind_of_wire(msg.chat_type);
            if kind.is_none() {
                warn!(
                    "chat: unmodeled wire type {:#04x} dropped: {:?}",
                    msg.chat_type, msg.text
                );
                return Built::Steps(steps);
            }
            // The base name; `stamp_channel` adds arg4's number and arg7-arg9 below.
            let channel_base = msg.channel.clone().unwrap_or_default();
            // The `$`-macro parser over the monster, boss, battleground and award types
            // (`0x50a120`-`0x50a157` -> `0x579590`), against the receiver: a failed token leaves the
            // wire text whole (`0x50a17d`).
            let award = matches!(
                msg.chat_type,
                benilla_protocol::messages::CHAT_MSG_ACHIEVEMENT
                    | benilla_protocol::messages::CHAT_MSG_GUILD_ACHIEVEMENT
            );
            let expanded = if MACRO_EXPANDED_TYPES.contains(&msg.chat_type) || award {
                let subject_guid = msg.target_guid;
                let subject = macro_subject(
                    subject_guid,
                    target_name.as_deref(),
                    &guids,
                    &stores,
                    &names,
                );
                // `$a`: the earner's link, completed today with every criterion (`0x578f20`).
                let link = award.then(|| {
                    let (month, day, year) = award_date(&award_time);
                    award_db.as_ref().map_or_else(String::new, |db| {
                        benilla_ui::script::achievement_hyperlink(
                            &db.0,
                            msg.achievement,
                            subject_guid,
                            true,
                            benilla_ui::script::WowDate::from_date(month, day, year),
                            [u32::MAX; 4],
                        )
                    })
                });
                let (text, clean) = crate::npc_text::substitute_checked(
                    &msg.text,
                    &crate::npc_text::MacroContext {
                        subject: subject.as_ref(),
                        states: &states,
                        achievement: link.as_deref(),
                    },
                );
                clean.then_some(text)
            } else {
                None
            };
            let plain = expanded.unwrap_or_else(|| msg.text.clone());
            // The gesture reads the raw type and `plain`, not the garbled text: the selector is
            // in the parser, on the buffer the handler hands the composer, so a Horde `lol`
            // laughs for every observer (`0x50a18a`-`0x50a26e`).
            let speaker_flags = guids
                .0
                .get(&msg.sender_guid)
                .and_then(|e| stores.get(*e).ok())
                .map(|s| s.0.unit_flags());
            if let Some(gesture) =
                crate::creature_anim::select_gesture(msg.chat_type, &plain, |n| {
                    script
                        .lua()
                        .globals()
                        .get::<String>(format!("LAUGH_WORD{n}"))
                        .ok()
                })
                .filter(|_| !gesture_prevented(speaker_flags))
            {
                speaker.gestures.push(msg.sender_guid, gesture);
            }
            // The language off the type the line came with (`0x50a273`-`0x50a28a`).
            let language = langs.effective_language(msg.chat_type, msg.language);
            // An empty text reads the type's unknown line in the sender record's sex, a say or a
            // yell becoming an emote (`0x50a291`-`0x50a2df`).
            let (chat_type, wire_type, kind, plain) =
                match unknown_text(&script, msg.chat_type, &plain, female) {
                    Some((chat_type, text)) => (
                        chat_type,
                        if chat_type == msg.chat_type {
                            msg.wire_type
                        } else {
                            benilla_protocol::messages::wotlk_chat_type::EMOTE
                        },
                        kind_of_wire(chat_type),
                        text,
                    ),
                    None => (msg.chat_type, msg.wire_type, kind, plain),
                };
            // Each quest link takes the player's difficulty colour (`0x50a2ec`-`0x50a393`).
            let plain = recolor_quest_links(plain, player_level);
            // The language gate: the garble writes the one buffer the line, `arg1` and the
            // bubble read, as the reference fills `[ebp-0xd0c]` once and never reads the wire
            // text again.
            let mut text = langs.garble(language, &plain);

            // The text filters, on the garbled buffer as in the reference: spam first, then the
            // mask, so a dropped line consumes no mask indices (the phase is global).
            if text_filter.should_drop(wire_type, flag_of_tag(msg.chat_tag), langs.is_gm(), &text) {
                // Nothing shows in its place; a line the add took as type 7, a foreign whisper
                // included (`0x50bfc7`), from a set guid is reported (`0x50a6c4`-`0x50a718`).
                if wire_type == benilla_protocol::messages::wotlk_chat_type::WHISPER
                    && msg.sender_guid != 0
                {
                    let _ = commands.0.send(crate::net::ClientCommand::ChatFiltered {
                        guid: msg.sender_guid,
                    });
                }
                debug!("chat: spam filter dropped [{chat_type:#04x}] {text:?}");
                return Built::Steps(steps);
            }
            text_filter.mask_chat(wire_type, &mut text);
            let text = text;
            let mut event = ChatEvent {
                kind,
                text: text.clone(),
                // A player line got here named: an unnamed one is held above (`0x509f92`).
                sender: name.unwrap_or_default(),
                // The effective language's record name, empty with no record (`0x4fdbd5`-`0x4fdc00`).
                language: languages
                    .as_deref()
                    .and_then(|l| l.0.name(language))
                    .unwrap_or_default()
                    .to_string(),
                language_id: language,
                channel: channel_base,
                flag: flag_of_tag(msg.chat_tag).to_string(),
                sender_guid: msg.sender_guid,
                ..Default::default()
            };
            channels.stamp_channel(&mut event);
            // The record is written under the line id the event carries (`0x4fdd80`).
            let record = benilla_ui::script::ChatRecord {
                chat_type: u32::from(wire_type),
                sender_guid: msg.sender_guid,
                sender_word: msg.sender_word,
                sender_name: event.sender.clone(),
                channel: msg.channel.clone().unwrap_or_default(),
                zone_channel: event.zone_channel_id,
                text: text.clone(),
                line_id: 0,
                time: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as u32),
            };
            // The bubble gets the same text, a monster's say and yell with `%%` read as `%`
            // (`0x50a91a`-`0x50a968`), shown in this path (`0x50a970`).
            let bubble = if matches!(
                chat_type,
                benilla_protocol::messages::CHAT_MSG_MONSTER_SAY
                    | benilla_protocol::messages::CHAT_MSG_MONSTER_YELL
            ) {
                unescape_percent(&text)
            } else {
                text
            };
            steps.push(Step::Speech {
                event,
                record,
                text: bubble,
            });
        }
        ChatLine::Notice {
            notice,
            channel,
            a_guid,
            b_guid,
        } => {
            let a = names.chat_record(a_guid, &commands).map(str::to_string);
            let b = if b_guid != 0 {
                names.chat_record(b_guid, &commands).map(str::to_string)
            } else {
                Some(String::new())
            };
            let invite = notice == channel_notice::INVITE;
            if a.is_none() || b.is_none() {
                log.invite_waiting |= invite;
                return Built::Held(ChatLine::Notice {
                    notice,
                    channel,
                    a_guid,
                    b_guid,
                });
            }
            if invite {
                log.invite_waiting = false;
                // A named inviter asks the player first (`0x50e7fb`, `0x4fb606`).
                if let Some(name) = &a {
                    steps.push(Step::Fire(
                        "CHANNEL_INVITE_REQUEST",
                        vec![
                            benilla_ui::script::ScriptValue::Str(channel.clone()),
                            benilla_ui::script::ScriptValue::Str(name.clone()),
                        ],
                    ));
                }
            }
            // Stamped against the channel list as it stands at delivery: the reference hands
            // the composer the notice's channel name, directly when the names are cached, else
            // from the queued line's node (`+0x2c`) in the name-query callback, and the
            // composer looks it up among the joined channels. Unstamped, arg7-arg9 are empty
            // and the stock filter (`ChatFrame.lua:1374-1392`) drops the line.
            if let Some(event) = notice_event(notice, channel, a, b) {
                steps.push(Step::Deliver(event));
            }
        }
        ChatLine::Mode {
            channel,
            guid,
            old_flags,
            new_flags,
        } => {
            // Only a cached name prints; a miss asks for it and prints nothing (`0x50cfe7`).
            let Some(name) = names.resolve(guid, &commands).map(str::to_string) else {
                return Built::Steps(steps);
            };
            for token in mode_change_tokens(old_flags, new_flags) {
                let event = ChatEvent {
                    kind: Some(ChatEventKind::ChannelNoticeUser),
                    text: token.to_string(),
                    sender: name.clone(),
                    channel: channel.clone(),
                    ..Default::default()
                };
                steps.push(Step::Deliver(event));
            }
        }
        ChatLine::Roll {
            min,
            max,
            roll,
            guid,
        } => {
            // Composed with the record the handler or its callback found (`0x6e3848`, `0x6cb990`).
            let Some(name) = names.peek(guid).map(str::to_string) else {
                return Built::Steps(steps);
            };
            // `RANDOM_ROLL_RESULT`, "%s rolls %d (%d-%d)" (`GlobalStrings.lua:3290`).
            let line = {
                use benilla_ui::strings::Arg;
                keyed(
                    &globals(&script),
                    "RANDOM_ROLL_RESULT",
                    &[
                        Arg::S(&name),
                        Arg::D(i64::from(roll)),
                        Arg::D(i64::from(min)),
                        Arg::D(i64::from(max)),
                    ],
                )
            };
            if let Some(text) = line {
                steps.push(Step::Route(ChatEvent::text_only(
                    ChatEventKind::System,
                    text,
                )));
            }
        }
        ChatLine::XpGain { line } => {
            // The victim's name read in the call (`0x50b20a`, `0x50b23e`): never held.
            let victim = line.victim();
            let name = (victim != 0).then(|| {
                let store = guids.0.get(&victim).and_then(|e| stores.get(*e).ok());
                unit_name(&script, victim, store, &names, &commands)
            });
            let line = xp_gain_line(&line, name.as_deref(), &globals(&script));
            if let Some(text) = line {
                steps.push(Step::Route(ChatEvent::text_only(
                    ChatEventKind::CombatXpGain,
                    text,
                )));
            }
        }
        ChatLine::HonorGain {
            victim,
            honor,
            rank,
        } => {
            // `victim == 0` is the award form: no name, no rank title. A victim not in view prints
            // nothing (`0x50b709`); its name is read in the call (`0x50b7c0`), never held.
            let store = (victim != 0)
                .then(|| guids.0.get(&victim).and_then(|e| stores.get(*e).ok()))
                .flatten();
            if victim != 0 && store.is_none() {
                return Built::Steps(steps);
            }
            let name =
                store.map(|store| unit_name(&script, victim, Some(store), &names, &commands));
            // `rank < 0` takes `COMBATLOG_HONORGAIN_NO_RANK`; `rank >= 0` takes
            // `COMBATLOG_HONORGAIN` (`0x50b717`-`0x50b72f`). The title's side is the victim's
            // faction template group: Horde (`0x4`) side 0, else Alliance (`0x2`) side 1
            // (`0x50b755`-`0x50b784`); its gender ours (`0x50b7ab`). A side-less victim skips the
            // title's write (`0x50b787`) and 12340 reads an uninitialized buffer; benilla keeps
            // side 0 (a bug). A missing global resolves to `""` (`0x819d40`).
            let team = crate::target::ring::faction_group_mask(factions.as_deref(), store)
                .map_or(0, |mask| u8::from(mask & 0x4 == 0 && mask & 0x2 != 0));
            let female = self_guid
                .0
                .and_then(|g| names.player_traits(g))
                .is_some_and(|(_, _, gender)| gender == 1);
            let title = (victim != 0 && rank >= 0).then(|| {
                script
                    .pvp_rank_title(rank, team, female)
                    .unwrap_or_default()
            });
            let line = honor_gain_line(name.as_deref(), title.as_deref(), honor, &globals(&script));
            if let Some(text) = line {
                steps.push(Step::Route(ChatEvent::text_only(
                    ChatEventKind::CombatHonorGain,
                    text,
                )));
            }
        }
        ChatLine::TextEmote {
            performer,
            text_id,
            target,
        } => {
            // The text emote's own line (`0x503660`), never the add's: no active player, no line
            // (`0x5036cb`); the performer's name record read with no callback, the handler having
            // queued the emote on a miss (`0x5040ef`-`0x504107`).
            let Some(me) = self_guid
                .0
                .filter(|g| guids.0.get(g).is_some_and(|e| stores.get(*e).is_ok()))
            else {
                return Built::Steps(steps);
            };
            let Some(performer_name) = names.chat_record(performer, &commands).map(str::to_string)
            else {
                return Built::Held(ChatLine::TextEmote {
                    performer,
                    text_id,
                    target,
                });
            };
            let Some(cat) = emote_texts.as_deref() else {
                debug!("chat: text emote {text_id} from {performer:#x} has no sentence source");
                return Built::Steps(steps);
            };
            // Our unit's name for the "at you" test (`0x503753`), never waited on.
            let store = guids.0.get(&me).and_then(|e| stores.get(*e).ok());
            let your_name = unit_name(&script, me, store, &names, &commands);
            let event = text_emote_event(
                &cat.0,
                text_id,
                &EmoteLine {
                    performer: &performer_name,
                    performer_is_you: me == performer,
                    // Sex 1 is female, where the reference reads it (`record+0x144`, `0x50377a`).
                    performer_female: names
                        .player_traits(performer)
                        .is_some_and(|(_, _, sex)| sex == 1),
                    target: &target,
                    your_name: &your_name,
                },
            );
            // No sentence is a real outcome: `/sit` prints nothing (`0x5037fb` -> `0x50394f`).
            let Some(mut event) = event else {
                return Built::Steps(steps);
            };
            // arg3 the language record 0's name, empty with none (`0x503876`-`0x5038a6`); arg12 the
            // performer's guid (`0x5038e2`).
            event.language = languages
                .as_deref()
                .and_then(|l| l.0.name(0))
                .unwrap_or_default()
                .to_string();
            event.sender_guid = performer;
            steps.push(Step::TextEmote(event));
        }
        ChatLine::Combat(mut line) => {
            // The item record comes first: uncached, it is asked with the composer's callback,
            // which composes the line again on its answer, and nothing shows now (`0x50bdb1`,
            // `0x50bb11`); the open-lock line asks with none, which sends no query (`0x67ca5d`), and
            // shows nothing (`0x50ddeb`).
            if let super::combat::Named::Item(entry, callback) = line.named {
                let name = match callback {
                    Some(callback) => {
                        let name = items
                            .ask_template(entry, 0, &commands)
                            .map(|t| t.name.clone());
                        if name.is_none() {
                            if let Some(waits) = record_waits.as_deref_mut() {
                                waits.wait(entry, callback, false);
                            }
                        }
                        name
                    }
                    None => items.template_cached(entry).map(|t| t.name.clone()),
                };
                let Some(name) = name else {
                    return Built::Steps(steps);
                };
                line.fills.named = name;
            }
            // The unit names are read in the call (`0x50be09`, `0x50bce3`, `0x50de48`), never held;
            // guid 0 needs none. The feed-pet line reads its caster's name record with no callback
            // and shows nothing without one (`0x50bb80`-`0x50bb87`).
            if line.subject != 0 {
                if line.family.stem == super::combat::FEEDPET_LOG.stem {
                    let record = is_player_guid(line.subject)
                        .then(|| names.peek(line.subject))
                        .flatten();
                    let Some(name) = record else {
                        return Built::Steps(steps);
                    };
                    line.fills.subject = name.to_string();
                } else {
                    let store = guids.0.get(&line.subject).and_then(|e| stores.get(*e).ok());
                    line.fills.subject = unit_name(&script, line.subject, store, &names, &commands);
                }
            }
            if let super::combat::Named::Unit(guid) = line.named {
                let store = guids.0.get(&guid).and_then(|e| stores.get(*e).ok());
                line.fills.named = unit_name(&script, guid, store, &names, &commands);
            }
            let composed = super::combat::compose_line(&script, line.family, line.me, &line.fills);
            if let Some(text) = composed {
                steps.push(Step::Route(ChatEvent::text_only(line.kind, text)));
            }
        }
        ChatLine::Discovery { area, xp } => {
            // The toast fires every time; the chat line only rides XP.
            let (toast, line) = {
                let get = globals(&script);
                (
                    exploration_toast(&area, &get),
                    (xp > 0)
                        .then(|| exploration_line(&area, xp, &get))
                        .flatten(),
                )
            };
            if let Some(toast) = toast {
                steps.push(Step::Fire(
                    "UI_INFO_MESSAGE",
                    vec![benilla_ui::script::ScriptValue::Str(toast)],
                ));
            }
            if let Some(text) = line {
                steps.push(Step::Route(ChatEvent::text_only(
                    ChatEventKind::System,
                    text,
                )));
            }
        }
    }
    if passes {
        steps.retain_mut(|step| match step {
            Step::Route(event) | Step::Deliver(event) => composed_passes(
                event,
                player_level,
                &mut text_filter,
                langs.is_gm(),
                &commands,
            ),
            Step::Fire(..) | Step::Speech { .. } | Step::TextEmote(_) => true,
        });
    }
    Built::Steps(steps)
}

/// The add's text passes on a line its caller composed, every one handed over with no conversation
/// (`[ebp+0x40]` 0, as at `0x50d8c9`): each quest link recoloured (`0x50a2e2` -> `0x50a2ec`-
/// `0x50a393`), the spam filter on its 3.3.5a type, a dropped whisper with a sender reported
/// (`0x50a6b8`-`0x50a718`), then the mask (`0x50a732`). False when the filter drops the line.
fn composed_passes(
    event: &mut ChatEvent,
    player_level: Option<i32>,
    text_filter: &mut crate::text_filter::ChatTextFilter,
    viewer_is_gm: bool,
    commands: &NetCommands,
) -> bool {
    let Some(add_type) = event.kind.and_then(super::event::wotlk_type_of_kind) else {
        return true;
    };
    event.text = recolor_quest_links(std::mem::take(&mut event.text), player_level);
    if text_filter.should_drop(add_type, &event.flag, viewer_is_gm, &event.text) {
        if add_type == benilla_protocol::messages::wotlk_chat_type::WHISPER
            && event.sender_guid != 0
        {
            let _ = commands.0.send(crate::net::ClientCommand::ChatFiltered {
                guid: event.sender_guid,
            });
        }
        debug!(
            "chat: spam filter dropped [{add_type:#04x}] {:?}",
            event.text
        );
        return false;
    }
    text_filter.mask_chat(add_type, &mut event.text);
    true
}

/// One line through the add (`0x509dd0`): built with the parameters fetched, then each step run
/// with no borrow held across its raise; the line back when a name it needs is not cached yet.
fn add_one(world: &mut World, line: ChatLine) -> Option<ChatLine> {
    let mut script = world.get_non_send_resource::<UiScript>()?.share();
    let built = match world.run_system_cached_with(build, (line, script.share(), true)) {
        Ok(built) => built,
        Err(e) => {
            warn!("chat: the add did not run: {e}");
            return None;
        }
    };
    match built {
        Built::Held(line) => Some(line),
        Built::Steps(steps) => {
            for step in steps {
                run_step(world, &mut script, step);
            }
            None
        }
    }
}

/// Run one step of an added line with the `World` lent around its raise.
fn run_step(world: &mut World, script: &mut UiScript, step: Step) {
    match step {
        Step::Fire(event, args) => crate::ui_script::safe_world::raise(world, event, args),
        Step::Route(event) => {
            let record = composed_record(&event);
            route_in_world(world, script, &event, record);
        }
        Step::TextEmote(event) => {
            crate::ui_script::safe_world::raise(
                world,
                "CHAT_MSG_TEXT_EMOTE",
                event.script_args_with_line(0),
            );
            // The log flag as the listeners left it (`0x50392a`).
            if script.logging_flags().0 {
                if let Some(mut windows) = world.get_resource_mut::<ChatWindows>() {
                    if windows.logs.0.is_open() {
                        windows.logs.0.record(event.text.as_bytes());
                    }
                }
            }
        }
        Step::Speech {
            event,
            record,
            text,
        } => {
            let Some(kind) = route_raise(world, script, &event, Some(record)) else {
                return;
            };
            // The bubble after the event, ahead of the log (`0x50a970`, `0x50a975`).
            if world.contains_resource::<crate::chat_bubble::BubbleQueue>() {
                world.resource_scope(|world, mut bubbles: Mut<crate::chat_bubble::BubbleQueue>| {
                    if let Some(cfg) = world.get_resource::<crate::chat_bubble::BubbleConfig>() {
                        bubbles.push(cfg, event.sender_guid, kind, &text);
                    }
                });
            }
            route_log(world, script, &event, kind);
        }
        Step::Deliver(mut event) => {
            let Some(mut channels) = world.get_resource_mut::<ChannelState>() else {
                return;
            };
            let (password, relist) = deliver_before(&mut channels, &mut event);
            if let Some(relist) = relist {
                let name = &event.channel;
                crate::ui_script::safe_world::lend(world, |s| {
                    if relist == Relist::Left {
                        s.channel_left(name);
                    }
                    s.channels_updated();
                });
            }
            if password {
                crate::ui_script::safe_world::raise(
                    world,
                    "CHANNEL_PASSWORD_REQUEST",
                    password_request_args(&event),
                );
            }
            let after = deliver_mid(&mut world.resource_mut::<ChannelState>(), &mut event);
            let record = composed_record(&event);
            route_in_world(world, script, &event, record);
            if deliver_after(&mut world.resource_mut::<ChannelState>(), after) {
                channels_updated(world);
            }
        }
    }
}

/// The chat record the add writes for a line its caller composed (`0x50a8a5` -> `0x4fdd80`): the
/// caller's type, name, channel and guid, with no sender word (a defense line pushes 0 for each but
/// the channel, `0x50c4a3`-`0x50c4c6`); the record keeps only its listed types (`0x4fb1a0`).
fn composed_record(event: &ChatEvent) -> Option<benilla_ui::script::ChatRecord> {
    let chat_type = event.kind.and_then(super::event::wotlk_type_of_kind)?;
    Some(benilla_ui::script::ChatRecord {
        chat_type: u32::from(chat_type),
        sender_guid: event.sender_guid,
        sender_word: 0,
        sender_name: event.sender.clone(),
        channel: if event.channel_base.is_empty() {
            event.channel.clone()
        } else {
            event.channel_base.clone()
        },
        zone_channel: event.zone_channel_id,
        text: event.text.clone(),
        line_id: 0,
        time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as u32),
    })
}

/// The add's tail for a built line: the next line id and the chat record under it (`0x4fdd80`),
/// the line recorded on its type and its event raised with the `World` lent (`0x50a8a5`, `0x4fdbc0`
/// at `0x4fdd5c`), then the log's line, composed only while the log is open as the add finds it
/// then (`0x50a975`).
fn route_in_world(
    world: &mut World,
    script: &mut UiScript,
    event: &ChatEvent,
    record: Option<benilla_ui::script::ChatRecord>,
) {
    if let Some(kind) = route_raise(world, script, event, record) {
        route_log(world, script, event, kind);
    }
}

/// [`route_in_world`] up to the raise: the line id, the record and the event (`0x50a8a5`,
/// `0x50a8d5`); the kind raised, `None` for a line with none. A type whose event has no name
/// raises nothing (slot 658 for `0x32`, `0x9fbe80`).
fn route_raise(
    world: &mut World,
    script: &mut UiScript,
    event: &ChatEvent,
    record: Option<benilla_ui::script::ChatRecord>,
) -> Option<ChatEventKind> {
    let Some(kind) = event.kind else {
        warn!("chat: unroutable event (no kind): {:?}", event.text);
        return None;
    };
    // The active player the add read at its call (`0x50a3ad`), 0 with none in view.
    let player = world
        .get_resource::<SelfGuid>()
        .and_then(|s| s.0)
        .filter(|g| {
            world
                .get_resource::<GuidIndex>()
                .is_some_and(|i| i.0.contains_key(g))
        })
        .unwrap_or(0);
    let mut windows = world.get_resource_mut::<ChatWindows>()?;
    windows.line_id = windows.line_id.wrapping_add(1);
    let line_id = windows.line_id;
    if let Some(mut record) = record {
        record.line_id = line_id;
        script.record_chat(record, player);
    }
    let name = super::event::event_name(kind);
    if !name.is_empty() {
        let args = event.script_args_with_line(line_id);
        crate::ui_script::safe_world::raise(world, name, args);
    }
    Some(kind)
}

/// [`route_in_world`]'s log line, written while the log flag is set (`0x50a975`).
fn route_log(world: &mut World, script: &UiScript, event: &ChatEvent, kind: ChatEventKind) {
    // The flag as the listeners left it (`[0xb759d0]`).
    if script.logging_flags().0 {
        let default_language = super::language::default_language_id(world);
        if let Some(mut windows) = world.get_resource_mut::<ChatWindows>() {
            super::frames::log_added(
                &mut windows,
                event,
                kind,
                default_language,
                &globals(script),
            );
        }
    }
}

/// Add `line` in the caller's turn (`0x509dd0`), behind what the caller queued so far.
pub(crate) fn add(world: &mut World, line: ChatLine) {
    crate::ui_script::live::fire_handler_events(world);
    add_now(world, line);
}

/// The add (`0x509dd0`): the names it needs asked with the answer's callback, a whisper's sender
/// entered in autocomplete (`0x509e51`, `0x509ee0`, `0x509f33`); then the line queued while the
/// UI is outside the world (`0x509f48`), held when a name is not cached (`0x509f92` ->
/// `0x50ad82`), else built and its event raised with the `World` lent before the caller goes on.
pub(crate) fn add_now(world: &mut World, line: ChatLine) {
    let Some(log) = world.get_resource::<ChatLog>() else {
        return;
    };
    // A second channel invite is dropped while one waits on its inviter's name (`0x50e79e`).
    if log.invite_waiting
        && matches!(line, ChatLine::Notice { notice, .. } if notice == channel_notice::INVITE)
    {
        return;
    }
    let queue_chat = log.queue_chat;
    match &line {
        // The text emote handler (`0x504070`) asks with its own callback (`0x5040ef`); a miss queues
        // the emote on its own list (`0x504107`), never the add's.
        ChatLine::TextEmote { performer, .. } => {
            if ask_with(world, *performer, NameWait::TextEmote).is_none() {
                world.resource_mut::<ChatLog>().emotes.push(line);
                return;
            }
            // Outside the world a named emote is queued marked (`0x50368c` -> `0x502ea0`) on a
            // list nothing walks again (`0x503fea` skips it), so it never shows.
            if queue_chat {
                return;
            }
        }
        // The roll handler asks with the roll as its callback's param (`0x6e3817`); a miss leaves
        // the line to that callback (`0x6e382e`), which composes it at the answer.
        ChatLine::Roll {
            min,
            max,
            roll,
            guid,
        } => {
            let wait = NameWait::Roll {
                min: *min,
                max: *max,
                roll: *roll,
            };
            if ask_with(world, *guid, wait).is_none() {
                return;
            }
        }
        _ => ask_names(world, &line),
    }
    if queue_chat {
        if line.composed_by_caller() {
            queue_composed(world, line);
        } else {
            let line = with_read_names(world, line, true);
            world.resource_mut::<ChatLog>().pending.push((line, true));
        }
        return;
    }
    if let Some(held) = add_one(world, line) {
        let mut log = world.resource_mut::<ChatLog>();
        match held {
            ChatLine::TextEmote { .. } => log.emotes.push(held),
            held => log.pending.push((held, false)),
        }
    }
}

/// Queue a line its caller composes ahead of the add while the UI is outside the world: its text
/// built now, with the names as they read now, and queued as text (`0x509f92`); a step that is no
/// chat line (the discovery toast) runs at once.
fn queue_composed(world: &mut World, line: ChatLine) {
    let Some(mut script) = world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::share)
    else {
        world.resource_mut::<ChatLog>().pending.push((line, true));
        return;
    };
    let built = match world.run_system_cached_with(build, (line, script.share(), false)) {
        Ok(built) => built,
        Err(e) => {
            warn!("chat: the add did not run: {e}");
            return;
        }
    };
    match built {
        Built::Held(line) => world.resource_mut::<ChatLog>().pending.push((line, false)),
        Built::Steps(steps) => {
            for step in steps {
                match step {
                    Step::Route(event) => world
                        .resource_mut::<ChatLog>()
                        .pending
                        .push((ChatLine::Event(event), true)),
                    other => run_step(world, &mut script, other),
                }
            }
        }
    }
}

/// Register the roster's callback on `guid`'s record after its lookup missed (`0x67d880`).
pub(crate) fn register_roster_wait(world: &mut World, guid: u64) {
    if let Some(mut log) = world.get_resource_mut::<ChatLog>() {
        log.register(NameKey::Player(guid), NameWait::Roster);
    }
}

/// Ask `guid`'s name record with `wait` as its callback (`0x67d770`): the cached name, else
/// `None` with the callback registered on the record, a missing record asked for.
fn ask_with(world: &mut World, guid: u64, wait: NameWait) -> Option<String> {
    let name = {
        let (Some(names), Some(commands)) = (
            world.get_resource::<NameCache>(),
            world.get_resource::<NetCommands>(),
        ) else {
            return None;
        };
        names.chat_record(guid, commands).map(str::to_string)
    };
    if name.is_none() {
        if let Some(key @ (NameKey::Player(_) | NameKey::Pet(_))) = NameKey::of(guid, None) {
            world.resource_mut::<ChatLog>().register(key, wait);
        }
    }
    name
}

/// The add's name lookups ahead of its queue test (`0x509f48`): each player guid the line names
/// is asked with the answer's callback when missing (`0x509e51`, `0x509f33`), then a whisper's
/// sender entered in autocomplete with the name the add read ([`note_whisper`]).
fn ask_names(world: &mut World, line: &ChatLine) {
    // A player guid asks the name cache, a pet guid the pet-name cache (`0x509f33`, `0x50a06f`).
    for guid in line.player_guids() {
        if is_player_guid(guid) || is_pet_guid(guid) {
            let _ = ask_with(world, guid, NameWait::Chat);
        }
    }
    if let ChatLine::Wire { msg, .. } = line {
        note_whisper(world, msg);
    }
}

/// The add's autocomplete entry, once per add call (`0x509ec7`-`0x509ee0`): a whisper or its
/// echo, an addon whisper aside, enters its sender's guid with the name the add read: the passed
/// name (`0x509e17`), else a cached player's full name (`0x509e6a`) or the pet record's
/// (`0x509eb6`), else none.
fn note_whisper(world: &mut World, msg: &benilla_protocol::messages::ChatMessage) {
    let whisper = matches!(
        msg.chat_type,
        benilla_protocol::messages::CHAT_MSG_WHISPER
            | benilla_protocol::messages::CHAT_MSG_WHISPER_INFORM
    ) && msg.language != 0xFFFF_FFFF;
    if !whisper {
        return;
    }
    let guid = msg.sender_guid;
    let name = msg.sender_name.clone().or_else(|| {
        let names = world.get_resource::<NameCache>()?;
        if is_player_guid(guid) {
            names.peek_full_name(guid)
        } else if is_pet_guid(guid) {
            names.peek(guid).map(str::to_string)
        } else {
            None
        }
    });
    if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
        script.note_autocomplete_contact(guid, name.as_deref());
    }
}

/// A name record's answer (`0x680c60`) or denial (`0x67a2d0`): this module's callbacks on `key`
/// run head to tail in registration order (`0x680d03`-`0x680d2c`), each over its own list.
pub(crate) fn name_landed(world: &mut World, key: NameKey) {
    let Some(mut log) = world.get_resource_mut::<ChatLog>() else {
        return;
    };
    let mut waits = Vec::new();
    log.waits.retain(|&(k, wait)| {
        if k == key {
            waits.push(wait);
        }
        k != key
    });
    for wait in waits {
        match wait {
            NameWait::Chat => chat_callback(world, key),
            NameWait::TextEmote => text_emote_callback(world, key),
            NameWait::Roll { min, max, roll } => roll_callback(world, key, min, max, roll),
            NameWait::Roster => {
                if let NameKey::Player(guid) = key {
                    super::net::roster_callback(world, guid);
                }
            }
        }
    }
}

/// The chat add's callback over its held lines (`0x503c80`): each unqueued line naming `key` is
/// added once all its names are cached, in hold order (`0x503edb`), one whose `key` came back
/// denied is dropped (`0x503e5c`), and the lines queued for the world entry stay (`0x503cbb`).
fn chat_callback(world: &mut World, key: NameKey) {
    let Some(mut log) = world.get_resource_mut::<ChatLog>() else {
        return;
    };
    let queue_chat = log.queue_chat;
    let held = std::mem::take(&mut log.pending);
    let mut kept = Vec::new();
    for (line, queued) in held {
        if queued || !line.names_key(key) {
            kept.push((line, queued));
            continue;
        }
        let (missing, ready) = world
            .get_resource::<NameCache>()
            .map_or((false, false), |names| {
                (line.names_missing(key, names), line.names_ready(names))
            });
        if missing {
            if matches!(line, ChatLine::Notice { notice, .. } if notice == channel_notice::INVITE) {
                world.resource_mut::<ChatLog>().invite_waiting = false;
            }
            continue;
        }
        // A line still waiting on its other name stays for that answer (`0x503ef9`).
        if !ready {
            kept.push((line, false));
            continue;
        }
        // Outside the world a ready line is marked queued for the entry's walk (`0x503e78`).
        if queue_chat {
            kept.push((line, true));
            continue;
        }
        // The callback hands the add the records' bare names (`0x503e87`-`0x503e98`) and calls it
        // again (`0x503edb`), which asks its names again.
        let line = with_read_names(world, line, false);
        ask_names(world, &line);
        if let Some(back) = add_one(world, line) {
            kept.push((back, false));
        }
    }
    restore(world, kept);
}

/// The text emotes' callback over their own list (`0x503fb0`): each unqueued emote by `key`'s
/// performer leaves it, shown (`0x50403f`) when the name is cached and the UI is in the world;
/// outside it the emote is marked queued (`0x504026`) where nothing walks it again, so it never
/// shows, and a denied name drops it (`0x50401c`).
fn text_emote_callback(world: &mut World, key: NameKey) {
    let Some(mut log) = world.get_resource_mut::<ChatLog>() else {
        return;
    };
    let queue_chat = log.queue_chat;
    let held = std::mem::take(&mut log.emotes);
    let mut kept = Vec::new();
    for line in held {
        let ChatLine::TextEmote { performer, .. } = &line else {
            continue;
        };
        if NameKey::of(*performer, None) != Some(key) {
            kept.push(line);
            continue;
        }
        let named = world.get_resource::<NameCache>().is_some_and(|names| {
            names.peek(*performer).is_some() && !names.chat_denied(*performer)
        });
        if !named || queue_chat {
            continue;
        }
        if let Some(back) = add_one(world, line) {
            kept.push(back);
        }
    }
    let mut log = world.resource_mut::<ChatLog>();
    let later = std::mem::replace(&mut log.emotes, kept);
    log.emotes.extend(later);
}

/// A roll's callback (`0x6cb990`): with the roller's record found it composes
/// `RANDOM_ROLL_RESULT` and calls the add with the text; a denial drops the roll.
fn roll_callback(world: &mut World, key: NameKey, min: u32, max: u32, roll: u32) {
    let NameKey::Player(guid) = key else {
        return;
    };
    let found = world
        .get_resource::<NameCache>()
        .is_some_and(|names| names.peek(guid).is_some() && !names.chat_denied(guid));
    if found {
        add_now(
            world,
            ChatLine::Roll {
                min,
                max,
                roll,
                guid,
            },
        );
    }
}

/// A wire line with the names the add has read so far filled in, as it queues them with the line
/// (`0x50ad4c`) or the answer's callback passes them (`0x503e87`-`0x503e98`): a player sender's
/// full name when `full` (`0x509e6a`), else the bare record name; the receiver's bare.
fn with_read_names(world: &World, line: ChatLine, full: bool) -> ChatLine {
    let ChatLine::Wire { mut msg } = line else {
        return line;
    };
    let Some(names) = world.get_resource::<NameCache>() else {
        return ChatLine::Wire { msg };
    };
    if msg.sender_name.is_none() {
        let guid = msg.sender_guid;
        msg.sender_name = if full && is_player_guid(guid) {
            names.peek_full_name(guid)
        } else if is_player_guid(guid) || is_pet_guid(guid) {
            names.peek(guid).map(str::to_string)
        } else {
            None
        };
    }
    let target = msg.target_guid;
    if msg.receiver_name.is_none() && (is_player_guid(target) || is_pet_guid(target)) {
        msg.receiver_name = names.peek(target).map(str::to_string);
    }
    ChatLine::Wire { msg }
}

/// The UI's world entry for chat (`0x5281b3` -> `0x50df80`): the AFK mirror seeded from our flags
/// (`0x50dfab`), then the add stops queueing (`0x50dfbd`)
/// and the queued lines go through it in order (`0x50dfc3`), one still missing a name held for
/// its answer.
pub(crate) fn enter_world(world: &mut World) {
    // The AFK mirror's seed first (`0x50dfa7`-`0x50dfb7`).
    super::away::seed_on_enter_world(world);
    let Some(mut log) = world.get_resource_mut::<ChatLog>() else {
        return;
    };
    log.queue_chat = false;
    let held = std::mem::take(&mut log.pending);
    let mut kept = Vec::new();
    for (line, queued) in held {
        if !queued {
            kept.push((line, false));
            continue;
        }
        let mut line = line;
        if let ChatLine::Wire { msg } = &mut line {
            if !walk_names(world, msg) {
                // The walk's lookups ask with the chat callback (`0x50aeaf`, `0x50aedd`, `0x50af34`).
                wait_on_missing(world, &line);
                kept.push((line, false));
                continue;
            }
        }
        // The walk calls the add again on each queued line (`0x50afdc`).
        if let ChatLine::Wire { msg } = &line {
            note_whisper(world, msg);
        }
        if let Some(back) = add_one(world, line) {
            kept.push((back, false));
        }
    }
    restore(world, kept);
}

/// Register the chat callback on each player or pet record `line` names that is not cached.
fn wait_on_missing(world: &mut World, line: &ChatLine) {
    let Some(names) = world.get_resource::<NameCache>() else {
        return;
    };
    let missing: Vec<NameKey> = line
        .player_guids()
        .into_iter()
        .filter(|&g| g != 0 && (names.peek(g).is_none() || names.chat_denied(g)))
        .filter_map(|g| NameKey::of(g, None))
        .filter(|key| matches!(key, NameKey::Player(_) | NameKey::Pet(_)))
        .collect();
    let mut log = world.resource_mut::<ChatLog>();
    for key in missing {
        log.register(key, NameWait::Chat);
    }
}

/// The world-entry walk's names for a queued wire line (`0x50ae80`-`0x50af9a`): a player or pet
/// sender and receiver are read off their records again, asked with the answer's callback; false on
/// a miss, which unmarks the line for that callback (`0x50ae86`, `0x50aef9`, `0x50af7e`). The add
/// then gets the records' bare names, the stored sender name only on a GM line (`0x50af8e`).
fn walk_names(world: &World, msg: &mut benilla_protocol::messages::ChatMessage) -> bool {
    let (Some(names), Some(commands)) = (
        world.get_resource::<NameCache>(),
        world.get_resource::<NetCommands>(),
    ) else {
        return true;
    };
    let sender = msg.sender_guid;
    let sender_record = if is_player_guid(sender) || is_pet_guid(sender) {
        match names.chat_record(sender, commands) {
            Some(name) => Some(name.to_string()),
            None => return false,
        }
    } else {
        None
    };
    let target = msg.target_guid;
    if is_player_guid(target) || is_pet_guid(target) {
        match names.chat_record(target, commands) {
            Some(name) => msg.receiver_name = Some(name.to_string()),
            None => return false,
        }
    }
    if let Some(name) = sender_record.filter(|_| !msg.gm) {
        msg.sender_name = Some(name);
    }
    true
}

/// The UI's entry, then the queued chat lines added and the held ones retried, as the handlers and
/// the name answers would: a test's one pass over the chat add.
#[cfg(test)]
pub(crate) fn drain_for_test(world: &mut World) {
    enter_world(world);
    crate::ui_script::live::fire_handler_events(world);
    land_cached(world);
}

/// Run the answer of every asked record now cached or denied, in first-registration order, as its answer
/// would; a pet record always.
#[cfg(test)]
pub(crate) fn land_cached(world: &mut World) {
    let mut keys: Vec<NameKey> = Vec::new();
    for (key, _) in &world.resource::<ChatLog>().waits {
        if !keys.contains(key) {
            keys.push(*key);
        }
    }
    for key in keys {
        let cached = match key {
            NameKey::Player(g) => {
                let names = world.resource::<NameCache>();
                names.peek(g).is_some() || names.chat_denied(g)
            }
            _ => true,
        };
        if cached {
            name_landed(world, key);
        }
    }
}

/// Everything the add reads, defaulted, with the UI past its world entry; the wire's receiver.
#[cfg(test)]
pub(crate) fn insert_add_resources(
    world: &mut World,
) -> crossbeam_channel::Receiver<crate::net::ClientCommand> {
    let (tx, sent) = crossbeam_channel::unbounded();
    world.insert_resource(ChatLog {
        queue_chat: false,
        ..ChatLog::default()
    });
    world.init_resource::<crate::ui_script::live::UiEvents>();
    world.init_resource::<ChatWindows>();
    world.init_resource::<ChannelState>();
    world.init_resource::<NameCache>();
    world.init_resource::<crate::chat_bubble::BubbleQueue>();
    world.init_resource::<crate::chat_bubble::BubbleConfig>();
    world.init_resource::<crate::creature_anim::GestureQueue>();
    world.insert_resource(NetCommands(tx));
    world.init_resource::<SelfGuid>();
    world.init_resource::<GuidIndex>();
    world.init_resource::<crate::world_state::WorldStates>();
    world.init_resource::<super::language::ChatLanguages>();
    world.init_resource::<crate::items::Items>();
    world.init_resource::<crate::text_filter::TextFilter>();
    world.init_resource::<crate::text_filter::TextFilterSwitches>();
    world.init_resource::<crate::net::ServerTime>();
    sent
}

/// Put the walked lines back ahead of any a listener held during the walk.
fn restore(world: &mut World, kept: Vec<(ChatLine, bool)>) {
    let mut log = world.resource_mut::<ChatLog>();
    let later = std::mem::replace(&mut log.pending, kept);
    log.pending.extend(later);
}

/// The game date an award line's link carries, off the client's game clock; zero before the clock
/// lands.
fn award_date(time: &crate::net::ServerTime) -> (u32, u32, u32) {
    time.0.as_ref().map_or((0, 0, 0), |t| t.date())
}

/// The add's player test on a guid: no high type nibble, and not all zero (`0x509f5d`-`0x509f78`).
fn is_player_guid(guid: u64) -> bool {
    guid >> 60 == 0 && guid & 0xf07f_ffff_ffff_ffff != 0
}

/// The add's pet test on a guid: the high word's type bits are a pet's (`0x509f7a`-`0x509f86`).
fn is_pet_guid(guid: u64) -> bool {
    (guid >> 32) as u32 & 0xf0f0_0000 == 0xf040_0000
}

/// The `$`-macro parser's subject (`0x579590`): the unit in view, else the player name record;
/// its name is the receiver's sent name first, then the unit's, then the record's. `None` when
/// there is neither unit, record nor name, which fails every person token.
fn macro_subject(
    guid: u64,
    receiver_name: Option<&str>,
    index: &crate::net::GuidIndex,
    stores: &Query<&ObjectStore>,
    names: &NameCache,
) -> Option<crate::npc_text::Subject> {
    let store = index.0.get(&guid).and_then(|e| stores.get(*e).ok());
    let record = is_player_guid(guid)
        .then(|| names.peek(guid))
        .flatten()
        .filter(|_| store.is_none());
    if store.is_none() && record.is_none() && receiver_name.is_none() {
        return None;
    }
    let name = receiver_name
        .or_else(|| store.and_then(|s| names.peek_unit(guid, Some(s))))
        .or(record)
        .unwrap_or_default()
        .to_string();
    let (race, class, gender) = match store {
        Some(s) => (
            s.0.unit_race().unwrap_or(0),
            s.0.unit_class().unwrap_or(0),
            s.0.unit_gender().unwrap_or(0),
        ),
        None => names.player_traits(guid).unwrap_or((0, 0, 0)),
    };
    Some(crate::npc_text::Subject {
        name,
        race,
        class,
        gender,
    })
}

/// The add's sender test, on the guid and not the chat type: a line sent without its sender's
/// name from a player or a pet guid reads the name and waits on it (`0x509f5d`-`0x509f92`).
fn sender_waits(msg: &benilla_protocol::messages::ChatMessage) -> bool {
    msg.sender_name.is_none() && (is_player_guid(msg.sender_guid) || is_pet_guid(msg.sender_guid))
}

/// The add's line for an empty text (`0x50a291`-`0x50a2df`): a say reads `CHAT_SAY_UNKNOWN` and a
/// yell `CHAT_YELL_UNKNOWN`, both then an emote (`0x50a2d9`), an emote `CHAT_EMOTE_UNKNOWN`, each
/// in its `_FEMALE` form for a female sender record (`0x819d86`); `None` when the text is not
/// empty or the type has none.
fn unknown_text(
    script: &UiScript,
    chat_type: u8,
    text: &str,
    female: bool,
) -> Option<(u8, String)> {
    use benilla_protocol::messages as m;
    if !text.is_empty() {
        return None;
    }
    let key = match chat_type {
        m::CHAT_MSG_SAY => "CHAT_SAY_UNKNOWN",
        m::CHAT_MSG_YELL => "CHAT_YELL_UNKNOWN",
        m::CHAT_MSG_EMOTE => "CHAT_EMOTE_UNKNOWN",
        _ => return None,
    };
    Some((m::CHAT_MSG_EMOTE, gendered_text(script, key, female)))
}

/// The VM's string for `key` as the lookup reads it in a gender (`0x819d40`): the `_FEMALE` form
/// first when female, then the plain key, empty when neither is set.
fn gendered_text(script: &UiScript, key: &str, female: bool) -> String {
    let get = |k: &str| script.lua().globals().get::<String>(k).ok();
    female
        .then(|| get(&format!("{key}_FEMALE")))
        .flatten()
        .or_else(|| get(key))
        .unwrap_or_default()
}

/// The add's quest-link pass (`0x50a2ec`-`0x50a393`): each `|Hquest:` not escaped by a `|` before
/// it reads its level after the second `:` (`0x50a33b`), -1 standing for the active player's
/// (`0x50a34b`), and its ten bytes before take that level's difficulty colour (`0x50a35d`,
/// `0x50a369`). 12340 stops the client on a link without its two `:` (`0x50a3a6`) and writes ahead
/// of the buffer for a link in its first ten bytes; here such a link ends the pass, or is left.
pub(super) fn recolor_quest_links(text: String, player_level: Option<i32>) -> String {
    const LINK: &[u8] = b"|Hquest:";
    if !text.contains("|Hquest:") {
        return text;
    }
    let mut bytes = text.into_bytes();
    let mut from = 0;
    while let Some(at) = bytes[from..]
        .windows(LINK.len())
        .position(|w| w == LINK)
        .map(|i| from + i)
    {
        from = at + LINK.len();
        if at > 0 && bytes[at - 1] == b'|' {
            continue;
        }
        let colon = |start: usize| {
            bytes[start..]
                .iter()
                .position(|&b| b == b':')
                .map(|i| start + i)
        };
        let Some(level_at) = colon(at).and_then(|c| colon(c + 1)) else {
            break;
        };
        let mut level = str_to_int(&bytes[level_at + 1..]);
        if level == -1 {
            if let Some(player) = player_level {
                level = player;
            }
        }
        let color = benilla_ui::script::quest_difficulty_color(level, player_level).as_bytes();
        if let Some(start) = at.checked_sub(color.len()) {
            bytes[start..at].copy_from_slice(color);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The string-to-int read (`0x76f0d0`): an optional `-`, then decimal digits up to the first other
/// byte.
fn str_to_int(bytes: &[u8]) -> i32 {
    let (negative, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        _ => (false, bytes),
    };
    let value = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0i32, |n, b| {
            n.wrapping_mul(10).wrapping_add(i32::from(b - b'0'))
        });
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// A monster bubble's text (`0x50a92a`-`0x50a960`): each `%%` read as one `%`.
pub(super) fn unescape_percent(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '%' && chars.peek() == Some(&'%') {
            chars.next();
        }
    }
    out
}
