//! Runtime `SetParent` (`0x7a1550`): strata and level re-assignment, the hide and show round-trip,
//! and the binding's errors.

use super::common::script;

/// `strata := parent.strata`, `level := parent.level + 1`, and the same-strata subtree shifts by
/// the same step (`0x4911b0` → `0x4910a0` with propagate 1), so a child stays above its parent.
#[test]
fn reparent_shifts_the_subtree_with_the_moved_frame() {
    let s = script();
    s.run(
        r#"
        High = CreateFrame("Frame", "High")
        High:SetFrameStrata("DIALOG")
        High:SetFrameLevel(6)
        Panel = CreateFrame("Frame", "Panel")     -- MEDIUM 0
        Child = CreateFrame("Frame", "Child", Panel)  -- MEDIUM 1 (creation inherit)
        Panel:SetParent(High)
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Panel:GetFrameStrata()").unwrap(),
        "DIALOG",
        "strata := parent's, forced onto the moved frame"
    );
    assert_eq!(
        s.eval::<i64>("return Panel:GetFrameLevel()").unwrap(),
        7,
        "level := parent.level + 1"
    );
    assert_eq!(
        s.eval::<String>("return Child:GetFrameStrata()").unwrap(),
        "DIALOG",
        "the strata force recurses over the subtree"
    );
    assert_eq!(
        s.eval::<i64>("return Child:GetFrameLevel()").unwrap(),
        8,
        "the child moves by the panel's step, 1 + 7, and stays above it"
    );
}

/// The shift skips a child in another strata: a strata already equal to the parent's returns
/// before recursing (`0x491020`), so a child's own strata survives and its level with it.
#[test]
fn reparent_leaves_a_cross_strata_child_level_alone() {
    let s = script();
    s.run(
        r#"
        Host = CreateFrame("Frame", "XSHost")
        Host:SetFrameLevel(5)                      -- MEDIUM 5
        Panel = CreateFrame("Frame", "XSPanel")    -- MEDIUM 0
        Same = CreateFrame("Frame", "XSSame", Panel)   -- MEDIUM 1
        Other = CreateFrame("Frame", "XSOther", Panel) -- MEDIUM 1, then HIGH
        Other:SetFrameStrata("HIGH")
        Panel:SetParent(Host)
        "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return Panel:GetFrameLevel()").unwrap(), 6);
    assert_eq!(s.eval::<i64>("return Same:GetFrameLevel()").unwrap(), 7);
    assert_eq!(
        s.eval::<String>("return Other:GetFrameStrata()").unwrap(),
        "HIGH"
    );
    assert_eq!(
        s.eval::<i64>("return Other:GetFrameLevel()").unwrap(),
        1,
        "a HIGH child under a MEDIUM panel is not shifted"
    );
}

/// `SetParent(nil)` resets to strata MEDIUM, level 0 (`0x4912b1`).
#[test]
fn reparent_to_nil_resets_strata_and_level() {
    let s = script();
    s.run(
        r#"
        High = CreateFrame("Frame", "SPHigh")
        High:SetFrameStrata("TOOLTIP")
        High:SetFrameLevel(9)
        F = CreateFrame("Frame", "SPF", High)
        F:SetParent(nil)
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return F:GetFrameStrata()").unwrap(),
        "MEDIUM"
    );
    assert_eq!(s.eval::<i64>("return F:GetFrameLevel()").unwrap(), 0);
}

/// A visible frame's reparent fires `OnHide` under the old parent, then `OnShow` down the subtree,
/// where AtlasLoot's `SetFrameLevel(GetParent():GetFrameLevel()+1)` lifts a child back up.
#[test]
fn a_visible_reparent_refires_onhide_then_onshow() {
    let s = script();
    s.run(
        r#"
        log = {}
        Old = CreateFrame("Frame", "RTOld")
        New = CreateFrame("Frame", "RTNew")
        New:SetFrameLevel(4)
        Mover = CreateFrame("Frame", "RTMover", Old)
        Kid = CreateFrame("Frame", "RTKid", Mover)
        Mover:SetScript("OnHide", function()
            table.insert(log, "hide:" .. Mover:GetParent():GetName())
        end)
        Mover:SetScript("OnShow", function()
            table.insert(log, "show:" .. Mover:GetParent():GetName())
        end)
        Kid:SetScript("OnShow", function()
            Kid:SetFrameLevel(Kid:GetParent():GetFrameLevel() + 1)
            table.insert(log, "kidshow")
        end)
        Mover:SetParent(New)
        "#,
    )
    .unwrap();
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["hide:RTOld", "show:RTNew", "kidshow"],
        "OnHide observes the OLD parent, OnShow the new, and the refire walks the subtree"
    );
    assert_eq!(
        s.eval::<i64>("return RTMover:GetFrameLevel()").unwrap(),
        5,
        "the moved frame sits at parent+1"
    );
    assert_eq!(
        s.eval::<i64>("return RTKid:GetFrameLevel()").unwrap(),
        6,
        "the child's own OnShow hand-repaired it above its parent — the propagate-0 law's \
         period-correct workaround"
    );
}

