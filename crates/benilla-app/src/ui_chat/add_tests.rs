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
    super::feed::addon_line("oRA\tSYNC", benilla_protocol::messages::CHAT_MSG_PARTY, ANN)
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

/// arg3 is the effective language's `Languages.dbc` record name (`0x4fdbd5`-`0x4fdc00`), the
/// same record the log's header reads (`0x50abd6`), so a 3.3.5a language such as Draenei (35)
/// is named, and an id with no record is empty.
#[test]
fn a_line_names_its_language_off_its_record() {
    use benilla_protocol::messages::CHAT_MSG_SAY;
    let (mut world, _sent) = world();
    hear(&world, &["CHAT_MSG_SAY"]);
    world.insert_resource(super::language::ChatLanguages::with_player());
    world.insert_resource(crate::ui_unit::LanguagesRes(
        benilla_formats::Languages::from_rows(&[(7, "Common"), (35, "Draenei")]),
    ));
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    enter_world(&mut world);
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 35, "hi"));
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 99, "hi"));
    assert_eq!(
        heard(&world),
        ["CHAT_MSG_SAY:hi:Ann:Draenei", "CHAT_MSG_SAY:hi:Ann:"]
    );
    let strings = |key: &str| (key == "CHAT_SAY_GET").then(|| "%s says: ".to_string());
    let event = ChatEvent {
        kind: Some(ChatEventKind::Say),
        text: "hi".into(),
        sender: "Ann".into(),
        language: "Draenei".into(),
        language_id: 35,
        ..Default::default()
    };
    assert_eq!(
        super::frames::log_line(&event, ChatEventKind::Say, Some(7), &strings).unwrap(),
        "Ann says: [Draenei] hi"
    );
}

/// An addon line holds only on a nonzero player or pet sender with no name (`0x509f5d`-`0x509f92`):
/// from guid 0 or a creature it fires `CHAT_MSG_ADDON` at once with `""`, nothing asked or held.
/// The distribution is the type's (`0x50a09e`): a whisper's and a Battle.net line's `"WHISPER"`,
/// a type outside the table `"UNKNOWN"`.
#[test]
fn an_addon_line_from_no_player_fires_at_once_with_an_empty_sender() {
    use benilla_protocol::messages as m;
    let (mut world, sent) = world();
    hear(&world, &["CHAT_MSG_ADDON"]);
    enter_world(&mut world);
    add(
        &mut world,
        super::feed::addon_line("A\tx", m::CHAT_MSG_WHISPER, 0),
    );
    add(
        &mut world,
        super::feed::addon_line("B\ty", m::CHAT_MSG_BATTLENET, 0xF130_0000_0001_0001),
    );
    add(
        &mut world,
        super::feed::addon_line("C\tz", m::CHAT_MSG_RAID_LEADER, 0),
    );
    assert_eq!(
        heard(&world),
        [
            "CHAT_MSG_ADDON:A:x:WHISPER",
            "CHAT_MSG_ADDON:B:y:WHISPER",
            "CHAT_MSG_ADDON:C:z:UNKNOWN"
        ]
    );
    assert_eq!(
        seen(&world),
        [
            "CHAT_MSG_ADDON:A:",
            "CHAT_MSG_ADDON:B:",
            "CHAT_MSG_ADDON:C:"
        ],
        "no name to read: the sender is empty"
    );
    assert!(sent.try_recv().is_err(), "nothing is asked");
    assert_eq!(world.resource::<super::ChatLog>().pending_count(), 0);
}

/// A denied answer leaves no record (`0x67a2d0`), so a later line naming that guid asks again with
/// the callback (`0x509e51`) and waits, an unrelated answer leaving it; the next denial drops it
/// (`0x503e5c`), and nothing stays held.
#[test]
fn a_line_from_a_denied_name_asks_again_and_drops_on_the_next_denial() {
    use benilla_protocol::messages::CHAT_MSG_SAY;
    let (mut world, sent) = world();
    enter_world(&mut world);
    add(&mut world, addon_line());
    assert!(matches!(
        sent.try_recv(),
        Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
    ));
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, String::new(), None);
    name_landed(&mut world);
    assert_eq!(world.resource::<super::ChatLog>().pending_count(), 0);

    add(&mut world, addon_line());
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 0, "hi"));
    assert!(
        matches!(
            sent.try_recv(),
            Ok(crate::net::ClientCommand::NameQuery { guid: ANN })
        ),
        "the denied guid is asked again"
    );
    assert!(sent.try_recv().is_err(), "once");
    assert_eq!(world.resource::<super::ChatLog>().pending_count(), 2);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(0x22, "Bob".into(), None);
    name_landed(&mut world);
    assert_eq!(
        world.resource::<super::ChatLog>().pending_count(),
        2,
        "another guid's answer leaves them waiting"
    );
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, String::new(), None);
    name_landed(&mut world);
    assert_eq!(world.resource::<super::ChatLog>().pending_count(), 0);
    assert!(seen(&world).is_empty());
}

