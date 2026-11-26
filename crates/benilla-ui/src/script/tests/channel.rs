//! The joined-channel verbs `GetChannelName` and `GetChannelList` ([`crate::script::channel`]),
//! the guild-recruitment latch, and the channel pane ([`crate::script::channel_pane`]).

use super::common::script;
use crate::script::live::{test_view, TestView};
use crate::script::{
    ChannelCommand, ChannelSlotInfo, PartyMemberInfo, PartyState, RaidMemberInfo, UnitGuids,
};

/// Two channels in join order, which is the numbering; deliberately not sorted.
fn joined() -> TestView {
    TestView {
        channel_slots: vec![slot("World", None, true), slot("Trade - City", None, true)],
        ..Default::default()
    }
}

/// The slot is the 1-based position in join order, not a DBC id or an alphabetical rank.
#[test]
fn a_joined_channel_answers_its_slot_name_and_instance() {
    let mut s = script();
    let mut v = joined();
    s.with_live(&mut v, |s| {
        let (id, name, instance): (i64, String, i64) = s.eval("return GetChannelName(2)").unwrap();
        assert_eq!(id, 2, "the 1-based slot in join order");
        assert_eq!(name, "Trade - City");
        assert_eq!(
            instance, 0,
            "instanceID is 0 on every vanilla emulator, and a NUMBER so a caller can compare it"
        );
    });
}

/// Addons look up by name in any case: `_LazyPig` passes `"world"` for the server's `"World"`.
#[test]
fn the_name_form_resolves_case_insensitively_to_the_same_slot() {
    let mut s = script();
    let mut v = joined();
    s.with_live(&mut v, |s| {
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
    });
}

/// Stock callers compare the first return numerically (`ChatFrame.lua:2114`, `:2232`).
#[test]
fn a_channel_that_is_not_joined_answers_the_number_zero_never_nil() {
    let mut s = script();
    let mut v = joined();
    s.with_live(&mut v, |s| {
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
    });
}

/// `ChatFrame.lua:2113` passes the `"1"` it cuts out of `/1`, which Lua coerces to a number.
#[test]
fn a_numeric_string_resolves_as_an_index_not_as_a_name() {
    let mut s = script();
    let mut v = joined();
    s.with_live(&mut v, |s| {
        let id: i64 = s.eval("return GetChannelName('2')").unwrap();
        assert_eq!(id, 2, "the `/2` slash-command path depends on this");
    });
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
    let mut s = script();
    let mut v = joined();
    s.with_live(&mut v, |s| {
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
    });
}

/// With no records at the call, nothing is listed.
/// The number legs read only a record in state 0 (`0x4fe8a3`, `0x4fe681`): a left or suspended one
/// answers (0, nil, 0) by number and is not listed, while its name still finds it (`0x4fe8e4`). A
/// zone channel lists by its row's Shortcut (`0x4fe6cf`), a custom one by its name; the third value
/// is the record's `+0x98`; a table is the usage error (`0x4fe938`).
#[test]
fn the_number_legs_read_only_joined_records_and_list_zone_channels_by_shortcut() {
    let mut s = script();
    let mut trade = slot("Trade - City", Some("Trade"), false);
    trade.as_mut().unwrap().instance = 4;
    let mut general = slot("General - Elwynn Forest", Some("General"), true);
    general.as_mut().unwrap().instance = 7;
    let mut v = TestView {
        channel_slots: vec![general, slot("MyChan", None, true), trade],
        ..Default::default()
    };
    s.with_live(&mut v, |s| {
        s.run(
            "local a, b, c = GetChannelName(3)              assert(a == 0 and b == nil and c == 0, 'a suspended record by number')              a, b, c = GetChannelName('3')              assert(a == 0 and b == nil and c == 0, 'by numeric string')              a, b, c = GetChannelName('trade - city')              assert(a == 3 and b == 'Trade - City' and c == 4, 'by name, whatever the state')              a, b, c = GetChannelName(1.9)              assert(a == 1 and b == 'General - Elwynn Forest' and c == 7, 'truncated number')              local l = {GetChannelList()}              assert(#l == 4, 'two joined records: ' .. #l)              assert(l[1] == 1 and l[2] == 'General', 'a zone channel by its Shortcut')              assert(l[3] == 2 and l[4] == 'MyChan', 'a custom channel by its name')",
        )
        .unwrap();
        let err = s.run("GetChannelName({})").unwrap_err();
        assert!(
            err.to_string()
                .contains("Usage: GetChannelName([channelIndex] or [channelName])"),
            "{err}"
        );
    });
}

