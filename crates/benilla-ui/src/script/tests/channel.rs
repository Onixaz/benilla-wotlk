//! The joined-channel verbs `GetChannelName` and `GetChannelList` ([`crate::script::channel`]),
//! the guild-recruitment latch, and the channel pane ([`crate::script::channel_pane`]).

use super::common::script;
use crate::script::{
    ChannelCommand, ChannelSlotInfo, PartyMemberInfo, PartyState, RaidMemberInfo, UnitGuids,
};

/// Two channels in join order, which is the numbering; deliberately not sorted.
fn joined() -> crate::script::UiScript {
    let mut s = script();
    s.set_joined_channels(vec![Some("World".into()), Some("Trade - City".into())]);
    s
}

/// The slot is the 1-based position in join order, not a DBC id or an alphabetical rank.
#[test]
fn a_joined_channel_answers_its_slot_name_and_instance() {
    let s = joined();
    let (id, name, instance): (i64, String, i64) = s.eval("return GetChannelName(2)").unwrap();
    assert_eq!(id, 2, "the 1-based slot in join order");
    assert_eq!(name, "Trade - City");
    assert_eq!(
        instance, 0,
        "instanceID is 0 on every vanilla emulator, and a NUMBER so a caller can compare it"
    );
}

/// Addons look up by name in any case: `_LazyPig` passes `"world"` for the server's `"World"`.
#[test]
fn the_name_form_resolves_case_insensitively_to_the_same_slot() {
    let s = joined();
    let by_name: i64 = s.eval("return GetChannelName('world')").unwrap();
    let by_index: i64 = s.eval("return GetChannelName(1)").unwrap();
    assert_eq!(by_name, 1);
    assert_eq!(by_name, by_index, "both directions must agree on the slot");

    let name: String = s
        .eval("local _, n = GetChannelName('TRADE - CITY') return n")
        .unwrap();
    assert_eq!(
        name, "Trade - City",
        "the answer is the JOINED spelling, not the caller's"
    );
}

/// Stock callers compare the first return numerically (`ChatFrame.lua:2114`, `:2232`).
#[test]
fn a_channel_that_is_not_joined_answers_the_number_zero_never_nil() {
    let s = joined();

    let id: i64 = s.eval("return GetChannelName('NoSuchChannel')").unwrap();
    assert_eq!(id, 0);

    let guard: bool = s
        .eval("local id = GetChannelName('NoSuchChannel') return id > 0")
        .unwrap();
    assert!(
        !guard,
        "the reference's `if ( channelNum > 0 )` must run and be false"
    );

    let out_of_range: bool = s
        .eval("local id = GetChannelName(99) return id > 0")
        .unwrap();
    assert!(!out_of_range, "an out-of-range index takes the same branch");

    let zero: bool = s
        .eval("local id = GetChannelName(0) return id > 0")
        .unwrap();
    assert!(
        !zero,
        "the client bounds-checks 1 <= n <= count, so 0 is not a slot"
    );
}

/// `ChatFrame.lua:2113` passes the `"1"` it cuts out of `/1`, which Lua coerces to a number.
#[test]
fn a_numeric_string_resolves_as_an_index_not_as_a_name() {
    let s = joined();
    let id: i64 = s.eval("return GetChannelName('2')").unwrap();
    assert_eq!(id, 2, "the `/2` slash-command path depends on this");
}

/// `ui_chat::feed` appends on the server's YOU_JOINED notice, never on the request.
#[test]
fn nothing_is_joined_before_the_server_confirms_it() {
    let s = script();
    let id: i64 = s.eval("return GetChannelName('World')").unwrap();
    assert_eq!(id, 0);
}

/// The shape its consumers pin: `FCFDropDown_LoadChannels` walks `for i=1, arg.n, 2` reading
/// `arg[i+1]` as the name, and `ChatLog.lua:424` packs it and spots an id by `type == "number"`.
#[test]
fn get_channel_list_is_a_flat_slot_name_vararg_in_join_order() {
    let s = joined();

    // Two pairs for two joined channels.
    assert_eq!(s.arity("GetChannelList()").unwrap(), 4);

    // Slot 1 is the first joined; alphabetically "Trade - City" would come first.
    let (s1, n1, s2, n2) = s
        .eval::<(i64, String, i64, String)>("return GetChannelList()")
        .unwrap();
    assert_eq!((s1, n1.as_str()), (1, "World"));
    assert_eq!((s2, n2.as_str()), (2, "Trade - City"));

    assert!(s
        .eval::<bool>(
            "local t = { GetChannelList() } \
             return table.getn(t) == 4 and type(t[1]) == 'number' and type(t[2]) == 'string'"
        )
        .unwrap());

    assert!(s
        .eval::<bool>("local i = GetChannelName('Trade - City') return i == 2")
        .unwrap());

    let empty = crate::script::tests::common::script();
    assert_eq!(empty.arity("GetChannelList()").unwrap(), 0);
}

