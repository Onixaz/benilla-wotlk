//! The social window's raid tab (`RaidFrame.xml` and `Blizzard_RaidUI`): the 8x5 grid's seating
//! and colouring, the drag, the row menu, the ready check and the saved-instance panel.

use benilla_ui::script::{
    PartyMemberInfo, PartyRequest, PartyState, RaidMemberInfo, SavedInstanceInfo, UiScript,
};

fn setup() -> UiScript {
    super::test_ui::social_ui(|_| {})
}

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible()"))
        .unwrap()
}

fn text_of(s: &UiScript, region: &str) -> String {
    s.eval::<String>(&format!("return {region}:GetText() or \"\""))
        .unwrap()
}

/// One raid row; `rank` is 2 leader, 1 assistant, 0 member. `subgroup` is the 1-based number the
/// pane shows: the record stores it 0-based and the binding adds one (`0x4bb61a`), so a 1-based
/// record would seat every row one group to the right.
fn row(
    name: &str,
    rank: u32,
    subgroup: u32,
    class: &str,
    online: bool,
    dead: bool,
) -> RaidMemberInfo {
    RaidMemberInfo {
        name: name.to_string(),
        // Unique per name, so no two rows share a token's guid.
        guid: name.bytes().fold(0xF000u64, |g, b| g * 31 + u64::from(b)),
        rank,
        subgroup: subgroup - 1,
        level: 60,
        class: Some(class.to_string()),
        class_file: Some(class.to_uppercase()),
        zone: Some("Molten Core".to_string()),
        online,
        ninth: dead,
        assignments: 0,
        roles: 0,
    }
}

/// Pushes a roster, its `raidN` snapshots and its event, as `ui_party`'s `feed_party` does. Row 1
/// is us: the player snapshot takes its guid, so `raid1` and `"player"` are one unit.
fn push_raid(s: &mut UiScript, raid: Vec<RaidMemberInfo>) {
    if let Some(me) = raid.first() {
        s.set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                name: Some(me.name.clone()),
                guid: me.guid,
                level: me.level,
                is_player: true,
                player_controlled: true,
                is_connected: true,
                class: me.class.clone(),
                class_file: me.class_file.clone(),
                faction_group: Some("Alliance".into()),
                faction_group_localized: Some("Alliance".into()),
                ..Default::default()
            }),
        );
    }
    for i in 1..=40usize {
        let snap = raid.get(i - 1).map(|r| benilla_ui::script::UnitState {
            exists: true,
            name: Some(r.name.clone()),
            guid: r.guid,
            level: r.level,
            is_player: true,
            player_controlled: true,
            is_connected: r.online,
            dead: r.ninth,
            reaction: 5,
            ..Default::default()
        });
        s.set_unit(&format!("raid{i}"), snap);
    }
    // `members` is our own subgroup's slice; non-empty, it makes `IsPartyLeader()` and
    // `IsRaidLeader()` answer 1 for `leader_index` 0.
    let members = raid
        .iter()
        .skip(1)
        .take(4)
        .map(|r| PartyMemberInfo {
            name: r.name.clone(),
            guid: r.guid,
        })
        .collect();
    s.set_party(PartyState {
        members,
        leader_index: 0,
        // Row 1 is us, and we lead.
        leader_guid: raid.first().map_or(0, |r| r.guid),
        own_guid: raid.first().map_or(0, |r| r.guid),
        raid,
        loot_method: "group".into(),
        master_looter: None,
        loot_threshold: 2,
        real_party_members: 0,
        real_raid_members: 0,
        loot_master_guid: 0,
        member_assignments: [0; 4],
    });
    s.fire_event("RAID_ROSTER_UPDATE", Vec::new());
}

/// A 12-member raid across three subgroups, led by us, with row 2 an assistant.
fn twelve() -> Vec<RaidMemberInfo> {
    let mut raid = vec![row("Me", 2, 1, "Warrior", true, false)];
    for i in 1..12u32 {
        let subgroup = i / 4 + 1;
        raid.push(row(
            &format!("Member{i}"),
            if i == 1 { 1 } else { 0 },
            subgroup,
            "Priest",
            true,
            false,
        ));
    }
    raid
}

