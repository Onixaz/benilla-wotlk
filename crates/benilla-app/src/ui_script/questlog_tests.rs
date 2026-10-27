//! The quest log window on the whole 3.3.5a interface: stock `QuestLogFrame.xml` (a
//! `HybridScrollFrame` list and its control panel), the detail drawn through `QuestInfo.lua`, and
//! the `WatchFrame` tracker, fed a synthetic log.

use benilla_ui::script::{
    PartyMemberInfo, PartyState, QuadContent, QuestItemView, QuestLogDetail, QuestLogEntryView,
    QuestLogObjectiveView, QuestLogState, SoundRequest, UiScript,
};

use super::test_ui::stock_ui;

thread_local! {
    /// The detail every fixture row carries, so any selection reads it.
    static DETAIL_FIXTURE: QuestLogDetail = QuestLogDetail {
            description: "Speak with Marshal McBride.".into(),
            objectives_text: "Report to Marshal McBride.".into(),
            required_money: 0,
            reward_money: 40,
            choices: vec![],
            rewards: vec![QuestItemView {
                item_id: 2024,
                name: Some("Militia Hammer".into()),
                texture: Some("Interface\\Icons\\INV_Hammer_15".into()),
                count: 1,
                quality: 1,
                usable: true,
                // What `GetQuestLogItemLink` returns for a ctrl or shift click.
                link: Some(HAMMER_LINK.into()),
            }],
            reward_spell: None,
            extras: Default::default(),
        };
}

/// `n` quests, one header-free list; only quest 1 has an objective, and each has its own
/// `quest_id`, the watch set's key.
fn entries(n: u32) -> QuestLogState {
    let entries = (1..=n)
        .map(|i| QuestLogEntryView {
            quest_id: i,
            title: format!("Quest {i}"),
            level: 5,
            complete: 0,
            objectives: if i == 1 {
                vec![QuestLogObjectiveView {
                    text: "Kobold Vermin slain: 3/10".into(),
                    kind: "monster".into(),
                    finished: false,
                    cur: 3,
                    req: 10,
                }]
            } else {
                vec![]
            },
            detail: Some(DETAIL_FIXTURE.with(Clone::clone)),
            ..Default::default()
        })
        .collect();
    QuestLogState {
        num_quests: n,
        entries,
        hidden_quest_ids: Vec::new(),
        daily_completed: 0,
    }
}

/// Eight quests, fewer than the list's rows.
fn eight_entries() -> QuestLogState {
    entries(8)
}

/// The fixture reward's escaped link, a white (quality 1) item.
const HAMMER_LINK: &str = "|cffffffff|Hitem:2024:0:0:0|h[Militia Hammer]|h|r";

/// The whole interface with `log` pushed and the window opened by the `L` binding's body,
/// `ToggleFrame(QuestLogFrame)` (`Bindings.xml:659-661`).
fn open_log(log: QuestLogState) -> UiScript {
    let mut s = stock_ui(1024.0, 768.0);
    s.set_quest_log(log);
    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    s
}

/// The centre of the `i`th list row, `QuestLogScrollFrameButton<i>` (`HybridScrollFrame.lua:176`).
fn row_centre(s: &mut UiScript, i: u32) -> (f32, f32) {
    s.resolve();
    s.eval::<(f32, f32)>(&format!(
        "local b = QuestLogScrollFrameButton{i} \
         return (b:GetLeft() + b:GetRight()) / 2, (b:GetTop() + b:GetBottom()) / 2"
    ))
    .unwrap()
}

/// A left click at `at`, with shift held when `shift`.
fn click_at(s: &mut UiScript, at: (f32, f32), shift: bool) {
    s.set_modifiers(shift, false, false);
    s.mouse_button(at.0, at.1, "LeftButton", true);
    s.mouse_button(at.0, at.1, "LeftButton", false);
    s.set_modifiers(false, false, false);
}

/// Whether any drawn text quad reads `text` exactly.
fn draws_text(s: &mut UiScript, text: &str) -> bool {
    s.resolve();
    s.extract()
        .iter()
        .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == text))
}

/// The list builds its rows at load: one per 16 px of its 335 px, plus one
/// (`QuestLogFrame.lua:227-231`, `HybridScrollFrame.lua:198`).
#[test]
fn shipped_questlog_frame_loads_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = stock_ui(1024.0, 768.0);
    assert!(!s.eval::<bool>("return QuestLogFrame:IsShown()").unwrap());
    assert_eq!(
        s.eval::<i64>("return #QuestLogScrollFrame.buttons")
            .unwrap(),
        22
    );
    assert!(s
        .eval::<bool>(
            "return QuestLogScrollFrameButton22 ~= nil and QuestLogScrollFrameButton23 == nil"
        )
        .unwrap());
}

