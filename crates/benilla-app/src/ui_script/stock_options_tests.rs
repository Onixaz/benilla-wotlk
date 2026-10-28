//! The stock 3.3.5a settings windows off the player's chain (`VideoOptionsFrame`,
//! `AudioOptionsFrame`, `InterfaceOptionsFrame`) on the production load: every panel opens,
//! applies, cancels and resets with no Lua error.

use benilla_ui::script::{ScreenResolution, ScriptValue, UiScript, VideoCaps};

use crate::local_state::test_env::ENV_LOCK;

/// The display facts the Video panels' `OnLoad`s read, as the host pushes them.
fn seed_display(s: &mut UiScript) {
    s.set_screen_resolutions(
        vec![
            ScreenResolution {
                width: 1280,
                height: 720,
            },
            ScreenResolution {
                width: 1920,
                height: 1080,
            },
        ],
        Some(ScreenResolution {
            width: 1600,
            height: 900,
        }),
    );
    s.set_video_caps(VideoCaps {
        anisotropic: true,
        pixel_shaders: true,
        vertex_shaders: true,
        trilinear: true,
        triple_buffering: false,
        max_anisotropy: 16,
        hardware_cursor: true,
    });
}

/// Opens every panel of each window in turn, then Okay, Defaults and Cancel.
const DRIVE_EVERY_PANEL: &str = r#"
local function each_panel(frame, open)
    for _, panel in ipairs(frame.categoryList) do
        open(panel)
    end
end
for _, frame in ipairs({ VideoOptionsFrame, AudioOptionsFrame }) do
    ShowUIPanel(frame)
    each_panel(frame, function(panel) OptionsFrame_OpenToCategory(frame, panel) end)
    _G[frame:GetName() .. "Okay"]:Click()
    ShowUIPanel(frame)
    OptionsFrame_SetAllToDefaults(frame)
    ShowUIPanel(frame)
    _G[frame:GetName() .. "Cancel"]:Click()
end
ShowUIPanel(InterfaceOptionsFrame)
for _, panel in ipairs({ InterfaceOptionsFramePanelContainer:GetChildren() }) do
    if panel.name then InterfaceOptionsFrame_OpenToCategory(panel) end
end
InterfaceOptionsFrameOkay:Click()
ShowUIPanel(InterfaceOptionsFrame)
InterfaceOptionsFrame_SetAllToDefaults()
ShowUIPanel(InterfaceOptionsFrame)
InterfaceOptionsFrameCancel:Click()
"#;

/// Every error the load and the drive raise from the stock options files.
fn options_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("Options") || e.contains("BindingUI"))
        .collect()
}

/// The production load with the display facts seeded, its load errors drained.
fn loaded(tag: &str) -> (UiScript, Vec<String>) {
    let (mut s, failures) =
        super::layer_tests::production_load_seeded(tag, false, "", |_| {}, seed_display);
    let mut errors = options_errors(failures);
    errors.extend(options_errors(s.take_errors()));
    (s, errors)
}

/// The stock stack loads, and every panel of all three windows opens, applies, resets and cancels,
/// with no error from an options file.
#[test]
fn every_stock_options_panel_opens_applies_and_cancels_cleanly() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, load_errors) = loaded("opt-drive");
    assert!(load_errors.is_empty(), "load errors: {load_errors:#?}");
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    s.run(DRIVE_EVERY_PANEL).unwrap();
    let errors = options_errors(s.take_errors());
    assert!(errors.is_empty(), "drive errors: {errors:#?}");
    let panels: (i64, i64) = s
        .eval("return #VideoOptionsFrame.categoryList, #AudioOptionsFrame.categoryList")
        .unwrap();
    assert!(
        panels.0 >= 2 && panels.1 >= 1,
        "panels registered: {panels:?}"
    );
}