/// Opens the tab and drains the `RequestRaidInfo()` its OnShow sends (`RaidFrame.xml:350`).
fn open_raid_tab(s: &mut UiScript) {
    s.run("ToggleFriendsFrame(5)").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::RequestRaidInfo],
        "showing the pane asks the server for our lockouts"
    );
}

// ── The tab ──────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_fifth_tab_opens_the_raid_pane() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    assert_eq!(
        s.eval::<i64>("return FriendsFrame.numTabs").unwrap(),
        5,
        "friends, who, guild, channels and raid (`FriendsFrame.lua:322-328`)"
    );
    open_raid_tab(&mut s);
    assert!(visible(&s, "RaidFrame"), "the pane opens");
    assert_eq!(text_of(&s, "FriendsFrameTitleText"), "Raid");
    for other in [
        "FriendsListFrame",
        "IgnoreListFrame",
        "WhoFrame",
        "GuildFrame",
    ] {
        assert!(!visible(&s, other), "{other} goes away");
    }
    s.run("FriendsFrameTab1:Click()").unwrap();
    assert!(!visible(&s, "RaidFrame"));
    s.run("FriendsFrameTab5:Click()").unwrap();
    assert!(visible(&s, "RaidFrame"), "the tab button opens it too");
}

/// `FriendsFrame_OnHide` hides `RaidInfoFrame` too.
#[test]
fn hiding_the_window_closes_the_raid_info_panel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    s.run("RaidInfoFrame:Show()").unwrap();
    assert!(visible(&s, "RaidInfoFrame"));
    s.run("HideUIPanel(FriendsFrame)").unwrap();
    assert!(
        !visible(&s, "RaidInfoFrame"),
        "the flyout goes with the window"
    );
}

// ── The not-in-a-raid state ──────────────────────────────────────────────────────────────────────