/// Open, read the list and the detail, wheel the list, abandon through the confirm, close.
#[test]
fn shipped_questlog_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    // More quests than the 22 rows, so the list scrolls.
    s.set_quest_log(entries(25));
    s.take_sounds();
    assert!(!s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());

    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());
    assert!(
        s.take_sounds()
            .contains(&SoundRequest::KitName("igQuestLogOpen".into())),
        "opening the quest log plays igQuestLogOpen (`QuestLogFrame.lua:288`)"
    );

    // With no selection the first quest selects itself (`QuestLogFrame.lua:349-357`).
    assert_eq!(
        s.eval::<String>("return QuestLogScrollFrameButton1NormalText:GetText()")
            .unwrap(),
        "  Quest 1"
    );
    // MAX_QUESTLOG_QUESTS is 25 (`QuestLogFrame.lua:4`).
    assert_eq!(
        s.eval::<String>("return QuestLogQuestCount:GetText()")
            .unwrap(),
        "Quests: |cffffffff25/25|r"
    );
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);

    // The detail is `QUEST_TEMPLATE_LOG` through `QuestInfo_Display` (`QuestLogFrame.lua:543`).
    let text = |s: &UiScript, name: &str| {
        s.eval::<String>(&format!("return {name}:GetText()"))
            .unwrap()
    };
    assert_eq!(text(&s, "QuestInfoTitleHeader"), "Quest 1");
    assert_eq!(text(&s, "QuestInfoObjective1"), "Kobold Vermin slain: 3/10");
    assert_eq!(text(&s, "QuestInfoItem1Name"), "Militia Hammer");
    assert_eq!(text(&s, "QuestInfoDescriptionHeader"), "Description");
    assert!(s
        .eval::<bool>("return QuestInfoRewardsFrame:IsShown()")
        .unwrap());
    // No choices: REWARD_ITEMS_ONLY, and the choose text hidden (`QuestInfo.lua:297-299`, `:390`).
    assert_eq!(text(&s, "QuestInfoItemReceiveText"), "You will receive:");
    assert!(!s
        .eval::<bool>("return QuestInfoItemChooseText:IsShown()")
        .unwrap());
    assert!(!s.eval::<bool>("return QuestInfoItem2:IsShown()").unwrap());

    // Wheel down over row 3: the hybrid list moves its rows' ids.
    let at = row_centre(&mut s, 3);
    s.mouse_wheel(at.0, at.1, -1.0);
    assert!(s.errors().is_empty(), "wheel errors: {:?}", s.errors());
    assert!(
        s.eval::<i64>("return QuestLogScrollFrameButton1:GetID()")
            .unwrap()
            > 1,
        "wheel-down scrolled the list"
    );
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);

    // Abandon, then No: the ABANDON_QUEST popup closes and queues nothing.
    s.take_sounds();
    s.run("QuestLogFrameAbandonButton:Click()").unwrap();
    assert!(s.eval::<bool>("return StaticPopup1:IsShown()").unwrap());
    assert_eq!(text(&s, "StaticPopup1Text"), "Abandon \"Quest 1\"?");
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsShown()").unwrap());
    assert!(
        s.take_quest_log_abandons().is_empty(),
        "No queues no abandon intent"
    );
    assert!(!s
        .take_sounds()
        .contains(&SoundRequest::KitName("igQuestLogAbandonQuest".into())));

    // Yes: the index marked at click time drains, with the abandon kit (`StaticPopup.lua:1696`).
    s.run("QuestLogFrameAbandonButton:Click()").unwrap();
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_quest_log_abandons(), vec![1]);
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igQuestLogAbandonQuest".into())));
    assert!(!s.eval::<bool>("return StaticPopup1:IsShown()").unwrap());

    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());
    assert!(
        s.take_sounds()
            .contains(&SoundRequest::KitName("igQuestLogClose".into())),
        "closing the quest log plays igQuestLogClose"
    );
}

