use std::sync::Arc;

use super::*;
use crate::script::UiScript;

/// Categories: 92 General (root), 1 Statistics, 130 Character (a statistic category under 1).
/// Achievements: 6 and 7 a series in 92 (7 supersedes 6), 8 an earned-only feat in 81, 9 a
/// two-criteria progress-bar row in 92, 10 a statistic in 130, 11 hidden and uncategorised, 12
/// Horde-only.
fn db() -> AchievementDb {
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
        vec![crit(100, 9, 5), crit(101, 9, 3), crit(102, 10, 0)],
    )
}

fn date(month0: u32, day0: u32, year: u32) -> u32 {
    WowDate::from_date(month0, day0, year).0
}

fn script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.register_cvars([("trackedAchievements", "")]);
    s.set_achievement_db(Arc::new(db()));
    s.set_achievement_book(AchievementBook::default());
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
    s
}

fn log(s: &UiScript) -> String {
    s.eval::<String>("local l = table.concat(log, ' '); log = {}; return l")
        .unwrap()
}

fn list(s: &mut UiScript, achievements: Vec<(u32, u32)>, criteria: Vec<CriteriaWire>) {
    s.apply_achievement(AchievementUpdate::List {
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
    let db = db();
    let mut book = AchievementBook::default();
    let mut tracked = Vec::new();
    let same = date(1, 2, 8);
    book.apply(
        &db,
        AchievementUpdate::List {
            achievements: vec![(6, date(0, 0, 8)), (12, same), (7, same), (999, same)],
            criteria: Vec::new(),
            now: 0,
        },
        &mut tracked,
    );
    let ids: Vec<u32> = book.completed.iter().map(|d| d.id).collect();
    // 999 is unknown and refused; 7 (order 2) leads 12 (order 1) on the same date.
    assert_eq!(ids, vec![7, 12, 6]);
}

#[test]
fn the_list_fires_criteria_then_received_and_answers_the_summary_verbs() {
    let mut s = script();
    assert!(!s.eval::<bool>("return CanShowAchievementUI()").unwrap());
    list(&mut s, vec![(6, date(0, 4, 8))], vec![progress(100, 2, 0)]);
    assert_eq!(log(&s), "CRITERIA_UPDATE() RECEIVED_ACHIEVEMENT_LIST()");
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

#[test]
fn the_category_lists_split_at_the_statistics_root() {
    let s = script();
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
    log(&s);
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
    assert_eq!(log(&s), "TRACKED_ACHIEVEMENT_UPDATE(9)");
    s.apply_achievement(AchievementUpdate::Criteria {
        criterion: progress(100, 5, 0),
        now: 1000,
    });
    assert_eq!(log(&s), "CRITERIA_UPDATE() TRACKED_ACHIEVEMENT_UPDATE(9)");
    let text: String = s
        .eval("return select(9, GetAchievementCriteriaInfo(9, 1))")
        .unwrap();
    assert_eq!(text, "1 / 2");
    let met: bool = s
        .eval("return select(3, GetAchievementCriteriaInfo(100))")
        .unwrap();
    assert!(met);

    s.apply_achievement(AchievementUpdate::CriteriaDeleted(100));
    assert_eq!(log(&s), "CRITERIA_UPDATE()");
}

#[test]
fn earning_untracks_and_fires_the_toast_event() {
    let mut s = script();
    s.run("AddTrackedAchievement(6)").unwrap();
    log(&s);
    s.apply_achievement(AchievementUpdate::Earned {
        player: 1,
        id: 6,
        date: date(0, 0, 8),
        silent: false,
        own: true,
    });
    assert_eq!(
        log(&s),
        "TRACKED_ACHIEVEMENT_UPDATE(6) ACHIEVEMENT_EARNED(6)"
    );
    assert_eq!(
        s.eval::<i64>("return GetNumTrackedAchievements()").unwrap(),
        0
    );
    // Another player's award fires nothing; a silent one adds without the event.
    s.apply_achievement(AchievementUpdate::Earned {
        player: 2,
        id: 7,
        date: 0,
        silent: false,
        own: false,
    });
    s.apply_achievement(AchievementUpdate::Earned {
        player: 1,
        id: 9,
        date: 0,
        silent: true,
        own: true,
    });
    assert_eq!(log(&s), "");
    assert!(s
        .eval::<bool>("return select(4, GetAchievementInfo(9))")
        .unwrap());
    s.apply_achievement(AchievementUpdate::AchievementDeleted(9));
    assert_eq!(log(&s), "ACHIEVEMENT_EARNED(9)");
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
    let saved = s.cvar("trackedAchievements").unwrap();
    assert_eq!(decode_tracked(&saved), vec![6, 9, 10, 12]);
    // A fresh seed loads the CVar back.
    s.set_achievement_book(AchievementBook::default());
    assert_eq!(s.tracked_achievements(), vec![6, 9, 10, 12]);

    let mut ids = Vec::new();
    for id in 1..=12 {
        add_tracked(&mut ids, id);
    }
    assert_eq!(ids.len(), 10);
    assert_eq!(encode_tracked(&[1, 93 * 93]), "v\u{1}##$$##");
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
    s.set_unit_guids(&crate::script::UnitGuids {
        target: 0x77,
        ..Default::default()
    });
    assert_eq!(
        s.eval::<i64>("return SetAchievementComparisonUnit('target')")
            .unwrap(),
        1
    );
    assert_eq!(s.take_inspect_achievement_queries(), vec![0x77]);
    s.apply_achievement(AchievementUpdate::Inspect {
        guid: 0x77,
        achievements: vec![(6, date(2, 3, 9))],
        criteria: Vec::new(),
        now: 0,
    });
    assert_eq!(log(&s), "INSPECT_ACHIEVEMENT_READY()");
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
    s.set_achievement_player(0x2a);
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
