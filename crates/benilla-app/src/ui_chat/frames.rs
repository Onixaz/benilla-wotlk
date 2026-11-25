//! The chat router: [`route`] fires each [`ChatEvent`] as its `CHAT_MSG_*` event, which the stock
//! `ChatFrame_MessageEventHandler` composes and prints, and tees the composed line to the log
//! files. [`compose`] transcribes that composition for the log, every format resolved by key from
//! the player's `GlobalStrings.lua` as the handler resolves it.

use bevy::prelude::*;

use benilla_ui::strings::{fill, Arg};

#[cfg(test)]
use super::event::event_name;
#[cfg(test)]
use super::event::notice_token;
use super::event::{ChatEvent, ChatEventKind};

/// What the app keeps beside the stock chat frames: the log files and the line id.
#[derive(Resource, Default)]
pub(crate) struct ChatWindows {
    /// The `LoggingChat` file, teed in [`route`].
    pub logs: super::logging::ChatLogFile,
    /// 3.3.5a's line id, the last one handed out as a fire's arg11.
    pub line_id: u32,
}

/// Fire one event's `CHAT_MSG_*` at the VM, where the stock handler prints it, then tee its line to
/// the chat log, which the reference composes C-side after the event, not from Lua (`0x50a975`).
#[cfg(test)]
pub(crate) fn route(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    event: &ChatEvent,
) {
    let Some(kind) = event.kind else {
        warn!("chat: unroutable event (no kind): {:?}", event.text);
        return;
    };
    windows.line_id = windows.line_id.wrapping_add(1);
    script.fire_event(
        event_name(kind),
        event.script_args_with_line(windows.line_id),
    );
    if script.logging_flags().0 {
        log_added(windows, event, kind, None, &|key| {
            script.lua().globals().get::<String>(key).ok()
        });
    }
}

/// The chat log's line for an added line, written only while the log is open as the add finds it
/// after the event (`0x50a975`); `default_language` is the active player's default language id read
/// at the call, `None` with no active player.
pub(crate) fn log_added(
    windows: &mut ChatWindows,
    event: &ChatEvent,
    kind: ChatEventKind,
    default_language: Option<u32>,
    get: &dyn Fn(&str) -> Option<String>,
) {
    if windows.logs.0.is_open() {
        if let Some(line) = log_line(event, kind, default_language, get) {
            windows.logs.0.record(line.as_bytes());
        }
    }
}

/// A chat line an app handler or step adds in its turn (`0x509dd0`): recorded on `chat_type` (`0x50a8a5`) and its event raised with the `World` lent (`0x4fdbc0` at `0x4fdd5c`), queued while the UI is outside the world.
pub(crate) fn add_chat_line_now(world: &mut World, chat_type: u8, text: String) {
    super::feed::add(
        world,
        ChatEvent::text_only(kind_of_type(chat_type), text).into(),
    );
}

/// The message display's chat line added inside a verb's call (`0x52183c` -> `0x509dd0`), on the
/// catalog row's chat type: 26 a skill line, 0 a system line, the catalog's only two.
pub(crate) fn add_chat_line_in_call(world: &mut World, chat_type: u8, text: String) {
    super::feed::add_now(
        world,
        ChatEvent::text_only(kind_of_type(chat_type), text).into(),
    );
}

/// A combat-family line an app handler adds at its step, the faction lines' (`0x50b8c0`, `0x50ba10` -> `0x509dd0`): composed off the VM's strings, then added as [`add_chat_line_now`] adds one.
pub(crate) fn add_combat_line_now(world: &mut World, line: super::combat::PendingCombat) {
    super::feed::add(world, super::feed::ChatLine::Combat(Box::new(line)));
}

/// A line's chat type as the event kind: 26 a skill line, 27 a loot line, any other a system line.
fn kind_of_type(chat_type: u8) -> ChatEventKind {
    match chat_type {
        26 => ChatEventKind::Skill,
        27 => ChatEventKind::Loot,
        _ => ChatEventKind::System,
    }
}

