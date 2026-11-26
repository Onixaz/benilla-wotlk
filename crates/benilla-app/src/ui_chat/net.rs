//! The chat window's packet handlers: the spoken line, and the notices and answers that print as
//! chat. `SMSG_NOTIFICATION` and `SMSG_AREA_TRIGGER_MESSAGE` are mostly `UIErrorsFrame` toasts,
//! shown in the handler's turn through [`TextSink`].

use crate::ui_chat::ChatSink as _;
use benilla_protocol::messages::ChatMessage;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{Broadcast, ChatEvent, ChatEventKind, ChatLog};
use crate::net::{
    ClientCommand, GuidIndex, NetCommands, NetHandlerApp, SelfGuid, ServerSaidMessage,
};
use crate::ui_action::signals::SignalSink;
use crate::ui_action::{TextSink, UiError};
use crate::ui_script::live::UiEvents;
use crate::ui_social::SocialState;
use benilla_ui::messages::MsgKind;

/// Register the chat window's handlers and its session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::Chat, on_chat)
        .net_handler(K::ChannelList, on_channel_list)
        .net_handler(K::ChannelUserList, on_channel_user_list)
        .net_handler(K::ChatPlayerNotFound, on_whisper_refusal)
        .net_handler(K::ChatWrongFaction, on_whisper_refusal)
        .net_handler(K::ChatNotInParty, on_chat_not_in_party)
        .net_handler(K::ZoneUnderAttack, on_broadcast)
        .net_handler(K::DefenseMessage, on_broadcast)
        .net_handler(K::ServerMessage, on_broadcast)
        .net_handler(K::ChatRestricted, on_chat_restricted)
        .net_handler(K::Notification, on_toast)
        .net_handler(K::AreaTriggerMessage, on_toast)
        .net_handler(K::PlayedTime, on_played_time)
        .net_handler(K::ChannelNotify, on_channel_notify)
        .net_handler(K::RandomRoll, on_random_roll)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_chat(
    In(ev): In<SessionEvent>,
    mut chat_log: ResMut<crate::ui_script::live::UiEvents>,
    social: Res<SocialState>,
    mut log: ResMut<ChatLog>,
    (self_guid, guids): (Res<SelfGuid>, Res<GuidIndex>),
    commands: Res<NetCommands>,
    mut server_said: MessageWriter<ServerSaidMessage>,
) {
    if let SessionEvent::Chat(m) = ev {
        let me = self_guid.0.filter(|g| guids.0.contains_key(g));
        if let Some(line) = chat(m, &social, me, &mut log, &commands, &mut server_said) {
            chat_log.push_line(line);
        }
    }
}

/// `SMSG_CHANNEL_LIST` (`0x505dc0`): a display reply goes to the channel pane (`0x503990`) with
/// the names the cache holds, the rest asked; any other prints the `/chatlist` roster.
fn on_channel_list(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::ChannelList {
        display,
        channel,
        flags,
        members,
    } = ev
    else {
        return;
    };
    if !display {
        super::feed::add(world, channel_list(channel, &members).into());
        return;
    }
    debug!(
        "chat: display list {channel:?} ({} members, flags {flags:#04x})",
        members.len()
    );
    // With the `World` lent: our own row is the active player's at the packet (`0x503990`).
    crate::ui_script::live::with_live_world(world, move |s| {
        s.display_channel_list(&channel, flags, &members, player_name);
    });
}

/// Whether a member's name record is held, read off the lent `World`'s name cache; a miss asks the
/// record with the roster's callback (`0x503bdb`, `0x50423b`).
fn player_name(guid: u64) -> bool {
    crate::ui_script::safe_world::with_world(|w| {
        let held = {
            let (Some(names), Some(commands)) = (
                w.get_resource::<crate::names::NameCache>(),
                w.get_resource::<NetCommands>(),
            ) else {
                return true;
            };
            names
                .answer(crate::names::NameKey::Player(guid), guid, commands)
                .is_some()
        };
        if !held {
            super::feed::register_roster_wait(w, guid);
        }
        held
    })
    .unwrap_or(true)
}

/// `SMSG_USERLIST_ADD`, `_REMOVE` and `_UPDATE`: the watched channel's roster follows its members, its events fired with the `World` lent.
fn on_channel_user_list(In(ev): In<SessionEvent>, world: &mut World) {
    use benilla_protocol::messages::ChannelUserListKind as K;
    let SessionEvent::ChannelUserList(u) = ev else {
        return;
    };
    debug!(
        "chat: user list {:?} {:?} {:#x} flags {:#04x}",
        u.kind, u.channel, u.guid, u.member_flags
    );
    crate::ui_script::live::with_live_world(world, move |script| match u.kind {
        K::Add => script.channel_user_added(
            &u.channel,
            u.guid,
            u.member_flags,
            u.channel_flags,
            player_name,
        ),
        K::Remove => script.channel_user_removed(&u.channel, u.guid, u.channel_flags),
        K::Update => {
            script.channel_user_updated(&u.channel, u.guid, u.member_flags, u.channel_flags)
        }
    });
}

/// The channel pane's name callback (`0x4ffe30`), run from the record's callback list: a member
/// of the roster counts down its pending names, and the last fires its count and roster with the
/// `World` lent.
pub(crate) fn roster_callback(world: &mut World, guid: u64) {
    crate::ui_script::safe_world::lend(world, |s| s.channel_roster_named(guid));
}

fn on_whisper_refusal(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    match ev {
        SessionEvent::ChatPlayerNotFound { name } => chat_player_not_found(&name, &mut *ui),
        SessionEvent::ChatWrongFaction => chat_wrong_faction(&mut *ui),
        _ => {}
    }
}

/// `SMSG_CHAT_NOT_IN_PARTY` (`0x6e397b`): the party and battleground-party types say not in a
/// group, the raid types not in a raid, any other nothing.
fn not_in_party_key(chat_type: u32) -> Option<&'static str> {
    match chat_type {
        2 | 51 => Some("ERR_NOT_IN_GROUP"),
        3 | 39 | 40 => Some("ERR_NOT_IN_RAID"),
        _ => None,
    }
}

