use std::sync::Arc;

use super::*;
use crate::script::live::{lending, TestView};
use crate::script::UiScript;

/// Categories: 92 General (root), 1 Statistics, 130 Character (a statistic category under 1).
/// Achievements: 6 and 7 a series in 92 (7 supersedes 6), 8 an earned-only feat in 81, 9 a
/// two-criteria progress-bar row in 92, 10 a statistic in 130, 11 hidden and uncategorised, 12
/// Horde-only. Criterion 103 of 12 is timed.
fn db() -> AchievementDb {
    db_with(|_| {})
}

/// [`db`] with `edit` over the achievement rows.
fn db_with(edit: impl FnOnce(&mut Vec<AchievementRow>)) -> AchievementDb {
    let row = |id, category, points, ui_order, flags| AchievementRow {
        id,
        faction: -1,
        title: format!("A{id}"),
        description: format!("D{id}"),
        category,
        points,
        ui_order,
        flags,
        icon: format!("Interface\\Icons\\I{id}"),
        ..Default::default()
    };
    let mut rows = vec![
        row(6, 92, 10, 1, 0),
        AchievementRow {
            supercedes: 6,
            ..row(7, 92, 10, 2, 0)
        },
        row(8, FEATS_OF_STRENGTH, 0, 1, 0),
        row(9, 92, 20, 3, flag::PROGRESS_BAR | flag::COUNT),
        row(10, 130, 0, 1, flag::STATISTIC | flag::SUM),
        row(11, -1, 0, 1, flag::HIDDEN),
        AchievementRow {
            faction: 0,
            ..row(12, 92, 5, 1, 0)
        },
    ];
    rows[3].minimum_criteria = 2;
    edit(&mut rows);
    let cat = |id, parent: i32| AchievementCategoryRow {
        id,
        parent,
        name: format!("C{id}"),
    };
    let crit = |id, achievement, quantity| AchievementCriteriaRow {
        id,
        achievement,
        quantity,
        description: format!("K{id}"),
        ..Default::default()
    };
    AchievementDb::new(
        rows,
        vec![cat(92, -1), cat(1, -1), cat(130, 1), cat(81, -1)],
        vec![
            crit(100, 9, 5),
            crit(101, 9, 3),
            crit(102, 10, 0),
            AchievementCriteriaRow {
                timer_time: 60,
                ..crit(103, 12, 1)
            },
        ],
    )
}

fn date(month0: u32, day0: u32, year: u32) -> u32 {
    WowDate::from_date(month0, day0, year).0
}

/// A VM with a test view lending the tables and the lists, as the app lends its own.
struct Rig {
    s: UiScript,
    view: TestView,
}

/// The handler host over the rig: the view's book, each raise fired with the view lent.
struct Host<'a> {
    rig: &'a mut Rig,
}

impl AchievementHost for Host<'_> {
    fn book(&mut self) -> &mut AchievementBook {
        &mut self.rig.view.achievements
    }

    fn raise(&mut self, (event, args): AchievementEvent) {
        let Rig { s, view } = &mut *self.rig;
        lending(view, || s.fire_event(event, args));
    }
}

impl Rig {
    fn eval<T: mlua::FromLuaMulti>(&mut self, code: &str) -> mlua::Result<T> {
        let Rig { s, view } = self;
        lending(view, || s.eval::<T>(code))
    }

    fn run(&mut self, code: &str) -> mlua::Result<()> {
        let Rig { s, view } = self;
        lending(view, || s.run(code))
    }

    /// One packet through the handler's steps.
    fn apply(&mut self, update: AchievementUpdate) {
        let db = self.view.achievement_db.clone().expect("tables");
        handle_achievement_packet(&db, update, &mut Host { rig: self });
    }

    fn cvar(&self, name: &str) -> Option<String> {
        self.s.cvar(name)
    }
}