/// With chat closed, a title shift-click toggles the watch (`QuestLogFrame.lua:137-160`): the
/// row's check and the `WatchFrame` tracker. A manual watch has no timer and never expires.
#[test]
fn shift_click_toggles_the_watch_checkbox_and_the_tracker_hud() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = open_log(eight_entries());

    assert!(!s.eval::<bool>("return WatchFrameHeader:IsShown()").unwrap());
    assert!(!s
        .eval::<bool>("return QuestLogScrollFrameButton1Check:IsShown()")
        .unwrap());

    let at = row_centre(&mut s, 1);
    click_at(&mut s, at, true);
    assert!(
        s.errors().is_empty(),
        "shift-click errors: {:?}",
        s.errors()
    );
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
    assert!(
        s.eval::<bool>("return QuestLogScrollFrameButton1Check:IsShown()")
            .unwrap(),
        "a watched row shows its check (`QuestLogFrame.lua:467-471`)"
    );

    // The tracker: its header counts the objectives, and the lines name the quest and its one.
    assert!(s.eval::<bool>("return WatchFrameHeader:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return WatchFrameTitle:GetText()")
            .unwrap(),
        "Objectives (1)"
    );
    assert!(
        draws_text(&mut s, "Quest 1"),
        "the tracker titles the quest"
    );
    assert!(
        draws_text(&mut s, "Kobold Vermin slain: 3/10"),
        "the tracker lists the objective"
    );
    s.tick(400.0);
    assert!(
        s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "a manual watch has no timer"
    );

    // Unwatching the last quest hides the tracker's header.
    click_at(&mut s, at, true);
    assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(!s
        .eval::<bool>("return QuestLogScrollFrameButton1Check:IsShown()")
        .unwrap());
    assert!(!s.eval::<bool>("return WatchFrameHeader:IsShown()").unwrap());
}

/// 3.3.5a watches a quest with no objectives, and refuses only past `MAX_WATCHABLE_QUESTS` with a
/// red line; either way the click then selects the row (`QuestLogFrame.lua:100-113`, `:152-155`).
#[test]
fn watch_guards_no_op_without_erroring() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = open_log(entries(26));

    // Quest 2 has no objectives, and is watched all the same.
    let at = row_centre(&mut s, 2);
    click_at(&mut s, at, true);
    assert!(s.errors().is_empty(), "watch errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return IsQuestWatched(2)").unwrap());
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 2);

    // Fill the 25 watches with quests 2-26; the guard is the click handler's, not
    // `AddQuestWatch`'s.
    for i in 3..=26 {
        s.run(&format!("AddQuestWatch({i})")).unwrap();
    }
    assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 25);

    let at = row_centre(&mut s, 1);
    click_at(&mut s, at, true);
    assert!(
        s.errors().is_empty(),
        "too-many guard errors: {:?}",
        s.errors()
    );
    assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 25);
    assert!(
        draws_text(&mut s, "You may only watch 25 quests at a time."),
        "QUEST_WATCH_TOO_MANY on UIErrorsFrame"
    );
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);
}