#[test]
fn get_channel_list_is_empty_with_no_records() {
    let empty = crate::script::tests::common::script();
    assert_eq!(empty.arity("GetChannelList()").unwrap(), 0);
}

/// 3.3.5a has no `Get`/`SetGuildRecruitmentMode` verb (the `guildRecruitmentChannel` CVar drives
/// the latch); the host setter moves the latch once per change and arms the chat cache's save.
#[test]
fn the_guild_recruitment_latch_moves_from_the_cvar_alone() {
    let mut s = script();
    assert!(s
        .eval::<bool>("return GetGuildRecruitmentMode == nil and SetGuildRecruitmentMode == nil")
        .unwrap());
    assert_eq!(s.guild_recruitment_mode(), 1, "boots AUTO");
    assert!(s.set_guild_recruitment_mode_from_cvar(0));
    assert!(s.take_guild_recruitment_change(), "a move arms the save");
    assert!(
        !s.set_guild_recruitment_mode_from_cvar(0),
        "unchanged: no move"
    );
    assert!(!s.take_guild_recruitment_change());
    assert_eq!(s.guild_recruitment_mode(), 0);
}

/// A manual join of `GuildRecruitment` drops the latch to 0 (the recruitment-mode set with
/// `(0, true)`, `0x508260`): it dirties the file and leaves nothing.
#[test]
fn a_manual_guild_recruitment_verb_resets_the_latch() {
    let mut s = script();
    assert!(s.reset_guild_recruitment_mode(), "1 → 0 moved");
    assert_eq!(s.guild_recruitment_mode(), 0);
    assert!(s.take_guild_recruitment_change());
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
        flags: 0,
        instance: 0,
    })
}

/// General (a live world channel), a custom channel and a suspended Trade in slot order, with the
/// texts the pane reads seated and the player's guid; the pane's events collect in `E`.
fn pane() -> (crate::script::UiScript, TestView) {
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
    let mut v = TestView {
        channel_slots: vec![
            slot(GENERAL, Some("General"), true),
            slot("MyChan", None, true),
            slot("Trade - City", Some("Trade"), false),
        ],
        ..Default::default()
    };
    s.with_live(&mut v, |s| s.channels_updated());
    (s, v)
}

/// The events fired since the last call, in order.
fn events(s: &crate::script::UiScript) -> Vec<String> {
    s.eval::<Vec<String>>("local e = E E = {} return e")
        .unwrap()
}

/// The display-list rebuild (`0x504840`): the group header, the world header over the
/// `ChatChannels.dbc` slots, the custom header over the rest, each channel in slot order;
/// `GetChannelDisplayInfo` (`0x4ff580`) answers nine values per entry and none past the list.
#[test]
fn the_display_list_puts_world_then_custom_channels_under_their_headers() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        assert_eq!(
            events(s),
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
        // A custom record lists under the custom header, live.
        test_view(|v| v.channel_slots = vec![slot("World", None, true)]);
        s.run(
            "assert(GetNumDisplayChannels() == 4) \
             local n, _, _, num, _, act, cat = GetChannelDisplayInfo(4) \
             assert(n == 'World' and num == 1 and act == 1 and cat == 'CHANNEL_CATEGORY_CUSTOM')",
        )
        .unwrap();
    });
}