/// 12340's `CSimpleFrame::SetParent` (`0x4911b0`) runs the show half for a shown frame (`+0xdc`)
/// under a visible parent (`+0xe0`), so a frame moved out of a hidden parent shows, firing
/// `OnShow` once; it hid nothing, so no `OnHide`.
#[test]
fn a_shown_frame_moved_out_of_a_hidden_parent_shows() {
    let s = script();
    s.run(
        r#"
        fired = 0
        Hidden = CreateFrame("Frame", "STHidden")
        Hidden:Hide()
        Vis = CreateFrame("Frame", "STVis")
        F = CreateFrame("Frame", "STF", Hidden)   -- shown bit true, chain hidden
        F:SetScript("OnShow", function() fired = fired + 1 end)
        F:SetScript("OnHide", function() fired = fired + 1 end)
        F:SetParent(Vis)
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<i64>("return fired").unwrap(),
        1,
        "the move's OnShow alone"
    );
    assert!(
        s.eval::<bool>("return STF:IsShown() and STF:IsVisible() == 1")
            .unwrap(),
        "visible under its visible new parent"
    );
}

/// A cycle, self included, raises (`0x87cb14`); an absent argument takes the bad-name raise, not
/// the nil path.
#[test]
fn the_binding_raises_on_cycle_bad_name_and_absent_argument() {
    let s = script();
    s.run(
        r#"
        A = CreateFrame("Frame", "ErrA")
        B = CreateFrame("Frame", "ErrB", A)
        "#,
    )
    .unwrap();
    let cycle = s.run("A:SetParent(B)").unwrap_err().to_string();
    assert!(
        cycle.contains("Would create a loop parenting to ErrB"),
        "cycle raises: {cycle}"
    );
    let self_cycle = s.run("A:SetParent(A)").unwrap_err().to_string();
    assert!(self_cycle.contains("Would create a loop"), "{self_cycle}");
    let bad = s.run("A:SetParent('NoSuchFrame')").unwrap_err().to_string();
    assert!(
        bad.contains("Couldn't find region named 'NoSuchFrame'"),
        "{bad}"
    );
    let absent = s.run("A:SetParent()").unwrap_err().to_string();
    assert!(absent.contains("Couldn't find region named"), "{absent}");

    // B's parent is already A: the same-parent call is a total no-op, firing nothing.
    s.run(
        r#"
        B:SetFrameLevel(9)
        n = 0
        B:SetScript("OnHide", function() n = n + 1 end)
        B:SetScript("OnShow", function() n = n + 1 end)
        B:SetParent(A)
        "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return n").unwrap(), 0);
    assert_eq!(
        s.eval::<i64>("return B:GetFrameLevel()").unwrap(),
        9,
        "0x76ab20 skips everything — the level is not re-derived"
    );
}

/// Under `WOW_LAYOUT_VERIFY`, on for this crate's tests, every incremental pass is checked against
/// a full one, so the test only needs a reparent that changes the child's scale.
#[test]
fn a_reparent_under_a_scale_change_moves_the_childs_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        one = CreateFrame("Frame", "ScaleOne")
        one:SetPoint("BOTTOMLEFT", 0, 0); one:SetWidth(800); one:SetHeight(600)
        two = CreateFrame("Frame", "ScaleTwo")
        two:SetPoint("BOTTOMLEFT", 0, 0); two:SetWidth(800); two:SetHeight(600)
        two:SetScale(2)
        kid = CreateFrame("Frame", "ScaleKid", one)
        kid:SetPoint("BOTTOMLEFT", one, "BOTTOMLEFT", 10, 20); kid:SetWidth(100); kid:SetHeight(50)
    "#,
    )
    .unwrap();
    s.resolve();
    let at_one: (f32, f32, f32) = s
        .eval("return ScaleKid:GetLeft(), ScaleKid:GetBottom(), ScaleKid:GetWidth()")
        .unwrap();

    // Own-space reads (`GetLeft`, `GetWidth`) stay put while the screen rect doubles.
    s.run("kid:SetParent(two)").unwrap();
    s.resolve();
    let at_two: (f32, f32, f32) = s
        .eval("return ScaleKid:GetLeft(), ScaleKid:GetBottom(), ScaleKid:GetWidth()")
        .unwrap();

    assert_eq!(at_one, (10.0, 20.0, 100.0), "seated in the unscaled parent");
    assert_eq!(
        at_two,
        (10.0, 20.0, 100.0),
        "own-space coordinates are scale-relative, so these do not move"
    );
    // The extraction is in screen units.
    let widths: Vec<f32> = s
        .extract()
        .iter()
        .filter_map(|q| match q.target {
            crate::order::ZTarget::Frame(_) => q.rect.map(|r| r.right - r.left),
            _ => None,
        })
        .collect();
    assert!(
        widths.iter().any(|w| (w - 200.0).abs() < 1e-3),
        "the reparented child should be 100 x scale 2 = 200 screen units wide; got {widths:?}"
    );
}

/// A frame re-parented from a hidden parent into a frame whose own `OnShow` does the move ends up
/// visible with it: the stock quest log attaches its detail pane that way
/// (`QuestLogDetailFrame_AttachToQuestLog` from `QuestLog_OnShow`, `QuestLogFrame.lua:294`).
#[test]
fn a_frame_moved_in_by_its_new_parents_onshow_is_visible() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"
        local holder = CreateFrame("Frame", "SpHolder")
        holder:Hide()
        local pane = CreateFrame("Frame", "SpPane", holder)
        local log = CreateFrame("Frame", "SpLog")
        log:Hide()
        log:SetScript("OnShow", function() SpPane:SetParent(SpLog) end)
        log:Show()
        assert(SpLog:IsVisible() == 1, "the log is visible")
        assert(SpPane:GetParent() == SpLog, "the pane moved")
        assert(SpPane:IsVisible() == 1, "the pane shows with its new parent")
        "#,
    )
    .unwrap();
}
