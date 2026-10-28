//! The stock combo-point dots (`ComboFrame.lua`), which repaint on `UNIT_COMBO_POINTS` for
//! `PlayerFrame.unit` and on `PLAYER_TARGET_CHANGED`, reading `GetComboPoints(PlayerFrame.unit,
//! "target")`. The binding holds two gates the Lua does not: rogue or druid only, and the points
//! count only on the unit they are banked on.

use benilla_ui::script::{PlayerReqState, ScriptValue, UiScript, UnitGuids, UnitState};

const ME: u64 = 0x0000_0000_0000_0007;
const MOB_A: u64 = 0xF130_0000_0000_0001;
const MOB_B: u64 = 0xF130_0000_0000_0002;

fn load_combo_frame() -> UiScript {
    let mut s = super::test_ui::stock_ui(1024.0, 768.0);
    s.set_unit_guids(&UnitGuids {
        player: ME,
        ..Default::default()
    });
    s
}

fn play_as(s: &mut UiScript, class_id: u32, target: Option<u64>) {
    s.set_player_req_state(PlayerReqState {
        class_id,
        ..Default::default()
    });
    s.set_unit(
        "target",
        target.map(|guid| UnitState {
            exists: true,
            guid,
            ..Default::default()
        }),
    );
}

/// One `SMSG_UPDATE_COMBO_POINTS`: the bank, then the event to `"player"` (`0x519780`).
fn bank(s: &mut UiScript, points: u8, on: u64) {
    s.set_combo_points(points, on);
    s.fire_event("UNIT_COMBO_POINTS", vec![ScriptValue::Str("player".into())]);
}

/// Run 2 s of fades, past the 0.4 + 0.3 + 0.4 s highlight and shine chain (`ComboFrame.lua:3`),
/// through the fade manager's `UIFrameFade_OnUpdate(self, elapsed)` (`UIParent.lua`).
fn settle(s: &mut UiScript) {
    s.eval::<()>(
        "local d = CreateFrame('Frame') for i = 1, 40 do UIFrameFade_OnUpdate(d, 0.05) end",
    )
    .unwrap();
}

fn shown(s: &mut UiScript) -> bool {
    s.eval("return ComboFrame:IsShown()").unwrap()
}

fn points(s: &mut UiScript) -> i64 {
    s.eval("return GetComboPoints(PlayerFrame.unit, 'target')")
        .unwrap()
}

fn highlight_alpha(s: &mut UiScript, i: u32) -> f64 {
    s.eval(&format!("return ComboPoint{i}Highlight:GetAlpha()"))
        .unwrap()
}

/// The highlight arrives through `UIFrameFade`, so every alpha read settles the fades first.
#[test]
fn combo_frame_follows_the_point_count() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, Some(MOB_A)); // rogue
    assert_eq!(
        s.eval::<String>("return PlayerFrame.unit").unwrap(),
        "player"
    );

    bank(&mut s, 0, 0);
    assert!(!shown(&mut s), "no points ⇒ hidden");

    bank(&mut s, 1, MOB_A);
    assert!(shown(&mut s), "one point ⇒ shown");
    settle(&mut s);
    let lit = highlight_alpha(&mut s, 1);
    assert!(lit > 0.99, "point 1 lit, got alpha {lit}");
    assert_eq!(
        highlight_alpha(&mut s, 2),
        0.0,
        "point 2 stays dark at one combo point"
    );

    bank(&mut s, 5, MOB_A);
    settle(&mut s);
    let all_lit: bool = s
        .eval(
            r#"
            for i = 1, MAX_COMBO_POINTS do
                if _G["ComboPoint" .. i .. "Highlight"]:GetAlpha() < 0.99 then
                    return false
                end
            end
            return true
        "#,
        )
        .unwrap();
    assert!(all_lit, "all five dots lit at five points");

    // The server clears the count on spend or expiry; that packet alone takes the dots down.
    bank(&mut s, 0, 0);
    assert!(!shown(&mut s), "zero points ⇒ hidden again");
}

