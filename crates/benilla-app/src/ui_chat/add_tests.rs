//! The chat add in its caller's turn (`0x509dd0`): lines queued outside the world until the UI's
//! entry (`0x509f48`, `0x50dfc3`), lines held on a missing name until its answer's callback
//! (`0x509f92` -> `0x50ad82`, `0x503c80`), and the chat log's own composition (`0x50ab09`).

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::event::{ChatEvent, ChatEventKind};
use super::feed::{add, enter_world, ChatLine};

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
    super::feed::land_cached(&mut world);
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
    super::feed::land_cached(&mut world);
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
    super::feed::land_cached(&mut world);
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
            gm: false,
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
    super::feed::land_cached(&mut world);
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
            wire_type: wotlk_type(chat_type),
            sender_word: 0,
            gm: false,
        },
    }
}

/// The 3.3.5a type the decoder reads for a shared `chat_type`.
fn wotlk_type(chat_type: u8) -> u8 {
    (0..=0x34)
        .find(|&w| benilla_protocol::messages::wotlk_chat_type_shared(w) == Some(chat_type))
        .expect("a 3.3.5a lane")
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
    super::feed::land_cached(&mut world);
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
    super::feed::land_cached(&mut world);
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
    super::feed::land_cached(&mut world);
    assert_eq!(
        world.resource::<super::ChatLog>().pending_count(),
        2,
        "another guid's answer leaves them waiting"
    );
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, String::new(), None);
    super::feed::land_cached(&mut world);
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
    super::feed::land_cached(&mut world);
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

/// A line the client composes takes the add's text passes too, as every caller hands it over with
/// `[ebp+0x40]` 0 (`0x50d8c9`): a system line's quest link takes the level-10 player's difficulty
/// colour (`0x50a2ec`-`0x50a393`) and its profanity is masked, the system type being in the mask
/// table (`0x50a621`); a loot line, not type 0, goes through the spam filter
/// (`0x50a6a7`), and a system line never does.
#[test]
fn a_composed_line_takes_the_quest_colour_the_spam_filter_and_the_mask() {
    use benilla_protocol::field::{FIELD_OBJECT_TYPE, FIELD_UNIT_LEVEL};
    let (mut world, sent) = world();
    hear(&world, &["CHAT_MSG_SYSTEM", "CHAT_MSG_LOOT"]);
    let me = world
        .spawn(crate::net::ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[
                (FIELD_OBJECT_TYPE, 0x19),
                (FIELD_UNIT_LEVEL, 10),
            ])
            .into_created(benilla_protocol::messages::ObjectType::Player),
        ))
        .id();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ANN);
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ANN, me);
    world.insert_resource(crate::text_filter::TextFilter::new(
        &[benilla_formats::FilterPattern {
            id: 1,
            pattern: "darn".into(),
        }],
        &[],
    ));
    world
        .resource_mut::<crate::text_filter::TextFilter>()
        .set_server_spam(&["gold".to_string()]);
    enter_world(&mut world);
    add(
        &mut world,
        system_line("|cffffff00|Hquest:1:80|h[X]|h|r darn"),
    );
    add(
        &mut world,
        ChatEvent::text_only(ChatEventKind::Loot, "cheap gold".into()).into(),
    );
    add(&mut world, system_line("cheap gold"));
    add(
        &mut world,
        ChatEvent::text_only(ChatEventKind::Loot, "a darn ring".into()).into(),
    );
    assert_eq!(
        heard(&world),
        [
            "CHAT_MSG_SYSTEM:|cffff2020|Hquest:1:80|h[X]|h|r !@#$::",
            "CHAT_MSG_SYSTEM:cheap gold::",
            "CHAT_MSG_LOOT:a darn ring::",
        ],
        "the loot line's spam is dropped, and a loot line is not masked"
    );
    assert!(sent.try_iter().next().is_none(), "no report off a whisper");
}