/// `GetGuildRecruitmentMode` (`0x4a0040`) always pushes its int global as a number; it boots at 1
/// from a `.data` initialiser (`raw 0x443608`), the value `UIOptionsFrame_SetDefaults` sets.
#[test]
fn the_guild_recruitment_mode_boots_auto_and_answers_a_number() {
    let s = script();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        1.0,
        "the reference's own .data initialiser, not a BSS zero"
    );
    assert!(s
        .eval::<bool>("return type(GetGuildRecruitmentMode()) == 'number'")
        .unwrap());
    s.run("SetGuildRecruitmentMode(0)").unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );
}

/// `0x4a0060` raises `Usage:` unless `lua_isnumber` (`0x6f34d0`), truncates through `__ftol`
/// (`0x40a2b0`), and raises `invalid mode` outside `0 <= mode < 2`; success pushes no values.
#[test]
fn the_guild_recruitment_setter_gates_its_argument_the_way_the_reference_does() {
    let s = script();

    s.run(r#"SetGuildRecruitmentMode("0")"#).unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );

    // 1.7 truncates to a legal 1.
    s.run("SetGuildRecruitmentMode(1.7)").unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        1.0
    );

    for bad in ["", "nil", r#""AUTO""#, "{}", "true"] {
        let e = s
            .run(&format!("SetGuildRecruitmentMode({bad})"))
            .expect_err(&format!("SetGuildRecruitmentMode({bad}) must raise"));
        assert!(
            e.to_string().contains("Usage: SetGuildRecruitmentMode"),
            "{bad}: {e}"
        );
    }
    for bad in ["-1", "2", "-1.7", "37"] {
        let e = s
            .run(&format!("SetGuildRecruitmentMode({bad})"))
            .expect_err(&format!("SetGuildRecruitmentMode({bad}) must raise"));
        assert!(e.to_string().contains("invalid mode"), "{bad}: {e}");
    }

    // None of the refused calls moved the latch.
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        1.0
    );
    assert_eq!(
        s.arity("SetGuildRecruitmentMode(0)").unwrap(),
        0,
        "0x4a0060 returns `xor eax,eax` — zero values, not a nil"
    );
}

/// `0x49ea70` stores the latch and jumps into the cascade `0x49ea90` whenever the new value is 1,
/// moved or not; `0x4a00a4`/`0x4a00a9` fire `UPDATE_CHAT_WINDOWS` on every successful call.
#[test]
fn the_setter_asks_for_the_cascade_on_one_and_fires_update_chat_windows() {
    let mut s = script();
    s.run(
        r#"
        n = 0
        local f = CreateFrame("Frame", "GRF")
        f:RegisterEvent("UPDATE_CHAT_WINDOWS")
        f:SetScript("OnEvent", function() n = n + 1 end)
    "#,
    )
    .unwrap();
    assert!(!s.take_guild_recruitment_cascade(), "nothing asked yet");

    // Boots at 1; a Set(1) that moves nothing still asks for the cascade.
    s.run("SetGuildRecruitmentMode(1)").unwrap();
    assert!(s.take_guild_recruitment_cascade());
    assert!(!s.take_guild_recruitment_cascade(), "drained");
    assert!(
        !s.take_guild_recruitment_change(),
        "…and the file is not dirtied by a no-move"
    );

    s.run("SetGuildRecruitmentMode(0)").unwrap();
    assert!(
        !s.take_guild_recruitment_cascade(),
        "mode 0 is the latch alone"
    );
    assert!(
        s.take_guild_recruitment_change(),
        "the file is dirtied by the move"
    );

    // A refused call fires nothing.
    s.run("SetGuildRecruitmentMode(2)").unwrap_err();
    s.tick(0.016);
    assert_eq!(
        s.eval::<i64>("return n").unwrap(),
        2,
        "one UPDATE_CHAT_WINDOWS per successful call: Set(1), Set(0); the raise fired none"
    );
}

