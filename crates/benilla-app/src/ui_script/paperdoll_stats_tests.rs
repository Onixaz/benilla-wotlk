//! The stock 3.3.5a character sheet's two stat panes (`PaperDollFrame.lua`'s
//! `UpdatePaperdollStats`) on the production load: the dropdown CVars pick the class default at
//! `VARIABLES_LOADED`, and every pane fills each row from the combat-stats snapshot.

use benilla_ui::script::{ScriptValue, UiScript, UnitCombatStats, UnitState};

/// Errors from the sheet's own code; the production load carries unrelated 3.3.5a gaps, and the
/// title picker's (`PlayerTitleFrame_*`, #160) runs after the panes fill.
fn sheet_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("PaperDoll") || e.contains("UIDropDownMenu"))
        .filter(|e| !e.contains("PlayerTitleFrame"))
        .collect()
}

fn mage() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Probesix".into()),
        level: 80,
        power_type: 0,
        class: Some("Mage".into()),
        class_file: Some("MAGE".into()),
        is_player: true,
        ..Default::default()
    }
}

/// A snapshot with every figure the five panes read set nonzero.
fn sheet_stats() -> UnitCombatStats {
    let mut s = UnitCombatStats {
        stats: [40, 50, 300, 600, 400],
        resistances: [900, 0, 10, 0, 0, 0, 0],
        min_damage: 100.0,
        max_damage: 150.0,
        main_attack_time_ms: 2900,
        attack_power: 120,
        ranged_attack_power: 30,
        ranged_attack_time_ms: 1500,
        ranged_min_damage: 200.0,
        ranged_max_damage: 300.0,
        has_wand: true,
        defense_skill: (400, 0),
        main_weapon_skill: (400, 0),
        dodge_percent: 5.0,
        parry_percent: 0.0,
        block_percent: 0.0,
        crit_percent: 6.0,
        ranged_crit_percent: 4.0,
        spell_healing_bonus: 1500,
        shield_block: 0,
        expertise: [3, 0],
        mana_regen: Some([40.0, 25.0]),
        damage_bonus_stat: Some(0),
        crit_from_agility: 2.5,
        spell_crit_from_intellect: 9.0,
        health_regen_from_spirit: 20.0,
        mana_regen_from_spirit: 30.0,
        ..Default::default()
    };
    s.spell_crit_percent = [0.0, 30.0, 31.0, 31.0, 32.0, 31.0, 31.0];
    s.spell_damage_bonus = [0, 1500, 1600, 1600, 1500, 1500, 1500];
    s.combat_ratings[5] = 20;
    s.combat_ratings[7] = 120;
    s.combat_rating_bonus[7] = 4.5;
    s
}

/// Each row's label and value, `nil` where the pane leaves one empty.
fn pane_rows(s: &mut UiScript, prefix: &str) -> Vec<(Option<String>, Option<String>)> {
    (1..=6)
        .map(|i| {
            let row = format!("{prefix}{i}");
            s.eval::<(Option<String>, Option<String>)>(&format!(
                "if not {row}:IsShown() then return 'hidden', 'hidden' end \
                 return {row}Label:GetText(), {row}StatText:GetText()"
            ))
            .unwrap()
        })
        .collect()
}

/// A mage's sheet: the panes default to Base Stats and Spell, both dropdowns carry those labels,
/// and every row of every pane fills with no error from the sheet's code.
#[test]
fn the_stock_stat_panes_fill_every_row_for_each_dropdown_choice() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("paperdoll-stats", false, "", |_| {});
    let _ = s.take_errors();
    s.set_unit("player", Some(mage()));
    s.set_player_combat_stats(Some(sheet_stats()));

    // A fresh character: both CVars at the registered "", so `VARIABLES_LOADED` writes the
    // class default.
    assert_eq!(
        s.eval::<Option<String>>("return GetCVar('playerStatLeftDropdown')")
            .unwrap()
            .as_deref(),
        Some("")
    );
    s.fire_event("VARIABLES_LOADED", Vec::<ScriptValue>::new());
    assert_eq!(
        s.eval::<String>("return GetCVar('playerStatLeftDropdown')")
            .unwrap(),
        "PLAYERSTAT_BASE_STATS"
    );
    assert_eq!(
        s.eval::<String>("return GetCVar('playerStatRightDropdown')")
            .unwrap(),
        "PLAYERSTAT_SPELL_COMBAT"
    );

    // `CharacterFrame_ShowSubFrame` raises on the unloaded `TokenFrame` (#281) after it has shown
    // the paper doll, so the open goes through `pcall`.
    s.run(r#"pcall(ToggleCharacter, "PaperDollFrame")"#).unwrap();
    assert!(s.eval::<bool>("return PaperDollFrame:IsShown()").unwrap());
    let errors = sheet_errors(s.take_errors());
    assert!(errors.is_empty(), "sheet errors on open: {errors:#?}");
    assert_eq!(
        s.eval::<Option<String>>("return PlayerStatFrameLeftDropDownText:GetText()")
            .unwrap(),
        s.eval::<Option<String>>("return PLAYERSTAT_BASE_STATS")
            .unwrap()
    );
    assert_eq!(
        s.eval::<Option<String>>("return PlayerStatFrameRightDropDownText:GetText()")
            .unwrap(),
        s.eval::<Option<String>>("return PLAYERSTAT_SPELL_COMBAT")
            .unwrap()
    );

    for choice in [
        "PLAYERSTAT_SPELL_COMBAT",
        "PLAYERSTAT_MELEE_COMBAT",
        "PLAYERSTAT_RANGED_COMBAT",
        "PLAYERSTAT_DEFENSES",
    ] {
        s.run(&format!(
            "SetCVar('playerStatRightDropdown', '{choice}') PaperDollFrame_UpdateStats()"
        ))
        .unwrap();
        let errors = sheet_errors(s.take_errors());
        assert!(errors.is_empty(), "{choice}: {errors:#?}");
        for (prefix, pane) in [
            ("PlayerStatFrameLeft", "PLAYERSTAT_BASE_STATS"),
            ("PlayerStatFrameRight", choice),
        ] {
            for (i, (label, value)) in pane_rows(&mut s, prefix).into_iter().enumerate() {
                assert!(
                    label.as_deref().is_some_and(|t| !t.is_empty())
                        && value.as_deref().is_some_and(|t| !t.is_empty()),
                    "{pane} row {}: label {label:?}, value {value:?}",
                    i + 1
                );
            }
        }
    }
}
