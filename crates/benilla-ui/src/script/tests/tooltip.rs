//! The GameTooltip widget's engine mechanics: the line stack and its auto-size, `SetOwner`'s
//! anchor modes, the implicit and gated shows, and the fade.

use super::common::script;
use crate::script::*;

/// Answer every pending line-measure with deterministic per-text sizes.
fn measure_all(s: &mut UiScript, sizes: &[(&str, f32, f32)]) {
    let reqs = s.fontstrings_needing_measure();
    let answers: Vec<(u32, f32, f32, u64)> = reqs
        .iter()
        .filter_map(|r| {
            sizes
                .iter()
                .find(|(t, _, _)| *t == r.text)
                .map(|&(_, w, h)| (r.id, w, h, r.key))
        })
        .collect();
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
}

#[test]
fn line_stack_autosize_and_right_flush() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local owner = CreateFrame("Button", "Slot")
        owner:SetPoint("TOPLEFT", 100, -100); owner:SetWidth(40); owner:SetHeight(40)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(owner, "ANCHOR_RIGHT")
        tt:AddLine("Tough Jerky", 1, 1, 1)
        tt:AddDoubleLine("One-Hand", "Sword", 1, 1, 1, 1, 1, 1)
        tt:AddLine("5 - 9 Damage")
        tt:Show()
        assert(tt:NumLines() == 3, "NumLines")
        assert(TTTextLeft1 and TTTextRight2, "line globals published")
        assert(TTTextLeft1:GetText() == "Tough Jerky", "line 1 text readable by name")
    "#,
    )
    .unwrap();
    s.resolve();
    measure_all(
        &mut s,
        &[
            ("Tough Jerky", 80.0, 14.0),
            ("One-Hand", 50.0, 12.0),
            ("Sword", 30.0, 12.0),
            ("5 - 9 Damage", 70.0, 12.0),
        ],
    );
    // maxw = max(80, 50+40+30, 70) = 120 ⇒ width 140; totalh = 14+2+12+2+12 = 42 ⇒ height 62.
    // ANCHOR_RIGHT: tooltip BOTTOMLEFT at owner TOPRIGHT (140, 500).
    s.run(
        r#"
        assert(TT:GetWidth() == 140, "auto width, got " .. TT:GetWidth())
        assert(TT:GetHeight() == 62, "auto height, got " .. TT:GetHeight())
    "#,
    )
    .unwrap();
    let quads = s.extract();
    let rect_of = |needle: &str| {
        quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Text { text: Some(t), .. } if t == needle => q.rect,
                _ => None,
            })
            .unwrap_or_else(|| panic!("no quad for {needle}"))
    };
    let l1 = rect_of("Tough Jerky");
    // Frame: left 140, bottom 500 ⇒ text inset TOPLEFT (150, 552).
    assert_eq!((l1.left, l1.top), (150.0, 552.0));
    let r2 = rect_of("Sword");
    // Right-flushed: right edge at frame.right − pad = 280 − 10 = 270.
    assert_eq!(r2.right, 270.0);
    // Seated on line 2's band (left2 top = 552 − 14 − 2 = 536).
    let l2 = rect_of("One-Hand");
    assert_eq!(l2.top, 536.0);
}