/// Progress arms a timed watch (`MAX_QUEST_WATCH_TIME`, `QuestLogFrame.lua:265-271`), which
/// expires unless fresh progress re-arms it.
#[test]
fn progress_auto_watches_for_five_minutes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_quest_log(eight_entries());
    // The Objectives panel fills its uvars at world entry (`OptionsPanelTemplates.lua:312`).
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert_eq!(
        s.eval::<Option<String>>("return AUTO_QUEST_PROGRESS")
            .unwrap()
            .as_deref(),
        Some("1"),
        "the Objectives panel's uvar, from `autoQuestProgress`"
    );

    // The app fires this with the quest's log row, then the log update the progress brings,
    // which redraws the tracker (`WatchFrame.lua:236-243`).
    s.fire_event(
        "QUEST_WATCH_UPDATE",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    let errors = super::test_ui::errors_but_unported(&s);
    assert!(errors.is_empty(), "auto-watch errors: {errors:?}");
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(s.eval::<bool>("return WatchFrameHeader:IsShown()").unwrap());

    s.tick(299.0);
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    s.fire_event(
        "QUEST_WATCH_UPDATE",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    s.tick(299.0);
    assert!(
        s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "re-armed by fresh progress"
    );
    s.tick(2.0);
    assert!(
        !s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "expired after 300 s without progress"
    );
    s.tick(0.1);
    assert!(!s.eval::<bool>("return WatchFrameHeader:IsShown()").unwrap());
    let errors = super::test_ui::errors_but_unported(&s);
    assert!(errors.is_empty(), "expiry errors: {errors:?}");
}

/// `AUTO_QUEST_WATCH` gates the watch a new quest takes as a "1"/"0" string
/// (`QuestLogFrame.lua:260-264`); it boots "1" (`InterfaceOptionsFrame.lua:321`).
#[test]
fn the_auto_watch_flag_is_the_references_uvar_and_gates_the_watch() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_quest_log(eight_entries());
    assert_eq!(s.eval::<String>("return AUTO_QUEST_WATCH").unwrap(), "1");

    s.run(r#"AUTO_QUEST_WATCH = "0""#).unwrap();
    s.fire_event(
        "QUEST_ACCEPTED",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(
        !s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "the flag off watches nothing"
    );

    s.run(r#"AUTO_QUEST_WATCH = "1""#).unwrap();
    s.fire_event(
        "QUEST_ACCEPTED",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(s.errors().is_empty(), "auto-watch errors: {:?}", s.errors());
}

/// A row's tag paints in parentheses on its right-flush `$parentTag`, and a Failed or Complete
/// state replaces it (`QuestLogFrame.lua:432-451`); headers hide theirs.
#[test]
fn the_row_tag_is_its_own_right_flush_string_and_the_state_word_wins() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut state = eight_entries();
    // Row 1 tagged, row 2 tagged and complete, row 3 failed, row 4 a header.
    state.entries[0].tag = Some("Elite".into());
    state.entries[1].tag = Some("Raid".into());
    state.entries[1].complete = 1;
    state.entries[2].complete = -1;
    state.entries[3].is_header = true;
    state.entries[3].title = "Elwynn Forest".into();
    state.entries[3].tag = None;
    let mut s = open_log(state);

    let tag = |s: &mut UiScript, i: u32| {
        s.eval::<Option<String>>(&format!(
            "local t = QuestLogScrollFrameButton{i}Tag \
             if t:IsShown() then return t:GetText() end"
        ))
        .unwrap()
    };
    assert_eq!(
        tag(&mut s, 1).as_deref(),
        Some("(Elite)"),
        "the engine's tag, in parentheses"
    );
    assert_eq!(
        tag(&mut s, 2).as_deref(),
        Some("(Complete)"),
        "COMPLETE overwrites the quest's own tag"
    );
    assert_eq!(tag(&mut s, 3).as_deref(), Some("(Failed)"));
    assert_eq!(tag(&mut s, 4), None, "a header hides its tag");

    // The title is the indented bare name; the state word is only on the tag.
    assert_eq!(
        s.eval::<String>("return QuestLogScrollFrameButton2NormalText:GetText()")
            .unwrap(),
        "  Quest 2"
    );

    // Anchored RIGHT, -2 (`QuestLogFrame.xml:11-22`).
    let (row_right, tag_right) = s
        .eval::<(f32, f32)>(
            "return QuestLogScrollFrameButton1:GetRight(), QuestLogScrollFrameButton1Tag:GetRight()",
        )
        .unwrap();
    assert!(
        (row_right - tag_right - 2.0).abs() < 0.5,
        "tag right-flush at row right -2 (row {row_right}, tag {tag_right})"
    );
}

/// An empty log shows `EmptyQuestLogFrame`, hides every row and the detail, and darkens the control
/// panel (`QuestLogFrame.lua:331-340`, `:855-860`).
#[test]
fn empty_quest_log_hides_rows_and_disables_abandon() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = open_log(QuestLogState::default());

    assert!(s
        .eval::<bool>("return EmptyQuestLogFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return QuestLogNoQuestsText:GetText()")
            .unwrap(),
        "No Active Quests"
    );
    assert!(!s
        .eval::<bool>("return QuestLogScrollFrameButton1:IsVisible()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return QuestLogFrameAbandonButton:IsEnabled() == 1")
        .unwrap());
    assert!(
        !s.eval::<bool>("return QuestLogDetailScrollFrame:IsVisible()")
            .unwrap(),
        "no selection: the detail pane is hidden"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogQuestCount:GetText()")
            .unwrap(),
        "Quests: |cffffffff0/25|r"
    );
}

/// `QuestInfo_ShowRewards` lays choices out two per row and anchors "You will also receive:" under
/// the last choice row's left item, then the reward (`QuestInfo.lua:327-470`).
#[test]
fn reward_rows_follow_the_refs_two_per_row_layout() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut state = eight_entries();
    let item = |name: &str| QuestItemView {
        name: Some(name.into()),
        count: 1,
        quality: 1,
        usable: true,
        ..Default::default()
    };
    let detail = QuestLogDetail {
        description: "Speak with Marshal McBride.".into(),
        objectives_text: "Report to Marshal McBride.".into(),
        reward_money: 150, // 1s 50c
        choices: vec![item("Worn Sword"), item("Worn Mace")],
        rewards: vec![item("Militia Hammer")],
        ..Default::default()
    };
    for e in &mut state.entries {
        e.detail = Some(detail.clone());
    }
    let s = open_log(state);

    // One pool: items 1-2 are the choices, 3 the reward, 4 the first unused slot.
    let shown = |i: u32| {
        s.eval::<bool>(&format!("return QuestInfoItem{i}:IsShown()"))
            .unwrap()
    };
    assert!(shown(1) && shown(2) && shown(3) && !shown(4));
    assert_eq!(
        s.eval::<String>("return QuestInfoItem2Name:GetText()")
            .unwrap(),
        "Worn Mace"
    );
    // The log never chooses: REWARD_CHOICES.
    assert_eq!(
        s.eval::<String>("return QuestInfoItemChooseText:GetText()")
            .unwrap(),
        "You will be able to choose one of these rewards:"
    );
    assert_eq!(
        s.eval::<String>("return QuestInfoItemReceiveText:GetText()")
            .unwrap(),
        "You will also receive:"
    );
    assert_eq!(
        s.eval::<String>(
            "local p, rel, rp, x, y = QuestInfoItemReceiveText:GetPoint() \
             return p .. '|' .. rel:GetName() .. '|' .. rp .. '|' .. x .. '|' .. y"
        )
        .unwrap(),
        "TOPLEFT|QuestInfoItem1|BOTTOMLEFT|3|-5"
    );
    assert_eq!(
        s.eval::<String>("return QuestInfoItem3Name:GetText()")
            .unwrap(),
        "Militia Hammer"
    );
    assert_eq!(
        s.eval::<String>("return QuestInfoRewardsHeader:GetText()")
            .unwrap(),
        "Rewards"
    );
}

/// Answer every pending measure with `FixedWidthFont(6.0)`'s metrics: 6 px a glyph, 12 px a line.
fn answer_measures(s: &mut UiScript) {
    let answers: Vec<(u32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .into_iter()
        .map(|r| {
            let ink = r.text.chars().count() as f32 * 6.0;
            match r.wrap_width {
                Some(w) if w > 0.0 && ink > w => (r.id, w, (ink / w).ceil() * 12.0, r.key),
                _ => (r.id, ink, 12.0, r.key),
            }
        })
        .collect();
    s.set_measured_text_unwrapped(&answers);
}

fn overflowing_entry() -> QuestLogState {
    let objectives = (1..=10)
        .map(|i| QuestLogObjectiveView {
            text: format!("Objective {i} of 10 slain: 0/5"),
            kind: "monster".into(),
            finished: false,
            cur: 0,
            req: 5,
        })
        .collect();
    QuestLogState {
        num_quests: 1,
        hidden_quest_ids: Vec::new(),
        daily_completed: 0,
        entries: vec![QuestLogEntryView {
            quest_id: 1,
            title: "A Very Long Quest".into(),
            level: 5,
            complete: 0,
            objectives,
            detail: Some(QuestLogDetail {
                description: "A very long description. ".repeat(20),
                objectives_text: "Report back once every objective below is complete.".into(),
                rewards: vec![QuestItemView {
                    name: Some("Militia Hammer".into()),
                    count: 1,
                    quality: 1,
                    usable: true,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }],
    }
}

/// `QuestLogDetailScrollFrame`'s rect, attached to the log (`QuestLogFrame.lua:808-816`).
fn detail_rect(s: &mut UiScript) -> benilla_ui::layout::Rect {
    s.resolve();
    let (left, right, bottom, top) = s
        .eval::<(f32, f32, f32, f32)>(
            "local f = QuestLogDetailScrollFrame \
             return f:GetLeft(), f:GetRight(), f:GetBottom(), f:GetTop()",
        )
        .unwrap();
    benilla_ui::layout::Rect {
        left,
        right,
        bottom,
        top,
    }
}

/// Quads under the scroll child carry `QuestLogDetailScrollFrame`'s rect as their clip; the
/// window's own art outside it carries none.
#[test]
fn overflowing_detail_content_clips_to_the_scrollframe_rect() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = open_log(overflowing_entry());

    let scroll_rect = detail_rect(&mut s);
    let quads = s.extract();
    let reward_plate_clips: Vec<Option<benilla_ui::layout::Rect>> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } if p.contains("UI-QuestItemNameFrame") => {
                Some(q.clip)
            }
            _ => None,
        })
        .collect();
    assert!(
        !reward_plate_clips.is_empty(),
        "expected at least one reward-row name-plate quad in the fixture"
    );
    for clip in reward_plate_clips {
        let clip = clip.expect("a reward row inside the scroll child is clipped");
        assert!(
            (clip.left - scroll_rect.left).abs() < 0.5
                && (clip.top - scroll_rect.top).abs() < 0.5
                && (clip.right - scroll_rect.right).abs() < 0.5
                && (clip.bottom - scroll_rect.bottom).abs() < 0.5,
            "a reward row clips to the ScrollFrame's own rect: {clip:?} vs {scroll_rect:?}"
        );
    }

    let book_icon_clip = quads.iter().find_map(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } if p.contains("UI-QuestLog-BookIcon") => {
            Some(q.clip)
        }
        _ => None,
    });
    assert_eq!(
        book_icon_clip,
        Some(None),
        "the window's own chrome sits outside the scroll child and is unclipped"
    );
}

