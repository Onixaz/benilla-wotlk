//! The stock 3.3.5a objective tracker (`WatchFrame.lua`) on the production load, over a quest log
//! as the live client pushes it and the install's achievement tables.

use std::sync::Arc;

use benilla_ui::script::{
    AchievementBook, AchievementDb, AchievementUpdate, QuestLogDetail, QuestLogEntryView,
    QuestLogObjectiveView, QuestLogState, ScriptValue,
};

/// Errors from the tracker's and the quest log's own code.
fn tracker_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Watch", "Quest"].iter().any(|k| e.contains(k)))
        .collect()
}

fn model() -> QuestLogState {
    let header = QuestLogEntryView {
        title: "Northshire Valley".into(),
        is_header: true,
        ..Default::default()
    };
    let kill = QuestLogEntryView {
        quest_id: 7,
        title: "Kobold Camp Cleanup".into(),
        level: 2,
        objectives: vec![QuestLogObjectiveView {
            text: "Kobold Vermin slain: 3/8".into(),
            kind: "monster".into(),
            finished: false,
            intermediate: false,
            cur: 3,
            req: 8,
        }],
        detail: Some(QuestLogDetail::default()),
        poi_objectives: 1,
        ..Default::default()
    };
    let done = QuestLogEntryView {
        quest_id: 2158,
        title: "Rest and Relaxation".into(),
        level: 5,
        complete: 1,
        detail: Some(QuestLogDetail::default()),
        completion_text: "Return to Innkeeper Farley.".into(),
        poi_objectives: u32::MAX,
        ..Default::default()
    };
    QuestLogState {
        entries: vec![header, kill, done],
        num_quests: 2,
        daily_completed: 0,
    }
}

/// Two shift-clicked quests and a tracked achievement draw in the stock tracker after the world
/// entry's events: the header counts all three, the kill quest shows its reversed objective, the
/// complete one its completion text, and nothing latches `WatchFrame.updating`.
#[test]
fn the_stock_tracker_draws_watched_quests_and_a_tracked_achievement() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let catalog = benilla_formats::load_achievement_catalog(&mut chain).unwrap();
    // An unearned, listed achievement with points, tracked by id.
    let tracked = catalog
        .achievements
        .iter()
        .find(|a| a.faction == -1 && a.flags & 0x8a3 == 0 && a.category > 0 && a.points > 0)
        .expect("a listed achievement")
        .id;
    let db: Arc<AchievementDb> = Arc::new(crate::ui_achievement::db_of(catalog));

    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("watchframe", false, "", |_| {});
    s.resolve();
    let _ = s.take_errors();
    s.set_achievement_db(db);
    s.set_achievement_book(AchievementBook::default());
    s.apply_achievement(AchievementUpdate::List {
        achievements: Vec::new(),
        criteria: Vec::new(),
        now: 0,
    });
    for event in ["VARIABLES_LOADED", "PLAYER_ENTERING_WORLD"] {
        s.fire_event(event, Vec::<ScriptValue>::new());
    }
    s.set_quest_log(model());
    s.run(&format!(
        "AddQuestWatch(2) AddQuestWatch(3) AddTrackedAchievement({tracked})"
    ))
    .unwrap();
    s.fire_event("QUEST_LOG_UPDATE", Vec::<ScriptValue>::new());
    s.resolve();
    let errors = tracker_errors(s.take_errors());
    assert!(errors.is_empty(), "tracker errors: {errors:#?}");

    let lines: Vec<String> = s
        .eval(
            r#"local t = {}
            for _, l in ipairs({ WatchFrameLines:GetChildren() }) do
                if l:IsShown() and l.text and l.text:GetText() then
                    t[#t + 1] = l.text:GetText()
                end
            end
            return t"#,
        )
        .unwrap();
    s.run(
        r#"
        assert(WatchFrame.updating == nil, "the update finished")
        assert(WatchFrameHeader:IsShown(), "the header shows")
        local title = WatchFrameTitle:GetText()
        assert(title == OBJECTIVES_TRACKER_LABEL .. " (3)", "header count: " .. tostring(title))
        "#,
    )
    .unwrap();
    let achievement: String = s
        .eval(&format!("return select(2, GetAchievementInfo({tracked}))"))
        .unwrap();
    for want in [
        achievement.as_str(),
        "Kobold Camp Cleanup",
        "3/8 Kobold Vermin slain",
        "Rest and Relaxation",
        "Return to Innkeeper Farley.",
    ] {
        assert!(
            lines.iter().any(|l| l == want),
            "no tracker line {want:?} in {lines:#?}"
        );
    }

    // Unwatching drops the quest's lines and the count.
    s.run("RemoveQuestWatch(2)").unwrap();
    s.fire_event("QUEST_LOG_UPDATE", Vec::<ScriptValue>::new());
    s.run(
        r#"assert(WatchFrameTitle:GetText() == OBJECTIVES_TRACKER_LABEL .. " (2)",
            WatchFrameTitle:GetText())"#,
    )
    .unwrap();
    let errors = tracker_errors(s.take_errors());
    assert!(errors.is_empty(), "tracker errors: {errors:#?}");
}

/// `QUEST_ACCEPTED` with `autoQuestWatch` at its default watches the quest, as
/// `QuestLog_OnEvent` adds it (`QuestLogFrame.lua:260-264`).
#[test]
fn an_accepted_quest_is_watched_by_the_stock_log() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("watchaccept", false, "", |_| {});
    s.fire_event("VARIABLES_LOADED", Vec::<ScriptValue>::new());
    s.set_quest_log(model());
    let _ = s.take_errors();
    s.fire_event("QUEST_ACCEPTED", vec![ScriptValue::Int(2)]);
    assert_eq!(s.quest_log_watched(), vec![7]);
    let errors = tracker_errors(s.take_errors());
    assert!(errors.is_empty(), "accept errors: {errors:#?}");
}