/// An empty line is a one-unit row plus its 2 px gap, as the reference's size getters floor a
/// FontString at one FrameXML unit; the row and the plate read that floor from one constant.
#[test]
fn empty_line_is_a_one_unit_row_and_the_chain_stays_inside_the_plate() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local owner = CreateFrame("Button", "Slot2")
        owner:SetPoint("TOPLEFT", 100, -100); owner:SetWidth(40); owner:SetHeight(40)
        local tt = CreateFrame("GameTooltip", "TTE")
        tt:SetOwner(owner, "ANCHOR_RIGHT")
        tt:AddLine("Marshal McBride", 0.25, 0.75, 0.25)
        tt:AddLine("")
        tt:AddLine("Level 20")
        tt:AddLine("PvP")
        tt:Show()
        assert(tt:NumLines() == 4, "NumLines counts the empty line")
    "#,
    )
    .unwrap();
    s.resolve();
    measure_all(
        &mut s,
        &[
            ("Marshal McBride", 90.0, 14.0),
            ("Level 20", 50.0, 12.0),
            ("PvP", 24.0, 12.0),
        ],
    );
    // Rows 14 + 1 + 12 + 12 with 3 slot gaps ⇒ totalh 45, height 65; maxw 90 ⇒ width 110.
    // The reference's `AddLine("")` returns without adding a line (`0x530270`); ours adds this row.
    s.run(
        r#"
        assert(TTE:GetWidth() == 110, "auto width, got " .. tostring(TTE:GetWidth()))
        assert(TTE:GetHeight() == 65, "auto height, got " .. tostring(TTE:GetHeight()))
        -- The chain stays contiguous through the blank row: Level 20 sits gap+1+gap under the name.
        assert(TTETextLeft3:GetTop() == TTETextLeft1:GetBottom() - 5,
               "chain contiguous through the empty row, got " .. tostring(TTETextLeft3:GetTop())
               .. " vs " .. tostring(TTETextLeft1:GetBottom()))
        -- And the tail line lands INSIDE the plate, a full pad above its bottom edge.
        assert(TTETextLeft4:GetBottom() == TTE:GetBottom() + 10,
               "tail line inside the plate, got " .. tostring(TTETextLeft4:GetBottom())
               .. " vs plate bottom " .. tostring(TTE:GetBottom()))
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

#[test]
fn owner_clear_and_hide_lifecycle() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        cleared = 0
        local a = CreateFrame("Button", "A"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local b = CreateFrame("Button", "B"); b:SetPoint("CENTER", 50, 0); b:SetWidth(10); b:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT2")
        tt:SetScript("OnTooltipCleared", function() cleared = cleared + 1 end)
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:AddLine("first hover")
        tt:Show()
        assert(tt:IsOwned(a) and not tt:IsOwned(b), "owned by a")
        tt:SetScript("OnHide", function() hidden = (hidden or 0) + 1 end)
        tt:SetOwner(b, "ANCHOR_LEFT")
        assert(not tt:IsShown() and hidden == 1, "SetOwner hid the shown tooltip")
        assert(cleared >= 1, "SetOwner cleared the old content")
        assert(tt:NumLines() == 0, "content cleared on re-own")
        assert(tt:IsOwned(b) and not tt:IsOwned(a), "owner moved")
        tt:AddLine("second hover")
        tt:Hide()
        assert(not tt:IsShown(), "hidden")
        assert(not tt:IsOwned(b), "owner dropped on hide")
        assert(tt:NumLines() == 0, "content cleared on hide")
        assert(cleared >= 2, "hide fired OnTooltipCleared")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// The host reads a tooltip's owner by name; Lua has only `IsOwned(frame)`, and no method of ours
/// answers the owner.
#[test]
fn the_host_reads_the_owner_and_lua_cannot() {
    let s = script();
    s.run(
        r#"
        local a = CreateFrame("Button", "OwnA")
        local b = CreateFrame("Button", "OwnB")
        TTO = CreateFrame("GameTooltip", "TTO")
        TTO:SetOwner(a, "ANCHOR_RIGHT")
        "#,
    )
    .unwrap();
    assert_eq!(s.tooltip_owner_name("TTO").as_deref(), Some("OwnA"));
    s.run(r#"TTO:SetOwner(OwnB, "ANCHOR_LEFT")"#).unwrap();
    assert_eq!(s.tooltip_owner_name("TTO").as_deref(), Some("OwnB"));
    s.run("TTO:Hide()").unwrap();
    assert_eq!(s.tooltip_owner_name("TTO"), None, "the hide un-owns");
    assert_eq!(s.tooltip_owner_name("NoSuchTooltip"), None);
    assert_eq!(s.tooltip_owner_name("OwnA"), None, "a Button is no tooltip");
    for method in ["BenillaGetTooltipOwner", "BenillaSetItemById"] {
        assert!(
            s.eval::<bool>(&format!("return TTO.{method} == nil"))
                .unwrap(),
            "GameTooltip:{method} is reachable"
        );
    }
}

/// `SetText` shows the tooltip and `AddLine` does not. `r, g, b` apply only when the r slot is a
/// number, so `AddLine(text, "", r, g, b)` draws in the default gold, as the reference's zone
/// tooltip does.
#[test]
fn settext_shows_and_both_addline_shapes() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local a = CreateFrame("Button", "A3"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT3")
        tt:Hide() -- the XML instance ships hidden="true"; CreateFrame defaults shown
        tt:SetOwner(a, "ANCHOR_RIGHT")
        assert(not tt:IsShown(), "SetOwner alone does not show")
        tt:AddLine("plain")
        assert(not tt:IsShown(), "AddLine does not show")
        tt:SetText("Head", 1, 1, 1)
        assert(tt:IsShown(), "SetText shows")
        assert(tt:NumLines() == 1, "SetText replaced the stack")
        -- the archaic 1.12 shape: AddLine(text, "", r, g, b) — the "" kills the colour tail
        tt:AddLine("Zone Name", "", 1.0, 0.25, 0.5)
        -- no colour at all — same default
        tt:AddLine("plain gold")
        -- the modern shape with wrap flag
        tt:AddLine("wrapped tail", 0.2, 0.4, 0.6, 1)
        -- a numeric r gates the block ON; missing g/b are UNGATED tonumber reads -> 0.0
        tt:AddLine("red only", 1)
        -- SetText requires its text: the binding raises its Usage error and adds no line
        assert(not pcall(function() tt:SetText() end), "SetText() must error")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    let color_of = |needle: &str| {
        quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Text {
                    text: Some(t),
                    color,
                    ..
                } if t == needle => *color,
                _ => None,
            })
            .unwrap_or_else(|| panic!("no quad for {needle}"))
    };
    // The default gold `0xffffd200`, where both no-colour shapes land.
    let gold = [1.0, 210.0 / 255.0, 0.0, 1.0];
    assert_eq!(color_of("Zone Name"), gold);
    assert_eq!(color_of("plain gold"), gold);
    assert_eq!(color_of("wrapped tail"), [0.2, 0.4, 0.6, 1.0]);
    // A numeric r gates the block on, and a missing g or b reads as 0.
    assert_eq!(color_of("red only"), [1.0, 0.0, 0.0, 1.0]);
    assert!(s.take_errors().is_empty());
}

/// Until its lines are measured, a tooltip keeps its declared size rather than a gaps-only one.
#[test]
fn unmeasured_lines_hold_declared_size() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local a = CreateFrame("Button", "A6"); a:SetPoint("TOPLEFT", 100, -100); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT6")
        tt:SetWidth(120); tt:SetHeight(32)
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:AddLine("Tough Jerky")
        tt:AddDoubleLine("One-Hand", "Sword")
        tt:AddLine("5 - 9 Damage")
        tt:AddLine("(3.7 damage per second)")
        tt:Show()
    "#,
    )
    .unwrap();
    // Resolve without answering the measure: the declared 120×32 holds.
    s.resolve();
    s.run(
        r#"
        assert(TT6:GetWidth() == 120, "declared width holds unmeasured, got " .. TT6:GetWidth())
        assert(TT6:GetHeight() == 32, "declared height holds unmeasured, got " .. TT6:GetHeight())
    "#,
    )
    .unwrap();
    // Then the measures land and the auto-size takes over.
    measure_all(
        &mut s,
        &[
            ("Tough Jerky", 80.0, 14.0),
            ("One-Hand", 50.0, 12.0),
            ("Sword", 30.0, 12.0),
            ("5 - 9 Damage", 70.0, 12.0),
            ("(3.7 damage per second)", 110.0, 12.0),
        ],
    );
    s.run(r#"assert(TT6:GetWidth() == 140, "auto width after measures, got " .. TT6:GetWidth())"#)
        .unwrap();
}

