//! The chat add (`0x509dd0`): every chat line a handler, step or verb hands over is built here in
//! its caller's turn, its names read from [`crate::names::NameCache`], held until a missing name's
//! answer lands, and routed by [`super::frames::route`].

use bevy::prelude::*;

use benilla_protocol::messages::{
    channel_notice, ChannelNoticeTail, ChatMessage, MACRO_EXPANDED_TYPES,
};

use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{EmoteLine, EmoteTextCatalog};

use crate::names::NameCache;
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
    /// A `/random` broadcast awaiting the roller's name.
    Roll {
        min: u32,
        max: u32,
        roll: u32,
        guid: u64,
    },
    /// An XP line, its victim's name read in the call (`0x50b20a`).
    XpGain { line: XpLine },
    /// An honor award, its victim's name read in the call (`0x50b7c0`); `rank` (0 none) becomes a
    /// title at the add, from the victim's faction template.
    HonorGain { victim: u64, honor: i32, rank: u8 },
    /// A text emote waiting on its performer's name: the reference's pending-text-emote node,
    /// queued when the emote handler finds no cached name and added when the name arrives.
    /// `target` is the wire's name, empty when untargeted, used as sent.
    TextEmote {
        performer: u64,
        text_id: u32,
        target: String,
    },
    /// An area discovery (`SMSG_EXPLORATION_EXPERIENCE`).
    Discovery { area: String, xp: u32 },
    /// One combat-log line, classified at the packet: its unit names read in the call, waiting
    /// only on an item's record.
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

impl From<ChatEvent> for ChatLine {
    fn from(event: ChatEvent) -> Self {
        Self::Event(event)
    }
}

impl ChatLine {
    /// A line whose text its caller composes before the add, so a queue holds it as text: the
    /// XP, honor and combat lines and the discovery (`0x50b1d0`, `0x50b6d0`, `0x50bd90`).
    fn composed_by_caller(&self) -> bool {
        matches!(
            self,
            ChatLine::XpGain { .. }
                | ChatLine::HonorGain { .. }
                | ChatLine::Combat(_)
                | ChatLine::Discovery { .. }
        )
    }

    /// Every name the line waits on is cached (`0x503e19`-`0x503e25`).
    fn names_ready(&self, names: &NameCache, self_guid: Option<u64>) -> bool {
        let own = match self {
            ChatLine::TextEmote { .. } => self_guid.unwrap_or(0),
            _ => 0,
        };
        self.player_guids()
            .into_iter()
            .chain([own])
            .all(|g| g == 0 || names.peek(g).is_some())
    }

    /// A guid this line waits on whose answer came back denied (`0x503e5c`): the line is dropped.
    fn names_missing(&self, names: &NameCache) -> bool {
        self.player_guids()
            .into_iter()
            .any(|g| g != 0 && names.chat_denied(g))
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
            ChatLine::Roll { guid, .. } => [*guid, 0],
            ChatLine::TextEmote { performer, .. } => [*performer, 0],
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
    /// The add queues every line (`[0xac7a38]`): set from the start and at the world exit
    /// (`0x4fb190`), cleared at the UI's world entry (`0x50dfbd`).
    queue_chat: bool,
    /// A channel invite waits on its inviter's name; another is dropped meanwhile (`0x50e79e`).
    invite_waiting: bool,
}

impl Default for ChatLog {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            queue_chat: true,
            invite_waiting: false,
        }
    }
}

impl ChatLog {
    /// The UI's world exit (`0x528cc9` -> `0x4fb190`): the add queues every line until the next
    /// entry.
    pub(crate) fn leave_world(&mut self) {
        self.queue_chat = true;
    }