/// An event for another unit repaints nothing (`ComboFrame.lua:13`).
#[test]
fn another_units_event_is_ignored() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, Some(MOB_A));
    s.set_combo_points(2, MOB_A);
    s.fire_event("UNIT_COMBO_POINTS", vec![ScriptValue::Str("pet".into())]);
    assert!(
        !shown(&mut s),
        "the pet's event leaves the player's dots alone"
    );
    s.fire_event("UNIT_COMBO_POINTS", vec![ScriptValue::Str("player".into())]);
    assert!(shown(&mut s));
}

/// `PLAYER_TARGET_CHANGED` repaints too; `COMBO_FRAME_LAST_NUM_POINTS` stops lit dots re-flaring.
#[test]
fn a_repaint_at_the_same_count_does_not_re_flare() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, Some(MOB_A));
    bank(&mut s, 2, MOB_A);
    settle(&mut s);

    let shine: f64 = s.eval("return ComboPoint2Shine:GetAlpha()").unwrap();
    assert_eq!(shine, 0.0, "the shine fades back out");

    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let fading: bool = s
        .eval("return UIFrameIsFading(ComboPoint1Highlight) ~= nil")
        .unwrap();
    assert!(!fading, "an unchanged count re-flares nothing");
    assert!(
        highlight_alpha(&mut s, 2) > 0.99,
        "and leaves the lit dots lit"
    );
}

/// `GetComboPoints` (`0x611670`) answers 0 for our unit outside rogue and druid (classes 4 and
/// 11, `0x611712`), though the server banks a point for a warrior's Overpower on a dodge.
#[test]
fn a_warriors_overpower_point_lights_no_dot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 1, Some(MOB_A)); // warrior
    bank(&mut s, 1, MOB_A);
    assert!(!shown(&mut s), "a warrior's banked point shows no dots");
    assert_eq!(points(&mut s), 0, "and the binding itself reports zero");

    play_as(&mut s, 11, Some(MOB_A));
    bank(&mut s, 1, MOB_A);
    assert!(shown(&mut s), "a druid sees the same banked point");
}

/// The points count only on the banked unit (`0x513920`), so the dots follow the selection while
/// the bank stays put.
#[test]
fn re_targeting_empties_the_dots_without_the_count_moving() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, Some(MOB_A));
    bank(&mut s, 3, MOB_A);
    settle(&mut s);
    assert!(shown(&mut s), "three points on the selected mob ⇒ shown");

    play_as(&mut s, 4, Some(MOB_B));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!shown(&mut s), "points banked elsewhere ⇒ no dots");
    assert_eq!(
        points(&mut s),
        0,
        "the binding hides them, the bank keeps them"
    );

    play_as(&mut s, 4, Some(MOB_A));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    settle(&mut s);
    assert!(shown(&mut s), "re-selecting the banked mob refills them");
    assert!(highlight_alpha(&mut s, 3) > 0.99, "all three, not just one");

    play_as(&mut s, 4, None);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!shown(&mut s), "no target ⇒ no dots");
}

/// The binding's shape: a unit is required, a unit that is neither ours nor the pet answers 0, and
/// the second token names whose points to read.
#[test]
fn get_combo_points_reads_the_named_unit_and_target() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, Some(MOB_A));
    s.set_combo_points(4, MOB_A);
    assert!(s.eval::<i64>("return GetComboPoints()").is_err(), "usage");
    assert_eq!(s.eval::<i64>("return GetComboPoints('player')").unwrap(), 4);
    assert_eq!(s.eval::<i64>("return GetComboPoints('target')").unwrap(), 0);
    assert_eq!(
        s.eval::<i64>("return GetComboPoints('player', 'player')")
            .unwrap(),
        0,
        "points on the target, not on us"
    );
}