/// Out of a raid the pane is the not-in-raid blurb and Convert To Raid, live only for a party
/// leader of level 10 or more outside LFG restrictions (`RaidFrame_Update`, `RaidFrame.lua:64-72`).
#[test]
fn convert_to_raid_is_live_only_for_a_party_leader() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    assert!(visible(&s, "RaidFrameNotInRaid"), "solo: the blurb");
    assert!(visible(&s, "RaidFrameConvertToRaidButton"));
    assert_eq!(
        s.eval::<i64>("return RaidFrameConvertToRaidButton:IsEnabled()")
            .unwrap(),
        0,
        "solo there is no party to convert"
    );
    assert!(
        !visible(&s, "RaidFrameReadyCheckButton"),
        "and no raid verbs"
    );

    // A party we lead: `leader_index == 0` with members.
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA,
        }],
        leader_index: 0,
        ..Default::default()
    });
    s.fire_event("PARTY_MEMBERS_CHANGED", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameConvertToRaidButton:IsEnabled()")
            .unwrap(),
        1,
        "a party leader can convert"
    );
    s.run("RaidFrameConvertToRaidButton:Click()").unwrap();
    assert!(
        s.take_party_requests()
            .contains(&PartyRequest::ConvertToRaid),
        "and the button really sends it"
    );

    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Alice".into(),
            guid: 0xA,
        }],
        leader_index: 1,
        ..Default::default()
    });
    s.fire_event("PARTY_LEADER_CHANGED", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameConvertToRaidButton:IsEnabled()")
            .unwrap(),
        0,
        "a party member cannot"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The grid ─────────────────────────────────────────────────────────────────────────────────────

/// Row order and slot order differ; every drag, kick and menu action addresses a row by row index.
/// The rank shows as an icon, not text (`RaidGroupFrame_Update`, `Blizzard_RaidUI.lua:398-407`).
#[test]
fn the_grid_seats_each_row_in_its_own_subgroups_next_free_slot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    // Groups 1-3 hold 4/4/4; the roster is 12 with us at row 1 in group 1.
    assert_eq!(
        s.eval::<String>("return RaidGroupButton1.slot:GetName()")
            .unwrap(),
        "RaidGroup1Slot1",
        "row 1 (us) takes group 1's first seat"
    );
    assert_eq!(
        s.eval::<String>("return RaidGroupButton5.slot:GetName()")
            .unwrap(),
        "RaidGroup2Slot1",
        "row 5 is the first of group 2 — NOT group 1's fifth seat"
    );
    assert_eq!(
        s.eval::<String>("return RaidGroupButton12.slot:GetName()")
            .unwrap(),
        "RaidGroup3Slot4"
    );
    assert_eq!(
        s.eval::<String>("return RaidGroup2Slot1.button").unwrap(),
        "RaidGroupButton5"
    );

    assert_eq!(text_of(&s, "RaidGroupButton1Name"), "Me");
    let rank_icon = |s: &UiScript, row: u32| {
        s.eval::<String>(&format!(
            "return (RaidGroupButton{row}RankTexture:GetTexture() or ''):lower()"
        ))
        .unwrap()
    };
    assert_eq!(
        rank_icon(&s, 1),
        "interface\\groupframe\\ui-group-leadericon",
        "the leader's crown"
    );
    assert_eq!(
        rank_icon(&s, 2),
        "interface\\groupframe\\ui-group-assistanticon",
        "the assistant's icon"
    );
    assert!(
        !visible(&s, "RaidGroupButton3Rank"),
        "a plain member shows no rank frame"
    );
    assert_eq!(text_of(&s, "RaidGroupButton1Level"), "60");

    assert!(visible(&s, "RaidGroupButton12"), "the last real row shows");
    assert!(
        !visible(&s, "RaidGroupButton13"),
        "and the first empty one does not"
    );
    assert!(
        visible(&s, "RaidGroup8"),
        "all eight groups show while in a raid"
    );

    push_raid(&mut s, Vec::new());
    assert!(!visible(&s, "RaidGroup1"));
    assert!(!visible(&s, "RaidGroupButton1"));
    assert!(visible(&s, "RaidFrameNotInRaid"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Offline grey is tested first, then dead red, then the class colour, on all three columns
/// (`Blizzard_RaidUI.lua:135-152`).
#[test]
fn a_rows_colour_is_offline_then_dead_then_class() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(
        &mut s,
        vec![
            row("Me", 2, 1, "Warrior", true, false),
            row("Corpse", 0, 1, "Mage", true, true),
            row("Gone", 0, 1, "Mage", false, false),
            row("GoneDead", 0, 1, "Mage", false, true),
        ],
    );
    let color = |button: &str| {
        s.eval::<(f32, f32, f32)>(&format!(
            "local r, g, b = {button}Name:GetTextColor() return r, g, b"
        ))
        .unwrap()
    };
    let round = |(r, g, b): (f32, f32, f32)| {
        (
            (r * 100.0).round() / 100.0,
            (g * 100.0).round() / 100.0,
            (b * 100.0).round() / 100.0,
        )
    };
    // WARRIOR is 0.78, 0.61, 0.43 (`RAID_CLASS_COLORS`, `Fonts.xml:52`).
    let warrior: (f32, f32, f32) = s
        .eval("return RAID_CLASS_COLORS.WARRIOR.r, RAID_CLASS_COLORS.WARRIOR.g, RAID_CLASS_COLORS.WARRIOR.b")
        .unwrap();
    assert_eq!(
        round(color("RaidGroupButton1")),
        round(warrior),
        "alive: class colour"
    );
    assert_eq!(
        round(color("RaidGroupButton2")),
        (1.0, 0.1, 0.1),
        "dead: RED_FONT_COLOR"
    );
    assert_eq!(
        round(color("RaidGroupButton3")),
        (0.5, 0.5, 0.5),
        "offline: GRAY_FONT_COLOR"
    );
    assert_eq!(
        round(color("RaidGroupButton4")),
        (0.5, 0.5, 0.5),
        "offline AND dead is grey — offline is tested first"
    );
    // Name and level share the row's colour; the class column is a Button, whose text colour
    // this engine cannot read back.
    assert_eq!(
        round(color("RaidGroupButton2")),
        round(
            s.eval::<(f32, f32, f32)>(
                "local r, g, b = RaidGroupButton2Level:GetTextColor() return r, g, b"
            )
            .unwrap()
        ),
        "name and level share the row's colour"
    );
}

/// `UNIT_LEVEL` repaints one `raidN` row's level without the roster event; `UNIT_HEALTH` repaints
/// nothing, as the stock arm matches its pattern against the literal `"..."`
/// (`Blizzard_RaidUI.lua:174-179`).
#[test]
fn a_units_level_repaints_its_row_and_its_health_repaints_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let mut raid = twelve();
    push_raid(&mut s, raid.clone());
    assert_eq!(text_of(&s, "RaidGroupButton3Level"), "60");
    let green = |s: &UiScript| {
        s.eval::<f32>("local r, g = RaidGroupButton3Name:GetTextColor() return g")
            .unwrap()
    };
    let alive = green(&s);

    // The row dies; only the per-unit event fires, and the row keeps its colour.
    raid[2].ninth = true;
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Member1".into(),
            guid: raid[1].guid,
        }],
        leader_index: 0,
        raid: raid.clone(),
        ..Default::default()
    });
    s.fire_event(
        "UNIT_HEALTH",
        vec![benilla_ui::script::ScriptValue::Str("raid3".into())],
    );
    assert_eq!(green(&s), alive, "the health arm never matches a token");

    s.set_unit(
        "raid3",
        Some(benilla_ui::script::UnitState {
            exists: true,
            level: 61,
            ..Default::default()
        }),
    );
    s.fire_event(
        "UNIT_LEVEL",
        vec![benilla_ui::script::ScriptValue::Str("raid3".into())],
    );
    assert_eq!(text_of(&s, "RaidGroupButton3Level"), "61");
    // A token that is not `raidN` is ignored.
    s.fire_event(
        "UNIT_LEVEL",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
    assert_eq!(text_of(&s, "RaidGroupButton3Level"), "61");
}