/// A composed line's kind hands the add its 3.3.5a type: system 0 (`0x50d8f3`), loot `0x1b`
/// (`0x523b52`), a whisper 7, never the foreign 8 (`0x50bfc7`), arena points `0x32` (`0x6d0ff4`).
#[test]
fn a_composed_kind_hands_the_add_its_3_3_5a_type() {
    use super::event::wotlk_type_of_kind as of;
    use benilla_protocol::messages::wotlk_chat_type as w;
    assert_eq!(of(ChatEventKind::System), Some(w::SYSTEM));
    assert_eq!(of(ChatEventKind::Loot), Some(w::LOOT));
    assert_eq!(of(ChatEventKind::Whisper), Some(w::WHISPER));
    assert_eq!(of(ChatEventKind::ArenaPoints), Some(w::ARENA_POINTS));
    assert_eq!(of(ChatEventKind::CombatXpGain), Some(0x21));
}

/// Each add call enters a whisper's sender in autocomplete once, by guid, with the name the add
/// read (`0x509ec7`-`0x509ee0`, `0x57b7c0`): a cached sender's full name (`0x509e6a`), so a realm's
/// player is offered as `Name-Realm` alone, in the world and when queued outside it; a held
/// sender enters with no name and is named by the callback's bare name (`0x503e98`).
#[test]
fn a_whisper_enters_autocomplete_once_with_the_name_the_add_read() {
    use benilla_protocol::messages::CHAT_MSG_WHISPER;
    const BOB: u64 = 0x22;
    const CY: u64 = 0x33;
    let ask = "return GetAutoCompleteResults('', 0x10, 0, 9, nil, true)";
    let (mut world, _sent) = world();
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(ANN, "Ann".into(), None);
        names.set_player_realm(ANN, "Icecrown".into());
        names.insert_player(CY, "Cy".into(), None);
        names.set_player_realm(CY, "Lordaeron".into());
    }
    // Outside the world the whisper queues; the entry's walk adds it again.
    add(&mut world, player_line(CHAT_MSG_WHISPER, CY, 0, "early"));
    enter_world(&mut world);
    add(&mut world, player_line(CHAT_MSG_WHISPER, ANN, 0, "hi"));
    add(&mut world, player_line(CHAT_MSG_WHISPER, BOB, 0, "yo"));
    let offered = |world: &World| {
        let mut v = world
            .non_send_resource::<UiScript>()
            .eval::<mlua::MultiValue>(ask)
            .unwrap()
            .into_iter()
            .map(|v| v.to_string().unwrap())
            .collect::<Vec<_>>();
        v.sort();
        v
    };
    assert_eq!(offered(&world), ["Ann-Icecrown", "Cy-Lordaeron"]);
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(BOB, "Bob".into(), None);
        names.set_player_realm(BOB, "Icecrown".into());
    }
    super::feed::land_cached(&mut world);
    assert_eq!(offered(&world), ["Ann-Icecrown", "Bob", "Cy-Lordaeron"]);
}

/// The world-entry walk reads a player sender's record again and hands the add its bare name
/// (`0x50aeab`-`0x50aeff`, `0x50af87` -> `0x50afdc`), whether the line was queued with the
/// cached full name or marked queued by its answer's callback; only a GM line keeps the name it
/// came with (`0x50af8e`).
#[test]
fn the_world_entry_walk_passes_the_senders_bare_name() {
    use benilla_protocol::messages::{CHAT_MSG_SAY, CHAT_MSG_WHISPER};
    const BOB: u64 = 0x22;
    const CY: u64 = 0x33;
    let (mut world, sent) = world();
    hear(&world, &["CHAT_MSG_WHISPER", "CHAT_MSG_SAY"]);
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(ANN, "Ann".into(), None);
        names.set_player_realm(ANN, "Icecrown".into());
        names.insert_player(CY, "Cy".into(), None);
        names.set_player_realm(CY, "Lordaeron".into());
    }
    add(&mut world, player_line(CHAT_MSG_WHISPER, CY, 0, "early"));
    add(&mut world, player_line(CHAT_MSG_SAY, BOB, 0, "late"));
    let mut gm = player_line(CHAT_MSG_SAY, ANN, 0, "gm");
    if let ChatLine::Wire { msg } = &mut gm {
        msg.sender_name = Some("Ann-Icecrown".into());
        msg.gm = true;
    }
    add(&mut world, gm);
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(BOB, "Bob".into(), None);
        names.set_player_realm(BOB, "Icecrown".into());
    }
    // Outside the world the answer marks Bob's line queued (`0x503e78`).
    super::feed::land_cached(&mut world);
    assert!(heard(&world).is_empty());
    while sent.try_recv().is_ok() {}
    enter_world(&mut world);
    assert_eq!(
        heard(&world),
        [
            "CHAT_MSG_WHISPER:early:Cy:",
            "CHAT_MSG_SAY:late:Bob:",
            "CHAT_MSG_SAY:gm:Ann-Icecrown:"
        ]
    );
    assert!(sent.try_recv().is_err(), "every record was cached");
}

