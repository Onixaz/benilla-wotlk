//! The stock Video window's Resolution and Effects pages off the player's chain, on the production
//! load: each control reads and writes its 3.3.5a row, Okay asks for the `RestartGx` the panels'
//! `restart` flags call for, the two Defaults verbs put the rows back, and benilla's layer leaves
//! no restart or logout prompt on the Effects page.

use benilla_ui::script::VideoCaps;
use benilla_ui::script::{MultisampleFormat, ScreenMode, ScreenResolution, ScriptValue, UiScript};

use crate::local_state::test_env::ENV_LOCK;

/// Two monitor sizes with their rates, the current one the larger, and two multisample formats.
fn seed_display(s: &mut UiScript) {
    let mode = |width, height, rates: &[u32]| ScreenMode {
        size: ScreenResolution { width, height },
        refresh_rates: rates.to_vec(),
    };
    s.set_screen_modes(
        vec![
            mode(1280, 720, &[60, 144]),
            mode(1920, 1080, &[60, 120, 144]),
        ],
        Some(ScreenResolution {
            width: 1920,
            height: 1080,
        }),
    );
    s.set_multisample_formats(vec![
        MultisampleFormat {
            color_bits: 24,
            depth_bits: 24,
            samples: 1,
        },
        MultisampleFormat {
            color_bits: 24,
            depth_bits: 24,
            samples: 4,
        },
    ]);
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

/// The production load with the display seeded and the world entered, the Video window open on
/// `panel`; asserts no options file raised.
fn opened(tag: &str, panel: &str) -> UiScript {
    let (mut s, failures) = super::layer_tests::production_load_seeded(
        tag,
        false,
        "",
        |_| {},
        |s| {
            s.register_cvars([("gxColorBits", "24"), ("gxDepthBits", "24")]);
            seed_display(s);
        },
    );
    let errors: Vec<String> = failures
        .into_iter()
        .chain(s.take_errors())
        .filter(|e| e.contains("Options"))
        .collect();
    assert!(errors.is_empty(), "load errors: {errors:#?}");
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::<ScriptValue>::new());
    s.run(&format!(
        "ShowUIPanel(VideoOptionsFrame) OptionsFrame_OpenToCategory(VideoOptionsFrame, {panel})"
    ))
    .unwrap();
    let _ = s.take_cvar_changes();
    let _ = s.take_restart_gx_asks();
    s
}

fn changed(s: &mut UiScript) -> Vec<(String, String)> {
    s.take_cvar_changes()
}

/// The dropdowns read the host's lists the stock way: the refresh menu offers the first size's
/// rates (it calls `GetRefreshRates()` with no index), and the current entry is the size in use.
#[test]
fn the_resolution_dropdowns_read_the_published_modes() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let s = opened("vid-read", "VideoOptionsResolutionPanel");
    let rates: String = s
        .eval("return table.concat({ GetRefreshRates() }, ',')")
        .unwrap();
    assert_eq!(rates, "60,144");
    assert_eq!(s.eval::<f64>("return GetCurrentResolution()").unwrap(), 2.0);
    assert_eq!(
        s.eval::<f64>(
            "return UIDropDownMenu_GetSelectedID(VideoOptionsResolutionPanelResolutionDropDown)"
        )
        .unwrap(),
        2.0
    );
}

/// Okay writes each changed Resolution row through its own verb and asks for one restart.
#[test]
fn okay_writes_the_resolution_rows_and_restarts() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = opened("vid-okay", "VideoOptionsResolutionPanel");
    s.run(
        "VideoOptionsResolutionPanelResolutionDropDown.newValue = 1 \
         VideoOptionsResolutionPanelRefreshDropDown.newValue = 144 \
         VideoOptionsResolutionPanelMultiSampleDropDown.newValue = 2 \
         VideoOptionsResolutionPanelVSync:Click() \
         VideoOptionsResolutionPanelWindowed:Click() \
         VideoOptionsFrameOkay:Click()",
    )
    .unwrap();
    let changes = changed(&mut s);
    for (name, value) in [
        ("gxResolution", "1280x720"),
        ("gxRefresh", "144"),
        ("gxMultisample", "4"),
        ("gxVSync", "0"),
        ("gxWindow", "1"),
    ] {
        assert!(
            changes.iter().any(|(n, v)| n == name && v == value),
            "{name} = {value} not written: {changes:?}"
        );
    }
    assert!(
        s.take_restart_gx_asks() >= 1,
        "the panel's restart flags ask for RestartGx"
    );
    let errors: Vec<String> = s
        .take_errors()
        .into_iter()
        .filter(|e| e.contains("Options"))
        .collect();
    assert!(errors.is_empty(), "drive errors: {errors:#?}");
}