// ── The management buttons ───────────────────────────────────────────────────────────────────────

/// The Ready Check button is a leader's or an officer's (`RaidFrameReadyCheckButton_Update`,
/// `Blizzard_RaidUI.lua:1502-1508`); out of a raid it hides with the grid.
#[test]
fn ready_check_is_for_the_leader_and_the_officers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());
    assert!(
        visible(&s, "RaidFrameReadyCheckButton"),
        "we lead, so we may ask"
    );
    assert!(
        !visible(&s, "RaidFrameConvertToRaidButton"),
        "already a raid"
    );
    assert!(!visible(&s, "RaidFrameNotInRaid"));

    s.run("RaidFrameReadyCheckButton:Click()").unwrap();
    assert!(s
        .take_party_requests()
        .contains(&PartyRequest::ReadyCheckStart));

    // Member 1 leads now and we are a plain member: no button.
    let mut raid = twelve();
    raid[0].rank = 0;
    raid[1].rank = 2;
    let me = raid[0].guid;
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Member1".into(),
            guid: raid[1].guid,
        }],
        leader_index: 1,
        leader_guid: raid[1].guid,
        own_guid: me,
        raid: raid.clone(),
        ..Default::default()
    });
    s.fire_event("RAID_ROSTER_UPDATE", Vec::new());
    assert!(!visible(&s, "RaidFrameReadyCheckButton"));

    // Promoted to assistant, we may ask again (`IsRaidOfficer`, `0x573b00`).
    raid[0].rank = 1;
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Member1".into(),
            guid: raid[1].guid,
        }],
        leader_index: 1,
        leader_guid: raid[1].guid,
        own_guid: me,
        raid,
        ..Default::default()
    });
    s.fire_event("RAID_ROSTER_UPDATE", Vec::new());
    assert!(
        visible(&s, "RaidFrameReadyCheckButton"),
        "an assistant asks too"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The drag ─────────────────────────────────────────────────────────────────────────────────────

/// An empty slot in another group moves, an occupied one swaps, anything else springs the row home
/// (`Blizzard_RaidUI.lua:257-273`); `TARGET_RAID_SLOT` stands in for the hover sweep.
#[test]
fn dragging_a_row_moves_swaps_or_springs_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    let drag = |s: &UiScript, button: &str, onto: &str| {
        s.run(&format!(
            "MOVING_RAID_MEMBER = {button}; TARGET_RAID_SLOT = {onto}; \
             RaidGroupButton_OnDragStop({button})"
        ))
        .unwrap();
    };

    // Row 12 (group 3, seat 4) onto group 3's fifth seat, its own group: nothing goes out.
    drag(&s, "RaidGroupButton12", "RaidGroup3Slot5");
    assert!(
        s.take_party_requests().is_empty(),
        "a drop inside the row's own group is not a move"
    );

    // Row 12 onto group 5's empty first seat: a move, with the 1-based subgroup.
    drag(&s, "RaidGroupButton12", "RaidGroup5Slot1");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SetSubgroup {
            index: 12,
            group: 5
        }]
    );

    // Row 12 onto group 1's second seat, which row 2 holds: a swap, by the two row indices.
    drag(&s, "RaidGroupButton12", "RaidGroup1Slot2");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SwapSubgroup {
            index: 12,
            other: 2
        }]
    );
}

