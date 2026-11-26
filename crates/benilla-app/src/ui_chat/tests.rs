use super::event::{default_color, ChatEvent, ChatEventKind as K};
use super::input::{emote_gate, emote_send_eligible, emote_target, EmoteGate, ParsedChat};
use super::ChatSink as _;

thread_local! {
    /// The shipped `GlobalStrings.lua`, run in a VM once per test thread; built lazily, so a
    /// caller's `wow_data_or_skip!()` runs first.
    static GLOBAL_STRINGS: benilla_ui::script::UiScript = {
        let s = benilla_ui::script::UiScript::new().expect("VM");
        crate::ui_script::test_ui::load_ui(&s, "Interface\\FrameXML\\GlobalStrings.lua");
        s
    };
}

/// Run `f` with a key lookup into the shipped string table.
fn with_strings<T>(f: impl FnOnce(&dyn Fn(&str) -> Option<String>) -> T) -> T {
    GLOBAL_STRINGS.with(|s| f(&|key: &str| s.lua().globals().get::<String>(key).ok()))
}

/// The line id [`compose`] composes for, arg11 in the player links it builds.
pub(super) const LINE: u32 = 7;

/// [`super::frames::compose`] for line [`LINE`] against the shipped table, for this file and
/// [`super::broadcast`].
pub(super) fn compose(event: &ChatEvent, kind: K, default_language: &str) -> Option<String> {
    GLOBAL_STRINGS.with(|s| {
        super::frames::compose(event, kind, default_language, LINE, &|key| {
            s.lua().globals().get::<String>(key).ok()
        })
    })
}

/// A player-line event, as the wire bridge builds it.
fn ev(kind: K, text: &str, sender: &str) -> ChatEvent {
    ChatEvent {
        kind: Some(kind),
        text: text.into(),
        sender: sender.into(),
        ..Default::default()
    }
}

#[test]
fn player_lines_link_the_name_except_emote() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The `|Hplayer` link of `ChatFrame.lua:2896`: name, line id, chat group.
    assert_eq!(
        compose(&ev(K::Say, "hi there", "Bob"), K::Say, "Common").unwrap(),
        "|Hplayer:Bob:7:SAY|h[Bob]|h says: hi there"
    );
    assert_eq!(
        compose(
            &ev(K::WhisperInform, "hey", "Bob"),
            K::WhisperInform,
            "Common"
        )
        .unwrap(),
        "To |Hplayer:Bob:7:WHISPER:BOB|h[Bob]|h: hey",
        "a whisper's link carries the upper-cased target (`:2731-2733`)"
    );
    // EMOTE links the name without brackets (`ChatFrame.lua:2912-2913`).
    assert_eq!(
        compose(&ev(K::Emote, "dances.", "Bob"), K::Emote, "Common").unwrap(),
        "|Hplayer:Bob:7:EMOTE|hBob|h dances."
    );
}

#[test]
fn group_prefixed_kinds_wear_their_brackets() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        compose(&ev(K::Party, "inc 3", "Ann"), K::Party, "Common").unwrap(),
        "|Hchannel:PARTY|h[Party]|h |Hplayer:Ann:7:PARTY|h[Ann]|h: inc 3"
    );
    assert_eq!(
        compose(&ev(K::Guild, "gz", "Ann"), K::Guild, "Common").unwrap(),
        "|Hchannel:GUILD|h[Guild]|h |Hplayer:Ann:7:GUILD|h[Ann]|h: gz"
    );
    assert_eq!(
        compose(&ev(K::RaidWarning, "move", "Ann"), K::RaidWarning, "Common").unwrap(),
        "[Raid Warning] |Hplayer:Ann:7:RAID|h[Ann]|h: move"
    );
}

#[test]
fn flags_prefix_the_name_and_afk_uses_its_get() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ev(K::Say, "brb", "Bob");
    e.flag = "AFK".into();
    assert_eq!(
        compose(&e, K::Say, "Common").unwrap(),
        "<Away>|Hplayer:Bob:7:SAY|h[Bob]|h says: brb"
    );
    // A GM wears the Blizzard icon, not `CHAT_FLAG_GM` (`ChatFrame.lua:2853-2859`).
    e.flag = "GM".into();
    assert_eq!(
        compose(&e, K::Say, "Common").unwrap(),
        r"|TInterface\ChatFrame\UI-ChatIcon-Blizz.blp:0:2:0:-3|t |Hplayer:Bob:7:SAY|h[Bob]|h says: brb"
    );
    // A GM's whisper goes to the GM chat window, not this one (`:2854-2856`).
    let mut whisper = ev(K::Whisper, "hello", "Bob");
    whisper.flag = "GM".into();
    assert_eq!(compose(&whisper, K::Whisper, "Common"), None);
    assert_eq!(
        compose(&ev(K::Afk, "farming", "Bob"), K::Afk, "Common").unwrap(),
        "|Hplayer:Bob:7:WHISPER:BOB|h[Bob]|h is Away: farming"
    );
}

#[test]
fn language_header_rides_non_default_tongues() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ev(K::Say, "throm-ka", "Grunk");
    e.language = "Orcish".into();
    assert_eq!(
        compose(&e, K::Say, "Common").unwrap(),
        "|Hplayer:Grunk:7:SAY|h[Grunk]|h says: [Orcish] throm-ka"
    );
    // Neither the frame's default tongue nor Universal renders a header (`ChatFrame.lua:2901`).
    for tongue in ["Common", "Universal"] {
        e.language = tongue.into();
        assert_eq!(
            compose(&e, K::Say, "Common").unwrap(),
            "|Hplayer:Grunk:7:SAY|h[Grunk]|h says: throm-ka"
        );
    }
}

#[test]
fn system_and_loot_lines_are_verbatim() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        compose(
            &ChatEvent::text_only(K::System, "Additem: Wool Cloth added.".into()),
            K::System,
            "Common"
        )
        .unwrap(),
        "Additem: Wool Cloth added."
    );
    // A LOOT line arrives composed, item link and all; its escapes pass through untouched.
    assert_eq!(
        compose(
            &ChatEvent::text_only(
                K::Loot,
                "You receive loot: |cffffffff|Hitem:117:0:0:0|h[Tough Jerky]|h|r.".into()
            ),
            K::Loot,
            "Common"
        )
        .unwrap(),
        "You receive loot: |cffffffff|Hitem:117:0:0:0|h[Tough Jerky]|h|r."
    );
}

/// The performer rides in `sender` (arg2), and the handler links its first mention in the sentence
/// (`gsub(arg1, arg2, …, 1)`, `ChatFrame.lua:2914-2915`), bracketing nothing on.
#[test]
fn text_emote_lines_link_the_performers_first_mention() {
    let _data = benilla_formats::wow_data_or_skip!();
    let e = ev(K::TextEmote, "Bob waves at Bob's pet.", "Bob");
    assert_eq!(
        compose(&e, K::TextEmote, "Common").unwrap(),
        "|Hplayer:Bob:7:TEXT_EMOTE|hBob|h waves at Bob's pet."
    );
    // The control: a SAY does get the bracketed link.
    assert!(compose(&ev(K::Say, "hi", "Bob"), K::Say, "Common")
        .unwrap()
        .contains("[Bob]"));
}

/// `DoEmote` sends a self-target as guid 0 (`0x6ddaa0`), so there is no self-emote sentence.
#[test]
fn emoting_at_your_own_selection_sends_an_untargeted_emote() {
    use crate::target::Selection;
    use benilla_ui::script::EmoteTarget as T;

    const ME: u64 = 0x111;
    let sel = |guid| Selection {
        guid,
        ..Default::default()
    };
    assert_eq!(
        emote_target(T::Selection, &sel(Some(ME)), no_name, Some(ME)),
        0
    );
    assert_eq!(
        emote_target(T::Selection, &sel(Some(0xdead_beef)), no_name, Some(ME)),
        0xdead_beef
    );
    // No selection is untargeted, and with no self guid yet a selection goes out untouched.
    assert_eq!(emote_target(T::Selection, &sel(None), no_name, Some(ME)), 0);
    assert_eq!(
        emote_target(T::Selection, &sel(Some(0xdead_beef)), no_name, None),
        0xdead_beef
    );
}

fn no_name(_: &str) -> Option<u64> {
    None
}

/// A token's guid and a name replace the selection; a miss is guid 0; the selection arm searches
/// nothing.
#[test]
fn an_emote_argument_replaces_the_selection() {
    use crate::target::Selection;
    use benilla_ui::script::EmoteTarget as T;

    const ME: u64 = 0x111;
    let sel = Selection {
        guid: Some(0x5e1),
        ..Default::default()
    };
    let find = |n: &str| (n == "Bob").then_some(0xb0b);
    let name = |n: &str| T::Name(n.to_string());
    assert_eq!(emote_target(name("Bob"), &sel, find, Some(ME)), 0xb0b);
    assert_eq!(
        emote_target(name("Nobody"), &sel, find, Some(ME)),
        0,
        "a miss is untargeted"
    );
    assert_eq!(emote_target(T::Guid(0xf0c), &sel, no_name, Some(ME)), 0xf0c);
    assert_eq!(
        emote_target(T::Guid(0), &sel, no_name, Some(ME)),
        0,
        "a creature token through the type test"
    );
    let panics = |_: &str| -> Option<u64> { panic!("no search") };
    assert_eq!(emote_target(T::Selection, &sel, panics, Some(ME)), 0x5e1);
    // Naming yourself is the self-target edge: untargeted.
    let own = |_: &str| Some(ME);
    assert_eq!(emote_target(name("Me"), &sel, own, Some(ME)), 0);
    assert_eq!(emote_target(T::Guid(ME), &sel, no_name, Some(ME)), 0);
}

#[test]
fn a_received_text_emote_composes_its_sentence_and_names_the_performer() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_emote_text_catalog(&mut chain).expect("emote text catalog");
    const WAVE: u32 = 101;
    const SIT: u32 = 86;

    fn them(target: &'static str) -> benilla_formats::EmoteLine<'static> {
        benilla_formats::EmoteLine {
            performer: "Bob",
            performer_is_you: false,
            performer_female: false,
            target: if target == "-" { "" } else { target },
            your_name: "Me",
        }
    }
    fn mine(target: &'static str) -> benilla_formats::EmoteLine<'static> {
        benilla_formats::EmoteLine {
            performer: "Me",
            performer_is_you: true,
            ..them(target)
        }
    }
    let line = |text_id, l| super::feed::text_emote_event(&cat, text_id, &l);

    for (l, expected) in [
        (them("Jane"), "Bob waves at Jane."),
        (them("Me"), "Bob waves at you."),
        (them("-"), "Bob waves."),
    ] {
        let e = line(WAVE, l).expect("a sentence");
        assert_eq!(e.text, expected);
        assert_eq!(e.kind, Some(K::TextEmote));
        // arg2 is the performer, not the target.
        assert_eq!(e.sender, "Bob");
    }
    for (l, expected) in [
        (mine("Jane"), "You wave at Jane."),
        (mine("-"), "You wave."),
    ] {
        let e = line(WAVE, l).expect("a sentence");
        assert_eq!(e.text, expected);
        assert_eq!(e.sender, "Me");
    }
    // SIT's EmotesTextData rows ship blank: no line, not an empty one.
    assert!(line(SIT, them("-")).is_none(), "/sit prints nothing");
}

/// `COMBATLOG_HONORAWARD`, `COMBATLOG_HONORGAIN` and `COMBATLOG_HONORGAIN_NO_RANK`, the forks of
/// the honor line (`0x50b6d0`).
#[test]
fn honor_gain_lines_pick_the_reference_form() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(None, None, 42, g)).as_deref(),
        Some("You have been awarded 42 honor points.")
    );
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Grimtusk"), Some("Sergeant"), 137, g))
            .as_deref(),
        Some("Grimtusk dies, honorable kill Rank: Sergeant (137 Honor Points)")
    );
    // A kill under 1 honor prints nothing: 12340 has no dishonorable line (`0x50b71f`).
    for honor in [0, -37] {
        assert_eq!(
            with_strings(|g| super::feed::honor_gain_line(
                Some("Grimtusk"),
                Some("Sergeant"),
                honor,
                g
            )),
            None,
            "{honor} honor"
        );
    }
    // No rank title: the `_NO_RANK` form.
    assert_eq!(
        with_strings(|g| super::feed::honor_gain_line(Some("Grimtusk"), None, 5, g)).as_deref(),
        Some("Grimtusk dies, honorable kill (5 Honor Points)")
    );
}

#[test]
fn xp_gain_lines_pick_the_reference_form() {
    use super::XpLine;
    let _data = benilla_formats::wow_data_or_skip!();
    let line = |l: &XpLine, victim: Option<&str>| {
        with_strings(|g| super::feed::xp_gain_line(l, victim, g))
    };
    let plain = |victim, total, group_rate| XpLine::Plain {
        victim,
        total,
        group_rate,
    };
    assert_eq!(
        line(&plain(7, 35, 1.0), Some("Kobold Vermin")).as_deref(),
        Some("Kobold Vermin dies, you gain 35 experience.")
    );
    assert_eq!(
        line(&plain(0, 120, 1.0), None).as_deref(),
        Some("You gain 120 experience.")
    );
    // A group rate over 1 cut the shown award: trunc(120 / 1.5) = 80, a bonus of 40.
    assert_eq!(
        line(&plain(7, 120, 1.5), Some("Kobold Vermin")).as_deref(),
        Some("Kobold Vermin dies, you gain 120 experience. (+40 group bonus)")
    );
    assert_eq!(
        line(&plain(7, 100, 0.5), Some("Kobold Vermin")).as_deref(),
        Some("Kobold Vermin dies, you gain 100 experience. (-100 raid penalty)")
    );
    let rested = |group_rate| XpLine::Rested {
        victim: 7,
        key: "COMBATLOG_XPGAIN_EXHAUSTION1",
        state: "Rested".into(),
        total: 52,
        base: 35,
        group_rate,
    };
    assert_eq!(
        line(&rested(1.0), Some("Kobold Vermin")).as_deref(),
        Some("Kobold Vermin dies, you gain 52 experience. (+17 exp Rested bonus)")
    );
    assert_eq!(
        line(
            &XpLine::Loss {
                total: -300i32 as u32
            },
            None
        )
        .as_deref(),
        Some("You lose 300 experience.")
    );
    // The shipped lavender, chat-cache row 46.
    assert_eq!(default_color(K::CombatXpGain), [111, 111, 255]);
}

#[test]
fn exploration_lines_pick_the_reference_form() {
    let _data = benilla_formats::wow_data_or_skip!();
    // `ERR_ZONE_EXPLORED` (`GlobalStrings.lua:1925`) is the toast on every exploration packet;
    // `ERR_ZONE_EXPLORED_XP` (`:1926`) is the chat line, only when xp > 0.
    assert_eq!(
        with_strings(|g| super::feed::exploration_toast("Westfall", g)).as_deref(),
        Some("Discovered: Westfall")
    );
    assert_eq!(
        with_strings(|g| super::feed::exploration_line("Westfall", 85, g)).as_deref(),
        Some("Discovered Westfall: 85 experience gained")
    );
}

#[test]
fn monster_lines_use_the_bare_inline_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        compose(
            &ev(K::MonsterSay, "Intruders!", "Guard"),
            K::MonsterSay,
            "Common"
        )
        .unwrap(),
        "Guard says: Intruders!"
    );
    // MONSTER_EMOTE embeds %s where the name goes (CHAT_MONSTER_EMOTE_GET = "").
    assert_eq!(
        compose(
            &ev(K::MonsterEmote, "%s beckons you closer.", "Sentinel"),
            K::MonsterEmote,
            "Common"
        )
        .unwrap(),
        "Sentinel beckons you closer."
    );
}

#[test]
fn channel_line_prefixes_the_stripped_channel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ev(K::Channel, "wts boar livers", "Bob");
    e.channel = "1. General - Elwynn Forest".into();
    e.channel_number = 1;
    assert_eq!(
        compose(&e, K::Channel, "Common").unwrap(),
        "|Hchannel:channel:1|h[1. General]|h |Hplayer:Bob:7:CHANNEL:1|h[Bob]|h: wts boar livers"
    );
}

/// The notice arms print arg4 whole, zone tail and all: the strip at `ChatFrame.lua:1463` is the
/// speech arm's alone. The bridge builds each fixture, so each takes the arm its byte selects.
#[test]
fn channel_notices_compose_by_the_notice_law() {
    let _data = benilla_formats::wow_data_or_skip!();
    let notice = |byte: u8, channel: &str, a: Option<&str>, b: Option<&str>| {
        let e = super::feed::notice_event(
            byte,
            channel.to_string(),
            a.map(str::to_string),
            b.map(str::to_string),
        )
        .expect("the bridge builds an event for this notice");
        let kind = e.kind.expect("a built notice always carries its kind");
        compose(&e, kind, "Common")
    };
    assert_eq!(
        notice(0x02, "General - Elwynn Forest", None, None).unwrap(), // YOU_JOINED
        "Joined Channel: |Hchannel:CHANNEL:0|h[General - Elwynn Forest]|h",
        "the `_BN` twin wins (`ChatFrame.lua:2792-2795`), filled with arg8 then arg4"
    );
    assert_eq!(
        notice(0x12, "World", Some("Ann"), Some("Mod")).unwrap(), // PLAYER_KICKED
        "|Hchannel:CHANNEL:0|h[World]|h Player Ann kicked by Mod."
    );
    // A member join is a CHANNEL_JOIN event, linked like a player line.
    let mut join = ev(K::ChannelJoin, "", "Ann");
    join.channel = "World".into();
    assert_eq!(
        compose(&join, K::ChannelJoin, "Common").unwrap(),
        "|Hchannel:channel:0|h[World]|h |Hplayer:Ann:7:CHANNEL:0|h[Ann]|h joined channel."
    );
}

// ── the Lua face: the CHAT_MSG_* fire ───────────────────────────────────────────────────────────

