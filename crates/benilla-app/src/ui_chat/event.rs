//! [`ChatEvent`]: the reference's `CHAT_MSG_*` event and its `arg1..arg10`, typed. Every source,
//! the wire and the client-composed lines alike, produces one, and [`super::frames::route`] fires
//! it at the VM, where the stock `ChatFrame_OnEvent` prints it.

use benilla_ui::script::ScriptValue;

/// The chat-event kinds: the `ChatTypeInfo` keys benilla produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ChatEventKind {
    Say,
    Party,
    /// 3.3.5a's party line from the group's leader, a lane of its own (`0x33`).
    PartyLeader,
    Raid,
    Guild,
    Officer,
    Yell,
    Whisper,
    WhisperInform,
    Emote,
    TextEmote,
    System,
    MonsterSay,
    MonsterYell,
    MonsterEmote,
    MonsterWhisper,
    Channel,
    ChannelJoin,
    ChannelLeave,
    ChannelNotice,
    ChannelNoticeUser,
    ChannelList,
    Afk,
    Dnd,
    Ignored,
    Skill,
    Loot,
    Money,
    /// The client-composed XP line (`SMSG_LOG_XPGAIN`).
    CombatXpGain,
    /// The client-composed honor line (`SMSG_PVP_CREDIT`), built once the victim's name resolves.
    CombatHonorGain,
    RaidLeader,
    RaidWarning,
    RaidBossEmote,
    /// `CHAT_MSG_FILTERED` (`0x5B`), the server's "your message was filtered" notice; `arg2` is the
    /// addressee the stock frame formats `CHAT_FILTERED` over (`ChatFrame.lua:1406`).
    Filtered,
    Battleground,
    BattlegroundLeader,
    BgSystemNeutral,
    BgSystemAlliance,
    BgSystemHorde,
    /// 3.3.5a's award lines, the `$a` link already written.
    Achievement,
    GuildAchievement,
    // ── the lines beside the combat log ──────────────────────────────────────────────
    /// `CHAT_MSG_COMBAT_MISC_INFO`: the death's durability loss.
    CombatMiscInfo,
    /// `CHAT_MSG_COMBAT_FACTION_CHANGE`, a reputation delta.
    CombatFactionChange,
    /// `CHAT_MSG_TRADESKILLS`, a created item (`0x50bd90`).
    Tradeskills,
    /// `CHAT_MSG_PET_INFO`: a pet fed, dismissed or drained of happiness.
    PetInfo,
    /// `CHAT_MSG_OPENING`, a lock opened (`0x50dde0`).
    Opening,
}

impl ChatEventKind {
    /// The `COMBAT_` lines the stock frame prints verbatim by their prefix; `COMBAT_XP_GAIN` and
    /// `COMBAT_HONOR_GAIN`, composed apart, are outside this set.
    pub(crate) fn is_combat_log(self) -> bool {
        use ChatEventKind as K;
        matches!(self, K::CombatMiscInfo | K::CombatFactionChange)
    }

    /// Every kind, for the exhaustive sweeps; `ui_chat::tests::every_kind_is_in_all` fails when one
    /// is missing.
    #[cfg(test)]
    pub(crate) const ALL: &'static [ChatEventKind] = {
        use ChatEventKind as K;
        &[
            K::Say,
            K::Party,
            K::PartyLeader,
            K::Raid,
            K::Guild,
            K::Officer,
            K::Yell,
            K::Whisper,
            K::WhisperInform,
            K::Emote,
            K::TextEmote,
            K::System,
            K::MonsterSay,
            K::MonsterYell,
            K::MonsterEmote,
            K::MonsterWhisper,
            K::Channel,
            K::ChannelJoin,
            K::ChannelLeave,
            K::ChannelNotice,
            K::ChannelNoticeUser,
            K::ChannelList,
            K::Afk,
            K::Dnd,
            K::Ignored,
            K::Skill,
            K::Loot,
            K::Money,
            K::CombatXpGain,
            K::CombatHonorGain,
            K::RaidLeader,
            K::RaidWarning,
            K::RaidBossEmote,
            K::Filtered,
            K::Battleground,
            K::BattlegroundLeader,
            K::BgSystemNeutral,
            K::BgSystemAlliance,
            K::BgSystemHorde,
            K::Achievement,
            K::GuildAchievement,
            K::CombatMiscInfo,
            K::CombatFactionChange,
            K::Tradeskills,
            K::PetInfo,
            K::Opening,
        ]
    };
}