/// `OnDragStart` and `OnDragStop` both return for a non-leader (`Blizzard_RaidUI.lua:236`, `:248`).
#[test]
fn only_the_leader_may_drag() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let raid = twelve();
    s.set_party(PartyState {
        members: vec![PartyMemberInfo {
            name: "Member1".into(),
            guid: raid[1].guid,
        }],
        leader_index: 1,
        raid,
        ..Default::default()
    });
    s.fire_event("RAID_ROSTER_UPDATE", Vec::new());
    s.run(
        "MOVING_RAID_MEMBER = RaidGroupButton12; TARGET_RAID_SLOT = RaidGroup5Slot1; \
         RaidGroupButton_OnDragStop(RaidGroupButton12)",
    )
    .unwrap();
    assert!(
        s.take_party_requests().is_empty(),
        "a member's drag sends nothing"
    );
}

// ── The row's click and menu ─────────────────────────────────────────────────────────────────────

/// A row is a `SecureUnitButtonTemplate` whose OnLoad seats no `type` attribute
/// (`RaidGroupButton_OnLoad`, `Blizzard_RaidUI.lua:604-612`), so a left click acts on nothing.
#[test]
fn left_clicking_a_row_does_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());
    s.run("RaidGroupButton7:Click(\"LeftButton\")").unwrap();
    assert_eq!(s.take_selection_requests(), vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The RAID menu's rank rules (`UnitPopup.lua:725-776`): promote for a plain member, demote for
/// an assistant, main tank and assist for anyone not already marked, and no lead or kick for the
/// leader's own row.
#[test]
fn the_row_menu_offers_the_rank_verbs_by_rank() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let mut raid = twelve();
    raid[3].assignments = 0x2; // row 4 is the main tank
    push_raid(&mut s, raid);

    s.run("RaidGroupButton3:Click(\"RightButton\")").unwrap();
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the menu opens"
    );
    let shown = |s: &UiScript, key: &str| {
        s.eval::<bool>(&format!(
            "for i = 1, UIDROPDOWNMENU_MAXBUTTONS do \
                 local b = _G[\"DropDownList1Button\"..i] \
                 if b and b:IsShown() and b.value == \"{key}\" then return true end \
             end return false"
        ))
        .unwrap()
    };
    assert!(
        shown(&s, "RAID_PROMOTE"),
        "a plain member can be made an assistant"
    );
    assert!(!shown(&s, "RAID_DEMOTE"), "and cannot be demoted from one");
    assert!(shown(&s, "RAID_LEADER"), "and can be handed the lead");
    assert!(shown(&s, "RAID_MAINTANK"));
    assert!(shown(&s, "RAID_MAINASSIST"));
    assert!(shown(&s, "RAID_REMOVE"));

    // Row 2 is the assistant: demote, not promote.
    s.run("HideDropDownMenu(1) RaidGroupButton2:Click(\"RightButton\")")
        .unwrap();
    assert!(shown(&s, "RAID_DEMOTE"));
    assert!(!shown(&s, "RAID_PROMOTE"));

    // Row 4 is already the main tank (`GetRaidRosterInfo`'s tenth return).
    s.run("HideDropDownMenu(1) RaidGroupButton4:Click(\"RightButton\")")
        .unwrap();
    assert!(!shown(&s, "RAID_MAINTANK"));
    assert!(shown(&s, "RAID_MAINASSIST"));
    assert!(shown(&s, "RAID_DEMOTE"), "a marked member can be cleared");

    s.run("HideDropDownMenu(1) RaidGroupButton1:Click(\"RightButton\")")
        .unwrap();
    assert!(!shown(&s, "RAID_LEADER"));
    assert!(
        !shown(&s, "RAID_REMOVE"),
        "the leader cannot be kicked, not even by themself"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The verbs by name (`UnitPopup.lua:1330-1348`): promote, the main tank mark through
/// `SetPartyAssignment`, and the kick through `UninviteUnit`.
#[test]
fn the_menu_verbs_queue_the_right_requests() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let raid = twelve();
    let member2 = raid[2].guid;
    push_raid(&mut s, raid);
    let click = |s: &UiScript, key: &str| {
        s.run(&format!(
            "for i = 1, UIDROPDOWNMENU_MAXBUTTONS do \
                 local b = _G[\"DropDownList1Button\"..i] \
                 if b and b:IsShown() and b.value == \"{key}\" then b:Click() return end \
             end error(\"{key} not shown\")"
        ))
        .unwrap();
    };
    s.run("RaidGroupButton3:Click(\"RightButton\")").unwrap();
    click(&s, "RAID_PROMOTE");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::AssistantLeader {
            name: "Member2".into(),
            grant: true
        }]
    );

    s.run("HideDropDownMenu(1) RaidGroupButton3:Click(\"RightButton\")")
        .unwrap();
    click(&s, "RAID_MAINTANK");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::PartyAssignment {
            assignment: 0,
            set: true,
            guid: member2
        }]
    );

    s.run("HideDropDownMenu(1) RaidGroupButton3:Click(\"RightButton\")")
        .unwrap();
    click(&s, "RAID_REMOVE");
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::UninviteUnit {
            unit: "Member2".into(),
            reason: String::new()
        }]
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── Raid Info ────────────────────────────────────────────────────────────────────────────────────