fn script() -> Rig {
    let s = UiScript::new().unwrap();
    s.register_cvars([("trackedAchievements", "")]);
    s.run(
        r#"
        log = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"RECEIVED_ACHIEVEMENT_LIST", "ACHIEVEMENT_EARNED", "CRITERIA_UPDATE",
            "TRACKED_ACHIEVEMENT_UPDATE", "INSPECT_ACHIEVEMENT_READY"}) do f:RegisterEvent(e) end
        f:SetScript("OnEvent", function(self, e, ...)
            local a = {...}
            for i = 1, #a do a[i] = tostring(a[i]) end
            table.insert(log, e .. "(" .. table.concat(a, ",") .. ")")
        end)
    "#,
    )
    .unwrap();
    Rig {
        s,
        view: TestView {
            achievement_db: Some(Arc::new(db())),
            ..Default::default()
        },
    }
}

fn log(s: &mut Rig) -> String {
    s.eval::<String>("local l = table.concat(log, ' '); log = {}; return l")
        .unwrap()
}

fn list(s: &mut Rig, achievements: Vec<(u32, u32)>, criteria: Vec<CriteriaWire>) {
    s.apply(AchievementUpdate::List {
        achievements,
        criteria,
        now: 1000,
    });
}

fn progress(id: u32, counter: u64, d: u32) -> CriteriaWire {
    CriteriaWire {
        id,
        counter,
        date: d,
        ..Default::default()
    }
}

#[test]
fn the_completed_list_runs_newest_first_and_same_dates_by_ui_order() {
    let mut s = script();
    let same = date(1, 2, 8);
    list(
        &mut s,
        vec![(6, date(0, 0, 8)), (12, same), (7, same), (999, same)],
        Vec::new(),
    );
    let ids: Vec<u32> = s.view.achievements.completed.iter().map(|d| d.id).collect();
    // 999 is unknown and refused; 7 (order 2) leads 12 (order 1) on the same date.
    assert_eq!(ids, vec![7, 12, 6]);
}

#[test]
fn the_list_fires_criteria_then_received_and_answers_the_summary_verbs() {
    let mut s = script();
    assert!(!s.eval::<bool>("return CanShowAchievementUI()").unwrap());
    list(&mut s, vec![(6, date(0, 4, 8))], vec![progress(100, 2, 0)]);
    assert_eq!(log(&mut s), "CRITERIA_UPDATE() RECEIVED_ACHIEVEMENT_LIST()");
    assert!(s.eval::<bool>("return CanShowAchievementUI()").unwrap());
    assert!(s
        .eval::<bool>("return HasCompletedAnyAchievement()")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return GetTotalAchievementPoints()").unwrap(),
        10
    );
    // Real achievements bar 81, hidden and earned-only: 6, 7, 9, 12.
    assert_eq!(
        s.eval::<(i64, i64)>("return GetNumCompletedAchievements()")
            .unwrap(),
        (4, 1)
    );
    assert_eq!(
        s.eval::<String>("return table.concat({GetLatestCompletedAchievements()}, ',')")
            .unwrap(),
        "6"
    );
}

/// The faction filter reads the active player and the compared player at the call (`0x5b3d0a`,
/// `0x5b3bf0`): no player object held reads every row, an Alliance player drops the Horde row 12.
#[test]
fn the_faction_filter_reads_the_held_players_at_the_call() {
    let mut s = script();
    list(
        &mut s,
        vec![(6, date(0, 4, 8)), (12, date(0, 4, 8))],
        Vec::new(),
    );
    let count = "return GetNumCompletedAchievements()";
    assert_eq!(s.eval::<(i64, i64)>(count).unwrap(), (4, 2));
    s.view.player = Some(0x10);
    s.view.player_factions.insert(0x10, 1);
    assert_eq!(s.eval::<(i64, i64)>(count).unwrap(), (3, 1));
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCategoryNumAchievements(92)")
            .unwrap(),
        (3, 1)
    );
    // The compared player's side, read when the client holds that player.
    s.view.achievements.inspect_guid = 0x77;
    s.view.achievements.inspect_completed = s.view.achievements.completed.clone();
    let compared = "return GetNumComparisonCompletedAchievements()";
    assert_eq!(s.eval::<(i64, i64)>(compared).unwrap(), (4, 2));
    s.view.player_factions.insert(0x77, 0);
    assert_eq!(s.eval::<(i64, i64)>(compared).unwrap(), (4, 2));
    s.view.player_factions.insert(0x77, 1);
    assert_eq!(s.eval::<(i64, i64)>(compared).unwrap(), (3, 1));
}