/// The detail pane's scroll frame takes the wheel (`UIPanelScrollFrameTemplate`).
#[test]
fn wheel_over_the_detail_pane_changes_vertical_scroll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    s.set_quest_log(overflowing_entry());
    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The scroll range spans the measured content, so answer the measures as the app does.
    s.resolve();
    answer_measures(&mut s);
    s.resolve();
    s.resolve();

    let scroll_rect = detail_rect(&mut s);
    // Near the pane's top-left, clear of the reward row.
    let (x, y) = (scroll_rect.left + 10.0, scroll_rect.top - 10.0);
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        0.0
    );
    s.mouse_wheel(x, y, -1.0); // -1 is down
    assert!(s.errors().is_empty(), "wheel errors: {:?}", s.errors());
    let after = s
        .eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
        .unwrap();
    assert!(
        after > 0.0,
        "wheel-down over the detail pane increased its scroll offset, got {after}"
    );
}

/// A selection resets the detail scroll (`QuestLog_UpdateQuestDetails(true)`,
/// `QuestLogFrame.lua:542-548`, `:666`); the `QUEST_LOG_UPDATE` refresh passes false (`:255`).
#[test]
fn selection_change_resets_detail_scroll_but_a_quest_log_update_refresh_does_not() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    s.set_quest_log(overflowing_entry());
    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();
    answer_measures(&mut s);
    s.resolve();
    s.resolve();

    s.run("QuestLogDetailScrollFrameScrollBar:SetValue(10)")
        .unwrap();
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        10.0
    );

    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    assert!(s.errors().is_empty(), "refresh errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        10.0,
        "QUEST_LOG_UPDATE's data-refresh path must not yank the scroll position"
    );

    s.run("QuestLogScrollFrameButton1:Click()").unwrap();
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        0.0,
        "a manual reselect snaps the detail pane's scroll back to the top"
    );
}