/// A cached sender is named by its record's full name, `Name-Realm` for another realm's player
/// (`0x509e6a` -> `0x4fd220`); a line held for its name is added by the answer's callback with the
/// record's bare name (`0x503e98`).
#[test]
fn a_cached_sender_reads_its_full_name_and_a_held_one_its_bare_name() {
    use benilla_protocol::messages::CHAT_MSG_SAY;
    const BOB: u64 = 0x22;
    let (mut world, _sent) = world();
    hear(&world, &["CHAT_MSG_SAY"]);
    enter_world(&mut world);
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(ANN, "Ann".into(), None);
        names.set_player_realm(ANN, "Icecrown".into());
    }
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 0, "hi"));
    add(&mut world, player_line(CHAT_MSG_SAY, BOB, 0, "yo"));
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(BOB, "Bob".into(), None);
        names.set_player_realm(BOB, "Icecrown".into());
    }
    name_landed(&mut world);
    assert_eq!(
        heard(&world),
        ["CHAT_MSG_SAY:hi:Ann-Icecrown:", "CHAT_MSG_SAY:yo:Bob:"]
    );
}

/// An empty say or yell reads its type's unknown line as an emote, an empty emote
/// `CHAT_EMOTE_UNKNOWN`, each in the `_FEMALE` form for a female sender record when the VM has one
/// (`0x50a291`-`0x50a2df`, `0x819d86`).
#[test]
fn an_empty_text_reads_its_types_unknown_line() {
    use benilla_protocol::messages::{CHAT_MSG_EMOTE, CHAT_MSG_SAY, CHAT_MSG_YELL};
    const BOB: u64 = 0x22;
    let (mut world, _sent) = world();
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"
            CHAT_SAY_UNKNOWN = "says something unintelligible."
            CHAT_SAY_UNKNOWN_FEMALE = "says something unintelligible, she."
            CHAT_YELL_UNKNOWN = "yells something unintelligible."
            CHAT_EMOTE_UNKNOWN = "makes some strange gestures."
        "#,
        )
        .unwrap();
    hear(&world, &["CHAT_MSG_SAY", "CHAT_MSG_YELL", "CHAT_MSG_EMOTE"]);
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(ANN, "Ann".into(), Some((1, 1, 0)));
        names.insert_player(BOB, "Bob".into(), Some((1, 1, 1)));
    }
    enter_world(&mut world);
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 0, ""));
    add(&mut world, player_line(CHAT_MSG_SAY, BOB, 0, ""));
    add(&mut world, player_line(CHAT_MSG_YELL, BOB, 0, ""));
    add(&mut world, player_line(CHAT_MSG_EMOTE, ANN, 0, ""));
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 0, "hi"));
    assert_eq!(
        heard(&world),
        [
            "CHAT_MSG_EMOTE:says something unintelligible.:Ann:",
            "CHAT_MSG_EMOTE:says something unintelligible, she.:Bob:",
            "CHAT_MSG_EMOTE:yells something unintelligible.:Bob:",
            "CHAT_MSG_EMOTE:makes some strange gestures.:Ann:",
            "CHAT_MSG_SAY:hi:Ann:",
        ]
    );
}

