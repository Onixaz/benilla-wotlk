//! The engine verbs no stock file calls (#399): the empty debug toggles, the refused file verbs,
//! the build flags, the profiler's readers with profiling off, the event listener list,
//! `GetCVarInfo` and `GetCoinTextureString`.

use crate::script::{AddOnInfo, SeededCvar, UiScript};

fn raised(s: &UiScript, chunk: &str) -> String {
    s.run(chunk)
        .expect_err(&format!("{chunk} must raise"))
        .to_string()
}

/// `0x8e5250`: any arguments, nothing returned.
#[test]
fn the_empty_debug_toggles_take_anything_and_return_nothing() {
    let s = UiScript::new().unwrap();
    for name in [
        "QuestPOIUpdateTexture",
        "ResetPerformanceValues",
        "ToggleCollision",
        "ToggleCollisionDisplay",
        "TogglePerformanceDisplay",
        "TogglePerformancePause",
        "TogglePerformanceValues",
        "TogglePlayerBounds",
        "TogglePortals",
        "ToggleTris",
    ] {
        assert_eq!(
            s.eval::<i64>(&format!("return select('#', {name}(1, 'x', {{}}))"))
                .unwrap(),
            0,
            "{name}"
        );
    }
}

/// `0x510bb0` raises `Access Denied` whatever it is passed.
#[test]
fn the_file_verbs_are_refused() {
    let s = UiScript::new().unwrap();
    for name in [
        "AppendToFile",
        "DeleteFile",
        "GMRequestPlayerInfo",
        "ReadFile",
    ] {
        assert!(
            raised(&s, &format!("{name}('x.txt', 'y')")).contains("Access Denied"),
            "{name}"
        );
    }
}

#[test]
fn the_build_flags_answer_as_the_shipped_client_on_a_realm_login() {
    let s = UiScript::new().unwrap();
    assert!(!s.eval::<bool>("return IsDebugBuild()").unwrap());
    assert_eq!(s.eval::<i64>("return IsDesaturateSupported()").unwrap(), 1);
    assert!(!s.eval::<bool>("return IsBNLogin()").unwrap());
}

/// The in-game binding of the glue's verb: `gxResolution`'s width.
#[test]
fn the_movie_resolution_is_the_resolution_width_in_game_too() {
    let s = UiScript::new().unwrap();
    s.register_cvars([("gxResolution", "1600x900")]);
    assert_eq!(s.eval::<i64>("return GetMovieResolution()").unwrap(), 1600);
}

/// With profiling off every time is 0, but `0x81aa00` counts each signalled event, and
/// `GetEventCPUUsage` sums them over every event whatever name it is passed.
#[test]
fn the_event_signal_count_is_kept_without_profiling_and_reset() {
    let mut s = UiScript::new().unwrap();
    s.run("ResetCPUUsage()").unwrap();
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::new());
    s.fire_event("BAG_UPDATE", Vec::new());
    s.fire_event("BAG_UPDATE", Vec::new());
    for name in ["", "'BAG_UPDATE'", "'PLAYER_ENTERING_WORLD'"] {
        let (time, count): (f64, i64) =
            s.eval(&format!("return GetEventCPUUsage({name})")).unwrap();
        assert_eq!((time, count), (0.0, 3), "GetEventCPUUsage({name})");
    }
    s.run("ResetCPUUsage()").unwrap();
    assert_eq!(
        s.eval::<(f64, i64)>("return GetEventCPUUsage()").unwrap(),
        (0.0, 0)
    );
    assert_eq!(s.eval::<f64>("return GetScriptCPUUsage()").unwrap(), 0.0);
}

#[test]
fn the_frame_and_function_timers_read_zero_and_check_their_arguments() {
    let s = UiScript::new().unwrap();
    s.run("F = CreateFrame('Frame')").unwrap();
    assert_eq!(
        s.eval::<(f64, i64)>("return GetFrameCPUUsage(F, false)")
            .unwrap(),
        (0.0, 0)
    );
    assert_eq!(
        s.eval::<(f64, i64)>("return GetFunctionCPUUsage(print, true)")
            .unwrap(),
        (0.0, 0)
    );
    assert!(raised(&s, "GetFrameCPUUsage('F')").contains("Usage: GetFrameCPUUsage(frame"));
    assert!(raised(&s, "GetFrameCPUUsage({})").contains("Couldn't find 'this'"));
    assert!(raised(&s, "GetFunctionCPUUsage(F)").contains("Usage: GetFunctionCPUUsage(function"));
}

