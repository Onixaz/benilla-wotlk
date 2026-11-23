//! The stock 3.3.5a Reputation tab and its watch bar, both `ReputationFrame.xml`, on the production
//! load: what the faction list becomes on screen.

use benilla_ui::script::{
    FactionEntry, QuadContent, ReputationSend, ScriptValue, UiScript, UnitState,
};

use super::test_ui::{centre_of, click, hover, unhover};
use crate::ui_reputation::rep_view::RepView;

/// A met faction bar: Friendly, 1000 into its 3000..9000 rank window.
fn entry(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        faction_id,
        rep_list_id,
        parent_id,
        name: name.into(),
        description: format!("About the {name}."),
        standing: 4000,
        standing_id: 5,
        bar_min: 3000,
        bar_max: 9000,
        fits_player: true,
        visible: true,
        is_header: false,
        at_war: false,
        peace_forced: false,
        inactive: false,
        has_rep: false,
    }
}

/// A header faction as the wire seeds one: flag `0x08`, unmet, at Neutral 0.
fn header(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
    FactionEntry {
        standing: 0,
        standing_id: 4,
        bar_min: 0,
        bar_max: 3000,
        visible: false,
        is_header: true,
        ..entry(faction_id, rep_list_id, parent_id, name)
    }
}

/// `benilla-ui`'s nested fixture. Rows: 1 Classic, 2 Alliance, 3 Ironforge, 4 Stormwind,
/// 5 Steamwheedle Cartel, 6 Booty Bay, 7 Wrath of the Lich King, 8 Argent Crusade, 9 Alliance
/// Vanguard (a header with its own bar, Revered), 10 Valiance Expedition, 11 Other, 12 Argent
/// Dawn, 13 Bloodsail Buccaneers.
fn state() -> Vec<FactionEntry> {
    vec![
        header(1118, 96, 0, "Classic"),
        header(469, 11, 1118, "Alliance"),
        entry(72, 19, 469, "Stormwind"),
        entry(47, 20, 469, "Ironforge"),
        header(169, 10, 1118, "Steamwheedle Cartel"),
        entry(21, 1, 169, "Booty Bay"),
        header(1097, 89, 0, "Wrath of the Lich King"),
        entry(1106, 101, 1097, "Argent Crusade"),
        FactionEntry {
            visible: true,
            has_rep: true,
            standing: 21500,
            standing_id: 7,
            bar_min: 21000,
            bar_max: 42000,
            ..header(1037, 88, 1097, "Alliance Vanguard")
        },
        entry(1050, 92, 1037, "Valiance Expedition"),
        entry(529, 13, 0, "Argent Dawn"),
        entry(87, 0, 0, "Bloodsail Buccaneers"),
    ]
}

const ROWS: [&str; 13] = [
    "Classic",
    "Alliance",
    "Ironforge",
    "Stormwind",
    "Steamwheedle Cartel",
    "Booty Bay",
    "Wrath of the Lich King",
    "Argent Crusade",
    "Alliance Vanguard",
    "Valiance Expedition",
    "Other",
    "Argent Dawn",
    "Bloodsail Buccaneers",
];

fn player(level: u32) -> UnitState {
    UnitState {
        exists: true,
        name: Some("Probesix".into()),
        level,
        race: Some("Human".into()),
        race_file: Some("Human".into()),
        class: Some("Mage".into()),
        class_file: Some("MAGE".into()),
        is_player: true,
        ..UnitState::default()
    }
}

/// Errors from the pane's own code; the production load carries unrelated 3.3.5a gaps.
fn pane_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("Reputation") || e.contains("Faction"))
        .collect()
}

/// The production load on a Wrath account (`ReputationFrame_OnLoad` reads the level cap off
/// `GetAccountExpansionLevel()`), a level-40 player and the list built over `st` as the world entry
/// builds it, its events raised with the view lent.
fn loaded(tag: &str, st: Vec<FactionEntry>) -> (UiScript, RepView) {
    let (mut s, failures) = super::layer_tests::production_load_seeded(
        tag,
        false,
        "",
        |_| {},
        |s| s.set_account_expansion(2),
    );
    let ours = pane_errors(failures);
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    let _ = s.take_errors();
    s.set_unit("player", Some(player(40)));
    let mut v = RepView::new(st);
    v.enter_world(&mut s);
    (s, v)
}