fn on_chat_not_in_party(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::ChatNotInParty { chat_type } = ev {
        if let Some(key) = not_in_party_key(chat_type) {
            ui.line(UiError::key(key));
        }
    }
}

/// The world broadcasts (`0x50c3c0`, `0x50c850`, `0x50c980`): each resolves against the joined
/// channels and the player's zone as it lands and adds its lines in the handler's turn.
fn on_broadcast(In(ev): In<SessionEvent>, world: &mut World) {
    let b = match ev {
        SessionEvent::ZoneUnderAttack { area_id } => Broadcast::ZoneUnderAttack { area_id },
        SessionEvent::DefenseMessage { zone_id, text } => Broadcast::Defense { zone_id, text },
        SessionEvent::ServerMessage { message_type, text } => {
            Broadcast::Server { message_type, text }
        }
        _ => return,
    };
    // A defense broadcast outside the defense channels prints nothing, so the log is its only trace.
    info!("net: world broadcast — {b:?}");
    let template = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| super::combat::global_string(s, "ZONE_UNDER_ATTACK"));
    match world.run_system_cached_with(super::broadcast::broadcast_lines, (b, template)) {
        Ok(lines) => {
            for line in lines {
                super::feed::add(world, line.into());
            }
        }
        Err(e) => warn!("chat: the broadcast was not resolved: {e}"),
    }
}

/// `SMSG_CHAT_RESTRICTED` (`0x6e3cb9`): the kind picks its line through `0x6e44d0`; past 3,
/// nothing (`0x6e3ccb`).
fn chat_restricted_key(kind: u8) -> Option<&'static str> {
    Some(match kind {
        0 => "ERR_CHAT_RESTRICTED",
        1 => "ERR_CHAT_THROTTLED",
        2 => "ERR_USER_SQUELCHED",
        3 => "ERR_YELL_RESTRICTED",
        _ => return None,
    })
}

fn on_chat_restricted(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::ChatRestricted { kind } = ev {
        if let Some(key) = chat_restricted_key(kind) {
            ui.line(UiError::key(key));
        }
    }
}

fn on_toast(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    match ev {
        SessionEvent::Notification { text } => {
            if let Some(line) = notification(&text, &mut *ui) {
                ui.push_event(line);
            }
        }
        SessionEvent::AreaTriggerMessage { text } => area_trigger_message(text, &mut *ui),
        _ => {}
    }
}

/// `SMSG_PLAYED_TIME` (`0x401390`): the played clock's seed, and for a shown answer the console
/// breakdown (`0x401400`-`0x40144c`, the log here) then `TIME_PLAYED_MSG(total, level)` raised in
/// the handler (`0x401463`), whose stock handler prints the chat lines.
fn on_played_time(
    In(ev): In<SessionEvent>,
    mut clock: ResMut<crate::net::PlayedClock>,
    mut ui: ResMut<UiEvents>,
    active: Query<(), With<crate::net::SelfPlayer>>,
) {
    if let SessionEvent::PlayedTime { total, level, show } = ev {
        debug!("net: played time {total} s (level {level} s, shown {show})");
        // Only the active player's clock is seeded (`0x4013c6`-`0x4013f1`).
        if !active.is_empty() {
            clock.0 = Some((total, std::time::Instant::now()));
        }
        // Only a shown answer prints and fires `TIME_PLAYED_MSG` (`0x4013f6`).
        if show {
            info!(
                "console: total time played {}, this level {}",
                played_breakdown(total),
                played_breakdown(level)
            );
            ui.fire(
                "TIME_PLAYED_MSG",
                vec![
                    benilla_ui::script::ScriptValue::Int(i64::from(total)),
                    benilla_ui::script::ScriptValue::Int(i64::from(level)),
                ],
            );
        }
    }
}

fn on_channel_notify(
    In(ev): In<SessionEvent>,
    mut chat_log: ResMut<crate::ui_script::live::UiEvents>,
    social: Res<SocialState>,
    mut channels: ResMut<super::edit::ChannelState>,
) {
    if let SessionEvent::ChannelNotify {
        notice,
        channel,
        tail,
    } = ev
    {
        use benilla_protocol::messages::channel_notice as n;
        // A member's join or leave shows only on a record in state 0, found by name; any other
        // ends the handler before the name is asked (`0x50e1b0` -> `0x4fdee0`, `0x4fdf19`).
        if matches!(notice, n::JOINED | n::LEFT)
            && channels.slot_state(&channel) != Some(super::edit::SlotState::Joined)
        {
            return;
        }
        // An invite from an ignored or non-player guid is dropped whole (`0x50e76b`,
        // `0x50e778`-`0x50e798`).
        if notice == benilla_protocol::messages::channel_notice::INVITE {
            if let benilla_protocol::messages::ChannelNoticeTail::Actor(g) = tail {
                let player = g >> 60 == 0 && g & 0xf07f_ffff_ffff_ffff != 0;
                if social.is_ignored(g) || !player {
                    return;
                }
            }
        }
        // The voice arms (jump table `0x50eb08`: `0x22` -> `0x50e964`, `0x23` -> `0x50ea05`, `0x24`
        // -> `0x50e9bc`) write the record found by the wire name ahead of the line.
        let on = match notice {
            n::VOICE_ON | n::VOICE_ENABLED => Some(true),
            n::VOICE_OFF => Some(false),
            _ => None,
        };
        let rebuild = on.is_some_and(|on| channels.set_voice(&channel, on));
        // `VOICE_ENABLED` adds no line (`0x50ea00` jumps past the add).
        chat_log.push_channel_notice(notice, channel, &tail);
        // A found record's rebuild after the line, with its signal (`0x50eaf3` -> `0x504840`).
        if rebuild {
            chat_log.world(super::feed::channels_updated);
        }
    }
}