/// The menu's rungs are the stock file's own: Video, Sound & Voice and Interface open their stock
/// windows, Key Bindings loads `Blizzard_BindingUI` and opens its window, Macros the macro window.
#[test]
fn the_game_menu_rungs_open_the_stock_windows() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = loaded("opt-menu");
    for (rung, window) in [
        ("GameMenuButtonOptions", "VideoOptionsFrame"),
        ("GameMenuButtonSoundOptions", "AudioOptionsFrame"),
        ("GameMenuButtonUIOptions", "InterfaceOptionsFrame"),
        ("GameMenuButtonKeybindings", "KeyBindingFrame"),
        ("GameMenuButtonMacros", "MacroFrame"),
    ] {
        s.run("CloseAllWindows() ShowUIPanel(GameMenuFrame)")
            .unwrap();
        assert!(
            s.eval::<bool>(&format!("return {rung}:IsVisible() == 1"))
                .unwrap(),
            "{rung} shows on the menu"
        );
        s.run(&format!("{rung}:Click()")).unwrap();
        assert!(
            s.eval::<bool>(&format!(
                "return {window} ~= nil and {window}:IsShown() == 1"
            ))
            .unwrap(),
            "{rung} opens {window}"
        );
        s.run(&format!("HideUIPanel({window})")).unwrap();
    }
    let errors: Vec<String> = s
        .take_errors()
        .into_iter()
        .filter(|e| {
            ["Options", "Binding", "Macro", "GameMenu"]
                .iter()
                .any(|k| e.contains(k))
        })
        .collect();
    assert!(errors.is_empty(), "rung errors: {errors:#?}");
}

/// Logout and Exit Game are the stock rungs' `Logout()` and `Quit()`.
#[test]
fn the_logout_and_exit_rungs_queue_their_session_requests() {
    use benilla_ui::script::SessionRequest;
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = loaded("opt-exit");
    let _ = s.take_session_requests();
    s.run("ShowUIPanel(GameMenuFrame) GameMenuButtonLogout:Click()")
        .unwrap();
    assert_eq!(s.take_session_requests(), vec![SessionRequest::Logout]);
    assert!(!s
        .eval::<bool>("return GameMenuFrame:IsShown() == 1")
        .unwrap());
    s.run("ShowUIPanel(GameMenuFrame) GameMenuButtonQuit:Click()")
        .unwrap();
    assert_eq!(s.take_session_requests(), vec![SessionRequest::Quit]);
}

/// A panel's Okay writes its CVar through `SetCVar`: Auto Loot on the Controls panel.
#[test]
fn an_interface_panel_okay_writes_its_cvar() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = loaded("opt-okay");
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    let _ = s.take_cvar_changes();
    assert_eq!(s.cvar("autoLootDefault").as_deref(), Some("0"));
    s.run(
        "InterfaceOptionsFrame_OpenToCategory(InterfaceOptionsControlsPanel) \
         InterfaceOptionsControlsPanelAutoLootCorpse:Click() \
         InterfaceOptionsFrameOkay:Click()",
    )
    .unwrap();
    assert_eq!(s.cvar("autoLootDefault").as_deref(), Some("1"));
    assert!(s
        .take_cvar_changes()
        .contains(&("autoLootDefault".to_string(), "1".to_string())));
    assert!(!s
        .eval::<bool>("return InterfaceOptionsFrame:IsShown() == 1")
        .unwrap());
}

/// The Controls panel's checkboxes, `(button suffix, CVar, registered default)`
/// (`InterfaceOptionsPanels.xml`, `CGGameUI::RegisterGameCVars 0x51d9b0`).
const CONTROLS_CHECKBOXES: [(&str, &str, &str); 6] = [
    ("StickyTargeting", "deselectOnClick", "1"),
    ("AutoDismount", "autoDismountFlying", "0"),
    ("AutoClearAFK", "autoClearAFK", "1"),
    ("BlockTrades", "blockTrades", "0"),
    ("LootAtMouse", "lootUnderMouse", "1"),
    ("AutoLootCorpse", "autoLootDefault", "0"),
];