/// `CollapseChannelHeader` (`0x5052e0`) and `ExpandChannelHeader` (`0x505360`) flip a header's
/// flag and relist with `CHANNEL_UI_UPDATE` inside the call; a collapsed header keeps its count.
#[test]
fn a_header_collapses_and_expands_with_a_ui_update() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        events(s);
        s.run("CollapseChannelHeader(2)").unwrap();
        assert_eq!(events(s), vec!["CHANNEL_UI_UPDATE:nil:nil"]);
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
            events(s).is_empty(),
            "a channel entry, past the list and a non-number do nothing"
        );
        s.run("ExpandChannelHeader(2)").unwrap();
        assert_eq!(events(s).len(), 1);
        assert_eq!(s.eval::<i64>("return GetNumDisplayChannels()").unwrap(), 6);
        assert_eq!(s.arity("CollapseChannelHeader(1)").unwrap(), 0);
    });
}

/// `SetSelectedDisplayChannel` (`0x504e50`) watches the channel and asks its list
/// (`CMSG_SET_CHANNEL_WATCH`, `CMSG_CHANNEL_DISPLAY_LIST`); the display reply (`0x503990`) fills
/// the roster once every name is in (`0x4ffe30`), sorted by name (`0x4fb0f0`).
#[test]
fn a_selected_channel_is_watched_and_its_display_list_fills_the_roster() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
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
        events(s);

        // Only the selected channel's list is read, in any case.
        s.display_channel_list("MyChan", 0, &[(0x99, 0)], |_| true);
        assert!(events(s).is_empty());
        test_view(|v| {
            v.player_names.insert(OWN, "Me".into());
            v.player_names.insert(0x20, "bob".into());
        });
        s.display_channel_list(
            "general - elwynn forest",
            0x18,
            &[(OWN, 0x01), (0x20, 0x00), (0x30, 0x02)],
            |g| g != 0x30,
        );
        assert_eq!(
            events(s),
            vec!["CHANNEL_FLAGS_UPDATED:3:nil", "CHANNEL_FLAGS_UPDATED:3:nil"],
            "the channel's flags moved, then the player's own row (`0x503a7f`, `0x503bc6`)"
        );
        assert!(
            s.eval::<Option<i64>>("return GetNumChannelMembers(3)")
                .unwrap()
                .is_none(),
            "a name is still asked"
        );
        assert_eq!(s.arity("GetChannelRosterInfo(3, 1)").unwrap(), 0);

        // The answer caches the name before the record's callbacks run (`0x680c60`).
        test_view(|v| {
            v.player_names.insert(0x30, "Ann".into());
        });
        s.channel_roster_named(0x30);
        assert_eq!(
            events(s),
            vec!["CHANNEL_COUNT_UPDATE:3:3", "CHANNEL_ROSTER_UPDATE:3:3"]
        );
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
    });
}

/// A name the cache has no record for reads `UNKNOWN` (`0x50505a`); the server's leave of the
/// selected channel unselects it ahead of the relist (`0x5079b3`-`0x507a0f`).
#[test]
fn an_unnamed_member_reads_unknown_and_a_leave_unselects() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        s.run("SetSelectedDisplayChannel(6)").unwrap();
        s.display_channel_list("MyChan", 0, &[(0x40, 0)], |_| true);
        s.run("assert(GetChannelRosterInfo(6, 1) == 'nobody')")
            .unwrap();
        assert!(
            s.eval::<Option<i64>>("return IsDisplayChannelOwner()")
                .unwrap()
                .is_none(),
            "the player has no row"
        );
        test_view(|v| {
            v.channel_slots = vec![
                slot(GENERAL, Some("General"), true),
                None,
                slot("Trade - City", Some("Trade"), false),
            ]
        });
        s.channel_left("mychan");
        s.channels_updated();
        assert!(s
            .eval::<Option<i64>>("return GetSelectedDisplayChannel()")
            .unwrap()
            .is_none());
        assert_eq!(s.eval::<i64>("return GetNumDisplayChannels()").unwrap(), 5);
    });
}