/// A fresh VM with the interface the app loads, so `ChatFrame1` is the real window.
pub(super) fn chat_vm() -> benilla_ui::script::UiScript {
    crate::ui_script::test_ui::chat_ui()
}

/// An addon that records a `CHAT_MSG_*` fire: the count, the event and `arg1..arg10` joined with
/// `|`, which raises on a `nil` in any slot.
pub(super) const SPY: &str = r#"
    SpyN, SpyEvent, SpyLine = 0, "", ""
    Spy = CreateFrame("Frame", "BenillaChatSpy")
    Spy:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10)
        SpyN = SpyN + 1
        SpyEvent = event
        SpyLine = arg1.."|"..arg2.."|"..arg3.."|"..arg4.."|"..arg5.."|"..arg6..
                  "|"..arg7.."|"..arg8.."|"..arg9.."|"..arg10
    end)
"#;

/// How many lines `ChatFrame1` is holding (`GetNumMessages`).
pub(super) fn lines_in_window(s: &benilla_ui::script::UiScript) -> i64 {
    s.eval::<i64>("return ChatFrame1:GetNumMessages()").unwrap()
}

/// A routed line prints once, even after an addon registers `ChatFrame1` for its event again.
#[test]
fn an_addon_registering_our_own_chat_frame_does_not_double_print() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_SAY")"#)
        .unwrap();

    assert_eq!(lines_in_window(&s), 0, "the window starts empty");
    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi there", "Bob"));
    assert_eq!(lines_in_window(&s), 1, "our window prints exactly once");
    assert_eq!(
        s.eval::<i64>("return SpyN").unwrap(),
        1,
        "the addon saw the fire — otherwise the count above proves nothing"
    );

    // An addon registers ChatFrame1 for the event again: the double-print case, if there is one.
    s.run(r#"ChatFrame1:RegisterEvent("CHAT_MSG_SAY")"#)
        .unwrap();
    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi again", "Bob"));
    assert_eq!(
        lines_in_window(&s),
        2,
        "one more line, not two — registering an event again never fires it twice"
    );
    assert_eq!(s.eval::<i64>("return SpyN").unwrap(), 2);
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
}

/// Listeners run in registration order, as the reference's do, and `ChatFrame1` registered first.
#[test]
fn an_addons_handler_sees_the_line_already_in_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(
        r#"
        SeenAtFireTime = -1
        Spy = CreateFrame("Frame", "BenillaChatSpy")
        Spy:SetScript("OnEvent", function()
            SeenAtFireTime = ChatFrame1:GetNumMessages()
        end)
        BenillaChatSpy:RegisterEvent("CHAT_MSG_SAY")
    "#,
    )
    .unwrap();

    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi there", "Bob"));
    assert_eq!(
        s.eval::<i64>("return SeenAtFireTime").unwrap(),
        1,
        "the handler ran AFTER our window took the line, as registration order requires"
    );
}

/// arg7 and arg10 are numbers, zero for a non-channel line.
#[test]
fn a_say_line_fires_chat_msg_say_in_the_references_arg_positions() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_SAY")"#)
        .unwrap();

    let mut e = ev(K::Say, "throm-ka", "Grunk");
    e.language = "Orcish".into();
    e.flag = "GM".into();
    super::frames::route(&mut s, &mut windows, &e);

    assert_eq!(s.eval::<String>("return SpyEvent").unwrap(), "CHAT_MSG_SAY");
    // arg1 is the raw body: the reference's Lua adds "%s says: " and the link.
    assert_eq!(
        s.eval::<String>("return SpyLine").unwrap(),
        "throm-ka|Grunk|Orcish|||GM|0|0||0"
    );
}

/// A channel notice fires its token in arg1 and the numbered name in arg4; the handler repeats
/// `ChatFrame_OnEvent`'s bare `arg7 > 0` and `arg10 > 0`, which raise on a `nil`.
#[test]
fn a_channel_notice_fires_its_token_and_the_reference_reads_arg7_and_arg10_bare() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(
        r#"
        SpyZone, SpySuffix = nil, nil
        BenillaChatSpy:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10)
            SpyN = SpyN + 1
            SpyEvent = event
            SpyLine = arg1.."|"..arg2.."|"..arg3.."|"..arg4.."|"..arg5.."|"..arg6..
                      "|"..arg7.."|"..arg8.."|"..arg9.."|"..arg10
            -- ChatFrame_OnEvent l.1379 and l.1421, verbatim shape.
            if arg7 > 0 then SpyZone = arg7 end
            if arg10 > 0 then SpySuffix = arg10 end
        end)
        BenillaChatSpy:RegisterEvent("CHAT_MSG_CHANNEL_NOTICE")
    "#,
    )
    .unwrap();
    // The window prints only a channel it carries (`ChatFrame.lua:1374-1391`); `/join` adds it.
    s.run("ChatFrame_AddChannel(ChatFrame1, 'General - Elwynn Forest')")
        .unwrap();

    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.notice = "2".into(); // YOU_JOINED
    e.channel = "1. General - Elwynn Forest".into();
    e.channel_base = "General - Elwynn Forest".into();
    e.channel_number = 1;
    e.zone_channel_id = 1; // ChatChannels.dbc General
    super::frames::route(&mut s, &mut windows, &e);

    assert_eq!(s.eval::<i64>("return SpyN").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return SpyEvent").unwrap(),
        "CHAT_MSG_CHANNEL_NOTICE"
    );
    assert_eq!(
        s.eval::<String>("return SpyLine").unwrap(),
        "YOU_JOINED|||1. General - Elwynn Forest|||1|1|General - Elwynn Forest|0"
    );
    assert_eq!(s.eval::<i64>("return SpyZone").unwrap(), 1);
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
    assert_eq!(lines_in_window(&s), 1);
}

/// The notice switch's MODE_CHANGE arm prints its own lines (`0x50e559` -> `0x50cfc0`), so the
/// notice parks as a mode change, not as a notice event.
#[test]
fn a_mode_change_notice_parks_for_its_member_lines() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_protocol::messages::{channel_notice, ChannelNoticeTail};

    let mut log = crate::ui_script::live::UiEvents::default();
    log.push_channel_notice(
        channel_notice::MODE_CHANGE,
        "World".into(),
        &ChannelNoticeTail::ModeChange {
            guid: 42,
            old_flags: 0,
            new_flags: 1,
        },
    );
    let lines = log.take_chat();
    assert!(
        matches!(lines[..], [super::ChatLine::Mode { guid: 42, .. }]),
        "one mode change waits on the member's name"
    );

    // The control: a notice that fires is queued.
    log.push_channel_notice(
        channel_notice::YOU_JOINED,
        "World".into(),
        &ChannelNoticeTail::YouJoined {
            flags: 0,
            instance: 0,
            text: None,
        },
    );
    assert!(matches!(log.take_chat()[..], [super::ChatLine::Event(_)]));
}

/// arg4, arg7, arg8 and arg9 are one channel record in the reference, defaulted together.
#[test]
fn a_channel_we_are_not_in_fires_the_bare_name_and_zeroes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    s.run(SPY).unwrap();
    s.run(r#"BenillaChatSpy:RegisterEvent("CHAT_MSG_CHANNEL")"#)
        .unwrap();

    let channels = super::edit::ChannelState::default();
    let mut e = ev(K::Channel, "wts boar livers", "Bob");
    e.channel = "SomeoneElsesChannel".into();
    channels.stamp_channel(&mut e);
    super::frames::route(&mut s, &mut windows, &e);

    assert_eq!(
        s.eval::<String>("return SpyLine").unwrap(),
        "wts boar livers|Bob||SomeoneElsesChannel|||0|0||0",
        "arg4 keeps the bare INCOMING name (the miss leg still has one); arg7/8/9/10 are the \
         record we do not have, so 0/0/\"\"/0"
    );
}

#[test]
fn stamping_a_channel_splits_the_display_form_from_the_base_name() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut channels = super::edit::ChannelState::default();
    channels.claim_slot("World");
    channels.claim_slot("General - Elwynn Forest");

    let mut e = ev(K::Channel, "wts boar livers", "Bob");
    e.channel = "General - Elwynn Forest".into();
    channels.stamp_channel(&mut e);
    assert_eq!(e.channel, "2. General - Elwynn Forest"); // arg4
    assert_eq!(e.channel_number, 2); // arg8
    assert_eq!(e.channel_base, "General - Elwynn Forest"); // arg9, zone tail intact

    // A channel we are not in: its bare name in arg4, the rest empty.
    let mut other = ev(K::Channel, "hi", "Bob");
    other.channel = "SomeoneElsesChannel".into();
    channels.stamp_channel(&mut other);
    assert_eq!(other.channel, "SomeoneElsesChannel"); // arg4: the bare incoming name
    assert_eq!(other.channel_number, 0); // arg8
    assert_eq!(other.channel_base, ""); // arg9, not the name
    assert_eq!(other.zone_channel_id, 0); // arg7
}

/// The channel family's colour is `ChatTypeInfo["CHANNEL"..arg8]` (`ChatFrame.lua:1381`).
#[test]
fn a_channel_notice_renders_in_the_channels_color_not_the_notice_row() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::QuadContent;

    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState::default();
    channels.claim_slot("General - Elwynn Forest");

    // The window prints only a channel it carries (`ChatFrame.lua:1374-1391`).
    s.run("ChatFrame_AddChannel(ChatFrame1, 'General - Elwynn Forest')")
        .unwrap();
    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.channel = "General - Elwynn Forest".into();
    e.notice = "2".into(); // YOU_JOINED
    super::feed::deliver(&mut s, &mut windows, &mut channels, &mut e);
    s.resolve();

    let line = "Joined Channel: |Hchannel:CHANNEL:1|h[1. General - Elwynn Forest]|h";
    let quads = s.extract();
    let color = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text {
                text: Some(t),
                color: Some(c),
                ..
            } if t == line => Some(*c),
            _ => None,
        })
        .unwrap_or_else(|| {
            let drawn: Vec<&String> = quads
                .iter()
                .filter_map(|q| match &q.content {
                    QuadContent::Text { text: Some(t), .. } => Some(t),
                    _ => None,
                })
                .collect();
            panic!("the notice line {line:?} rendered; drawn: {drawn:?}")
        });
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(
        near(color[0], 1.0) && near(color[1], 192.0 / 255.0) && near(color[2], 192.0 / 255.0),
        "FFC0C0 (the CHANNEL1 row), not C0C0C0 (the CHANNEL_NOTICE row): {color:?}"
    );
}

/// The stock `YOU_LEFT` arm deletes the window's registration for the channel it matches
/// (`ChatFrame.lua:1382-1384`) and no join re-adds it, so the leave of a renamed slot must match
/// nothing. The window registers by `ChannelID`, as the chat cache does (`ChatFrame.lua:1379`).
#[test]
fn a_zone_change_must_not_deregister_the_channel_it_renames() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 1,
                flags: 0x0_0003,
                pattern: "General - %s".into(),
                shortcut: "General".into(),
            },
        ]),
        ..Default::default()
    };
    channels.claim_slot("General - Elwynn Forest");

    // `ChatFrame_RegisterForChannels(GetChatWindowChannels(1))`'s two writes, by hand.
    s.run("ChatFrame1.channelList[1] = 'General' ChatFrame1.zoneChannelList[1] = 1")
        .unwrap();

    let channel_line = |name: &str| {
        let mut e = ChatEvent::text_only(K::Channel, "anybody out here".into());
        e.sender = "Bob".into();
        e.channel = name.into();
        e
    };
    let notice = |name: &str, byte: &str| {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = name.into();
        e.notice = byte.into();
        e
    };

    // The control: registered by id, a General line reaches the window.
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut channel_line("General - Elwynn Forest"),
    );
    assert_eq!(
        lines_in_window(&s),
        before + 1,
        "the control must print — otherwise the assertion below proves nothing"
    );

    // The border crossing: the leave goes out, the slot is renamed in place, the join goes out, and
    // only then do the two notices land.
    let renamed = channels.rename_slot("General - Elwynn Forest", "General - Westfall");
    assert_eq!(
        renamed,
        Some(1),
        "renamed in place — the slot number does not move"
    );

    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut notice("General - Elwynn Forest", "3"), // YOU_LEFT
    );
    assert_eq!(
        lines_in_window(&s),
        before,
        "the leave prints NOTHING: no slot carries the old name any more, so arg7/arg8/arg9 come \
         out defaulted and the stock handler returns at `found == 0` — which is also why it never \
         reaches the arm that would deregister the channel"
    );

    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut notice("General - Westfall", "2"), // YOU_JOINED
    );

    assert_eq!(
        s.eval::<Option<i64>>("return ChatFrame1.zoneChannelList[1]")
            .unwrap(),
        Some(1),
        "the window must still be registered for ChannelID 1 after the rename — a nil here is \
         General going silent for the rest of the session"
    );

    // Speech from the new zone still lands.
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut channel_line("General - Westfall"),
    );
    assert_eq!(
        lines_in_window(&s),
        before + 1,
        "a General line in the new zone must reach the window"
    );
}

/// Leaving a city sends the leave but keeps Trade's slot in state 3: the notice comes back as
/// `SUSPENDED`, the stock `YOU_LEFT` arm never runs, and a re-join takes the same slot.
#[test]
fn leaving_a_capital_suspends_trade_rather_than_deregistering_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 2,
                flags: 0x0_003B,
                pattern: "Trade - %s".into(),
                shortcut: "Trade".into(),
            },
        ]),
        ..Default::default()
    };
    channels.claim_slot("Trade - City");
    s.run("ChatFrame1.channelList[1] = 'Trade' ChatFrame1.zoneChannelList[1] = 2")
        .unwrap();

    let notice = |name: &str, byte: &str| {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = name.into();
        e.notice = byte.into();
        e
    };

    // The walk: LEAVE goes out, then the eligibility test suspends the slot.
    assert_eq!(channels.suspend_slot("Trade - City"), Some(1));
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut ChatEvent {
            left_constant: true,           // the constant channel's byte
            ..notice("Trade - City", "3")  // YOU_LEFT
        },
    );
    s.resolve();

    // `CHAT_SUSPENDED_NOTICE` reads as `CHAT_YOU_LEFT_NOTICE` does; only arg1, which the stock
    // handler branches on, differs.
    assert_eq!(lines_in_window(&s), before + 1);
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            benilla_ui::script::QuadContent::Text { text: Some(t), .. }
                if t == "Left Channel: |Hchannel:CHANNEL:1|h[1. Trade - City]|h"
        )),
        "the suspended leave renders the same text as an ordinary one"
    );

    assert_eq!(
        channels.number_of("Trade - City"),
        Some(1),
        "the record and its number survive — `/1` still addresses Trade, and the state-3 bypass \
         needs the slot to be there to bypass onto"
    );
    assert_eq!(
        s.eval::<Option<i64>>("return ChatFrame1.zoneChannelList[1]")
            .unwrap(),
        Some(2),
        "and the window is still registered for it: the notice carried the SUSPENDED token, so \
         the stock handler never reached the arm that deletes the registration"
    );

    // Walking back in: the same slot and number, and the notice prints.
    let before = lines_in_window(&s);
    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut notice("Trade - City", "2"), // YOU_JOINED
    );
    assert_eq!(
        lines_in_window(&s),
        before + 1,
        "the re-join prints — it could not have, with the registration gone"
    );
    assert_eq!(channels.number_of("Trade - City"), Some(1));
    assert_eq!(
        channels.slot_state("Trade - City"),
        Some(super::edit::SlotState::Joined),
        "and the slot is back to plain joined"
    );
}

/// A renamed slot's join notice reads as `YOU_CHANGED`: the `0x02` arm splits on `rec+0x9c == 2`.
#[test]
fn a_renamed_zone_channel_confirms_as_changed_not_joined() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 1,
                flags: 0x0_0003,
                pattern: "General - %s".into(),
                shortcut: "General".into(),
            },
        ]),
        ..Default::default()
    };
    channels.claim_slot("General - Elwynn Forest");
    s.run("ChatFrame1.channelList[1] = 'General' ChatFrame1.zoneChannelList[1] = 1")
        .unwrap();
    channels.rename_slot("General - Elwynn Forest", "General - Westfall");

    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.channel = "General - Westfall".into();
    e.notice = "2".into(); // YOU_JOINED
    super::feed::deliver(&mut s, &mut windows, &mut channels, &mut e);
    s.resolve();

    let want = "Changed Channel: |Hchannel:CHANNEL:1|h[1. General - Westfall]|h";
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            benilla_ui::script::QuadContent::Text { text: Some(t), .. } if t == want
        )),
        "expected {want:?} in the window"
    );
    assert_eq!(
        channels.slot_state("General - Westfall"),
        Some(super::edit::SlotState::Joined),
        "and the confirming notice resolves the state — a second crossing must read as a rename \
         of its own, not as a leftover"
    );
}

/// The `YOU_LEFT` arm tears the record down after the fire, so [`super::feed::deliver`] stamps the
/// line before it frees the slot.
#[test]
fn a_leave_notice_keeps_its_number_because_the_record_dies_after_the_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut channels = super::edit::ChannelState::default();
    channels.claim_slot("World");
    channels.claim_slot("General - Elwynn Forest");

    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.channel = "General - Elwynn Forest".into();
    e.notice = "3".into(); // YOU_LEFT
    super::feed::deliver(&mut s, &mut windows, &mut channels, &mut e);

    assert_eq!(e.channel, "2. General - Elwynn Forest", "arg4 was stamped");
    assert_eq!(
        e.channel_number, 2,
        "arg8 — what the color resolves through"
    );
    assert_eq!(
        compose(&e, K::ChannelNotice, "Common").unwrap(),
        "Left Channel: |Hchannel:CHANNEL:2|h[2. General - Elwynn Forest]|h"
    );
    assert_eq!(
        channels.names(),
        [Some("World".to_string()), None],
        "and only THEN is the record gone — as a HOLE at slot 2, not a shortened list"
    );
}