/// A composed line of a recorded type is written to the chat record under its line id like a
/// wire line (`0x50a8a5` -> `0x4fdd80`, kept by `0x4fb1a0`): a defense line on its channel (0x11)
/// is found by `CanComplainChat(lineID)`; a system line (type 0) is not recorded.
#[test]
fn a_composed_channel_line_is_recorded_under_its_line_id() {
    let (mut world, _sent) = world();
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"
            Ids = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_CHANNEL")
            f:RegisterEvent("CHAT_MSG_SYSTEM")
            f:SetScript("OnEvent", function(_, e, ...) Ids[#Ids + 1] = select(11, ...) end)
        "#,
        )
        .unwrap();
    {
        let mut s = world.non_send_resource_mut::<UiScript>();
        s.set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                guid: ME,
                ..Default::default()
            }),
        );
        s.set_complaint_status(1);
    }
    enter_world(&mut world);
    add(
        &mut world,
        ChatEvent {
            kind: Some(ChatEventKind::Channel),
            text: "Westfall is under attack!".into(),
            channel: "LocalDefense - Westfall".into(),
            ..Default::default()
        }
        .into(),
    );
    add(&mut world, system_line("hello"));
    let ids: Vec<u32> = world
        .non_send_resource::<UiScript>()
        .eval("return Ids")
        .unwrap();
    assert_eq!(ids.len(), 2);
    let can = |id: u32| {
        world
            .non_send_resource::<UiScript>()
            .eval::<Option<i32>>(&format!("return CanComplainChat({id})"))
            .unwrap()
    };
    assert_eq!(can(ids[0]), Some(1), "the defense line is recorded");
    assert_eq!(can(ids[1]), None, "a system line is not");
}

const ME: u64 = 0x22;

