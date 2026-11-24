//! The chat add in its caller's turn (`0x509dd0`): lines queued outside the world until the UI's
//! entry (`0x509f48`, `0x50dfc3`), lines held on a missing name until its answer's callback
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
        f:RegisterEvent("CHAT_MSG_MONSTER_SAY")
        f:SetScript("OnEvent", function(_, e, a1, a2, a3, a4)
            Seen[#Seen + 1] = e .. ":" .. tostring(a1) .. ":" .. tostring(a4)
        end)
    "#,
    )
    .unwrap();
    let mut world = World::new();
    world.insert_non_send_resource(s);
    let sent = super::feed::insert_add_resources(&mut world);
    // Outside the world until a test enters it (`0x4fb190`).
    world.insert_resource(super::ChatLog::default());
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

/// Outside the world every line queues (`0x509f48`), its sender's name asked first with the
/// answer's callback (`0x509e51`); the UI's entry adds them in order (`0x50dfc3`), and one whose
/// sender is still not cached waits for its answer, asked once.
#[test]
fn lines_before_the_world_entry_are_added_at_it_and_an_unnamed_one_at_its_answer() {
    let (mut world, sent) = world();
    add(&mut world, system_line("first"));
    add(&mut world, addon_line());
    assert!(
        matches!(
            sent.try_recv(),
            Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
        ),
        "the add asks the name ahead of its queue test"
    );
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
    assert!(sent.try_recv().is_err(), "asked once");

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

/// The UI's world exit queues the add again (`0x528cc9` -> `0x4fb190`): a line during a loading
/// screen waits for the entry, and a held line whose name lands meanwhile is marked queued
/// (`0x503e78`), so the entry adds both in hold order (`0x50dfc3`).
#[test]
fn the_world_exit_queues_lines_until_the_next_entry() {
    let (mut world, _sent) = world();
    enter_world(&mut world);
    add(&mut world, addon_line());
    world.init_resource::<crate::ui_script::LeavingWorldArmed>();
    world.init_resource::<Messages<crate::ui_script::WorldLeaveSweepMessage>>();
    world
        .resource_mut::<crate::ui_script::LeavingWorldArmed>()
        .arm();
    assert!(crate::ui_script::run_leave_world_sweep(&mut world));
    add(&mut world, system_line("loading"));
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    name_landed(&mut world);
    assert!(
        seen(&world).is_empty(),
        "nothing is added on the loading screen"
    );
    enter_world(&mut world);
    assert_eq!(
        seen(&world),
        ["CHAT_MSG_ADDON:oRA:Ann", "CHAT_MSG_SYSTEM:loading:"]
    );
}

/// The XP line reads its victim's name in the call (`0x50b20a` -> `0x72a000`): a name not cached
/// is asked and reads `UNKNOWNOBJECT`, and the line is added at once, never held.
#[test]
fn an_xp_line_with_an_uncached_victim_is_added_in_the_call() {
    let (mut world, sent) = world();
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"
            COMBATLOG_XPGAIN_FIRSTPERSON = "%s dies, you gain %d experience."
            UNKNOWNOBJECT = "Unknown"
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_COMBAT_XP_GAIN")
            f:SetScript("OnEvent", function(_, e, a1) Seen[#Seen + 1] = e .. ":" .. a1 end)
        "#,
        )
        .unwrap();
    enter_world(&mut world);
    add(
        &mut world,
        ChatLine::XpGain {
            line: super::XpLine::Plain {
                victim: ANN,
                total: 35,
                group_rate: 1.0,
            },
        },
    );
    assert_eq!(
        seen(&world),
        ["CHAT_MSG_COMBAT_XP_GAIN:Unknown dies, you gain 35 experience."]
    );
    assert!(matches!(
        sent.try_recv(),
        Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
    ));
    assert_eq!(world.resource::<super::ChatLog>().pending_len(), 0);
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
        "CHAT_YOU_JOINED_NOTICE" => Some("Joined Channel: %d [%s]".to_string()),
        _ => None,
    };
    let say = ChatEvent {
        kind: Some(ChatEventKind::Say),
        text: "buy |cff1eff00|Hitem:2589:0|h[Linen Cloth]|h|r at 50% || cheap".into(),
        sender: "Ann".into(),
        language: "Orcish".into(),
        language_id: 1,
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&say, ChatEventKind::Say, Some(7), &strings).unwrap(),
        "Ann says: [Orcish] buy Linen Cloth at 50% | cheap"
    );
    // The player's own default (Orcish for a Horde race) and no active player print no header
    // (`0x50abb7`-`0x50abc9`).
    for default in [Some(1), None] {
        assert_eq!(
            super::frames::log_line(&say, ChatEventKind::Say, default, &strings).unwrap(),
            "Ann says: buy Linen Cloth at 50% | cheap",
            "{default:?}"
        );
    }
    let channel = ChatEvent {
        kind: Some(ChatEventKind::Channel),
        text: "lfg".into(),
        sender: "Ann".into(),
        channel: "2. Trade - City".into(),
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&channel, ChatEventKind::Channel, Some(7), &strings).unwrap(),
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
        super::frames::log_line(&monster, ChatEventKind::MonsterSay, Some(7), &strings).unwrap(),
        "Hogger says: Hogger attacks!"
    );
    let joined = ChatEvent {
        kind: Some(ChatEventKind::ChannelNotice),
        text: "YOU_JOINED".into(),
        channel: "1. General".into(),
        channel_number: 1,
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&joined, ChatEventKind::ChannelNotice, Some(7), &strings).unwrap(),
        "Joined Channel: 1 [1. General]"
    );
}