/// The reference frees a slot in place and refills the first free one, so no other number moves.
#[test]
fn a_freed_slot_is_reused_and_the_others_keep_their_numbers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut c = super::edit::ChannelState::default();
    assert_eq!(c.claim_slot("General - Teldrassil"), Some(1));
    assert_eq!(c.claim_slot("Trade - City"), Some(2));
    assert_eq!(c.claim_slot("LocalDefense - Teldrassil"), Some(3));

    // Cross a zone border: General and LocalDefense rename, Trade is untouched.
    assert_eq!(c.free_slot("General - Teldrassil"), Some(1));
    assert_eq!(
        c.claim_slot("General - The Barrens"),
        Some(1),
        "the freed slot is reused — the client scans for a zeroed record before growing"
    );
    assert_eq!(c.free_slot("LocalDefense - Teldrassil"), Some(3));
    assert_eq!(c.claim_slot("LocalDefense - The Barrens"), Some(3));
    assert_eq!(
        c.number_of("Trade - City"),
        Some(2),
        "Trade never moved: /2 still reaches it, which is the whole complaint"
    );

    // Leaving the city drops Trade; the hole it leaves is what the next join takes.
    assert_eq!(c.free_slot("Trade - City"), Some(2));
    assert!(c.joined[1].is_none(), "a hole answers 'not joined'");
    assert_eq!(c.number_of("General - The Barrens"), Some(1), "still 1");
    assert_eq!(
        c.claim_slot("Trade - City"),
        Some(2),
        "and back into slot 2"
    );

    // The ceiling is ten slots.
    for i in 4..=super::edit::MAX_CHANNELS {
        assert_eq!(c.claim_slot(&format!("Custom{i}")), Some(i as u32));
    }
    assert_eq!(c.claim_slot("OneTooMany"), None);
    assert_eq!(c.free_slot("Custom7"), Some(7));
    assert_eq!(
        c.claim_slot("OneTooMany"),
        Some(7),
        "full means no free slot, not a permanent ceiling"
    );
}

/// The reference destroys its Lua state at logout, so the next character's window is empty.
#[test]
fn a_session_end_empties_the_window_and_the_boxs_memory() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let mut log = super::ChatLog::default();

    super::frames::route(&mut s, &mut windows, &ev(K::Say, "hi there", "Bob"));
    super::frames::route(&mut s, &mut windows, &ev(K::Whisper, "psst", "Ann"));
    assert_eq!(
        lines_in_window(&s),
        2,
        "the window has this session's lines"
    );

    super::end_chat_session(Some(&mut s), &mut log);

    assert_eq!(
        lines_in_window(&s),
        0,
        "and the next character starts clean"
    );
}

/// Each token names a `CHAT_<X>_NOTICE` string (`ChatFrame.lua:1416`, `:1424`); the tokenless bytes
/// are MODE_CHANGE (`0x0C`), which fires nothing, and the member lines JOINED and LEFT. 1.12.1's
/// bytes end at THROTTLED (`0x1F`): its reader refuses the rest, and 3.3.5a's four past it are
/// checked against 3.3.5a's strings, not these.
#[test]
fn every_notice_token_resolves_and_the_tokenless_bytes_stay_silent() {
    let _data = benilla_formats::wow_data_or_skip!();
    for byte in 0x00u8..=benilla_protocol::messages::channel_notice::THROTTLED {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = "World".into();
        e.notice = byte.to_string();
        let rendered = GLOBAL_STRINGS.with(|s| {
            super::frames::compose_notice(&e, K::ChannelNotice, &|key| {
                s.lua().globals().get::<String>(key).ok()
            })
        });
        match super::event::notice_token(byte, None, false) {
            Some(token) => assert!(
                rendered.is_some(),
                "notice {byte:#04x} has token {token} but CHAT_{token}_NOTICE resolves to nothing"
            ),
            None => assert_eq!(
                rendered, None,
                "notice {byte:#04x} has no token and must render nothing"
            ),
        }
    }
}

/// `CHAT_INVITE_NOTICE` is `"%2$s has invited you to join the channel '%1$s'."`, filled from the
/// fixed `(arg4, arg2)` list (`ChatFrame.lua:1418`).
#[test]
fn the_invite_notice_reorders_its_two_names() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut e = ChatEvent::text_only(K::ChannelNoticeUser, String::new());
    e.channel = "2. Trade - City".into();
    e.sender = "Ann".into();
    e.notice = benilla_protocol::messages::channel_notice::INVITE.to_string();
    let line = GLOBAL_STRINGS.with(|s| {
        super::frames::compose_notice(&e, K::ChannelNoticeUser, &|key| {
            s.lua().globals().get::<String>(key).ok()
        })
    });
    assert_eq!(
        line.as_deref(),
        Some("Ann has invited you to join the channel '2. Trade - City'."),
        "the inviter fills %2$s and the channel — arg4, zone tail and all — fills %1$s"
    );
}

/// The log's `CHAT_<TOKEN>_NOTICE` takes the window's fill, arg8 for `%d` and arg4 for `[%s]`:
/// 12340's log arm (`0x50aa65`-`0x50aaa9`) passes a buffer's address and the sender (DEVIATIONS.md).
#[test]
fn the_log_writes_a_join_and_a_leave_notice_as_the_window_fills_them() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_protocol::messages::channel_notice as n;
    for (byte, expected) in [
        (
            n::YOU_JOINED,
            "Joined Channel: |Hchannel:1|h[1. General - Stormwind City]|h",
        ),
        (
            n::YOU_LEFT,
            "Left Channel: |Hchannel:1|h[1. General - Stormwind City]|h",
        ),
    ] {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = "1. General - Stormwind City".into();
        e.channel_number = 1;
        e.notice = byte.to_string();
        let log = with_strings(|get| super::frames::log_line(&e, K::ChannelNotice, Some(7), get));
        assert_eq!(log.as_deref(), Some(expected), "notice {byte:#04x}");
    }
    // A token with no string (NOT_IN_LFG has none) still writes its line, the lookup's empty
    // default (`0x819d4c`) filled, as the log write stamps whatever it is handed (`0x775ade`).
    let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
    e.channel = "6. LookingForGroup".into();
    e.channel_number = 6;
    e.notice = n::NOT_IN_LFG.to_string();
    let log = with_strings(|get| super::frames::log_line(&e, K::ChannelNotice, Some(7), get));
    assert_eq!(log.as_deref(), Some(""));
}

/// A new kind breaks [`super::event::event_name`]'s exhaustive match; this makes it join `ALL`.
#[test]
fn every_kind_is_in_all() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut seen: Vec<&str> = K::ALL
        .iter()
        .map(|&k| super::event::event_name(k))
        .collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), before, "a kind is listed twice in ALL");
    assert_eq!(before, 51, "51 kinds — update this when the kind set grows");
}

#[test]
fn colors_match_the_shipped_table() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(default_color(K::Say), [255, 255, 255]);
    assert_eq!(default_color(K::System), [255, 255, 0]);
    assert_eq!(default_color(K::Yell), [255, 64, 64]);
    assert_eq!(default_color(K::Emote), [255, 128, 64]);
    assert_eq!(default_color(K::MonsterSay), [255, 255, 159]);
    assert_eq!(default_color(K::Loot), [0, 170, 0]);
    assert_eq!(default_color(K::Money), [255, 255, 0]);
    assert_eq!(default_color(K::ChannelNotice), [192, 192, 192]);
    assert_eq!(default_color(K::RaidWarning), [255, 72, 0]);
    assert_eq!(default_color(K::RaidLeader), [255, 72, 9]);
    assert_eq!(default_color(K::MonsterWhisper), [255, 181, 235]);
    assert_eq!(default_color(K::BgSystemAlliance), [0, 174, 239]);
    // Every kind has a row: the lookup panics on one the table does not carry.
    for &kind in K::ALL {
        let _ = default_color(kind);
    }
}

// ── the submitted-line grammar: type switches and action commands ──────────────────────────

/// A command table from a stub of the shipped `SLASH_<INDEX><n>` and `EMOTE<i>_CMD<j>` strings.
fn stub_table() -> super::commands::SlashCommands {
    const STRINGS: &[(&str, &str)] = &[
        ("SLASH_JOIN1", "/join"),
        ("SLASH_LEAVE1", "/leave"),
        ("SLASH_LIST_CHANNEL1", "/chatlist"),
        ("SLASH_CHAT_AFK1", "/afk"),
        ("SLASH_CHAT_DND1", "/dnd"),
        ("SLASH_RANDOM1", "/random"),
        ("SLASH_RANDOM2", "/roll"),
        ("SLASH_PLAYED1", "/played"),
        ("SLASH_HELP1", "/help"),
        ("SLASH_PVP1", "/pvp"),
        ("SLASH_REPLY1", "/r"),
        ("SLASH_LOGOUT1", "/logout"),
        ("SLASH_LOGOUT2", "/camp"),
        ("SLASH_QUIT1", "/quit"),
        ("SLASH_TRADE1", "/trade"),
        ("SLASH_SCRIPT1", "/script"),
        // One emote index, in the two-table shape: the alias, and the token it resolves through.
        ("EMOTE1_CMD1", "/wave"),
        ("EMOTE1_CMD2", "/hello"), // an alias that is not the token
        ("EMOTE1_TOKEN", "WAVE"),
    ];
    super::commands::SlashCommands::build(
        |key| {
            STRINGS
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        },
        |token| (token == "WAVE").then_some(101),
    )
}

#[test]
fn action_commands_parse() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    assert_eq!(
        parse_line("/join world secret"),
        ParsedChat::Join {
            name: "world".into(),
            password: "secret".into(),
        }
    );
    assert_eq!(
        parse_line("/leave world"),
        ParsedChat::Leave {
            name: "world".into()
        }
    );
    assert_eq!(
        parse_line("/chatlist world"),
        ParsedChat::ChatList {
            name: "world".into()
        }
    );
    // `/afk` and `/dnd` run the stock `SlashCmdList` bodies, with the argument whole.
    assert_eq!(
        parse_line("/afk farming"),
        ParsedChat::Lua {
            body: "SlashCmdList[\"CHAT_AFK\"](\"farming\")".into()
        }
    );
    // Bare is the toggle: the empty string survives to the call.
    assert_eq!(
        parse_line("/dnd"),
        ParsedChat::Lua {
            body: "SlashCmdList[\"CHAT_DND\"](\"\")".into()
        }
    );
    assert_eq!(parse_line("/roll"), ParsedChat::Random { min: 1, max: 100 });
    assert_eq!(
        parse_line("/random 50"),
        ParsedChat::Random { min: 1, max: 50 }
    );
    assert_eq!(
        parse_line("/random 2 8"),
        ParsedChat::Random { min: 2, max: 8 }
    );
    assert_eq!(parse_line("/played"), ParsedChat::Played);
    assert_eq!(parse_line("/help"), ParsedChat::Help);
    // /pvp takes no argument; a trailing word is ignored.
    assert_eq!(parse_line("/pvp"), ParsedChat::Pvp);
    assert_eq!(parse_line("/pvp on"), ParsedChat::Pvp);
    // /r rides its own arm: the reply state lives on the chat edit state.
    assert_eq!(
        parse_line("/r hey"),
        ParsedChat::Reply { text: "hey".into() }
    );
}

fn emote(text_id: u32, arg: Option<&str>) -> ParsedChat {
    ParsedChat::TextEmote {
        text_id,
        arg: arg.map(str::to_string),
    }
}

#[test]
fn emote_aliases_resolve_through_the_table() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    assert_eq!(parse_line("/wave"), emote(101, None));
    // An alias that is not its token's `EmotesText` name resolves too, as `/lol` (LAUGH) does.
    assert_eq!(parse_line("/hello"), emote(101, None));
    // An emote takes an argument (`DoEmote(token, msg)`): the command is the first word only.
    assert_eq!(parse_line("/wave Bob"), emote(101, Some("Bob")));
    assert_eq!(parse_line("/wave %t"), emote(101, Some("%t")));
    assert_eq!(parse_line("/nosuch"), ParsedChat::Unknown);
}

#[test]
fn logout_and_camp_parse() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    for line in ["/logout", "/camp", "/LOGOUT", "/logout now"] {
        assert_eq!(parse_line(line), ParsedChat::Logout);
    }
    assert_eq!(parse_line("/quit"), ParsedChat::Quit);
}

#[test]
fn one_line_reference_bodies_run_in_the_vm() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    // `/trade` is the stock `InitiateTrade("target")`.
    assert_eq!(
        parse_line("/trade"),
        ParsedChat::Lua {
            body: "InitiateTrade(\"target\")".into()
        }
    );
    // `/script` runs the typed text as the chunk (stock `RunScript(msg)`); bare is a no-op.
    assert_eq!(
        parse_line("/script Print(\"hi\")"),
        ParsedChat::Lua {
            body: "Print(\"hi\")".into()
        }
    );
    assert_eq!(parse_line("/script"), ParsedChat::Unknown);
}

/// `/castvis` is a dev instrument: a player build never claims it (`run_mode::dev_affordances()`).
#[test]
fn castvis_parses_id_and_phase() {
    let _data = benilla_formats::wow_data_or_skip!();
    use crate::creature_anim::CastEventKind;
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    if !crate::run_mode::dev_affordances() {
        assert_eq!(
            parse_line("/castvis 133"),
            ParsedChat::Unknown,
            "a player build must not claim an instrument's alias"
        );
        return;
    }
    assert_eq!(
        parse_line("/castvis 133"),
        ParsedChat::CastVis {
            spell_id: 133,
            kind: CastEventKind::Start { visuals: true },
            ground: false
        }
    );
    assert_eq!(
        parse_line("/castvis 133 go"),
        ParsedChat::CastVis {
            spell_id: 133,
            kind: CastEventKind::Go { ends_cast: true },
            ground: false
        }
    );
    // `ground` is a GO too, the pure-destination shape.
    assert_eq!(
        parse_line("/castvis 1543 GROUND"),
        ParsedChat::CastVis {
            spell_id: 1543,
            kind: CastEventKind::Go { ends_cast: true },
            ground: true
        }
    );
    assert_eq!(
        parse_line("/castvis 689 FAIL"),
        ParsedChat::CastVis {
            spell_id: 689,
            kind: CastEventKind::Fail {
                interrupt: false,
                wire_reason: 0,
                ends_cast: true,
            },
            ground: false
        }
    );
    assert_eq!(parse_line("/castvis"), ParsedChat::Unknown);
    assert_eq!(parse_line("/castvis abc"), ParsedChat::Unknown);
    assert_eq!(parse_line("/castvis 133 nope"), ParsedChat::Unknown);
}

#[test]
fn unknown_slash_command_is_dropped_not_said_aloud() {
    let _data = benilla_formats::wow_data_or_skip!();
    let t = stub_table();
    let parse_line = |line: &str| super::input::parse_line(&t, line);
    assert_eq!(parse_line("/dancemove"), ParsedChat::Unknown);
    assert_eq!(parse_line("/frobnicate"), ParsedChat::Unknown);
}