/// `MSG_RANDOM_ROLL` (`0x6e37af`): an ignored roller's roll shows nothing (`0x6e3801`).
fn on_random_roll(
    In(ev): In<SessionEvent>,
    mut chat_log: ResMut<crate::ui_script::live::UiEvents>,
    social: Res<SocialState>,
) {
    if let SessionEvent::RandomRoll {
        min,
        max,
        roll,
        guid,
    } = ev
    {
        if social.is_ignored(guid) {
            debug!("net: roll by ignored {guid:#x} dropped");
            return;
        }
        chat_log.push_roll(min, max, roll, guid);
    }
}

/// The session's end drops the held lines and leaves the world as the UI's world exit does
/// (`0x528cc9` -> `0x4fb190`): a same-character reconnect queues every line until its world entry.
fn on_session_end(In(_): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    chat_log.clear_session();
    chat_log.leave_world();
}

/// A spoken line (`SMSG_MESSAGECHAT`, `0x50be70`): the line the handler adds, `None` for one it
/// drops. System lines (`0x0A`), the server's answers to GM commands, log at `info!` and go out as
/// [`ServerSaidMessage`]; other chat logs at `debug!`. `me` is the active player's guid while its
/// object exists (`0x50bea4`).
fn chat(
    mut m: ChatMessage,
    social: &SocialState,
    me: Option<u64>,
    log: &mut ChatLog,
    net_commands: &NetCommands,
    server_said: &mut MessageWriter<ServerSaidMessage>,
) -> Option<super::ChatLine> {
    use benilla_protocol::messages::wotlk_chat_type as w;
    if m.is_addon() {
        debug!(
            "net: addon chat [{:#04x}] from {:#x}: {:?}",
            m.chat_type, m.sender_guid, m.text
        );
        if benilla_assets::trace::enabled() {
            // `<-` inbound; the outbound drain writes `->` under the same tag.
            benilla_assets::trace::line(
                "addon",
                &format!("<- [{:#04x}] {:?}", m.chat_type, m.text),
            );
        }
    } else {
        if m.chat_type == 0x0A {
            info!("net: server says — {}", m.text);
            // Also as a message: server state with no descriptor field (god mode) has no other tell.
            server_said.write(ServerSaidMessage {
                text: m.text.clone(),
            });
        } else {
            debug!("net: chat [{:#04x}] {}", m.chat_type, m.text);
        }
        // On the trace clock too, so a `.gps` answer lines up with the movement trace.
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "sys",
                &format!("[{:#04x}] {}", m.chat_type, m.text.replace('\n', " ⏎ ")),
            );
        }
    }
    match m.wire_type {
        // The monster and Battle.net arm drops a line whose receiver is ignored (`0x50c34a`-`0x50c35b`).
        w::MONSTER_SAY
        | w::MONSTER_YELL
        | w::RAID_BOSS_EMOTE
        | w::RAID_BOSS_WHISPER
        | w::MONSTER_EMOTE
        | w::MONSTER_WHISPER
        | w::MONSTER_PARTY
        | w::BATTLENET => {
            m.chat_tag = 0;
            if social.is_ignored(m.target_guid) {
                return None;
            }
        }
        // A foreign whisper from an ignored sender is dropped with no reply (`0x50bfba`-`0x50bfc1`);
        // the rest go on as a whisper (`0x50bfc7`), past the default arm (`0x50bfcb`).
        w::WHISPER_FOREIGN => {
            m.chat_tag = 0;
            if social.is_ignored(m.sender_guid) {
                return None;
            }
            m.wire_type = w::WHISPER;
        }
        // The battleground and achievement arms test no ignore (`0x50c289`, `0x50c20e`).
        w::BG_SYSTEM_NEUTRAL
        | w::BG_SYSTEM_ALLIANCE
        | w::BG_SYSTEM_HORDE
        | w::ACHIEVEMENT
        | w::GUILD_ACHIEVEMENT => m.chat_tag = 0,
        _ => {
            if !default_arm(&m, social, me, log, net_commands) {
                return None;
            }
        }
    }
    Some(super::ChatLine::Wire { msg: m })
}

/// The handler's default arm past its read (`0x50c070`-`0x50c19f`): whether the line goes on to
/// the add. Only this arm reads the chat tag into the add's flag (`0x50c070`); the other arms
/// leave the flag `""` (`0x50be9d`), so [`chat`] clears their tag.
fn default_arm(
    m: &ChatMessage,
    social: &SocialState,
    me: Option<u64>,
    log: &mut ChatLog,
    net_commands: &NetCommands,
) -> bool {
    use benilla_protocol::messages::{chat_tag, wotlk_chat_type as w};
    const ADDON: u32 = benilla_protocol::messages::LANGUAGE_ADDON;
    let sender = m.sender_guid;
    // A whisper enters its sender in the source history (`0x50c0cc`), its echo its receiver in the
    // destination history (`0x50c0bd`).
    match m.wire_type {
        w::WHISPER | w::BATTLENET => log.record_whisper_source(sender),
        w::WHISPER_INFORM => log.record_whisper_dest(sender),
        _ => {}
    }
    // A commentator's whisper (tag `0x8` without `0x4`) from anyone but us, a friend or one we
    // whispered is refused with reason 1 and dropped (`0x50c0da`-`0x50c14b`).
    let commentator = m.chat_tag & chat_tag::COMMENTATOR != 0 && m.chat_tag & chat_tag::GM == 0;
    if matches!(m.wire_type, w::WHISPER | w::BATTLENET)
        && commentator
        && m.language != ADDON
        && me != Some(sender)
        && !social.is_friend(sender)
        && !log.whispered(sender)
    {
        let _ = net_commands.0.send(ClientCommand::ChatIgnored {
            guid: sender,
            reason: 1,
        });
        return false;
    }
    // An ignored sender's line is dropped unless the tag says GM (`0x50c160`-`0x50c16d`): a
    // whisper replies with reason 0 first, an addon one aside (`0x50c176`-`0x50c18a`); its echo,
    // the ignore notice and a restricted line still post (`0x50c195`-`0x50c19f`).
    if social.is_ignored(sender) && m.chat_tag & chat_tag::GM == 0 {
        if m.wire_type == w::WHISPER {
            if m.language == ADDON {
                return false;
            }
            let _ = net_commands.0.send(ClientCommand::ChatIgnored {
                guid: sender,
                reason: 0,
            });
        }
        return matches!(m.wire_type, w::WHISPER_INFORM | w::IGNORED | w::RESTRICTED);
    }
    true
}