fn lockout(name: &str, instance: u64, reset: u32, difficulty_name: &str) -> SavedInstanceInfo {
    SavedInstanceInfo {
        map: 533,
        name: Some(name.into()),
        instance,
        reset,
        difficulty: 1,
        locked: true,
        extended: false,
        is_raid: true,
        max_players: 25,
        difficulty_name: difficulty_name.into(),
    }
}

/// `RaidInfoFrame_Update` (`RaidFrame.lua:84-155`): a row per lockout with its difficulty and the
/// time left through `SecondsToTime`; an expired lock greys out; past the list the rows hide.
#[test]
fn the_raid_info_panel_lists_the_saved_lockouts() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    // The first `UPDATE_INSTANCE_INFO` only arms `RaidFrame.hasRaidInfo` (`RaidFrame.lua:46-50`).
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    s.set_saved_instances(vec![
        lockout("Naxxramas", 1234, 3 * 86_400, "25 Player"),
        SavedInstanceInfo {
            locked: false,
            ..lockout("The Eye of Eternity", 77, 3_600, "10 Player")
        },
    ]);
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        1
    );
    s.run("RaidInfoFrame:Show()").unwrap();
    let row = |s: &UiScript, i: u32| {
        s.eval::<(String, String, String, bool)>(&format!(
            "local b = RaidInfoScrollFrameButton{i} \
             return b.name:GetText() or '', b.difficulty:GetText() or '', \
             b.reset:GetText() or '', b:IsVisible()"
        ))
        .unwrap()
    };
    let (name, difficulty, reset, shown) = row(&s, 1);
    assert!(shown);
    assert_eq!(
        (name.as_str(), difficulty.as_str()),
        ("Naxxramas", "25 Player")
    );
    assert!(
        reset.starts_with("3 Days"),
        "the reset is a remaining duration through SecondsToTime: {reset:?}"
    );
    let (name, _, reset, shown) = row(&s, 2);
    assert!(shown);
    assert!(
        name.contains("The Eye of Eternity") && name.starts_with("|cff808080"),
        "an expired lock is grey: {name:?}"
    );
    assert!(reset.starts_with("|cff808080"), "{reset:?}");
    assert!(!row(&s, 3).3, "rows past the list stay away");

    s.set_saved_instances(Vec::new());
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        0
    );
    assert!(!row(&s, 1).3, "an empty list hides every row");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An ordinary session gets two empty answers, for `PLAYER_ENTERING_WORLD`'s `RequestRaidInfo` and