/// The command table built as boot builds it: `GlobalStrings.lua` and `ChatFrame.lua`'s token
/// table run in a VM, joined to `EmotesText.dbc`.
#[test]
fn real_alias_table_resolves_the_shipped_commands() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let s = benilla_ui::script::UiScript::new().expect("VM");
    for file in ["GlobalStrings.lua", "ChatFrame.lua"] {
        let src = chain
            .read_file(&format!("Interface\\FrameXML\\{file}"))
            .expect("FrameXML file in the chain");
        let src = String::from_utf8_lossy(&src).into_owned();
        // GlobalStrings runs whole; ChatFrame contributes its token table alone.
        let src = if file == "ChatFrame.lua" {
            src.lines()
                .map(str::trim)
                .filter(|l| crate::ui_script::is_emote_token_line(l))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            src
        };
        s.run(&src).expect("runs clean");
    }
    let cat = benilla_formats::load_emote_sound_catalog(&mut chain).expect("emote catalog");
    let globals = s.lua().globals();
    let table = super::commands::SlashCommands::build(
        |name| globals.get::<String>(name).ok().filter(|v| !v.is_empty()),
        |token| cat.text_id(token),
    );
    let parse_line = |line: &str| super::input::parse_line(&table, line);

    // `/sit` is EmotesText 86, whose `Emotes.dbc` row 13 (STATE_SIT) sets stand state 1.
    assert_eq!(parse_line("/sit"), emote(86, None));
    assert_eq!(
        cat.text_emote(86).and_then(|e| cat.posture_state(e)),
        Some(1)
    );
    for (line, state) in [
        ("/stand", 0),
        ("/sit", 1),
        ("/sleep", 3),
        ("/liedown", 3),
        ("/kneel", 8),
    ] {
        let ParsedChat::TextEmote { text_id, .. } = parse_line(line) else {
            panic!("{line} is an emote");
        };
        let posture = cat.text_emote(text_id).and_then(|e| cat.posture_state(e));
        assert_eq!(posture, Some(state), "{line} sets stand state {state}");
    }
    // Aliases that differ from their token's DBC name.
    for line in [
        "/lol",
        "/hi",
        "/ty",
        "/thanks",
        "/congrats",
        "/sorry",
        "/yes",
        "/bravo",
        "/weep",
        "/goodbye",
        "/pizza",
        "/strong",
    ] {
        assert!(
            matches!(parse_line(line), ParsedChat::TextEmote { .. }),
            "{line} resolves to an emote"
        );
    }
    // DBC emote names the reference has no command for.
    for line in ["/joke", "/puzzle", "/attackmytarget"] {
        assert_eq!(parse_line(line), ParsedChat::Unknown, "{line}");
    }
    // `SlashCmdList["FOLLOW"]`, over the three distinct aliases of SLASH_FOLLOW1-6.
    for line in ["/follow", "/f", "/fol"] {
        assert_eq!(
            parse_line(line),
            ParsedChat::Follow { name: None },
            "{line}"
        );
    }
    assert_eq!(
        parse_line("/follow Probeone"),
        ParsedChat::Follow {
            name: Some("Probeone".into())
        }
    );
    // A command whose handler benilla does not register answers like any unknown command.
    assert_eq!(parse_line("/ginvite"), ParsedChat::Unknown);
    // /target and /assist take the whole argument as one name; `/tar` and `/a` are shipped aliases.
    assert_eq!(
        parse_line("/target Kobold Vermin"),
        ParsedChat::Target {
            name: Some("Kobold Vermin".into())
        },
        "the argument is trimmed WHOLE — `GetSlashCmdTarget`'s gsub, not a first-word split"
    );
    assert_eq!(
        parse_line("/tar   Hogger  "),
        ParsedChat::Target {
            name: Some("Hogger".into())
        }
    );
    assert_eq!(parse_line("/target"), ParsedChat::Target { name: None });
    assert_eq!(
        parse_line("/a Bob"),
        ParsedChat::Assist {
            name: Some("Bob".into())
        }
    );
    assert_eq!(parse_line("/assist"), ParsedChat::Assist { name: None });
    // `/cast` and `/spell` run the stock `CastSpellByName(msg)`; `/macro` and `/m` open the window.
    assert_eq!(
        parse_line("/cast Fireball(Rank 1)"),
        ParsedChat::Lua {
            body: "CastSpellByName(\"Fireball(Rank 1)\")".into()
        }
    );
    assert_eq!(
        parse_line("/spell Frostbolt"),
        ParsedChat::Lua {
            body: "CastSpellByName(\"Frostbolt\")".into()
        }
    );
    assert_eq!(
        parse_line("/cast"),
        ParsedChat::Unknown,
        "a bare /cast is the ref's own no-op (`if msg ~= \"\"`)"
    );
    for line in ["/macro", "/m"] {
        assert_eq!(
            parse_line(line),
            ParsedChat::Lua {
                body: "ShowMacroFrame()".into()
            },
            "{line}"
        );
    }
    assert_eq!(parse_line("/macrohelp"), ParsedChat::MacroHelp);
    // 1.12 has none of these: `/convertraid` (the Raid tab's button converts), and benilla's
    // `/reload` and `/errors`, which are its layer's `SlashCmdList` rows, not this table's.
    for line in ["/convertraid", "/reload", "/errors", "/err"] {
        assert_eq!(parse_line(line), ParsedChat::Unknown, "{line}");
    }
    // `/console` from a line that skipped the stock edit box forwards to the stock handler's verb.
    assert_eq!(
        parse_line("/console fpsJournal 1"),
        ParsedChat::Lua {
            body: "ConsoleExec(\"fpsJournal 1\")".into()
        }
    );
    assert_eq!(
        parse_line("/console reloadUI"),
        ParsedChat::Lua {
            body: "ConsoleExec(\"reloadUI\")".into()
        }
    );
    // The quoting is a short string: 1.12's Lua lexer has no long-string levels.
    assert_eq!(super::input::lua_quoted_string("a]]b"), "\"a]]b\"");
    assert_eq!(super::input::lua_quoted_string("a]]b]=]c"), "\"a]]b]=]c\"");
    assert_eq!(
        super::input::lua_quoted_string("say \"hi\"\\n"),
        "\"say \\\"hi\\\"\\\\n\""
    );
    // The literal round-trips through a real VM.
    {
        let vm = benilla_ui::script::UiScript::new().expect("VM");
        for payload in [
            "fpsJournal 1",
            "a]]b",
            "a]]b]=]c",
            "quote \" and backslash \\",
            "tab\there",
        ] {
            let lit = super::input::lua_quoted_string(payload);
            let got: String = vm
                .eval(&format!("return {lit}"))
                .unwrap_or_else(|e| panic!("{lit} must compile on a 1.12-grammar VM: {e}"));
            assert_eq!(got, payload, "and must carry the text unchanged");
        }
    }
    // The shipped surface: 71 distinct aliases over 35 `SlashCmdList` indices and 230 emote
    // commands over 170 `EmotesText` names (aliases repeat; EMOTE27 "UNUSED" has no row), then 7
    // instrument aliases, in dev builds only.
    let instruments = if crate::run_mode::dev_affordances() {
        7
    } else {
        0
    };
    assert_eq!(
        table.counts(),
        (71, 230, instruments),
        "(slash, emote, instrument) aliases"
    );
}

/// The `0x4000` "stand still" flag reports [`EmoteGate::Moving`] on any bit of the reference's
/// `0x20ff` mask (`move_flags::INTEGRATED`), which holds no SWIMMING.
#[test]
fn the_standing_still_arm_reports_moving_and_ignores_swimming() {
    use super::input::EmoteGate;
    use crate::creature_anim::move_flags;

    const STILL: u32 = 0x4000;

    assert_eq!(
        emote_send_eligible(STILL, 0, false, 0, false),
        EmoteGate::Send
    );
    // Any INTEGRATED bit trips it: a direction, a turn or a fall.
    for f in [
        move_flags::FORWARD,
        move_flags::BACKWARD,
        move_flags::STRAFE_LEFT,
        move_flags::TURN_RIGHT,
        move_flags::FALLING,
    ] {
        assert_eq!(
            emote_send_eligible(STILL, 0, false, f, false),
            EmoteGate::Moving,
            "move flag {f:#x}"
        );
    }
    // SWIMMING is not in `0x20ff`: a still swimmer is still.
    assert_eq!(
        emote_send_eligible(STILL, 0, false, move_flags::SWIMMING, false),
        EmoteGate::Send,
        "the mover-integration mask carries no SWIMMING"
    );
    // WALK_MODE and LEVITATING are mode bits, not motion.
    assert_eq!(
        emote_send_eligible(STILL, 0, false, move_flags::WALK_MODE, false),
        EmoteGate::Send
    );
    // Without the flag, movement is irrelevant.
    assert_eq!(
        emote_send_eligible(0, 0, false, move_flags::FORWARD, false),
        EmoteGate::Send
    );
    // An unconditionally suppressed emote (`0x0400`) never reaches this arm: it stays silent.
    assert_eq!(
        emote_send_eligible(0x4400, 0, false, move_flags::FORWARD, false),
        EmoteGate::Suppressed
    );
}

/// `0x8000` refuses silently on the flying bit or a flying spline, ahead of the asleep arm, and the
/// same emote on the ground passes; another row flies free.
#[test]
fn the_flight_arm_refuses_a_mount_special_in_the_air() {
    use super::input::EmoteGate;
    use crate::creature_anim::move_flags;

    const NOT_IN_FLIGHT: u32 = 0x8000;
    let gate = |flags, flags_word, spline| emote_send_eligible(flags, 0, false, flags_word, spline);
    assert_eq!(
        gate(NOT_IN_FLIGHT, move_flags::FLYING, false),
        EmoteGate::Suppressed
    );
    assert_eq!(
        gate(NOT_IN_FLIGHT, 0, true),
        EmoteGate::Suppressed,
        "a taxi"
    );
    assert_eq!(
        gate(NOT_IN_FLIGHT, 0, false),
        EmoteGate::Send,
        "on the ground"
    );
    assert_eq!(
        gate(0, move_flags::FLYING, true),
        EmoteGate::Send,
        "the bit gates"
    );
    assert_eq!(
        emote_gate(Some(NOT_IN_FLIGHT), 0, 0, false, 0, true),
        EmoteGate::Suppressed,
        "through the row"
    );
}

/// Self-control is false only while confused, fleeing or move-disabled, the `0xc00004` mask, which
/// leaves STUNNED out.
#[test]
fn self_controlled_is_true_for_an_ordinary_player() {
    use crate::player::self_controlled;

    assert!(self_controlled(0));
    assert!(self_controlled(1 << 3), "an ordinary PvP-attackable player");
    assert!(!self_controlled(0x0040_0000), "confused");
    assert!(!self_controlled(0x0080_0000), "fleeing");
    assert!(!self_controlled(0x0000_0004), "move-disabled");
    assert!(
        self_controlled(0x0004_0000),
        "STUNNED is not in the 0xc00004 mask — it has its own gate"
    );
}

// ── the send-side emote gate, the emote eligibility check, on real `EmoteFlags` values ──
const BOW: u32 = 0x4801;
const RUDE: u32 = 0x0001;
const APPLAUD: u32 = 0x0000;
const CHEER: u32 = 0x0800;
const SALUTE: u32 = 0x0800;
const LAUGH: u32 = 0x0980;

/// A unit with `UNIT_FLAG_POSSESSED` (`UNIT_FIELD_FLAGS & 0x01000000`) refuses every emote,
/// silently; a missing `Emotes.dbc` row returns too.
#[test]
fn a_possessed_unit_or_a_missing_row_refuses_the_emote() {
    assert_eq!(emote_gate(Some(0), 0, 0, false, 0, false), EmoteGate::Send);
    assert_eq!(
        emote_gate(Some(0), 0x0100_0000, 0, false, 0, false),
        EmoteGate::Suppressed
    );
    assert_eq!(
        emote_gate(Some(0), 0x0100_0008, 0, false, 0, false),
        EmoteGate::Suppressed
    );
    assert_eq!(
        emote_gate(Some(0), 0x0200_0000, 0, false, 0, false),
        EmoteGate::Send,
        "a neighbour bit"
    );
    assert_eq!(
        emote_gate(None, 0, 0, false, 0, false),
        EmoteGate::Suppressed
    );
}

/// A chat-only text emote has `EmoteID` 0, so `DoEmote` reads `Emotes.dbc` row 0 (flags 0), which
/// lacks `0x200`: dead (7) and asleep (3) refuse it, as they refuse a `/wave`.
#[test]
fn a_chat_only_emote_reads_row_zero_and_is_refused_dead_or_asleep() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_emote_sound_catalog(&mut chain).expect("emote catalog");
    let smile = cat.text_id("smile").expect("SMILE");
    assert_eq!(cat.text_emote(smile), None, "SMILE is chat-only");
    let row = cat.emote_flags(cat.text_emote(smile).unwrap_or(0));
    assert_eq!(row, Some(0), "row 0, no flags");
    assert_eq!(
        emote_gate(row, 0, 7, false, 0, false),
        EmoteGate::Suppressed,
        "dead"
    );
    assert_eq!(
        emote_gate(row, 0, 3, false, 0, false),
        EmoteGate::Suppressed,
        "asleep"
    );
    assert_eq!(
        emote_gate(row, 0, 0, false, 0, false),
        EmoteGate::Send,
        "standing"
    );
    assert_eq!(
        emote_gate(row, 0, 1, false, 0, false),
        EmoteGate::Send,
        "sitting"
    );
}

#[test]
fn seated_stand_required_emotes_are_suppressed() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        emote_send_eligible(BOW, 1, false, 0, false),
        EmoteGate::Suppressed
    ); // 0x1 needs STAND
    assert_eq!(
        emote_send_eligible(RUDE, 1, false, 0, false),
        EmoteGate::Suppressed
    );
}

#[test]
fn seated_non_stand_emotes_pass() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        emote_send_eligible(APPLAUD, 1, false, 0, false),
        EmoteGate::Send
    );
    assert_eq!(
        emote_send_eligible(CHEER, 1, false, 0, false),
        EmoteGate::Send
    );
    assert_eq!(
        emote_send_eligible(LAUGH, 1, false, 0, false),
        EmoteGate::Send
    );
    assert_eq!(
        emote_send_eligible(SALUTE, 1, false, 0, false),
        EmoteGate::Send
    );
}

#[test]
fn swimming_suppresses_only_the_0x80_emotes() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        emote_send_eligible(LAUGH, 0, true, 0, false),
        EmoteGate::Suppressed
    ); // 0x0980 has 0x80
    assert_eq!(
        emote_send_eligible(CHEER, 0, true, 0, false),
        EmoteGate::Send
    );
}

#[test]
fn standing_and_dry_everyone_is_eligible() {
    let _data = benilla_formats::wow_data_or_skip!();
    for flags in [BOW, RUDE, APPLAUD, CHEER, SALUTE, LAUGH] {
        assert_eq!(
            emote_send_eligible(flags, 0, false, 0, false),
            EmoteGate::Send,
            "flags {flags:#x}"
        );
    }
}

#[test]
fn unconditional_and_sleep_dead_rules() {
    let _data = benilla_formats::wow_data_or_skip!();
    assert_eq!(
        emote_send_eligible(0x0400, 0, false, 0, false),
        EmoteGate::Suppressed
    ); // unconditional suppress
    assert_eq!(
        emote_send_eligible(0, 3, false, 0, false),
        EmoteGate::Suppressed
    ); // SLEEP, no allow bit
    assert_eq!(
        emote_send_eligible(0, 7, false, 0, false),
        EmoteGate::Suppressed
    ); // DEAD, no allow bit
    assert_eq!(
        emote_send_eligible(0x0200, 3, false, 0, false),
        EmoteGate::Send
    ); // 0x0200 allows it
}

// ── received lines: the addon lane, the language header, the talk gesture ──────────────────

/// The text divides on its first tab, and with no tab it is all prefix; a lane without a name
/// reports `"UNKNOWN"`.
#[test]
fn an_inbound_addon_line_splits_on_the_first_tab_only() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The lane bytes come from the protocol crate, never copied by hand: the inbound line's shared
    // numbering.
    use benilla_protocol::messages as m;
    let party = m::CHAT_MSG_PARTY;
    let raid = m::CHAT_MSG_RAID;
    let guild = m::CHAT_MSG_GUILD;
    let battleground = m::CHAT_MSG_BATTLEGROUND;
    let say = m::CHAT_MSG_SAY;
    #[allow(non_snake_case)]
    let (PARTY, RAID, GUILD, BATTLEGROUND, SAY) = (party, raid, guild, battleground, say);

    let mut log = crate::ui_script::live::UiEvents::default();
    log.push_addon("oRA\tSYNC:1", PARTY, 7);
    // Tabs in the message: only the first divides.
    log.push_addon("CTRA\tA\tB\tC", RAID, 7);
    // No tab: all prefix, an empty message.
    log.push_addon("BareTag", GUILD, 7);
    // A trailing tab: an empty message.
    log.push_addon("Tag\t", BATTLEGROUND, 7);
    // A lane with no name still arrives, labelled.
    log.push_addon("X\ty", SAY, 7);

    assert_eq!(
        super::feed::addon_parts(log.take_chat()),
        vec![
            ("oRA".into(), "SYNC:1".into(), "PARTY".into()),
            ("CTRA".into(), "A\tB\tC".into(), "RAID".into()),
            ("BareTag".into(), String::new(), "GUILD".into()),
            ("Tag".into(), String::new(), "BATTLEGROUND".into()),
            ("X".into(), "y".into(), "UNKNOWN".into()),
        ]
    );
}

/// `CHAT_MSG_ADDON` carries prefix, message, distribution, sender, in that order.
#[test]
fn the_addon_event_reaches_lua_with_four_arguments_in_order() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.run(
        r#"
        seen = nil
        f = CreateFrame("Frame", "AddonSink")
        f:RegisterEvent("CHAT_MSG_ADDON")
        f:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9)
            seen = { arg1, arg2, arg3, arg4 }
        end)
        "#,
    )
    .unwrap();

    super::feed::fire_addon_message(
        &mut s,
        "oRA".into(),
        "SYNC:1".into(),
        "PARTY".into(),
        "Someone".into(),
    );

    assert!(
        s.errors().is_empty(),
        "the fire must not raise: {:?}",
        s.errors()
    );
    assert_eq!(
        s.eval::<String>("return seen[1]").unwrap(),
        "oRA",
        "arg1 is the PREFIX"
    );
    assert_eq!(
        s.eval::<String>("return seen[2]").unwrap(),
        "SYNC:1",
        "arg2 is the MESSAGE"
    );
    assert_eq!(
        s.eval::<String>("return seen[3]").unwrap(),
        "PARTY",
        "arg3 is the DISTRIBUTION, not the sender"
    );
    assert_eq!(
        s.eval::<String>("return seen[4]").unwrap(),
        "Someone",
        "arg4 is the SENDER, and a name rather than a guid"
    );
}

/// The send joins on a tab and the receive splits on the first, so a payload's own tabs come back
/// intact.
#[test]
fn an_addon_message_survives_its_own_send_and_receive() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.run(r#"SendAddonMessage("oRA", "SYNC\t1\t2", "PARTY")"#)
        .unwrap();
    assert!(s.errors().is_empty(), "send raised: {:?}", s.errors());

    let sends = s.take_addon_sends();
    assert_eq!(sends.len(), 1, "one broadcast queued");
    let sent = &sends[0];
    assert_eq!(sent.distribution.token(), "PARTY");

    // The same text arrives as a PARTY line in LANG_ADDON.
    let mut log = crate::ui_script::live::UiEvents::default();
    log.push_addon(std::str::from_utf8(&sent.text).unwrap(), 0x01, 7);

    assert_eq!(
        super::feed::addon_parts(log.take_chat()),
        vec![(
            "oRA".to_string(),
            "SYNC\t1\t2".to_string(),
            "PARTY".to_string()
        )],
        "what one half composed, the other must recover — tabs in the payload included"
    );
}