/// The `/chatlist` roster (`SMSG_CHANNEL_LIST`). Members arrive as guids and are not resolved, so
/// the line gives their count where the reference lists their names.
fn channel_list(channel: String, members: &[(u64, u8)]) -> ChatEvent {
    let mut ev = ChatEvent::text_only(
        ChatEventKind::ChannelList,
        format!("{} member(s)", members.len()),
    );
    ev.channel = channel;
    ev
}

/// A whisper target is offline: message 241, `ERR_CHAT_PLAYER_NOT_FOUND_S`, with the server's
/// name, shown in the handler's turn; its row routes it to chat.
fn chat_player_not_found(name: &str, sink: &mut dyn SignalSink) {
    sink.line(UiError::s("ERR_CHAT_PLAYER_NOT_FOUND_S", name));
}

/// A cross-faction whisper was refused: message 240, `ERR_CHAT_WRONG_FACTION`.
fn chat_wrong_faction(sink: &mut dyn SignalSink) {
    sink.line(UiError::key("ERR_CHAT_WRONG_FACTION"));
}

/// Where one `SMSG_NOTIFICATION` text shows, after `0x401280`'s control prefix.
#[derive(Debug, PartialEq, Eq)]
struct NoticeRoute<'a> {
    text: &'a str,
    /// The error-display line and its flag: red for colour 3, else yellow.
    toast: Option<MsgKind>,
    /// A chat line of this 3.3.5a chat type.
    chat: Option<i32>,
}

/// `0x4012ac`-`0x401328`: a leading `"DEBUGACTION: "` is cut; with no `~` next the text is the red
/// line alone. After a `~`, `>` drops the line, `#` drops it and adds a chat line, and `^N` sets
/// the chat type and the console colour to digit N, the line red only for N = 3.
fn notice_route(text: &str) -> NoticeRoute<'_> {
    let text = text.strip_prefix("DEBUGACTION: ").unwrap_or(text);
    let Some(mut rest) = text.strip_prefix('~') else {
        return NoticeRoute {
            text,
            toast: Some(MsgKind::Error),
            chat: None,
        };
    };
    let (mut toast, mut chat) = (true, false);
    if let Some(r) = rest.strip_prefix('>') {
        toast = false;
        rest = r;
    }
    if let Some(r) = rest.strip_prefix('#') {
        (toast, chat) = (false, true);
        rest = r;
    }
    let (mut chat_type, mut color) = (0, 3);
    if let Some(r) = rest.strip_prefix('^') {
        let digit = r.bytes().next().map_or(0, |b| i32::from(b as i8));
        (chat_type, color) = (digit - 0x30, digit - 0x30);
        rest = r.char_indices().nth(1).map_or("", |(i, _)| &r[i..]);
    }
    NoticeRoute {
        text: rest,
        toast: toast.then_some(if color == 3 {
            MsgKind::Error
        } else {
            MsgKind::Info
        }),
        chat: chat.then_some(chat_type),
    }
}

/// `SMSG_NOTIFICATION` (`0x401280`): the error-display line, the console and a chat line as the
/// text's prefix says ([`notice_route`]). vmangos sends `.gm on` as both a system line and a
/// notification (`Objects/Player.cpp:2676-2677`).
fn notification(text: &str, errors: &mut dyn TextSink) -> Option<ChatEvent> {
    let route = notice_route(text);
    info!("net: notification — {}", route.text);
    match route.toast {
        Some(MsgKind::Error) => errors.text_line(MsgKind::Error, route.text.to_string()),
        Some(_) => errors.text_line(MsgKind::Info, route.text.to_string()),
        None => {}
    }
    let kind = route
        .chat
        .and_then(|t| u8::try_from(t).ok())
        .and_then(benilla_protocol::messages::wotlk_chat_type_shared)
        .and_then(super::event::kind_of_wire);
    kind.map(|kind| ChatEvent::text_only(kind, route.text.to_string()))
}

/// An area trigger's refusal (`SMSG_AREA_TRIGGER_MESSAGE`, `0x2b8`): the yellow `UIErrorsFrame`
/// line, `UI_INFO_MESSAGE` from `(text, 0)`.
fn area_trigger_message(text: String, errors: &mut dyn TextSink) {
    info!("net: area-trigger message — {text}");
    errors.text_line(MsgKind::Info, text);
}