/// One `CHAT_MSG_*` fire, typed. The fire helper `0x49b0b0` signals it (`0x703f50`) with the
/// format `"%s%s%s%s%s%s%d%d%s%d"` (`.rdata 0x844608`): arg7, arg8 and arg10 are numbers, the
/// rest strings, never nil, as `ChatFrame_OnEvent` compares `arg7 > 0` bare.
///
/// | arg | field | what `ChatFrame_OnEvent` reads |
/// |---|---|---|
/// | 1 | `text`, `notice` | the body, or a channel notice's `CHAT_<token>_NOTICE` token |
/// | 2 | `sender` | the speaker, or a notice's affected player |
/// | 3 | `language` | a language name; empty prints no header |
/// | 4 | `channel` | the display form, "N. Name - Zone" when numbered |
/// | 5 | `target` | a two-name notice's second name, "X kicked by Y" |
/// | 6 | `flag` | "AFK", "DND" or "GM", as `CHAT_FLAG_<flag>` |
/// | 7 | `zone_channel_id` | the ChannelID, 0 for a custom channel (`ChatFrame.lua:1379`) |
/// | 8 | `channel_number` | the local slot, `ChatTypeInfo["CHANNEL"..arg8]` |
/// | 9 | `channel_base` | the channel name without its number |
/// | 10 | none | the split index, 0 from vmangos (`Chat/Channel.cpp:823-827`) |
///
/// arg4, arg7, arg8 and arg9 are one record ([`super::edit::ChannelState::stamp_channel`]). arg1
/// is the garbled text: the reference fills one buffer (`SStrCopy` at `0x49a9f0`, the garble
/// `0x49b560` at `0x49aa7c`) that the line, arg1 and the bubble share ([`super::language`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChatEvent {
    pub kind: Option<ChatEventKind>,
    pub text: String,
    pub sender: String,
    pub language: String,
    pub channel: String,
    pub target: String,
    pub flag: String,
    /// arg7, the ChannelID behind a zone channel (1 General, 2 Trade, 22 LocalDefense), else 0.
    pub zone_channel_id: u32,
    pub channel_number: u32,
    pub channel_base: String,
    pub notice: String,
    /// The speaker's guid on a wire line, 0 otherwise: 3.3.5a's arg12.
    pub sender_guid: u64,
    /// Our slot's state for this channel as the notice arrived (`None`: no slot), set before
    /// [`super::feed::deliver`] changes it: the reference's notice arms read `slot+0x9c` for the
    /// token (`0x49c0c2`) before they write it (`0x49bb20`).
    pub slot_state: Option<super::edit::SlotState>,
}

impl ChatEvent {
    /// A bare kind+text event (SYSTEM lines, loot receive lines, TEXT_EMOTE sentences).
    pub(crate) fn text_only(kind: ChatEventKind, text: String) -> Self {
        ChatEvent {
            kind: Some(kind),
            text,
            ..Default::default()
        }
    }

    /// The `SMSG_CHANNEL_NOTIFY` byte behind a channel notice, which `notice` holds in decimal.
    pub(crate) fn notice_byte(&self) -> Option<u8> {
        self.notice.parse().ok()
    }

    /// This event's `arg1..arg10` in the reference's order and types; ten values, never nil.
    pub(crate) fn script_args(&self) -> Vec<ScriptValue> {
        // arg1 is the token for a channel notice, else the body.
        let arg1 = match (self.kind, self.notice_byte()) {
            (Some(ChatEventKind::ChannelNotice | ChatEventKind::ChannelNoticeUser), Some(byte)) => {
                notice_token(byte, self.slot_state)
                    .unwrap_or_default()
                    .to_string()
            }
            _ => self.text.clone(),
        };
        vec![
            ScriptValue::Str(arg1),
            ScriptValue::Str(self.sender.clone()),
            ScriptValue::Str(self.language.clone()),
            ScriptValue::Str(self.channel.clone()),
            ScriptValue::Str(self.target.clone()),
            ScriptValue::Str(self.flag.clone()),
            ScriptValue::Int(i64::from(self.zone_channel_id)),
            ScriptValue::Int(i64::from(self.channel_number)),
            ScriptValue::Str(self.channel_base.clone()),
            // arg10: always 0, see the struct doc.
            ScriptValue::Int(0),
        ]
    }