/// The header shows unless arg3 is empty, "Universal" or `self.defaultLanguage`
/// (`ChatFrame.lua:2901`), the faction's language (`GetDefaultLanguage`).
#[test]
fn the_language_header_suppresses_only_the_frames_own_default_tongue() {
    let _data = benilla_formats::wow_data_or_skip!();
    let orcish = ChatEvent {
        language: "Orcish".into(),
        ..ev(K::Say, "lok'tar", "Grom")
    };
    let common = ChatEvent {
        language: "Common".into(),
        ..ev(K::Say, "hello", "Ann")
    };

    // An Alliance body (default Common): Orcish is tagged, Common is not.
    assert_eq!(
        compose(&orcish, K::Say, "Common").unwrap(),
        "|Hplayer:Grom:7:SAY|h[Grom]|h says: [Orcish] lok'tar"
    );
    assert_eq!(
        compose(&common, K::Say, "Common").unwrap(),
        "|Hplayer:Ann:7:SAY|h[Ann]|h says: hello"
    );

    // A Horde body (default Orcish): the mirror.
    assert_eq!(
        compose(&orcish, K::Say, "Orcish").unwrap(),
        "|Hplayer:Grom:7:SAY|h[Grom]|h says: lok'tar"
    );
    assert_eq!(
        compose(&common, K::Say, "Orcish").unwrap(),
        "|Hplayer:Ann:7:SAY|h[Ann]|h says: [Common] hello"
    );

    // Language 0 arrives as an empty arg3 and is never tagged.
    let universal = ev(K::Say, "system", "Ann");
    assert_eq!(
        compose(&universal, K::Say, "Orcish").unwrap(),
        "|Hplayer:Ann:7:SAY|h[Ann]|h says: system"
    );

    // Comprehension does not matter: an understood foreign tongue is still tagged.
    let dwarvish = ChatEvent {
        language: "Dwarvish".into(),
        ..ev(K::Say, "here we go", "Bran")
    };
    assert_eq!(
        compose(&dwarvish, K::Say, "Common").unwrap(),
        "|Hplayer:Bran:7:SAY|h[Bran]|h says: [Dwarvish] here we go"
    );
}

/// The gesture selector in the parser matches the plaintext that the handler then hands to the
/// display path, so a laugh is language-independent.
#[test]
fn the_talk_gesture_reads_the_plaintext_not_the_garbled_line() {
    let _data = benilla_formats::wow_data_or_skip!();
    use crate::creature_anim::{select_gesture, Gesture};
    use benilla_protocol::messages::CHAT_MSG_SAY;

    let Some(data) = benilla_formats::wow_data() else {
        return; // no client data
    };
    let mut chain = benilla_formats::Chain::open(&data).expect("open patch chain");
    let words = benilla_formats::load_language_words(&mut chain).expect("load word pools");

    // An Orcish `lol` heard by someone with no Orcish at all.
    let garbled = benilla_formats::garble_chat(&words, 1, 0, "lol");
    assert_ne!(garbled, "lol", "the two inputs must actually differ");

    let laugh_words = |n: u32| (n == 1).then(|| "lol".to_string());
    assert_eq!(
        select_gesture(CHAT_MSG_SAY, "lol", laugh_words),
        Some(Gesture::Laugh),
        "the plaintext laughs"
    );
    assert_eq!(
        select_gesture(CHAT_MSG_SAY, &garbled, laugh_words),
        Some(Gesture::Talk),
        "the garbled form would NOT laugh — which is why the feed must pass the plaintext"
    );
}

/// The party leader's own party line is the leader type; a member's and any other lane stay.
#[test]
fn the_party_leader_sends_the_leader_lane() {
    use super::input::party_lane;
    use crate::net::ChatKind as K;
    assert_eq!(party_lane(K::Party, true), K::PartyLeader);
    assert_eq!(party_lane(K::Raid, true), K::RaidLeader);
    assert_eq!(party_lane(K::Battleground, true), K::BattlegroundLeader);
    assert_eq!(party_lane(K::Party, false), K::Party);
    assert_eq!(party_lane(K::Say, true), K::Say);
}

/// A speaker whose `UNIT_FIELD_FLAGS` hold `0x20000000` (Polymorph, Kidney Shot, ...) plays no talk
/// gesture, a line held for the name query included: it re-enters the one composer.
#[test]
fn a_speaker_that_cannot_animate_queues_no_talk_gesture() {
    use super::feed::gesture_prevented;
    use crate::creature_anim::select_gesture;
    use benilla_protocol::messages::CHAT_MSG_SAY;

    let laugh_words = |n: u32| (n == 1).then(|| "lol".to_string());
    let speak = |flags: Option<u32>| {
        select_gesture(CHAT_MSG_SAY, "lol", laugh_words)
            .filter(|_| !gesture_prevented(flags))
            .into_iter()
            .count()
    };
    assert_eq!(speak(Some(0x2000_0000)), 0, "a polymorphed speaker");
    assert_eq!(speak(Some(0x2000_0000 | 0x8)), 0, "among other bits");
    assert_eq!(speak(Some(0)), 1, "an unflagged speaker laughs");
    assert_eq!(
        speak(Some(0x1000_0000 | 0x0100_0000)),
        1,
        "neighbouring bits do not gate"
    );
    assert_eq!(
        speak(None),
        1,
        "an unstreamed speaker is the queue's call, not this gate"
    );
}

// ── the combat log reaches addons ────────────────────────────────────────────────────────────

#[test]
fn every_combat_log_kind_reaches_an_addon() {
    let _data = benilla_formats::wow_data_or_skip!();
    for kind in [K::CombatMiscInfo, K::CombatFactionChange] {
        let name = super::event::event_name(kind);
        let mut s = chat_vm();
        let mut windows = super::frames::ChatWindows::default();
        s.run(SPY).unwrap();
        s.run(&format!(r#"BenillaChatSpy:RegisterEvent("{name}")"#))
            .unwrap();

        let line = format!("a line for {name}");
        super::frames::route(
            &mut s,
            &mut windows,
            &ChatEvent::text_only(kind, line.clone()),
        );

        assert_eq!(
            s.eval::<String>("return SpyEvent").unwrap(),
            name,
            "{name} did not fire under its own name"
        );
        assert_eq!(
            s.eval::<i64>("return SpyN").unwrap(),
            1,
            "{name} fired more than once"
        );
        let seen: String = s.eval("return SpyLine").unwrap();
        assert!(
            seen.starts_with(&format!("{line}|")),
            "{name}: arg1 must be the whole sentence — got {seen:?}"
        );
        assert!(
            s.errors().is_empty(),
            "{name} handler errors: {:?}",
            s.errors()
        );
    }
}

/// #441: the `COMBAT_` kinds are chat lines, so they go to the chat log; the combat log file
/// holds only packed entries (`0x4fb3c0`).
#[test]
fn a_combat_chat_kind_goes_to_the_chat_log() {
    let path = std::env::temp_dir().join(format!("benilla-chatlog-441-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    // `LoggingChat`'s flag, which the add reads (`0x50a975`).
    s.set_logging_flags(true, false);
    let mut windows = super::frames::ChatWindows {
        logs: super::logging::ChatLogFile(super::logging::LogFile::at(&path)),
        ..Default::default()
    };
    for kind in [K::CombatMiscInfo, K::CombatFactionChange] {
        let line = format!("a line for {}", super::event::event_name(kind));
        super::frames::route(&mut s, &mut windows, &ChatEvent::text_only(kind, line));
    }
    drop(windows);
    let written = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let lines: Vec<&str> = written
        .split("\r\n")
        .filter(|l| !l.is_empty())
        .map(|l| l.split_once("  ").map_or(l, |(_, text)| text))
        .collect();
    assert_eq!(
        lines,
        [
            "a line for CHAT_MSG_COMBAT_MISC_INFO",
            "a line for CHAT_MSG_COMBAT_FACTION_CHANGE"
        ]
    );
}

/// The `COMBAT_` and `SPELL_` arms only `AddMessage(arg1)` (`ChatFrame.lua:1397`, `:1399`).
#[test]
fn a_combat_log_line_renders_verbatim() {
    let _data = benilla_formats::wow_data_or_skip!();
    let default_language = String::from("Common");
    for kind in [K::CombatMiscInfo, K::CombatFactionChange] {
        let mut e = ChatEvent::text_only(kind, "You hit Kobold Vermin for 5.".into());
        // Populated so a fall-through to the player branch would show.
        e.sender = "Somebody".into();
        e.flag = "AFK".into();
        e.language = "Orcish".into();
        assert_eq!(
            compose(&e, kind, &default_language).as_deref(),
            Some("You hit Kobold Vermin for 5."),
            "{} must render verbatim",
            super::event::event_name(kind)
        );
    }
}

#[test]
fn both_dock_tabs_exist_and_select_their_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_vm();
    for id in [1, 2] {
        assert!(
            s.eval::<bool>(&format!("return ChatFrame{id}Tab ~= nil"))
                .unwrap(),
            "ChatFrame{id}Tab is missing — its window is unreachable from the screen"
        );
    }
    // The default dock: General selected and shown, Combat Log hidden behind its tab.
    assert_eq!(
        s.eval::<i64>("return SELECTED_DOCK_FRAME:GetID()").unwrap(),
        1
    );
    assert!(s.eval::<bool>("return ChatFrame1:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return ChatFrame2:IsShown()").unwrap());

    // Clicking the Combat Log tab swaps them.
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    assert_eq!(
        s.eval::<i64>("return SELECTED_DOCK_FRAME:GetID()").unwrap(),
        2
    );
    assert!(s.eval::<bool>("return ChatFrame2:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return ChatFrame1:IsShown()").unwrap());
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
}

/// The stock tabs default to the `GENERAL` and `COMBAT_LOG` GlobalStrings
/// (`FloatingChatFrame.lua:684-686`).
#[test]
fn the_dock_tab_labels_come_from_the_install() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let s = benilla_ui::script::UiScript::new().expect("VM");
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    for key in ["GENERAL", "COMBAT_LOG"] {
        let text: String = s.lua().globals().get(key).unwrap_or_default();
        assert!(!text.is_empty(), "{key} is not a GlobalString");
    }
}

#[test]
fn the_combat_log_window_has_the_docks_rect() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_vm();
    let edge = |frame: &str, get: &str| {
        s.eval::<Option<f64>>(&format!("return {frame}:{get}()"))
            .unwrap()
    };
    for get in ["GetLeft", "GetRight", "GetTop", "GetBottom"] {
        let one = edge("ChatFrame1", get);
        let two = edge("ChatFrame2", get);
        assert!(
            one.is_some(),
            "ChatFrame1:{get}() is nil — the dock has no rect"
        );
        assert_eq!(
            two, one,
            "ChatFrame2:{get}() must equal ChatFrame1's — a docked window shares the dock's rect"
        );
    }
    // `FCFDock_AddChatFrame` anchors a docked window with `SetAllPoints(dock.primary)`
    // (`FloatingChatFrame.lua:1996-1998`), so 2 means the dock seeding ran.
    assert_eq!(
        s.eval::<i64>("return ChatFrame2:GetNumPoints()").unwrap(),
        2,
        "SetAllPoints' two points — any other count means the dock never seeded this window"
    );
    assert_eq!(
        s.eval::<i64>("return #GENERAL_CHAT_DOCK.DOCKED_CHAT_FRAMES")
            .unwrap(),
        2,
        "the dock is ChatFrame1 + ChatFrame2, seeded from GetChatWindowInfo's stored positions"
    );
    assert_eq!(
        s.eval::<String>("return SELECTED_DOCK_FRAME:GetName()")
            .unwrap(),
        "ChatFrame1"
    );
    assert!(
        s.eval::<bool>(
            "return ChatFrame1.isDocked and ChatFrame2.isDocked and not ChatFrame3.isDocked"
        )
        .unwrap(),
        "FCF_DockFrame set the flag on the dock's two and nothing else"
    );
}

/// The stock fade and tint walk `CHAT_FRAME_TEXTURES` by frame name (`FloatingChatFrame.lua:30-49`),
/// so each dock window carries all eighteen pieces.
#[test]
fn both_dock_windows_carry_the_same_chrome() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_vm();
    // Read out of the VM, so the check follows the list.
    let n: i64 = s.eval("return #CHAT_FRAME_TEXTURES").unwrap();
    assert_eq!(
        n, 18,
        "the window's plate and border, then its button frame's"
    );
    let suffixes: Vec<String> = (1..=n)
        .map(|i| {
            s.eval::<String>(&format!("return CHAT_FRAME_TEXTURES[{i}]"))
                .unwrap()
        })
        .collect();
    for suffix in &suffixes {
        for id in [1, 2] {
            assert!(
                s.eval::<bool>(&format!("return ChatFrame{id}{suffix} ~= nil"))
                    .unwrap(),
                "ChatFrame{id}{suffix} is missing — the dock's chrome dies when window {id} is up"
            );
        }
    }
    // The one resize grip `FCF_SetLocked` shows and hides (`FloatingChatFrame.lua:950-959`).
    for id in [1, 2] {
        assert!(
            s.eval::<bool>(&format!(
                "return ChatFrame{id}ResizeButton ~= nil and ChatFrame{id}.resizeButton == ChatFrame{id}ResizeButton"
            ))
            .unwrap(),
            "ChatFrame{id}ResizeButton is missing — the window has no handle"
        );
    }
}

/// The stock pass skips `ChatFrame2` (`UIParent.lua:1301`) and ends in `FCF_DockUpdate()`
/// (`:1944`), so a docked window stays on the dock.
#[test]
fn a_docked_chat_window_survives_the_managed_position_pass() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    assert!(
        s.eval::<bool>("return ChatFrame2.isDocked == 1").unwrap(),
        "the combat log ships docked"
    );
    s.run("UIParent_ManageFramePositions()").unwrap();
    s.resolve();
    let rect = |name: &str| {
        s.eval::<(f64, f64, f64, f64)>(&format!(
            "return {name}:GetLeft(), {name}:GetBottom(), {name}:GetRight(), {name}:GetTop()"
        ))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    assert_eq!(
        rect("ChatFrame2"),
        rect("ChatFrame1"),
        "the docked window still sits exactly on the dock"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The swim bit (`EmoteFlags & 0x0080`) is clear on every posture emote, so the refusal to sit in
/// water is the stand-state setter's (`player::state::stand_state_refused`).
#[test]
fn the_posture_emotes_carry_no_swim_suppression_flag() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_emote_sound_catalog(&mut chain).expect("emote catalog");
    // Every posture row, found by scanning.
    let posture: Vec<(u32, u32, u32)> = (0..600u32)
        .filter_map(|id| Some((id, cat.posture_state(id)?, cat.emote_flags(id)?)))
        .collect();
    let states: Vec<u32> = posture.iter().map(|&(_, s, _)| s).collect();
    for state in [0u32, 1, 3, 8] {
        assert!(
            states.contains(&state),
            "the shipped table has a posture emote for stand state {state}; found {states:?}"
        );
    }
    for (id, state, flags) in posture {
        assert_eq!(
            flags & 0x0080,
            0,
            "posture emote {id} (state {state}) carries the swim-suppress bit: {flags:#x}"
        );
        // STATE_DEAD (emote 65, `0x6602`) carries the unconditional suppress bit `0x0400`.
        if flags & 0x0400 != 0 {
            assert_eq!(state, 7, "only STATE_DEAD is unconditionally suppressed");
            continue;
        }
        // A standing swimmer's posture emote passes the gate.
        assert_eq!(
            super::input::emote_send_eligible(flags, 0, true, 0, false),
            super::input::EmoteGate::Send,
            "posture emote {id} passes the emote gate while swimming"
        );
    }
}

/// Fires `PLAYER_LEVEL_UP` with the nine reference args and returns every line the stock handler
/// added to `ChatFrame1`.
fn ding_lines(s: &mut benilla_ui::script::UiScript, args: [i64; 9]) -> Vec<String> {
    s.run(
        r#"
        DingLines = {}
        local add = ChatFrame1.AddMessage
        ChatFrame1.AddMessage = function(self, text, ...)
            table.insert(DingLines, text)
            return add(self, text, unpack(arg))
        end
        "#,
    )
    .unwrap();

    let args = args
        .into_iter()
        .map(benilla_ui::script::ScriptValue::Int)
        .collect();
    s.fire_event("PLAYER_LEVEL_UP", args);
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());

    let n: usize = s.eval("return table.getn(DingLines)").unwrap();
    (1..=n)
        .map(|i| s.eval::<String>(&format!("return DingLines[{i}]")).unwrap())
        .collect()
}

/// Whether `ChatFrame1` draws a line reading `text`.
fn rendered(s: &mut benilla_ui::script::UiScript, text: &str) -> bool {
    s.resolve();
    s.extract().iter().any(|q| {
        matches!(&q.content, benilla_ui::script::QuadContent::Text { text: Some(t), .. } if t == text)
    })
}

/// Stock `ChatFrame_OnEvent` prints the whole level-up block from `PLAYER_LEVEL_UP`
/// (`ChatFrame.lua:2562-2602`), once; the app composes none of it. `LEVEL_UP_CHAR_POINTS` reaches
/// `AddMessage` with its `|4` plural escape, which the window resolves on the count.
#[test]
fn the_ding_block_is_printed_once() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let lines = ding_lines(&mut s, [10, 22, 15, 1, 1, 0, 0, 0, 0]);
    assert_eq!(
        lines,
        [
            "Congratulations, you have reached level 10!",
            "You have gained 22 hit points and 15 mana.",
            "You have gained 1 talent |4point:points;.",
            "Your Strength increases by 1.",
        ]
    );
    assert!(
        rendered(&mut s, "You have gained 1 talent point."),
        "the window draws the singular"
    );
}

/// A loss reaches stock signed, as the reference's `%d` fire passes it: the health line prints it,
/// and a lost stat prints no line (`if ( argN > 0 )`, `ChatFrame.lua:2577-2601`).
#[test]
fn a_negative_gain_prints_signed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let lines = ding_lines(&mut s, [2, -2, 5, 0, -1, 0, 0, 0, 0]);
    assert_eq!(
        lines,
        [
            "Congratulations, you have reached level 2!",
            "You have gained -2 hit points and 5 mana.",
        ]
    );
}

