//! StatusBar behaviour, as its XML load sets it up.

use super::common::script;
use crate::script::*;

#[test]
fn statusbar_methods_exist_only_on_statusbars() {
    let s = script();
    let ok: bool = s
        .eval(
            r#"
        local bar = CreateFrame("StatusBar", "SbDuck")
        local plain = CreateFrame("Frame")
        -- Duck-typing: addons branch on `if frame.SetValue then` — a plain frame must say nil.
        return (type(bar.SetValue) == "function") and (plain.SetValue == nil)
            and (type(bar.SetMinMaxValues) == "function") and (plain.SetMinMaxValues == nil)
            and (type(bar.Show) == "function") -- base methods still reachable through the fallback
    "#,
        )
        .unwrap();
    assert!(ok);
}

#[test]
fn statusbar_value_clamps_and_a_reversed_pair_clamps_min_to_max() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbClamp")
        b:SetMinMaxValues(0, 100)
        b:SetValue(250)
        assert(b:GetValue() == 100, "clamped to max")
        b:SetValue(-5)
        assert(b:GetValue() == 0, "clamped to min")
        -- A reversed pair clamps to min = max (`0x96180c`), the XML load's setter too.
        b:SetMinMaxValues(80, 20)
        local mn, mx = b:GetMinMaxValues()
        assert(mn == 20 and mx == 20, "reversed pair clamped")
        assert(b:GetValue() == 20, "held value re-clamped into the new range")
    "#,
    )
    .unwrap();
}

#[test]
fn statusbar_setminmaxvalues_range_checks_raise() {
    let s = script();
    s.run(r#"B = CreateFrame("StatusBar", "SbRange")"#).unwrap();
    for (call, want) in [
        ("B:SetMinMaxValues(-2e12, 0)", "Min or Max out of range"),
        ("B:SetMinMaxValues(0, 2e12)", "Min or Max out of range"),
        (
            "B:SetMinMaxValues(-9e11, 9e11)",
            "Min and Max too far apart",
        ),
    ] {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(want), "{call}: {err}");
    }
    let err = s.run("B:SetMinMaxValues(1)").unwrap_err().to_string();
    assert!(
        err.contains("Usage: SbRange:SetMinMaxValues(min, max)"),
        "{err}"
    );
    s.run("B:SetMinMaxValues('1', '2')")
        .expect("lua_isnumber takes a numeric string");
}

#[test]
fn statusbar_unchanged_range_fires_nothing_and_leaves_the_value() {
    let s = script();
    s.run(
        r#"
        B = CreateFrame("StatusBar", "SbSame")
        B:SetMinMaxValues(0, 10)
        B:SetValue(5)
        n = 0
        B:SetScript("OnMinMaxChanged", function() n = n + 1 end)
        B:SetMinMaxValues(0, 10)
        assert(n == 0, "unchanged range fires nothing")
        B:SetMinMaxValues(0, 3)
        assert(n == 1, "a new range fires once")
        assert(B:GetValue() == 3)
    "#,
    )
    .unwrap();
}

#[test]
fn statusbar_setvalue_fires_onvaluechanged() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbEvt")
        b:SetMinMaxValues(0, 10)
        seen = {}
        b:SetScript("OnValueChanged", function(self, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) local self, value = this, arg1
            table.insert(seen, value)
            assert(self == b and arg1 == value, "handler-firing conventions carry the value")
        end)
        b:SetValue(4)
        b:SetValue(4)          -- no change, no fire
        b:SetMinMaxValues(0, 3) -- re-clamp 4 -> 3: a value change, fires
        assert(table.getn(seen) == 2 and seen[1] == 4 and seen[2] == 3,
               "fired once per actual change: " .. table.getn(seen))
    "#,
    )
    .unwrap();
}