#[test]
fn the_category_lists_split_at_the_statistics_root() {
    let mut s = script();
    assert_eq!(
        s.eval::<String>("return table.concat(GetCategoryList(), ',')")
            .unwrap(),
        "92,81"
    );
    assert_eq!(
        s.eval::<String>("return table.concat(GetStatisticsCategoryList(), ',')")
            .unwrap(),
        "130"
    );
    assert_eq!(
        s.eval::<(String, i64, i64)>("return GetCategoryInfo(130)")
            .unwrap(),
        ("C130".into(), -1, 0)
    );
}

#[test]
fn a_category_lists_a_series_by_its_first_open_step() {
    let mut s = script();
    // Nothing earned: 6 (then 7 skipped as its successor), 9, 12; 11 is hidden.
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCategoryNumAchievements(92)")
            .unwrap(),
        (3, 0)
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {} for i = 1, 3 do t[i] = (GetAchievementInfo(92, i)) end \
             return table.concat(t, ',')"
        )
        .unwrap(),
        "6,9,12"
    );
    // 6 earned: 6 lists as completed and 7 opens.
    list(&mut s, vec![(6, date(0, 4, 8))], Vec::new());
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCategoryNumAchievements(92)")
            .unwrap(),
        (4, 1)
    );
    assert_eq!(
        s.eval::<String>(
            "local t = {} for i = 1, 4 do t[i] = (GetAchievementInfo(92, i)) end \
             return table.concat(t, ',')"
        )
        .unwrap(),
        "6,7,9,12"
    );
    let (id, name, points, done, month, day, year): (i64, String, i64, bool, i64, i64, i64) =
        s.eval("return GetAchievementInfo(6)").unwrap();
    assert_eq!((id, name.as_str(), points, done), (6, "A6", 10, true));
    assert_eq!((month, day, year), (1, 5, 8));
    // 7 earned too: 6 is superseded and drops from the category.
    list(&mut s, vec![(7, date(0, 5, 8))], Vec::new());
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCategoryNumAchievements(92)")
            .unwrap(),
        (3, 1)
    );
    // The feat lists only once earned; a hidden row answers nothing.
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCategoryNumAchievements(81)")
            .unwrap(),
        (0, 0)
    );
    assert!(s
        .eval::<Option<i64>>("return GetAchievementInfo(11)")
        .unwrap()
        .is_none());
}

#[test]
fn a_criteria_update_moves_the_progress_bar_and_its_events() {
    let mut s = script();
    list(&mut s, Vec::new(), vec![progress(100, 2, 0)]);
    log(&mut s);
    let (desc, _, done, quantity, req, _, flags, _, text, id): (
        String,
        i64,
        bool,
        i64,
        i64,
        Option<String>,
        i64,
        i64,
        String,
        i64,
    ) = s.eval("return GetAchievementCriteriaInfo(9, 1)").unwrap();
    assert_eq!((desc.as_str(), done, id), ("K100", false, 100));
    // A progress-bar row of "2 of these": the bar reads met criteria out of the minimum.
    assert_eq!((quantity, req, flags & 1), (0, 2, 1));
    assert_eq!(text, "0 / 2");
    assert_eq!(
        s.eval::<i64>("return GetAchievementNumCriteria(9)")
            .unwrap(),
        3
    );

    s.run("AddTrackedAchievement(9)").unwrap();
    assert_eq!(log(&mut s), "TRACKED_ACHIEVEMENT_UPDATE(9)");
    s.apply(AchievementUpdate::Criteria {
        criterion: progress(100, 5, 0),
        now: 1000,
    });
    assert_eq!(
        log(&mut s),
        "CRITERIA_UPDATE() TRACKED_ACHIEVEMENT_UPDATE(9)"
    );
    let text: String = s
        .eval("return select(9, GetAchievementCriteriaInfo(9, 1))")
        .unwrap();
    assert_eq!(text, "1 / 2");
    let met: bool = s
        .eval("return select(3, GetAchievementCriteriaInfo(100))")
        .unwrap();
    assert!(met);

    s.apply(AchievementUpdate::CriteriaDeleted(100));
    assert_eq!(log(&mut s), "CRITERIA_UPDATE()");
}

