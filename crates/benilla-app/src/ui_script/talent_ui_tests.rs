//! The stock 3.3.5a talent window (`Blizzard_TalentUI`, `TalentFrameBase.lua`) on the production
//! load, over a talent model the live client builds: a dwarf hunter's three trees from the
//! install, two talent groups and a learned talent, as `SMSG_TALENTS_INFO` would name them.

use std::collections::{BTreeSet, HashMap};

use benilla_ui::script::{ScriptValue, UiScript};

use crate::ui_talent::{build_pages, PlayerTalentGroups};

const DWARF: u8 = 3;
const HUNTER: u8 = 3;

/// Errors from the talent window's own code; the production load carries unrelated 3.3.5a gaps.
fn talent_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Talent", "SpecTab", "Glyph"].iter().any(|k| e.contains(k)))
        .collect()
}

/// The hunter's first tree, its first talent at rank 2 in group 1 (active, one point left), and
/// an empty group 2.
fn push_talents(s: &mut UiScript) -> u32 {
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let catalog = benilla_formats::load_talent_catalog(&mut chain).unwrap();
    let spells = benilla_formats::load_spell_catalog(&mut chain).unwrap();
    let first_tab = catalog.tabs_for_class(DWARF, HUNTER)[0].id;
    let first = catalog.talents_in_tab(first_tab)[0].id;
    let mut held = PlayerTalentGroups::fresh(1);
    held.ranks = vec![HashMap::from([(first, 1u8)]), HashMap::new()];
    let state = build_pages(
        &catalog,
        &BTreeSet::new(),
        &spells,
        DWARF,
        HUNTER,
        (1, 0),
        &held,
    );
    s.set_talents(state);
    s.fire_event("PLAYER_TALENT_UPDATE", Vec::<ScriptValue>::new());
    first
}

/// The Talents button's chain (`ToggleTalentFrame` → `TalentFrame_LoadUI` →
/// `UIParentLoadAddOn("Blizzard_TalentUI")`) loads the stock window, which opens on the active
/// spec with its trees, ranks and points; a click on a talent with points left queues its next
/// rank, and the inactive spec's tab is there to select.
#[test]
fn the_stock_talent_window_opens_on_the_model_and_learns_a_rank() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("talents", false, "", |_| {});
    let _ = s.take_errors();
    let first = push_talents(&mut s);

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
    assert_eq!(
        s.take_talent_learns(),
        vec![(first, 2)],
        "the click learns the next 0-based rank"
    );

    // The inactive spec: its tab selects it, the Activate button shows and spends nothing.
    s.run("PlayerSpecTab2:Click()").unwrap();
    s.run(
        r#"
        assert(PlayerTalentFrameActivateButton:IsShown(), "an inactive spec offers Activate")
        PlayerTalentFrameTalent1:Click("LeftButton")
        "#,
    )
    .unwrap();
    assert!(
        s.take_talent_learns().is_empty(),
        "an inactive spec's buttons learn nothing"
    );
    let errors = talent_errors(s.take_errors());
    assert!(errors.is_empty(), "talent window errors: {errors:#?}");
}
