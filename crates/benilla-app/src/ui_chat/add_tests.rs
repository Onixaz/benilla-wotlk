//! The chat add in its caller's turn (`0x509dd0`): lines queued outside the world until the UI's
//! entry (`0x509f48`, `0x50ae30`), lines held on a missing name until its answer's callback
//! (`0x509f92` -> `0x50ad82`, `0x503c80`), and the chat log's own composition (`0x50ab09`).

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::event::{ChatEvent, ChatEventKind};
use super::feed::{add, enter_world, name_landed, ChatLine};

const ANN: u64 = 0x11;

/// A world holding everything the add reads, with a VM whose spy records each chat event.
fn world() -> (
    World,
    crossbeam_channel::Receiver<crate::net::ClientCommand>,
) {
    let s = UiScript::new().unwrap();
    s.run(
        r#"
        Seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("CHAT_MSG_ADDON")
        f:RegisterEvent("CHAT_MSG_SYSTEM")
        f:SetScript("OnEvent", function(_, e, a1, a2, a3, a4)
            Seen[#Seen + 1] = e .. ":" .. tostring(a1) .. ":" .. tostring(a4)
        end)
    "#,
    )
    .unwrap();
    let (tx, sent) = crossbeam_channel::unbounded();
    let mut world = World::new();
    world.insert_non_send_resource(s);
    world.insert_resource(super::ChatLog::default());
    world.insert_resource(super::frames::ChatWindows::default());
    world.insert_resource(super::edit::ChannelState::default());
    world.insert_resource(crate::names::NameCache::default());
    world.insert_resource(crate::chat_bubble::BubbleQueue::default());
    world.insert_resource(crate::chat_bubble::BubbleConfig::default());
    world.insert_resource(crate::creature_anim::GestureQueue::default());
    world.insert_resource(crate::net::NetCommands(tx));
    world.insert_resource(crate::net::SelfGuid::default());
    world.insert_resource(crate::net::GuidIndex::default());
    world.insert_resource(crate::world_state::WorldStates::default());
    world.insert_resource(super::language::ChatLanguages::default());
    world.insert_resource(crate::items::Items::default());
    world.insert_resource(crate::text_filter::TextFilter::default());
    world.insert_resource(crate::text_filter::TextFilterSwitches::default());
    world.insert_resource(crate::net::ServerTime::default());
    (world, sent)
}

fn seen(world: &World) -> Vec<String> {
    world
        .non_send_resource::<UiScript>()
        .eval::<Vec<String>>("return Seen")
        .unwrap()
}

fn addon_line() -> ChatLine {
    ChatLine::Addon {
        prefix: "oRA".into(),
        message: "SYNC".into(),
        distribution: "PARTY".into(),
        guid: ANN,
    }
}

fn system_line(text: &str) -> ChatLine {
    ChatEvent::text_only(ChatEventKind::System, text.into()).into()
}

/// Outside the world every line queues (`0x509f48`); the UI's entry adds them in order
/// (`0x50ae30`), and one whose sender is not cached waits for its answer, asked once.
#[test]
fn lines_before_the_world_entry_are_added_at_it_and_an_unnamed_one_at_its_answer() {
    let (mut world, sent) = world();
    add(&mut world, system_line("first"));
    add(&mut world, addon_line());
    add(&mut world, system_line("second"));
    assert!(
        seen(&world).is_empty(),
        "nothing is added outside the world"
    );
    assert_eq!(world.resource::<super::ChatLog>().pending_len(), 2);

    enter_world(&mut world);
    assert_eq!(
        seen(&world),
        ["CHAT_MSG_SYSTEM:first:", "CHAT_MSG_SYSTEM:second:"],
        "the queued lines in order; the addon line waits on Ann's name"
    );
    assert!(matches!(
        sent.try_recv(),
        Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
    ));

    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    name_landed(&mut world);
    assert_eq!(seen(&world)[2], "CHAT_MSG_ADDON:oRA:Ann");
    assert_eq!(world.resource::<super::ChatLog>().pending_addons(), []);
}

/// In the world a line is added in the call, its event raised before the caller goes on.
#[test]
fn a_line_in_the_world_is_added_in_the_call() {
    let (mut world, _sent) = world();
    enter_world(&mut world);
    add(&mut world, system_line("now"));
    assert_eq!(seen(&world), ["CHAT_MSG_SYSTEM:now:"]);
}

/// A name answered empty drops the lines waiting on it (`0x503e5c`): no `"Unknown"` line.
#[test]
fn a_name_answered_empty_drops_its_waiting_line() {
    let (mut world, _sent) = world();
    enter_world(&mut world);
    add(&mut world, addon_line());
    assert_eq!(world.resource::<super::ChatLog>().pending_addons().len(), 1);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, String::new(), None);
    name_landed(&mut world);
    assert!(seen(&world).is_empty());
    assert!(world
        .resource::<super::ChatLog>()
        .pending_addons()
        .is_empty());
}

/// The log's composition (`0x50ab09`): the channel prefix cut at its zone, the sender filled into
/// `CHAT_<TYPE>_GET`, the language header off the default, links collapsed to their names and `%`
/// doubled, then the whole formatted with the sender.
#[test]
fn the_log_line_is_the_add_s_own_composition() {
    let strings = |key: &str| match key {
        "CHAT_SAY_GET" => Some("%s says: ".to_string()),
        "CHAT_CHANNEL_GET" => Some("%s: ".to_string()),
        "CHAT_MONSTER_SAY_GET" => Some("%s says: ".to_string()),
        "CHAT_YOU_JOINED_NOTICE" => Some("Joined Channel: [%s]".to_string()),
        _ => None,
    };
    let say = ChatEvent {
        kind: Some(ChatEventKind::Say),
        text: "buy |cff1eff00|Hitem:2589:0|h[Linen Cloth]|h|r at 50% || cheap".into(),
        sender: "Ann".into(),
        language: "Orcish".into(),
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&say, ChatEventKind::Say, "Common", &strings).unwrap(),
        "Ann says: [Orcish] buy Linen Cloth at 50% | cheap"
    );
    let channel = ChatEvent {
        kind: Some(ChatEventKind::Channel),
        text: "lfg".into(),
        sender: "Ann".into(),
        channel: "2. Trade - City".into(),
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&channel, ChatEventKind::Channel, "Common", &strings).unwrap(),
        "[2. Trade] Ann: lfg"
    );
    // A monster line's own `%s` is the name's slot: not doubled, filled at the write.
    let monster = ChatEvent {
        kind: Some(ChatEventKind::MonsterSay),
        text: "%s attacks!".into(),
        sender: "Hogger".into(),
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&monster, ChatEventKind::MonsterSay, "Common", &strings).unwrap(),
        "Hogger says: Hogger attacks!"
    );
    let joined = ChatEvent {
        kind: Some(ChatEventKind::ChannelNotice),
        text: "YOU_JOINED".into(),
        channel: "1. General".into(),
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&joined, ChatEventKind::ChannelNotice, "Common", &strings).unwrap(),
        "Joined Channel: [1. General]"
    );
}