#[test]
fn earning_untracks_and_fires_the_toast_event() {
    let mut s = script();
    s.run("AddTrackedAchievement(6)").unwrap();
    log(&mut s);
    s.apply(AchievementUpdate::Earned {
        player: 1,
        id: 6,
        date: date(0, 0, 8),
        silent: false,
        own: true,
    });
    assert_eq!(
        log(&mut s),
        "TRACKED_ACHIEVEMENT_UPDATE(6) ACHIEVEMENT_EARNED(6)"
    );
    assert_eq!(
        s.eval::<i64>("return GetNumTrackedAchievements()").unwrap(),
        0
    );
    // Another player's award fires nothing; a silent one adds without the event.
    s.apply(AchievementUpdate::Earned {
        player: 2,
        id: 7,
        date: 0,
        silent: false,
        own: false,
    });
    s.apply(AchievementUpdate::Earned {
        player: 1,
        id: 9,
        date: 0,
        silent: true,
        own: true,
    });
    assert_eq!(log(&mut s), "");
    assert!(s
        .eval::<bool>("return select(4, GetAchievementInfo(9))")
        .unwrap());
    s.apply(AchievementUpdate::AchievementDeleted(9));
    assert_eq!(log(&mut s), "ACHIEVEMENT_EARNED(9)");
}

#[test]
fn the_tracked_list_caps_at_ten_and_round_trips_its_cvar() {
    let mut s = script();
    for id in [6, 7, 9, 10, 12] {
        s.run(&format!("AddTrackedAchievement({id})")).unwrap();
    }
    assert!(s.run("AddTrackedAchievement(4242)").is_err(), "unknown id");
    assert!(s.run("RemoveTrackedAchievement(0)").is_err());
    s.run("RemoveTrackedAchievement(7)").unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat({GetTrackedAchievements()}, ',')")
            .unwrap(),
        "6,9,10,12"
    );
    assert!(s.eval::<bool>("return IsTrackedAchievement(9)").unwrap());
    // The CVar moves only at the UI shutdown's save (`0x528f00` -> `0x5b1fa0`).
    assert_eq!(s.cvar("trackedAchievements").as_deref(), Some(""));
    {
        let Rig { s, view } = &mut s;
        lending(view, || s.save_tracked_achievements());
    }
    let saved = s.cvar("trackedAchievements").unwrap();
    assert_eq!(decode_tracked(&saved), vec![6, 9, 10, 12]);
    // A new game's load (`0x5b3840`) adds each id back, each add announced.
    let mut fresh = script();
    fresh.s.set_cvar_engine("trackedAchievements", &saved);
    {
        let Rig { s, view } = &mut fresh;
        lending(view, || s.load_tracked_achievements());
    }
    assert_eq!(fresh.view.achievements.tracked(), &[6, 9, 10, 12]);
    assert_eq!(
        log(&mut fresh),
        "TRACKED_ACHIEVEMENT_UPDATE(6) TRACKED_ACHIEVEMENT_UPDATE(9) \
         TRACKED_ACHIEVEMENT_UPDATE(10) TRACKED_ACHIEVEMENT_UPDATE(12)"
    );
    // A reload's load keeps the held list and re-announces each id (`0x5b241b`).
    {
        let Rig { s, view } = &mut fresh;
        lending(view, || s.load_tracked_achievements());
    }
    assert_eq!(fresh.view.achievements.tracked(), &[6, 9, 10, 12]);

    let mut ids = Vec::new();
    for id in 1..=12 {
        add_tracked(&mut ids, id);
    }
    assert_eq!(ids.len(), 10);
    assert_eq!(encode_tracked(&[1, 93 * 93]), "v\u{1}##$$##");
    // The read takes any other tag as it is (`0x766f00` returns), and stops at an id of 0.
    assert_eq!(decode_tracked("v\u{2}##$###$##"), vec![1]);
    // Each byte is sign-extended (`0x766fcd`): `C3 A9 23` is (-96 * 93 - 122) * 93 + 0.
    assert_eq!(
        decode_tracked("v\u{1}\u{e9}#"),
        vec![((-96i32 * 93 - 122) * 93) as u32]
    );
    // An untagged value is the version-0 form, upgraded under the tag (`0x766f00`).
    let v0 = format!("#{}{}", char::from(36u8), char::from(35u8 + 5));
    assert_eq!(decode_tracked(&v0), vec![220 + 5]);
    assert_eq!(
        upgrade_tracked(&v0).as_deref(),
        Some(encode_tracked(&[225]).as_str())
    );
    assert_eq!(upgrade_tracked("").as_deref(), Some("v\u{1}"));
    assert_eq!(upgrade_tracked("v\u{1}##$"), None);
}