    /// 3.3.5a's thirteen: [`Self::script_args`], then arg11 the line id, arg12 the sender's guid
    /// as `0x` and sixteen upper-case hex digits (`0x74d0d0`) when it is a player's (high nibble
    /// clear, `0x4fdc1b`) and else empty, and arg13 the Battle.net presence id, 0 for a realm
    /// line.
    pub(crate) fn script_args_with_line(&self, line_id: u32) -> Vec<ScriptValue> {
        let guid = self.sender_guid;
        let player = guid >> 60 == 0 && guid & 0xf07f_ffff_ffff_ffff != 0;
        let mut args = self.script_args();
        args.push(ScriptValue::Int(i64::from(line_id)));
        args.push(ScriptValue::Str(if player {
            format!("0x{guid:016X}")
        } else {
            String::new()
        }));
        args.push(ScriptValue::Int(0));
        args
    }
}

/// The reference's event name for a kind, `CHAT_MSG_` and its `ChatTypeInfo` key, which
/// `ChatFrame_OnEvent` recovers with `strsub(event, 10)`.
pub(crate) fn event_name(kind: ChatEventKind) -> &'static str {
    use ChatEventKind as K;
    match kind {
        K::Say => "CHAT_MSG_SAY",
        K::Party => "CHAT_MSG_PARTY",
        K::PartyLeader => "CHAT_MSG_PARTY_LEADER",
        K::Raid => "CHAT_MSG_RAID",
        K::Guild => "CHAT_MSG_GUILD",
        K::Officer => "CHAT_MSG_OFFICER",
        K::Yell => "CHAT_MSG_YELL",
        K::Whisper => "CHAT_MSG_WHISPER",
        K::WhisperInform => "CHAT_MSG_WHISPER_INFORM",
        K::Emote => "CHAT_MSG_EMOTE",
        K::TextEmote => "CHAT_MSG_TEXT_EMOTE",
        K::System => "CHAT_MSG_SYSTEM",
        K::MonsterSay => "CHAT_MSG_MONSTER_SAY",
        K::MonsterYell => "CHAT_MSG_MONSTER_YELL",
        K::MonsterEmote => "CHAT_MSG_MONSTER_EMOTE",
        K::MonsterWhisper => "CHAT_MSG_MONSTER_WHISPER",
        K::Channel => "CHAT_MSG_CHANNEL",
        K::ChannelJoin => "CHAT_MSG_CHANNEL_JOIN",
        K::ChannelLeave => "CHAT_MSG_CHANNEL_LEAVE",
        K::ChannelNotice => "CHAT_MSG_CHANNEL_NOTICE",
        K::ChannelNoticeUser => "CHAT_MSG_CHANNEL_NOTICE_USER",
        K::ChannelList => "CHAT_MSG_CHANNEL_LIST",
        K::Afk => "CHAT_MSG_AFK",
        K::Dnd => "CHAT_MSG_DND",
        K::Ignored => "CHAT_MSG_IGNORED",
        K::Skill => "CHAT_MSG_SKILL",
        K::Loot => "CHAT_MSG_LOOT",
        K::Money => "CHAT_MSG_MONEY",
        K::CombatXpGain => "CHAT_MSG_COMBAT_XP_GAIN",
        K::CombatHonorGain => "CHAT_MSG_COMBAT_HONOR_GAIN",
        K::RaidLeader => "CHAT_MSG_RAID_LEADER",
        K::RaidWarning => "CHAT_MSG_RAID_WARNING",
        K::RaidBossEmote => "CHAT_MSG_RAID_BOSS_EMOTE",
        K::Filtered => "CHAT_MSG_FILTERED",
        K::Battleground => "CHAT_MSG_BATTLEGROUND",
        K::BattlegroundLeader => "CHAT_MSG_BATTLEGROUND_LEADER",
        K::BgSystemNeutral => "CHAT_MSG_BG_SYSTEM_NEUTRAL",
        K::BgSystemAlliance => "CHAT_MSG_BG_SYSTEM_ALLIANCE",
        K::BgSystemHorde => "CHAT_MSG_BG_SYSTEM_HORDE",
        K::Achievement => "CHAT_MSG_ACHIEVEMENT",
        K::GuildAchievement => "CHAT_MSG_GUILD_ACHIEVEMENT",
        K::CombatMiscInfo => "CHAT_MSG_COMBAT_MISC_INFO",
        K::CombatFactionChange => "CHAT_MSG_COMBAT_FACTION_CHANGE",
        K::Tradeskills => "CHAT_MSG_TRADESKILLS",
        K::PetInfo => "CHAT_MSG_PET_INFO",
        K::Opening => "CHAT_MSG_OPENING",
    }
}

