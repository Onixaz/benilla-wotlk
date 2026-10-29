//! Tests for the PvP and honor bindings. Several pin reference facts that look like bugs, so each
//! is written to fail against the natural reading.

use super::*;
use crate::script::{UiScript, UnitState};

/// Every number distinct, so a swapped return cannot pass: the tens digit is the block (today
/// 1x, yesterday 2x, lifetime 5x), the units digit the position in it.
fn honor_state() -> HonorState {
    HonorState {
        today_hk: 11,
        today_honor: 12,
        yesterday_hk: 21,
        yesterday_honor: 23,
        lifetime_hk: 51,
        highest_rank: 11,
        rank: 9,
        honor_currency: 4_321,
        arena_currency: 765,
    }
}

/// The inspect reply's six, distinct in return order.
fn inspect_state() -> InspectHonorData {
    InspectHonorData {
        guid: 0xDEAD_BEEF,
        today_hk: 61,
        today_honor: 62,
        yesterday_hk: 63,
        yesterday_honor: 64,
        lifetime_hk: 65,
        highest_rank: 12,
    }
}

/// A female Alliance player at rank 9 (visual 5).
fn player() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Benilla".into()),
        sex: 3, // female
        is_player: true,
        // Both, and they differ: `faction_group` is the live faction template, `pvp_team` the race
        // walk that keys every rank title.
        faction_group: Some("Alliance".into()),
        pvp_team: 1,
        pvp_rank: 9,
        ..Default::default()
    }
}

fn seated() -> UiScript {
    let mut s = UiScript::new().expect("VM");
    s.set_honor(Some(honor_state()));
    s.set_unit("player", Some(player()));
    s
}

/// The rank titles `GlobalStrings.lua` puts on `_G`: both teams, one female twin, the dishonorable
/// end and rank 19. Rank 17 is left out as the key no locale has.
fn seat_rank_globals(s: &UiScript) {
    let g = s.lua().globals();
    for (key, value) in [
        ("PVP_RANK_1_1", "Pariah"),
        ("PVP_RANK_4_1", "Dishonored"),
        ("PVP_RANK_5_0", "Scout"),
        ("PVP_RANK_5_1", "Private"),
        ("PVP_RANK_9_0", "Senior Sergeant"),
        ("PVP_RANK_9_1", "Sergeant Major"),
        ("PVP_RANK_9_1_FEMALE", "Sergeant Major (f)"),
        ("PVP_RANK_18_1", "Grand Marshal"),
        ("PVP_RANK_19_1", "Leader"),
        ("PVP_RANK_19_1_FEMALE", "Leader (f)"),
    ] {
        g.set(key, value).expect("global string");
    }
}

/// The enUS GlobalStrings `UnitPVPName`'s decoration reads.
fn seat_name_globals(s: &UiScript) {
    let g = s.lua().globals();
    for (key, value) in [
        ("UNIT_PVP_NAME", "%s %s"),
        ("PVP_RANK_CIVILIAN", "Civilian"),
        ("PVP_MEDAL1", "Guardian of Stormwind"),
    ] {
        g.set(key, value).expect("global string");
    }
}

/// `PVPFrame.lua`'s own destructuring, line for line: two values per period.
#[test]
fn the_reference_destructuring_lands_every_value_in_the_right_slot() {
    let s = seated();
    assert_eq!(
        s.eval::<(i64, i64)>("local hk, cp = GetPVPSessionStats() return hk, cp")
            .unwrap(),
        (11, 12)
    );
    assert_eq!(
        s.eval::<(i64, i64)>(
            "local hk, contribution = GetPVPYesterdayStats() return hk, contribution"
        )
        .unwrap(),
        (21, 23)
    );
    assert_eq!(
        s.eval::<(i64, i64)>("local hk, highest = GetPVPLifetimeStats() return hk, highest")
            .unwrap(),
        (51, 11)
    );
    assert_eq!(
        s.eval::<(i64, i64)>("return GetHonorCurrency()").unwrap(),
        (4_321, 75_000)
    );
    assert_eq!(
        s.eval::<(i64, i64)>("return GetArenaCurrency()").unwrap(),
        (765, 10_000)
    );
}