/// The reward row's OnEnter calls `SetQuestLogItem` (`QuestInfo.xml:7-26`): a cold store shows the
/// name and asks once, and after the answer a re-hover shows the full item lines.
#[test]
fn reward_row_hover_serves_the_shared_item_tooltip() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ItemTemplateView;

    let mut s = open_log(eight_entries());
    let icon_rect = s
        .extract()
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("INV_Hammer_15"))
        })
        .and_then(|q| q.rect)
        .expect("the reward row icon");
    let (cx, cy) = (
        (icon_rect.left + icon_rect.right) * 0.5,
        (icon_rect.bottom + icon_rect.top) * 0.5,
    );

    s.mouse_move(cx, cy);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "tooltip shows on hover"
    );
    assert_eq!(
        s.take_item_stat_asks(),
        vec![2024],
        "the miss recorded the ask"
    );

    s.set_item_template(
        2024,
        ItemTemplateView {
            name: "Militia Hammer".into(),
            quality: 1,
            inventory_type: 21, // Main Hand
            class: 2,
            subclass: 4, // mace
            damages: vec![(4.0, 9.0, 0)],
            delay_ms: 2200,
            ..Default::default()
        },
    );
    s.mouse_move(0.0, 0.0);
    s.mouse_move(cx, cy);
    assert!(s.errors().is_empty(), "re-hover errors: {:?}", s.errors());
    assert!(
        draws_text(&mut s, "Main Hand"),
        "slot line from the shared store"
    );
    assert!(
        draws_text(&mut s, "4 - 9 Damage"),
        "damage line from the shared store"
    );
}