/// Each quest link takes the difficulty colour of its level against the player's over the ten
/// bytes before it (`0x50a2ec`-`0x50a393`): -1 is the player's own level, a `||` escape is left,
/// and with no active player every link reads yellow (`0x61e562`).
#[test]
fn quest_links_take_the_players_difficulty_colour() {
    use super::feed::recolor_quest_links as recolor;
    let link = |level: &str| format!("|cff000000|Hquest:123:{level}|h[Q]|h|r");
    assert_eq!(
        recolor(link("80"), Some(70)),
        "|cffff2020|Hquest:123:80|h[Q]|h|r"
    );
    assert_eq!(
        recolor(link("73"), Some(70)),
        "|cffff8040|Hquest:123:73|h[Q]|h|r"
    );
    assert_eq!(
        recolor(link("-1"), Some(70)),
        "|cffffff00|Hquest:123:-1|h[Q]|h|r"
    );
    assert_eq!(
        recolor(link("80"), None),
        "|cffffff00|Hquest:123:80|h[Q]|h|r"
    );
    let yellow = "|cffffff00|Hquest:123:80|h[Q]|h|r";
    assert_eq!(
        recolor(format!("a {} b {}", link("80"), link("80")), Some(80)),
        format!("a {yellow} b {yellow}")
    );
    let escaped = "|cff000000||Hquest:1:80|h".to_string();
    assert_eq!(recolor(escaped.clone(), Some(1)), escaped);
}

/// A monster's say or yell bubble reads each `%%` as `%` (`0x50a92a`-`0x50a960`).
#[test]
fn a_monster_bubble_reads_a_doubled_percent_as_one() {
    assert_eq!(super::feed::unescape_percent("50%% off %%%"), "50% off %%");
    assert_eq!(super::feed::unescape_percent("100%"), "100%");
}

/// The chat session end zeroes both logging flags (`0x5072d6`), and it runs at the unattended
/// reconnect's re-entry, an exit from the world to the world.
#[test]
fn the_session_end_zeroes_the_logging_flags_at_a_reconnects_re_entry() {
    use crate::char_select::ClientState;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
        .insert_state(ClientState::InWorld)
        .init_resource::<super::ChatLog>()
        .init_resource::<super::frames::ChatWindows>()
        .init_resource::<super::logging::CombatLogFile>()
        .add_systems(OnExit(ClientState::InWorld), super::end_session_chat);
    let mut s = UiScript::new().unwrap();
    s.set_logging_flags(true, true);
    app.insert_non_send_resource(s);
    app.update();
    assert_eq!(
        app.world().non_send_resource::<UiScript>().logging_flags(),
        (true, true)
    );
    app.world_mut()
        .resource_mut::<NextState<ClientState>>()
        .set(ClientState::InWorld);
    app.update();
    assert_eq!(
        app.world().non_send_resource::<UiScript>().logging_flags(),
        (false, false)
    );
}

/// A whisper the spam filter drops is reported with its sender's guid (`0x50a6c4`-`0x50a718`), a
/// foreign whisper too, since the add takes it as type 7 (`0x50bfc7`); another type's drop sends
/// nothing, and a system line is never filtered (`0x50a6a7`).
#[test]
fn a_spam_filtered_whisper_is_reported_and_a_system_line_never_filtered() {
    use benilla_protocol::messages::{CHAT_MSG_SAY, CHAT_MSG_SYSTEM, CHAT_MSG_WHISPER};
    let (mut world, sent) = world();
    hear(&world, &["CHAT_MSG_SYSTEM"]);
    world
        .resource_mut::<crate::text_filter::TextFilter>()
        .set_server_spam(&["gold".to_string()]);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    enter_world(&mut world);
    let whisper = |sender: u64, name: Option<&str>| {
        let mut line = player_line(CHAT_MSG_WHISPER, sender, 0, "cheap gold");
        let ChatLine::Wire { msg } = &mut line else {
            unreachable!()
        };
        msg.wire_type = benilla_protocol::messages::wotlk_chat_type::WHISPER;
        msg.sender_name = name.map(Into::into);
        line
    };
    add(&mut world, whisper(ANN, None));
    // A foreign whisper, as the decoder hands it on: its sender's name inline, type 7.
    add(&mut world, whisper(0x22, Some("Bob-Other")));
    add(&mut world, whisper(0, None));
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 0, "cheap gold"));
    add(&mut world, player_line(CHAT_MSG_SYSTEM, 0, 0, "cheap gold"));
    let sent: Vec<_> = sent.try_iter().collect();
    assert!(
        matches!(
            sent.as_slice(),
            [
                crate::net::ClientCommand::ChatFiltered { guid: ANN },
                crate::net::ClientCommand::ChatFiltered { guid: 0x22 }
            ]
        ),
        "{}",
        sent.len()
    );
    assert_eq!(heard(&world), ["CHAT_MSG_SYSTEM:cheap gold::"]);
}