/// One played time as days, hours, minutes and seconds, for the console line.
fn played_breakdown(secs: u32) -> String {
    let (d, rem) = (secs / 86_400, secs % 86_400);
    let (h, rem) = (rem / 3_600, rem % 3_600);
    let (m, sec) = (rem / 60, rem % 60);
    format!("{d}d {h}h {m}m {sec}s")
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use benilla_protocol::messages::{CHAT_MSG_PARTY, CHAT_MSG_SYSTEM, LANGUAGE_ADDON};

    fn a_party_line(language: u32, text: &str) -> ChatMessage {
        ChatMessage {
            chat_type: CHAT_MSG_PARTY,
            language,
            sender_guid: 0x21,
            target_guid: 0x21,
            sender_name: None,
            receiver_name: None,
            channel: None,
            text: text.to_string(),
            chat_tag: 0,
            achievement: 0,
            wire_type: 0,
            sender_word: 0,
            gm: false,
        }
    }

    /// A line of 3.3.5a type `wire` from `sender` to `receiver`, as the decoder hands it over.
    fn a_line(wire: u8, sender: u64, receiver: u64, tag: u8) -> ChatMessage {
        ChatMessage {
            chat_type: benilla_protocol::messages::wotlk_chat_type_shared(wire)
                .expect("a shared type"),
            sender_guid: sender,
            target_guid: receiver,
            chat_tag: tag,
            wire_type: wire,
            ..a_party_line(0, "hi")
        }
    }

    /// The handler over `m`: the line it hands the add, and the `CMSG_CHAT_IGNORED` replies it
    /// sent as `(guid, reason)`.
    fn hear(
        m: ChatMessage,
        social: &SocialState,
        log: &mut ChatLog,
        me: Option<u64>,
    ) -> (Option<ChatMessage>, Vec<(u64, u8)>) {
        let mut world = World::new();
        world.init_resource::<Messages<ServerSaidMessage>>();
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut state =
            bevy::ecs::system::SystemState::<MessageWriter<ServerSaidMessage>>::new(&mut world);
        let mut server_said = state.get_mut(&mut world);
        let line = chat(m, social, me, log, &commands, &mut server_said).map(|l| match l {
            super::super::ChatLine::Wire { msg } => msg,
            _ => panic!("the handler hands the add a wire line"),
        });
        let replies = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::ChatIgnored { guid, reason } => Some((guid, reason)),
                _ => None,
            })
            .collect();
        (line, replies)
    }

    fn ignoring(guids: &[u64]) -> SocialState {
        let mut social = SocialState::default();
        social.set_ignores_for_test(guids.to_vec());
        social
    }

    /// The monster and Battle.net arm tests the receiver, never the sender (`0x50c34a`-`0x50c35b`).
    #[test]
    fn a_monster_line_drops_on_an_ignored_receiver() {
        use benilla_protocol::messages::wotlk_chat_type as w;
        let social = ignoring(&[0x21]);
        let mut log = ChatLog::default();
        for wire in [w::MONSTER_SAY, w::MONSTER_WHISPER, w::RAID_BOSS_EMOTE] {
            let (line, replies) = hear(a_line(wire, 0x22, 0x21, 0), &social, &mut log, None);
            assert!(line.is_none(), "type {wire:#x}: the receiver is ignored");
            assert!(replies.is_empty());
            let (line, _) = hear(a_line(wire, 0x21, 0x22, 0), &social, &mut log, None);
            assert!(
                line.is_some(),
                "type {wire:#x}: an ignored sender still speaks"
            );
        }
    }

    /// The achievement and battleground arms test no ignore (`0x50c20e`, `0x50c289`).
    #[test]
    fn an_ignored_players_achievement_and_battleground_lines_show() {
        use benilla_protocol::messages::wotlk_chat_type as w;
        let social = ignoring(&[0x21]);
        let mut log = ChatLog::default();
        for wire in [
            w::ACHIEVEMENT,
            w::GUILD_ACHIEVEMENT,
            w::BG_SYSTEM_NEUTRAL,
            w::BG_SYSTEM_HORDE,
        ] {
            let (line, _) = hear(a_line(wire, 0x21, 0x21, 0), &social, &mut log, None);
            assert!(line.is_some(), "type {wire:#x}");
        }
    }

    /// The default arm: a GM-tagged ignored sender still speaks (`0x50c169`); an ignored whisper
    /// replies with reason 0 (`0x50c186`), an addon whisper with none (`0x50c17a`); an echo, an
    /// ignore notice and a restricted line still post (`0x50c195`-`0x50c19f`).
    #[test]
    fn the_default_arm_drops_an_ignored_sender_bar_a_gm() {
        use benilla_protocol::messages::{chat_tag, wotlk_chat_type as w};
        let social = ignoring(&[0x21]);
        let mut log = ChatLog::default();
        let (line, _) = hear(a_line(w::SAY, 0x21, 0, 0), &social, &mut log, None);
        assert!(line.is_none());
        let (line, replies) = hear(
            a_line(w::SAY, 0x21, 0, chat_tag::GM),
            &social,
            &mut log,
            None,
        );
        assert!(line.is_some(), "a GM tag lifts the ignore");
        assert!(replies.is_empty());
        let (line, replies) = hear(a_line(w::WHISPER, 0x21, 0, 0), &social, &mut log, None);
        assert!(line.is_none());
        assert_eq!(replies, [(0x21, 0)]);
        let addon = ChatMessage {
            language: LANGUAGE_ADDON,
            ..a_line(w::WHISPER, 0x21, 0, 0)
        };
        let (line, replies) = hear(addon, &social, &mut log, None);
        assert!(line.is_none());
        assert!(replies.is_empty(), "an addon whisper gets no reply");
        for wire in [w::WHISPER_INFORM, w::IGNORED, w::RESTRICTED] {
            let (line, replies) = hear(a_line(wire, 0x21, 0, 0), &social, &mut log, None);
            assert!(line.is_some(), "type {wire:#x}");
            assert!(replies.is_empty());
        }
    }

    /// A foreign whisper from an ignored sender is dropped with no reply (`0x50bfba`-`0x50bfc1`);
    /// another goes on as a whisper with no flag (`0x50bfc7`, `0x50bfcb`).
    #[test]
    fn a_foreign_whisper_is_dropped_silently_when_ignored() {
        use benilla_protocol::messages::{chat_tag, wotlk_chat_type as w};
        let social = ignoring(&[0x21]);
        let mut log = ChatLog::default();
        let (line, replies) = hear(
            a_line(w::WHISPER_FOREIGN, 0x21, 0, 0),
            &social,
            &mut log,
            None,
        );
        assert!(line.is_none());
        assert!(replies.is_empty(), "no CMSG_CHAT_IGNORED");
        let (line, _) = hear(
            a_line(w::WHISPER_FOREIGN, 0x22, 0, chat_tag::GM),
            &social,
            &mut log,
            None,
        );
        let line = line.expect("a stranger's foreign whisper shows");
        assert_eq!(line.wire_type, w::WHISPER);
        assert_eq!(
            line.chat_tag, 0,
            "the flag is decoded only in the default arm"
        );
        assert!(
            log.whisper_sources().is_empty(),
            "nor is it entered in the history"
        );
    }

    /// Only the default arm reads the tag into the flag (`0x50c070`): a monster line's is `""`.
    #[test]
    fn the_flag_is_read_in_the_default_arm_alone() {
        use benilla_protocol::messages::{chat_tag, wotlk_chat_type as w};
        let social = SocialState::default();
        let mut log = ChatLog::default();
        let (line, _) = hear(
            a_line(w::SAY, 0x21, 0, chat_tag::GM),
            &social,
            &mut log,
            None,
        );
        assert_eq!(line.unwrap().chat_tag, chat_tag::GM);
        for wire in [w::MONSTER_SAY, w::ACHIEVEMENT, w::BG_SYSTEM_ALLIANCE] {
            let (line, _) = hear(a_line(wire, 0x21, 0, chat_tag::GM), &social, &mut log, None);
            assert_eq!(line.unwrap().chat_tag, 0, "type {wire:#x}");
        }
    }

    /// A commentator's whisper (tag `0x8`, no `0x4`) from someone we never whispered, not a friend
    /// and not us, is refused with reason 1 and dropped (`0x50c0da`-`0x50c14b`).
    #[test]
    fn a_commentators_whisper_needs_a_contact() {
        use benilla_protocol::messages::{chat_tag, wotlk_chat_type as w};
        let com = chat_tag::COMMENTATOR;
        let mut social = SocialState::default();
        let mut log = ChatLog::default();
        let (line, replies) = hear(a_line(w::WHISPER, 0x21, 0, com), &social, &mut log, None);
        assert!(line.is_none());
        assert_eq!(replies, [(0x21, 1)]);
        // A GM tag, an addon line or another type passes.
        let (line, _) = hear(
            a_line(w::WHISPER, 0x21, 0, com | chat_tag::GM),
            &social,
            &mut log,
            None,
        );
        assert!(line.is_some());
        let addon = ChatMessage {
            language: LANGUAGE_ADDON,
            ..a_line(w::WHISPER, 0x21, 0, com)
        };
        assert!(hear(addon, &social, &mut log, None).0.is_some());
        assert!(hear(a_line(w::SAY, 0x21, 0, com), &social, &mut log, None)
            .0
            .is_some());
        // Ourselves.
        assert!(hear(
            a_line(w::WHISPER, 0x21, 0, com),
            &social,
            &mut log,
            Some(0x21)
        )
        .0
        .is_some());
        // A friend.
        social.set_friends_for_test(&[0x21]);
        assert!(
            hear(a_line(w::WHISPER, 0x21, 0, com), &social, &mut log, None)
                .0
                .is_some()
        );
        // One our whisper reached: its echo enters the destination history (`0x50c0bd`).
        let social = SocialState::default();
        let (line, replies) = hear(a_line(w::WHISPER, 0x22, 0, com), &social, &mut log, None);
        assert!(line.is_none());
        assert_eq!(replies, [(0x22, 1)]);
        hear(
            a_line(w::WHISPER_INFORM, 0x22, 0, 0),
            &social,
            &mut log,
            None,
        );
        let (line, replies) = hear(a_line(w::WHISPER, 0x22, 0, com), &social, &mut log, None);
        assert!(line.is_some());
        assert!(replies.is_empty());
    }

    /// Each history moves a repeated guid to the front and drops its oldest past its depth, 16 for
    /// the sources (`0x4fb710`) and 128 for the destinations (`0x4fb820`); guid 0 enters neither.
    #[test]
    fn the_whisper_histories_keep_the_newest() {
        let mut log = ChatLog::default();
        for g in 1..=17 {
            log.record_whisper_source(g);
        }
        assert_eq!(log.whisper_sources(), (2..=17).rev().collect::<Vec<_>>());
        log.record_whisper_source(9);
        assert_eq!(log.whisper_sources()[..3], [9, 17, 16]);
        assert_eq!(log.whisper_sources().len(), 16);
        log.record_whisper_source(0);
        assert_eq!(log.whisper_sources()[0], 9);
        for g in 1..=129 {
            log.record_whisper_dest(g);
        }
        assert!(!log.whispered(1), "the oldest fell off at 128");
        assert!(log.whispered(2) && log.whispered(129));
        assert!(!log.whispered(0));
    }

    /// Run one inbound line through the real arm body and report whether it reached the chat log.
    fn reaches_the_chat_window(m: ChatMessage) -> bool {
        chat_lines(m) > 0
    }

    /// How many lines one inbound message put in the chat window.
    fn chat_lines(m: ChatMessage) -> usize {
        let mut world = World::new();
        world.init_resource::<Messages<ServerSaidMessage>>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.insert_resource(SocialState::default());
        world.insert_resource(ChatLog::default());
        let line = world
            .run_system_once(
                move |social: Res<SocialState>,
                      mut log: ResMut<ChatLog>,
                      net_commands: Res<NetCommands>,
                      mut server_said: MessageWriter<ServerSaidMessage>| {
                    chat(
                        m.clone(),
                        &social,
                        None,
                        &mut log,
                        &net_commands,
                        &mut server_said,
                    )
                },
            )
            .unwrap();
        // Outside the world the add queues the line (`0x509f48`).
        if let Some(line) = line {
            super::super::feed::add_now(&mut world, line);
        }
        world.resource::<ChatLog>().pending_len()
    }

    /// An addon line rides an ordinary lane; only its `language` keeps it out of the window.
    #[test]
    fn addon_chat_never_reaches_the_chat_window() {
        assert!(
            !reaches_the_chat_window(a_party_line(LANGUAGE_ADDON, "Quiver\tVERSION:3.1.4")),
            "an addon broadcast must be dropped before the chat log"
        );
        // No tab, or no text: still addon traffic, since the gate reads the language alone.
        assert!(!reaches_the_chat_window(a_party_line(
            LANGUAGE_ADDON,
            "nopayloadseparator"
        )));
        assert!(!reaches_the_chat_window(a_party_line(LANGUAGE_ADDON, "")));
    }

    /// `pending_len` excludes parked addon items; this checks the line is parked, not lost.
    #[test]
    fn an_addon_line_is_parked_for_the_addon_event_not_discarded() {
        let mut world = World::new();
        world.init_resource::<Messages<ServerSaidMessage>>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.insert_resource(SocialState::default());
        world.insert_resource(ChatLog::default());
        let m = a_party_line(LANGUAGE_ADDON, "Quiver\tVERSION:3.1.4");
        let line = world
            .run_system_once(
                move |social: Res<SocialState>,
                      mut log: ResMut<ChatLog>,
                      net_commands: Res<NetCommands>,
                      mut server_said: MessageWriter<ServerSaidMessage>| {
                    chat(
                        m.clone(),
                        &social,
                        None,
                        &mut log,
                        &net_commands,
                        &mut server_said,
                    )
                },
            )
            .unwrap();
        // Outside the world the add queues the line (`0x509f48`).
        if let Some(line) = line {
            super::super::feed::add_now(&mut world, line);
        }
        let log = world.resource::<ChatLog>();
        assert_eq!(
            log.pending_addons(),
            vec![(
                "Quiver".to_string(),
                "VERSION:3.1.4".to_string(),
                "PARTY".to_string()
            )],
            "the line must be queued for CHAT_MSG_ADDON, not dropped"
        );
        assert_eq!(log.pending_len(), 0, "and still never headed for a window");
    }

    #[test]
    fn speech_on_the_addon_lane_still_renders() {
        for language in [0, 1, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 33] {
            assert!(
                reaches_the_chat_window(a_party_line(language, "party control line")),
                "language {language} is a tongue — the line must render"
            );
        }
    }

    /// `.gm on` arrives as a system line and a notification (`Objects/Player.cpp:2676-2677`): one
    /// chat line, one red toast.
    #[test]
    fn toggling_gm_mode_prints_one_chat_line_and_one_toast() {
        let sys_line = ChatMessage {
            chat_type: CHAT_MSG_SYSTEM,
            ..a_party_line(0, "GM mode is ON")
        };
        assert_eq!(
            chat_lines(sys_line),
            1,
            "the SendSysMessage half is the chat line, and it stays"
        );

        let mut errors: Vec<(String, MsgKind)> = Vec::new();
        let mut log = Vec::new();
        log.extend(notification("GM mode is ON", &mut errors));
        assert_eq!(
            errors,
            [(
                "GM mode is ON".to_string(),
                benilla_ui::messages::MsgKind::Error
            )],
            "the SendNotification half is the RED toast, never a second line"
        );
        assert!(log.is_empty());
    }

    /// `0x4012ac`-`0x401371`: each prefix's surfaces, with the prefix cut from the text.
    #[test]
    fn a_notification_prefix_picks_its_surfaces() {
        let route = |toast, chat, text| NoticeRoute { text, toast, chat };
        let (red, yellow) = (Some(MsgKind::Error), Some(MsgKind::Info));
        assert_eq!(notice_route("plain"), route(red, None, "plain"));
        assert_eq!(
            notice_route("DEBUGACTION: ~#hi"),
            route(None, Some(0), "hi")
        );
        assert_eq!(notice_route("~>quiet"), route(None, None, "quiet"));
        assert_eq!(notice_route("~>#both"), route(None, Some(0), "both"));
        assert_eq!(notice_route("~^1info"), route(yellow, None, "info"));
        assert_eq!(notice_route("~^3red"), route(red, None, "red"));
        assert_eq!(notice_route("~#^1say"), route(None, Some(1), "say"));
        assert_eq!(notice_route("~text"), route(red, None, "text"));
        assert_eq!(notice_route("x~text"), route(red, None, "x~text"));
    }

    /// A `#` line lands in chat as its 3.3.5a type's lane, beside no toast.
    #[test]
    fn a_hash_notification_writes_a_chat_line_of_its_type() {
        let mut errors: Vec<(String, MsgKind)> = Vec::new();
        let mut log = Vec::new();
        log.extend(notification("~#^1hello", &mut errors));
        log.extend(notification("~#plain system", &mut errors));
        log.extend(notification("~#^/", &mut errors));
        assert!(errors.is_empty());
        assert_eq!(
            log.len(),
            2,
            "type 1 (say) and type 0 (system); -1 has no lane"
        );
    }

    /// `0x6e44d0`: kinds 0-3 each take their line, and past 3 nothing shows.
    #[test]
    fn a_chat_restriction_shows_its_kinds_line() {
        assert_eq!(chat_restricted_key(0), Some("ERR_CHAT_RESTRICTED"));
        assert_eq!(chat_restricted_key(1), Some("ERR_CHAT_THROTTLED"));
        assert_eq!(chat_restricted_key(2), Some("ERR_USER_SQUELCHED"));
        assert_eq!(chat_restricted_key(3), Some("ERR_YELL_RESTRICTED"));
        assert_eq!(chat_restricted_key(4), None);
    }

    /// `0x6e397b`: the party types say not in a group, the raid types not in a raid.
    #[test]
    fn a_line_outside_the_group_names_the_group_it_needed() {
        for t in [2, 51] {
            assert_eq!(not_in_party_key(t), Some("ERR_NOT_IN_GROUP"), "type {t}");
        }
        for t in [3, 39, 40] {
            assert_eq!(not_in_party_key(t), Some("ERR_NOT_IN_RAID"), "type {t}");
        }
        for t in [0, 1, 4, 44, 52] {
            assert_eq!(not_in_party_key(t), None, "type {t}");
        }
    }

    /// `0x6e3801`: a roll from an ignored player never queues; any other does.
    #[test]
    fn an_ignored_players_roll_is_dropped() {
        let mut world = World::new();
        world.init_resource::<UiEvents>();
        let mut social = SocialState::default();
        social.set_ignores_for_test(vec![0x21]);
        world.insert_resource(social);
        for guid in [0x21, 0x22] {
            let ev = SessionEvent::RandomRoll {
                min: 1,
                max: 100,
                roll: 42,
                guid,
            };
            world.run_system_once_with(on_random_roll, ev).unwrap();
        }
        assert_eq!(
            world.resource_mut::<UiEvents>().take_chat().len(),
            1,
            "only 0x22's roll queues"
        );
    }

    #[test]
    fn an_area_trigger_refusal_takes_the_yellow_arm() {
        let mut errors: Vec<(String, MsgKind)> = Vec::new();
        area_trigger_message(
            "You must be at least level 58 to enter.".to_string(),
            &mut errors,
        );
        assert_eq!(
            errors,
            [(
                "You must be at least level 58 to enter.".to_string(),
                benilla_ui::messages::MsgKind::Info
            )]
        );
    }

    /// A display `SMSG_CHANNEL_LIST` seats our own row's flags against the active player the
    /// `World` holds at the handler (`0x503990`), with no unit fixture: the owner gate reads it.
    #[test]
    fn a_display_list_seats_our_own_row_against_the_player_held() {
        use benilla_ui::script::UiScript;
        const ME: u64 = 0x10;
        const GENERAL: &str = "General - Elwynn Forest";
        let mut world = World::new();
        world.init_resource::<ChatLog>();
        world.init_resource::<crate::ui_script::live::UiEvents>();
        world.spawn((crate::net::SelfPlayer, crate::net::Guid(ME)));
        world.insert_resource(crate::ui_chat::ChannelState {
            joined: vec![Some(crate::ui_chat::edit::ChannelSlot::zone(GENERAL, 1))],
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 1,
                    flags: 0x80003,
                    pattern: "General - %s".into(),
                    shortcut: "General".into(),
                },
            ]),
            ..Default::default()
        });
        world.insert_non_send_resource(UiScript::new().expect("a VM"));
        crate::ui_script::safe_world::lend(&mut world, |s| {
            s.run("GetNumDisplayChannels() SetSelectedDisplayChannel(3)")
                .expect("the channel is listed under the world header")
        });
        let owner = |w: &mut World| {
            crate::ui_script::safe_world::lend(w, |s| {
                s.eval::<bool>("return IsDisplayChannelOwner() == 1")
                    .unwrap()
            })
            .unwrap()
        };
        assert!(!owner(&mut world));
        world
            .run_system_once_with(
                on_channel_list,
                SessionEvent::ChannelList {
                    display: true,
                    channel: GENERAL.into(),
                    flags: 0x18,
                    members: vec![(ME, 0x01), (0x20, 0)],
                },
            )
            .unwrap();
        crate::ui_script::live::fire_handler_events(&mut world);
        assert!(owner(&mut world), "our row carries the owner bit");
    }

    /// The pane's selection and collapsed headers are game state, which `GetSelectedDisplayChannel`
    /// reads (`0x4ffa10`): a new VM, as `/reload` builds, answers them unchanged; only the
    /// session's clear (`0x50f198`-`0x50f202`) empties them.
    #[test]
    fn the_pane_outlives_a_reload_and_ends_with_the_session() {
        use benilla_ui::script::UiScript;
        const GENERAL: &str = "General - Elwynn Forest";
        let mut world = World::new();
        world.init_resource::<ChatLog>();
        world.init_resource::<crate::ui_script::live::UiEvents>();
        world.init_resource::<super::super::channels::ZoneChannelWalk>();
        world.spawn((crate::net::SelfPlayer, crate::net::Guid(0x10)));
        world.insert_resource(crate::ui_chat::ChannelState {
            joined: vec![
                Some(crate::ui_chat::edit::ChannelSlot::zone(GENERAL, 1)),
                Some(crate::ui_chat::edit::ChannelSlot::joined("MyChan")),
            ],
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: 1,
                    flags: 0x80003,
                    pattern: "General - %s".into(),
                    shortcut: "General".into(),
                },
            ]),
            ..Default::default()
        });
        world.insert_non_send_resource(UiScript::new().expect("a VM"));
        let pane = |w: &mut World| {
            crate::ui_script::safe_world::lend(w, |s| {
                s.eval::<(Option<i64>, Option<i64>)>(
                    "GetNumDisplayChannels() local _, _, c = GetChannelDisplayInfo(2) \
                     return GetSelectedDisplayChannel(), c",
                )
                .unwrap()
            })
            .unwrap()
        };
        crate::ui_script::safe_world::lend(&mut world, |s| {
            s.run("GetNumDisplayChannels() SetSelectedDisplayChannel(5) CollapseChannelHeader(2)")
                .unwrap()
        });
        assert_eq!(
            pane(&mut world),
            (Some(4), Some(1)),
            "MyChan under the folded world header"
        );
        world.insert_non_send_resource(UiScript::new().expect("the reloaded VM"));
        assert_eq!(pane(&mut world), (Some(4), Some(1)), "a reload keeps both");
        world
            .run_system_once(super::super::channels::end_session_channels)
            .unwrap();
        assert_eq!(pane(&mut world), (None, None), "the session clears both");
    }

    /// A dropped session leaves the world (`0x528cc9` -> `0x4fb190`): a line the reconnect brings
    /// before its world entry is queued, not added.
    #[test]
    fn a_dropped_session_queues_the_chat_until_the_next_entry() {
        let mut world = World::new();
        world.insert_resource(ChatLog::default());
        super::super::feed::enter_world(&mut world);
        assert!(!world.resource::<ChatLog>().queues());
        let lost = SessionEvent::Disconnected {
            reason: String::new(),
            end: benilla_protocol::SessionEnd::Lost,
        };
        world.run_system_once_with(on_session_end, lost).unwrap();
        assert!(
            world.resource::<ChatLog>().queues(),
            "the add queues until the reconnect's world entry"
        );
    }

    /// The answer seeds the active player's clock alone (`0x4013c6`-`0x4013f1`): with no active
    /// player it is dropped, and the clock still reads 0.
    #[test]
    fn a_played_answer_seeds_only_the_active_players_clock() {
        let mut world = World::new();
        world.init_resource::<crate::net::PlayedClock>();
        world.init_resource::<UiEvents>();
        let answer = || SessionEvent::PlayedTime {
            total: 3_600,
            level: 60,
            show: false,
        };
        world
            .run_system_once_with(on_played_time, answer())
            .unwrap();
        assert_eq!(world.resource::<crate::net::PlayedClock>().now(), 0);
        world.spawn(crate::net::SelfPlayer);
        world
            .run_system_once_with(on_played_time, answer())
            .unwrap();
        assert_eq!(world.resource::<crate::net::PlayedClock>().now(), 3_600);
    }
}