/// A world past its entry with our player in view (`0x5036c3`), WAVE's sentences, the language
/// record 0 and Ann named; a spy keeps a1, a2, a3, a11 and a12 of each line it hears.
fn emote_world() -> World {
    use benilla_formats::EmoteTextCatalog;
    let (mut world, _sent) = world();
    let s = world.non_send_resource::<UiScript>();
    s.run(
        r#"
        Lines = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("CHAT_MSG_TEXT_EMOTE")
        f:RegisterEvent("CHAT_MSG_SAY")
        f:SetScript("OnEvent", function(_, e, a1, a2, a3, _, _, _, _, _, _, _, a11, a12)
            Lines[#Lines + 1] = table.concat({ e, a1, a2, a3, tostring(a11), a12 }, "|")
        end)
    "#,
    )
    .unwrap();
    world.insert_resource(super::feed::EmoteTexts(EmoteTextCatalog::from_rows(
        101,
        &[
            (0, "%s waves at %s."),
            (1, "%s waves at you."),
            (4, "%s waves."),
        ],
    )));
    world.insert_resource(crate::ui_unit::LanguagesRes(
        benilla_formats::Languages::from_rows(&[(0, "Universal"), (7, "Common")]),
    ));
    let me = world.spawn(crate::net::ObjectStore::default()).id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    world
}

fn lines(world: &World) -> Vec<String> {
    world
        .non_send_resource::<UiScript>()
        .eval::<Vec<String>>("return Lines")
        .unwrap()
}

fn wave(target: &str) -> ChatLine {
    ChatLine::TextEmote {
        performer: ANN,
        text_id: 101,
        target: target.into(),
    }
}

/// A text emote is its own line (`0x503660`), never the add's: it fires with arg11 0
/// (`0x5038f3`), arg3 the language record 0's name (`0x503876`-`0x5038a6`) and arg12 the
/// performer's guid (`0x5038e2`), and takes no line id, which only the add counts (`0x50a884`), so
/// the next say is line 1.
#[test]
fn a_text_emote_fires_outside_the_add_with_no_line_id() {
    use benilla_protocol::messages::CHAT_MSG_SAY;
    let mut world = emote_world();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ME, "Me".into(), None);
    enter_world(&mut world);
    add(&mut world, wave("Me"));
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 0, "hi"));
    assert_eq!(
        lines(&world),
        [
            "CHAT_MSG_TEXT_EMOTE|Ann waves at you.|Ann|Universal|0|0x0000000000000011",
            "CHAT_MSG_SAY|hi|Ann|Universal|1|0x0000000000000011",
        ]
    );
    assert_eq!(world.resource::<super::ChatWindows>().line_id, 1);
}

/// The "at you" test reads our unit's name at the call (`0x503753`) and never waits on it: with our
/// name not cached the emote still shows, at someone else.
#[test]
fn a_text_emote_never_waits_on_our_own_name() {
    let mut world = emote_world();
    enter_world(&mut world);
    add(&mut world, wave("Me"));
    assert_eq!(
        lines(&world),
        ["CHAT_MSG_TEXT_EMOTE|Ann waves at Me.|Ann|Universal|0|0x0000000000000011"]
    );
}

/// With no active player the emote shows nothing (`0x5036cb`).
#[test]
fn a_text_emote_with_no_active_player_shows_nothing() {
    let mut world = emote_world();
    world.resource_mut::<crate::net::GuidIndex>().0.clear();
    enter_world(&mut world);
    add(&mut world, wave(""));
    assert!(lines(&world).is_empty());
}

/// The log takes the sentence raw (`0x503945`), not the add's `CHAT_<TYPE>_GET` composition.
#[test]
fn a_text_emote_is_logged_raw() {
    let mut world = emote_world();
    let dir = std::env::temp_dir().join(format!("benilla-emote-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chat.txt");
    let _ = std::fs::remove_file(&path);
    let s = world.non_send_resource::<UiScript>();
    s.run(r#"CHAT_TEXT_EMOTE_GET = "%s: ""#).unwrap();
    world
        .non_send_resource_mut::<UiScript>()
        .set_logging_flags(true, false);
    world.resource_mut::<super::ChatWindows>().logs =
        super::logging::ChatLogFile(super::logging::LogFile::at(&path));
    enter_world(&mut world);
    add(&mut world, wave(""));
    let log = std::fs::read_to_string(&path).unwrap();
    let logged: Vec<&str> = log
        .lines()
        .map(|l| l.split_once("  ").map_or(l, |(_, rest)| rest))
        .collect();
    assert_eq!(logged, ["Ann waves."]);
}

/// Outside the world a named text emote is queued marked (`0x50368c` -> `0x502ea0`) on a list
/// nothing walks again (`0x503fea`), so the entry shows nothing; one waiting on its performer's
/// name is held, and its name landing while still outside marks it the same way (`0x504026`).
#[test]
fn a_text_emote_outside_the_world_never_shows() {
    let mut world = emote_world();
    add(&mut world, wave(""));
    assert_eq!(world.resource::<super::ChatLog>().pending_count(), 0);
    const BOB: u64 = 0x33;
    add(
        &mut world,
        ChatLine::TextEmote {
            performer: BOB,
            text_id: 101,
            target: String::new(),
        },
    );
    assert_eq!(world.resource::<super::ChatLog>().pending_count(), 1);
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(BOB, "Bob".into(), None);
    super::feed::land_cached(&mut world);
    assert_eq!(world.resource::<super::ChatLog>().pending_count(), 0);
    enter_world(&mut world);
    assert!(lines(&world).is_empty());
    // Past the entry the same emote shows at once.
    add(&mut world, wave(""));
    assert_eq!(lines(&world).len(), 1);
}

/// The add reads the active player's GM bit at its call (`0x50a3bd`-`0x50a3cc`): a `PLAYER_FLAGS`
/// write earlier in the same frame turns the next line to language 0, with no feed between.
#[test]
fn the_add_reads_the_gm_bit_at_its_call() {
    use benilla_protocol::messages::CHAT_MSG_SAY;
    let mut world = emote_world();
    enter_world(&mut world);
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 7, "hi"));
    world.spawn((
        crate::net::SelfPlayer,
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_PLAYER_FLAGS,
            0x8,
        )])),
    ));
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 7, "hi"));
    assert_eq!(
        lines(&world),
        [
            "CHAT_MSG_SAY|hi|Ann|Common|1|0x0000000000000011",
            "CHAT_MSG_SAY|hi|Ann|Universal|2|0x0000000000000011",
        ]
    );
}

