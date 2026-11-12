//! The stock interface loaded as the Mac build of the client, on whatever OS the tests run: its Mac
//! arms (`IsMacClient()` true) and the `platform="mac"` binding rows.

use benilla_ui::script::{ClientOs, UiScript};

/// The whole production interface in a VM answering as the Mac build, with the Mac build's CVars.
fn mac_ui() -> UiScript {
    let mut s = UiScript::new_on(ClientOs::Mac).unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.register_cvars(
        crate::cvars::REGISTERED_MAC
            .iter()
            .map(|r| (r.name, r.default)),
    );
    super::test_ui::load_stock_ui(&mut s);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// The Mac arms load without a raise: the game menu grows its Mac Options button, and a key with
/// a Mac name is labelled by it.
#[test]
fn the_stock_interface_loads_as_the_mac_client() {
    benilla_formats::wow_data_or_skip!();
    let s = mac_ui();
    assert!(s
        .eval::<bool>(
            "return GameMenuButtonMacOptions:IsShown() == 1 \
             and math.abs(GameMenuFrame:GetHeight() - 266) < 0.01 \
             and select(2, GameMenuButtonUIOptions:GetPoint()) == GameMenuButtonMacOptions"
        )
        .unwrap());
    for key in ["PRINTSCREEN", "BACKSPACE", "ENTER", "NUMLOCK"] {
        assert!(
            s.eval::<bool>(&format!(
                "return GetBindingText('{key}', 'KEY_') == KEY_{key}_MAC \
                 and KEY_{key}_MAC ~= KEY_{key}"
            ))
            .unwrap(),
            "{key}"
        );
    }
    // A key with no Mac name keeps the shared one.
    assert!(s
        .eval::<bool>(
            "return KEY_ESCAPE_MAC == nil and GetBindingText('ESCAPE', 'KEY_') == KEY_ESCAPE"
        )
        .unwrap());
}

/// The PC build takes the other arm of the same files.
#[test]
fn the_pc_build_keeps_the_pc_key_names() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new_on(ClientOs::Pc).unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_stock_ui(&mut s);
    assert!(s
        .eval::<bool>(
            "return GetBindingText('PRINTSCREEN', 'KEY_') == KEY_PRINTSCREEN \
             and not GameMenuButtonMacOptions:IsShown()"
        )
        .unwrap());
    let names: Vec<String> = s.keybind_snapshot().into_iter().map(|(n, _)| n).collect();
    assert!(!names.iter().any(|n| n.starts_with("ITUNES_")), "{names:?}");
}

/// The nine `platform="mac"` rows are commands under their two headers, and each body runs.
#[test]
fn the_mac_binding_rows_register_and_run() {
    benilla_formats::wow_data_or_skip!();
    let s = mac_ui();
    let names: Vec<String> = s.keybind_snapshot().into_iter().map(|(n, _)| n).collect();
    let mac_rows = [
        "ITUNES_PLAYPAUSE",
        "ITUNES_NEXTTRACK",
        "ITUNES_BACKTRACK",
        "ITUNES_VOLUMEUP",
        "ITUNES_VOLUMEDOWN",
        "MOVIE_RECORDING_STARTSTOP",
        "MOVIE_RECORDING_CANCEL",
        "MOVIE_RECORDING_COMPRESS",
        "MOVIE_RECORDING_GUI",
    ];
    s.run(
        "local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE')          f:SetScript('OnEvent', function(_, _, text) mac_warning = text end)",
    )
    .unwrap();
    for row in mac_rows {
        assert!(names.iter().any(|n| n == row), "{row}: {names:?}");
        assert!(s.execute_binding(row, true).unwrap(), "{row} ran nothing");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The recording key showed the requirements warning; the GUI key flipped its CVar.
    assert!(s
        .eval::<bool>("return mac_warning == MOVIE_RECORDING_WARNING_REQUIREMENTS")
        .unwrap());
    assert_eq!(s.cvar("MovieRecordingGUI").as_deref(), Some("0"));
}

/// The Mac Options window opens with the recorder's controls greyed and the music player's two
/// live, and its Defaults and Okay run to the end.
#[test]
fn the_mac_options_window_opens_and_saves() {
    benilla_formats::wow_data_or_skip!();
    let mut s = mac_ui();
    s.run("ShowUIPanel(MacOptionsFrame)").unwrap();
    s.resolve();
    assert!(s.errors().is_empty(), "on show: {:?}", s.errors());
    assert!(s
        .eval::<bool>(
            "for i = 1, 6 do \
                 if _G['MacOptionsFrameCheckButton' .. i]:IsEnabled() == 1 then return false end \
             end \
             return MacOptionsFrame:IsShown() == 1 \
                 and MacOptionsFrameCheckButton7:IsEnabled() == 1 \
                 and MacOptionsFrameCheckButton8:IsEnabled() == 1 \
                 and MacOptionsFrameCheckButton7:GetChecked() == 1 \
                 and MacOptionsFrameCheckButton8:GetChecked() == nil \
                 and MacOptionsButtonCompress:IsEnabled() == 0 \
                 and UIDropDownMenu_GetSelectedValue(MacOptionsFrameResolutionDropDown) == '640x480' \
                 and UIDropDownMenu_GetSelectedValue(MacOptionsFrameFramerateDropDown) == '29.97' \
                 and MacOptionsFrameText2:GetText() == '0:00:00' \
                 and MacOptionsFrameText4:GetText() == '13.170 MB/s'"
        )
        .unwrap());
    s.run(
        "MacOptionsFrameCheckButton8:SetChecked(1) \
         MacOptionsFrameDefaults:Click() \
         MacOptionsFrameCheckButton8:SetChecked(1) \
         MacOptionsFrameOkay:Click()",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "defaults and okay: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return not MacOptionsFrame:IsShown()")
        .unwrap());
    assert_eq!(s.cvar("iTunesTrackDisplay").as_deref(), Some("1"));
    assert_eq!(s.cvar("MovieRecordingWidth").as_deref(), Some("640"));
}