/// `0x611bd3 cmp al,5; jb`: 4 vanishes, 5 survives.
#[test]
fn a_lifetime_rank_below_five_is_reported_as_zero() {
    let mut s = seated();
    let second = |s: &UiScript| {
        s.eval::<i64>("local _, highest = GetPVPLifetimeStats() return highest")
            .unwrap()
    };
    for (highest, reported) in [(0u8, 0i64), (1, 0), (4, 0), (5, 5), (6, 6), (18, 18)] {
        s.set_honor(Some(HonorState {
            highest_rank: highest,
            ..honor_state()
        }));
        assert_eq!(second(&s), reported, "highest lifetime rank {highest}");
    }
}

/// A getter returning one value too many passes the destructuring test; the count catches it.
#[test]
fn every_getter_returns_exactly_the_reference_arity() {
    let s = seated();
    for (call, width) in [
        ("GetPVPSessionStats()", 2),
        ("GetPVPYesterdayStats()", 2),
        ("GetPVPLifetimeStats()", 2),
        ("GetPVPRankProgress()", 1),
        ("GetHonorCurrency()", 2),
        ("GetArenaCurrency()", 2),
        ("GetPVPRankInfo(9)", 2),
        ("GetPVPRankInfo(0)", 2),
        ("UnitPVPRank('player')", 1),
        ("SetPVP(1)", 0),
    ] {
        assert_eq!(s.arity(call).unwrap(), width, "{call} arity");
    }
}

/// The reference feeds these straight into `SetText`, where a nil would show nothing.
#[test]
fn the_self_getters_answer_zeroed_before_the_first_push() {
    let s = UiScript::new().expect("VM");
    assert_eq!(
        s.eval::<(i64, i64)>("return GetPVPSessionStats()").unwrap(),
        (0, 0)
    );
    assert_eq!(s.eval::<f64>("return GetPVPRankProgress()").unwrap(), 0.0);
}

/// `SetPVP` sends the state it is given, nil as 0; `TogglePVP` stays the empty toggle.
#[test]
fn set_pvp_queues_a_state_byte_apart_from_the_toggle() {
    let mut s = UiScript::new().expect("VM");
    s.run("SetPVP(1) SetPVP(nil) SetPVP() SetPVP('1') SetPVP(true) TogglePVP()")
        .unwrap();
    assert_eq!(s.take_pvp_sets(), vec![1, 0, 0, 1, 0]);
    assert_eq!(s.take_pvp_toggles(), 1);
}

/// `IsPVPTimerRunning` reads the timer flag alone; `GetPVPTimer` is the pushed timer plus a second.
#[test]
fn the_pvp_timer_pair_reads_the_flag_and_the_pushed_timer() {
    let mut s = seated();
    assert_eq!(s.arity("IsPVPTimerRunning()").unwrap(), 1);
    assert!(s
        .eval::<Option<i64>>("return IsPVPTimerRunning()")
        .unwrap()
        .is_none());
    s.set_unit(
        "player",
        Some(UnitState {
            player_flags: PLAYER_FLAGS_PVP_TIMER,
            ..player()
        }),
    );
    assert_eq!(s.eval::<i64>("return IsPVPTimerRunning()").unwrap(), 1);
    s.set_pvp_toggle_timer(299_000);
    assert_eq!(s.eval::<i64>("return GetPVPTimer()").unwrap(), 300_000);
    s.set_pvp_toggle_timer(-1);
    assert_eq!(s.eval::<i64>("return GetPVPTimer()").unwrap(), 999);
}

/// Runs the panes' own `NONE` fallback, not just the nil.
#[test]
fn rank_zero_names_nothing_and_the_pane_falls_back_to_none() {
    let s = seated();
    seat_rank_globals(&s);
    s.lua().globals().set("NONE", "None").expect("NONE");

    let (name, number) = s
        .eval::<(Option<String>, i64)>("local n, r = GetPVPRankInfo(0) return n, r")
        .unwrap();
    assert_eq!(name, None, "no PVP_RANK_0_* exists");
    assert_eq!(number, 0, "and the badge (rankNumber > 0) stays off");

    assert_eq!(
        s.eval::<String>(
            "local name = GetPVPRankInfo(0) if not name then name = NONE end return name"
        )
        .unwrap(),
        "None"
    );
}