/// A world for the combat lines: their sentences, a spy on their events, and the record waits.
fn combat_world() -> (
    World,
    crossbeam_channel::Receiver<crate::net::ClientCommand>,
) {
    let (mut world, sent) = world();
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"
            TRADESKILL_LOG_FIRSTPERSON = "You create %s."
            TRADESKILL_LOG_THIRDPERSON = "%s creates %s."
            FEEDPET_LOG_FIRSTPERSON = "Your pet eats %s."
            FEEDPET_LOG_THIRDPERSON = "%s's pet eats %s."
            OPEN_LOCK_SELF = "You perform %s on %s."
            OPEN_LOCK_OTHER = "%s performs %s on %s."
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_TRADESKILLS")
            f:RegisterEvent("CHAT_MSG_PET_INFO")
            f:RegisterEvent("CHAT_MSG_OPENING")
            f:SetScript("OnEvent", function(_, e, a1) Seen[#Seen + 1] = e .. ":" .. a1 end)
        "#,
        )
        .unwrap();
    world.insert_non_send_resource(crate::items::RecordWaits::default());
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ANN);
    enter_world(&mut world);
    (world, sent)
}

/// The combat lines one builder queued, each through the add.
fn add_combat(world: &mut World, build: impl FnOnce(&mut crate::ui_script::live::UiEvents)) {
    let mut ui = crate::ui_script::live::UiEvents::default();
    build(&mut ui);
    for line in ui.take_chat() {
        add(world, line);
    }
}

fn item_queries(sent: &crossbeam_channel::Receiver<crate::net::ClientCommand>) -> Vec<u32> {
    sent.try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::ItemQuery { entry, .. } => Some(entry),
            _ => None,
        })
        .collect()
}

/// An uncached item's tradeskill or feed-pet line shows nothing: its composer asks the record
/// with its callback (`0x50bdb1` -> `0x4fb970`, `0x50bb11` -> `0x4fb950`), which composes the line
/// again on the answer; a cached one shows at once.
#[test]
fn an_uncached_items_combat_line_waits_on_its_record_callback() {
    use crate::items::RecordWait;
    let (mut world, sent) = combat_world();
    add_combat(&mut world, |ui| {
        crate::combat_log::chat::tradeskill_create(ui, Some(ANN), ANN, 7_001);
        crate::combat_log::chat::feed_pet(ui, Some(ANN), ANN, 7_002);
    });
    assert!(seen(&world).is_empty());
    assert_eq!(item_queries(&sent), [7_001, 7_002]);
    assert_eq!(
        world
            .non_send_resource::<crate::items::RecordWaits>()
            .waiting(),
        [
            (7_001, RecordWait::TradeskillLine(ANN)),
            (7_002, RecordWait::FeedPetLine(ANN))
        ]
    );
    assert_eq!(
        world.resource::<super::ChatLog>().pending_count(),
        0,
        "no line is held on the chat log"
    );
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(7_001, Some(crate::items::test_template("Elixir")));
    add_combat(&mut world, |ui| {
        crate::combat_log::chat::tradeskill_create(ui, Some(ANN), ANN, 7_001);
    });
    assert_eq!(seen(&world), ["CHAT_MSG_TRADESKILLS:You create Elixir."]);
}