#[test]
fn minimum_width_floors_autosize() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local a = CreateFrame("Button", "A4"); a:SetPoint("TOPLEFT", 100, -100); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT4")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:AddLine("tiny")
        tt:SetMinimumWidth(90)
        tt:Show()
    "#,
    )
    .unwrap();
    s.resolve();
    measure_all(&mut s, &[("tiny", 20.0, 12.0)]);
    // floor 90 beats content 20 ⇒ width 110.
    s.run(r#"assert(TT4:GetWidth() == 110, "floored width, got " .. TT4:GetWidth())"#)
        .unwrap();
}

/// Fresh content mid-fade cancels the ramp at full alpha.
#[test]
fn fadeout_ramps_then_hides() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        cleared = 0
        local a = CreateFrame("Button", "A5"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT5")
        tt:SetScript("OnTooltipCleared", function() cleared = cleared + 1 end)
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetText("Fading Unit")
        tt:FadeOut()
    "#,
    )
    .unwrap();
    // Half the ramp: still shown, alpha well below 1.
    s.tick(0.25);
    s.run(r#"assert(TT5:IsShown(), "still shown mid-fade")"#)
        .unwrap();
    s.resolve();
    let mid_alpha = s
        .extract()
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if t == "Fading Unit" => Some(q.alpha),
            _ => None,
        })
        .expect("fading line still draws");
    assert!(
        mid_alpha > 0.2 && mid_alpha < 0.8,
        "mid-fade alpha ~0.5, got {mid_alpha}"
    );
    s.run(
        r#"
        TT5:SetText("Fresh Hover")
    "#,
    )
    .unwrap();
    s.tick(0.05);
    s.resolve();
    let fresh_alpha = s
        .extract()
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if t == "Fresh Hover" => Some(q.alpha),
            _ => None,
        })
        .expect("fresh line draws");
    assert_eq!(fresh_alpha, 1.0, "fresh content restored full alpha");
    // A full ramp ends hidden and cleared.
    s.run("TT5:FadeOut()").unwrap();
    s.tick(0.6);
    s.run(
        r#"
        assert(not TT5:IsShown(), "hidden at ramp end")
        assert(TT5:NumLines() == 0, "content dropped at ramp end")
        assert(cleared >= 1, "OnTooltipCleared fired")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// Every GameTooltip clamps to the screen (the geometry-flags bit 4 clamp, `assemble 0x767a20`), so
/// the zone text's `ANCHOR_LEFT` (`Minimap.lua:39`) hangs down from the screen top, not above it.
#[test]
fn owner_anchored_tooltip_clamps_to_screen() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local owner = CreateFrame("Button", "ZoneTextBtn")
        owner:SetPoint("TOPRIGHT", -100, 0); owner:SetWidth(90); owner:SetHeight(14)
        local tt = CreateFrame("GameTooltip", "TTC")
        assert(tt:IsClampedToScreen(), "a GameTooltip clamps by construction")
        assert(not owner:IsClampedToScreen(), "a plain frame does not")
        tt:SetOwner(owner, "ANCHOR_LEFT")
        tt:AddLine("Goldshire")
        tt:AddLine("Alliance Territory")
        tt:Show()
    "#,
    )
    .unwrap();
    s.resolve();
    measure_all(
        &mut s,
        &[
            ("Goldshire", 70.0, 14.0),
            ("Alliance Territory", 120.0, 12.0),
        ],
    );
    // Auto-size: maxw 120 ⇒ width 140; totalh 14+2+12 ⇒ height 48. Unclamped, the plate's
    // BOTTOMRIGHT at the owner's TOPLEFT (610, 600) puts its top at 648.
    s.run(
        r#"
        assert(TTC:GetTop() == 600, "top clamped to the screen top, got " .. TTC:GetTop())
        assert(TTC:GetBottom() == 552, "size preserved, got " .. TTC:GetBottom())
        assert(TTC:GetRight() == 610, "X untouched (inside), got " .. TTC:GetRight())
        TTC:SetClampedToScreen(false)
    "#,
    )
    .unwrap();
    s.resolve();
    // Unclamped, the plate goes back above the window.
    s.run(
        r#"
        assert(not TTC:IsClampedToScreen(), "flag readable")
        assert(TTC:GetBottom() == 600, "unclamped: bottom back at the owner's top")
        assert(TTC:GetTop() == 648, "unclamped: off the window again")
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// `0x52fa50`'s duration ladder over stand-in templates: the arm edges, the ceiling on every arm
/// but seconds, and the `_P1` plural pick.
#[test]
fn the_duration_ladder_ceils_every_arm_but_seconds() {
    // Not the shipped wording: the test is which key is reached and what number fills it.
    let table = |key: &str| -> Option<String> {
        Some(
            match key {
                "T_DAYS" => "<%d day>",
                "T_DAYS_P1" => "<%d days>",
                "T_HOURS" => "<%d hour>",
                "T_HOURS_P1" => "<%d hours>",
                "T_MIN" => "<%d min>",
                "T_MIN_P1" => "<%d mins>",
                "T_SEC" => "<%d sec>",
                "T_SEC_P1" => "<%d secs>",
                _ => return None,
            }
            .to_string(),
        )
    };
    let d = |ms: u32| crate::script::tooltip::duration_text(ms, "T", true, &table);

    // The three arm edges are exact, and the arm below counts in its own unit up to the edge.
    assert_eq!(d(86_400_000).as_deref(), Some("<1 day>"), "the day edge");
    assert_eq!(
        d(86_399_999).as_deref(),
        Some("<24 hours>"),
        "one ms under a day is still HOURS, and ceil takes it to 24"
    );
    assert_eq!(d(3_600_000).as_deref(), Some("<1 hour>"), "the hour edge");
    assert_eq!(
        d(3_599_999).as_deref(),
        Some("<60 mins>"),
        "the worked example: no '1 hour' until the hour is whole"
    );
    assert_eq!(d(60_000).as_deref(), Some("<1 min>"), "the minute edge");
    assert_eq!(
        d(61_000).as_deref(),
        Some("<2 mins>"),
        "the worked example: 61 s ceils to 2, it does not truncate to 1"
    );

    // roundUp reaches the top three arms only.
    assert_eq!(
        d(59_999).as_deref(),
        Some("<59 secs>"),
        "seconds TRUNCATE — a ceil here would read 60"
    );
    assert_eq!(d(5_400).as_deref(), Some("<5 secs>"), "5.4 s is 5, not 6");
    assert_eq!(
        d(400).as_deref(),
        Some("<0 secs>"),
        "the lapsing second: 0, and PLURAL — the reference's own last reading"
    );
    assert_eq!(d(0).as_deref(), Some("<0 secs>"), "a fully lapsed aura");
    assert_eq!(
        d(1_000).as_deref(),
        Some("<1 sec>"),
        "exactly one is singular"
    );

    // roundUp = 0 truncates the top arms too.
    assert_eq!(
        crate::script::tooltip::duration_text(61_000, "T", false, &table).as_deref(),
        Some("<1 min>"),
    );

    // A key the string table lacks yields no line.
    assert_eq!(
        crate::script::tooltip::duration_text(1_000, "NOPE", true, &table),
        None
    );
    // A family with only the singular falls back to it.
    let singular_only = |key: &str| (key == "S_SEC").then(|| "<%d s>".to_string());
    assert_eq!(
        crate::script::tooltip::duration_text(5_000, "S", true, &singular_only).as_deref(),
        Some("<5 s>"),
    );
}

/// The solver keeps a line's last measured box, unkeyed, while a re-measure is in flight, and an
/// empty line is never measured, so a pooled cell that goes empty must drop its old box: the item
/// tooltip's set block puts blank spacers on cells that held text.
#[test]
fn an_emptied_pooled_line_drops_its_stale_box_and_the_plate_still_contains_the_chain() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    // Hover one: line 2 carries real text, and gets measured.
    s.run(
        r#"
        local owner = CreateFrame("Button", "SlotR")
        owner:SetPoint("TOPLEFT", 100, -100); owner:SetWidth(40); owner:SetHeight(40)
        local tt = CreateFrame("GameTooltip", "TTR")
        tt:SetOwner(owner, "ANCHOR_RIGHT")
        tt:AddLine("Marshal McBride")
        tt:AddLine("Level 20")
        tt:AddLine("PvP")
        tt:Show()
    "#,
    )
    .unwrap();
    s.resolve();
    let sizes = &[
        ("Marshal McBride", 90.0, 14.0),
        ("Level 20", 50.0, 12.0),
        ("PvP", 24.0, 12.0),
    ];
    measure_all(&mut s, sizes);
    // Hover two, same pooled cells: line 2 is now the blank spacer, which is never measured.
    s.run(
        r#"
        TTR:ClearLines()
        TTR:AddLine("Marshal McBride")
        -- WRAPPED, as the set spacer is (`render.rs`'s `addw`): the wrap pin writes a non-zero
        -- width into `size`, so only the HEIGHT falls through to the measure cache.
        TTR:AddLine("", 1, 0.82, 0, true)
        TTR:AddLine("PvP")
    "#,
    )
    .unwrap();
    s.resolve();
    measure_all(&mut s, sizes);
    s.run(
        r#"
        -- Rows 14 + 1 + 12 with two slot gaps ⇒ totalh 31, height 51: the blank costs its gap
        -- and one floored unit, exactly as it does on a cell that never held text.
        assert(TTR:GetHeight() == 51, "auto height, got " .. tostring(TTR:GetHeight()))
        -- The one that was wrong: the tail line sits a full pad above the plate's bottom edge.
        -- Pre-fix it sat 12 BELOW it — line 2's dead "Level 20" box, drawn but never counted.
        assert(TTRTextLeft3:GetBottom() == TTR:GetBottom() + 10,
               "tail line inside the plate, got " .. tostring(TTRTextLeft3:GetBottom())
               .. " vs plate bottom " .. tostring(TTR:GetBottom()))
        -- And the chain is contiguous through the blank row, as on a cold cell.
        assert(TTRTextLeft3:GetTop() == TTRTextLeft1:GetBottom() - 5,
               "chain contiguous through the emptied row, got " .. tostring(TTRTextLeft3:GetTop()))
    "#,
    )
    .unwrap();
    assert!(s.take_errors().is_empty());
}

/// An omitted `anchorType` is mode 0, ANCHOR_LEFT: `0x61ec64` zeroes the local and the
/// `lua_isstring` gate (`0x61ec70`) skips the compare chain. `ResetPosition` (`0x61b040`) returns
/// for mode 10 (`0x61b05c`) before its clear (`0x61b070`), and the core always passes the arg
/// that gates mode 9's skip as 1 (`0x61ea9c`), so only ANCHOR_PRESERVE keeps the points.
#[test]
fn set_owner_defaults_to_anchor_left_and_only_preserve_keeps_the_placement() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Plate = CreateFrame("GameTooltip", "Plate")
        Owner = CreateFrame("Frame", "Owner")
        Owner:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 300, 300)
        Owner:SetWidth(40) Owner:SetHeight(40)
        Plate:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        Plate:SetWidth(50) Plate:SetHeight(20)
        "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.eval::<f32>("return Plate:GetLeft()").unwrap(), 100.0);

    // ANCHOR_PRESERVE (mode 10) is the one mode that leaves the plate alone.
    s.run(r#"Plate:SetOwner(Owner, "ANCHOR_PRESERVE")"#)
        .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return Plate:GetLeft()").unwrap(),
        100.0,
        "mode 10 returns before the ClearAllPoints"
    );
    assert_eq!(s.eval::<i64>("return Plate:GetNumPoints()").unwrap(), 1);

    // ANCHOR_NONE (mode 9) clears, which `GameTooltip.lua:73`'s default anchor relies on.
    s.run(r#"Plate:SetOwner(Owner, "ANCHOR_NONE")"#).unwrap();
    assert_eq!(
        s.eval::<i64>("return Plate:GetNumPoints()").unwrap(),
        0,
        "mode 9 reaches the clear: arg1 is always 1 from SetOwner's core"
    );

    // No anchor string: mode 0 places the plate's BOTTOMRIGHT at the owner's TOPLEFT.
    s.run(r#"Plate:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)"#)
        .unwrap();
    s.run("Plate:SetOwner(Owner)").unwrap();
    assert_eq!(
        s.eval::<String>("return Plate:GetAnchorType()").unwrap(),
        "ANCHOR_LEFT",
        "the omitted anchorType is the binding's zero-initialised local, mode 0"
    );
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return Plate:GetLeft()").unwrap(),
        250.0,
        "the owner's left edge (300) less the plate's own 50 wide"
    );

    // An unrecognised string falls through the compare chain: the same silent mode 0.
    s.run(r#"Plate:SetOwner(Owner, "ANCHOR_SIDEWAYS")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return Plate:GetAnchorType()").unwrap(),
        "ANCHOR_LEFT"
    );
}

