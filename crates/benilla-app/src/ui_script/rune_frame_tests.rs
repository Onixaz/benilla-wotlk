//! The stock rune bar (`RuneFrame.xml`) on the production load: every class loads it clean, and a
//! death knight's buttons follow the rune table through `RUNE_POWER_UPDATE` and
//! `RUNE_TYPE_UPDATE`.

use benilla_ui::script::{PlayerRecord, RuneState, ScriptValue, UiScript, RUNE_KINDS};

/// The production load as `class`, with the environment lock the load needs held.
fn setup(class: (&str, &str)) -> (std::sync::MutexGuard<'static, ()>, UiScript, Vec<String>) {
    let lock = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) = super::layer_tests::production_load_seeded(
        "runes",
        false,
        "",
        |_| {},
        |s| {
            s.set_player_record(PlayerRecord {
                name: "Probedk".into(),
                class: Some((class.0.into(), class.1.into())),
                ..Default::default()
            });
        },
    );
    s.resolve();
    (lock, s, failures)
}

/// A death knight's table: ten seconds a rune for every kind.
fn dk_runes() -> RuneState {
    RuneState {
        regen: [0.1; RUNE_KINDS],
        ..Default::default()
    }
}

/// A session clock past zero: a last-used time of 0 reads as a ready rune.
const NOW_MS: u32 = 50_000;

fn power_update(s: &mut UiScript, rune: i64, usable: bool) {
    s.fire_event(
        "RUNE_POWER_UPDATE",
        vec![ScriptValue::Int(rune), ScriptValue::Bool(usable)],
    );
}

/// `RuneButton_OnLoad` reads every rune's kind on any class, so a missing verb raised six times.
#[test]
fn the_rune_bar_loads_clean_and_hides_off_a_death_knight() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, s, failures) = setup(("Warrior", "WARRIOR"));
    let ours: Vec<_> = failures.iter().filter(|f| f.contains("Rune")).collect();
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    assert!(!s.eval::<bool>("return RuneFrame:IsShown()").unwrap());
}

/// A spent rune runs its button's cooldown until the table reads it ready; a conversion repaints
/// the icon.
#[test]
fn a_death_knight_s_runes_follow_the_table() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s, failures) = setup(("Death Knight", "DEATHKNIGHT"));
    let ours: Vec<_> = failures.iter().filter(|f| f.contains("Rune")).collect();
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    assert!(s.eval::<bool>("return RuneFrame:IsShown()").unwrap());
    s.set_runes(dk_runes());
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    let icon = |s: &UiScript, i: u32| {
        s.eval::<String>(&format!("return RuneButtonIndividual{i}Rune:GetTexture()"))
            .unwrap()
    };
    assert!(icon(&s, 1).contains("Blood"), "{}", icon(&s, 1));
    // `RuneFrame_FixRunes` swaps the frost and unholy buttons at the first entry.
    assert!(icon(&s, 3).contains("Frost"), "{}", icon(&s, 3));
    assert!(icon(&s, 5).contains("Unholy"), "{}", icon(&s, 5));

    // Rune 1 spent now: the event arms the button's ticker, which starts the cooldown.
    let mut runes = dk_runes();
    runes.set_predicted(0x3f, 0x3e, None, NOW_MS);
    s.set_runes(runes.clone());
    power_update(&mut s, 1, false);
    let tick = "local b = RuneButtonIndividual1 local f = b:GetScript('OnUpdate') \
        if f then f(b, 0.1) end return b:GetScript('OnUpdate') ~= nil";
    assert!(s.eval::<bool>(tick).unwrap(), "the ticker runs while spent");
    assert!(s
        .eval::<bool>("return RuneButtonIndividual1Cooldown:IsShown()")
        .unwrap());

    // Ready again: the next tick sees it and stops.
    runes.set_predicted(0x3e, 0x3f, None, NOW_MS);
    s.set_runes(runes.clone());
    power_update(&mut s, 1, true);
    assert!(
        !s.eval::<bool>(tick).unwrap(),
        "a ready rune stops its ticker"
    );

    // A death rune repaints the button.
    runes.convert(0, 3);
    s.set_runes(runes);
    s.fire_event("RUNE_TYPE_UPDATE", vec![ScriptValue::Int(1)]);
    assert!(icon(&s, 1).contains("Death"), "{}", icon(&s, 1));
    // A bare VM's `PLAYER_ENTERING_WORLD` trips other frames; only the rune bar's count here.
    let errors: Vec<_> = s
        .take_errors()
        .into_iter()
        .filter(|e| e.contains("Rune"))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}