/// The list update (`0x505d40`) and its rebuild (`0x504840`) never touch the selection: a channel
/// selected while its record is still pending keeps its selection, roster and own flags across a
/// relist, and a leave of another record keeps it too; only its own leave (`0x5079cd`) clears it.
#[test]
fn a_relist_keeps_a_pending_selection_and_only_its_own_leave_clears_it() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        s.run("SetSelectedDisplayChannel(6)").unwrap();
        s.display_channel_list("MyChan", 0, &[(OWN, 0x1), (0x40, 0)], |_| true);
        assert_eq!(
            s.eval::<i64>("return GetSelectedDisplayChannel()").unwrap(),
            6
        );
        // The record falls back to pending (another join's claim, a voice rebuild): not listed.
        test_view(|v| {
            v.channel_slots = vec![
                slot(GENERAL, Some("General"), true),
                slot("MyChan", None, false),
                slot("Trade - City", Some("Trade"), false),
            ]
        });
        s.channel_left(GENERAL);
        s.channels_updated();
        // Listed again: the selection, its roster and the owner flag are all still there.
        test_view(|v| {
            v.channel_slots = vec![
                slot(GENERAL, Some("General"), true),
                slot("MyChan", None, true),
                slot("Trade - City", Some("Trade"), false),
            ]
        });
        s.channels_updated();
        assert_eq!(
            s.eval::<i64>("return GetSelectedDisplayChannel()").unwrap(),
            6
        );
        assert_eq!(s.eval::<i64>("return GetNumChannelMembers(6)").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return IsDisplayChannelOwner()").unwrap(), 1);
        s.channel_left("MyChan");
        s.channels_updated();
        assert!(s
            .eval::<Option<i64>>("return GetSelectedDisplayChannel()")
            .unwrap()
            .is_none());
    });
}

/// The pane's state is the game's, which a reload's UI shutdown and FrameXML init never touch
/// (only the session init clears it, `0x50f198`-`0x50f202`): a new VM over the same game state answers the same
/// selection (`0x4ffa10`), collapsed header, roster and owner flag, and asks nothing again.
#[test]
fn a_reload_keeps_the_selection_the_collapsed_headers_and_the_roster() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        s.run("SetSelectedDisplayChannel(6)").unwrap();
        s.display_channel_list("MyChan", 0, &[(OWN, 0x1), (0x40, 0)], |_| true);
        s.run("CollapseChannelHeader(2)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetSelectedDisplayChannel()").unwrap(),
            4
        );
        s.take_channel_commands();
    });
    let mut reloaded = script();
    reloaded.with_live(&mut v, |s| {
        assert_eq!(
            s.eval::<i64>("return GetSelectedDisplayChannel()").unwrap(),
            4,
            "the selection outlives the VM"
        );
        s.run(
            "local _, h, c = GetChannelDisplayInfo(2) assert(h == 1 and c == 1, 'world collapsed') \
             assert(GetNumDisplayChannels() == 4, 'the rebuild keeps the fold') \
             assert(GetNumChannelMembers(4) == 2, 'the roster outlives the VM') \
             assert(IsDisplayChannelOwner() == 1, 'the own flags outlive the VM')",
        )
        .unwrap();
        assert!(
            s.take_channel_commands().is_empty(),
            "the kept selection asks nothing"
        );
    });
}