#[test]
fn a_statistic_reads_its_criteria_sum() {
    let mut s = script();
    assert_eq!(s.eval::<String>("return GetStatistic(10)").unwrap(), "--");
    list(&mut s, Vec::new(), vec![progress(102, 42, 0)]);
    assert_eq!(s.eval::<String>("return GetStatistic(10)").unwrap(), "42");
    assert_eq!(
        s.eval::<String>("return table.concat({GetLatestUpdatedStats()}, ',')")
            .unwrap(),
        "102"
    );
}

#[test]
fn the_comparison_unit_queues_a_query_and_reads_the_inspected_list() {
    let mut s = script();
    s.s.set_unit_guids(&crate::script::UnitGuids {
        target: 0x77,
        ..Default::default()
    });
    assert_eq!(
        s.eval::<i64>("return SetAchievementComparisonUnit('target')")
            .unwrap(),
        1
    );
    // The query goes out inside the call (`0x5b1ab0`).
    assert_eq!(s.view.inspect_achievement_queries, vec![0x77]);
    assert_eq!(s.view.achievements.inspect_guid, 0x77);
    s.apply(AchievementUpdate::Inspect {
        guid: 0x77,
        achievements: vec![(6, date(2, 3, 9))],
        criteria: Vec::new(),
        now: 0,
    });
    assert_eq!(log(&mut s), "INSPECT_ACHIEVEMENT_READY()");
    assert_eq!(
        s.eval::<(bool, i64, i64, i64)>("return GetAchievementComparisonInfo(6)")
            .unwrap(),
        (true, 3, 4, 9)
    );
    assert_eq!(
        s.eval::<i64>("return GetComparisonAchievementPoints()")
            .unwrap(),
        10
    );
    s.run("ClearAchievementComparisonUnit()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetComparisonAchievementPoints()")
            .unwrap(),
        0
    );
}

#[test]
fn the_link_names_the_player_and_marks_met_criteria() {
    let mut s = script();
    s.view.player = Some(0x2a);
    list(&mut s, Vec::new(), vec![progress(101, 3, 0)]);
    let link: String = s.eval("return GetAchievementLink(9)").unwrap();
    assert_eq!(
        link,
        "|cffffff00|Hachievement:9:000000000000002A:0:0:0:-1:2:0:0:0|h[A9]|h|r"
    );
    let parsed = tooltip::parse_achievement_link(&link).unwrap();
    assert_eq!((parsed.id, parsed.guid, parsed.bits[0]), (9, 0x2a, 2));
    list(&mut s, vec![(6, date(0, 4, 8))], Vec::new());
    let link: String = s.eval("return GetAchievementLink(6)").unwrap();
    assert!(link.contains(":1:1:5:8:4294967295:"), "{link}");
}

