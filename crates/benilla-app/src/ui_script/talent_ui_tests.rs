//! The stock 3.3.5a talent window (`Blizzard_TalentUI`, `TalentFrameBase.lua`) on the production
//! load, over the talent state the live client keeps, lent at each call: a dwarf hunter's three
//! trees from the install, two talent groups and a learned talent, as `SMSG_TALENTS_INFO` names
//! them.

use benilla_ui::script::{ScriptValue, TalentInfo, UiScript};

use crate::ui_talent::tests::{body, catalogs, valid_over, TalentTestView};
use crate::ui_talent::{pet_pages, player_pages};

const DWARF: u8 = 3;
const HUNTER: u8 = 3;
const ME: u64 = 0x10;
const PET: u64 = 0xF140_0000_0000_0021;

/// Errors from the talent window's own code; the production load carries unrelated 3.3.5a gaps.
fn talent_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Talent", "SpecTab", "Glyph"].iter().any(|k| e.contains(k)))
        .collect()
}

/// The hunter's tabs built at the world entry, its first talent at rank 2 in group 1 (active, one
/// point left), and an empty group 2; the talent's id.
fn hunter_view() -> (TalentTestView, u32) {
    let (catalog, spells) = catalogs();
    let pages = player_pages(&catalog, &spells, DWARF, HUNTER);
    let first = pages.talents[0][0].id;
    let mut info = TalentInfo::default();
    info.update_talent_data(
        &body(1, 0, vec![(vec![(first, 1)], vec![]), (vec![], vec![])]),
        &valid_over(&catalog, &spells),
    );
    info.enter_world(|| Some(pages));
    (
        TalentTestView {
            info,
            units: (Some(ME), None),
            ..Default::default()
        },
        first,
    )
}

/// The Talents button's chain (`ToggleTalentFrame` → `TalentFrame_LoadUI` →
/// `UIParentLoadAddOn("Blizzard_TalentUI")`) loads the stock window, which opens on the active
/// spec with its trees, ranks and points; a click on a talent with points left sends its next
/// rank inside the click, and the inactive spec's tab is there to select.
#[test]
fn the_stock_talent_window_opens_on_the_state_and_learns_a_rank() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("talents", false, "", |_| {});
    let _ = s.take_errors();
    let (mut v, first) = hunter_view();
    s.with_live(&mut v, |s| {
        s.fire_event("PLAYER_TALENT_UPDATE", Vec::<ScriptValue>::new());
        s.run("ToggleTalentFrame()").unwrap();
        let errors = talent_errors(s.take_errors());
        assert!(errors.is_empty(), "talent window errors: {errors:#?}");
        s.run(
            r#"
        assert(IsAddOnLoaded("Blizzard_TalentUI") == 1, "the addon loaded")
        assert(PlayerTalentFrame:IsShown(), "the window is open")
        assert(PlayerSpecTab1:IsShown() and PlayerSpecTab2:IsShown(), "both specs have a tab")
        assert(not PlayerSpecTab3:IsShown(), "no pet spec without pet talents")
        assert(PlayerTalentFrameTalent1:IsShown(), "the first talent has a button")
        local rank = PlayerTalentFrameTalent1Rank:GetText()
        assert(rank == "2", "the learned rank reads 2, got " .. tostring(rank))
        local points = PlayerTalentFramePointsBar and PlayerTalentFrameTalentPointsText:GetText()
        assert(points and points:find("1"), "one point unspent, got " .. tostring(points))
        assert(PlayerTalentFrameTab1:GetText() ~= nil, "the first tree names its tab")
        "#,
        )
        .unwrap();
        s.run(r#"PlayerTalentFrameTalent1:Click("LeftButton")"#)
            .unwrap();
    });
    assert_eq!(
        v.learns,
        vec![(None, first, 2)],
        "the click learns the next 0-based rank"
    );
    v.learns.clear();
    s.with_live(&mut v, |s| {
        // The inactive spec: its tab selects it, the Activate button shows and spends nothing.
        s.run("PlayerSpecTab2:Click()").unwrap();
        s.run(
            r#"
        assert(PlayerTalentFrameActivateButton:IsShown(), "an inactive spec offers Activate")
        PlayerTalentFrameTalent1:Click("LeftButton")
        "#,
        )
        .unwrap();
    });
    assert!(
        v.learns.is_empty(),
        "an inactive spec's buttons learn nothing"
    );
    let errors = talent_errors(s.take_errors());
    assert!(errors.is_empty(), "talent window errors: {errors:#?}");
}