/// Each Controls checkbox boots at its CVar's 12340 default (Sticky Targeting shown inverted), a
/// click and Okay writes the flip, a click and Cancel writes nothing, and Defaults puts the
/// registered value back.
#[test]
fn every_controls_checkbox_shows_writes_cancels_and_defaults_its_cvar() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = loaded("opt-controls");
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    let flip = |v: &str| if v == "1" { "0" } else { "1" };
    for (suffix, cvar, default) in CONTROLS_CHECKBOXES {
        let button = format!("InterfaceOptionsControlsPanel{suffix}");
        assert_eq!(s.cvar(cvar).as_deref(), Some(default), "{cvar} boots");
        s.run("InterfaceOptionsFrame_OpenToCategory(InterfaceOptionsControlsPanel)")
            .unwrap();
        let checked = s
            .eval::<bool>(&format!("return {button}:GetChecked() == 1"))
            .unwrap();
        let shown = if suffix == "StickyTargeting" {
            default == "0"
        } else {
            default == "1"
        };
        assert_eq!(checked, shown, "{button} shows {cvar}={default}");
        s.run(&format!(
            "{button}:Click() InterfaceOptionsFrameOkay:Click()"
        ))
        .unwrap();
        assert_eq!(
            s.cvar(cvar).as_deref(),
            Some(flip(default)),
            "Okay writes {cvar}"
        );
        s.run(&format!(
            "InterfaceOptionsFrame_OpenToCategory(InterfaceOptionsControlsPanel) \
             {button}:Click() InterfaceOptionsFrameCancel:Click()"
        ))
        .unwrap();
        assert_eq!(
            s.cvar(cvar).as_deref(),
            Some(flip(default)),
            "Cancel keeps {cvar}"
        );
        s.run(
            "InterfaceOptionsFrame_OpenToCategory(InterfaceOptionsControlsPanel) \
             InterfaceOptionsFrame_SetAllToDefaults() InterfaceOptionsFrameOkay:Click()",
        )
        .unwrap();
        assert_eq!(
            s.cvar(cvar).as_deref(),
            Some(default),
            "Defaults resets {cvar}"
        );
    }
    let errors = options_errors(s.take_errors());
    assert!(errors.is_empty(), "errors: {errors:#?}");
}

/// The Auto Loot Key dropdown reads and writes the `AUTOLOOTTOGGLE` modified click
/// (`InterfaceOptionsControlsPanelAutoLootKeyDropDown_OnEvent`), which the loot interact inverts
/// `autoLootDefault` by (`IsAutoLooting 0x513700`); its label follows the Auto Loot checkbox.
#[test]
fn the_auto_loot_key_dropdown_rebinds_the_modified_click() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = loaded("opt-lootkey");
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    let initial: String = s
        .eval("return InterfaceOptionsControlsPanelAutoLootKeyDropDown.value")
        .unwrap();
    assert_eq!(
        initial,
        s.eval::<String>("return GetModifiedClick('AUTOLOOTTOGGLE')")
            .unwrap(),
        "the dropdown shows the live binding"
    );
    for key in ["CTRL", "ALT", "NONE", "SHIFT"] {
        s.run(&format!(
            "InterfaceOptionsControlsPanelAutoLootKeyDropDown:SetValue('{key}')"
        ))
        .unwrap();
        assert_eq!(
            s.eval::<String>("return GetModifiedClick('AUTOLOOTTOGGLE')")
                .unwrap(),
            key
        );
    }
    let label = |s: &mut UiScript| {
        s.eval::<String>("return InterfaceOptionsControlsPanelAutoLootKeyDropDownLabel:GetText()")
            .unwrap()
    };
    let auto_loot_key = s.eval::<String>("return AUTO_LOOT_KEY_TEXT").unwrap();
    let loot_key = s.eval::<String>("return LOOT_KEY_TEXT").unwrap();
    assert_eq!(label(&mut s), auto_loot_key, "auto loot off");
    s.run(
        "InterfaceOptionsFrame_OpenToCategory(InterfaceOptionsControlsPanel) \
         InterfaceOptionsControlsPanelAutoLootCorpse:Click()",
    )
    .unwrap();
    assert_eq!(label(&mut s), loot_key, "auto loot on");
    let errors = options_errors(s.take_errors());
    assert!(errors.is_empty(), "errors: {errors:#?}");
}