/// The list's timed tracker update fires as its criterion lands (`0x5b3454`), before the next
/// criterion is read in (`0x5b340b`) and before `CRITERIA_UPDATE` (`0x5b3478`): its listener reads
/// the lists as of that step.
#[test]
fn a_timed_criterion_in_the_list_fires_before_the_next_criterion_lands() {
    let mut s = script();
    s.run(
        r#"
        seen = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("TRACKED_ACHIEVEMENT_UPDATE")
        f:RegisterEvent("CRITERIA_UPDATE")
        f:SetScript("OnEvent", function(_, e)
            seen[#seen + 1] = e .. ":" .. GetStatistic(10)
        end)
        "#,
    )
    .unwrap();
    list(
        &mut s,
        Vec::new(),
        vec![progress(103, 1, 0), progress(102, 42, 0)],
    );
    assert_eq!(
        s.eval::<String>("return table.concat(seen, ' ')").unwrap(),
        "TRACKED_ACHIEVEMENT_UPDATE:-- CRITERIA_UPDATE:42"
    );
    // The timed update names the achievement, the criterion, the elapsed time and the limit.
    assert_eq!(
        log(&mut s),
        "TRACKED_ACHIEVEMENT_UPDATE(12,103,0,60) CRITERIA_UPDATE() RECEIVED_ACHIEVEMENT_LIST()"
    );
}

/// The criteria update's tracked walk saves each node's next link before its raise (`0x5b3298`): a
/// listener that untracks the achievement it hears about leaves the walk on the saved next one.
#[test]
fn the_criteria_walk_reads_the_tracked_list_a_listener_edits() {
    let mut s = script();
    // 7 reads 9's criteria too, so both hear about criterion 100.
    let db = db_with(|rows| {
        if let Some(a) = rows.iter_mut().find(|a| a.id == 7) {
            a.shares_criteria = 9;
        }
    });
    s.view.achievement_db = Some(Arc::new(db));
    s.run("AddTrackedAchievement(9) AddTrackedAchievement(7) AddTrackedAchievement(6)")
        .unwrap();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:RegisterEvent("TRACKED_ACHIEVEMENT_UPDATE")
        f:SetScript("OnEvent", function(_, e, id)
            if id == 9 then RemoveTrackedAchievement(9) end
        end)
        "#,
    )
    .unwrap();
    log(&mut s);
    s.apply(AchievementUpdate::Criteria {
        criterion: progress(100, 1, 0),
        now: 1000,
    });
    // 9's update, its listener's removal of 9, then 7's: 6 reads none of 100's achievement.
    assert_eq!(
        log(&mut s),
        "CRITERIA_UPDATE() TRACKED_ACHIEVEMENT_UPDATE(9) TRACKED_ACHIEVEMENT_UPDATE(9) \
         TRACKED_ACHIEVEMENT_UPDATE(7)"
    );
    assert_eq!(s.view.achievements.tracked(), &[7, 6]);
}

/// The walk resumes from the link saved before the raise (`0x5b3298`, `0x5b32e0`): a node a
/// listener appends behind the last one is never visited, so its only update is the add's
/// (`0x5b24b6`).
#[test]
fn the_criteria_walk_ends_at_the_saved_link_past_a_listener_append() {
    let mut s = script();
    // 7 reads 9's criteria, so it hears about criterion 100.
    let db = db_with(|rows| {
        if let Some(a) = rows.iter_mut().find(|a| a.id == 7) {
            a.shares_criteria = 9;
        }
    });
    s.view.achievement_db = Some(Arc::new(db));
    s.run("AddTrackedAchievement(7)").unwrap();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:RegisterEvent("TRACKED_ACHIEVEMENT_UPDATE")
        f:SetScript("OnEvent", function(_, e, id)
            if id == 7 then AddTrackedAchievement(9) end
        end)
        "#,
    )
    .unwrap();
    log(&mut s);
    s.apply(AchievementUpdate::Criteria {
        criterion: progress(100, 1, 0),
        now: 1000,
    });
    // 7's update, the add's update of 9, and no walk update of 9.
    assert_eq!(
        log(&mut s),
        "CRITERIA_UPDATE() TRACKED_ACHIEVEMENT_UPDATE(7) TRACKED_ACHIEVEMENT_UPDATE(9)"
    );
    assert_eq!(s.view.achievements.tracked(), &[7, 9]);
}