    /// Drop every held line on disconnect.
    pub(crate) fn clear_session(&mut self) {
        self.pending.clear();
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

    /// Every held line, addon lines included.
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
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
    fn push_pvp_credit(&mut self, honor: i32, victim: u64, rank: u8) {
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
    // The voice flag's set prints nothing, and benilla's channel pane has no voice column.
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

/// Deliver one built event: a `YOU_JOINED` claims its slot before the render, a `YOU_LEFT` frees it
/// after, so the leave line keeps its number and colour ("Left Channel: [2. General - Elwynn
/// Forest]") and a handler's `GetChannelName` still finds it. The reference flags the teardown,
/// fires the event, then drops the record; its suspended leg skips the flag. Both arms mirror the
/// list into the VM for `GetChannelName`.
#[cfg(test)]
pub(super) fn deliver(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    channels: &mut ChannelState,
    event: &mut ChatEvent,
) {
    if deliver_before(script, channels, event) {
        script.fire_event("CHANNEL_PASSWORD_REQUEST", password_request_args(event));
    }
    let after = deliver_mid(channels, event);
    route(script, windows, event);
    deliver_after(script, channels, event, after);
}

/// A `WRONG_PASSWORD` notice's `CHANNEL_PASSWORD_REQUEST` argument, ahead of the line with the
/// wire name (`0x50e3aa`).
fn password_request_args(event: &ChatEvent) -> Vec<benilla_ui::script::ScriptValue> {
    vec![benilla_ui::script::ScriptValue::Str(event.channel.clone())]
}

/// [`deliver`]'s writes ahead of the line; whether a password request fires before it.
fn deliver_before(
    script: &mut benilla_ui::script::UiScript,
    channels: &mut ChannelState,
    event: &mut ChatEvent,
) -> bool {
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
    if notice == Some(channel_notice::YOU_JOINED) {
        // A confirmed join sets the `ZONECHANNELS` bit, its only runtime growth.
        channels.note_zone_channel_joined(&event.channel);
        // A slot the walk renamed is numbered already, but its state must return to `Joined`.
        channels.confirm_slot(&event.channel);
    }
    // A zone channel's record is made at send time; one the server joined unasked has none.
    if notice == Some(channel_notice::YOU_JOINED)
        && channels.number_of(&event.channel).is_none()
        && channels.channels.zone_channel_id(&event.channel) == 0
    {
        match channels.claim_slot(&event.channel) {
            Some(slot) => {
                debug!(
                    "chat: server confirms channel {:?} joined (slot {slot})",
                    event.channel
                );
            }
            // All ten slots taken: no number, as in the reference, whose slot allocator also prints
            // `ERR_TOO_MANY_CHAT_CHANNELS` (`0x199`); here it is only logged.
            None => warn!(
                "chat: server confirms channel {:?} joined but all {} slots are taken — it has no \
                 number, so /N cannot reach it",
                event.channel,
                super::edit::MAX_CHANNELS
            ),
        }
    }
    // A custom channel's confirmed join enters the chat cache's re-join list, under its number.
    if notice == Some(channel_notice::YOU_JOINED) {
        channels.note_custom_channel_joined(&event.channel);
        // Every confirmed join relists the channel pane before the notice prints (`0x507880`).
        script.set_channel_slots(channels.ui_slots());
    }
    password
}

/// The wire names of a leave and a refusal, read before the stamp numbers the line.
struct AfterLine {
    leaving: Option<String>,
    refused: Option<String>,
}

/// [`deliver`]'s reads just before the line: the wire names of a leave and a refusal, then the
/// stamp.
fn deliver_mid(channels: &mut ChannelState, event: &mut ChatEvent) -> AfterLine {
    let notice = event
        .notice_byte()
        .filter(|_| event.kind == Some(ChatEventKind::ChannelNotice));
    // The wire name, kept before `stamp_channel` decorates arg4 with the slot number.
    let leaving = (notice == Some(channel_notice::YOU_LEFT)).then(|| event.channel.clone());
    // 3.3.5a drops the record after a `NOT_IN_AREA` or `NOT_IN_LFG` refusal, so the refused channel
    // gives its number back.
    let refused = matches!(
        notice,
        Some(channel_notice::NOT_IN_AREA | channel_notice::NOT_IN_LFG)
    )
    .then(|| event.channel.clone());
    // The channel renders numbered when its slot is known.
    channels.stamp_channel(event);
    AfterLine { leaving, refused }
}

/// [`deliver`]'s writes after the line: a leave frees its slot unless suspended, a refusal frees
/// its own; either relists the channel pane.
fn deliver_after(
    script: &mut benilla_ui::script::UiScript,
    channels: &mut ChannelState,
    event: &ChatEvent,
    AfterLine { leaving, refused }: AfterLine,
) {
    if let Some(name) = leaving {
        // A suspended leave keeps the record: the reference skips the teardown, so leaving a
        // capital keeps Trade's number for the re-join.
        if event.suspended {
            debug!("chat: channel {name:?} suspended — the record and its number stay");
        } else {
            // Cleared in place, never compacted: slot 3 stays 3 when slot 2 empties.
            let freed = channels.free_slot(&name);
            debug!("chat: server confirms channel {name:?} left (slot {freed:?} now free)");
        }
        // Either leg relists the channel pane (`0x507900`).
        script.set_channel_slots(channels.ui_slots());
    }
    if let Some(name) = refused {
        let freed = channels.free_slot(&name);
        debug!("chat: server refused channel {name:?} (slot {freed:?} now free)");
        script.set_channel_slots(channels.ui_slots());
    }
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
    /// The language gate: the word pool, this character's fluency and the GM bit.
    langs: Res<'w, super::language::ChatLanguages>,
    /// `Languages.dbc`, whose record names arg3 and the log's header (`0x4fdbfd`, `0x50abd6`).
    languages: Option<Res<'w, crate::ui_unit::LanguagesRes>>,
    /// Item names for `TRADESKILL_LOG`, `FEEDPET_LOG`, `ITEMENCHANTMENT*`, `SPELLDURABILITYDAMAGE`.
    items: Res<'w, crate::items::Items>,
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
            let Some(name) = names.chat_record(guid, &commands).map(str::to_string) else {
                return Built::Held(ChatLine::Roll {
                    min,
                    max,
                    roll,
                    guid,
                });
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
            // The title's side is the victim's faction template group: Horde (`0x4`) side 0, else
            // Alliance (`0x2`) side 1 (`0x50b755`-`0x50b784`); its gender ours (the title lookup
            // genders against the local player). A side-less victim skips the title's write
            // (`0x50b787`) and 12340 reads an uninitialized buffer; benilla keeps side 0 (a bug).
            let team = crate::target::ring::faction_group_mask(factions.as_deref(), store)
                .map_or(0, |mask| u8::from(mask & 0x4 == 0 && mask & 0x2 != 0));
            let female = self_guid
                .0
                .and_then(|g| names.player_traits(g))
                .is_some_and(|(_, _, gender)| gender == 1);
            let title = (victim != 0).then(|| script.pvp_rank_title(rank, team, female));
            let line = honor_gain_line(
                name.as_deref(),
                title.flatten().as_deref(),
                honor,
                &globals(&script),
            );
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
            // Both names ask-once: the performer's, and ours for the "is the target me?" test.
            let performer_name = names.chat_record(performer, &commands).map(str::to_string);
            let your_name = self_guid
                .0
                .and_then(|g| names.resolve(g, &commands).map(str::to_string));
            if performer_name.is_none() || your_name.is_none() {
                return Built::Held(ChatLine::TextEmote {
                    performer,
                    text_id,
                    target,
                });
            }
            // No performer name, no line (not "Unknown"): the reference bails when its
            // player-only name cache misses; the animation rode `EmoteMessage`.
            let (Some(performer_name), Some(cat)) = (performer_name, emote_texts.as_deref()) else {
                debug!("chat: text emote {text_id} from {performer:#x} has no sentence source");
                return Built::Steps(steps);
            };
            let event = text_emote_event(
                &cat.0,
                text_id,
                &EmoteLine {
                    performer: &performer_name,
                    performer_is_you: self_guid.0 == Some(performer),
                    // Sex 1 is female, where the reference reads it (`record+0x13c`).
                    performer_female: names
                        .player_traits(performer)
                        .is_some_and(|(_, _, sex)| sex == 1),
                    target: &target,
                    your_name: your_name.as_deref().unwrap_or_default(),
                },
            );
            // No sentence is a real outcome: `/sit` prints nothing.
            let Some(event) = event else {
                return Built::Steps(steps);
            };
            steps.push(Step::Route(event));
        }
        ChatLine::Combat(mut line) => {
            // The unit names are read in the call (`0x50be09`, `0x50bce3`, `0x50de48`), never held;
            // guid 0 needs none.
            let mut wait = false;
            if line.subject != 0 {
                let store = guids.0.get(&line.subject).and_then(|e| stores.get(*e).ok());
                line.fills.subject = unit_name(&script, line.subject, store, &names, &commands);
            }
            // An item the server answered unknown does not wait: the line composes with what
            // the arm left in `named` (the reference falls back to `"UKNOWNOBJECT"`).
            match line.named {
                super::combat::Named::Ready => {}
                super::combat::Named::Item(entry) => {
                    match items.template(entry, 0, &commands).map(|t| t.name.clone()) {
                        Some(name) => line.fills.named = name,
                        None if !items.template_answered_unknown(entry) => wait = true,
                        None => {}
                    }
                }
                super::combat::Named::Unit(guid) => {
                    let store = guids.0.get(&guid).and_then(|e| stores.get(*e).ok());
                    line.fills.named = unit_name(&script, guid, store, &names, &commands);
                }
            }
            // Only the item waits, on its record's callback (`0x50bdb1` asks with `0x4fb970`).
            if wait {
                return Built::Held(ChatLine::Combat(line));
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
            Step::Fire(..) | Step::Speech { .. } => true,
        });
    }
    Built::Steps(steps)
}

/// The add's text passes on a line its caller composed, every one handed over with no conversation
/// (`[ebp+0x40]` 0, as at `0x50d8cf`): each quest link recoloured (`0x50a2e2` -> `0x50a2ec`-
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
        Step::Route(event) => route_in_world(world, script, &event, None),
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
            if deliver_before(script, &mut channels, &mut event) {
                crate::ui_script::safe_world::raise(
                    world,
                    "CHANNEL_PASSWORD_REQUEST",
                    password_request_args(&event),
                );
            }
            let after = deliver_mid(&mut world.resource_mut::<ChannelState>(), &mut event);
            route_in_world(world, script, &event, None);
            deliver_after(
                script,
                &mut world.resource_mut::<ChannelState>(),
                &event,
                after,
            );
        }
    }
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
    let mut windows = world.get_resource_mut::<ChatWindows>()?;
    windows.line_id = windows.line_id.wrapping_add(1);
    let line_id = windows.line_id;
    if let Some(mut record) = record {
        record.line_id = line_id;
        script.record_chat(record);
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
    ask_names(world, &line);
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
        world.resource_mut::<ChatLog>().pending.push((held, false));
    }
}