/// [`loaded`], with the window open on the Reputation tab.
fn shown_page(tag: &str, st: Vec<FactionEntry>) -> (UiScript, RepView) {
    let (mut s, mut v) = loaded(tag, st);
    v.lend(&mut s, |s| {
        s.run(r#"ToggleCharacter("ReputationFrame")"#).unwrap();
        s.resolve();
    });
    (s, v)
}

fn text_of(s: &mut UiScript, name: &str) -> String {
    s.eval::<Option<String>>(&format!("return {name}:GetText()"))
        .unwrap()
        .unwrap_or_default()
}

fn shown(s: &mut UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!("return {name}:IsShown() and true or false"))
        .unwrap()
}

/// `IsVisible`, not `IsShown`, for the scroll bar: when the list fits, `FauxScrollFrame_Update`
/// hides the scroll frame, not the bar.
fn visible(s: &mut UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!("return {name}:IsVisible() and true or false"))
        .unwrap_or(false)
}

fn global(s: &mut UiScript, name: &str) -> String {
    s.eval::<String>(&format!("return {name}")).unwrap()
}

fn assert_clean(s: &mut UiScript) {
    let errors = pane_errors(s.take_errors());
    assert!(errors.is_empty(), "pane errors: {errors:#?}");
}

/// `ToggleCharacter` selects the tab from the page's own `id=`: Reputation is 3.
#[test]
fn the_reputation_page_opens_on_the_windows_third_tab() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, mut v) = shown_page("rep-tab", state());
    v.lend(&mut s, |s| {
        assert!(visible(s, "ReputationFrame"));
        assert_eq!(
            s.eval::<i64>("return PanelTemplates_GetSelectedTab(CharacterFrame)")
                .unwrap(),
            3
        );
        assert_eq!(s.eval::<i64>("return GetNumFactions()").unwrap(), 13);
        assert!(shown(s, "ReputationBar13"));
        assert!(!shown(s, "ReputationBar14"), "slot 14 has no row");
        assert!(!visible(s, "ReputationListScrollFrameScrollBar"));
        assert_clean(s);
    });
}

/// Every row carries its faction's name, and `ReputationFrame_SetRowType(row, isChild +
/// 2*isHeader, hasRep)` places it: the fold button on headers only, the bar on factions and on a
/// header that is a bar, and the indent by row type (`ReputationFrame.lua:44-121`).
#[test]
fn every_row_names_its_faction_and_takes_its_tree_shape() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, mut v) = shown_page("rep-shape", state());
    v.lend(&mut s, |s| {
        // (indent, fold button, bar): rowType 0 sits at 44, 1 at 62, 2 at 20, 3 at 39.
        let expected: [(f32, bool, bool); 13] = [
            (20.0, true, false), // Classic
            (39.0, true, false), // Alliance
            (62.0, false, true),
            (62.0, false, true),
            (39.0, true, false), // Steamwheedle Cartel
            (62.0, false, true),
            (20.0, true, false), // Wrath of the Lich King
            (44.0, false, true), // Argent Crusade, directly under the expansion
            (39.0, true, true),  // Alliance Vanguard
            (62.0, false, true),
            (20.0, true, false), // Other
            (44.0, false, true),
            (44.0, false, true),
        ];
        for (i, (name, (indent, button, bar))) in ROWS.iter().zip(expected).enumerate() {
            let row = format!("ReputationBar{}", i + 1);
            assert_eq!(text_of(s, &format!("{row}FactionName")), *name);
            let left = s
                .eval::<f32>(&format!(
                    "return {row}:GetLeft() - ReputationFrame:GetLeft()"
                ))
                .unwrap();
            assert_eq!(left, indent, "{name}'s indent");
            assert_eq!(
                shown(s, &format!("{row}ExpandOrCollapseButton")),
                button,
                "{name}'s fold button"
            );
            assert_eq!(
                shown(s, &format!("{row}ReputationBar")),
                bar,
                "{name}'s bar"
            );
        }
        let revered = global(s, "FACTION_STANDING_LABEL7");
        assert_eq!(
            text_of(s, "ReputationBar9ReputationBarFactionStanding"),
            revered,
            "a header bar reads its own standing"
        );
        let icon = s
            .eval::<String>(
                "return ReputationBar1ExpandOrCollapseButton:GetNormalTexture():GetTexture()",
            )
            .unwrap();
        assert!(icon.contains("UI-MinusButton-Up"), "{icon}");
        assert_clean(s);
    });
}