/// An owner whose name is not cached leaves the tooltip as it was and asks with the tooltip's
/// callback (`0x627220`); the answer rebuilds only a tooltip still showing an achievement
/// (`0x61de50`), and a later set with the name cached fills it.
#[test]
fn an_uncached_owner_asks_and_its_answer_rebuilds_only_a_shown_achievement() {
    let mut s = script();
    s.s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        ACHIEVEMENT_TOOLTIP_IN_PROGRESS = "In progress for %s"
        ACHIEVEMENT_TOOLTIP_COMPLETE = "Done by %s on %d/%d/%d"
        set = 0
        T = CreateFrame("GameTooltip", "T")
        CreateFrame("Frame", "Owner")
        T:SetOwner(Owner, "ANCHOR_PRESERVE")
        T:SetScript("OnTooltipSetAchievement", function() set = set + 1 end)
        "#,
    )
    .unwrap();
    let link = |id: u32, guid: u64| format!("achievement:{id}:{guid:016X}:0:0:0:-1:0:0:0:0");
    s.run(&format!("T:SetHyperlink({:?})", link(6, 0x2a)))
        .unwrap();
    assert_eq!(s.view.name_asks, vec![0x2a]);
    assert_eq!(s.eval::<i64>("return T:NumLines()").unwrap(), 0);
    // The answer finds no achievement shown: the callback builds nothing.
    s.view.player_names.insert(0x2a, "Probe".into());
    {
        let Rig { s, view } = &mut s;
        lending(view, || s.achievement_name_landed(0x2a, true));
    }
    assert_eq!(s.eval::<i64>("return set").unwrap(), 0);
    // Cached now: the set fills the tooltip.
    s.run(&format!("T:SetHyperlink({:?})", link(6, 0x2a)))
        .unwrap();
    assert_eq!(s.eval::<i64>("return set").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return TTextLeft3:GetText()").unwrap(),
        "In progress for Probe"
    );
    // Showing 6, a link of an uncached owner asks and leaves 6; the answer rebuilds 6.
    s.run(&format!("T:SetHyperlink({:?})", link(7, 0x2b)))
        .unwrap();
    assert_eq!(
        s.eval::<String>("return TTextLeft1:GetText()").unwrap(),
        "A6"
    );
    {
        let Rig { s, view } = &mut s;
        lending(view, || s.achievement_name_landed(0x2b, true));
    }
    assert_eq!(s.eval::<i64>("return set").unwrap(), 2);
    assert_eq!(
        s.eval::<String>("return TTextLeft1:GetText()").unwrap(),
        "A6"
    );
}

/// A pending name keeps one callback per tooltip (`0x67d84a`-`0x67d876`): the same uncached link
/// set twice rebuilds once on the answer; a zero guid asks nothing and leaves no wait (`0x67d783`).
#[test]
fn the_same_uncached_owner_set_twice_rebuilds_once_and_a_zero_guid_waits_on_nothing() {
    let mut s = script();
    s.s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        ACHIEVEMENT_TOOLTIP_IN_PROGRESS = "In progress for %s"
        set, cleared = 0, 0
        T = CreateFrame("GameTooltip", "T")
        CreateFrame("Frame", "Owner")
        T:SetOwner(Owner, "ANCHOR_PRESERVE")
        T:SetScript("OnTooltipSetAchievement", function() set = set + 1 end)
        T:SetScript("OnTooltipCleared", function() cleared = cleared + 1 end)
        "#,
    )
    .unwrap();
    let link = |id: u32, guid: u64| format!("achievement:{id}:{guid:016X}:0:0:0:-1:0:0:0:0");
    s.view.player_names.insert(0x2a, "Probe".into());
    s.run(&format!("T:SetHyperlink({:?})", link(6, 0x2a)))
        .unwrap();
    assert_eq!(s.eval::<i64>("return set").unwrap(), 1);
    s.run(&format!(
        "T:SetHyperlink({:?}) T:SetHyperlink({:?})",
        link(7, 0x2b),
        link(7, 0x2b)
    ))
    .unwrap();
    s.run("cleared = 0").unwrap();
    s.view.player_names.insert(0x2b, "Other".into());
    {
        let Rig { s, view } = &mut s;
        lending(view, || s.achievement_name_landed(0x2b, true));
    }
    assert_eq!(s.eval::<i64>("return set").unwrap(), 2);
    assert_eq!(s.eval::<i64>("return cleared").unwrap(), 1);
    // A zero guid: no ask, the tooltip as it was, nothing waiting.
    s.view.name_asks.clear();
    s.run(&format!("T:SetHyperlink({:?})", link(7, 0))).unwrap();
    assert!(s.view.name_asks.is_empty());
    assert_eq!(s.eval::<i64>("return set").unwrap(), 2);
    assert!(s.s.model_mut().achievement.name_waits.is_empty());
}
