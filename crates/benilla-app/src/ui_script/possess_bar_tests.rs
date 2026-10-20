//! The stock possess bar, `PossessBarFrame` in `BonusActionBarFrame.xml`, on the production load:
//! a pushed possess spell shows its two slots, the cancel slot cancels the spell's aura by name
//! (`BonusActionBarFrame.lua:266-278`), and the hover is the spell's tooltip.

use benilla_ui::script::{AuraState, PossessView, SpellTooltipView, UiScript, UnitGuids};

const ME: u64 = 0x10;
const SPELL: u32 = 9001;

/// The production load; every failure naming the bonus bars is a test failure.
fn load(tag: &str) -> UiScript {
    let (s, failures) = super::layer_tests::production_load_seeded(tag, false, "", |_| {}, |_| {});
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("BonusActionBar") || f.contains("Possess"))
        .collect();
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    s
}

fn errors(s: &mut UiScript) -> Vec<String> {
    s.take_errors()
        .into_iter()
        .filter(|e| e.contains("BonusActionBar") || e.contains("Possess"))
        .collect()
}

fn shown(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsShown() ~= nil"))
        .unwrap()
}

fn texture(s: &UiScript, region: &str) -> Option<String> {
    s.eval::<Option<String>>(&format!("return {region}:GetTexture()"))
        .unwrap()
}

fn held() -> PossessView {
    PossessView {
        spell_id: SPELL,
        bar_visible: true,
        name: Some("possess-spell".into()),
        icon: Some("spell-icon".into()),
        cancel_icon: Some("cancel-icon".into()),
        cancel_enabled: true,
    }
}

#[test]
fn the_possess_bar_shows_the_held_spell_and_cancels_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load("possess-bar");
    s.set_unit_guids(&UnitGuids {
        player: ME,
        ..Default::default()
    });
    // The caster's own aura of the channel, the one the cancel slot names.
    s.set_player_auras(vec![AuraState {
        spell_id: SPELL,
        name: Some("possess-spell".into()),
        count: 1,
        helpful: true,
        channeled: true,
        caster: ME,
        ..Default::default()
    }]);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    assert!(!shown(&s, "PossessBarFrame"), "no possess spell, no bar");

    s.set_possess_spell(Some(held()));
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.resolve();
    assert!(shown(&s, "PossessBarFrame"));
    assert!(shown(&s, "PossessButton1") && shown(&s, "PossessButton2"));
    assert!(texture(&s, "PossessButton1Icon").is_some_and(|t| t.contains("spell-icon")));
    assert!(texture(&s, "PossessButton2Icon").is_some_and(|t| t.contains("cancel-icon")));

    // The spell slot's hover is the spell's tooltip (`GameTooltip:SetPossession(1)`).
    s.set_spell_tooltip(
        SPELL,
        SpellTooltipView {
            name: "possess-spell".into(),
            ..Default::default()
        },
    );
    s.run("PossessButton1:GetScript('OnEnter')(PossessButton1)")
        .unwrap();
    assert_eq!(
        s.eval::<Option<String>>("return GameTooltipTextLeft1:GetText()")
            .unwrap()
            .as_deref(),
        Some("possess-spell")
    );
    s.run("GameTooltip:Hide()").unwrap();

    // The cancel slot cancels the caster's aura by the spell's name.
    s.run("PossessButton2:Click('LeftButton')").unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![SPELL]);

    // A cancel-disabled spell hides that slot alone; no spell hides the bar.
    s.set_possess_spell(Some(PossessView {
        cancel_enabled: false,
        ..held()
    }));
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert!(shown(&s, "PossessButton1") && !shown(&s, "PossessButton2"));
    s.set_possess_spell(None);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert!(!shown(&s, "PossessBarFrame"));

    let run = errors(&mut s);
    assert!(run.is_empty(), "runtime errors: {run:#?}");
}

/// `IsAtStableMaster()` (`0x5a1090`) is a real boolean, false with no stable open, so the stock
/// `PetStable_Update` that a pet change runs (`PetStable.lua:22-25`) raises nothing (#408).
#[test]
fn a_pet_change_away_from_a_stable_raises_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load("possess-stable");
    s.take_errors();
    assert_eq!(
        s.eval::<String>("local v = IsAtStableMaster() return type(v) .. ' ' .. tostring(v)")
            .unwrap(),
        "boolean false"
    );
    s.fire_event(
        "UNIT_PET",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
    let stable: Vec<_> = s
        .take_errors()
        .into_iter()
        .filter(|e| e.contains("PetStable"))
        .collect();
    assert!(stable.is_empty(), "runtime errors: {stable:#?}");
    s.set_stable(Some(benilla_ui::script::StableState {
        num_stable_slots: 0,
        next_slot_cost: 0,
        slots: Default::default(),
        has_live_pet: false,
    }));
    assert_eq!(
        s.eval::<String>("local v = IsAtStableMaster() return type(v) .. ' ' .. tostring(v)")
            .unwrap(),
        "boolean true"
    );
}