/// the pane's OnShow; the first arms the latch, the second disables the button.
#[test]
fn a_player_with_no_lockouts_loses_the_raid_info_button_on_the_second_answer() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);

    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrame.hasRaidInfo or 0").unwrap(),
        1,
        "the first answer only arms the latch"
    );

    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    assert_eq!(
        s.eval::<i64>("return RaidFrameRaidInfoButton:IsEnabled()")
            .unwrap(),
        0,
        "an empty lockout list is a dead button"
    );

    // The button is dead to a real click through the pointer, not merely drawn grey.
    let (bx, by) = {
        let l: f32 = s.eval("return RaidFrameRaidInfoButton:GetLeft()").unwrap();
        let r: f32 = s.eval("return RaidFrameRaidInfoButton:GetRight()").unwrap();
        let t: f32 = s.eval("return RaidFrameRaidInfoButton:GetTop()").unwrap();
        let b: f32 = s
            .eval("return RaidFrameRaidInfoButton:GetBottom()")
            .unwrap();
        ((l + r) / 2.0, (t + b) / 2.0)
    };
    s.mouse_button(bx, by, "LeftButton", true);
    s.mouse_button(bx, by, "LeftButton", false);
    s.tick(0.016);
    assert!(
        !visible(&s, "RaidInfoFrame"),
        "a disabled button does not open the panel"
    );
}

/// The list is a `HybridScrollFrame` (`RaidInfoScrollFrame_OnLoad`, `RaidFrame.lua:157-161`):
/// more lockouts than rows scroll, and a scroll re-seats the rows on the later lockouts.
#[test]
fn the_lockout_list_scrolls_past_its_rows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    s.set_saved_instances(
        (1..=12)
            .map(|i| lockout(&format!("Instance {i}"), 1000 + i, i as u32 * 3_600, ""))
            .collect(),
    );
    s.fire_event("UPDATE_INSTANCE_INFO", Vec::new());
    s.run("RaidInfoFrame:Show() RaidInfoFrame_Update()")
        .unwrap();
    let rows: i64 = s.eval("return #RaidInfoScrollFrame.buttons").unwrap();
    assert!(rows < 12, "fewer rows ({rows}) than lockouts");
    assert!(
        s.eval::<f64>("return RaidInfoScrollFrame.range").unwrap() > 0.0,
        "so the frame has a range to scroll"
    );
    let first = |s: &UiScript| {
        s.eval::<String>("return RaidInfoScrollFrameButton1.name:GetText()")
            .unwrap()
    };
    assert_eq!(first(&s), "Instance 1");
    s.run(
        "local h = RaidInfoScrollFrame.buttons[1]:GetHeight() \
         RaidInfoScrollFrame.scrollBar:SetValue(h * 2)",
    )
    .unwrap();
    assert_eq!(first(&s), "Instance 3", "two rows down");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The ready check ──────────────────────────────────────────────────────────────────────────────

/// `READY_CHECK` opens the popup naming the leader; Yes calls `ConfirmReadyCheck(1)`, No calls it
/// with no argument (`Blizzard_RaidUI.xml:710`, `:728`).
#[test]
#[ignore = "the 3.3.5a ready check wire is not built (#361)"]
fn the_ready_check_popup_opens_and_answers() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());
    assert!(!visible(&s, "ReadyCheckFrame"), "nothing to answer yet");

    s.fire_event("READY_CHECK", Vec::new());
    assert!(visible(&s, "ReadyCheckFrame"), "UIParent's arm opens it");
    assert!(
        text_of(&s, "ReadyCheckFrameText").starts_with("Me has initiated a ready check."),
        "named for the rank-2 row: {:?}",
        text_of(&s, "ReadyCheckFrameText")
    );

    s.run("ReadyCheckFrameYesButton:Click()").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::ReadyCheckAnswer(true)]
    );
    assert!(!visible(&s, "ReadyCheckFrame"));

    s.fire_event("READY_CHECK", Vec::new());
    s.run("ReadyCheckFrameNoButton:Click()").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::ReadyCheckAnswer(false)],
        "No passes no argument at all, and absent must mean not-ready"
    );

    s.fire_event("READY_CHECK", Vec::new());
    assert!(visible(&s, "ReadyCheckFrame"));
    push_raid(&mut s, Vec::new());
    assert!(
        !visible(&s, "ReadyCheckFrame"),
        "the raid went, so the question went"
    );
}