#[test]
fn addon_cpu_usage_is_zero_and_checks_its_index_like_memory_usage() {
    let mut s = UiScript::new().unwrap();
    s.register_addons(
        vec![AddOnInfo {
            name: "Probe".into(),
            interface: 30300,
            enabled: true,
            ..Default::default()
        }],
        None,
        None,
        None,
    );
    s.note_addon_info_reply(&[]);
    s.run("UpdateAddOnCPUUsage()").unwrap();
    assert_eq!(s.eval::<f64>("return GetAddOnCPUUsage(1)").unwrap(), 0.0);
    assert_eq!(
        s.eval::<f64>("return GetAddOnCPUUsage('Probe')").unwrap(),
        0.0
    );
    assert!(
        raised(&s, "GetAddOnCPUUsage(2)").contains("AddOn index must be in the range of 1 to 1")
    );
    assert!(raised(&s, "GetAddOnCPUUsage()").contains("Usage: GetAddOnCPUUsage"));
}

/// Registration order, an unregistered frame gone, and nothing for an event no frame holds.
#[test]
fn frames_registered_for_an_event_come_back_in_registration_order() {
    let s = UiScript::new().unwrap();
    s.run(
        "A = CreateFrame('Frame') B = CreateFrame('Frame') C = CreateFrame('Frame')
         B:RegisterEvent('BAG_UPDATE') A:RegisterEvent('BAG_UPDATE') C:RegisterEvent('BAG_UPDATE')
         C:UnregisterEvent('BAG_UPDATE')",
    )
    .unwrap();
    assert!(s
        .eval::<bool>(
            "local x, y, z = GetFramesRegisteredForEvent('BAG_UPDATE')
             return x == B and y == A and z == nil"
        )
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return select('#', GetFramesRegisteredForEvent('UNIT_AURA'))")
            .unwrap(),
        0
    );
    assert!(raised(&s, "GetFramesRegisteredForEvent()").contains("Usage:"));
}

/// Value, default, then 1 or nil for the per-account and per-character flags; four nils for a
/// name the mirror lacks.
#[test]
fn cvar_info_reports_value_default_and_where_it_is_saved() {
    let s = UiScript::new().unwrap();
    let row = |name: &str, account: bool, character: bool| SeededCvar {
        name: name.into(),
        value: "1".into(),
        default: "0".into(),
        device_default: None,
        latched: false,
        per_account: account,
        per_character: character,
    };
    s.seed_cvars([
        row("autoLootDefault", true, false),
        row("lockActionBars", false, true),
        row("gxWindow", false, false),
    ]);
    let info = |name: &str| {
        s.eval::<(Option<String>, Option<String>, Option<i64>, Option<i64>)>(&format!(
            "return GetCVarInfo('{name}')"
        ))
        .unwrap()
    };
    let some = |v: &str| Some(v.to_string());
    assert_eq!(
        info("autoLootDefault"),
        (some("1"), some("0"), Some(1), None)
    );
    assert_eq!(
        info("lockActionBars"),
        (some("1"), some("0"), None, Some(1))
    );
    assert_eq!(info("gxWindow"), (some("1"), some("0"), None, None));
    assert_eq!(info("noSuchCVar"), (None, None, None, None));
    assert_eq!(
        s.eval::<i64>("return select('#', GetCVarInfo('noSuchCVar'))")
            .unwrap(),
        4
    );
    assert!(raised(&s, "GetCVarInfo()").contains("Usage: GetCVarInfo(\"cvar\")"));
}

#[test]
fn coin_texture_string_formats_each_denomination_at_the_font_height() {
    let s = UiScript::new().unwrap();
    s.run(
        r#"GOLD_AMOUNT_TEXTURE = "%d|TInterface\\MoneyFrame\\UI-GoldIcon:%d:%d:2:0|t"
           SILVER_AMOUNT_TEXTURE = "%d|TInterface\\MoneyFrame\\UI-SilverIcon:%d:%d:2:0|t"
           COPPER_AMOUNT_TEXTURE = "%d|TInterface\\MoneyFrame\\UI-CopperIcon:%d:%d:2:0|t""#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return GetCoinTextureString(10203)")
            .unwrap(),
        r"1|TInterface\MoneyFrame\UI-GoldIcon:14:14:2:0|t 2|TInterface\MoneyFrame\UI-SilverIcon:14:14:2:0|t 3|TInterface\MoneyFrame\UI-CopperIcon:14:14:2:0|t"
    );
    assert_eq!(
        s.eval::<String>("return GetCoinTextureString(50000, 12.9)")
            .unwrap(),
        r"5|TInterface\MoneyFrame\UI-GoldIcon:12:12:2:0|t"
    );
    assert_eq!(
        s.eval::<String>("return GetCoinTextureString(0, 'x')")
            .unwrap(),
        r"0|TInterface\MoneyFrame\UI-CopperIcon:14:14:2:0|t"
    );
    assert!(
        raised(&s, "GetCoinTextureString('x')").contains("Usage: GetCoinText(amount, fontHeight)")
    );
}