/// The `SetOwner` core `0x61ea00` sets alpha 255 (`0x61ea12`), whatever dimmed the plate.
#[test]
fn set_owner_stamps_full_alpha_even_with_no_fade_running() {
    let s = script();
    s.run(
        r#"
        Owner = CreateFrame("Frame", "Owner")
        Tip = CreateFrame("GameTooltip", "Tip")
        Tip:SetOwner(Owner, "ANCHOR_RIGHT")
        Tip:AddLine("Tough Jerky", 1, 1, 1)
        Tip:Show()
        -- No fade is running: this is an outside party dimming the plate.
        Tip:SetAlpha(0.3)
        assert(Tip:GetAlpha() < 0.31, "the dimming took")
        Tip:SetOwner(Owner, "ANCHOR_RIGHT")
        assert(Tip:GetAlpha() > 0.99, "the next SetOwner stamps 255 back, fade or no fade")
        "#,
    )
    .unwrap();
}

/// `Show` (`ShowThis`, `0x61cff0`) shows only with an owner (`+0x29c`) and a line (`+0x2a4`);
/// otherwise it hides (`0x61d028`), and the tooltip's hide (`0x61eb20`) is the `SetOwner` core
/// with a NULL owner, which un-owns. `QuestieTracker.lua`'s OnEnter can call `Show` with no line.
#[test]
fn show_with_no_lines_self_hides_and_un_owns() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Owner = CreateFrame("Button", "Owner")
        Owner:SetPoint("TOPLEFT", nil, "TOPLEFT", 100, -100)
        Owner:SetWidth(40) Owner:SetHeight(40)
        Tip = CreateFrame("GameTooltip", "Tip")
        Tip:SetOwner(Owner, "ANCHOR_RIGHT")
        Tip:AddLine("Tough Jerky", 1, 1, 1)
        Tip:Show()
        assert(Tip:IsShown(), "owner + 1 line: both halves of the gate pass")
        assert(Tip:IsOwned(Owner), "and the owner survives a real show")
        "#,
    )
    .unwrap();

    // Questie's shape: a re-hover's `SetOwner` clears the lines, nothing is added, `Show` runs.
    s.run(
        r#"
        Tip:SetOwner(Owner, "ANCHOR_RIGHT")
        assert(Tip:NumLines() == 0, "SetOwner cleared the last hover's lines")
        Tip:Show()
        assert(not Tip:IsShown(), "zero lines takes the self-hide leg of 0x61cff0")
        assert(not Tip:IsOwned(Owner), "and 0x61eb20 un-owns: it is the SetOwner core with NULL")
        "#,
    )
    .unwrap();

    // A line but no owner is the other half of the gate.
    s.run(
        r#"
        Tip:AddLine("Orphan", 1, 1, 1)
        Tip:Show()
        assert(not Tip:IsShown(), "lines without an owner is the same self-hide")
        "#,
    )
    .unwrap();
}