/// The fold button calls the binding, whose sort raises `UPDATE_FACTION`, the pane's repaint.
#[test]
fn a_header_folds_on_its_button_and_the_pane_repaints() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, mut v) = shown_page("rep-fold", state());
    v.lend(&mut s, |s| {
        click(s, "ReputationBar2ExpandOrCollapseButton", "LeftButton"); // Alliance
        s.resolve();
        let icon = s
            .eval::<String>(
                "return ReputationBar2ExpandOrCollapseButton:GetNormalTexture():GetTexture()",
            )
            .unwrap();
        assert!(icon.contains("UI-PlusButton-Up"), "{icon}");
        assert_eq!(s.eval::<i64>("return GetNumFactions()").unwrap(), 11);
        assert_eq!(
            text_of(s, "ReputationBar3FactionName"),
            "Steamwheedle Cartel"
        );
        assert!(!shown(s, "ReputationBar12"));

        click(s, "ReputationBar1ExpandOrCollapseButton", "LeftButton"); // Classic
        s.resolve();
        assert_eq!(
            text_of(s, "ReputationBar2FactionName"),
            "Wrath of the Lich King",
            "the expansion folds its sub-headers with it"
        );
        click(s, "ReputationBar1ExpandOrCollapseButton", "LeftButton");
        s.resolve();
        assert_eq!(
            s.eval::<i64>("return GetNumFactions()").unwrap(),
            11,
            "Alliance keeps its own fold"
        );
        assert_clean(s);
    });
}

/// The bar's range is the rank window normalized (`ReputationFrame.lua:170-178`), its colour
/// `FACTION_BAR_COLORS[standingID]`; hovering swaps the standing for the numbers.
#[test]
fn a_bar_row_paints_its_standing_and_colour() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, mut v) = shown_page("rep-bar", state());
    v.lend(&mut s, |s| {
        let friendly = global(s, "FACTION_STANDING_LABEL5");
        let standing = "ReputationBar3ReputationBarFactionStanding";
        assert_eq!(text_of(s, standing), friendly);
        assert_eq!(
            s.eval::<(f64, f64, f64)>(
                "local b = ReputationBar3ReputationBar local lo, hi = b:GetMinMaxValues() \
             return lo, hi, b:GetValue()"
            )
            .unwrap(),
            (0.0, 6000.0, 1000.0)
        );
        let (r, g, b) = s
            .eval::<(f32, f32, f32)>("return ReputationBar3ReputationBar:GetStatusBarColor()")
            .unwrap();
        assert_eq!((r, g, b), (0.0, 0.6, 0.1));

        hover(s, "ReputationBar3");
        let numbers = s
            .eval::<String>(
                "return HIGHLIGHT_FONT_COLOR_CODE..' 1000 / 6000'..FONT_COLOR_CODE_CLOSE",
            )
            .unwrap();
        assert_eq!(text_of(s, standing), numbers);
        assert!(shown(s, "ReputationBar3ReputationBarHighlight1"));
        unhover(s);
        assert_eq!(text_of(s, standing), friendly);
        assert!(!shown(s, "ReputationBar3ReputationBarHighlight1"));
        assert_clean(s);
    });
}