/// The `arg1` token for a `SMSG_CHANNEL_NOTIFY` byte, read as `CHAT_<token>_NOTICE`
/// (`GlobalStrings.lua:494-745`), matching the client's jump table `0x49c60c`. `None`: the
/// member-line bytes `0x00`/`0x01`, MODE_CHANGE `0x0C`, which fires no event (`0x49c24d`), and
/// anything past `0x1F`. `0x02` answers `YOU_CHANGED` in state 2 (`0x49c087`); `0x03` answers
/// `SUSPENDED` in state 3 (`0x49c0e0`), where `YOU_LEFT` would delete the window's channel.
pub(crate) fn notice_token(
    byte: u8,
    state: Option<super::edit::SlotState>,
) -> Option<&'static str> {
    use super::edit::SlotState;
    use benilla_protocol::messages::channel_notice as n;
    Some(match byte {
        // A border crossing prints "Changed Channel: [1. General - Westfall]".
        n::YOU_JOINED if state == Some(SlotState::Renamed) => "YOU_CHANGED",
        n::YOU_JOINED => "YOU_JOINED",
        // Walking out of a capital keeps the record, so not the token that deletes it.
        n::YOU_LEFT if state == Some(SlotState::Suspended) => "SUSPENDED",
        n::YOU_LEFT => "YOU_LEFT",
        n::WRONG_PASSWORD => "WRONG_PASSWORD",
        n::NOT_MEMBER => "NOT_MEMBER",
        n::NOT_MODERATOR => "NOT_MODERATOR",
        n::PASSWORD_CHANGED => "PASSWORD_CHANGED",
        n::OWNER_CHANGED => "OWNER_CHANGED",
        n::PLAYER_NOT_FOUND => "PLAYER_NOT_FOUND",
        n::NOT_OWNER => "NOT_OWNER",
        n::CHANNEL_OWNER => "CHANNEL_OWNER",
        n::ANNOUNCEMENTS_ON => "ANNOUNCEMENTS_ON",
        n::ANNOUNCEMENTS_OFF => "ANNOUNCEMENTS_OFF",
        n::MODERATION_ON => "MODERATION_ON",
        n::MODERATION_OFF => "MODERATION_OFF",
        n::MUTED => "MUTED",
        n::PLAYER_KICKED => "PLAYER_KICKED",
        n::BANNED => "BANNED",
        n::PLAYER_BANNED => "PLAYER_BANNED",
        n::PLAYER_UNBANNED => "PLAYER_UNBANNED",
        n::PLAYER_NOT_BANNED => "PLAYER_NOT_BANNED",
        n::PLAYER_ALREADY_MEMBER => "PLAYER_ALREADY_MEMBER",
        n::INVITE => "INVITE",
        n::INVITE_WRONG_FACTION => "INVITE_WRONG_FACTION",
        n::WRONG_FACTION => "WRONG_FACTION",
        n::INVALID_NAME => "INVALID_NAME",
        n::NOT_MODERATED => "NOT_MODERATED",
        n::PLAYER_INVITED => "PLAYER_INVITED",
        n::PLAYER_INVITE_BANNED => "PLAYER_INVITE_BANNED",
        n::THROTTLED => "THROTTLED",
        // 3.3.5a's tokens (`0x9fcfb8`, `0x9fcfac`, `0x9fcfa0`, `0x9fcf94`).
        n::NOT_IN_AREA => "NOT_IN_AREA",
        n::NOT_IN_LFG => "NOT_IN_LFG",
        n::VOICE_ON => "VOICE_ON",
        n::VOICE_OFF => "VOICE_OFF",
        _ => return None,
    })
}

/// The kind's row in the shipped default colour table (`0x9fa558`), which the chat bubble and the
/// edit box header tint with; the window's line colour is `ChatTypeInfo`'s.
pub(crate) fn default_color(kind: ChatEventKind) -> [u8; 3] {
    let key = event_name(kind)
        .strip_prefix("CHAT_MSG_")
        .unwrap_or_default();
    benilla_ui::script::default_chat_color(key)
        .unwrap_or_else(|| panic!("{key} is a fixed row of the reference's chat type table"))
}