/// `lua_isstring` (`0x61ec66`) passes only a string or a number; any other tag jumps the compare
/// chain (`0x61ec70 je 0x61edbc`) to mode 0 and raises nothing. `QuestieNotes.lua` passes a frame here
/// (`SetOwner(this, this)`).
#[test]
fn a_non_string_anchor_is_silently_mode_zero_and_never_raises() {
    let s = script();
    s.run(
        r#"
        Plate = CreateFrame("GameTooltip", "Plate")
        Owner = CreateFrame("Frame", "Owner")
        "#,
    )
    .unwrap();

    // A table, as Questie passes.
    s.run("Plate:SetOwner(Owner, Owner)")
        .expect("a table anchor takes the isstring gate's jump, it does not raise");
    assert_eq!(
        s.eval::<String>("return Plate:GetAnchorType()").unwrap(),
        "ANCHOR_LEFT"
    );

    // A boolean and an explicit nil take the same jump.
    for arg in ["true", "false", "nil"] {
        s.run(&format!(
            r#"Plate:SetOwner(Owner, "ANCHOR_RIGHT") Plate:SetOwner(Owner, {arg})"#
        ))
        .unwrap_or_else(|e| panic!("SetOwner(Owner, {arg}) raised: {e}"));
        assert_eq!(
            s.eval::<String>("return Plate:GetAnchorType()").unwrap(),
            "ANCHOR_LEFT",
            "{arg} is indistinguishable from absent at this position"
        );
    }

    // A number passes the gate as a string that matches no mode: mode 0 again.
    s.run(r#"Plate:SetOwner(Owner, "ANCHOR_RIGHT") Plate:SetOwner(Owner, 5)"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return Plate:GetAnchorType()").unwrap(),
        "ANCHOR_LEFT"
    );
}

