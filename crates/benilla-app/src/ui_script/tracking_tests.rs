//! The stock `MiniMapTracking` button and its menu (`Minimap.xml:406-500`, `Minimap.lua:408-468`)
//! on the production load, driven through the tracking verbs.

use benilla_ui::script::{ScriptCall, TrackingSpell, UiScript};

const HUNTER: u8 = 3;

fn setup() -> (std::sync::MutexGuard<'static, ()>, UiScript) {
    let lock = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) = super::layer_tests::production_load_with("tracking", false, "", |_| {});
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("Minimap") || f.contains("Tracking"))
        .collect();
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    s.set_tracking_spells(
        vec![TrackingSpell {
            spell_id: 2580,
            name: "Find Minerals".into(),
            icon: Some(r"Interface\Icons\Spell_Nature_Earthquake".into()),
            active_icon: None,
        }],
        HUNTER,
    );
    s.resolve();
    s.take_errors();
    (lock, s)
}

fn icon(s: &UiScript) -> String {
    s.eval::<String>("return MiniMapTrackingIcon:GetTexture() or ''")
        .unwrap()
        .to_ascii_lowercase()
}

#[test]
fn the_button_shows_none_and_follows_the_town_type() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = setup();
    assert_eq!(
        icon(&s),
        r"interface\minimap\tracking\none",
        "not a black disc"
    );

    s.run("SetTracking(2)").unwrap();
    assert_eq!(icon(&s), r"interface\minimap\tracking\repair");
    assert_eq!(
        s.cvar("minimapTrackedInfo").unwrap(),
        "MINIMAP_TRACKING_REPAIR"
    );
    assert!(s.take_errors().is_empty());
}

#[test]
fn a_click_opens_the_spells_the_town_types_and_none() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = setup();
    s.run("MiniMapTrackingButton:Click()").unwrap();
    assert!(s
        .eval::<bool>("return DropDownList1:IsShown() == 1")
        .unwrap());
    // One spell, the hunter's fourteen town types and the trailing None.
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        16
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button1:GetText()")
            .unwrap(),
        "Find Minerals"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button2:GetText()")
            .unwrap(),
        s.eval::<String>("return MINIMAP_TRACKING_REPAIR").unwrap()
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button16:GetText()")
            .unwrap(),
        s.eval::<String>("return NONE").unwrap()
    );

    // The spell row casts it.
    s.run("DropDownList1Button1:Click()").unwrap();
    assert!(s
        .take_script_calls()
        .iter()
        .any(|c| matches!(c, ScriptCall::CastSpell { spell_id: 2580, .. })));
    assert!(s.take_errors().is_empty());
}

#[test]
fn the_hover_names_the_tracking() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = setup();
    s.run(r#"MiniMapTrackingButton:GetScript("OnEnter")(MiniMapTrackingButton)"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        s.eval::<String>("return MINIMAP_TRACKING_TOOLTIP_NONE")
            .unwrap()
    );
    s.run("SetTracking(13)").unwrap();
    s.run(r#"MiniMapTrackingButton:GetScript("OnEnter")(MiniMapTrackingButton)"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        s.eval::<String>("return MINIMAP_TRACKING_BANKER").unwrap()
    );
    assert!(s.take_errors().is_empty());
}