/// `GetPVPRankInfo` passes gender 0, so a female character gets the default title with the
/// `_FEMALE` twin right there on `_G`.
#[test]
fn the_pane_title_is_keyed_by_team_and_is_never_gendered() {
    let mut s = seated();
    seat_rank_globals(&s);

    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );
    assert_eq!(
        s.eval::<String>("return PVP_RANK_9_1_FEMALE").unwrap(),
        "Sergeant Major (f)",
        "the twin is on _G — the binding declines it, it does not miss it"
    );

    s.set_unit("player", Some(UnitState { sex: 2, ..player() }));
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );

    s.set_unit(
        "player",
        Some(UnitState {
            sex: 2,
            faction_group: Some("Horde".into()),
            pvp_team: 0,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Senior Sergeant"
    );
}

/// The credit line's lookup resolves through the reference with the local player's gender, where
/// the binding passes 0, and has no range check, so rank 19 names "Leader" here.
#[test]
fn the_rust_side_title_lookup_is_gendered_where_the_binding_is_not() {
    let s = seated();
    seat_rank_globals(&s);

    assert_eq!(
        s.pvp_rank_title(9, 0, false).as_deref(),
        Some("Senior Sergeant")
    );
    assert_eq!(
        s.pvp_rank_title(9, 1, true).as_deref(),
        Some("Sergeant Major (f)"),
        "the credit line IS gendered"
    );
    // …and the binding, for that same female Alliance player, is not.
    assert_ne!(
        s.pvp_rank_title(9, 1, true),
        s.eval::<Option<String>>("return (GetPVPRankInfo(9))")
            .unwrap()
    );
    assert_eq!(
        s.pvp_rank_title(9, 1, false),
        s.eval::<Option<String>>("return (GetPVPRankInfo(9))")
            .unwrap(),
        "same key construction once the gender is out of it"
    );

    // Gendered means preferred: a rank with no twin falls back to the base key.
    assert_eq!(s.pvp_rank_title(5, 1, true).as_deref(), Some("Private"));

    assert_eq!(s.pvp_rank_title(19, 1, false).as_deref(), Some("Leader"));
    assert_eq!(s.pvp_rank_title(19, 1, true).as_deref(), Some("Leader (f)"));
    assert!(s.eval::<bool>("return GetPVPRankInfo(19) == nil").unwrap());

    assert_eq!(s.pvp_rank_title(0, 1, false), None);
    assert_eq!(s.pvp_rank_title(17, 1, false), None, "no PVP_RANK_17_1");
    s.lua().globals().set("PVP_RANK_17_1", "").expect("global");
    assert_eq!(
        s.pvp_rank_title(17, 1, false),
        None,
        "empty reads as absent"
    );
}

/// The panes' `if not rankName` fallback needs a nil, not an empty string or a raise.
#[test]
fn a_missing_rank_global_reads_nil_not_empty() {
    let s = seated(); // no rank globals seated at all
    assert!(s.eval::<bool>("return GetPVPRankInfo(9) == nil").unwrap());
    s.lua().globals().set("PVP_RANK_9_1", "").expect("global");
    assert!(s.eval::<bool>("return GetPVPRankInfo(9) == nil").unwrap());
    // A player with no side names nothing either, rather than picking a list.
    let mut s = s;
    s.set_unit(
        "player",
        Some(UnitState {
            pvp_team: -1,
            ..player()
        }),
    );
    seat_rank_globals(&s);
    assert!(s.eval::<bool>("return GetPVPRankInfo(9) == nil").unwrap());
}

/// `UnitFactionGroup` reads the live faction template and the title's team digit the race, so a
/// vmangos GM (template 35, no side) keeps his title, in both the key and `UnitPVPName`'s
/// decoration.
#[test]
fn a_sideless_player_still_has_a_team_digit_because_his_race_has_one() {
    let mut s = seated();
    seat_rank_globals(&s);
    s.lua()
        .globals()
        .set("UNIT_PVP_NAME", "%s %s")
        .expect("global");
    s.set_unit(
        "player",
        Some(UnitState {
            // `.gm on`: the template names nothing…
            faction_group: None,
            faction_group_localized: None,
            // …and the race still names Alliance.
            pvp_team: 1,
            sex: 2,
            pvp_rank: 18,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(18))").unwrap(),
        "Grand Marshal",
        "the sideless template must not reach the key"
    );
    // The control: a unit whose race has no side is -1, and -1 misses.
    s.set_unit(
        "player",
        Some(UnitState {
            faction_group: Some("Alliance".into()),
            pvp_team: -1,
            pvp_rank: 18,
            ..player()
        }),
    );
    assert!(
        s.eval::<bool>("return GetPVPRankInfo(18) == nil").unwrap(),
        "a side on the template cannot stand in for a missing team digit either"
    );
}

/// The reference subtracts 5 rather than negating, so rank 1 is -4 and rank 4 is -1, the opposite
/// of vmangos's `visualRank` (`HonorMgr.cpp:991`).
#[test]
fn the_visual_rank_arithmetic_runs_backwards_through_the_dishonorable_ranks() {
    for (internal, visual) in [
        (1i64, -4i64),
        (2, -3),
        (3, -2),
        (4, -1),
        (5, 1),
        (9, 5),
        (18, 14),
    ] {
        assert_eq!(visual_rank(internal), visual, "internal {internal}");
        // The server's form, which this is not.
        let servers = if internal > 4 {
            internal - 4
        } else {
            -internal
        };
        assert_eq!(
            servers == visual,
            internal >= 5,
            "the two forms agree above rank 4 and disagree below it"
        );
    }

    let s = seated();
    seat_rank_globals(&s);
    for (internal, visual) in [(1i64, -4i64), (4, -1), (5, 1), (18, 14)] {
        assert_eq!(
            s.eval::<i64>(&format!(
                "local _, number = GetPVPRankInfo({internal}) return number"
            ))
            .unwrap(),
            visual,
            "GetPVPRankInfo({internal})"
        );
    }
}

/// Both failure edges answer two values, `(nil, 0)`, since the panes destructure a pair.
#[test]
fn the_range_gate_refuses_rank_zero_and_rank_nineteen_alike() {
    let s = seated();
    seat_rank_globals(&s);

    for rank in [0i64, 19, 20, -1, 255] {
        let (name, number) = s
            .eval::<(Option<String>, i64)>(&format!(
                "local n, r = GetPVPRankInfo({rank}) return n, r"
            ))
            .unwrap();
        assert_eq!(name, None, "rank {rank} names nothing");
        assert_eq!(number, 0, "rank {rank} numbers 0");
        assert_eq!(
            s.arity(&format!("GetPVPRankInfo({rank})")).unwrap(),
            2,
            "rank {rank} still answers two values"
        );
    }

    // The refusal is the gate, not a missing key: `PVP_RANK_19_1` is seated.
    assert_eq!(s.eval::<String>("return PVP_RANK_19_1").unwrap(), "Leader");
    assert!(s.eval::<bool>("return GetPVPRankInfo(1) ~= nil").unwrap());
    assert!(s.eval::<bool>("return GetPVPRankInfo(18) ~= nil").unwrap());
}

#[test]
fn get_pvp_rank_info_takes_a_second_argument_three_different_ways() {
    let mut s = seated(); // an Alliance player
    seat_rank_globals(&s);
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Thrall".into()),
            is_player: true,
            faction_group: Some("Horde".into()),
            pvp_team: 0,
            pvp_rank: 14,
            ..Default::default()
        }),
    );

    // Absent: the local player's side.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );
    // A number is the digit, with no unit resolved, even one the player is not.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 0))").unwrap(),
        "Senior Sergeant"
    );
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 1))").unwrap(),
        "Sergeant Major"
    );
    // `lua_isnumber` accepts a numeric string, so "0" is a digit, not a token.
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "0"))"#)
            .unwrap(),
        "Senior Sergeant"
    );
    // …and truncates toward zero, like every other numeric argument.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 0.9))").unwrap(),
        "Senior Sergeant"
    );
    assert!(s
        .eval::<bool>("return GetPVPRankInfo(9, 7) == nil")
        .unwrap());

    // A string is a unit token: the foreign unit's side, not the player's.
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "target"))"#)
            .unwrap(),
        "Senior Sergeant",
        "Thrall is Horde, so his ladder names rank 9"
    );

    // A token naming nothing or a non-player falls to team 0, the register's initial value.
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "mouseover"))"#)
            .unwrap(),
        "Senior Sergeant"
    );
    s.set_unit(
        "mouseover",
        Some(UnitState {
            exists: true,
            name: Some("Timber Wolf".into()),
            pvp_team: 1,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "mouseover"))"#)
            .unwrap(),
        "Senior Sergeant",
        "a non-player is gated out before its side is read"
    );

    // A unit that resolves with no side is -1 and misses the key, unlike the not-found 0 above.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            is_player: true,
            pvp_team: -1,
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>(r#"return GetPVPRankInfo(9, "target") == nil"#)
        .unwrap());
}