/// A child takes its parent's strata and level + 1 at creation, so the DIALOG popup's buttons draw
/// above its own backdrop.
#[test]
fn popup_children_inherit_the_dialog_stratum() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.run(
        "StaticPopupDialogs[\"TEST_STRATUM\"] = { text = \"Abandon?\", button1 = \"Yes\", \
         button2 = \"No\", timeout = 0 }\n\
         StaticPopup_Show(\"TEST_STRATUM\")",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let backdrop_z = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Backdrop { path, .. } if path.contains("UI-DialogBox-Background") => {
                Some(q.z)
            }
            _ => None,
        })
        .expect("the popup backdrop");
    let button_z = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } if p.contains("UI-DialogBox-Button-Up") => {
                Some(q.z)
            }
            _ => None,
        })
        .expect("a popup button face");
    assert!(
        button_z > backdrop_z,
        "the buttons draw above their dialog's backdrop (button z {button_z:x} vs backdrop z {backdrop_z:x})"
    );
}

/// The log's reward row (`QuestInfo.xml:27-49`): a modified click goes to `HandleModifiedItemClick`,
/// ctrl trying the reward on and shift posting its link with chat open; a plain click selects
/// nothing, as the log template never chooses.
#[test]
fn reward_rows_preview_and_post_and_a_plain_click_stays_inert() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = open_log(eight_entries());
    // A main-hand template makes the hammer dressable (`IsDressableItem` `0x517530`).
    s.set_item_template(
        2024,
        benilla_ui::script::ItemTemplateView {
            name: "Militia Hammer".into(),
            quality: 1,
            inventory_type: 21,
            ..Default::default()
        },
    );
    assert!(
        s.eval::<bool>("return QuestInfoItem1:IsShown()").unwrap(),
        "the fixture's one fixed reward row is laid out"
    );
    let _ = s.take_dressup_intents();

    s.run("QuestInfoItem1:Click()").unwrap();
    assert!(
        s.take_dressup_intents().is_empty(),
        "a plain click never opens the dressing room"
    );
    assert_eq!(
        s.eval::<i64>("return QuestInfoFrame.itemChoice").unwrap(),
        0
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.run("ChatEdit_ActivateChat(ChatFrame1EditBox)").unwrap();
    s.set_modifiers(true, false, false);
    s.run("QuestInfoItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrame1EditBox:GetText()")
            .unwrap(),
        format!(" {HAMMER_LINK}"),
        "shift-click posted the reward's link after `ChatEdit_InsertLink`'s space"
    );
    assert!(
        s.take_dressup_intents().is_empty(),
        "the shift arm never opens the room"
    );

    s.set_modifiers(false, true, false);
    s.run("QuestInfoItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![
            benilla_ui::script::DressUpIntent::Dress,
            benilla_ui::script::DressUpIntent::TryOn(2024)
        ],
        "ctrl-click opened the room wearing the reward"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// With chat open a title shift-click inserts the quest's link, `GetQuestLink`
/// (`QuestLogFrame.lua:146-151`); with chat closed it toggles the watch (`:152-155`). Either way
/// the row ends selected.
#[test]
fn shift_click_on_a_title_posts_the_quest_name_with_chat_open_and_watches_with_it_closed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = open_log(eight_entries());
    let at = row_centre(&mut s, 2);

    click_at(&mut s, at, true);
    assert!(
        s.eval::<bool>("return IsQuestWatched(2)").unwrap(),
        "with chat closed, shift-click toggles the quest watch"
    );
    click_at(&mut s, at, true);
    assert!(!s.eval::<bool>("return IsQuestWatched(2)").unwrap());

    s.run("ChatEdit_ActivateChat(ChatFrame1EditBox)").unwrap();
    click_at(&mut s, at, true);
    let link = s.eval::<String>("return GetQuestLink(2)").unwrap();
    assert!(link.contains("|Hquest:2:"), "a quest link: {link}");
    assert_eq!(
        s.eval::<String>("return ChatFrame1EditBox:GetText()")
            .unwrap(),
        format!(" {link}"),
        "the quest's link landed in the chat box"
    );
    assert!(
        !s.eval::<bool>("return IsQuestWatched(2)").unwrap(),
        "with chat open the watch does not toggle"
    );
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 2);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The Share Quest button ──

/// A party of `n` others; `GetNumPartyMembers` is that list's length.
fn party(n: usize) -> PartyState {
    PartyState {
        members: (0..n)
            .map(|i| PartyMemberInfo {
                name: format!("Mate{i}"),
                guid: 0x300 + i as u64,
            })
            .collect(),
        ..PartyState::default()
    }
}

/// The eight quests with `pushable` set on `ids`; the app reads it from `QUEST_FLAGS_SHARABLE`.
fn entries_sharable(ids: &[u32]) -> QuestLogState {
    let mut state = eight_entries();
    for e in &mut state.entries {
        e.pushable = ids.contains(&e.quest_id);
    }
    state
}