/// A monster's line to `target`, its receiver's name as the packet carries it.
fn monster_say(target: u64, receiver: Option<&str>, text: &str) -> ChatLine {
    ChatLine::Wire {
        msg: benilla_protocol::messages::ChatMessage {
            chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
            language: 0,
            sender_guid: 0xF130_0000_0001_0001,
            target_guid: target,
            sender_name: Some("Hogger".into()),
            receiver_name: receiver.map(Into::into),
            channel: None,
            text: text.into(),
            chat_tag: 0,
            achievement: 0,
            wire_type: 0x0C,
            sender_word: 0,
        },
    }
}

/// A creature receiver's name comes with the packet (`0x50c2df`-`0x50c30e`) and the parser takes it
/// first (`0x579590`): the line is added in the call, nothing asked or held. Only a player or pet
/// receiver sent without its name holds the line (`0x509f98`-`0x509fd2`), asked once with the
/// answer's callback (`0x509f33`), and a line whose tokens fail shows the wire text (`0x50a17d`).
#[test]
fn a_monster_line_holds_only_on_an_unnamed_player_or_pet_receiver() {
    let (mut world, sent) = world();
    enter_world(&mut world);
    add(
        &mut world,
        monster_say(0xF130_0000_0002_0002, Some("Kobold"), "$n, die!"),
    );
    assert_eq!(seen(&world), ["CHAT_MSG_MONSTER_SAY:Kobold, die!:"]);
    assert!(
        sent.try_recv().is_err(),
        "a creature receiver is never asked"
    );
    assert_eq!(world.resource::<super::ChatLog>().pending_len(), 0);

    add(&mut world, monster_say(ANN, None, "$n, die!"));
    assert_eq!(seen(&world).len(), 1, "held on the player's name");
    assert!(matches!(
        sent.try_recv(),
        Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
    ));
    // No receiver at all: every person token fails, and the wire text shows whole.
    add(&mut world, monster_say(0, None, "$n, die!"));
    assert_eq!(seen(&world)[1], "CHAT_MSG_MONSTER_SAY:$n, die!:");
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    name_landed(&mut world);
    assert_eq!(seen(&world)[2], "CHAT_MSG_MONSTER_SAY:Ann, die!:");
    assert_eq!(world.resource::<super::ChatLog>().pending_len(), 0);
}

/// A player's wire line to nobody, `chat_type` from `sender` with its name not sent.
fn player_line(chat_type: u8, sender: u64, language: u32, text: &str) -> ChatLine {
    ChatLine::Wire {
        msg: benilla_protocol::messages::ChatMessage {
            chat_type,
            language,
            sender_guid: sender,
            target_guid: 0,
            sender_name: None,
            receiver_name: None,
            channel: None,
            text: text.into(),
            chat_tag: 0,
            achievement: 0,
            wire_type: 0,
            sender_word: 0,
        },
    }
}

/// Record each `event`'s first three arguments in `Heard`.
fn hear(world: &World, events: &[&str]) {
    let s = world.non_send_resource::<UiScript>();
    s.run("Heard = {}").unwrap();
    for e in events {
        s.run(&format!(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("{e}")
            f:SetScript("OnEvent", function(_, e, a1, a2, a3)
                Heard[#Heard + 1] = e .. ":" .. a1 .. ":" .. a2 .. ":" .. a3
            end)
        "#
        ))
        .unwrap();
    }
}

fn heard(world: &World) -> Vec<String> {
    world
        .non_send_resource::<UiScript>()
        .eval::<Vec<String>>("return Heard")
        .unwrap()
}

/// The add waits on the sender's guid, not the line's type (`0x509f5d`-`0x509f92`): a party
/// leader's line from a player not cached is asked with the answer's callback (`0x509e51`) and held
/// until the answer adds it, and a creature guid on a player's type is added at once with no name.
#[test]
fn a_wire_line_waits_on_a_player_sender_whatever_its_type() {
    use benilla_protocol::messages::{CHAT_MSG_PARTY_LEADER, CHAT_MSG_SAY};
    let (mut world, sent) = world();
    hear(&world, &["CHAT_MSG_PARTY_LEADER", "CHAT_MSG_SAY"]);
    enter_world(&mut world);
    add(
        &mut world,
        player_line(CHAT_MSG_PARTY_LEADER, ANN, 0, "pull"),
    );
    assert!(heard(&world).is_empty(), "held on Ann's name");
    assert!(matches!(
        sent.try_recv(),
        Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
    ));
    add(
        &mut world,
        player_line(CHAT_MSG_SAY, 0xF130_0000_0001_0001, 0, "grr"),
    );
    assert_eq!(heard(&world), ["CHAT_MSG_SAY:grr::"]);
    assert!(sent.try_recv().is_err(), "a creature sender is never asked");
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    name_landed(&mut world);
    assert_eq!(heard(&world)[1], "CHAT_MSG_PARTY_LEADER:pull:Ann:");
    assert_eq!(world.resource::<super::ChatLog>().pending_len(), 0);
}
