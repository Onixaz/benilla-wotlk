//! The chat window's packet handlers: the spoken line, and the notices and answers that print as
//! chat. `SMSG_NOTIFICATION` and `SMSG_AREA_TRIGGER_MESSAGE` are mostly `UIErrorsFrame` toasts,
//! queued on [`UiErrorTexts`].

use benilla_protocol::messages::{ChatMessage, CHAT_MSG_WHISPER};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{Broadcast, ChatEvent, ChatEventKind, ChatLog};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, PlayedTimeAnswer, ServerSaidMessage};
use crate::ui_action::{UiError, UiErrorKeys, UiErrorTexts};
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
    mut chat_log: ResMut<ChatLog>,
    social: Res<SocialState>,
    commands: Res<NetCommands>,
    mut server_said: MessageWriter<ServerSaidMessage>,
) {
    if let SessionEvent::Chat(m) = ev {
        chat(m, &mut chat_log, &social, &commands, &mut server_said);
    }
}

/// `SMSG_CHANNEL_LIST` (`0x505dc0`): a display reply goes to the channel pane (`0x503990`) with
/// the names the cache holds, the rest asked; any other prints the `/chatlist` roster.
fn on_channel_list(
    In(ev): In<SessionEvent>,
    mut chat_log: ResMut<ChatLog>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    names: Option<Res<crate::names::NameCache>>,
    commands: Option<Res<NetCommands>>,
) {
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
        channel_list(channel, &members, &mut chat_log);
        return;
    }
    debug!(
        "chat: display list {channel:?} ({} members, flags {flags:#04x})",
        members.len()
    );
    let Some(mut script) = script else {
        return;
    };
    script.display_channel_list(&channel, flags, &members, |guid| {
        let (Some(names), Some(commands)) = (names.as_deref(), commands.as_deref()) else {
            return None;
        };
        names
            .answer(crate::names::NameKey::Player(guid), guid, commands)
            .map(|name| name.map(str::to_string))
    });
}

/// `SMSG_USERLIST_ADD`, `_REMOVE` and `_UPDATE`: the watched channel's roster follows its members.
fn on_channel_user_list(
    In(ev): In<SessionEvent>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    names: Option<Res<crate::names::NameCache>>,
    commands: Option<Res<NetCommands>>,
) {
    use benilla_protocol::messages::ChannelUserListKind as K;
    let (SessionEvent::ChannelUserList(u), Some(mut script)) = (ev, script) else {
        return;
    };
    debug!(
        "chat: user list {:?} {:?} {:#x} flags {:#04x}",
        u.kind, u.channel, u.guid, u.member_flags
    );
    match u.kind {
        K::Add => script.channel_user_added(
            &u.channel,
            u.guid,
            u.member_flags,
            u.channel_flags,
            |guid| {
                let (Some(names), Some(commands)) = (names.as_deref(), commands.as_deref()) else {
                    return None;
                };
                names
                    .answer(crate::names::NameKey::Player(guid), guid, commands)
                    .map(|name| name.map(str::to_string))
            },
        ),
        K::Remove => script.channel_user_removed(&u.channel, u.guid, u.channel_flags),
        K::Update => {
            script.channel_user_updated(&u.channel, u.guid, u.member_flags, u.channel_flags)
        }
    }
}

/// The channel pane's roster waits on the name cache (`0x4ffe30`): each answer that has landed is
/// handed over, and the last one fires the roster's events.
pub(super) fn feed_channel_roster_names(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    names: Option<Res<crate::names::NameCache>>,
    commands: Option<Res<NetCommands>>,
) {
    let (Some(mut script), Some(names), Some(commands)) = (script, names, commands) else {
        return;
    };
    for guid in script.channel_roster_unnamed() {
        if let Some(answer) = names.answer(crate::names::NameKey::Player(guid), guid, &commands) {
            let answer = answer.map(str::to_string);
            script.channel_roster_named(guid, answer);
        }
    }
}

fn on_whisper_refusal(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    match ev {
        SessionEvent::ChatPlayerNotFound { name } => chat_player_not_found(&name, &mut errors),
        SessionEvent::ChatWrongFaction => chat_wrong_faction(&mut errors),
        _ => {}
    }
}