/// Queue a line its caller composes ahead of the add while the UI is outside the world: its text
/// built now, with the names as they read now, and queued as text (`0x509f92`); a step that is no
/// chat line (the discovery toast) runs at once. One still waiting on an item record stays held.
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

/// The add's name lookups ahead of its queue test (`0x509f48`): each player guid the line names
/// is asked with the answer's callback when missing (`0x509e51`, `0x509f33`), then a whisper's
/// sender entered in autocomplete with the name the add read ([`note_whisper`]).
fn ask_names(world: &mut World, line: &ChatLine) {
    let (Some(names), Some(commands)) = (
        world.get_resource::<NameCache>(),
        world.get_resource::<NetCommands>(),
    ) else {
        return;
    };
    // A player guid asks the name cache, a pet guid the pet-name cache (`0x509f33`, `0x50a06f`).
    for guid in line.player_guids() {
        if is_player_guid(guid) || is_pet_guid(guid) {
            let _ = names.chat_record(guid, commands);
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

/// A name answer's callback over the held lines (`0x503c80`), run as the answer lands: each line
/// whose names are now cached is added, in hold order (`0x503edb`), one naming a guid answered
/// empty is dropped (`0x503e5c`), and the lines queued for the world entry stay (`0x503e6f`).
pub(crate) fn name_landed(world: &mut World) {
    let Some(mut log) = world.get_resource_mut::<ChatLog>() else {
        return;
    };
    if log.pending.iter().all(|(_, queued)| *queued) {
        return;
    }
    let queue_chat = log.queue_chat;
    let held = std::mem::take(&mut log.pending);
    let mut kept = Vec::new();
    let self_guid = world.get_resource::<SelfGuid>().and_then(|g| g.0);
    for (line, queued) in held {
        if queued {
            kept.push((line, true));
            continue;
        }
        let (missing, ready) = world
            .get_resource::<NameCache>()
            .map_or((false, false), |names| {
                (
                    line.names_missing(names),
                    line.names_ready(names, self_guid),
                )
            });
        if missing {
            if matches!(line, ChatLine::Notice { notice, .. } if notice == channel_notice::INVITE) {
                world.resource_mut::<ChatLog>().invite_waiting = false;
            }
            continue;
        }
        // Outside the world a ready line is marked queued for the entry's walk (`0x503e78`).
        if queue_chat {
            kept.push((line, ready));
            continue;
        }
        // The callback hands the add the records' bare names (`0x503e87`-`0x503e98`) and calls it
        // again (`0x503edb`).
        let line = with_read_names(world, line, false);
        if let ChatLine::Wire { msg } = &line {
            note_whisper(world, msg);
        }
        if let Some(back) = add_one(world, line) {
            kept.push((back, false));
        }
    }
    restore(world, kept);
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

/// The UI's world entry for chat (`0x5281b3` -> `0x50df80`): the add stops queueing (`0x50dfbd`)
/// and the queued lines go through it in order (`0x50dfc3`), one still missing a name held for
/// its answer.
pub(crate) fn enter_world(world: &mut World) {
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

/// The UI's entry, then the queued chat lines added and the held ones retried, as the handlers and
/// the name answers would: a test's one pass over the chat add.
#[cfg(test)]
pub(crate) fn drain_for_test(world: &mut World) {
    enter_world(world);
    crate::ui_script::live::fire_handler_events(world);
    name_landed(world);
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