#[cfg(test)]
/// `ChatFrame_MessageEventHandler`'s composition (`ChatFrame.lua:2753-2937`) for the line that
/// carries `line_id` as arg11; `None` where it prints nothing (MODE_CHANGE, a GM's whisper). Its
/// frame-state reads stay out: the channel and whisper filters, `{icon}` tags, the timestamp and
/// class-coloured names, which `GetColoredName` gives only under a `colorNameByClass` the
/// reference ships off.
pub(crate) fn compose(
    event: &ChatEvent,
    kind: ChatEventKind,
    default_language: &str,
    line_id: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use ChatEventKind as K;
    let ty = chat_type(kind);
    Some(match kind {
        // `AddMessage(arg1)` (`:2753-2761`): the plain kinds and every `COMBAT_`, `SPELL_` and
        // `BG_SYSTEM_` line.
        K::System
        | K::Skill
        | K::Loot
        | K::Money
        | K::Opening
        | K::TargetIcons
        | K::Tradeskills
        | K::PetInfo
        | K::CombatXpGain
        | K::CombatHonorGain
        | K::CombatMiscInfo
        | K::CombatFactionChange
        | K::BgSystemNeutral
        | K::BgSystemAlliance
        | K::BgSystemHorde => event.text.clone(),
        // `format(arg1, "|Hplayer:"..arg2.."|h".."["..coloredName.."]".."|h")` (`:2762-2765`).
        K::Achievement | K::GuildAchievement => fill(
            &event.text,
            &[Arg::S(&format!("|Hplayer:{0}|h[{0}]|h", event.sender))],
        ),
        // `format(CHAT_IGNORED, arg2)` and `format(CHAT_FILTERED, arg2)` (`:2766-2769`).
        // `AddMessage(CHAT_RESTRICTED)` (`:2770-2771`).
        K::Restricted => get("CHAT_RESTRICTED").unwrap_or_default(),
        K::Ignored => fill(
            &get("CHAT_IGNORED").unwrap_or_default(),
            &[Arg::S(&event.sender)],
        ),
        K::Filtered => fill(
            &get("CHAT_FILTERED").unwrap_or_default(),
            &[Arg::S(&event.sender)],
        ),
        // `format(CHAT_CHANNEL_LIST_GET..arg1, tonumber(arg8), arg4)`, or arg1 bare without a
        // channel (`:2772-2777`).
        K::ChannelList if event.channel.is_empty() => event.text.clone(),
        K::ChannelList => fill(
            &format!(
                "{}{}",
                get("CHAT_CHANNEL_LIST_GET").unwrap_or_default(),
                event.text
            ),
            &[
                Arg::D(i64::from(event.channel_number)),
                Arg::S(&event.channel),
            ],
        ),
        K::ChannelNotice | K::ChannelNoticeUser => {
            return compose_notice(event, kind, get);
        }
        // Everything else is the speech branch (`:2840-2928`).
        _ => {
            // `pflag` (`:2851-2868`): a GM or DEV line wears the Blizzard icon, and a GM's whisper
            // goes to the GM chat addon instead of this window.
            let pflag = match event.flag.as_str() {
                "" => String::new(),
                "GM" if kind == K::Whisper => return None,
                "GM" | "DEV" => BLIZZARD_ICON.to_string(),
                flag => get(&format!("CHAT_FLAG_{flag}")).unwrap_or_default(),
            };
            // `MONSTER` and `RAID_BOSS` lines take no link, and only the others escape a typed
            // `%` (`:2873-2878`): a monster line's own `%s` is the name's slot.
            let show_link = !ty.starts_with("MONSTER") && !ty.starts_with("RAID_BOSS");
            let text = if show_link {
                event.text.replace('%', "%%")
            } else {
                event.text.clone()
            };
            let group = chat_category(ty);
            let target = match group {
                "CHANNEL" => Some(event.channel_number.to_string()),
                "WHISPER" => Some(event.sender.to_uppercase()),
                _ => None,
            };
            let player_link = format!(
                "|Hplayer:{}:{line_id}:{group}{}|h",
                event.sender,
                target.map(|t| format!(":{t}")).unwrap_or_default()
            );
            let get_pattern = get(&format!("CHAT_{ty}_GET")).unwrap_or_default();
            let sender = event.sender.as_str();
            let body = if !event.language.is_empty()
                && event.language != "Universal"
                && event.language != default_language
            {
                // The language header (`:2901-2907`).
                let pattern = format!("{get_pattern}[{}] {text}", event.language);
                if show_link && !sender.is_empty() {
                    fill(
                        &pattern,
                        &[Arg::S(&format!("{pflag}{player_link}[{sender}]|h"))],
                    )
                } else {
                    fill(&pattern, &[Arg::S(&format!("{pflag}{sender}"))])
                }
            } else if !show_link || sender.is_empty() {
                fill(
                    &format!("{get_pattern}{text}"),
                    &[Arg::S(&format!("{pflag}{sender}")), Arg::S(sender)],
                )
            } else if kind == K::Emote {
                fill(
                    &format!("{get_pattern}{text}"),
                    &[Arg::S(&format!("{pflag}{player_link}{sender}|h"))],
                )
            } else if kind == K::TextEmote {
                // `gsub(arg1, arg2, …, 1)`: the sentence's first mention of the performer.
                text.replacen(sender, &format!("{pflag}{player_link}{sender}|h"), 1)
            } else {
                fill(
                    &format!("{get_pattern}{text}"),
                    &[Arg::S(&format!("{pflag}{player_link}[{sender}]|h"))],
                )
            };
            // The channel link (`:2922-2928`): arg4 numbered, its zone tail cut.
            if event.channel.is_empty() {
                body
            } else {
                format!(
                    "|Hchannel:channel:{}|h[{}]|h {body}",
                    event.channel_number,
                    strip_zone(&event.channel)
                )
            }
        }
    })
}