/// A manual join or leave of `GuildRecruitment` calls `0x49ea70(0)` (`0x49ed3d`, `0x49ef8f`): it
/// dirties the file and, as mode 0, asks for no cascade.
#[test]
fn a_manual_guild_recruitment_verb_resets_the_latch() {
    let mut s = script();
    assert!(s.reset_guild_recruitment_mode(), "1 → 0 moved");
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );
    assert!(s.take_guild_recruitment_change());
    assert!(!s.take_guild_recruitment_cascade());
    assert!(
        !s.reset_guild_recruitment_mode(),
        "already 0: nothing moved"
    );
    assert!(!s.take_guild_recruitment_change());
}

// ── The channel pane ─────────────────────────────────────────────────────────

const GENERAL: &str = "General - Elwynn Forest";
const OWN: u64 = 0x10;

fn slot(name: &str, shortcut: Option<&str>, active: bool) -> Option<ChannelSlotInfo> {
    Some(ChannelSlotInfo {
        name: name.into(),
        shortcut: shortcut.map(Into::into),
        active,
    })
}

/// General (a live world channel), a custom channel and a suspended Trade in slot order, with the
/// texts the pane reads seated and the player's guid; the pane's events collect in `E`.
fn pane() -> crate::script::UiScript {
    let mut s = script();
    s.run(
        "CHANNEL_CATEGORY_GROUP = 'cat-group' CHANNEL_CATEGORY_WORLD = 'cat-world' \
         CHANNEL_CATEGORY_CUSTOM = 'cat-custom' CHAT_MSG_PARTY = 'grp-party' \
         CHAT_MSG_RAID = 'grp-raid' CHAT_MSG_BATTLEGROUND = 'grp-bg' UNKNOWN = 'nobody' \
         E = {} \
         local f = CreateFrame('Frame') \
         for _, e in ipairs({'CHANNEL_UI_UPDATE', 'CHANNEL_COUNT_UPDATE', \
             'CHANNEL_ROSTER_UPDATE', 'CHANNEL_FLAGS_UPDATED', 'CHANNEL_VOICE_UPDATE'}) do \
             f:RegisterEvent(e) end \
         f:SetScript('OnEvent', function(self, event, a, b) \
             table.insert(E, event .. ':' .. tostring(a) .. ':' .. tostring(b)) end)",
    )
    .unwrap();
    s.set_unit_guids(&UnitGuids {
        player: OWN,
        ..Default::default()
    });
    s.set_channel_slots(vec![
        slot(GENERAL, Some("General"), true),
        slot("MyChan", None, true),
        slot("Trade - City", Some("Trade"), false),
    ]);
    s
}

/// The events fired since the last call, in order.
fn events(s: &crate::script::UiScript) -> Vec<String> {
    s.eval::<Vec<String>>("local e = E E = {} return e")
        .unwrap()
}