/// The open-lock line asks its item with no callback, which sends no query (`0x50ec7f`,
/// `0x67ca5d`), and shows nothing without the record (`0x50ddeb`).
#[test]
fn an_open_lock_line_on_an_uncached_item_shows_nothing_and_asks_nothing() {
    use crate::combat_log::chat::{open_lock, Opened};
    let (mut world, sent) = combat_world();
    add_combat(&mut world, |ui| {
        open_lock(ui, Some(ANN), ANN, "Opening".into(), Opened::Item(7_003));
    });
    assert!(seen(&world).is_empty());
    assert!(item_queries(&sent).is_empty());
    assert!(world
        .non_send_resource::<crate::items::RecordWaits>()
        .waiting()
        .is_empty());
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(7_003, Some(crate::items::test_template("Lockbox")));
    add_combat(&mut world, |ui| {
        open_lock(ui, Some(ANN), ANN, "Opening".into(), Opened::Item(7_003));
    });
    assert_eq!(
        seen(&world),
        ["CHAT_MSG_OPENING:You perform Opening on Lockbox."]
    );
}

/// The feed-pet line names another caster off his name record, read with no callback, and shows
/// nothing without it (`0x50bb80`-`0x50bb87`).
#[test]
fn a_feed_pet_line_needs_its_casters_name_record() {
    const BOB: u64 = 0x33;
    let (mut world, sent) = combat_world();
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(7_004, Some(crate::items::test_template("Bread")));
    add_combat(&mut world, |ui| {
        crate::combat_log::chat::feed_pet(ui, Some(ANN), BOB, 7_004);
    });
    assert!(seen(&world).is_empty());
    assert!(
        !sent
            .try_iter()
            .any(|c| matches!(c, crate::net::ClientCommand::NameQuery { .. })),
        "the record is read with no ask"
    );
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(BOB, "Bob".into(), None);
    add_combat(&mut world, |ui| {
        crate::combat_log::chat::feed_pet(ui, Some(ANN), BOB, 7_004);
    });
    assert_eq!(seen(&world), ["CHAT_MSG_PET_INFO:Bob's pet eats Bread."]);
}