/// `ANCHOR_CURSOR`, mode 8, clears at `SetOwner` like mode 9; the per-frame `OnLayerUpdate`
/// (`0x61b2e0`) then pins the plate's BOTTOM to the screen's BOTTOMLEFT at the cursor, over its
/// effective scale.
#[test]
fn anchor_cursor_follows_the_cursor_every_frame() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Tip = CreateFrame("GameTooltip", "Tip")
        Host = CreateFrame("Frame", "Host")
        Tip:SetOwner(Host, "ANCHOR_CURSOR")
        Tip:AddLine("Copper Ore")
        -- An explicit size, because this VM has no text measurer: the auto-size pre-pass leaves a
        -- plate with no measured extents at its declared size, and the placement is what is under
        -- test, not the width.
        Tip:SetWidth(120) Tip:SetHeight(40)
        Tip:Show()
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Tip:GetAnchorType()").unwrap(),
        "ANCHOR_CURSOR",
        "the mode round-trips; it is no longer recorded as whatever it was placed by"
    );
    assert!(
        !s.warnings().iter().any(|w| w.contains("ANCHOR_CURSOR")),
        "a mode we honour is not warned about: {:?}",
        s.warnings()
    );

    s.mouse_move(300.0, 200.0);
    s.resolve();
    let (left, right, bottom): (f32, f32, f32) = s
        .eval("return Tip:GetLeft(), Tip:GetRight(), Tip:GetBottom()")
        .unwrap();
    assert_eq!(bottom, 200.0, "the plate's BOTTOM sits at the cursor");
    assert!(
        ((left + right) / 2.0 - 300.0).abs() < 0.001,
        "centred horizontally on the cursor: {left}..{right}"
    );

    // It follows the cursor with no second `SetOwner`.
    s.mouse_move(120.0, 480.0);
    s.resolve();
    let (left, right, bottom): (f32, f32, f32) = s
        .eval("return Tip:GetLeft(), Tip:GetRight(), Tip:GetBottom()")
        .unwrap();
    assert_eq!(bottom, 480.0);
    assert!(
        ((left + right) / 2.0 - 120.0).abs() < 0.001,
        "{left}..{right}"
    );
}