/// On `CHARACTER_POINTS_CHANGED` with `arg2 > 0`, stock `ChatFrame_OnEvent` prints
/// `LEVEL_UP_SKILL_POINTS` (`ChatFrame.lua:2603-2613`).
#[test]
fn the_free_professions_line_is_printed_once() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    s.run(
        r#"
        SkillLines = {}
        local add = ChatFrame1.AddMessage
        ChatFrame1.AddMessage = function(self, text, ...)
            table.insert(SkillLines, text)
            return add(self, text, unpack(arg))
        end
        "#,
    )
    .unwrap();
    let before = lines_in_window(&s);
    // The points watch's fire: arg1 = the talent delta, arg2 = the profession delta; the line
    // reads our points at the call.
    s.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: 0x10,
        ..Default::default()
    });
    let mut points = crate::ui_talent::tests::TalentTestView {
        units: (Some(0x10), None),
        points: Some((0, 2)),
        ..Default::default()
    };
    s.with_live(&mut points, |s| {
        s.fire_event(
            "CHARACTER_POINTS_CHANGED",
            vec![
                benilla_ui::script::ScriptValue::Int(0),
                benilla_ui::script::ScriptValue::Int(1),
            ],
        )
    });
    assert!(s.errors().is_empty(), "handler errors: {:?}", s.errors());
    assert_eq!(
        lines_in_window(&s) - before,
        1,
        "the stock frame prints the professions line from the event"
    );
    assert_eq!(
        s.eval::<String>("return SkillLines[1]").unwrap(),
        "You now have 2 free |4profession:professions;.",
        "LEVEL_UP_SKILL_POINTS, its `|4` escape intact"
    );
    assert!(
        rendered(&mut s, "You now have 2 free professions."),
        "the window draws the plural on cp2"
    );
}

// ───────────── the chat cache restores inside the login ─────────