/// `RebuildDisplayChannelList` (`0x504840`): the group header, the world header over the
/// `ChatChannels.dbc` slots, the custom header over the rest, each channel in slot order;
/// `GetChannelDisplayInfo` (`0x4ff580`) answers nine values per entry and none past the list.
#[test]
fn the_display_list_puts_world_then_custom_channels_under_their_headers() {
    let mut s = pane();
    assert_eq!(
        events(&s),
        vec!["CHANNEL_UI_UPDATE:nil:nil"],
        "a slot push relists, as a join or leave does (`0x505d40`)"
    );
    assert_eq!(s.eval::<i64>("return GetNumDisplayChannels()").unwrap(), 6);
    s.run(
        "local n, h, c, num, count, act, cat, v, va = GetChannelDisplayInfo(1) \
         assert(n == 'cat-group' and h == 1 and c == nil and num == nil and count == nil, 'group') \
         assert(act == nil and cat == nil and v == nil and va == nil, 'group tail') \
         n, h, c, num, count = GetChannelDisplayInfo(2) \
         assert(n == 'cat-world' and h == 1 and count == 2, 'world counts General and Trade') \
         n, h, c, num, count, act, cat, v, va = GetChannelDisplayInfo(3) \
         assert(n == 'General' and h == nil and num == 1 and count == nil and act == 1, 'General') \
         assert(cat == 'CHANNEL_CATEGORY_WORLD' and v == nil and va == nil, 'General tail') \
         n, h, c, num, count, act, cat = GetChannelDisplayInfo(4) \
         assert(n == 'Trade' and num == 3 and act == nil, 'Trade lists grey') \
         assert(cat == 'CHANNEL_CATEGORY_WORLD', 'Trade category') \
         n, h, c, num, count = GetChannelDisplayInfo(5) \
         assert(n == 'cat-custom' and count == 1, 'custom') \
         n, h, c, num, count, act, cat = GetChannelDisplayInfo(6) \
         assert(n == 'MyChan' and num == 2 and act == 1, 'MyChan') \
         assert(cat == 'CHANNEL_CATEGORY_CUSTOM', 'MyChan category')",
    )
    .unwrap();
    assert_eq!(s.arity("GetChannelDisplayInfo(1)").unwrap(), 9);
    assert_eq!(
        s.arity("GetChannelDisplayInfo(7)").unwrap(),
        0,
        "past the list"
    );
    assert_eq!(s.arity("GetChannelDisplayInfo(0)").unwrap(), 0);
    let e = s.run("GetChannelDisplayInfo('x')").unwrap_err();
    assert!(
        e.to_string()
            .contains("Usage: GetChannelDisplayInfo(index)"),
        "{e}"
    );
    // The names-only mirror lists every slot as a live custom channel.
    s.set_joined_channels(vec![Some("World".into())]);
    s.run(
        "assert(GetNumDisplayChannels() == 4) \
         local n, _, _, num, _, act, cat = GetChannelDisplayInfo(4) \
         assert(n == 'World' and num == 1 and act == 1 and cat == 'CHANNEL_CATEGORY_CUSTOM')",
    )
    .unwrap();
}

/// `CollapseChannelHeader` (`0x5052e0`) and `ExpandChannelHeader` (`0x505360`) flip a header's
/// flag and relist with `CHANNEL_UI_UPDATE` inside the call; a collapsed header keeps its count.
#[test]
fn a_header_collapses_and_expands_with_a_ui_update() {
    let s = pane();
    events(&s);
    s.run("CollapseChannelHeader(2)").unwrap();
    assert_eq!(events(&s), vec!["CHANNEL_UI_UPDATE:nil:nil"]);
    assert_eq!(s.eval::<i64>("return GetNumDisplayChannels()").unwrap(), 4);
    s.run(
        "local n, h, c, num, count = GetChannelDisplayInfo(2) \
         assert(c == 1 and count == 2, 'collapsed, still counting') \
         assert(GetChannelDisplayInfo(3) == 'cat-custom')",
    )
    .unwrap();
    s.run("CollapseChannelHeader(4) CollapseChannelHeader(9) CollapseChannelHeader('x')")
        .unwrap();
    assert!(
        events(&s).is_empty(),
        "a channel entry, past the list and a non-number do nothing"
    );
    s.run("ExpandChannelHeader(2)").unwrap();
    assert_eq!(events(&s).len(), 1);
    assert_eq!(s.eval::<i64>("return GetNumDisplayChannels()").unwrap(), 6);
    assert_eq!(s.arity("CollapseChannelHeader(1)").unwrap(), 0);
}

