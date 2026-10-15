//! Frame-API basics: `GetLocale`, `SetAllPoints` and `GetPoint` readback.

use super::common::script;

#[test]
fn get_locale_and_set_all_points() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    // `GetLocale`: benilla is enUS-only.
    assert_eq!(s.eval::<String>("return GetLocale()").unwrap(), "enUS");

    s.run(
        r#"
        p = CreateFrame("Frame", "SAP_Parent")
        p:SetPoint("BOTTOMLEFT", 100, 100); p:SetWidth(200); p:SetHeight(50)
        c = CreateFrame("Frame", "SAP_Child", p)
        c:SetAllPoints()                          -- default: the parent
        f = CreateFrame("Frame", "SAP_Free")
        f:SetAllPoints("SAP_Parent")              -- by name
    "#,
    )
    .unwrap();
    s.resolve();

    let ok: bool = s
        .eval(
            r#"
        return c:GetWidth() == 200 and c:GetHeight() == 50
           and f:GetWidth() == 200 and f:GetHeight() == 50
    "#,
        )
        .unwrap();
    assert!(ok, "SetAllPoints matched the target rect");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn getpoint_reads_back_anchors() {
    let s = script();
    s.run(
        r#"
        a = CreateFrame("Frame", "GP_A")
        a:SetPoint("BOTTOMLEFT", 100, 100)
        b = CreateFrame("Frame", "GP_B")
        b:SetPoint("TOPLEFT", a, "BOTTOMRIGHT", 4, -8)
    "#,
    )
    .unwrap();
    // Screen-anchored: `relativeTo` reads nil.
    let ok: bool = s
        .eval(
            r#"
        local p, rel, rp, x, y = GP_A:GetPoint()
        return p == "BOTTOMLEFT" and rel == nil and rp == "BOTTOMLEFT" and x == 100 and y == 100
    "#,
        )
        .unwrap();
    assert!(ok, "screen anchor reads back");
    // Frame-anchored: `relativeTo` is the same wrapper table.
    let ok: bool = s
        .eval(
            r#"
        local p, rel, rp, x, y = GP_B:GetPoint(1)
        return p == "TOPLEFT" and rel == a and rp == "BOTTOMRIGHT" and x == 4 and y == -8
    "#,
        )
        .unwrap();
    assert!(ok, "frame anchor reads back with wrapper identity");
    // Out-of-range n: all nils.
    assert!(s.eval::<bool>("return GP_B:GetPoint(9) == nil").unwrap());
}

/// `SetClampRectInsets` (`0x4a0310`) widens a clamped frame's bounds by each inset, scaled by the
/// frame, and the getter reads them back; a missing number raises the named usage.
#[test]
fn clamp_rect_insets_move_the_screen_clamp() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        cl = CreateFrame("Frame", "ClampProbe")
        cl:SetWidth(100); cl:SetHeight(50)
        cl:SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", -30, -20)
        cl:SetClampedToScreen(true)
        "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<(f32, f32)>("return cl:GetLeft(), cl:GetBottom()")
            .unwrap(),
        (0.0, 0.0),
        "no insets: the screen's own edges"
    );
    // The world map's (0, 0, 0, -60): no lower than 60 above the bottom edge.
    s.run("cl:SetClampRectInsets(10, 0, 0, -60)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<(f32, f32)>("return cl:GetLeft(), cl:GetBottom()")
            .unwrap(),
        (-10.0, 60.0)
    );
    s.run("cl:SetScale(2)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return cl:GetBottom()").unwrap(),
        60.0,
        "the inset scales with the frame, in its own units"
    );
    assert_eq!(
        s.eval::<(f64, f64, f64, f64)>("return cl:GetClampRectInsets()")
            .unwrap(),
        (10.0, 0.0, 0.0, -60.0)
    );
    let err = s
        .run("cl:SetClampRectInsets(1, 2, 3)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Usage: ClampProbe:SetClampRectInsets(left, right, top, bottom)"),
        "{err}"
    );
}

/// `QuestPOIFrame` is its own registered type over `Frame` (`0x5210c1`, `0x58fb00`), its
/// settings clamped as the methods clamp them; with no quest POI data no blob is drawn or hovered.
#[test]
fn a_quest_poi_frame_is_a_frame_with_its_blob_methods() {
    let s = script();
    s.run(
        r##"
        b = CreateFrame("QuestPOIFrame", "BlobProbe", UIParent)
        assert(b:GetObjectType() == "QuestPOIFrame")
        assert(b:IsObjectType("Frame") and b:IsObjectType("QuestPOIFrame"))
        b:SetFillTexture("Interface\\WorldMap\\UI-QuestBlob-Inside")
        b:SetBorderTexture("Interface\\WorldMap\\UI-QuestBlob-Outside")
        b:SetFillAlpha(300); b:SetBorderAlpha(-5); b:SetBorderScalar(1.0)
        b:SetMergeThreshold(0.9); b:SetNumSplinePoints(3)
        b:EnableSmoothing(true); b:EnableMerging(1)
        b:DrawQuestBlob(42, true); b:UpdateQuestPOI()
        assert(select("#", b:UpdateMouseOverTooltip(0.5, 0.5)) == 0)
        assert(b:GetNumTooltips() == 0 and b:GetTooltipIndex(1) == 0)
        b:SetScale(0.691)
        "##,
    )
    .unwrap();
    let model = s
        .lua()
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model");
    let h = model.arena.lookup("BlobProbe").expect("named");
    let crate::widget::KindState::QuestPoi(p) = &model.arena.frame(h).expect("live").kind_state
    else {
        panic!("not a QuestPOIFrame");
    };
    assert_eq!(
        (
            p.fill_alpha,
            p.border_alpha,
            p.merge_threshold,
            p.spline_points
        ),
        (255, 0, 0.5, 8)
    );
    assert!(p.smoothing && p.merging);
    assert_eq!(
        p.border_texture.as_deref(),
        Some("Interface\\WorldMap\\UI-QuestBlob-Outside")
    );
}