/// `GetChannelRosterInfo` reads the member's name off the cache at the call (`0x504f30`): a record
/// that changes after the roster fired answers its new name, and one that is gone reads `UNKNOWN`
/// (`0x50505a`); the roster's order stays as the last name sorted it.
#[test]
fn a_roster_name_is_read_off_the_cache_at_the_call() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        s.run("SetSelectedDisplayChannel(6)").unwrap();
        test_view(|v| {
            v.player_names.insert(0x40, "Ann".into());
            v.player_names.insert(0x41, "Bob".into());
        });
        s.display_channel_list("MyChan", 0, &[(0x41, 0), (0x40, 0)], |_| true);
        s.run(
            "assert(GetChannelRosterInfo(6, 1) == 'Ann' and GetChannelRosterInfo(6, 2) == 'Bob')",
        )
        .unwrap();
        test_view(|v| {
            v.player_names.insert(0x40, "Zoe".into());
        });
        s.run("assert(GetChannelRosterInfo(6, 1) == 'Zoe', 'the record at the call')")
            .unwrap();
        test_view(|v| {
            v.player_names.remove(&0x40);
        });
        s.run("assert(GetChannelRosterInfo(6, 1) == 'nobody', 'no record reads UNKNOWN')")
            .unwrap();
    });
}

/// `ResetChatWindows` sets each `flags & 1` row's `ZONECHANNELS` bit inside the call (`0x50211e`),
/// and no other row's.
#[test]
fn reset_chat_windows_sets_the_auto_join_rows_zone_bits_in_the_call() {
    let mut s = script();
    let row = |id: u32, auto_join: bool| crate::script::ZoneChannelRow {
        id,
        shortcut: format!("C{id}"),
        resolved: Some(format!("C{id} - Elwynn Forest")),
        listed: true,
        auto_join,
        guild_recruitment: false,
    };
    let mut v = TestView {
        zone_channels: vec![row(1, true), row(2, true), row(3, false), row(22, true)],
        ..Default::default()
    };
    s.with_live(&mut v, |s| {
        s.run("ResetChatWindows()").unwrap();
        test_view(|v| assert_eq!(v.zone_bits, vec![1, 2, 22], "set before the call returns"));
    });
}

/// The party display list (`0x5045c0`): the party lists under the group header, and selecting it
/// builds the player and the party at once, the leader as owner, with nothing on the wire.
#[test]
fn a_party_lists_under_the_group_header_and_builds_its_roster_at_once() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        s.set_player_record(crate::script::PlayerRecord {
            name: "Me".into(),
            ..Default::default()
        });
        s.set_party(PartyState {
            members: vec![
                PartyMemberInfo {
                    name: "Zed".into(),
                    guid: 0x50,
                    online: true,
                },
                PartyMemberInfo {
                    name: "amy".into(),
                    guid: 0x60,
                    online: true,
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
        // The rows' names are the name cache's at the call (`0x4fb123`, `0x50503e`).
        test_view(|v| {
            v.player_names.insert(OWN, "Me".into());
            v.player_names.insert(0x50, "Zed".into());
            v.player_names.insert(0x60, "amy".into());
        });
        assert_eq!(s.eval::<i64>("return GetNumChannelMembers(2)").unwrap(), 3);
        assert!(s.take_channel_commands().is_empty(), "a group asks nothing");
        s.run(
            "local n, o = GetChannelRosterInfo(2, 1) assert(n == 'amy' and o == nil, 'amy') \
             assert(GetChannelRosterInfo(2, 2) == 'Me', 'Me') \
             n, o = GetChannelRosterInfo(2, 3) assert(n == 'Zed' and o == 1, 'the leader owns') \
             assert(GetSelectedDisplayChannel() == 2 and IsDisplayChannelOwner() == nil, 'selection')",
        )
        .unwrap();
    });
}

/// The raid display list (`0x504720`): the raid's entry wins over the party's, ranks map to owner
/// and moderator; in a battleground (`0x572f50`) the raid lists as the battleground.
#[test]
fn a_raid_lists_its_entry_and_a_battleground_raid_the_battleground() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
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
                online: true,
            }],
            raid: vec![
                member("Me", OWN, 2),
                member("Zed", 0x50, 1),
                member("amy", 0x60, 0),
            ],
            ..Default::default()
        });
        // The rows' names are the name cache's at the call (`0x4fb123`, `0x50503e`).
        test_view(|v| {
            v.player_names.insert(OWN, "Me".into());
            v.player_names.insert(0x50, "Zed".into());
            v.player_names.insert(0x60, "amy".into());
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
    });
}