/// The reference tests `lua_isnumber` and raises: not a `(nil, 0)` edge.
#[test]
fn get_pvp_rank_info_raises_without_a_numeric_first_argument() {
    let s = seated();
    for call in [
        "GetPVPRankInfo()",
        "GetPVPRankInfo({})",
        "GetPVPRankInfo(nil)",
    ] {
        let err = s
            .eval::<Option<String>>(&format!("return ({call})"))
            .expect_err(call);
        assert!(
            format!("{err}").contains("Usage: GetPVPRankInfo(rank [, unit])"),
            "{call}: {err}"
        );
    }
    // A numeric string is a number to Lua, so it does not raise.
    assert!(s
        .eval::<Option<String>>(r#"return (GetPVPRankInfo("9"))"#)
        .is_ok());
}

/// `PLAYER_BYTES_3` is public, and the reference's inspect pane calls `UnitPVPRank("target")`.
#[test]
fn unit_pvp_rank_answers_for_a_foreign_unit() {
    let mut s = seated();
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Thrall".into()),
            is_player: true,
            faction_group: Some("Horde".into()),
            pvp_team: 0,
            pvp_rank: 14,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitPVPRank("target")"#).unwrap(),
        14
    );
    assert_eq!(s.eval::<i64>(r#"return UnitPVPRank("player")"#).unwrap(), 9);
    // A creature reads 0 because it has no player descriptor block to decode a rank from; the
    // binding does not re-gate on `is_player`.
    s.set_unit(
        "mouseover",
        Some(UnitState {
            exists: true,
            name: Some("Timber Wolf".into()),
            is_player: false,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitPVPRank("mouseover")"#).unwrap(),
        0
    );
    s.set_unit("mouseover", None);
    assert_eq!(
        s.eval::<i64>(r#"return UnitPVPRank("mouseover")"#).unwrap(),
        0,
        "no snapshot reads 0, like every other numeric Unit* getter"
    );
    // A non-string argument raises rather than answering 0.
    assert!(s.eval::<i64>("return UnitPVPRank()").is_err());
    assert!(s.eval::<i64>("return UnitPVPRank({})").is_err());
}

fn seat_titles(s: &mut UiScript) {
    s.set_title_rows(vec![
        crate::script::TitleRow {
            mask_id: 2,
            name: "Corporal %s".into(),
            name1: String::new(),
        },
        crate::script::TitleRow {
            mask_id: 46,
            name: "%s the Explorer".into(),
            name1: "%s the Exploress".into(),
        },
    ]);
}

/// `UnitPVPName` (`0x60e9a0` → `GetUnitChosenTitle` `0x72a290`): a player's chosen title,
/// gendered by this unit, with the medal line under it; no title, a creature or a mask no row has
/// is the plain name; the rank byte decorates nothing.
#[test]
fn unit_pvp_name_shows_the_chosen_title() {
    let mut s = seated();
    seat_name_globals(&s);
    seat_titles(&mut s);

    // The seated player is a female rank 9 with no chosen title.
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla"
    );
    s.set_unit(
        "player",
        Some(UnitState {
            chosen_title: 46,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla the Exploress"
    );
    s.set_unit(
        "player",
        Some(UnitState {
            chosen_title: 2,
            sex: 2,
            pvp_medal: 1,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Corporal Benilla
Guardian of Stormwind",
        "a female asking a row with no female form takes the male one; the medal rides the title"
    );
    s.set_unit(
        "player",
        Some(UnitState {
            chosen_title: 7,
            pvp_medal: 1,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla"
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Timber Wolf".into()),
            level: 5,
            chosen_title: 46,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("target")"#).unwrap(),
        "Timber Wolf"
    );

    assert!(s
        .eval::<bool>(r#"return UnitPVPName("mouseover") == nil"#)
        .unwrap());
    s.set_unit(
        "mouseover",
        Some(UnitState {
            exists: true,
            name: None,
            ..Default::default()
        }),
    );
    assert!(
        s.eval::<bool>(r#"return UnitPVPName("mouseover") == nil"#)
            .unwrap(),
        "a snapshot whose name has not resolved is nil, not an empty decoration"
    );
    assert!(s.eval::<String>("return UnitPVPName()").is_err());
}

/// Two varargs is the engine's limit (`add esp,0x14`), so a third `%s` has nothing to consume.
#[test]
fn the_two_string_template_is_substituted_not_hardcoded() {
    assert_eq!(format_two_strings("%s %s", "Rank", "Name"), "Rank Name");
    assert_eq!(format_two_strings("%s, %s!", "Rank", "Name"), "Rank, Name!");
    assert_eq!(format_two_strings("%s%s", "Rank", "Name"), "RankName");
    assert_eq!(format_two_strings("100%% %s %s", "R", "N"), "100% R N");
    assert_eq!(
        format_two_strings("no specifiers", "R", "N"),
        "no specifiers"
    );
    assert_eq!(format_two_strings("%s %s %s", "R", "N"), "R N ");
    assert_eq!(format_two_strings("%d %s %s", "R", "N"), "%d R N");
    assert_eq!(format_two_strings("trailing %", "R", "N"), "trailing %");
}

#[test]
fn has_inspect_honor_data_latches_on_the_push_and_clears_with_it() {
    let mut s = seated();
    let held = || r#"return HasInspectHonorData() and true or false"#;

    assert!(!s.eval::<bool>(held()).unwrap());
    s.set_inspect_honor(Some(inspect_state()));
    assert!(s.eval::<bool>(held()).unwrap());

    // The reference pushes the number 1 (`lua_pushnumber(1.0)`), not a boolean.
    assert_eq!(
        s.eval::<String>("return type(HasInspectHonorData())")
            .unwrap(),
        "number"
    );
    assert_eq!(
        s.eval::<String>("return tostring(HasInspectHonorData())")
            .unwrap(),
        "1"
    );

    s.set_inspect_honor(None);
    assert!(!s.eval::<bool>(held()).unwrap());
}

/// The reference's own six-wide destructure.
#[test]
fn get_inspect_honor_data_returns_the_six_in_the_reference_order() {
    let mut s = seated();
    s.set_inspect_honor(Some(inspect_state()));
    assert_eq!(s.arity("GetInspectHonorData()").unwrap(), 6);
    assert_eq!(
        s.eval::<Vec<i64>>("return {GetInspectHonorData()}")
            .unwrap(),
        [61, 62, 63, 64, 65, 12]
    );
}

/// `0x5e77c0` is ungated and reads zero-initialised slots.
#[test]
fn get_inspect_honor_data_answers_six_zeros_when_no_reply_is_held() {
    let s = seated();
    assert_eq!(
        s.eval::<Vec<i64>>("return {GetInspectHonorData()}")
            .unwrap(),
        vec![0; 6]
    );
}

/// Eighteen values for a cached team, nothing for an empty slot.
#[test]
fn get_inspect_arena_team_data_answers_eighteen_or_nothing() {
    let mut s = seated();
    assert_eq!(s.arity("GetInspectArenaTeamData(1)").unwrap(), 0);
    s.set_inspect_arena_teams([
        Some(InspectArenaTeamView {
            name: "Probes".into(),
            team_size: 2,
            rating: 1600,
            games: 20,
            wins: 12,
            played: 18,
            personal_rating: 1580,
            background: [1.0, 0.5, 0.0],
            emblem: 7,
            emblem_color: [0.0, 0.0, 1.0],
            border: 3,
            border_color: [0.2, 0.2, 0.2],
        }),
        None,
        None,
    ]);
    assert_eq!(s.arity("GetInspectArenaTeamData(1)").unwrap(), 18);
    assert_eq!(
        s.eval::<String>(
            "local n, size, rating, a, b, c, personal = GetInspectArenaTeamData(1)              return n .. size .. rating .. personal"
        )
        .unwrap(),
        "Probes216001580"
    );
    assert_eq!(s.arity("GetInspectArenaTeamData(2)").unwrap(), 0);
}

/// The reference refuses a query while one is in flight and once data is held; `TogglePVP` has no
/// latch.
#[test]
fn the_intent_queues_drain_and_the_honor_query_refuses_to_double_up() {
    let mut s = seated();
    assert_eq!(s.take_inspect_honor_requests(), 0);

    // Two calls, one query: the second sees `pending`.
    s.eval::<()>("RequestInspectHonorData() RequestInspectHonorData()")
        .unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 1);
    assert_eq!(s.take_inspect_honor_requests(), 0);

    // Still latched after the drain: sent, not yet answered.
    s.eval::<()>("RequestInspectHonorData()").unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 0, "still in flight");

    // The reply clears `pending` and sets `hasData`, which refuses the next ask instead.
    s.set_inspect_honor(Some(inspect_state()));
    s.eval::<()>("RequestInspectHonorData()").unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 0, "data already held");

    // Dropping the inspected player clears both, and the next ask goes out.
    s.set_inspect_honor(None);
    s.eval::<()>("RequestInspectHonorData()").unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 1);

    // The binding answers zero Lua values on every one of those paths.
    assert_eq!(s.arity("RequestInspectHonorData()").unwrap(), 0);

    s.eval::<()>("TogglePVP() TogglePVP()").unwrap();
    assert_eq!(s.take_pvp_toggles(), 2, "no latch here — two packets");
    assert_eq!(s.take_pvp_toggles(), 0);
}