/// `GetAnchorType` (`0x61d650`) answers one string, `[+0x2a0]` through the jump table
/// `0x61d79c`; `_Nameplates.lua:479` compares it with what it passed to `SetOwner`.
#[test]
fn tooltip_anchor_type_round_trips_every_reachable_mode() {
    let s = script();
    s.run(
        r#"
        AnchorTip = CreateFrame("GameTooltip", "AnchorTip")
        AnchorOwner = CreateFrame("Frame", "AnchorOwner")
        "#,
    )
    .unwrap();

    // Never nil: a plate nobody has owned answers a string too.
    assert_eq!(s.arity("AnchorTip:GetAnchorType()").unwrap(), 1, "arity 1");
    assert_eq!(
        s.eval::<String>("return type(AnchorTip:GetAnchorType())")
            .unwrap(),
        "string",
        "kind string, never nil"
    );
    assert_eq!(
        s.eval::<String>("return AnchorTip:GetAnchorType()")
            .unwrap(),
        "ANCHOR_NONE",
        "a plate nothing has owned is anchored to nothing"
    );

    // All twelve modes round-trip, ANCHOR_PRESERVE included: the reference stores it like any
    // other.
    for mode in [
        "ANCHOR_RIGHT",
        "ANCHOR_LEFT",
        "ANCHOR_TOPRIGHT",
        "ANCHOR_TOPLEFT",
        "ANCHOR_TOP",
        "ANCHOR_BOTTOMRIGHT",
        "ANCHOR_BOTTOMLEFT",
        "ANCHOR_BOTTOM",
        "ANCHOR_CURSOR",
        "ANCHOR_CURSOR_RIGHT",
        "ANCHOR_NONE",
        "ANCHOR_PRESERVE",
    ] {
        s.run(&format!(r#"AnchorTip:SetOwner(AnchorOwner, "{mode}")"#))
            .unwrap();
        assert_eq!(
            s.eval::<String>("return AnchorTip:GetAnchorType()")
                .unwrap(),
            mode,
            "round trip through SetOwner"
        );
    }

    // Lower case in, the canonical spelling out.
    s.run(r#"AnchorTip:SetOwner(AnchorOwner, "anchor_left")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return AnchorTip:GetAnchorType()")
            .unwrap(),
        "ANCHOR_LEFT"
    );

    // An unrecognised anchor is mode 0, with no raise (`0x61ec64`). Deviation: we also log a
    // warning so the unknown token shows up in diagnostics; no addon can see it.
    s.run(r#"AnchorTip:SetOwner(AnchorOwner, "ANCHOR_SIDEWAYS")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return AnchorTip:GetAnchorType()")
            .unwrap(),
        "ANCHOR_LEFT"
    );
    assert!(
        s.warnings().iter().any(|w| w.contains("ANCHOR_SIDEWAYS")),
        "the unrecognised anchor is still reported to us: {:?}",
        s.warnings()
    );
}

/// `GetOwner` (`0x61d350`) answers the owner's own object, the one `SetOwner` took, or nil before
/// any owner: `ActionButton_Update` compares it with `self` (`ActionButton.lua:265`).
#[test]
fn get_owner_answers_the_owner_frame_or_nil() {
    let s = script();
    s.run(
        r#"
        local tt = CreateFrame("GameTooltip", "OwnerTip")
        assert(tt:GetOwner() == nil, "no owner yet")
        local b = CreateFrame("Button", "OwnerBtn")
        tt:SetOwner(b, "ANCHOR_RIGHT")
        assert(tt:GetOwner() == b, "the same object SetOwner took")
        assert(tt:GetOwner() == OwnerBtn)
    "#,
    )
    .unwrap();
}

/// A Lua `SetOwner` runs the tooltip's hide first (`0x61eb76` → `0x61eb20`): its clear fires
/// OnTooltipCleared with the old owner still set, then OnHide fires for a shown plate only. The
/// core's own clear then fires OnTooltipCleared before the new owner is stored (`0x61ea06` before
/// `0x61ea7e`).
#[test]
fn set_owner_hides_then_clears_before_the_owner_is_stored() {
    let s = script();
    s.run(
        r#"
        log = {}
        local a = CreateFrame("Button", "SeqA")
        local b = CreateFrame("Button", "SeqB")
        Tip = CreateFrame("GameTooltip", "SeqTip")
        Tip:SetOwner(a, "ANCHOR_RIGHT")
        Tip:AddLine("first")
        Tip:Show()
        Tip:SetScript("OnTooltipCleared", function(self)
            local o = self:GetOwner()
            log[#log + 1] = "cleared:" .. (o and o:GetName() or "nil") .. ":"
                .. (self:IsShown() and "shown" or "hidden")
        end)
        Tip:SetScript("OnHide", function() log[#log + 1] = "hide" end)
        Tip:SetOwner(b, "ANCHOR_LEFT")
        first = table.concat(log, " ")
        log = {}
        Tip:SetOwner(a, "ANCHOR_LEFT")
        second = table.concat(log, " ")
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return first").unwrap(),
        "cleared:SeqA:shown hide cleared:nil:hidden"
    );
    assert_eq!(
        s.eval::<String>("return second").unwrap(),
        "cleared:SeqB:hidden cleared:nil:hidden",
        "an already hidden plate fires no OnHide"
    );
    assert!(s.eval::<bool>("return Tip:IsOwned(SeqA)").unwrap());
}

/// The binding hides before it checks the owner, then raises the reference's errors in order,
/// each naming the tooltip (`0x61ee6e`, `0x61ebc9`, `0x61ec1c`, `0x61ec4a`), `<unnamed>` without a
/// name (`0x9ea280`).
#[test]
fn set_owner_raises_the_reference_errors_after_hiding() {
    let s = script();
    s.run(
        r#"
        Owner = CreateFrame("Frame", "ErrOwner")
        Tip = CreateFrame("GameTooltip", "ErrTip")
        Tex = Tip:CreateTexture()
        function shown()
            Tip:SetOwner(Owner, "ANCHOR_RIGHT") Tip:AddLine("x") Tip:Show()
        end
        "#,
    )
    .unwrap();
    for (call, msg) in [
        ("Tip:SetOwner()", "Usage: ErrTip:SetOwner(frame)"),
        ("Tip:SetOwner('ErrOwner')", "Usage: ErrTip:SetOwner(frame)"),
        (
            "Tip:SetOwner({})",
            "ErrTip:SetOwner(): Couldn't find 'this' in frame object",
        ),
        (
            "Tip:SetOwner(Tex)",
            "ErrTip:SetOwner(): Wrong object type, expected frame",
        ),
        (
            "Tip:SetOwner(Tip)",
            "ErrTip:SetOwner(): Can't set owner to self",
        ),
    ] {
        s.run("shown()").unwrap();
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(msg), "{call}: {err}");
        let (shown, owned, lines, mode): (bool, bool, i64, String) = s
            .eval(
                "return Tip:IsShown() and true or false, Tip:IsOwned(Owner) and true or false, \
                 Tip:NumLines(), Tip:GetAnchorType()",
            )
            .unwrap();
        assert_eq!(
            (shown, owned, lines, mode.as_str()),
            (false, false, 0, "ANCHOR_NONE"),
            "{call} hid and un-owned before raising"
        );
    }
    let err = s
        .run("CreateFrame('GameTooltip'):SetOwner(nil)")
        .expect_err("unnamed")
        .to_string();
    assert!(err.contains("Usage: <unnamed>:SetOwner(frame)"), "{err}");
}

/// The offsets pass `lua_isnumber` (`0x61edc7`, `0x61ee00`): a numeric string counts, and any
/// other value is 0 with no error.
#[test]
fn set_owner_offsets_take_numeric_strings_and_default_to_zero() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Owner = CreateFrame("Frame", "OffOwner")
        Owner:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 300, 300)
        Owner:SetWidth(40) Owner:SetHeight(40)
        Plate = CreateFrame("GameTooltip", "OffPlate")
        Plate:SetWidth(50) Plate:SetHeight(20)
        Plate:SetOwner(Owner, "ANCHOR_RIGHT", "5", true)
        "#,
    )
    .unwrap();
    s.resolve();
    let (left, bottom): (f32, f32) = s.eval("return Plate:GetLeft(), Plate:GetBottom()").unwrap();
    assert_eq!((left, bottom), (345.0, 340.0));
}

/// 12340's own modes: ANCHOR_TOP (6) sets the plate's BOTTOM on the owner's TOP, ANCHOR_BOTTOM (3)
/// its TOP on the owner's BOTTOM (`0x61b1ed`, `0x61b101`), each at the offsets.
#[test]
fn anchor_top_and_bottom_centre_on_the_owner() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Owner = CreateFrame("Frame", "TbOwner")
        Owner:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 300, 300)
        Owner:SetWidth(40) Owner:SetHeight(40)
        Plate = CreateFrame("GameTooltip", "TbPlate")
        Plate:SetWidth(50) Plate:SetHeight(20)
        Plate:SetOwner(Owner, "ANCHOR_TOP", 2, 3)
        "#,
    )
    .unwrap();
    s.resolve();
    let (left, bottom): (f32, f32) = s.eval("return Plate:GetLeft(), Plate:GetBottom()").unwrap();
    assert_eq!((left, bottom), (297.0, 343.0));
    s.run(r#"Plate:SetOwner(Owner, "ANCHOR_BOTTOM")"#).unwrap();
    s.resolve();
    let (left, top): (f32, f32) = s.eval("return Plate:GetLeft(), Plate:GetTop()").unwrap();
    assert_eq!((left, top), (295.0, 300.0));
}