/// No release reaches a gesture once the OS pointer leaves the window, so
/// `UiScript::pointer_left_window` ends the drag: `OnDragStop` runs, the row goes home, and the
/// next row still drags.
#[test]
fn a_drag_carried_off_the_window_edge_ends_instead_of_gluing_the_row_to_the_cursor() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    push_raid(&mut s, twelve());

    let left_of =
        |s: &UiScript, f: &str| -> f32 { s.eval(&format!("return {f}:GetLeft()")).unwrap() };
    let centre = |s: &UiScript, f: &str| -> (f32, f32) {
        let l: f32 = s.eval(&format!("return {f}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {f}:GetRight()")).unwrap();
        let t: f32 = s.eval(&format!("return {f}:GetTop()")).unwrap();
        let b: f32 = s.eval(&format!("return {f}:GetBottom()")).unwrap();
        ((l + r) / 2.0, (t + b) / 2.0)
    };

    let home = left_of(&s, "RaidGroupButton7");
    let (fx, fy) = centre(&s, "RaidGroupButton7");
    s.mouse_button(fx, fy, "LeftButton", true);
    s.mouse_move(fx + 40.0, fy + 40.0); // past the threshold ⇒ OnDragStart ⇒ StartMoving
    s.tick(0.016);
    assert_eq!(
        s.eval::<String>("return MOVING_RAID_MEMBER:GetName()")
            .unwrap(),
        "RaidGroupButton7",
        "the drag is in flight"
    );

    s.pointer_left_window();
    s.tick(0.016);
    assert!(
        s.eval::<bool>("return MOVING_RAID_MEMBER == nil").unwrap(),
        "the pane's own drag state is cleared by the OnDragStop the abandon fires"
    );
    assert_eq!(
        left_of(&s, "RaidGroupButton7"),
        home,
        "the row springs back to its slot — a drop on nothing is not a move"
    );

    s.mouse_move(400.0, 300.0);
    s.tick(0.016);
    s.mouse_move(500.0, 200.0);
    s.tick(0.016);
    assert_eq!(
        left_of(&s, "RaidGroupButton7"),
        home,
        "…and it stays there however far the cursor travels"
    );

    let (gx, gy) = centre(&s, "RaidGroupButton8");
    let (tx, ty) = centre(&s, "RaidGroup5Slot1");
    s.mouse_button(gx, gy, "LeftButton", true);
    s.mouse_move(gx + 10.0, gy + 10.0);
    s.tick(0.016);
    s.mouse_move(tx, ty);
    s.tick(0.016);
    s.mouse_button(tx, ty, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::SetSubgroup { index: 8, group: 5 }],
        "the row after the abandoned one still moves"
    );
    assert!(
        s.errors().is_empty(),
        "and nothing raised: {:?}",
        s.errors()
    );
}

/// Three whole gestures through the real pointer path, with the roster echo `/partytest raid`
/// supplies in between.
#[test]
fn one_drag_does_not_cost_the_next_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    open_raid_tab(&mut s);
    let mut raid = vec![row("Me", 2, 1, "Warrior", true, false)];
    for i in 1..25u32 {
        raid.push(row(
            &format!("Member{i}"),
            0,
            i / 5 + 1,
            "Priest",
            true,
            false,
        ));
    }
    push_raid(&mut s, raid.clone());

    let centre = |s: &UiScript, f: &str| -> (f32, f32) {
        let l: f32 = s.eval(&format!("return {f}:GetLeft()")).unwrap();
        let r: f32 = s.eval(&format!("return {f}:GetRight()")).unwrap();
        let t: f32 = s.eval(&format!("return {f}:GetTop()")).unwrap();
        let b: f32 = s.eval(&format!("return {f}:GetBottom()")).unwrap();
        ((l + r) / 2.0, (t + b) / 2.0)
    };

    for (row_index, group) in [(7u32, 6u32), (8, 6), (9, 7)] {
        let from = format!("RaidGroupButton{row_index}");
        let seat = if group == 6 && row_index == 8 { 2 } else { 1 };
        let to = format!("RaidGroup{group}Slot{seat}");
        let (fx, fy) = centre(&s, &from);
        let (tx, ty) = centre(&s, &to);
        s.mouse_button(fx, fy, "LeftButton", true);
        s.mouse_move(fx + 10.0, fy + 10.0);
        s.tick(0.016);
        s.mouse_move(tx, ty);
        s.tick(0.016);
        s.mouse_button(tx, ty, "LeftButton", false);
        s.tick(0.016);
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::SetSubgroup {
                index: row_index,
                group
            }],
            "drag {row_index} onto group {group}"
        );
        assert!(s.errors().is_empty(), "{from}: {:?}", s.errors());
        // The echo, so the next drag starts from a repainted grid.
        raid[row_index as usize - 1].subgroup = group - 1;
        push_raid(&mut s, raid.clone());
    }
}