/// The chat log's line for one added line, composed C-side after its event while logging is on
/// (`0x50a975`): the channel list's prefix (`0x50a9fc`), a notice's `CHAT_<TOKEN>_NOTICE` filled
/// as the window fills it (`0x50aa65`, see docs/DEVIATIONS.md), the IGNORED, FILTERED and
/// RESTRICTED texts (`0x50a998`, `0x50a9c1`, `0x50a9e9`), else the speech line (`0x50ab09`): a
/// `[channel] ` prefix cut at its zone, `CHAT_<TYPE>_GET` filled with the sender, the language
/// header, then the text with its links collapsed to their bracketed name and `%` doubled outside
/// the monster types, the whole written as a format with the sender (`0x50ad3f`).
pub(crate) fn log_line(
    event: &ChatEvent,
    kind: ChatEventKind,
    default_language: Option<u32>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use ChatEventKind as K;
    let sender = event.sender.as_str();
    let channel = event.channel.as_str();
    Some(match kind {
        K::ChannelList => {
            let prefix = if channel.is_empty() {
                String::new()
            } else {
                let base = if event.channel_base.is_empty() {
                    channel
                } else {
                    event.channel_base.as_str()
                };
                fill(
                    &get("CHAT_CHANNEL_LIST_GET").unwrap_or_default(),
                    &[Arg::D(i64::from(event.zone_channel_id)), Arg::S(base)],
                )
            };
            format!("{prefix}{}", event.text)
        }
        K::ChannelNotice | K::ChannelNoticeUser => {
            let token = match event.notice_byte() {
                Some(byte) => super::event::notice_token(byte, event.slot_state, event.suspended)?,
                None => event.text.as_str(),
            };
            // 12340 fills this with the "%d. %s" buffer's address and the sender (`0x50aa65`),
            // printing a stack address and an empty bracket; benilla fills it as the window does.
            fill(
                &get(&format!("CHAT_{token}_NOTICE")).unwrap_or_default(),
                &notice_args(event, kind, token),
            )
        }
        // A missing string reads as the lookup's empty default (`0x819d4c`), still written.
        K::Ignored => fill(&get("CHAT_IGNORED").unwrap_or_default(), &[Arg::S(sender)]),
        K::Filtered => fill(&get("CHAT_FILTERED").unwrap_or_default(), &[Arg::S(sender)]),
        K::Restricted => fill(&get("CHAT_RESTRICTED").unwrap_or_default(), &[]),
        _ => {
            // `"[%s] "`, its first `" - "` replaced by `"] "` and the rest cut (`0x50ab19`-`0x50ab52`).
            let prefix = if channel.is_empty() {
                String::new()
            } else {
                match channel.find(" - ") {
                    Some(i) => format!("[{}] ", &channel[..i]),
                    None => format!("[{channel}] "),
                }
            };
            let ty = chat_type(kind);
            let mut header = fill(
                &get(&format!("CHAT_{ty}_GET")).unwrap_or_default(),
                &[Arg::S(sender)],
            );
            // The language record's name (`0x50abd6`) when it is set, a player is active and it
            // is not that player's default read now (`0x50abb7`-`0x50ac06`).
            // An id with no record faults 12340 at `0x50abd6` (a null record); benilla writes `[] `.
            if event.language_id != 0 && default_language.is_some_and(|d| d != event.language_id) {
                header.push_str(&format!("[{}] ", event.language));
            }
            let monster = matches!(
                kind,
                K::MonsterSay
                    | K::MonsterParty
                    | K::MonsterYell
                    | K::MonsterWhisper
                    | K::MonsterEmote
                    | K::RaidBossEmote
                    | K::RaidBossWhisper
            );
            let line = format!(
                "{prefix}{header}{}",
                log_text(&benilla_ui::language::process(&event.text), !monster)
            );
            fill(&line, &[Arg::S(sender)])
        }
    })
}

