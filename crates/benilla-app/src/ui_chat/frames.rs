//! The chat router: [`route`] fires each [`ChatEvent`] as its `CHAT_MSG_*` event, which the stock
//! `ChatFrame_MessageEventHandler` composes and prints, and tees the composed line to the log
//! files. [`compose`] transcribes that composition for the log, every format resolved by key from
//! the player's `GlobalStrings.lua` as the handler resolves it.

use bevy::prelude::*;

use benilla_ui::strings::{fill, Arg};

use super::event::{event_name, notice_token, ChatEvent, ChatEventKind};

/// What the app keeps beside the stock chat frames: the default language and the log files.
#[derive(Resource, Default)]
pub(crate) struct ChatWindows {
    /// The frame's `this.defaultLanguage`, the faction tongue `GetDefaultLanguage()` answers;
    /// empty until the player and `Languages.dbc` are loaded.
    pub default_language: String,
    /// The `LoggingChat`/`LoggingCombat` files, teed in [`route`].
    pub logs: super::logging::ChatLogFiles,
    /// 3.3.5a's line id, the last one handed out as a fire's arg11.
    pub line_id: u32,
}

/// Fire one event's `CHAT_MSG_*` at the VM, where the stock handler prints it, and tee its
/// composed line to the log files, which the reference writes C-side, not from Lua.
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
    let default_language = windows.default_language.clone();
    // The VM's own globals, so the log line matches the window's.
    if let Some(line) = compose(event, kind, &default_language, windows.line_id, &|key| {
        script.lua().globals().get::<String>(key).ok()
    }) {
        windows.logs.record(kind.is_combat_log(), &line);
    }
    let args = event.script_args_with_line(windows.line_id);
    script.fire_event(event_name(kind), args);
}

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

/// The pflag of a GM or DEV line (`ChatFrame.lua:2859`, `:2862`).
const BLIZZARD_ICON: &str = "|TInterface\\ChatFrame\\UI-ChatIcon-Blizz.blp:0:2:0:-3|t ";

/// The `ChatTypeInfo` key, `strsub(event, 10)` (`ChatFrame.lua:2677`).
fn chat_type(kind: ChatEventKind) -> &'static str {
    event_name(kind)
        .strip_prefix("CHAT_MSG_")
        .unwrap_or_default()
}

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

/// `gsub(arg4, "%s%-%s.*", "")`: "2. Trade - City" → "2. Trade". Only speech lines strip it
/// (`ChatFrame.lua:2923`); the notice and list arms print arg4 whole, as in
/// "Joined Channel: [1. General - Elwynn Forest]".
fn strip_zone(channel: &str) -> &str {
    match channel.find(" - ") {
        Some(i) => &channel[..i],
        None => channel,
    }
}

/// A channel notice: the notice byte's token ([`notice_token`], the jump table at `0x49c60c`)
/// names `CHAT_<TOKEN>_NOTICE_BN` or else `CHAT_<TOKEN>_NOTICE` (`ChatFrame.lua:2778-2802`),
/// filled with arg8 and arg4, then for CHANNEL_NOTICE_USER arg2 and any arg5; INVITE takes arg4
/// and arg2 alone. `None` for MODE_CHANGE and bytes past the table, which print nothing.
pub(crate) fn compose_notice(
    event: &ChatEvent,
    kind: ChatEventKind,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let token = notice_token(event.notice_byte()?, event.slot_state)?;
    let template =
        get(&format!("CHAT_{token}_NOTICE_BN")).or_else(|| get(&format!("CHAT_{token}_NOTICE")))?;
    let number = Arg::D(i64::from(event.channel_number));
    let channel = Arg::S(event.channel.as_str());
    let sender = Arg::S(event.sender.as_str());
    let args = if kind == ChatEventKind::ChannelNotice {
        vec![number, channel]
    } else if !event.target.is_empty() {
        vec![number, channel, sender, Arg::S(event.target.as_str())]
    } else if token == "INVITE" {
        vec![channel, sender]
    } else {
        vec![number, channel, sender]
    };
    let line = fill(&template, &args);
    (!line.is_empty()).then_some(line)
}