/// `SetActiveVoiceChannel` (`0x500ae0`): no entry carries voice, so an index opens nothing; 0
/// leaves the active entry, the first, signalling it.
#[test]
fn set_active_voice_channel_zero_signals_the_first_entry() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        events(s);
        s.run("SetActiveVoiceChannel(3) SetActiveVoiceChannel('x')")
            .unwrap();
        assert!(events(s).is_empty());
        s.run("SetActiveVoiceChannel(0.5)").unwrap();
        assert_eq!(events(s), vec!["CHANNEL_VOICE_UPDATE:1:nil"]);
        assert_eq!(s.arity("SetActiveVoiceChannel(0)").unwrap(), 0);
        s.run(
            "local _, _, _, _, _, _, _, v, va = GetChannelDisplayInfo(3) \
             assert(v == nil and va == nil)",
        )
        .unwrap();
    });
}

/// The user-list packets edit the watched roster (`0x504130`, `0x5042f0`, `0x500380`): a member
/// with a cached name joins sorted and fires the count and roster, one without waits on its name,
/// a removal keeps the others' order, and another channel's packet is ignored.
#[test]
fn the_user_list_packets_edit_the_watched_roster() {
    let (mut s, mut v) = pane();
    s.with_live(&mut v, |s| {
        s.run("SetSelectedDisplayChannel(3)").unwrap();
        s.take_channel_commands();
        test_view(|v| {
            v.player_names.insert(OWN, "Me".into());
            v.player_names.insert(0x20, "bob".into());
            v.player_names.insert(0x30, "Ann".into());
        });
        s.display_channel_list(GENERAL, 0x18, &[(OWN, 0x01), (0x20, 0)], |_| true);
        events(s);
        let names = |s: &crate::script::UiScript| -> Vec<String> {
            let n = s.eval::<i64>("return GetNumChannelMembers(3)").unwrap();
            (1..=n)
                .map(|i| {
                    s.eval::<String>(&format!("return (GetChannelRosterInfo(3, {i}))"))
                        .unwrap()
                })
                .collect()
        };

        s.channel_user_added(GENERAL, 0x30, 0, 0x18, |_| true);
        assert_eq!(
            events(s),
            vec!["CHANNEL_COUNT_UPDATE:3:3", "CHANNEL_ROSTER_UPDATE:3:3"]
        );
        assert_eq!(names(s), ["Ann", "bob", "Me"]);

        s.channel_user_added(GENERAL, 0x40, 0, 0x1a, |_| false);
        assert_eq!(
            events(s),
            vec!["CHANNEL_FLAGS_UPDATED:3:nil"],
            "the channel's flags moved; the roster waits on the name"
        );
        test_view(|v| {
            v.player_names.insert(0x40, "Cid".into());
        });
        s.channel_roster_named(0x40);
        assert_eq!(
            events(s),
            vec!["CHANNEL_COUNT_UPDATE:3:4", "CHANNEL_ROSTER_UPDATE:3:4"]
        );

        s.channel_user_removed(GENERAL, 0x20, 0x1a);
        assert_eq!(
            events(s),
            vec!["CHANNEL_COUNT_UPDATE:3:3", "CHANNEL_ROSTER_UPDATE:3:3"]
        );
        assert_eq!(names(s), ["Ann", "Cid", "Me"]);

        s.channel_user_updated(GENERAL, 0x30, 0x02, 0x1a);
        assert_eq!(
            events(s),
            vec!["CHANNEL_COUNT_UPDATE:3:3", "CHANNEL_ROSTER_UPDATE:3:3"]
        );
        s.run("local _, _, m = GetChannelRosterInfo(3, 1) assert(m == 1, 'moderator')")
            .unwrap();

        s.channel_user_removed("MyChan", 0x30, 0);
        s.channel_user_removed(GENERAL, 0x99, 0x1a);
        assert!(events(s).is_empty(), "another channel, an unknown member");
    });
}