/// Windowed Mode ticks and greys Use Desktop Gamma and greys the gamma slider, as
/// `VideoOptionsResolutionPanel_RefreshGammaControls` rules; the box writes `DesktopGamma`.
#[test]
fn windowed_mode_takes_the_desktop_gamma() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = opened("vid-gamma", "VideoOptionsResolutionPanel");
    s.run("VideoOptionsResolutionPanelWindowed:Click()")
        .unwrap();
    assert!(s
        .eval::<bool>("return VideoOptionsResolutionPanelDesktopGamma:IsEnabled() ~= 1")
        .unwrap());
    assert!(changed(&mut s)
        .iter()
        .any(|(n, v)| n == "DesktopGamma" && v == "1"));
}

/// No Effects control asks for a restart or a logout, and an Okay that moves the filtering raises
/// no prompt when the window closes.
#[test]
fn the_effects_page_asks_for_no_restart_or_logout() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = opened("vid-effects", "VideoOptionsEffectsPanel");
    let flagged: i64 = s
        .eval(
            "local n = 0 \
             for _, c in next, VideoOptionsEffectsPanel.controls do \
                 if c.gameRestart or c.logout or c.tooltipRequirement then n = n + 1 end \
             end \
             return n",
        )
        .unwrap();
    assert_eq!(flagged, 0);
    s.run(
        "VideoOptionsEffectsPanelTextureFiltering:SetValue(4) \
         VideoOptionsEffectsPanelPlayerTexture:SetValue(9) \
         VideoOptionsFrameOkay:Click()",
    )
    .unwrap();
    let changes = changed(&mut s);
    assert!(changes
        .iter()
        .any(|(n, v)| n == "textureFilteringMode" && v == "4"));
    assert!(changes
        .iter()
        .any(|(n, v)| n == "componentTextureLevel" && v == "9"));
    assert!(s
        .eval::<bool>(
            "return StaticPopup_Visible('CLIENT_RESTART_ALERT') == nil \
             and StaticPopup_Visible('CLIENT_LOGOUT_ALERT') == nil"
        )
        .unwrap());
}

/// The shadow slider spans the device's modes, which here are the blob mode alone.
#[test]
fn the_shadow_slider_caps_at_the_blob_mode() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let s = opened("vid-shadow", "VideoOptionsEffectsPanel");
    let (lo, hi): (f64, f64) = s
        .eval("return VideoOptionsEffectsPanelShadowQuality:GetMinMaxValues()")
        .unwrap();
    assert_eq!((lo, hi), (0.0, 0.0));
}

/// Each page's Defaults verb puts its own rows back, the Resolution page's with a restart.
#[test]
fn the_defaults_verbs_restore_their_own_page() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = opened("vid-defaults", "VideoOptionsEffectsPanel");
    s.run(
        "SetCVar('groundEffectDensity', '64') SetCVar('gxVSync', '0') \
         RestoreVideoEffectsDefaults()",
    )
    .unwrap();
    assert_eq!(s.cvar("groundEffectDensity").as_deref(), Some("24"));
    assert_eq!(
        s.cvar("gxVSync").as_deref(),
        Some("0"),
        "not the Effects page's row"
    );
    assert_eq!(s.take_restart_gx_asks(), 0);
    s.run("RestoreVideoResolutionDefaults()").unwrap();
    assert_eq!(s.cvar("gxVSync").as_deref(), Some("1"));
    assert_eq!(s.cvar("gxResolution").as_deref(), Some(""));
    assert_eq!(s.take_restart_gx_asks(), 1);
}

/// Hardware Cursor is live after world entry on a device that offers one, its text is not greyed,
/// and Okay writes the box to `gxCursor`.
#[test]
fn the_hardware_cursor_box_writes_gx_cursor() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = opened("vid-cursor", "VideoOptionsResolutionPanel");
    let state: String = s
        .eval(
            "local b = VideoOptionsResolutionPanelHardwareCursor \
             return tostring(b:IsEnabled()) .. ',' .. tostring(b:GetChecked()) .. ',' \
               .. tostring(b:IsVisible())",
        )
        .unwrap();
    assert_eq!(state, "1,1,1", "enabled, checked, shown");
    s.run("VideoOptionsResolutionPanelHardwareCursor:Click() VideoOptionsFrameOkay:Click()")
        .unwrap();
    let changes = changed(&mut s);
    assert!(
        changes.iter().any(|(n, v)| n == "gxCursor" && v == "0"),
        "gxCursor = 0 not written: {changes:?}"
    );
}