/// The name record runs its callbacks in registration order (`0x680d03`-`0x680d2c`), each over its
/// own list: an unnamed sender's say, text emote, roll and say register the chat callback
/// (`0x509f33`), the emotes' (`0x5040ef`) and the roll's (`0x6e3827`) on one record, so the answer
/// adds both says first (`0x503edb`), then shows the emote (`0x50403f`), then composes the roll
/// (`0x6cb990`).
#[test]
fn a_name_answer_runs_its_callbacks_in_registration_order() {
    use benilla_protocol::messages::CHAT_MSG_SAY;
    const BOB: u64 = 0x33;
    let mut world = emote_world();
    let s = world.non_send_resource::<UiScript>();
    s.run(
        r#"
        RANDOM_ROLL_RESULT = "%s rolls %d (%d-%d)"
        local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_SYSTEM")
        g:SetScript("OnEvent", function(_, e, a1) Lines[#Lines + 1] = e .. "|" .. a1 end)
    "#,
    )
    .unwrap();
    enter_world(&mut world);
    add(&mut world, player_line(CHAT_MSG_SAY, BOB, 0, "one"));
    add(
        &mut world,
        ChatLine::TextEmote {
            performer: BOB,
            text_id: 101,
            target: String::new(),
        },
    );
    add(
        &mut world,
        ChatLine::Roll {
            min: 1,
            max: 100,
            roll: 42,
            guid: BOB,
        },
    );
    add(&mut world, player_line(CHAT_MSG_SAY, BOB, 0, "two"));
    assert!(lines(&world).is_empty(), "all four wait on Bob's name");
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(BOB, "Bob".into(), None);
    super::feed::name_landed(&mut world, crate::names::NameKey::Player(BOB));
    let order: Vec<String> = lines(&world)
        .iter()
        .map(|l| l.split('|').take(2).collect::<Vec<_>>().join("|"))
        .collect();
    assert_eq!(
        order,
        [
            "CHAT_MSG_SAY|one",
            "CHAT_MSG_SAY|two",
            "CHAT_MSG_TEXT_EMOTE|Bob waves.",
            "CHAT_MSG_SYSTEM|Bob rolls 42 (1-100)",
        ]
    );
}

/// A roll is composed by its handler or its callback before the add (`0x6e3848`, `0x6cb990`), so
/// outside the world an unnamed roller's roll is queued as text at its answer: a cached sender's
/// say queued after the roll arrived still comes first at the entry (`0x50dfc3`).
#[test]
fn an_unnamed_roll_queues_at_its_answer_outside_the_world() {
    use benilla_protocol::messages::CHAT_MSG_SAY;
    const BOB: u64 = 0x33;
    let (mut world, _sent) = world();
    let s = world.non_send_resource::<UiScript>();
    s.run(
        r#"
        RANDOM_ROLL_RESULT = "%s rolls %d (%d-%d)"
        local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_SAY")
        g:SetScript("OnEvent", function(_, e, a1) Seen[#Seen + 1] = e .. ":" .. a1 end)
    "#,
    )
    .unwrap();
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    add(
        &mut world,
        ChatLine::Roll {
            min: 1,
            max: 100,
            roll: 7,
            guid: BOB,
        },
    );
    add(&mut world, player_line(CHAT_MSG_SAY, ANN, 0, "hi"));
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(BOB, "Bob".into(), None);
    super::feed::name_landed(&mut world, crate::names::NameKey::Player(BOB));
    assert!(
        seen(&world).is_empty(),
        "nothing is added outside the world"
    );
    enter_world(&mut world);
    assert_eq!(
        seen(&world),
        ["CHAT_MSG_SAY:hi", "CHAT_MSG_SYSTEM:Bob rolls 7 (1-100):"]
    );
}

/// The honor kill line (`0x50b6d0`): `rank < 0` takes `COMBATLOG_HONORGAIN_NO_RANK`, `rank >= 0`
/// takes `COMBATLOG_HONORGAIN` with `PVP_RANK_<rank>_<side>` from the victim's faction template
/// (`+0xc`, bits 4 and 2, `0x50b755`-`0x50b784`) gendered by the local player (`0x50b7ab`, `""` on
/// a missing global at `0x819d40`), a missing victim prints nothing (`0x50b709`), and victim 0
/// takes `COMBATLOG_HONORAWARD`.
#[test]
fn honor_gain_forks_on_signed_rank_and_resolves_title_from_victim_faction_template() {
    use benilla_formats::{FactionCatalog, FactionTemplate};
    use benilla_protocol::field::{FIELD_OBJECT_TYPE, FIELD_UNIT_FACTIONTEMPLATE};
    use std::collections::HashMap;

    const HORDE_VICTIM: u64 = 0x22;
    const ALLIANCE_VICTIM: u64 = 0x33;
    const MISSING_VICTIM: u64 = 0x44;

    let (mut world, _sent) = world();
    hear(&world, &["CHAT_MSG_COMBAT_HONOR_GAIN"]);
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"
            COMBATLOG_HONORAWARD = "You have been awarded %d honor points."
            COMBATLOG_HONORGAIN = "%s dies, honorable kill Rank: %s (%d Honor Points)"
            COMBATLOG_HONORGAIN_NO_RANK = "%s dies, honorable kill (%d Honor Points)"
            PVP_RANK_9_0 = "Senior Sergeant"
            PVP_RANK_9_1 = "Sergeant Major"
            PVP_RANK_9_1_FEMALE = "Sergeant Major (f)"
            "#,
        )
        .unwrap();

    let template = |group_mask| FactionTemplate {
        faction: 1,
        group_mask,
        friend_group_mask: 0,
        enemy_group_mask: 0,
        enemies: [0; 4],
        friends: [0; 4],
    };
    world.insert_resource(crate::target::ring::Factions::from_catalog(
        FactionCatalog::from_rows(
            HashMap::from([(1, template(0x4)), (2, template(0x2))]),
            HashMap::new(),
        ),
    ));

    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ANN);
    {
        let mut names = world.resource_mut::<crate::names::NameCache>();
        // Local player is female (gender 1); titles gender against the local player (`0x50b7ab`).
        names.insert_player(ANN, "Ann".into(), Some((1, 1, 1)));
        names.insert_player(HORDE_VICTIM, "Grimtusk".into(), Some((2, 1, 0)));
        names.insert_player(ALLIANCE_VICTIM, "Aldric".into(), Some((1, 1, 0)));
    }
    let spawn_victim = |world: &mut World, guid: u64, faction_template: u32| {
        let entity = world
            .spawn(crate::net::ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_TYPE, 0x19),
                    (FIELD_UNIT_FACTIONTEMPLATE, faction_template),
                ])
                .into_created(benilla_protocol::messages::ObjectType::Player),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, entity);
    };
    spawn_victim(&mut world, HORDE_VICTIM, 1);
    spawn_victim(&mut world, ALLIANCE_VICTIM, 2);
    enter_world(&mut world);

    // Negative rank -> COMBATLOG_HONORGAIN_NO_RANK.
    add(
        &mut world,
        ChatLine::HonorGain {
            victim: HORDE_VICTIM,
            honor: 12,
            rank: -1,
        },
    );
    // Non-negative rank -> COMBATLOG_HONORGAIN with Horde side (0) and Alliance female side (1).
    add(
        &mut world,
        ChatLine::HonorGain {
            victim: HORDE_VICTIM,
            honor: 137,
            rank: 9,
        },
    );
    add(
        &mut world,
        ChatLine::HonorGain {
            victim: ALLIANCE_VICTIM,
            honor: 88,
            rank: 9,
        },
    );
    // Non-negative rank with missing PVP_RANK_0_* still takes COMBATLOG_HONORGAIN with "".
    add(
        &mut world,
        ChatLine::HonorGain {
            victim: HORDE_VICTIM,
            honor: 5,
            rank: 0,
        },
    );
    // Victim not in view -> nothing (`0x50b709`).
    add(
        &mut world,
        ChatLine::HonorGain {
            victim: MISSING_VICTIM,
            honor: 50,
            rank: 9,
        },
    );
    // Zero victim -> COMBATLOG_HONORAWARD.
    add(
        &mut world,
        ChatLine::HonorGain {
            victim: 0,
            honor: 42,
            rank: 0,
        },
    );

    assert_eq!(
        heard(&world),
        [
            "CHAT_MSG_COMBAT_HONOR_GAIN:Grimtusk dies, honorable kill (12 Honor Points)::",
            "CHAT_MSG_COMBAT_HONOR_GAIN:Grimtusk dies, honorable kill Rank: Senior Sergeant (137 Honor Points)::",
            "CHAT_MSG_COMBAT_HONOR_GAIN:Aldric dies, honorable kill Rank: Sergeant Major (f) (88 Honor Points)::",
            "CHAT_MSG_COMBAT_HONOR_GAIN:Grimtusk dies, honorable kill Rank:  (5 Honor Points)::",
            "CHAT_MSG_COMBAT_HONOR_GAIN:You have been awarded 42 honor points.::",
        ]
    );
}
