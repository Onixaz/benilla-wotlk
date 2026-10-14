//! The stock 3.3.5a quest log (`QuestLogFrame.lua`, `QuestInfo.lua`, `WatchFrame.lua`) on the
//! production load, over a quest-log model as the live client pushes it: a zone header, a kill
//! quest in progress and a complete one.

use benilla_ui::script::{
    QuestItemView, QuestLogDetail, QuestLogEntryView, QuestLogObjectiveView, QuestLogState,
    UiScript,
};

/// Errors from the quest windows' own code; the production load carries unrelated 3.3.5a gaps.
fn quest_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Quest", "Watch"].iter().any(|k| e.contains(k)))
        .collect()
}

fn model() -> QuestLogState {
    let detail = QuestLogDetail {
        description: "Kobolds have infested the mine.".into(),
        objectives_text: "Kill 8 Kobold Vermin.".into(),
        reward_money: 125,
        rewards: vec![QuestItemView {
            item_id: 2024,
            name: Some("Militia Hammer".into()),
            count: 1,
            quality: 1,
            usable: true,
            ..Default::default()
        }],
        ..Default::default()
    };
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
            cur: 3,
            req: 8,
        }],
        detail: Some(detail.clone()),
        ..Default::default()
    };
    let done = QuestLogEntryView {
        quest_id: 2158,
        title: "Rest and Relaxation".into(),
        level: 5,
        complete: 1,
        detail: Some(detail),
        ..Default::default()
    };
    QuestLogState {
        entries: vec![header, kill, done],
        num_quests: 2,
        hidden_quest_ids: Vec::new(),
        daily_completed: 0,
    }
}

/// The L binding (`ToggleFrame(QuestLogFrame)`) opens the stock log over the model: the header
/// row wears its fold button, the quest rows carry their titles, and the selected quest's detail
/// pane and objective line draw, all without a script error.
#[test]
fn the_stock_quest_log_draws_the_model() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("quests", false, "", |_| {});
    // The first frame's layout, so the `OnSizeChanged` handlers it fires (`WatchFrame`'s update
    // among them) count with the load's errors.
    s.resolve();
    let _ = s.take_errors();
    s.set_quest_log(model());
    s.fire_event(
        "QUEST_LOG_UPDATE",
        Vec::<benilla_ui::script::ScriptValue>::new(),
    );

    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    s.run("QuestLog_SetSelection(2)").unwrap();
    s.run("QuestLog_Update()").unwrap();
    let errors = quest_errors(s.take_errors());
    assert!(errors.is_empty(), "quest log errors: {errors:#?}");
    check(&mut s);
}

fn check(s: &mut UiScript) {
    s.run(
        r#"
        assert(QuestLogFrame:IsShown(), "the log is open")
        local b = QuestLogScrollFrame.buttons
        assert(b[1]:GetText() == "Northshire Valley", "row 1 is the header: " .. tostring(b[1]:GetText()))
        local tex = b[1]:GetNormalTexture():GetTexture() or ""
        assert(tex:find("MinusButton"), "an open header wears the minus: " .. tex)
        assert(b[2]:GetText():find("Kobold Camp Cleanup"), "row 2 is the quest: " .. tostring(b[2]:GetText()))
        assert(not b[2]:GetNormalTexture() or (b[2]:GetNormalTexture():GetTexture() or "") == "",
            "a quest row wears no fold button")
        assert(not b[4]:IsShown(), "rows past the model hide")
        assert(QuestInfoTitleHeader:GetText() == "Kobold Camp Cleanup", "the detail names the quest")
        assert(QuestInfoObjective1:GetText():find("Kobold Vermin slain: 3/8"),
            "the objective line: " .. tostring(QuestInfoObjective1:GetText()))
        assert(not QuestLogDailyQuestCount:IsShown(),
            "no daily count with no daily done")
        "#,
    )
    .unwrap();
}
