//! The stock 3.3.5a pet paper doll (`PetPaperDollFrame.xml`) in the production load: three pages
//! behind sub-tabs, Pet, Companions and Mounts, each shown only with something to show
//! (`PetPaperDollFrame_UpdateTabs`), and the character window's Pets tab hidden when none is.

use benilla_ui::script::{
    CastAt, CompanionIntent, CompanionState, CompanionView, PetStats, ScriptCall, ScriptValue,
    UiScript, UnitCombatStats, UnitState,
};

/// The production load; every failure naming the pet page is a test failure.
fn load(tag: &str) -> UiScript {
    let (s, failures) = super::layer_tests::production_load_seeded(tag, false, "", |_| {}, |_| {});
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("PetPaperDoll") || f.contains("Companion"))
        .collect();
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    s
}

/// This page's errors: the rest of the stock UI raises on verbs other work builds.
fn errors(s: &mut UiScript) -> Vec<String> {
    s.take_errors()
        .into_iter()
        .filter(|e| e.contains("PetPaperDoll") || e.contains("Companion"))
        .collect()
}

/// `ToggleCharacter(tab)`.
fn toggle(s: &mut UiScript, tab: &str) {
    s.run(&format!("ToggleCharacter('{tab}')")).unwrap();
    s.resolve();
}

fn critter(spell_id: u32, creature_id: u32, name: &str) -> CompanionView {
    CompanionView {
        creature_id,
        name: Some(name.into()),
        spell_id,
        icon: Some(format!("Interface\\Icons\\Critter{spell_id}")),
        active: false,
        cooldown: None,
    }
}

/// Two critters and one mount, as the app's feed pushes them.
fn companions() -> CompanionState {
    CompanionState {
        critters: vec![
            critter(10673, 7385, "Bombay Cat"),
            critter(4055, 2671, "Squirrel"),
        ],
        mounts: vec![critter(458, 284, "Brown Horse")],
        random_pool: Vec::new(),
    }
}

fn pet_unit() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Snarl".into()),
        health: 900,
        max_health: 1000,
        level: 58,
        ..UnitState::default()
    }
}

fn give_pet(s: &mut UiScript) {
    s.set_unit("pet", Some(pet_unit()));
    s.set_pet_stats(
        true,
        PetStats {
            hunter_pet: true,
            experience: (4200, 8000),
            family: Some("Boar".into()),
            ..PetStats::default()
        },
    );
    s.set_pet_combat_stats(Some(UnitCombatStats {
        stats: [123, 88, 210, 20, 45],
        resistances: [2400, 0, 55, 0, 0, 0, 30],
        min_damage: 90.0,
        max_damage: 130.0,
        main_attack_time_ms: 2000,
        attack_power: 640,
        ..UnitCombatStats::default()
    }));
    s.fire_event("PET_BAR_UPDATE", vec![]);
}

fn shown(s: &mut UiScript, frame: &str) -> bool {
    s.resolve();
    s.eval::<bool>(&format!("return {frame}:IsShown() ~= nil"))
        .unwrap()
}

fn visible(s: &mut UiScript, frame: &str) -> bool {
    s.resolve();
    s.eval::<bool>(&format!("return {frame}:IsVisible() ~= nil"))
        .unwrap()
}

fn enter_world(s: &mut UiScript) {
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
}

