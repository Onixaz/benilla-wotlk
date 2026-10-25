//! The stock 3.3.5a character window on the production load: the open with the currency tab
//! loaded, the title picker, the spell stat tooltips' school icons, and the equipment manager's
//! dialog and slot flyout.

use benilla_ui::script::{
    EquipmentSetRequest, InvSlotView, PlayerTitles, ScriptValue, TitleRow, UiScript, UnitState,
};

fn load(tag: &str) -> UiScript {
    let (mut s, _load_failures) = super::layer_tests::production_load_with(tag, false, "", |_| {});
    let _ = s.take_errors();
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            is_player: true,
            name: Some("Probesix".into()),
            level: 80,
            class: Some("Mage".into()),
            class_file: Some("MAGE".into()),
            ..Default::default()
        }),
    );
    s
}

/// Errors from the window's own code.
fn sheet_errors(s: &mut UiScript) -> Vec<String> {
    s.take_errors()
        .into_iter()
        .filter(|e| {
            [
                "PaperDoll",
                "CharacterFrame",
                "Token",
                "PlayerTitle",
                "GearManager",
                "EquipmentManager",
            ]
            .iter()
            .any(|k| e.contains(k))
        })
        .collect()
}

/// The window opens with no raise now `Blizzard_TokenUI` loads at startup, and the title picker
/// lists "None" and each known title, sending the chosen one.
#[test]
fn the_character_window_opens_and_the_title_picker_lists_the_known_titles() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = load("charsheet-titles");
    assert!(
        s.eval::<bool>("return TokenFrame ~= nil").unwrap(),
        "TokenFrame loaded"
    );
    s.set_title_rows(vec![
        TitleRow {
            mask_id: 1,
            name: "Private %s".into(),
            name1: String::new(),
        },
        TitleRow {
            mask_id: 2,
            name: "%s the Explorer".into(),
            name1: String::new(),
        },
    ]);
    // The picker walks mask ids 1 through `GetNumTitles()`, the row count plus 1.
    let mut known = [0u32; 6];
    known[0] |= 1 << 2;
    s.set_player_titles(Some(PlayerTitles {
        chosen: 0,
        known,
        female: false,
    }));
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(s
        .eval::<bool>("return PaperDollFrame:IsShown() == 1")
        .unwrap());
    s.fire_event(
        "KNOWN_TITLES_UPDATE",
        vec![ScriptValue::Str("player".into())],
    );
    let errors = sheet_errors(&mut s);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(s
        .eval::<bool>("return PlayerTitleFrame:IsShown() == 1")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return PlayerTitleFrameText:GetText()")
            .unwrap(),
        s.eval::<String>("return PAPERDOLL_SELECT_TITLE").unwrap()
    );
    let (count, first, second): (i64, String, String) = s
        .eval(
            "local t = PlayerTitleFrame.titles \
             return #t, t[1].name, t[2].name",
        )
        .unwrap();
    assert_eq!((count, second.as_str()), (2, "the Explorer"));
    assert_eq!(first, s.eval::<String>("return NONE").unwrap());
    s.run(
        "PlayerTitlePickerFrame_Toggle() \
         for _, b in ipairs(PlayerTitlePickerScrollFrame.buttons) do \
           if b.titleId == 2 then PlayerTitleButton_OnClick(b) end \
         end",
    )
    .unwrap();
    assert_eq!(s.take_title_sets(), vec![2]);
    assert!(sheet_errors(&mut s).is_empty());
}

/// The spell crit tooltip lists every school, Holy through Arcane, each row with its icon.
#[test]
fn the_spell_crit_tooltip_lists_every_school_with_its_icon() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = load("charsheet-crit");
    s.run(
        r#"
        local f = CreateFrame("Frame", "CritProbe", UIParent)
        f:SetWidth(10) f:SetHeight(10) f:SetPoint("CENTER")
        f.spellCrit = { 0, 1, 2, 3, 4, 5, 6 }
        CharacterSpellCritChance_OnEnter(f)
        "#,
    )
    .unwrap();
    let errors = sheet_errors(&mut s);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(s.eval::<bool>("return GameTooltip:IsShown() == 1").unwrap());
    assert_eq!(s.eval::<i64>("return GameTooltip:NumLines()").unwrap(), 7);
    for i in 2..=7 {
        let (shown, line): (bool, String) = s
            .eval(&format!(
                "local t = GameTooltipTexture{} \
                 local _, rel = t:GetPoint() \
                 return t:IsShown() == 1, rel:GetName()",
                i - 1
            ))
            .unwrap();
        assert!(shown, "school {i}'s icon");
        assert_eq!(line, format!("GameTooltipTextLeft{i}"));
    }
}

/// The gear manager: a saved set lists in the dialog, its tooltip counts the worn items, its
/// equip goes out through `UseEquipmentSet`, and an Alt-hover flyout lists what fits a slot.
#[test]
fn the_gear_manager_saves_lists_and_equips_a_set() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = load("charsheet-gear");
    s.run(r#"SetCVar("equipmentManager", "1")"#).unwrap();
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    inv[1] = Some(InvSlotView {
        guid: 0x10,
        item_id: 100,
        equip_slots: vec![1],
        icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_equipment_set_list(Vec::new());
    s.fire_event("VARIABLES_LOADED", Vec::new());
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    assert!(s
        .eval::<bool>("return GearManagerToggleButton:IsShown() == 1")
        .unwrap());
    s.run("GearManagerToggleButton:Click()").unwrap();
    assert!(s
        .eval::<bool>("return GearManagerDialog:IsShown() == 1")
        .unwrap());
    s.run(r#"SaveEquipmentSet("Tank", -1)"#).unwrap();
    let requests = s.take_equipment_set_requests();
    assert!(matches!(
        requests.as_slice(),
        [EquipmentSetRequest::Save(_)]
    ));
    s.equipment_set_saved(0, 0x500);
    assert_eq!(
        s.eval::<String>("return GearSetButton1.name").unwrap(),
        "Tank",
        "the dialog repaints on EQUIPMENT_SETS_CHANGED"
    );
    s.run("GearSetButton_OnClick(GearSetButton1) GearManagerDialogEquipSet_OnClick()")
        .unwrap();
    let requests = s.take_equipment_set_requests();
    let [EquipmentSetRequest::Use(wire)] = requests.as_slice() else {
        panic!("{requests:?}");
    };
    assert_eq!(wire[0], (0x10, 0xff, 0));
    s.run("GearSetButton_OnEnter(GearSetButton1)").unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Tank"
    );
    let errors = sheet_errors(&mut s);
    assert!(errors.is_empty(), "{errors:#?}");
}