/// `lootUnderMouse` 1 opens the loot window at the cursor (`LootFrame_Show`: 175 left, 25 up for
/// an empty loot, the top clamped to 350); 0 leaves it at its UI panel spot.
#[test]
fn loot_under_mouse_opens_the_loot_window_at_the_cursor() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = loaded("opt-lootmouse");
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    let panel_spot = |s: &mut UiScript| {
        s.run("HideUIPanel(LootFrame) LootFrame.page = 1 LootFrame_Show(LootFrame)")
            .unwrap();
        s.eval::<(f64, f64)>("return LootFrame:GetLeft(), LootFrame:GetTop()")
            .unwrap()
    };
    s.run("SetCVar('lootUnderMouse', '0')").unwrap();
    let docked = panel_spot(&mut s);
    s.mouse_move(600.0, 500.0);
    s.run("SetCVar('lootUnderMouse', '1')").unwrap();
    let scale: f64 = s.eval("return LootFrame:GetEffectiveScale()").unwrap();
    let at = panel_spot(&mut s);
    assert!((at.0 - (600.0 / scale - 175.0)).abs() < 0.5, "left {at:?}");
    assert!((at.1 - (500.0 / scale + 25.0)).abs() < 0.5, "top {at:?}");
    s.mouse_move(600.0, 100.0);
    let low = panel_spot(&mut s);
    assert!(
        (low.1 - 350.0).abs() < 0.5,
        "the top clamps at 350: {low:?}"
    );
    s.run("SetCVar('lootUnderMouse', '0')").unwrap();
    let back = panel_spot(&mut s);
    assert_eq!(back, docked, "off, the window keeps its panel spot");
    let errors: Vec<String> = s
        .take_errors()
        .into_iter()
        .filter(|e| e.contains("Loot"))
        .collect();
    assert!(errors.is_empty(), "errors: {errors:#?}");
}

/// The Sound panel's device dropdown lists "System Default" and the engine's devices; a pick writes
/// `Sound_OutputDriverIndex` and restarts the sound system at once, and Cancel puts the old index
/// back with another restart (`AudioOptionsPanels.lua`, `AudioOptionsFrame_AudioRestart`).
#[test]
fn the_sound_panels_device_pick_writes_the_index_and_restarts() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _) = loaded("opt-device");
    s.set_sound_output_drivers(Some(vec!["Speakers".into(), "Headphones".into()]));
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    let _ = s.take_cvar_changes();
    assert!(!s.take_sound_restart());
    s.run(
        "AudioOptionsFrame_Toggle() \
         ToggleDropDownMenu(1, nil, AudioOptionsSoundPanelHardwareDropDown)",
    )
    .unwrap();
    let rows: (String, String, String) = s
        .eval(
            "return DropDownList1Button1:GetText(), DropDownList1Button2:GetText(), \
             DropDownList1Button3:GetText()",
        )
        .unwrap();
    assert_eq!(
        rows,
        (
            "System Default".to_string(),
            "Speakers".to_string(),
            "Headphones".to_string()
        )
    );
    s.run("DropDownList1Button3:Click()").unwrap();
    assert_eq!(s.cvar("Sound_OutputDriverIndex").as_deref(), Some("2"));
    assert!(s.take_sound_restart(), "a device pick restarts at once");
    assert_eq!(
        s.eval::<String>("return AudioOptionsSoundPanelHardwareDropDownText:GetText()")
            .unwrap(),
        "Headphones"
    );
    s.run("AudioOptionsFrameCancel:Click()").unwrap();
    assert_eq!(s.cvar("Sound_OutputDriverIndex").as_deref(), Some("0"));
    assert!(s.take_sound_restart(), "Cancel restarts on the old device");
    let errors = options_errors(s.take_errors());
    assert!(errors.is_empty(), "errors: {errors:#?}");
}

/// ESC on a window opened from the menu cancels it (`UIParent.lua`'s `ToggleGameMenu`), and its
/// `OnHide` hands the screen back to the menu it came from (`OptionsFrame_OnHide`'s `lastFrame`);
/// the next press closes the menu.
#[test]
fn escape_on_an_options_window_returns_to_the_menu() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, _) = loaded("opt-esc");
    s.run("ToggleGameMenu() GameMenuButtonOptions:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return VideoOptionsFrame:IsShown() == 1")
        .unwrap());
    s.run("ToggleGameMenu()").unwrap();
    assert!(!s
        .eval::<bool>("return VideoOptionsFrame:IsShown() == 1")
        .unwrap());
    assert!(
        s.eval::<bool>("return GameMenuFrame:IsShown() == 1")
            .unwrap(),
        "the menu is back"
    );
    s.run("ToggleGameMenu()").unwrap();
    assert!(!s
        .eval::<bool>("return GameMenuFrame:IsShown() == 1")
        .unwrap());
}