fn open_pet_page(s: &mut UiScript) {
    toggle(s, "PetPaperDollFrame");
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `PetPaperDollFrame_UpdateIsAvailable` (`:51-64`): no pet UI, no critter and no mount hide the
/// character window's Pets tab.
#[test]
fn with_nothing_to_show_the_pets_tab_hides() {
    benilla_formats::wow_data_or_skip!();
    let _l = env_lock();
    let mut s = load("pet-none");
    enter_world(&mut s);
    toggle(&mut s, "PaperDollFrame");
    s.fire_event("PET_UI_UPDATE", vec![]);
    s.resolve();
    assert!(!shown(&mut s, "CharacterFrameTab2"), "no Pets tab");
    open_pet_page(&mut s);
    assert!(
        !visible(&mut s, "PetPaperDollFrame"),
        "asking for the page without anything to show does nothing"
    );
    assert_eq!(errors(&mut s), Vec::<String>::new());
}

/// With companions the tab shows and opens on the Companions page: the grid, the model's name, the
/// sub-tabs, and Summon; the Mounts tab switches the list.
#[test]
fn companions_open_the_companion_page_and_the_mounts_tab_switches_it() {
    benilla_formats::wow_data_or_skip!();
    let _l = env_lock();
    let mut s = load("pet-companions");
    enter_world(&mut s);
    s.set_companions(companions());
    s.fire_event("COMPANION_UPDATE", vec![]);
    toggle(&mut s, "PaperDollFrame");
    s.resolve();
    assert!(shown(&mut s, "CharacterFrameTab2"), "the Pets tab shows");
    open_pet_page(&mut s);
    assert_eq!(errors(&mut s), Vec::<String>::new());
    assert!(visible(&mut s, "PetPaperDollFrameCompanionFrame"));
    assert!(!visible(&mut s, "PetPaperDollFramePetFrame"));
    assert!(
        !shown(&mut s, "PetPaperDollFrameTab1"),
        "no pet, no Pet tab"
    );
    assert!(shown(&mut s, "PetPaperDollFrameTab2") && shown(&mut s, "PetPaperDollFrameTab3"));
    assert_eq!(
        s.eval::<String>("return PetNameText:GetText()").unwrap(),
        s.eval::<String>("return COMPANIONS").unwrap()
    );
    assert_eq!(
        s.eval::<String>("return CompanionSelectedName:GetText()")
            .unwrap(),
        "Bombay Cat"
    );
    assert!(s
        .eval::<bool>(
            "return CompanionButton1:IsEnabled() == 1 and CompanionButton2:IsEnabled() == 1 \
             and CompanionButton3:IsEnabled() == 0 and CompanionButton1:GetChecked() == 1"
        )
        .unwrap());
    assert_eq!(
        s.eval::<String>("return CompanionButton1:GetNormalTexture():GetTexture()")
            .unwrap()
            .to_ascii_lowercase(),
        "interface\\icons\\critter10673"
    );
    assert_eq!(
        s.eval::<String>("return CompanionSummonButton:GetText()")
            .unwrap(),
        s.eval::<String>("return SUMMON").unwrap()
    );
    let model = s
        .model_pane("CompanionModelFrame")
        .expect("the preview pane");
    assert_eq!(model.creature, Some(7385), "SetCreature names the cat");

    s.run("PetPaperDollFrameTab3:Click()").unwrap();
    s.resolve();
    assert_eq!(errors(&mut s), Vec::<String>::new());
    assert_eq!(
        s.eval::<String>("return PetPaperDollFrameCompanionFrame.mode")
            .unwrap(),
        "MOUNT"
    );
    assert_eq!(
        s.eval::<String>("return CompanionSelectedName:GetText()")
            .unwrap(),
        "Brown Horse"
    );
    assert_eq!(
        s.eval::<String>("return CompanionSummonButton:GetText()")
            .unwrap(),
        s.eval::<String>("return MOUNT").unwrap()
    );
}

/// Summon casts the selected companion (`CallCompanion` `0x53e5e0`); once it is out the button
/// reads Dismiss and `DismissCompanion` asks the app to send it home.
#[test]
fn summon_casts_the_selection_and_dismiss_sends_it_home() {
    benilla_formats::wow_data_or_skip!();
    let _l = env_lock();
    let mut s = load("pet-summon");
    enter_world(&mut s);
    s.set_companions(companions());
    s.fire_event("COMPANION_UPDATE", vec![]);
    toggle(&mut s, "PaperDollFrame");
    open_pet_page(&mut s);
    let _ = s.take_script_calls();

    s.run("CompanionButton_OnEnter(CompanionButton1)").unwrap();
    s.run("CompanionSummonButton:Click()").unwrap();
    assert_eq!(
        s.take_script_calls(),
        vec![ScriptCall::CastSpell {
            spell_id: 10673,
            at: CastAt::Selection,
            via_book: false,
        }]
    );

    let mut out = companions();
    out.critters[0].active = true;
    s.set_companions(out);
    s.fire_event("COMPANION_UPDATE", vec![ScriptValue::Str("CRITTER".into())]);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return CompanionSummonButton:GetText()")
            .unwrap(),
        s.eval::<String>("return PET_DISMISS").unwrap()
    );
    assert!(shown(&mut s, "CompanionButton1ActiveTexture"));
    s.run("CompanionSummonButton:Click()").unwrap();
    assert_eq!(
        s.take_companion_intents(),
        vec![CompanionIntent::Dismiss(10673)]
    );
    assert_eq!(errors(&mut s), Vec::<String>::new());
}

/// With a pet the page opens on the Pet tab, its stats drawn, and the companion tabs beside it.
#[test]
fn a_pet_opens_the_pet_page_with_its_stats() {
    benilla_formats::wow_data_or_skip!();
    let _l = env_lock();
    let mut s = load("pet-pet");
    enter_world(&mut s);
    give_pet(&mut s);
    s.set_companions(companions());
    s.fire_event("COMPANION_UPDATE", vec![]);
    toggle(&mut s, "PaperDollFrame");
    open_pet_page(&mut s);
    assert_eq!(errors(&mut s), Vec::<String>::new());
    assert!(visible(&mut s, "PetPaperDollFramePetFrame"));
    assert!(!visible(&mut s, "PetPaperDollFrameCompanionFrame"));
    assert!(shown(&mut s, "PetPaperDollFrameTab1"));
    assert_eq!(
        s.eval::<String>("return PetNameText:GetText()").unwrap(),
        "Snarl"
    );
    assert_eq!(
        s.eval::<String>("return PetStatFrame1StatText:GetText()")
            .unwrap(),
        "123"
    );
}