/// `UPDATE_CHAT_WINDOWS` registers the chat frames for `CHAT_MSG_*`, and the `UPDATE_CHAT_COLOR`
/// burst repaints id-0 lines through `ChatTypeInfo["REPLY"]`, so both come before `PLAYER_LOGIN`.
/// The probe is a loose addon, because the entry load builds the VM it runs on.
#[test]
fn the_chat_cache_restore_is_finished_before_player_login() {
    let _data = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = std::env::temp_dir().join(format!("benilla-chat-order-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("benilla-config");
    let probe = home.join("Interface/AddOns").join("ChatOrderProbe");
    std::fs::create_dir_all(&probe).expect("hermetic home + probe addon dir");
    std::fs::write(
        probe.join("ChatOrderProbe.toc"),
        "## Interface: 30300\nChatOrderProbe.lua\n",
    )
    .expect("probe toc");
    std::fs::write(
        probe.join("ChatOrderProbe.lua"),
        r#"
        ChatOrderProbe = { order = "" }
        local f = CreateFrame("Frame")
        f:RegisterEvent("VARIABLES_LOADED")
        f:RegisterEvent("UPDATE_CHAT_WINDOWS")
        f:RegisterEvent("UPDATE_CHAT_COLOR")
        f:RegisterEvent("PLAYER_LOGIN")
        f:SetScript("OnEvent", function()
            -- The burst is 100+ events; record it once so the order string stays readable.
            if not string.find(ChatOrderProbe.order, event, 1, 1) then
                ChatOrderProbe.order = ChatOrderProbe.order .. event .. " "
            end
            if event == "UPDATE_CHAT_WINDOWS" then
                ChatOrderProbe.windows = (ChatOrderProbe.windows or 0) + 1
            elseif event == "UPDATE_CHAT_COLOR" then
                ChatOrderProbe.colors = (ChatOrderProbe.colors or 0) + 1
            elseif event == "PLAYER_LOGIN" then
                ChatOrderProbe.loginWindows = ChatOrderProbe.windows or 0
                ChatOrderProbe.loginColors = ChatOrderProbe.colors or 0
                ChatOrderProbe.loginRegistered =
                    (ChatFrame1 and ChatFrame1.messageTypeList
                        and table.concat(ChatFrame1.messageTypeList, ",")) or ""
            end
        end)
        "#,
    )
    .expect("probe lua");
    let _capture = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
    let _home =
        crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().expect("utf-8"));

    let mut world = bevy::prelude::World::new();
    world.init_resource::<crate::ui_script::AddOnIdentity>();
    world.init_resource::<crate::minimap::MinimapZoom>();
    world.init_resource::<crate::ui_script::ReloadUiPending>();
    world.init_resource::<super::edit::ChannelState>();
    world.init_resource::<crate::ui_party::GroupState>();
    world.init_resource::<crate::net::SelfGuid>();
    world.init_resource::<super::settings::ChatWindowFile>();
    crate::ui_script::setup_script(&mut world);

    world.insert_resource(crate::char_select::Roster::with_pending_pick(
        vec![benilla_protocol::Character {
            guid: 1,
            name: "Probeorder".into(),
            race: 1,  // Human → Alliance
            class: 1, // Warrior
            gender: 0,
            level: 60,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            zone: 0,
            map: 0,
            position: benilla_protocol::wire::Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            flags: 0,
            customize_flags: 0,
            equipment: [benilla_protocol::CharEnumItem::default(); 19],
            bags: [benilla_protocol::CharEnumItem::default(); 4],
            pet_display_id: 0,
            pet_level: 0,
            pet_family: 0,
        }],
        1,
    ));
    crate::ui_script::load_ingame_ui_on_world_entry(&mut world);
    // The world-entry edge, where `PLAYER_LOGIN` fires (`0x52816d`).
    crate::ui_script::fire_armed_player_login(
        &mut world.non_send_resource_mut::<benilla_ui::script::UiScript>(),
    );

    let read = |expr: &str| -> String {
        world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<Option<String>>(&format!("return tostring({expr})"))
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    assert_eq!(
        read("ChatOrderProbe.loginWindows"),
        "1",
        "UPDATE_CHAT_WINDOWS must have fired before PLAYER_LOGIN — it is the only thing that \
         registers a chat frame for CHAT_MSG_*, so a line routed before it is dropped in silence"
    );
    assert_ne!(
        read("ChatOrderProbe.loginColors"),
        "0",
        "the UPDATE_CHAT_COLOR burst must precede PLAYER_LOGIN — after it, its WHISPER→REPLY \
         mirror repaints every already-printed AceConsole line whisper-pink"
    );
    assert!(
        read("ChatOrderProbe.loginRegistered").contains("SYSTEM"),
        "ChatFrame1 must carry the SYSTEM message group at PLAYER_LOGIN, not {:?}",
        read("ChatOrderProbe.loginRegistered")
    );
    // The reference's login order: `ADDON_LOADED`, `VARIABLES_LOADED`, the chat-cache burst,
    // `PLAYER_LOGIN`.
    assert_eq!(
        read("ChatOrderProbe.order"),
        "VARIABLES_LOADED UPDATE_CHAT_WINDOWS UPDATE_CHAT_COLOR PLAYER_LOGIN ",
        "the reference fires the chat-cache burst BETWEEN VARIABLES_LOADED and PLAYER_LOGIN"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A world where `SendChatMessage` runs on the host inside its call, the chat add lent: the VM's
/// spy records each `CHAT_MSG_SYSTEM` text in order.
fn send_world(
    script: benilla_ui::script::UiScript,
) -> (
    bevy::prelude::World,
    crossbeam_channel::Receiver<crate::net::ClientCommand>,
) {
    script
        .run(
            r#"
            SystemSeen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_SYSTEM")
            f:SetScript("OnEvent", function(_, _, text) SystemSeen[#SystemSeen + 1] = text end)
            MARKED_AFK_MESSAGE = "You are now AFK: %s"
            CLEARED_AFK = "You are no longer AFK."
            CLEARED_DND = "You are no longer marked DND."
            MARKED_DND = "You are now DND: %s."
            DEFAULT_AFK_MESSAGE = "Away from Keyboard"
            DEFAULT_DND_MESSAGE = "Do not Disturb"
            "#,
        )
        .expect("lua");
    let mut world = bevy::prelude::World::new();
    world.insert_non_send_resource(script);
    let rx = super::feed::insert_add_resources(&mut world);
    world.init_resource::<super::away::AfkMirror>();
    world.init_resource::<crate::cvars::Cvars>();
    world.init_resource::<crate::ui_party::GroupState>();
    // The verb returns before its arguments with no active player (`0x50d1ab`).
    world.spawn((
        crate::net::SelfPlayer,
        crate::net::ObjectStore(benilla_protocol::messages::ObjectFields::default()),
    ));
    (world, rx)
}

/// Replace the active player's descriptor that [`send_world`] spawned.
fn set_self_store(world: &mut bevy::prelude::World, fields: benilla_protocol::ObjectFields) {
    let me = world
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<crate::net::SelfPlayer>>()
        .single(world)
        .unwrap();
    world.entity_mut(me).insert(crate::net::ObjectStore(fields));
}

/// Run `lua` with the `World` lent, as every call into Lua runs.
fn run_lent(world: &mut bevy::prelude::World, lua: &str) -> mlua::Result<()> {
    crate::ui_script::safe_world::lend(world, |s| s.run(lua)).expect("a VM")
}

/// The `CHAT_MSG_SYSTEM` texts the spy saw, drained.
fn system_seen(world: &bevy::prelude::World) -> Vec<String> {
    system_seen_on(world.non_send_resource::<benilla_ui::script::UiScript>())
}

/// [`system_seen`] on a VM held outside the `World`.
fn system_seen_on(script: &benilla_ui::script::UiScript) -> Vec<String> {
    script
        .eval::<Vec<String>>("local s = SystemSeen SystemSeen = {} return s")
        .unwrap()
}

/// `/afk` then `/dnd` inside their calls (`0x50d170`): every type but AFK clears a standing AFK
/// first (`0x50d425`), so `/dnd` (`0x15`) prints the clear and sends the empty `0x14` ahead of its
/// own line and packet, three lines in all, each added before the verb returns.
#[test]
fn afk_then_dnd_clears_the_afk_first() {
    use crate::net::{ChatKind, ClientCommand};

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    run_lent(
        &mut world,
        r#"
        SendChatMessage("", "AFK")
        AfterAfk = #SystemSeen
        SendChatMessage("", "DND")
        "#,
    )
    .expect("lua");
    assert_eq!(
        world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<i64>("return AfterAfk")
            .unwrap(),
        1,
        "the AFK line is added inside its call"
    );
    assert_eq!(
        system_seen(&world),
        vec![
            "You are now AFK: Away from Keyboard",
            "You are no longer AFK.",
            "You are now DND: Do not Disturb.",
        ]
    );
    let sent: Vec<(ChatKind, String)> = rx
        .try_iter()
        .map(|c| match c {
            ClientCommand::Chat { kind, text, .. } => (kind, String::from_utf8(text).unwrap()),
            other => panic!("unexpected command {other:?}"),
        })
        .collect();
    assert_eq!(
        sent,
        vec![
            (ChatKind::Afk, "Away from Keyboard".into()),
            (ChatKind::Afk, String::new()),
            (ChatKind::Dnd, "Do not Disturb".into()),
        ]
    );
    assert!(!world.resource::<super::away::AfkMirror>().is_afk());
}

/// The AFK mark and clear write language 0 into their packets (`0x6dc724`, `0x6d5356`), not the
/// speaker's tongue the plain send would take.
#[test]
fn the_afk_packets_carry_language_zero() {
    use crate::net::{ChatKind, ClientCommand};

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    run_lent(&mut world, r#"SendChatMessage("", "AFK")"#).expect("lua");
    super::input::clear_afk(&mut world, true);
    let sent: Vec<(ChatKind, Option<u32>)> = rx
        .try_iter()
        .map(|c| match c {
            ClientCommand::Chat { kind, language, .. } => (kind, language),
            other => panic!("unexpected command {other:?}"),
        })
        .collect();
    assert_eq!(
        sent,
        vec![(ChatKind::Afk, Some(0)), (ChatKind::Afk, Some(0))]
    );
}

/// The AFK clear (`0x6d52d0`) acts only with the mirror set (`0x6d52d9`) and, unforced,
/// `autoClearAFK` on (`0x6d52ea`); a mirror of 2 is set, as every reader tests non-zero. Acting, it
/// zeroes the mirror, adds `CLEARED_AFK` in the call and sends the empty AFK message.
#[test]
fn the_afk_clear_is_gated_on_the_mirror_and_the_cvar() {
    use crate::net::{ChatKind, ClientCommand};

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    world.resource_mut::<super::away::AfkMirror>().0 = 1;
    world
        .resource_mut::<crate::cvars::Cvars>()
        .set("autoClearAFK", "0");
    assert!(!super::input::clear_afk(&mut world, false), "cvar off");
    assert!(world.resource::<super::away::AfkMirror>().is_afk());
    assert!(
        super::input::clear_afk(&mut world, true),
        "forced past the cvar"
    );
    world
        .resource_mut::<crate::cvars::Cvars>()
        .set("autoClearAFK", "1");
    assert!(!super::input::clear_afk(&mut world, false), "not AFK");
    world.resource_mut::<super::away::AfkMirror>().0 = 2;
    assert!(super::input::clear_afk(&mut world, false), "2 is set");
    assert!(!world.resource::<super::away::AfkMirror>().is_afk());
    assert_eq!(
        system_seen(&world),
        vec!["You are no longer AFK.", "You are no longer AFK."]
    );
    let sent: Vec<(ChatKind, String)> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::Chat { kind, text, .. } => {
                Some((kind, String::from_utf8(text).unwrap()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        vec![
            (ChatKind::Afk, String::new()),
            (ChatKind::Afk, String::new())
        ]
    );
}

/// The chat commands a test's receiver holds, as (type, text).
fn chat_sent(
    rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>,
) -> Vec<(crate::net::ChatKind, String)> {
    rx.try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::Chat { kind, text, .. } => {
                Some((kind, String::from_utf8(text).unwrap()))
            }
            _ => None,
        })
        .collect()
}

/// An empty AFK line with the mirror set is the forced AFK clear (`0x50d5ee` -> `0x6d52d0`), which
/// zeroes the mirror (`0x6d530e`) before it adds `CLEARED_AFK` (`0x6d531d`): a `CHAT_MSG_SYSTEM`
/// listener that says something finds the mirror clear, so its SAY clears nothing and goes out
/// ahead of the clear's own empty AFK packet.
#[test]
fn the_afk_clear_zeroes_the_mirror_before_its_line() {
    use crate::net::ChatKind;

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    world.resource_mut::<super::away::AfkMirror>().0 = 1;
    run_lent(
        &mut world,
        r#"
        local f = CreateFrame("Frame")
        f:RegisterEvent("CHAT_MSG_SYSTEM")
        f:SetScript("OnEvent", function() if not Said then Said = true SendChatMessage("x", "SAY") end end)
        SendChatMessage("", "AFK")
        "#,
    )
    .expect("lua");
    assert_eq!(system_seen(&world), vec!["You are no longer AFK."]);
    assert_eq!(
        chat_sent(&rx),
        vec![(ChatKind::Say, "x".into()), (ChatKind::Afk, String::new())]
    );
    assert!(!world.resource::<super::away::AfkMirror>().is_afk());
}

/// The AFK mark (`0x6dc640`) under a live DND bit adds `CLEARED_DND` before `MARKED_AFK_MESSAGE`
/// (`0x6dc675`-`0x6dc6e0`); with the mirror already set it adds nothing and still sends the text.
#[test]
fn the_afk_mark_clears_a_standing_dnd_in_text_first() {
    use crate::net::ChatKind;

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    set_self_store(
        &mut world,
        benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_PLAYER_FLAGS,
            0x4,
        )]),
    );
    run_lent(&mut world, r#"SendChatMessage("brb", "AFK")"#).expect("lua");
    assert_eq!(
        system_seen(&world),
        vec!["You are no longer marked DND.", "You are now AFK: brb"]
    );
    run_lent(&mut world, r#"SendChatMessage("later", "AFK")"#).expect("lua");
    assert!(system_seen(&world).is_empty(), "a set mirror adds nothing");
    assert_eq!(
        chat_sent(&rx),
        vec![
            (ChatKind::Afk, "brb".into()),
            (ChatKind::Afk, "later".into())
        ]
    );
    // The idle leg's mark (`0x52b349`) is the same function: the DND line, then the default text.
    world.resource_mut::<super::away::AfkMirror>().0 = 0;
    super::input::set_afk(&mut world, b"");
    assert_eq!(
        system_seen(&world),
        vec![
            "You are no longer marked DND.",
            "You are now AFK: Away from Keyboard"
        ]
    );
    assert_eq!(
        chat_sent(&rx),
        vec![(ChatKind::Afk, "Away from Keyboard".into())]
    );
}

/// Speak `language` through the comprehend branch of the skill test (`0x6e0699`-`0x6e0751`): the
/// `UNIT_FIELD_FLAGS_2` bit `0x8` and a held aura of type 244 on it.
fn comprehend(world: &mut bevy::prelude::World, language: i32) {
    const TONGUE: u32 = 90_000;
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [(
            TONGUE,
            benilla_formats::SpellDisplay {
                effect_apply_aura: [244, 0, 0],
                effect_misc_value: [language, 0, 0],
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    );
    world.insert_resource(spells);
    world.init_resource::<crate::ui_action::PlayerActions>();
    world.insert_resource(crate::ui_spellbook::SkillLines {
        catalog: std::sync::Arc::new(benilla_formats::SkillLineCatalog::from_lines([])),
    });
    set_self_store(
        world,
        benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_FLAGS_2,
            0x8,
        )]),
    );
    let me = world
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<crate::net::SelfPlayer>>()
        .single(world)
        .unwrap();
    world
        .entity_mut(me)
        .insert(crate::unit_auras::UnitAuras::of(0, &[(0, TONGUE, 1)]));
}

/// `SendChatMessage`'s language inside the call: the id the binding resolved rides every type's
/// packet but AFK's, whose AFK mark never reads it and writes 0 (`0x6dc724`).
#[test]
fn a_named_language_rides_the_chat_command() {
    use crate::net::{ChatKind, ClientCommand};

    let mut script = benilla_ui::script::UiScript::new().expect("VM");
    script.set_language_table(vec![(2, "Darnassian".into()), (7, "Common".into())]);
    let (mut world, rx) = send_world(script);
    comprehend(&mut world, 2);
    run_lent(
        &mut world,
        r#"
        SendChatMessage("ishnu", "SAY", "darnassian")
        SendChatMessage("hi", "WHISPER", "Darnassian", "Bob")
        SendChatMessage("hi")
        SendChatMessage("brb", "AFK", "Darnassian")
        "#,
    )
    .expect("lua");
    let sent: Vec<(ChatKind, Option<u32>)> = rx
        .try_iter()
        .map(|c| match c {
            ClientCommand::Chat { kind, language, .. } => (kind, language),
            other => panic!("unexpected command {other:?}"),
        })
        .collect();
    assert_eq!(
        sent,
        vec![
            (ChatKind::Say, Some(2)),
            (ChatKind::Whisper, Some(2)),
            (ChatKind::Say, None),
            (ChatKind::Afk, Some(0)),
        ]
    );
}

/// `SendChatMessage`'s `CHANNEL` target inside the call: an integer parse, so the packet carries
/// the numbered slot's name, and a number naming no confirmed slot, or a name, sends nothing at all
/// (`0x50d52f`-`0x50d569`).
#[test]
fn a_channel_send_carries_the_numbered_slots_name() {
    use super::edit::{ChannelSlot, ChannelState, SlotState};
    use crate::net::{ChatKind, ClientCommand};

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    world.insert_resource(ChannelState {
        joined: vec![
            Some(ChannelSlot::zone("General - Elwynn Forest", 1)),
            Some(ChannelSlot::zone("Trade - City", 2)),
            None,
            Some(ChannelSlot {
                name: "LocalDefense - Elwynn Forest".into(),
                id: 22,
                state: SlotState::Suspended,
                leave_asked: false,
                flags: 0,
                instance: 0,
                voice: false,
                permanent: false,
            }),
        ],
        ..Default::default()
    });
    run_lent(
        &mut world,
        r#"
        SendChatMessage("lf1m", "CHANNEL", nil, 2)
        SendChatMessage("hello", "CHANNEL", nil, "1")
        SendChatMessage("float", "CHANNEL", nil, 2.7)
        SendChatMessage("hole", "CHANNEL", nil, 3)
        SendChatMessage("suspended", "CHANNEL", nil, 4)
        SendChatMessage("past the end", "CHANNEL", nil, 9)
        SendChatMessage("by name", "CHANNEL", nil, "General - Elwynn Forest")
        SendChatMessage("zero", "CHANNEL", nil, "0")
        SendChatMessage("tell", "WHISPER", nil, "2")
        "#,
    )
    .expect("lua");
    // No target is the verb's error and the chunk stops there (`0x50d587`, `0x50d302`).
    for (chunk, error) in [
        (
            r#"SendChatMessage("none", "CHANNEL") SendChatMessage("after", "SAY")"#,
            "SendChatMessage(): Channel send missing channel number",
        ),
        (
            r#"SendChatMessage("empty", "CHANNEL", nil, "") SendChatMessage("after", "SAY")"#,
            "SendChatMessage(): Channel send missing channel number",
        ),
        (
            r#"SendChatMessage("hi", "WHISPER") SendChatMessage("after", "SAY")"#,
            "SendChatMessage(): Whisper message missing target player!",
        ),
    ] {
        let err = run_lent(&mut world, chunk).expect_err(chunk);
        assert!(err.to_string().contains(error), "{chunk}: {err}");
    }
    let sent: Vec<(ChatKind, Option<String>, String)> = rx
        .try_iter()
        .map(|c| match c {
            ClientCommand::Chat {
                kind, target, text, ..
            } => (kind, target, String::from_utf8(text).unwrap()),
            other => panic!("unexpected command {other:?}"),
        })
        .collect();
    let chan = |target: &str, text: &str| {
        (
            ChatKind::Channel,
            Some(target.to_string()),
            text.to_string(),
        )
    };
    assert_eq!(
        sent,
        vec![
            chan("Trade - City", "lf1m"),
            chan("General - Elwynn Forest", "hello"),
            chan("Trade - City", "float"),
            (ChatKind::Whisper, Some("2".into()), "tell".into()),
        ]
    );
}

/// The join verb claims a record for any channel, custom ones included, before its send
/// (`0x507c47`); a held record skips the claim and the send but still answers (`0x507c1b` ->
/// `0x507cea`); with all ten taken it shows `ERR_TOO_MANY_CHAT_CHANNELS` and answers nothing,
/// sending nothing (`0x507755`, `0x507c51`, `0x50da57`).
#[test]
fn the_join_verb_claims_its_record_or_skips_the_send() {
    use super::edit::ChannelState;
    use crate::net::ClientCommand;

    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(
            r#"
            ERR_TOO_MANY_CHAT_CHANNELS = "You have joined too many channels."
            "#,
        )
        .expect("lua");
    let (mut world, rx) = send_world(script);
    world.init_resource::<crate::sound::MessageSounds>();
    world.insert_resource(ChannelState::default());
    let joins = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<String> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::JoinChannel { name, .. } => Some(name),
                _ => None,
            })
            .collect()
    };
    let eval = |world: &mut bevy::prelude::World, lua: &str| -> String {
        crate::ui_script::safe_world::lend(world, |s| s.eval::<String>(lua))
            .expect("a VM")
            .expect("lua")
    };
    assert_eq!(
        eval(
            &mut world,
            "local n = select('#', JoinChannelByName('Mine'))              return n .. ':' .. GetChannelName('Mine')"
        ),
        "2:1",
        "a custom channel's record is claimed inside the call"
    );
    assert_eq!(joins(&rx), vec!["Mine".to_string()]);
    assert_eq!(
        eval(
            &mut world,
            "return select('#', JoinChannelByName('mine', nil, 1))"
        ),
        "2",
        "a held record answers"
    );
    assert_eq!(joins(&rx), Vec::<String>::new(), "and sends nothing");
    for i in 2..=10 {
        eval(&mut world, &format!("JoinChannelByName('C{i}') return ''"));
    }
    assert_eq!(joins(&rx).len(), 9);
    run_lent(
        &mut world,
        r#"
        ErrSeen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("CHAT_MSG_SYSTEM")
        f:SetScript("OnEvent", function(_, _, text) ErrSeen[#ErrSeen + 1] = text end)
        "#,
    )
    .expect("lua");
    assert_eq!(
        eval(
            &mut world,
            "return select('#', JoinChannelByName('Eleventh', nil, 2))"
        ),
        "0",
        "a failed claim answers nothing"
    );
    assert_eq!(joins(&rx), Vec::<String>::new(), "and sends nothing");
    assert_eq!(
        eval(&mut world, "return table.concat(ErrSeen, '|')"),
        "You have joined too many channels."
    );
    assert_eq!(world.resource::<ChannelState>().number_of("Eleventh"), None);
}

/// The join's `ERR_TOO_MANY_CHAT_CHANNELS` shows inside the call on the calling VM (`0x507755` ->
/// `0x5216f0`): with the `World`'s VM held outside it, as the world-entry load and a reload hold
/// it while addons run, a file-scope join with ten held records still shows its line.
#[test]
fn a_join_with_ten_records_during_the_load_shows_its_line() {
    use super::edit::{ChannelSlot, ChannelState};

    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(r#"ERR_TOO_MANY_CHAT_CHANNELS = "You have joined too many channels.""#)
        .expect("lua");
    let (mut world, rx) = send_world(script);
    world.init_resource::<crate::sound::MessageSounds>();
    world.insert_resource(ChannelState {
        joined: (1..=10)
            .map(|i| Some(ChannelSlot::joined(&format!("C{i}"))))
            .collect(),
        ..Default::default()
    });
    world.resource_mut::<super::ChatLog>().leave_world();
    let mut vm = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");
    let answered = crate::ui_script::safe_world::lend_to(&mut world, &mut vm, |s| {
        s.eval::<i64>("return select('#', JoinChannelByName('Eleventh'))")
    })
    .expect("lua");
    assert_eq!(answered, 0, "a failed claim answers nothing");
    assert_eq!(rx.try_iter().count(), 0, "and sends nothing");
    world.insert_non_send_resource(vm);
    super::feed::enter_world(&mut world);
    assert_eq!(
        system_seen(&world),
        vec!["You have joined too many channels."]
    );
}

/// `SendChatMessage` cuts its line at 255 bytes (`0x577d65`) even inside a UTF-8 sequence, and
/// the packet carries the partial bytes as cut (`0x50d81c`).
#[test]
fn a_send_cut_inside_a_character_carries_its_bytes() {
    use crate::net::ClientCommand;

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    run_lent(
        &mut world,
        r#"SendChatMessage(string.rep("a", 254) .. "\195\169", "SAY")"#,
    )
    .expect("lua");
    let sent: Vec<Vec<u8>> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::Chat { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    let mut want = vec![b'a'; 254];
    want.push(0xC3);
    assert_eq!(sent, vec![want]);
}

/// `SendAddonMessage`'s gates and packet in the call (`0x500678`-`0x50071f`): with no group, PARTY
/// and a RAID taken as PARTY send nothing (`0x500684`) while GUILD and a WHISPER go out; in a
/// party RAID goes as PARTY (`0x50066c`); in a raid it stays RAID.
#[test]
fn the_addon_message_gates_run_in_the_call() {
    use crate::net::ClientCommand;
    use benilla_ui::script::AddonDistribution as D;

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    let sent =
        |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<(D, Option<String>, Vec<u8>)> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::AddonMessage {
                        distribution,
                        target,
                        text,
                    } => Some((distribution, target, text)),
                    _ => None,
                })
                .collect()
        };
    run_lent(
        &mut world,
        r#"
        SendAddonMessage("p", "a")
        SendAddonMessage("p", "b", "RAID")
        SendAddonMessage("p", "c", "GUILD")
        SendAddonMessage("p", "d", "WHISPER", "Bob")
        "#,
    )
    .expect("lua");
    assert_eq!(
        sent(&rx),
        vec![
            (D::Guild, None, b"p\tc".to_vec()),
            (D::Whisper, Some("Bob".into()), b"p\td".to_vec()),
        ]
    );
    lead_a_raid(&mut world, 0x99);
    world
        .resource_mut::<crate::ui_party::GroupState>()
        .group_type = 0;
    run_lent(&mut world, r#"SendAddonMessage("p", "e", "RAID")"#).expect("lua");
    assert_eq!(sent(&rx), vec![(D::Party, None, b"p\te".to_vec())]);
    lead_a_raid(&mut world, 0x99);
    run_lent(&mut world, r#"SendAddonMessage("p", "f", "RAID")"#).expect("lua");
    assert_eq!(sent(&rx), vec![(D::Raid, None, b"p\tf".to_vec())]);
}

/// A raid led by `leader` whose subgroup holds one other member, so the party test passes too.
fn lead_a_raid(world: &mut bevy::prelude::World, leader: u64) {
    lead_a_raid_rows(
        &mut world.resource_mut::<crate::ui_party::GroupState>(),
        leader,
    );
}

/// `SendChatMessage`'s first gates and its UTF-8 test: with no active player it returns before any
/// argument, its usage error included (`0x50d18b`-`0x50d1ab`); a bad lead or trail byte is the
/// UTF-8 error (`0x50d244`), while a sequence the line's end cuts, and an overlong form, walk valid
/// (`0x47bd40`).
#[test]
fn no_active_player_returns_first_and_the_line_walks_as_utf8() {
    use crate::net::{ChatKind, ClientCommand};

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    let me = world
        .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<crate::net::SelfPlayer>>()
        .single(&world)
        .unwrap();
    world.entity_mut(me).despawn();
    run_lent(
        &mut world,
        r#"SendChatMessage(nil) SendChatMessage("hi", "BOGUS") SendChatMessage("\255")"#,
    )
    .expect("no active player: nothing is read");
    assert_eq!(rx.try_iter().count(), 0);

    world.spawn((
        crate::net::SelfPlayer,
        crate::net::ObjectStore(benilla_protocol::messages::ObjectFields::default()),
    ));
    for chunk in [
        r#"SendChatMessage("a\255b")"#,
        r#"SendChatMessage("\195x")"#,
    ] {
        let err = run_lent(&mut world, chunk).expect_err(chunk);
        assert!(
            err.to_string()
                .contains("SendChatMessage(): Chat message must be UTF-8 text"),
            "{chunk}: {err}"
        );
    }
    run_lent(
        &mut world,
        r#"SendChatMessage("ok\195") SendChatMessage("\192\128")"#,
    )
    .expect("a cut sequence and an overlong form pass");
    let sent: Vec<ChatKind> = rx
        .try_iter()
        .map(|c| match c {
            ClientCommand::Chat { kind, .. } => kind,
            other => panic!("unexpected command {other:?}"),
        })
        .collect();
    assert_eq!(sent, vec![ChatKind::Say, ChatKind::Say]);
}

/// `SendChatMessage`'s line passes past the tutorial flag (`0x50d6ac`-`0x50d7ae`): `%t` names the
/// target from the name cache with no query and `%f` with no focus is `FOCUS_TOKEN_NOT_FOUND`
/// (`0x577d00`); a line break cuts the line (`0x50d6f7`); `||` passes, an unclosed `|c` cuts at its
/// `|` (`0x50d7a9`), and any other escape is the verb's Lua error with nothing sent (`0x50d7b1`).
#[test]
fn the_line_passes_name_tokens_cut_lines_and_walk_escapes() {
    use crate::net::ClientCommand;

    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(r#"FOCUS_TOKEN_NOT_FOUND = "<no focus>" TARGET_TOKEN_NOT_FOUND = "<no target>""#)
        .expect("lua");
    let (mut world, rx) = send_world(script);
    world.init_resource::<crate::target::Selection>();
    let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<String> {
        rx.try_iter()
            .map(|c| match c {
                ClientCommand::Chat { text, .. } => String::from_utf8(text).unwrap(),
                other => panic!("unexpected command {other:?}"),
            })
            .collect()
    };

    run_lent(&mut world, r#"SendChatMessage("at %t, %f")"#).expect("lua");
    assert_eq!(sent(&rx), vec!["at <no target>, <no focus>"]);

    // A player target out of view: the name cache's record, read with no query.
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(0x42, "Tom".into(), None);
    world.resource_mut::<crate::target::Selection>().guid = Some(0x42);
    run_lent(
        &mut world,
        r#"SendChatMessage("hi %T 50%") SendChatMessage("a\\nb") SendChatMessage("x\ny")"#,
    )
    .expect("lua");
    assert_eq!(sent(&rx), vec!["hi Tom 50%", "a", "x"]);

    run_lent(
        &mut world,
        r#"SendChatMessage("a || b") SendChatMessage("|cffff0000red|r ok") SendChatMessage("cut |cffff0000 here")"#,
    )
    .expect("lua");
    assert_eq!(
        sent(&rx),
        vec!["a || b", "|cffff0000red|r ok", "cut "],
        "a closed colour passes; an unclosed one cuts at its pipe"
    );

    for chunk in [
        r#"SendChatMessage("|Tfoo|t")"#,
        r#"SendChatMessage("trail |")"#,
        r#"SendChatMessage("|cff00ff00 ok|r |r")"#,
    ] {
        let err = run_lent(&mut world, chunk).expect_err(chunk);
        assert!(
            err.to_string()
                .contains("SendChatMessage(): Invalid escape code in chat message"),
            "{chunk}: {err}"
        );
    }
    assert_eq!(
        sent(&rx),
        Vec::<String>::new(),
        "a bad escape sends nothing"
    );
}

/// [`lead_a_raid`] on a borrowed state.
fn lead_a_raid_rows(g: &mut crate::ui_party::GroupState, leader: u64) {
    g.in_group = true;
    g.group_type = 1;
    g.leader = leader;
    g.own_flags = 0;
    g.members = vec![benilla_protocol::messages::GroupMemberEntry {
        name: "Bob".into(),
        guid: 0x99,
        status: 1,
        flags: 0,
        roles: 0,
    }];
}

/// `SendChatMessage`'s refusals ahead of the AFK clear (`0x50d417`), each in its call with no packet
/// and the AFK left standing: the language skill (`0x50d2b0` -> `ERR_CANT_SPEAK_LANGAGE`), PARTY
/// outside a party and a battleground raid (`0x50d31e`-`0x50d340`), RAID_WARNING from a raid's
/// plain member (`0x50d386`) and outside any group (`0x50d3a2`), RAID and BATTLEGROUND with no raid
/// (`0x50d3cc`-`0x50d3dd`). A party leader with no raid gets `ERR_NOT_IN_RAID`, never `0x27`.
#[test]
fn the_group_and_language_refusals_return_before_the_afk_clear() {
    use crate::net::{ChatKind, ClientCommand};

    let mut script = benilla_ui::script::UiScript::new().expect("VM");
    script.set_language_table(vec![(1, "Orcish".into()), (2, "Darnassian".into())]);
    script
        .run(
            r#"
            ERR_CANT_SPEAK_LANGAGE = "You can't speak that language."
            ERR_NOT_IN_GROUP = "You aren't in a party."
            ERR_NOT_LEADER = "You are not the party leader."
            ERR_NOT_IN_RAID = "You are not in a raid group"
            ERR_NOT_IN_BATTLEGROUND = "You aren't in a Battleground."
            ErrorSeen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UI_ERROR_MESSAGE")
            f:RegisterEvent("CHAT_MSG_SYSTEM")
            f:SetScript("OnEvent", function(_, _, text) ErrorSeen[#ErrorSeen + 1] = text end)
            "#,
        )
        .expect("lua");
    let (mut world, rx) = send_world(script);
    world.init_resource::<crate::sound::MessageSounds>();
    world.init_resource::<bevy::prelude::Messages<crate::tutorial::TutorialEvent>>();
    world.insert_resource(crate::net::SelfGuid(Some(0x7)));
    world.insert_resource(super::away::AfkMirror(2));
    // We comprehend Darnassian alone, so Orcish is refused.
    comprehend(&mut world, 2);
    let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<(ChatKind, String)> {
        rx.try_iter()
            .map(|c| match c {
                ClientCommand::Chat { kind, text, .. } => (kind, String::from_utf8(text).unwrap()),
                other => panic!("unexpected command {other:?}"),
            })
            .collect()
    };
    let errors = |world: &bevy::prelude::World| -> Vec<String> {
        world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval("local s = ErrorSeen ErrorSeen = {} return s")
            .unwrap()
    };

    // No group: every group type refuses; a named language we lack refuses first.
    run_lent(
        &mut world,
        r#"
        SendChatMessage("l", "PARTY", "Orcish")
        SendChatMessage("p", "PARTY")
        SendChatMessage("w", "RAID_WARNING")
        SendChatMessage("r", "RAID")
        SendChatMessage("b", "BATTLEGROUND")
        "#,
    )
    .expect("lua");
    assert_eq!(
        errors(&world),
        vec![
            "You can't speak that language.",
            "You aren't in a party.",
            "You aren't in a party.",
            "You are not in a raid group",
            "You aren't in a Battleground.",
        ]
    );
    assert_eq!(sent(&rx), vec![], "no refused line reaches the wire");
    system_seen(&world);
    assert!(world.resource::<super::away::AfkMirror>().is_afk());
    assert_eq!(
        world
            .resource_mut::<bevy::prelude::Messages<crate::tutorial::TutorialEvent>>()
            .drain()
            .count(),
        0,
        "no tutorial"
    );

    // A party we lead, no raid: RAID is refused, never the 0x27 lane; PARTY takes 0x33 and
    // RAID_WARNING passes outside a raid.
    {
        let mut g = world.resource_mut::<crate::ui_party::GroupState>();
        lead_a_raid_rows(&mut g, 0x7);
        g.group_type = 0;
    }
    run_lent(&mut world, r#"SendChatMessage("r", "RAID")"#).expect("lua");
    assert_eq!(errors(&world), vec!["You are not in a raid group"]);
    assert_eq!(sent(&rx), vec![]);
    assert!(world.resource::<super::away::AfkMirror>().is_afk());
    run_lent(
        &mut world,
        r#"SendChatMessage("p", "PARTY") SendChatMessage("w", "RAID_WARNING")"#,
    )
    .expect("lua");
    assert_eq!(
        sent(&rx),
        vec![
            (ChatKind::Afk, String::new()),
            (ChatKind::PartyLeader, "p".into()),
            (ChatKind::RaidWarning, "w".into()),
        ]
    );
    assert_eq!(errors(&world), vec!["You are no longer AFK."]);

    // A raid we do not lead: RAID_WARNING needs an officer; an assistant passes.
    lead_a_raid(&mut world, 0x99);
    run_lent(&mut world, r#"SendChatMessage("w", "RAID_WARNING")"#).expect("lua");
    assert_eq!(errors(&world), vec!["You are not the party leader."]);
    assert_eq!(sent(&rx), vec![]);
    world
        .resource_mut::<crate::ui_party::GroupState>()
        .own_flags |= benilla_protocol::messages::GROUP_MEMBER_ASSISTANT;
    run_lent(
        &mut world,
        r#"SendChatMessage("w", "RAID_WARNING") SendChatMessage("r", "RAID")"#,
    )
    .expect("lua");
    assert_eq!(
        sent(&rx),
        vec![
            (ChatKind::RaidWarning, "w".into()),
            (ChatKind::Raid, "r".into()),
        ]
    );
    assert!(errors(&world).is_empty());
}

/// `SendChatMessage` reads its strings and shows its refusals on the calling VM (`0x50d170`): with
/// the `World`'s VM held outside it, as the world-entry load and a reload hold it while addons run,
/// the DND default (`0x50d44e`) and its mark (`0x50d472`) and the AFK mark (`0x6dc6e0`) still read
/// their globals, and a refusal (`0x50d340`) still shows in the call.
#[test]
fn a_send_during_the_load_reads_the_calling_vms_strings() {
    use crate::net::ChatKind;

    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(r#"ERR_NOT_IN_GROUP = "You aren't in a party.""#)
        .expect("lua");
    let (mut world, rx) = send_world(script);
    world.init_resource::<crate::sound::MessageSounds>();
    world.init_resource::<bevy::prelude::Messages<crate::tutorial::TutorialEvent>>();
    // The load: the chat add queues (`0x528cc9`) and the VM is out of the `World`.
    world.resource_mut::<super::ChatLog>().leave_world();
    let mut vm = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("VM");
    crate::ui_script::safe_world::lend_to(&mut world, &mut vm, |s| {
        s.run(
            r#"
            SendChatMessage("", "DND")
            SendChatMessage("brb", "AFK")
            SendChatMessage("p", "PARTY")
            "#,
        )
    })
    .expect("lua");
    assert_eq!(
        chat_sent(&rx),
        vec![
            (ChatKind::Dnd, "Do not Disturb".into()),
            (ChatKind::Afk, "brb".into()),
        ]
    );
    // Every line went through the chat add in its call, which queues them until the entry.
    assert!(system_seen_on(&vm).is_empty());
    world.insert_non_send_resource(vm);
    super::feed::enter_world(&mut world);
    assert_eq!(
        system_seen(&world),
        vec![
            "You are now DND: Do not Disturb.",
            "You are now AFK: brb",
            "You aren't in a party."
        ]
    );
}

/// `SendChatMessage`'s steps in 12340's order (`0x50d170`): the type lookup before the language
/// (`0x50d203`, `0x50d28b`), the leader's own lanes (`0x50d516`, `0x50d412`, `0x50d4e9`), the AFK
/// clear (`0x50d425`), then the dead-or-ghost gate (`0x50d632`-`0x50d684`), then the chatting
/// tutorial for every type that reaches it, SAY included (`0x50d695`).
#[test]
fn the_send_runs_its_steps_in_order() {
    use crate::net::{ChatKind, ClientCommand};

    let mut script = benilla_ui::script::UiScript::new().expect("VM");
    script.set_language_table(vec![(7, "Common".into())]);
    script
        .run(
            r#"
            ERR_CHAT_WHILE_DEAD = "You can't chat when you're dead!"
            ErrorSeen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UI_ERROR_MESSAGE")
            f:SetScript("OnEvent", function(_, _, text) ErrorSeen[#ErrorSeen + 1] = text end)
            "#,
        )
        .expect("lua");
    let (mut world, rx) = send_world(script);
    world.init_resource::<crate::sound::MessageSounds>();
    world.init_resource::<bevy::prelude::Messages<crate::tutorial::TutorialEvent>>();
    world.insert_resource(crate::net::SelfGuid(Some(0x7)));
    lead_a_raid(&mut world, 0x7);
    let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<(ChatKind, String)> {
        rx.try_iter()
            .map(|c| match c {
                ClientCommand::Chat { kind, text, .. } => (kind, String::from_utf8(text).unwrap()),
                other => panic!("unexpected command {other:?}"),
            })
            .collect()
    };
    let tutorials = |world: &mut bevy::prelude::World| -> usize {
        world
            .resource_mut::<bevy::prelude::Messages<crate::tutorial::TutorialEvent>>()
            .drain()
            .count()
    };

    // The type is looked up before the language, and the leader lanes are no tokens.
    for (chunk, error) in [
        (
            r#"SendChatMessage("hi", "BOGUS", "Klingon")"#,
            "Unknown chat type",
        ),
        (
            r#"SendChatMessage("hi", "RAID_LEADER")"#,
            "Unknown chat type",
        ),
        (
            r#"SendChatMessage("hi", "BATTLEGROUND_LEADER")"#,
            "Unknown chat type",
        ),
        (
            r#"SendChatMessage("hi", "SAY", "Klingon")"#,
            "SendChatMessage(): Unknown language",
        ),
        (
            r#"SendChatMessage(nil)"#,
            "Usage: SendChatMessage(text [,type] [,language] [,targetPlayer])",
        ),
    ] {
        let err = run_lent(&mut world, chunk).expect_err(chunk);
        assert!(err.to_string().contains(error), "{chunk}: {err}");
    }
    assert_eq!(sent(&rx), vec![]);

    // Alive, the leader's group lines take the leader lanes and every send flags the tutorial.
    run_lent(
        &mut world,
        r#"
        SendChatMessage("p", "party")
        SendChatMessage("r", "RAID")
        SendChatMessage("b", "BATTLEGROUND")
        SendChatMessage("s", "SAY")
        "#,
    )
    .expect("lua");
    assert_eq!(
        sent(&rx),
        vec![
            (ChatKind::PartyLeader, "p".into()),
            (ChatKind::RaidLeader, "r".into()),
            (ChatKind::BattlegroundLeader, "b".into()),
            (ChatKind::Say, "s".into()),
        ]
    );
    assert_eq!(tutorials(&mut world), 4, "SAY flags it too");

    // Dead: the AFK clear runs first, then SAY, YELL and EMOTE show the refusal and send nothing,
    // while the exempt lanes still speak.
    set_self_store(
        &mut world,
        benilla_protocol::messages::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 0u32),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
        ]),
    );
    world.resource_mut::<super::away::AfkMirror>().0 = 2;
    run_lent(
        &mut world,
        r#"
        SendChatMessage("s", "SAY")
        SendChatMessage("y", "YELL")
        SendChatMessage("e", "EMOTE")
        SendChatMessage("g", "GUILD")
        SendChatMessage("w", "WHISPER", nil, "Bob")
        "#,
    )
    .expect("lua");
    assert_eq!(
        sent(&rx),
        vec![
            (ChatKind::Afk, String::new()),
            (ChatKind::Guild, "g".into()),
            (ChatKind::Whisper, "w".into()),
        ]
    );
    assert_eq!(
        system_seen(&world),
        vec!["You are no longer AFK.".to_string()],
        "the clear precedes the gate"
    );
    let errors: Vec<String> = world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .eval("local s = ErrorSeen ErrorSeen = {} return s")
        .unwrap();
    assert_eq!(errors, vec!["You can't chat when you're dead!"; 3]);
    assert_eq!(
        tutorials(&mut world),
        2,
        "a refused line never reaches the tutorial"
    );
}

/// `SendChatMessage`'s empty-line gate inside the call: an empty line, or one whose first byte is
/// NUL, of any type but AFK and DND sends nothing and leaves a standing AFK set, since the gate is
/// ahead of the AFK clear (`0x50d229`). The type check comes first, so an unknown type is the
/// verb's Lua error (`0x50d20f`).
#[test]
fn an_empty_line_sends_nothing_but_afk_and_dnd() {
    use super::edit::{ChannelSlot, ChannelState};
    use crate::net::{ChatKind, ClientCommand};

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    world.insert_resource(super::away::AfkMirror(2));
    world.insert_resource(ChannelState {
        joined: vec![Some(ChannelSlot::joined("General - Elwynn Forest"))],
        ..Default::default()
    });
    let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<(ChatKind, String)> {
        rx.try_iter()
            .map(|c| match c {
                ClientCommand::Chat { kind, text, .. } => (kind, String::from_utf8(text).unwrap()),
                other => panic!("unexpected command {other:?}"),
            })
            .collect()
    };
    run_lent(
        &mut world,
        r#"
        SendChatMessage("")
        SendChatMessage("", "PARTY")
        SendChatMessage("", "WHISPER", nil, "Bob")
        SendChatMessage("", "CHANNEL", nil, 1)
        SendChatMessage("\0hidden", "YELL")
        "#,
    )
    .expect("lua");
    let bogus = run_lent(&mut world, r#"SendChatMessage("", "BOGUS")"#).unwrap_err();
    assert!(
        bogus
            .to_string()
            .contains("SendChatMessage(): Unknown chat type"),
        "{bogus}"
    );
    assert_eq!(sent(&rx), vec![], "no empty line reaches the wire");
    assert!(
        system_seen(&world).is_empty(),
        "no AFK clear; the type check precedes the gate"
    );
    assert!(world.resource::<super::away::AfkMirror>().is_afk());

    // A line with text clears the standing AFK and says itself; an empty AFK marks it again.
    run_lent(
        &mut world,
        r#"SendChatMessage("hi") SendChatMessage("", "AFK")"#,
    )
    .expect("lua");
    assert_eq!(
        sent(&rx),
        vec![
            (ChatKind::Afk, String::new()),
            (ChatKind::Say, "hi".into()),
            (ChatKind::Afk, "Away from Keyboard".into()),
        ]
    );
}

/// The server moves a zone channel by joining its new name: the slot holding that ChannelID takes
/// the name and the notice reads `YOU_CHANGED`. A zone channel it joins unasked finds no slot, so it
/// claims none and sets no `ZONECHANNELS` bit (`0x507880`).
#[test]
fn a_server_join_renames_the_zone_slot_by_id_and_an_unasked_one_takes_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_vm();
    let mut windows = super::frames::ChatWindows::default();
    let row = |id, flags, pattern: &str| benilla_formats::ChatChannelRow {
        id,
        flags,
        pattern: pattern.into(),
        shortcut: pattern.split(' ').next().unwrap_or_default().into(),
    };
    let mut channels = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            row(1, 0x8_0003, "General - %s"),
            row(26, 0x4_0039, "LookingForGroup"),
        ]),
        zone_mask: Some(1),
        ..Default::default()
    };
    channels.claim_slot("General - Durotar");
    let you_joined = |name: &str| {
        let mut e = ChatEvent::text_only(K::ChannelNotice, String::new());
        e.channel = name.into();
        e.notice = "2".into();
        e
    };

    let mut moved = you_joined("General - Orgrimmar");
    super::feed::deliver(&mut s, &mut windows, &mut channels, &mut moved);
    assert_eq!(channels.names(), vec![Some("General - Orgrimmar".into())]);
    assert_eq!(moved.slot_state, Some(super::edit::SlotState::Renamed));
    assert_eq!(
        channels.slot_state("General - Orgrimmar"),
        Some(super::edit::SlotState::Joined)
    );

    super::feed::deliver(
        &mut s,
        &mut windows,
        &mut channels,
        &mut you_joined("LookingForGroup"),
    );
    assert_eq!(channels.number_of("LookingForGroup"), None);
    assert_eq!(channels.zone_mask, Some(1), "no LookingForGroup bit");
}

/// The join and the leave take a channel's ChannelID only from their verb's Shortcut match
/// (`0x507baa`, `0x509502`); a custom channel has id 0 and its record is found by name
/// (`0x507c0a`, `0x50959f`), never by a row its name contains. The leave sends and clears the
/// record's own ChannelID (`0x5095f1`, `0x509618`-`0x509639`).
#[test]
fn a_custom_channel_is_found_by_name_never_by_a_row_its_name_contains() {
    use super::edit::JoinRecord;
    let mut c = super::edit::ChannelState {
        channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
            benilla_formats::ChatChannelRow {
                id: 26,
                flags: 0x4_0039,
                pattern: "LookingForGroup".into(),
                shortcut: "LookingForGroup".into(),
            },
        ]),
        zone_mask: Some(1 << 25),
        ..Default::default()
    };
    assert_eq!(c.claim_walk_slot("LookingForGroup", 26), Some(1));
    assert_eq!(
        c.claim_for_join("LookingForGroupDK", 0, 0),
        JoinRecord::Claimed(2),
        "/join LookingForGroupDK claims its own record"
    );
    assert_eq!(c.slot(2).map(|s| s.id), Some(0));
    assert_eq!(
        c.claim_for_join("LookingForGroup", 26, 0),
        JoinRecord::Held,
        "the Shortcut's join finds the row's record by its id"
    );
    c.confirm_slot("LookingForGroupDK");
    assert_eq!(c.leave_record("LookingForGroupDK", 0), Some(2));
    assert_eq!(c.leave_record("LookingForGroup", 26), Some(1));
    assert!(matches!(
        c.leave_command("LookingForGroupDK".into()),
        crate::net::ClientCommand::LeaveChannel { channel_id: 0, ref name }
            if name == "LookingForGroupDK"
    ));
    c.note_zone_channel_left("LookingForGroupDK");
    assert_eq!(c.zone_mask, Some(1 << 25), "the custom leave keeps bit 26");
    assert!(matches!(
        c.leave_command("LookingForGroup".into()),
        crate::net::ClientCommand::LeaveChannel { channel_id: 26, .. }
    ));
    c.note_zone_channel_left("LookingForGroup");
    assert_eq!(c.zone_mask, Some(0));
}

/// A channel verb sends inside its call (`0x50daa0` -> `0x507a80`): the join is on the wire before
/// the verb returns, the zone row's slot claimed at the send.
#[test]
fn a_channel_join_sends_inside_its_call() {
    use crate::net::ClientCommand;

    let (mut world, rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    run_lent(&mut world, r#"JoinChannelByName("Charz")"#).expect("lua");
    assert!(
        rx.try_iter()
            .any(|c| matches!(c, ClientCommand::JoinChannel { ref name, .. } if name == "Charz")),
        "the join went out in the call"
    );
    assert!(
        world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .share()
            .take_channel_commands()
            .is_empty(),
        "nothing waits for a drain"
    );
}

/// `ListChannels()` (`0x50dae0`) adds one `CHAT_MSG_CHANNEL_LIST` line in the call: each joined
/// record as `[%d. %s] ` (`0x50db31`), a hole skipped, and an empty line with none (`0x50dbf2`).
#[test]
fn list_channels_adds_its_packed_line_in_the_call() {
    use super::edit::{ChannelSlot, ChannelState};

    let (mut world, _rx) = send_world(benilla_ui::script::UiScript::new().expect("VM"));
    world
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run(
            r#"ListSeen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_CHANNEL_LIST")
            f:SetScript("OnEvent", function(_, _, text) ListSeen[#ListSeen + 1] = text end)"#,
        )
        .unwrap();
    run_lent(&mut world, "ListChannels() AfterEmpty = #ListSeen").expect("lua");
    world.insert_resource(ChannelState {
        joined: vec![
            Some(ChannelSlot::joined("General - Elwynn Forest")),
            None,
            Some(ChannelSlot::joined("Charz")),
        ],
        ..Default::default()
    });
    run_lent(&mut world, "ListChannels()").expect("lua");
    let script = world.non_send_resource::<benilla_ui::script::UiScript>();
    assert_eq!(script.eval::<i64>("return AfterEmpty").unwrap(), 1);
    assert_eq!(
        script.eval::<Vec<String>>("return ListSeen").unwrap(),
        ["", "[1. General - Elwynn Forest] [3. Charz] "]
    );
}