/// The faux scroll kit re-binds the 15 fixed slots to a moving offset.
#[test]
fn the_scroll_offset_rebinds_the_fixed_row_slots() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut entries = vec![header(1118, 96, 0, "Classic")];
    for i in 1..=20u32 {
        entries.push(entry(2000 + i, i, 1118, &format!("Faction {i:02}")));
    }
    let (mut s, mut v) = shown_page("rep-scroll", entries);
    v.lend(&mut s, |s| {
        assert_eq!(s.eval::<i64>("return GetNumFactions()").unwrap(), 21);
        assert_eq!(text_of(s, "ReputationBar1FactionName"), "Classic");
        assert_eq!(text_of(s, "ReputationBar15FactionName"), "Faction 14");
        assert!(visible(s, "ReputationListScrollFrameScrollBar"));

        s.run("FauxScrollFrame_SetOffset(ReputationListScrollFrame, 3) ReputationFrame_Update()")
            .unwrap();
        assert_eq!(text_of(s, "ReputationBar1FactionName"), "Faction 03");
        assert!(!shown(s, "ReputationBar1ExpandOrCollapseButton"));
        assert_eq!(text_of(s, "ReputationBar15FactionName"), "Faction 17");

        s.run("FauxScrollFrame_SetOffset(ReputationListScrollFrame, 9) ReputationFrame_Update()")
            .unwrap();
        assert_eq!(text_of(s, "ReputationBar1FactionName"), "Faction 09");
        assert!(!shown(s, "ReputationBar15"), "past the list");
        assert_clean(s);
    });
}

/// A bar row opens the detail popup (`ReputationBar_OnClick` gates on `hasRep`); a peace-forced
/// faction's At War box is disabled and greyed, and so is a header's.
#[test]
fn clicking_a_bar_opens_the_detail_popup_on_that_faction() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut st = state();
    st[3].at_war = true; // Ironforge
    st[3].peace_forced = true;
    let (mut s, mut v) = shown_page("rep-detail", st);
    v.lend(&mut s, |s| {
        assert!(!shown(s, "ReputationDetailFrame"));
        assert!(shown(s, "ReputationBar3ReputationBarAtWarHighlight1"));
        click(s, "ReputationBar3", "LeftButton");
        assert!(shown(s, "ReputationDetailFrame"));
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 3);
        assert_eq!(text_of(s, "ReputationDetailFactionName"), "Ironforge");
        assert_eq!(
            text_of(s, "ReputationDetailFactionDescription"),
            "About the Ironforge."
        );
        let checked = |s: &mut UiScript, name: &str| {
            s.eval::<bool>(&format!("return {name}:GetChecked() and true or false"))
                .unwrap()
        };
        let enabled = |s: &mut UiScript, name: &str| {
            s.eval::<bool>(&format!("return {name}:IsEnabled() == 1"))
                .unwrap()
        };
        assert!(checked(s, "ReputationDetailAtWarCheckBox"));
        assert!(!enabled(s, "ReputationDetailAtWarCheckBox"));
        assert!(enabled(s, "ReputationDetailInactiveCheckBox"));
        click(s, "ReputationBar3", "LeftButton");
        assert!(!shown(s, "ReputationDetailFrame"), "clicked shut");

        // A header without a bar takes no click; one with a bar opens the popup, both boxes dead.
        click(s, "ReputationBar2", "LeftButton");
        assert!(!shown(s, "ReputationDetailFrame"));
        click(s, "ReputationBar9", "LeftButton");
        assert!(shown(s, "ReputationDetailFrame"));
        assert_eq!(
            text_of(s, "ReputationDetailFactionName"),
            "Alliance Vanguard"
        );
        assert!(!enabled(s, "ReputationDetailAtWarCheckBox"));
        assert!(!enabled(s, "ReputationDetailInactiveCheckBox"));
        assert_clean(s);
    });
}

/// The At War box has a `<NormalTexture>` and no `<DisabledTexture>`: disabled, it still draws its
/// box, under a grey tick at war.
#[test]
fn the_at_war_box_keeps_its_art_while_it_is_disabled() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut st = state();
    st[3].peace_forced = true; // Ironforge
    st[2].peace_forced = true; // Stormwind
    st[2].at_war = true;
    let (mut s, mut v) = shown_page("rep-box", st);
    v.lend(&mut s, |s| {
        fn box_art(s: &UiScript) -> Vec<String> {
            s.extract()
                .iter()
                .filter(|q| {
                    s.quad_owner_name(q.target).as_deref() == Some("ReputationDetailAtWarCheckBox")
                })
                .filter_map(|q| match &q.content {
                    QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
                    _ => None,
                })
                .collect()
        }
        let up = r"Interface\Buttons\UI-CheckBox-Up".to_string();

        click(s, "ReputationBar3", "LeftButton"); // Ironforge
        assert_eq!(box_art(s), vec![up.clone()]);
        click(s, "ReputationBar4", "LeftButton"); // Stormwind
        assert_eq!(
            box_art(s),
            vec![
                up.clone(),
                r"Interface\Buttons\UI-CheckBox-Check-Disabled".to_string()
            ]
        );
        click(s, "ReputationBar6", "LeftButton"); // Booty Bay, the control
        assert_eq!(box_art(s), vec![up]);
        assert_clean(s);
    });
}