/// Share needs both a sharable selection and a group (`QuestLogControlPanel_UpdateState`,
/// `QuestLogFrame.lua:868-872`); each half is moved alone.
#[test]
fn share_quest_needs_both_a_sharable_selection_and_a_party() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_quest_log(entries_sharable(&[1]));
    s.set_party(PartyState::default());
    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);
    let share = |s: &UiScript| {
        s.eval::<bool>("return QuestLogFramePushQuestButton:IsEnabled() == 1")
            .unwrap()
    };
    assert!(
        s.eval::<bool>("return GetQuestLogPushable() ~= nil")
            .unwrap(),
        "quest 1 carries the sharable bit"
    );
    assert!(!share(&s), "solo: sharable is not enough");

    // PARTY_MEMBERS_CHANGED alone re-tests it (`QuestLogFrame.lua:273-277`).
    s.set_party(party(2));
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    assert!(share(&s), "PARTY_MEMBERS_CHANGED alone re-tests the button");

    s.run("QuestLog_SetSelection(2)").unwrap();
    assert!(
        s.eval::<bool>("return GetQuestLogPushable() == nil")
            .unwrap(),
        "quest 2 has no sharable bit"
    );
    assert!(
        !share(&s),
        "in a party: an unsharable selection is still dark"
    );

    s.run("QuestLog_SetSelection(1)").unwrap();
    assert!(share(&s));
    s.set_party(PartyState::default());
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    assert!(!share(&s));
}

/// An empty log has no selection, which darkens Share before either other test
/// (`QuestLogFrame.lua:858-861`).
#[test]
fn share_quest_is_dark_on_an_empty_log_even_in_a_party() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_quest_log(QuestLogState::default());
    s.set_party(party(4));
    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(!s
        .eval::<bool>("return QuestLogFramePushQuestButton:IsEnabled() == 1")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The click queues the selected quest's id, not the row under the mouse.
#[test]
fn share_quest_click_queues_the_selected_quests_id() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_quest_log(entries_sharable(&[1, 5]));
    s.set_party(party(1));
    s.run("ToggleFrame(QuestLogFrame)").unwrap();

    s.run("QuestLog_SetSelection(5)").unwrap();
    s.run("QuestLogFramePushQuestButton:Click()").unwrap();
    assert_eq!(s.take_quest_log_pushes(), vec![5]);
    assert!(s.take_quest_log_pushes().is_empty(), "drained");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `QuestLogTitleButton_Resize` (`QuestLogFrame.lua:183-224`) resets the title's width each
/// update, then trims what overruns the tag's left edge less 4: a tagged title is capped at
/// `299 - 20 - 4 - tagWidth`, and the same row untagged takes its natural width back.
#[test]
fn the_stock_title_width_cap_bites_and_resets_each_update() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    // The cap reads `questTitleTag:GetWidth()` inside the update, so the harness needs a measurer.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    // A long Dungeon-tagged title.
    const LONG: &str = "The Left Piece of Lord Valthalak's Amulet, Recovered";
    let mut state = eight_entries();
    state.entries[0].title = LONG.into();
    state.entries[0].tag = Some("Dungeon".into());
    s.set_quest_log(state.clone());
    s.run("ToggleFrame(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let width =
        |s: &mut UiScript, q: &str| s.eval::<f32>(&format!("return {q}:GetWidth()")).unwrap();
    let ink = |s: &mut UiScript, q: &str| {
        s.eval::<f32>(&format!("return {q}:GetStringWidth()"))
            .unwrap()
    };

    let tag_w = width(&mut s, "QuestLogScrollFrameButton1Tag");
    let capped = width(&mut s, "QuestLogScrollFrameButton1NormalText");
    assert!(
        (capped - (299.0 - 20.0 - 4.0 - tag_w)).abs() < 0.5,
        "the title stops short of the {tag_w}-wide tag: got {capped}"
    );
    let natural = ink(&mut s, "QuestLogScrollFrameButton1NormalText");
    assert!(
        natural > capped,
        "the fixture title ({natural} px) must overflow the {capped} px cap"
    );

    // Untagged, the same row takes back its natural width: the resize starts from `SetWidth(0)`.
    state.entries[0].title = "Quest 1".into();
    state.entries[0].tag = None;
    s.set_quest_log(state);
    s.run("QuestLog_Update()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        (width(&mut s, "QuestLogScrollFrameButton1NormalText")
            - ink(&mut s, "QuestLogScrollFrameButton1NormalText"))
        .abs()
            < 0.5,
        "an untagged short title keeps its natural width"
    );
}