/// The log's copy of a line's text (`0x50ac60`-`0x50ad28`): `||` is one `|`, any other escape skips
/// to the next `[` and keeps what lies up to its `]`, dropping a `|h` then a `|r` after it, and a
/// `%` doubles when `escape`.
fn log_text(text: &str, escape: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '|' {
            if rest[1..].starts_with('|') {
                out.push('|');
                rest = &rest[2..];
                continue;
            }
            // No `[` after it ends the copy (`0x50ac91` -> `0x50ad30`), so a colour-only line logs empty.
            let Some(open) = rest.find('[') else {
                break;
            };
            rest = &rest[open..];
            loop {
                let Some(c) = rest.chars().next() else {
                    return out;
                };
                rest = &rest[c.len_utf8()..];
                match c {
                    '[' => {}
                    ']' => {
                        rest = rest.strip_prefix("|h").unwrap_or(rest);
                        rest = rest.strip_prefix("|r").unwrap_or(rest);
                        break;
                    }
                    c => out.push(c),
                }
            }
            continue;
        }
        if c == '%' && escape {
            out.push('%');
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[cfg(test)]
/// The pflag of a GM or DEV line (`ChatFrame.lua:2859`, `:2862`).
const BLIZZARD_ICON: &str = "|TInterface\\ChatFrame\\UI-ChatIcon-Blizz.blp:0:2:0:-3|t ";

/// The `ChatTypeInfo` key, `strsub(event, 10)` (`ChatFrame.lua:2677`).
fn chat_type(kind: ChatEventKind) -> &'static str {
    super::event::type_name(kind)
}

#[cfg(test)]
/// `Chat_GetChatCategory` over `CHAT_CATEGORY_LIST` (`ChatFrame.lua:268-288`).
fn chat_category(ty: &str) -> &str {
    match ty {
        "PARTY_LEADER" | "PARTY_GUIDE" | "MONSTER_PARTY" => "PARTY",
        "RAID_LEADER" | "RAID_WARNING" => "RAID",
        "GUILD_ACHIEVEMENT" => "GUILD",
        "WHISPER_INFORM" | "AFK" | "DND" => "WHISPER",
        "CHANNEL_JOIN" | "CHANNEL_LEAVE" | "CHANNEL_NOTICE" | "CHANNEL_USER" => "CHANNEL",
        "BATTLEGROUND_LEADER" => "BATTLEGROUND",
        "BN_WHISPER_INFORM" => "BN_WHISPER",
        "BN_CONVERSATION_NOTICE" | "BN_CONVERSATION_LIST" => "BN_CONVERSATION",
        _ => ty,
    }
}

#[cfg(test)]
/// `gsub(arg4, "%s%-%s.*", "")`: "2. Trade - City" → "2. Trade". Only speech lines strip it
/// (`ChatFrame.lua:2923`); the notice and list arms print arg4 whole, as in
/// "Joined Channel: [1. General - Elwynn Forest]".
fn strip_zone(channel: &str) -> &str {
    match channel.find(" - ") {
        Some(i) => &channel[..i],
        None => channel,
    }
}

#[cfg(test)]
/// A channel notice: the notice byte's token ([`notice_token`]) names `CHAT_<TOKEN>_NOTICE_BN` or
/// else `CHAT_<TOKEN>_NOTICE` (`ChatFrame.lua:2778-2802`), filled with arg8 and arg4, then for
/// CHANNEL_NOTICE_USER arg2 and any arg5; INVITE takes arg4 and arg2 alone. `None` for MODE_CHANGE
/// and bytes past the table, which print nothing.
pub(crate) fn compose_notice(
    event: &ChatEvent,
    kind: ChatEventKind,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let token = match event.notice_byte() {
        Some(byte) => notice_token(byte, event.slot_state, event.suspended)?,
        None => event.text.as_str(),
    };
    let template =
        get(&format!("CHAT_{token}_NOTICE_BN")).or_else(|| get(&format!("CHAT_{token}_NOTICE")))?;
    let line = fill(&template, &notice_args(event, kind, token));
    (!line.is_empty()).then_some(line)
}

/// The window's fill for a `CHAT_<TOKEN>_NOTICE` (`ChatFrame.lua:2778-2802`): arg8 and arg4, then
/// for CHANNEL_NOTICE_USER arg2 and any arg5; INVITE takes arg4 and arg2 alone.
fn notice_args<'a>(event: &'a ChatEvent, kind: ChatEventKind, token: &str) -> Vec<Arg<'a>> {
    let number = Arg::D(i64::from(event.channel_number));
    let channel = Arg::S(event.channel.as_str());
    let sender = Arg::S(event.sender.as_str());
    if kind == ChatEventKind::ChannelNotice {
        vec![number, channel]
    } else if !event.target.is_empty() {
        vec![number, channel, sender, Arg::S(event.target.as_str())]
    } else if token == "INVITE" {
        vec![channel, sender]
    } else {
        vec![number, channel, sender]
    }
}