/// The four world broadcasts, parked for [`super::broadcast`]'s resolve pass.
fn on_broadcast(In(ev): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    let b = match ev {
        SessionEvent::ZoneUnderAttack { area_id } => Broadcast::ZoneUnderAttack { area_id },
        SessionEvent::DefenseMessage { zone_id, text } => Broadcast::Defense { zone_id, text },
        SessionEvent::ServerMessage { message_type, text } => {
            Broadcast::Server { message_type, text }
        }
        _ => return,
    };
    broadcast(b, &mut chat_log);
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

fn on_chat_restricted(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::ChatRestricted { kind } = ev {
        errors.0.extend(chat_restricted_key(kind).map(UiError::key));
    }
}

fn on_toast(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<UiErrorTexts>,
    mut chat_log: ResMut<ChatLog>,
) {
    match ev {
        SessionEvent::Notification { text } => notification(&text, &mut errors, &mut chat_log),
        SessionEvent::AreaTriggerMessage { text } => area_trigger_message(text, &mut errors),
        _ => {}
    }
}

/// Fill the mailbox `feed::played_time_bridge` fires as `TIME_PLAYED_MSG`, whose stock handler
/// prints the chat lines; the breakdown goes to the console (`0x401400`-`0x40144c`), the log here.
fn on_played_time(
    In(ev): In<SessionEvent>,
    mut answer: ResMut<PlayedTimeAnswer>,
    mut clock: ResMut<crate::net::PlayedClock>,
) {
    if let SessionEvent::PlayedTime { total, level, show } = ev {
        debug!("net: played time {total} s (level {level} s, shown {show})");
        clock.0 = Some((total, std::time::Instant::now()));
        // Only a shown answer prints and fires `TIME_PLAYED_MSG` (`0x4013f6`).
        if show {
            answer.0 = Some((total, level));
            info!(
                "console: total time played {}, this level {}",
                played_breakdown(total),
                played_breakdown(level)
            );
        }
    }
}

fn on_channel_notify(
    In(ev): In<SessionEvent>,
    mut chat_log: ResMut<ChatLog>,
    social: Res<SocialState>,
) {
    if let SessionEvent::ChannelNotify {
        notice,
        channel,
        tail,
    } = ev
    {
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
        chat_log.push_channel_notice(notice, channel, &tail);
    }
}

/// `MSG_RANDOM_ROLL` (`0x6e37af`): an ignored roller's roll shows nothing (`0x6e3801`).
fn on_random_roll(
    In(ev): In<SessionEvent>,
    mut chat_log: ResMut<ChatLog>,
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

/// Clear the chat log's session state when the session ends.
fn on_session_end(In(_): In<SessionEvent>, mut chat_log: ResMut<ChatLog>) {
    chat_log.clear_session();
}

/// A spoken line (`SMSG_MESSAGECHAT`). System lines (`0x0A`), the server's answers to GM
/// commands, log at `info!` and go out as [`ServerSaidMessage`]; other chat logs at `debug!`.
fn chat(
    m: ChatMessage,
    chat_log: &mut ChatLog,
    social: &SocialState,
    net_commands: &NetCommands,
    server_said: &mut MessageWriter<ServerSaidMessage>,
) {
    // Addon traffic: 1.12 has no addon opcode or chat type, so `LANG_ADDON` on any lane is the
    // whole test. The line never renders; it fires `CHAT_MSG_ADDON` once the sender's name
    // resolves. An ignored sender's fires nothing, as the reference drops ignored lines first, and
    // needs no `CMSG_CHAT_IGNORED`: vmangos refuses addon whispers (`ChatHandler.cpp:84-103`).
    if m.is_addon() {
        if !social.is_ignored(m.sender_guid) {
            debug!(
                "net: addon chat [{:#04x}] from {:#x}: {:?} (suppressed — not a chat line)",
                m.chat_type, m.sender_guid, m.text
            );
            if benilla_assets::trace::enabled() {
                // `<-` inbound; the outbound drain writes `->` under the same tag.
                benilla_assets::trace::line(
                    "addon",
                    &format!("<- [{:#04x}] {:?}", m.chat_type, m.text),
                );
            }
            chat_log.push_addon(&m.text, m.chat_type, m.sender_guid);
        }
        return;
    }
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
    // An ignored speaker is dropped silently (the ignore list); a dropped whisper also
    // sends `CMSG_CHAT_IGNORED`, so the sender hears "is ignoring you". Whisper echoes, ignore
    // notices and restricted lines still post (`0x50c195`-`0x50c19d`).
    if social.is_ignored(m.sender_guid) {
        if m.chat_type == CHAT_MSG_WHISPER {
            let _ = net_commands.0.send(ClientCommand::ChatIgnored {
                guid: m.sender_guid,
            });
        }
        use benilla_protocol::messages as msg;
        if ![
            msg::CHAT_MSG_WHISPER_INFORM,
            msg::CHAT_MSG_IGNORED,
            msg::CHAT_MSG_RESTRICTED,
        ]
        .contains(&m.chat_type)
        {
            return;
        }
    }
    chat_log.push_wire(m);
}

/// The `/chatlist` roster (`SMSG_CHANNEL_LIST`). Members arrive as guids and are not resolved, so
/// the line gives their count where the reference lists their names.
fn channel_list(channel: String, members: &[(u64, u8)], chat_log: &mut ChatLog) {
    let mut ev = ChatEvent::text_only(
        ChatEventKind::ChannelList,
        format!("{} member(s)", members.len()),
    );
    ev.channel = channel;
    chat_log.push_event(ev);
}

/// A whisper target is offline: message 241, `ERR_CHAT_PLAYER_NOT_FOUND_S`, with the server's
/// name. The key travels, since there is no VM here; its row routes it to chat.
fn chat_player_not_found(name: &str, errors: &mut crate::ui_action::UiErrorKeys) {
    errors.0.push(crate::ui_action::UiError::s(
        "ERR_CHAT_PLAYER_NOT_FOUND_S",
        name,
    ));
}

/// A cross-faction whisper was refused: message 240, `ERR_CHAT_WRONG_FACTION`.
fn chat_wrong_faction(errors: &mut crate::ui_action::UiErrorKeys) {
    errors
        .0
        .push(crate::ui_action::UiError::key("ERR_CHAT_WRONG_FACTION"));
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
fn notification(text: &str, errors: &mut UiErrorTexts, chat_log: &mut ChatLog) {
    let route = notice_route(text);
    info!("net: notification — {}", route.text);
    match route.toast {
        Some(MsgKind::Error) => errors.error(route.text.to_string()),
        Some(_) => errors.info(route.text.to_string()),
        None => {}
    }
    let kind = route
        .chat
        .and_then(|t| u8::try_from(t).ok())
        .and_then(benilla_protocol::messages::wotlk_chat_type_shared)
        .and_then(super::event::kind_of_wire);
    if let Some(kind) = kind {
        chat_log.push_event(ChatEvent::text_only(kind, route.text.to_string()));
    }
}

/// An area trigger's refusal (`SMSG_AREA_TRIGGER_MESSAGE`, `0x2b8`): the yellow `UIErrorsFrame`
/// line, `UI_INFO_MESSAGE` from `(text, 0)`.
fn area_trigger_message(text: String, errors: &mut UiErrorTexts) {
    info!("net: area-trigger message — {text}");
    errors.info(text);
}

/// Park a world broadcast for `ui_chat::broadcast`'s resolve pass, logged: a defense broadcast
/// outside the defense channels prints nothing, so the log is its only trace.
fn broadcast(b: Broadcast, chat_log: &mut ChatLog) {
    info!("net: world broadcast — {b:?}");
    chat_log.push_broadcast(b);
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
            channel: None,
            text: text.to_string(),
            chat_tag: 0,
            achievement: 0,
        }
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
        world
            .run_system_once(
                move |mut chat_log: ResMut<ChatLog>,
                      social: Res<SocialState>,
                      net_commands: Res<NetCommands>,
                      mut server_said: MessageWriter<ServerSaidMessage>| {
                    chat(
                        m.clone(),
                        &mut chat_log,
                        &social,
                        &net_commands,
                        &mut server_said,
                    );
                },
            )
            .unwrap();
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
        world
            .run_system_once(
                move |mut chat_log: ResMut<ChatLog>,
                      social: Res<SocialState>,
                      net_commands: Res<NetCommands>,
                      mut server_said: MessageWriter<ServerSaidMessage>| {
                    chat(
                        m.clone(),
                        &mut chat_log,
                        &social,
                        &net_commands,
                        &mut server_said,
                    );
                },
            )
            .unwrap();
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

        let mut errors = UiErrorTexts::default();
        let mut log = ChatLog::default();
        notification("GM mode is ON", &mut errors, &mut log);
        assert_eq!(
            errors.0,
            [(
                "GM mode is ON".to_string(),
                benilla_ui::messages::MsgKind::Error
            )],
            "the SendNotification half is the RED toast, never a second line"
        );
        assert_eq!(log.pending_len(), 0);
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
        let mut errors = UiErrorTexts::default();
        let mut log = ChatLog::default();
        notification("~#^1hello", &mut errors, &mut log);
        notification("~#plain system", &mut errors, &mut log);
        notification("~#^/", &mut errors, &mut log);
        assert!(errors.0.is_empty());
        assert_eq!(
            log.pending_len(),
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

    /// `0x6e3801`: a roll from an ignored player never queues; any other does.
    #[test]
    fn an_ignored_players_roll_is_dropped() {
        let mut world = World::new();
        world.insert_resource(ChatLog::default());
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
            world.resource::<ChatLog>().pending_len(),
            1,
            "only 0x22's roll queues"
        );
    }

    #[test]
    fn an_area_trigger_refusal_takes_the_yellow_arm() {
        let mut errors = UiErrorTexts::default();
        area_trigger_message(
            "You must be at least level 58 to enter.".to_string(),
            &mut errors,
        );
        assert_eq!(
            errors.0,
            [(
                "You must be at least level 58 to enter.".to_string(),
                benilla_ui::messages::MsgKind::Info
            )]
        );
    }
}