/// `SetSelectedDisplayChannel` (`0x504e50`) watches the channel and asks its list
/// (`CMSG_SET_CHANNEL_WATCH`, `CMSG_CHANNEL_DISPLAY_LIST`); the display reply (`0x503990`) fills
/// the roster once every name is in (`0x4ffe30`), sorted by name (`0x4fb0f0`).
#[test]
fn a_selected_channel_is_watched_and_its_display_list_fills_the_roster() {
    let mut s = pane();
    assert!(
        s.eval::<Option<i64>>("return GetSelectedDisplayChannel()")
            .unwrap()
            .is_none(),
        "nil until a selection"
    );
    s.run("SetSelectedDisplayChannel(3)").unwrap();
    assert_eq!(
        s.take_channel_commands(),
        vec![
            ChannelCommand::Watch {
                name: GENERAL.into()
            },
            ChannelCommand::DisplayList {
                name: GENERAL.into()
            },
        ]
    );
    assert_eq!(
        s.eval::<i64>("return GetSelectedDisplayChannel()").unwrap(),
        3
    );
    s.run(
        "SetSelectedDisplayChannel(3) SetSelectedDisplayChannel(2) \
         SetSelectedDisplayChannel('x')",
    )
    .unwrap();
    assert!(
        s.take_channel_commands().is_empty(),
        "the current one, a header and a non-number ask nothing"
    );
    assert!(
        s.eval::<Option<i64>>("return GetNumChannelMembers(3)")
            .unwrap()
            .is_none(),
        "no list yet"
    );
    events(&s);

    // Only the selected channel's list is read, in any case.
    s.display_channel_list("MyChan", 0, &[(0x99, 0)], |_| Some(Some("x".into())));
    assert!(events(&s).is_empty());
    s.display_channel_list(
        "general - elwynn forest",
        0x18,
        &[(OWN, 0x01), (0x20, 0x00), (0x30, 0x02)],
        |g| match g {
            OWN => Some(Some("Me".into())),
            0x20 => Some(Some("bob".into())),
            _ => None,
        },
    );
    assert_eq!(
        events(&s),
        vec!["CHANNEL_FLAGS_UPDATED:3:nil", "CHANNEL_FLAGS_UPDATED:3:nil"],
        "the channel's flags moved, then the player's own row (`0x503a7f`, `0x503bc6`)"
    );
    assert_eq!(s.channel_roster_unnamed(), vec![0x30]);
    assert!(
        s.eval::<Option<i64>>("return GetNumChannelMembers(3)")
            .unwrap()
            .is_none(),
        "a name is still asked"
    );
    assert_eq!(s.arity("GetChannelRosterInfo(3, 1)").unwrap(), 0);

    s.channel_roster_named(0x30, Some("Ann".into()));
    assert_eq!(
        events(&s),
        vec!["CHANNEL_COUNT_UPDATE:3:3", "CHANNEL_ROSTER_UPDATE:3:3"]
    );
    assert!(s.channel_roster_unnamed().is_empty());
    assert_eq!(s.eval::<i64>("return GetNumChannelMembers(3)").unwrap(), 3);
    assert_eq!(s.arity("GetChannelRosterInfo(3, 1)").unwrap(), 6);
    s.run(
        "local n, o, m, mu, a, v = GetChannelRosterInfo(3, 1) \
         assert(n == 'Ann' and o == nil and m == 1 and mu == nil and a == nil and v == nil, 'Ann') \
         assert(GetChannelRosterInfo(3, 2) == 'bob', 'case-folded') \
         n, o = GetChannelRosterInfo(3, 3) assert(n == 'Me' and o == 1, 'Me') \
         assert(select('#', GetChannelRosterInfo(3, 4)) == 0, 'past the roster') \
         local _, _, _, _, count = GetChannelDisplayInfo(3) assert(count == 3, 'listed size') \
         assert(IsDisplayChannelOwner() == 1 and IsDisplayChannelModerator() == 1, 'own row')",
    )
    .unwrap();
    // A rebuild zeroes the listed size (`0x504854`); the roster stays.
    s.run(
        "GetNumDisplayChannels() \
         local _, _, _, _, count = GetChannelDisplayInfo(3) assert(count == nil)",
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumChannelMembers(3)").unwrap(), 3);
}

/// A name the cache has no record for reads `UNKNOWN` (`0x50505a`); a selected channel that leaves
/// the slots is unselected (`0x507900`).
#[test]
fn an_unnamed_member_reads_unknown_and_a_leave_unselects() {
    let mut s = pane();
    s.run("SetSelectedDisplayChannel(6)").unwrap();
    s.display_channel_list("MyChan", 0, &[(0x40, 0)], |_| Some(None));
    s.run("assert(GetChannelRosterInfo(6, 1) == 'nobody')")
        .unwrap();
    assert!(
        s.eval::<Option<i64>>("return IsDisplayChannelOwner()")
            .unwrap()
            .is_none(),
        "the player has no row"
    );
    s.set_channel_slots(vec![
        slot(GENERAL, Some("General"), true),
        None,
        slot("Trade - City", Some("Trade"), false),
    ]);
    assert!(s
        .eval::<Option<i64>>("return GetSelectedDisplayChannel()")
        .unwrap()
        .is_none());
    assert_eq!(s.eval::<i64>("return GetNumDisplayChannels()").unwrap(), 5);
}

/// `BuildPartyDisplayList` (`0x5045c0`): the party lists under the group header, and selecting it
/// builds the player and the party at once, the leader as owner, with nothing on the wire.
#[test]
fn a_party_lists_under_the_group_header_and_builds_its_roster_at_once() {
    let mut s = pane();
    s.set_player_record(crate::script::PlayerRecord {
        name: "Me".into(),
        ..Default::default()
    });
    s.set_party(PartyState {
        members: vec![
            PartyMemberInfo {
                name: "Zed".into(),
                guid: 0x50,
            },
            PartyMemberInfo {
                name: "amy".into(),
                guid: 0x60,
            },
        ],
        leader_guid: 0x50,
        ..Default::default()
    });
    assert_eq!(s.eval::<i64>("return GetNumDisplayChannels()").unwrap(), 7);
    s.run(
        "local n, h, c, num, count, act, cat = GetChannelDisplayInfo(2) \
         assert(n == 'grp-party' and h == nil and num == nil and count == 3 and act == 1, 'party') \
         assert(cat == 'CHANNEL_CATEGORY_GROUP', 'its category token') \
         local _, _, _, _, gc = GetChannelDisplayInfo(1) assert(gc == 1, 'one session')",
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumChannelMembers(2)").unwrap(), 3);
    assert!(s.take_channel_commands().is_empty(), "a group asks nothing");
    s.run(
        "local n, o = GetChannelRosterInfo(2, 1) assert(n == 'amy' and o == nil, 'amy') \
         assert(GetChannelRosterInfo(2, 2) == 'Me', 'Me') \
         n, o = GetChannelRosterInfo(2, 3) assert(n == 'Zed' and o == 1, 'the leader owns') \
         assert(GetSelectedDisplayChannel() == 2 and IsDisplayChannelOwner() == nil, 'selection')",
    )
    .unwrap();
}

/// `BuildRaidDisplayList` (`0x504720`): the raid's entry wins over the party's, ranks map to owner
/// and moderator; in a battleground (`0x572f50`) the raid lists as the battleground.
#[test]
fn a_raid_lists_its_entry_and_a_battleground_raid_the_battleground() {
    let mut s = pane();
    let member = |name: &str, guid: u64, rank: u32| RaidMemberInfo {
        name: name.into(),
        guid,
        rank,
        ..Default::default()
    };
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Zed".into(),
            guid: 0x50,
        }],
        raid: vec![
            member("Me", OWN, 2),
            member("Zed", 0x50, 1),
            member("amy", 0x60, 0),
        ],
        ..Default::default()
    });
    s.run(
        "assert(GetNumDisplayChannels() == 7, 'one group entry') \
         local n, _, _, _, count = GetChannelDisplayInfo(2) \
         assert(n == 'grp-raid' and count == 3, 'the raid')",
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return GetNumChannelMembers(2)").unwrap(), 3);
    s.run(
        "local n, o, m = GetChannelRosterInfo(2, 3) \
         assert(n == 'Zed' and o == nil and m == 1, 'an assistant moderates') \
         n, o, m = GetChannelRosterInfo(2, 2) assert(n == 'Me' and o == 1 and m == 1, 'leader') \
         assert(IsDisplayChannelOwner() == 1, 'the player leads')",
    )
    .unwrap();
    s.set_instance_type(Some(3));
    s.run(
        "assert(GetNumDisplayChannels() == 8, 'party and battleground') \
         local n, _, _, _, count = GetChannelDisplayInfo(2) \
         assert(n == 'grp-party' and count == 2, 'the party') \
         n, _, _, _, count = GetChannelDisplayInfo(3) \
         assert(n == 'grp-bg' and count == 3, 'the battleground') \
         local _, _, _, _, gc = GetChannelDisplayInfo(1) assert(gc == 2, 'two sessions') \
         assert(GetSelectedDisplayChannel() == nil, 'the raid entry is gone')",
    )
    .unwrap();
}

/// `SetActiveVoiceChannel` (`0x500ae0`): no entry carries voice, so an index opens nothing; 0
/// leaves the active entry, the first, signalling it.
#[test]
fn set_active_voice_channel_zero_signals_the_first_entry() {
    let s = pane();
    events(&s);
    s.run("SetActiveVoiceChannel(3) SetActiveVoiceChannel('x')")
        .unwrap();
    assert!(events(&s).is_empty());
    s.run("SetActiveVoiceChannel(0.5)").unwrap();
    assert_eq!(events(&s), vec!["CHANNEL_VOICE_UPDATE:1:nil"]);
    assert_eq!(s.arity("SetActiveVoiceChannel(0)").unwrap(), 0);
    s.run(
        "local _, _, _, _, _, _, _, v, va = GetChannelDisplayInfo(3) \
         assert(v == nil and va == nil)",
    )
    .unwrap();
}