#[test]
fn statusbar_bar_region_scales_by_fraction_on_extract() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbFill")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        b:SetWidth(200); b:SetHeight(20)
        b:SetStatusBarTexture(0.2, 0.7, 0.2)
        b:SetMinMaxValues(0, 100)
        b:SetValue(25)
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    // The bar-fill texture: a solid-color region whose rect is 25% of the frame width.
    let bar = quads
        .iter()
        .find(|q| {
            matches!(
                &q.content,
                QuadContent::Texture {
                    path: None,
                    color: Some(_),
                    ..
                }
            )
        })
        .expect("bar fill quad");
    let r = bar.rect.expect("bar rect resolved");
    assert_eq!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 150.0, 100.0, 120.0),
        "horizontal fill: left-pinned, 25% of 200px"
    );

    // Vertical: fills bottom-up.
    s.run(r#"SbFill:SetOrientation("VERTICAL")"#).unwrap();
    let quads = s.extract();
    let bar = quads
        .iter()
        .find(|q| {
            matches!(
                &q.content,
                QuadContent::Texture {
                    path: None,
                    color: Some(_),
                    ..
                }
            )
        })
        .expect("bar fill quad");
    let r = bar.rect.expect("bar rect resolved");
    assert_eq!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 300.0, 100.0, 105.0),
        "vertical fill: bottom-pinned, 25% of 20px"
    );
}

/// The fill crops its art, never squeezes it: `SetValue` sets the UV `u1` to the fill fraction
/// `(value - min) / (max - min)` and shrinks the quad by the same fraction.
#[test]
fn statusbar_bar_region_crops_its_texture_rather_than_stretching_it() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbCrop")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        b:SetWidth(200); b:SetHeight(20)
        b:SetStatusBarTexture("Interface\\TargetingFrame\\UI-StatusBar")
        b:SetMinMaxValues(0, 100)
        b:SetValue(25)
    "#,
    )
    .unwrap();
    let uv = |s: &UiScript| -> [f32; 4] {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(_),
                    tex_coords,
                    ..
                } => tex_coords.map(|tc| tc.edges()),
                _ => None,
            })
            .expect("the bar fill always carries its crop")
    };
    s.resolve();
    assert_eq!(
        uv(&s),
        [0.0, 0.25, 0.0, 1.0],
        "horizontal: u cropped to the fill fraction, v whole"
    );

    // Vertical fills bottom-up, so the art's bottom edge is pinned.
    s.run(r#"SbCrop:SetOrientation("VERTICAL")"#).unwrap();
    assert_eq!(
        uv(&s),
        [0.0, 1.0, 0.75, 1.0],
        "vertical: v cropped up from the bottom, u whole"
    );

    // An empty bar samples nothing; a full one samples everything.
    s.run("SbCrop:SetValue(0)").unwrap();
    assert_eq!(uv(&s), [0.0, 1.0, 1.0, 1.0], "empty");
    s.run("SbCrop:SetValue(100)").unwrap();
    assert_eq!(uv(&s), [0.0, 1.0, 0.0, 1.0], "full");
}

#[test]
fn statusbar_get_texture_returns_the_bar_region() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbTex")
        assert(b:GetStatusBarTexture() == nil, "no bar before one is set")
        b:SetStatusBarTexture("Interface\\TargetingFrame\\UI-StatusBar")
        local t = b:GetStatusBarTexture()
        assert(t ~= nil and t == b:GetStatusBarTexture(), "stable region wrapper")
        b:SetStatusBarColor(1, 0, 0)
        local r, g, bb, a = b:GetStatusBarColor()
        assert(r == 1 and g == 0 and bb == 0 and a == 1)
    "#,
    )
    .unwrap();
}

/// `SetMinMaxValues` runs `OnMinMaxChanged(min, max)` on a new range, and on the first one, before
/// the re-clamp's `OnValueChanged` (`0x961859` then the `SetValue` at `0x96187b`); a repeat of the
/// held range runs nothing (`0x96181f`).
#[test]
fn statusbar_min_max_changes_fire_on_min_max_changed() {
    let s = script();
    s.run(
        r#"
        log = {}
        b = CreateFrame("StatusBar", "RangeBar")
        b:SetScript("OnMinMaxChanged", function(self, mn, mx) table.insert(log, "range:" .. mn .. "-" .. mx) end)
        b:SetScript("OnValueChanged", function(self, v) table.insert(log, "value:" .. v) end)
        b:SetMinMaxValues(0, 0)
        b:SetMinMaxValues(0, 100)
        b:SetValue(80)
        b:SetMinMaxValues(0, 100)
        b:SetMinMaxValues(0, 50)
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "range:0-0,range:0-100,value:80,range:0-50,value:50"
    );
}