/// Below the Wrath cap the watch bar stacks on the XP bar in its own end art; at 80 it replaces
/// it (`ReputationFrame.lua:323-419`).
#[test]
fn the_watch_bar_shows_the_watched_factions_progress_and_swaps_at_max_level() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, mut v) = loaded("rep-watch", state());
    v.lend(&mut s, |s| s.resolve());
    assert!(!shown(&mut s, "ReputationWatchBar"));

    // The server sets the field; its watch raises `UPDATE_FACTION` (`0x6cdde1`).
    v.table.watched = Some(20); // Ironforge
    v.lend(&mut s, |s| {
        s.fire_event("UPDATE_FACTION", vec![ScriptValue::Str("player".into())]);
        s.resolve();
    });
    assert!(shown(&mut s, "ReputationWatchBar"));
    assert_eq!(
        text_of(&mut s, "ReputationWatchStatusBarText"),
        "Ironforge 1000 / 6000"
    );
    assert_eq!(
        s.eval::<(f32, f32, f32)>("return ReputationWatchStatusBar:GetStatusBarColor()")
            .unwrap(),
        (0.0, 0.6, 0.1)
    );
    assert!(shown(&mut s, "ReputationWatchBarTexture0"));
    assert!(!shown(&mut s, "ReputationXPBarTexture0"));
    assert_eq!(
        s.eval::<f64>("return ReputationWatchStatusBar:GetHeight()")
            .unwrap(),
        8.0
    );

    s.set_unit("player", Some(player(80)));
    v.lend(&mut s, |s| {
        s.fire_event("PLAYER_LEVEL_UP", vec![ScriptValue::Int(80)]);
        s.resolve();
    });
    assert!(shown(&mut s, "ReputationWatchBar"));
    assert!(
        !shown(&mut s, "MainMenuExpBar"),
        "at the cap it replaces the XP bar"
    );
    assert!(shown(&mut s, "ReputationXPBarTexture0"));
    assert_eq!(
        s.eval::<f64>("return ReputationWatchStatusBar:GetHeight()")
            .unwrap(),
        13.0
    );
    assert_clean(&mut s);
}

/// The "Show as Experience Bar" box sends the watch inside the call (`0x5d0ba0`), but the bar
/// waits for the server's field.
#[test]
fn the_show_as_experience_bar_box_sends_the_watch_and_the_server_brings_the_bar_up() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, mut v) = shown_page("rep-watchbox", state());
    v.table.sends.clear();

    v.lend(&mut s, |s| {
        click(s, "ReputationBar3", "LeftButton"); // Ironforge, slot 20
        assert!(shown(s, "ReputationDetailFrame"));
        let (x, y) = centre_of(s, "ReputationDetailMainScreenCheckBox");
        s.mouse_move(x, y);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_button(x, y, "LeftButton", false);
    });
    assert_eq!(
        std::mem::take(&mut v.table.sends),
        [ReputationSend::Watch(20)]
    );
    assert!(!shown(&mut s, "ReputationWatchBar"), "not optimistic");

    v.table.watched = Some(20);
    v.lend(&mut s, |s| {
        s.fire_event("UPDATE_FACTION", vec![ScriptValue::Str("player".into())]);
        s.resolve();
    });
    assert!(v.lend(&mut s, |s| s
        .eval::<bool>("return ReputationDetailMainScreenCheckBox:GetChecked() and true or false")
        .unwrap()));
    assert!(shown(&mut s, "ReputationWatchBar"));

    v.lend(&mut s, |s| {
        click(s, "ReputationDetailMainScreenCheckBox", "LeftButton")
    });
    assert_eq!(
        std::mem::take(&mut v.table.sends),
        [ReputationSend::Watch(-1)]
    );
    assert_clean(&mut s);
}