/// The stock tree never calls `UpdateScrollChildRect`: the bar learns its range from the engine's
/// own `OnScrollRangeChanged` once the laid-out tree outgrows the frame, and then a wheel spin
/// and the down arrow scroll it (#162).
#[test]
fn the_stock_talent_tree_scrolls_by_wheel_and_arrow() {
    use super::keybindings_tests::{centre, click};
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("talents", false, "", |_| {});
    let _ = s.take_errors();
    let (mut v, _) = hunter_view();
    s.with_live(&mut v, |s| {
        s.run("ToggleTalentFrame()").unwrap();
        s.resolve();

        const SF: &str = "PlayerTalentFrameScrollFrame";
        const BAR: &str = "PlayerTalentFrameScrollFrameScrollBar";
        let (range, lo, hi): (f64, f64, f64) = s
            .eval(&format!(
                "local lo, hi = {BAR}:GetMinMaxValues() \
                 return {SF}:GetVerticalScrollRange(), lo, hi"
            ))
            .unwrap();
        assert!(
            range > 0.0,
            "the hunter's tree outgrows the frame, range {range}"
        );
        assert_eq!((lo, hi), (0.0, range), "the bar spans the measured range");

        let scroll = |s: &UiScript| {
            s.eval::<f64>(&format!("return {SF}:GetVerticalScroll()"))
                .unwrap()
        };
        let (x, y) = centre(s, SF);
        s.mouse_wheel(x, y, -1.0);
        s.resolve();
        let wheeled = scroll(s);
        assert!(wheeled > 0.0, "a wheel notch down scrolls the tree");

        s.run(&format!("{BAR}:SetValue(0)")).unwrap();
        s.resolve();
        assert_eq!(scroll(s), 0.0, "the bar scrolls back to the top");
        click(s, &format!("{BAR}ScrollDownButton"), "LeftButton");
        s.resolve();
        assert!(scroll(s) > 0.0, "the down arrow scrolls the tree");
    });
    let errors = talent_errors(s.take_errors());
    assert!(errors.is_empty(), "talent window errors: {errors:#?}");
}

/// A pet with a talent tree (`GetNumTalentGroups(false, true)` 1) gets the pet spec tab; on it the
/// window shows the pet's tree, and a talent click sends the pet's learn, not the player's.
#[test]
fn the_pet_spec_tab_shows_the_pets_tree_and_learns_for_the_pet() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("talents", false, "", |_| {});
    let _ = s.take_errors();
    let (mut v, _) = hunter_view();

    let (catalog, spells) = catalogs();
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let families = benilla_formats::load_creature_families(&mut chain).unwrap();
    let wolf = families.get(1).unwrap();
    let pages = pet_pages(&catalog, &spells, &families, 1).expect("the wolf's tree");
    let first = pages.talents[0][0].id;
    v.info.update_pet_tabs(1, || Some(pages));
    v.info
        .update_pet_talent_data(3, &[], &valid_over(&catalog, &spells));
    v.units.1 = Some(PET);
    v.pet_category = Some(wolf.category);
    s.set_unit(
        "pet",
        Some(benilla_ui::script::UnitState {
            exists: true,
            has_object: true,
            name: Some("Wolf".into()),
            health: 100,
            max_health: 100,
            level: 10,
            ..Default::default()
        }),
    );
    s.with_live(&mut v, |s| {
        s.fire_event("PET_TALENT_UPDATE", Vec::<ScriptValue>::new());
        s.run("ToggleTalentFrame()").unwrap();
        s.run(
            r#"
        assert(PlayerSpecTab3:IsShown(), "a pet with a tree has the pet spec tab")
        PlayerSpecTab3:Click()
        assert(PlayerTalentFrame.pet, "the pet spec is selected")
        assert(PlayerTalentFrameTalent1:IsShown(), "the pet's first talent has a button")
        PlayerTalentFrameTalent1:Click("LeftButton")
        "#,
        )
        .unwrap();
    });
    assert_eq!(
        v.learns,
        vec![(Some(PET), first, 0)],
        "the pet's learn with its guid, nothing for the player"
    );
    let errors = talent_errors(s.take_errors());
    assert!(errors.is_empty(), "talent window errors: {errors:#?}");
}

/// The stock inspect window's talent tab (`InspectTalentFrame.lua`) reads the inspected player's
/// set: a human paladin's trees with two points in the first talent, not the player's hunter trees
/// and ranks. `INSPECT_TALENT_READY` refreshes it.
#[test]
fn the_stock_inspect_talent_tab_shows_the_inspected_players_trees() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("talents", false, "", |_| {});
    let _ = s.take_errors();
    let (mut v, _) = hunter_view();

    const HUMAN: u8 = 1;
    const PALADIN: u8 = 2;
    let (catalog, spells) = catalogs();
    let pages = player_pages(&catalog, &spells, HUMAN, PALADIN);
    let first = pages.talents[0][0].id;
    let first_tab = pages.tabs[0].name.clone();
    v.info.set_inspect_target(0x77, Some(pages));
    v.info.update_inspect_talent_data(
        &body(0, 0, vec![(vec![(first, 1)], vec![])]),
        &valid_over(&catalog, &spells),
    );

    s.with_live(&mut v, |s| {
        s.run(
            r#"
        LoadAddOn("Blizzard_InspectUI")
        InspectFrame.unit = "player"
        InspectFrame:Show()
        InspectTalentFrame:Show()
        "#,
        )
        .unwrap();
        s.fire_event("INSPECT_TALENT_READY", Vec::<ScriptValue>::new());
        let tab = s
            .eval::<String>("return InspectTalentFrameTab1:GetText()")
            .unwrap();
        assert_eq!(tab, first_tab, "the inspected class's first tree");
        assert_ne!(
            tab,
            s.eval::<String>("return (GetTalentTabInfo(1))").unwrap(),
            "not the player's own tree"
        );
        s.run(
            r#"
        local rank = InspectTalentFrameTalent1Rank:GetText()
        assert(rank == "2", "the inspected rank reads 2, got " .. tostring(rank))
        local spent = InspectTalentFrameSpentPointsText:GetText()
        assert(spent and spent:find("2"), "two points spent, got " .. tostring(spent))
        "#,
        )
        .unwrap();
    });
    let errors = talent_errors(s.take_errors());
    assert!(errors.is_empty(), "inspect talent errors: {errors:#?}");
}