/// `ANCHOR_CURSOR_RIGHT` (11) pins the plate's BOTTOMLEFT on the cursor plus the offsets every
/// frame (`0x61b363`); `ANCHOR_CURSOR` (8) ignores the offsets (`0x61b2fd`).
#[test]
fn anchor_cursor_right_follows_the_cursor_at_the_offsets() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Tip = CreateFrame("GameTooltip", "CrTip")
        Host = CreateFrame("Frame", "CrHost")
        Tip:SetOwner(Host, "ANCHOR_CURSOR_RIGHT", 10, -4)
        Tip:AddLine("Copper Ore")
        Tip:SetWidth(120) Tip:SetHeight(40)
        Tip:Show()
        "#,
    )
    .unwrap();
    s.mouse_move(300.0, 200.0);
    s.resolve();
    let (left, bottom): (f32, f32) = s.eval("return Tip:GetLeft(), Tip:GetBottom()").unwrap();
    assert_eq!((left, bottom), (310.0, 196.0));
    s.mouse_move(120.0, 480.0);
    s.resolve();
    let (left, bottom): (f32, f32) = s.eval("return Tip:GetLeft(), Tip:GetBottom()").unwrap();
    assert_eq!((left, bottom), (130.0, 476.0));

    s.run(
        r#"
        Tip:SetOwner(Host, "ANCHOR_CURSOR", 10, -4)
        Tip:AddLine("Copper Ore")
        Tip:Show()
        "#,
    )
    .unwrap();
    s.mouse_move(300.0, 200.0);
    s.resolve();
    let (left, right, bottom): (f32, f32, f32) = s
        .eval("return Tip:GetLeft(), Tip:GetRight(), Tip:GetBottom()")
        .unwrap();
    assert_eq!(bottom, 200.0, "ANCHOR_CURSOR takes no offset");
    assert!(((left + right) / 2.0 - 300.0).abs() < 0.001);
}

/// Every hide stores mode 9 (`0x61ea8a`), so `GetAnchorType` answers "ANCHOR_NONE" after one.
#[test]
fn a_hide_resets_the_anchor_type_to_none() {
    let s = script();
    s.run(
        r#"
        local o = CreateFrame("Frame")
        Tip = CreateFrame("GameTooltip", "HnTip")
        Tip:SetOwner(o, "ANCHOR_TOP")
        Tip:Hide()
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Tip:GetAnchorType()").unwrap(),
        "ANCHOR_NONE"
    );
}

/// `SetAnchorType(anchorType [, x, y])` (`0x61d3d0`) re-places an owned plate with no clear and no
/// event; un-owned, the mode does not store (`0x61d62a`). A first argument that is not a string or
/// number raises the usage error (`0xa24d3c`).
#[test]
fn set_anchor_type_re_places_an_owned_plate_only() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        cleared = 0
        Owner = CreateFrame("Frame", "SatOwner")
        Owner:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 300, 300)
        Owner:SetWidth(40) Owner:SetHeight(40)
        Tip = CreateFrame("GameTooltip", "SatTip")
        Tip:SetWidth(50) Tip:SetHeight(20)
        Tip:SetScript("OnTooltipCleared", function() cleared = cleared + 1 end)
        Tip:SetAnchorType("ANCHOR_TOP", 4, 5)
        assert(Tip:GetAnchorType() == "ANCHOR_NONE", "un-owned: the mode does not store")
        Tip:SetOwner(Owner, "ANCHOR_RIGHT")
        Tip:AddLine("kept")
        Tip:Show()
        cleared = 0
        Tip:SetAnchorType("anchor_top", "2", 3)
        "#,
    )
    .unwrap();
    s.resolve();
    let (mode, lines, cleared, shown, left, bottom): (String, i64, i64, bool, f32, f32) = s
        .eval(
            "return Tip:GetAnchorType(), Tip:NumLines(), cleared, Tip:IsShown() and true or false, \
             Tip:GetLeft(), Tip:GetBottom()",
        )
        .unwrap();
    assert_eq!(
        (mode.as_str(), lines, cleared, shown, left, bottom),
        ("ANCHOR_TOP", 1, 0, true, 297.0, 343.0)
    );
    let err = s
        .run("Tip:SetAnchorType()")
        .expect_err("no anchor")
        .to_string();
    assert!(
        err.contains("Usage: SatTip:SetAnchorType( anchorType [,Xoffset] [,Yoffset] )"),
        "{err}"
    );
}
