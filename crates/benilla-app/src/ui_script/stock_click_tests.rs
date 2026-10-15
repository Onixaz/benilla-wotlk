//! Mouse clicks on the stock 3.3.5a UI, on the production load: the action buttons'
//! `RegisterForClicks("AnyUp")`, the quest log's control panel after its runtime `SetParent`, and
//! the `CAMERAORSELECTORMOVE` release that asks `IsModifiedClick("STICKYCAMERA")`.

use benilla_ui::script::{ActionSlot, BindingInput, HeldInput, ScriptValue, UiScript};

fn loaded(tag: &str) -> UiScript {
    let (mut s, _load_failures) = super::layer_tests::production_load_with(tag, false, "", |_| {});
    s.resolve();
    let _ = s.take_errors();
    s
}

/// A frame's centre in the screen units `mouse_button` takes.
fn screen_centre(s: &mut UiScript, frame: &str) -> (f32, f32) {
    s.resolve();
    let (x, y, eff) = s
        .eval::<(f64, f64, f64)>(&format!(
            "local a, b = {frame}:GetCenter() return a, b, {frame}:GetEffectiveScale()"
        ))
        .unwrap_or_else(|e| panic!("{frame}:GetCenter() — {e}"));
    ((x * eff) as f32, (y * eff) as f32)
}

/// `ActionButton_OnLoad` registers `"AnyUp"`: a left and a right release on `ActionButton1` both
/// reach its secure `OnClick` and use the slot, and each press pushes the art first (#139).
#[test]
fn a_mouse_click_on_a_stock_action_button_uses_its_action() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = loaded("clicks-bar");
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Frost_FrostBolt02".into()),
            kind: 0x00,
            action: 116,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    let (x, y) = screen_centre(&mut s, "ActionButton1");
    assert_eq!(s.hit_test_name(x, y).as_deref(), Some("ActionButton1"));
    let _ = s.take_action_uses();
    for button in ["LeftButton", "RightButton"] {
        s.mouse_move(x, y);
        s.mouse_button(x, y, button, true);
        assert_eq!(
            s.eval::<String>("return ActionButton1:GetButtonState()")
                .unwrap(),
            "PUSHED",
            "{button} press pushes the art"
        );
        s.mouse_button(x, y, button, false);
        let uses: Vec<u32> = s.take_action_uses().into_iter().map(|u| u.action).collect();
        assert_eq!(uses, vec![1], "{button} release uses slot 1");
    }
    let errors: Vec<String> = s
        .take_errors()
        .into_iter()
        .filter(|e| e.contains("Action"))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}

fn one_quest() -> benilla_ui::script::QuestLogState {
    use benilla_ui::script::{QuestLogDetail, QuestLogEntryView, QuestLogState};
    let header = QuestLogEntryView {
        title: "Northshire Valley".into(),
        is_header: true,
        ..Default::default()
    };
    let quest = QuestLogEntryView {
        quest_id: 7,
        title: "Kobold Camp Cleanup".into(),
        level: 2,
        detail: Some(QuestLogDetail {
            description: "Kobolds have infested the mine.".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    QuestLogState {
        entries: vec![header, quest],
        num_quests: 1,
        hidden_quest_ids: Vec::new(),
        daily_completed: 0,
    }
}

/// `QuestLogControlPanel_UpdatePosition` re-parents the panel on every show; with 12340's
/// propagating `SetParent` its buttons stay above it, so a hit at each button's centre is the
/// button, Abandon raises its popup and Track toggles the watch (#218).
#[test]
fn the_quest_log_control_panel_buttons_take_their_clicks() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = loaded("clicks-questlog");
    s.set_quest_log(one_quest());
    s.fire_event("QUEST_LOG_UPDATE", Vec::<ScriptValue>::new());
    s.run("ShowUIPanel(QuestLogFrame) QuestLog_SetSelection(2)")
        .unwrap();
    assert!(
        s.eval::<bool>(
            "return QuestLogFrameAbandonButton:GetFrameLevel() > QuestLogControlPanel:GetFrameLevel()"
        )
        .unwrap(),
        "the buttons sit above their panel"
    );
    for name in [
        "QuestLogFrameAbandonButton",
        "QuestLogFrameTrackButton",
        "QuestLogFramePushQuestButton",
    ] {
        let (x, y) = screen_centre(&mut s, name);
        assert_eq!(
            s.hit_test_name(x, y).as_deref(),
            Some(name),
            "hit at {name}"
        );
    }

    let (x, y) = screen_centre(&mut s, "QuestLogFrameTrackButton");
    let watched = |s: &UiScript| {
        s.eval::<bool>("return IsQuestWatched(2) and true or false")
            .unwrap()
    };
    let before = watched(&s);
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert_ne!(watched(&s), before, "Track toggles the watch");

    let (x, y) = screen_centre(&mut s, "QuestLogFrameAbandonButton");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        s.eval::<bool>(r#"return StaticPopup_Visible("ABANDON_QUEST") ~= nil"#)
            .unwrap(),
        "Abandon raises its popup"
    );
}

/// The stock `CAMERAORSELECTORMOVE` release runs `CameraOrSelectOrMoveStop(IsModifiedClick(
/// "STICKYCAMERA"))` without an error, and the sticky flag follows the action's binding; the
/// table holds every `<ModifiedClick>` the shipped `Bindings.xml` declares, at its default (#61).
#[test]
fn the_stock_camera_release_asks_the_sticky_camera_click() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = loaded("clicks-camera");

    let bytes =
        super::reference_ui::read(super::reference_ui::BINDINGS).expect("the chain's Bindings.xml");
    let text = benilla_ui::source::decode(&bytes);
    let declared = benilla_ui::bindings_xml::parse_modified_clicks(&text).unwrap();
    assert!(!declared.is_empty());
    assert_eq!(
        s.eval::<i64>("return GetNumModifiedClickActions()")
            .unwrap(),
        declared.len() as i64
    );
    for (i, c) in declared.iter().enumerate() {
        let (action, binding) = s
            .eval::<(String, String)>(&format!(
                "local a = GetModifiedClickAction({}) return a, GetModifiedClick(a)",
                i + 1
            ))
            .unwrap();
        assert_eq!(action, c.action);
        let want = if c.default.is_empty() {
            "NONE"
        } else {
            &c.default
        };
        assert!(
            binding.eq_ignore_ascii_case(want),
            "{action}: {binding} vs {want}"
        );
    }

    let release = |s: &mut UiScript| {
        let _ = s.take_binding_input();
        assert!(s.execute_binding("CAMERAORSELECTORMOVE", true).unwrap());
        assert!(s.execute_binding("CAMERAORSELECTORMOVE", false).unwrap());
        s.take_binding_input()
    };
    let stops = |sticky| {
        vec![
            BindingInput::Start(HeldInput::CameraOrSelectOrMove),
            BindingInput::Stop(HeldInput::CameraOrSelectOrMove, sticky),
        ]
    };
    s.run(r#"SetModifiedClick("STICKYCAMERA", "CTRL")"#)
        .unwrap();
    assert_eq!(release(&mut s), stops(false), "a plain release");
    s.set_modifiers(false, true, false);
    assert_eq!(release(&mut s), stops(true), "a CTRL release is sticky");
    s.set_modifiers(false, false, false);
    let errors = s.take_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}