/// Map a wire `ChatMsg` byte to its kind; `None` for a type vmangos never sends as wire chat (the
/// combat log) or one not modelled, which the router drops loudly.
pub(crate) fn kind_of_wire(chat_type: u8) -> Option<ChatEventKind> {
    use benilla_protocol::messages as m;
    use ChatEventKind as K;
    Some(match chat_type {
        m::CHAT_MSG_SAY => K::Say,
        m::CHAT_MSG_PARTY => K::Party,
        m::CHAT_MSG_PARTY_LEADER => K::PartyLeader,
        m::CHAT_MSG_RAID => K::Raid,
        m::CHAT_MSG_GUILD => K::Guild,
        m::CHAT_MSG_OFFICER => K::Officer,
        m::CHAT_MSG_YELL => K::Yell,
        m::CHAT_MSG_WHISPER => K::Whisper,
        m::CHAT_MSG_WHISPER_INFORM => K::WhisperInform,
        m::CHAT_MSG_EMOTE => K::Emote,
        m::CHAT_MSG_SYSTEM => K::System,
        m::CHAT_MSG_MONSTER_SAY => K::MonsterSay,
        m::CHAT_MSG_MONSTER_YELL => K::MonsterYell,
        m::CHAT_MSG_MONSTER_EMOTE => K::MonsterEmote,
        m::CHAT_MSG_CHANNEL => K::Channel,
        m::CHAT_MSG_AFK => K::Afk,
        m::CHAT_MSG_DND => K::Dnd,
        m::CHAT_MSG_IGNORED => K::Ignored,
        m::CHAT_MSG_MONSTER_WHISPER | m::CHAT_MSG_RAID_BOSS_WHISPER => K::MonsterWhisper,
        m::CHAT_MSG_RAID_LEADER => K::RaidLeader,
        m::CHAT_MSG_RAID_WARNING => K::RaidWarning,
        m::CHAT_MSG_RAID_BOSS_EMOTE => K::RaidBossEmote,
        m::CHAT_MSG_FILTERED => K::Filtered,
        m::CHAT_MSG_BATTLEGROUND => K::Battleground,
        m::CHAT_MSG_BATTLEGROUND_LEADER => K::BattlegroundLeader,
        m::CHAT_MSG_BG_SYSTEM_NEUTRAL => K::BgSystemNeutral,
        m::CHAT_MSG_BG_SYSTEM_ALLIANCE => K::BgSystemAlliance,
        m::CHAT_MSG_BG_SYSTEM_HORDE => K::BgSystemHorde,
        m::CHAT_MSG_ACHIEVEMENT => K::Achievement,
        m::CHAT_MSG_GUILD_ACHIEVEMENT => K::GuildAchievement,
        _ => return None,
    })
}

/// The arg6 flag token for a wire chat-tag byte (vmangos `Player::GetChatTag`), read as
/// `CHAT_FLAG_<flag>`.
pub(crate) fn flag_of_tag(chat_tag: u8) -> &'static str {
    use benilla_protocol::messages::chat_tag as t;
    match chat_tag {
        t::AFK => "AFK",
        t::DND => "DND",
        t::GM => "GM",
        _ => "",
    }
}

/// The 1.12 language names by wire id (vmangos `SharedDefines.h`, `LANG_*`), for the `[Language]`
/// header. Language 0 has no `Languages.dbc` row, so the stock `arg3 ~= "Universal"` test never
/// fires: an empty arg3 is what suppresses the header (`ChatFrame.lua:1442`).
pub(crate) fn language_name(id: u32) -> &'static str {
    match id {
        1 => "Orcish",
        2 => "Darnassian",
        3 => "Taurahe",
        6 => "Dwarvish",
        7 => "Common",
        8 => "Demonic",
        9 => "Titan",
        10 => "Thalassian",
        11 => "Draconic",
        12 => "Kalimag",
        13 => "Gnomish",
        14 => "Troll",
        33 => "Gutterspeak",
        _ => "", // 0, Universal, and unknown ids: no header
    }
}

#[cfg(test)]
mod line_args_tests {
    use super::*;

    fn say(sender_guid: u64) -> ChatEvent {
        ChatEvent {
            sender_guid,
            ..ChatEvent::text_only(ChatEventKind::Say, "hello".into())
        }
    }

    /// 1.12.1 fires ten; 3.3.5a appends the line id, the player's guid string and presence 0.
    #[test]
    fn a_3_3_5a_fire_appends_the_line_id_guid_and_presence() {
        assert_eq!(say(0x21).script_args().len(), 10);
        let args = say(0x21).script_args_with_line(7);
        assert_eq!(args.len(), 13);
        assert_eq!(args[10], ScriptValue::Int(7));
        assert_eq!(args[11], ScriptValue::Str("0x0000000000000021".into()));
        assert_eq!(args[12], ScriptValue::Int(0));
    }

    /// A creature's or a system line's guid stays out of arg12.
    #[test]
    fn only_a_player_guid_fills_arg12() {
        let creature = say(0xF130_0000_0001_0002).script_args_with_line(1);
        assert_eq!(creature[11], ScriptValue::Str(String::new()));
        let system = say(0).script_args_with_line(1);
        assert_eq!(system[11], ScriptValue::Str(String::new()));
    }
}
