//! Moving an action between two stock 3.3.5a bar buttons, through the production load and the
//! real pointer path: a drag (`OnDragStart` → `PickupAction`, `OnReceiveDrag` → `PlaceAction`)
//! and a click-pickup then click-place (`UseAction`'s place fork, `0x5abbfb`).

use benilla_ui::script::{ActionSlot, CursorPayload, ScriptValue, UiScript};

fn spell(texture: &str, action: u32) -> ActionSlot {
    ActionSlot {
        texture: Some(texture.into()),
        kind: 0x00,
        action,
        count: 0,
        consumable: false,
        stackable: false,
        pet_spell: None,
    }
}

/// The stock UI with slot 2 holding spell 111 and, when `occupied`, slot 3 holding spell 222.
fn bar(tag: &str, occupied: bool) -> UiScript {
    let (mut s, _load_failures) = super::layer_tests::production_load_with(tag, false, "", |_| {});
    let _ = s.take_errors();
    s.set_action(2, Some(spell("Interface\\Icons\\Spell_A", 111)));
    if occupied {
        s.set_action(3, Some(spell("Interface\\Icons\\Spell_B", 222)));
    }
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    // Unlocked, so a plain drag picks up (`ActionBarFrame.xml`, `OnDragStart`).
    s.run("LOCK_ACTIONBAR = '0'").unwrap();
    s.resolve();
    let _ = s.take_action_sets();
    s
}

fn centre(s: &UiScript, name: &str) -> (f32, f32) {
    let x = s
        .eval::<f32>(&format!(
            "return ({name}:GetLeft() + {name}:GetRight()) / 2"
        ))
        .unwrap();
    let y = s
        .eval::<f32>(&format!(
            "return ({name}:GetTop() + {name}:GetBottom()) / 2"
        ))
        .unwrap();
    (x, y)
}

fn drag(s: &mut UiScript, from: &str, to: &str) {
    let (fx, fy) = centre(s, from);
    let (tx, ty) = centre(s, to);
    s.mouse_move(fx, fy);
    s.mouse_button(fx, fy, "LeftButton", true);
    s.mouse_move(fx + 8.0, fy + 8.0);
    s.tick(0.01);
    s.mouse_move(tx, ty);
    s.mouse_button(tx, ty, "LeftButton", false);
    s.tick(0.01);
}

fn click(s: &mut UiScript, name: &str) {
    let (x, y) = centre(s, name);
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.tick(0.01);
}

fn bar_errors(s: &mut UiScript) -> Vec<String> {
    s.take_errors()
        .into_iter()
        .filter(|e| e.contains("Action") || e.contains("Cursor"))
        .collect()
}

#[test]
fn a_drag_onto_an_empty_button_moves_the_action_and_clears_the_cursor() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = bar("barmove-drag-empty", false);

    drag(&mut s, "ActionButton2", "ActionButton3");

    assert!(
        !s.eval::<bool>("return HasAction(2)").unwrap(),
        "slot 2 emptied"
    );
    assert_eq!(
        s.eval::<String>("return GetActionTexture(3)").unwrap(),
        "Interface\\Icons\\Spell_A"
    );
    assert!(s.cursor_payload().is_none(), "{:?}", s.cursor_payload());
    assert_eq!(s.take_action_sets(), vec![(2, 0), (3, 111)]);
    assert!(s.take_action_uses().is_empty(), "the release is no click");
    let errors = bar_errors(&mut s);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_drag_onto_an_occupied_button_hops_the_displaced_action() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = bar("barmove-drag-occupied", true);

    drag(&mut s, "ActionButton2", "ActionButton3");

    assert_eq!(
        s.eval::<String>("return GetActionTexture(3)").unwrap(),
        "Interface\\Icons\\Spell_A"
    );
    assert!(
        matches!(s.cursor_payload(), Some(CursorPayload::Action(c)) if c.action == 222),
        "{:?}",
        s.cursor_payload()
    );
    assert_eq!(s.take_action_sets(), vec![(2, 0), (3, 111)]);

    // Placing the displaced action clears the cursor.
    click(&mut s, "ActionButton2");
    assert_eq!(
        s.eval::<String>("return GetActionTexture(2)").unwrap(),
        "Interface\\Icons\\Spell_B"
    );
    assert!(s.cursor_payload().is_none(), "{:?}", s.cursor_payload());
    assert_eq!(s.take_action_sets(), vec![(2, 222)]);
    assert!(
        s.take_action_uses().is_empty(),
        "a placing click uses nothing"
    );
    let errors = bar_errors(&mut s);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_pickup_then_a_click_places_the_action() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = bar("barmove-click", false);

    s.run("PickupAction(2)").unwrap();
    assert!(s.cursor_payload().is_some());
    let _ = s.take_action_sets();
    // `ACTIONBAR_SHOWGRID` fires on the tick and shows the empty buttons.
    s.tick(0.01);

    click(&mut s, "ActionButton3");

    assert_eq!(
        s.eval::<String>("return GetActionTexture(3)").unwrap(),
        "Interface\\Icons\\Spell_A"
    );
    assert!(s.cursor_payload().is_none(), "{:?}", s.cursor_payload());
    assert_eq!(s.take_action_sets(), vec![(3, 111)]);
    assert!(
        s.take_action_uses().is_empty(),
        "a placing click uses nothing"
    );
    let errors = bar_errors(&mut s);
    assert!(errors.is_empty(), "{errors:#?}");
}
