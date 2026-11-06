//! The UI scale as `UIParent`'s own frame scale over a 768-unit root (`0x513240`), and the parts of
//! any scaled frame the engine places itself: each is in the frame's own units and takes its
//! effective scale (`+0x7c`), so at 0.5 it comes out half size against the half-size rect.

use super::common::script;
use crate::script::{EditBoxAdvanceRequest, MeasureRequest, QuadContent, TextMeasure, UiScript};

/// A 1366×768 root with a `UIParent` on it, as the stock file sets it.
fn root() -> UiScript {
    let mut s = script();
    s.set_screen_size(1366.0, 768.0);
    s.run(r#"CreateFrame("Frame", "UIParent"):SetAllPoints()"#)
        .unwrap();
    s
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-3, "{a} vs {b}");
}

fn num(s: &UiScript, chunk: &str) -> f64 {
    s.eval::<f64>(&format!("return {chunk}")).unwrap()
}

/// The root-unit rect `(left, right, bottom, top)` of the texture quad whose path has `needle`.
fn quad(s: &UiScript, needle: &str) -> (f32, f32, f32, f32) {
    let q = s.extract();
    let q = q
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
        .expect("the quad");
    let r = q.rect.expect("resolved");
    (r.left, r.right, r.bottom, r.top)
}

#[test]
fn the_dial_scales_a_frame_inside_uiparent_and_not_one_outside() {
    let mut s = root();
    s.run(
        r#"
        for name, parent in pairs({ Inside = UIParent, Outside = false }) do
            local f = CreateFrame("Frame", name, parent or nil)
            f:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
            f:SetWidth(200); f:SetHeight(100)
        end
    "#,
    )
    .unwrap();
    s.set_ui_scale(0.5);
    s.resolve();

    near(num(&s, "UIParent:GetScale()"), 0.5);
    near(num(&s, "UIParent:GetEffectiveScale()"), 0.5);
    near(num(&s, "Inside:GetEffectiveScale()"), 0.5);
    near(num(&s, "Outside:GetEffectiveScale()"), 1.0);

    // The same own units land on half the root inside, on all of it outside.
    assert_eq!(s.hit_test_frame(120.0, 60.0), None);
    s.run("Inside:EnableMouse(true) Outside:EnableMouse(true)")
        .unwrap();
    s.resolve();
    assert_eq!(s.hit_test_name(70.0, 70.0).as_deref(), Some("Inside"));
    assert_eq!(s.hit_test_name(290.0, 190.0).as_deref(), Some("Outside"));

    // `UIParent` still fills the root.
    near(num(&s, "UIParent:GetWidth()"), 2732.0);
    near(num(&s, "UIParent:GetHeight()"), 1536.0);
}

#[test]
fn the_screen_size_divides_by_uiparent_and_the_cursor_stays_in_root_units() {
    let mut s = root();
    near(num(&s, "GetScreenWidth()"), 1366.0);
    near(num(&s, "GetScreenHeight()"), 768.0);

    s.set_ui_scale(0.8);
    near(num(&s, "GetScreenWidth()"), 1707.5);
    near(num(&s, "GetScreenHeight()"), 960.0);

    s.mouse_move(300.0, 200.0);
    let (x, y) = s.eval::<(f64, f64)>("return GetCursorPosition()").unwrap();
    assert_eq!((x, y), (300.0, 200.0));
}

#[test]
fn the_screen_size_is_the_roots_before_uiparent_exists() {
    let mut s = script();
    s.set_screen_size(1366.0, 768.0);
    assert!(!s.set_ui_scale(0.5), "nothing to scale yet");
    near(num(&s, "GetScreenHeight()"), 768.0);

    s.run(r#"CreateFrame("Frame", "UIParent"):SetAllPoints()"#)
        .unwrap();
    assert!(s.set_ui_scale(0.5), "the frame takes it once it exists");
    near(num(&s, "GetScreenHeight()"), 1536.0);
}

#[test]
fn a_dial_move_tells_the_chat_frames_and_an_addons_own_scale_stands_until_then() {
    let mut s = root();
    s.run(
        r#"
        FLOATING = 0
        local f = CreateFrame("Frame")
        f:RegisterEvent("UPDATE_FLOATING_CHAT_WINDOWS")
        f:SetScript("OnEvent", function() FLOATING = FLOATING + 1 end)
    "#,
    )
    .unwrap();

    assert!(s.set_ui_scale(0.9));
    near(num(&s, "FLOATING"), 1.0);
    assert!(!s.set_ui_scale(0.9), "the same dial is no write");
    near(num(&s, "FLOATING"), 1.0);

    s.run("UIParent:SetScale(0.7)").unwrap();
    assert!(!s.set_ui_scale(0.9), "only a dial move overrides the addon");
    near(num(&s, "UIParent:GetScale()"), 0.7);
    near(num(&s, "GetScreenHeight()"), 768.0 / 0.7);

    assert!(s.set_ui_scale(0.8));
    near(num(&s, "UIParent:GetScale()"), 0.8);
    near(num(&s, "FLOATING"), 2.0);
}

/// `0x48fa20` caches `inset * [+0x7c]` against the rect; `0x48eac0` answers the raw values.
#[test]
fn a_scaled_frames_hit_insets_scale_and_read_back_raw() {
    let mut s = root();
    s.run(
        r#"
        local a = CreateFrame("Frame", "A")
        a:SetScale(0.5)
        a:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        a:SetWidth(200); a:SetHeight(100); a:EnableMouse(true)
        a:SetHitRectInsets(20, 40, 10, 30)
    "#,
    )
    .unwrap();
    s.resolve();

    // Root rect x 50..150, y 50..100, so the mouse rect is x 60..130, y 65..95.
    let hit = |s: &UiScript, x, y| s.hit_test_name(x, y).as_deref() == Some("A");
    assert!(!hit(&s, 59.0, 80.0), "left inset");
    assert!(hit(&s, 61.0, 80.0));
    assert!(hit(&s, 129.0, 80.0));
    assert!(!hit(&s, 131.0, 80.0), "right inset");
    assert!(!hit(&s, 100.0, 64.0), "bottom inset");
    assert!(hit(&s, 100.0, 66.0));
    assert!(hit(&s, 100.0, 94.0));
    assert!(!hit(&s, 100.0, 96.0), "top inset");

    let raw = s
        .eval::<(f64, f64, f64, f64)>("return A:GetHitRectInsets()")
        .unwrap();
    assert_eq!(raw, (20.0, 40.0, 10.0, 30.0));
}

#[test]
fn a_scaled_frames_border_is_half_as_thick() {
    let mut s = root();
    s.run(
        r#"
        local f = CreateFrame("Frame", "Plate")
        f:SetScale(0.5)
        f:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        f:SetWidth(200); f:SetHeight(100)
        f:SetBackdrop({
            bgFile = "bg", edgeFile = "edge", tile = true,
            tileSize = 16, edgeSize = 16,
            insets = { left = 4, right = 4, top = 4, bottom = 4 },
        })
    "#,
    )
    .unwrap();
    s.resolve();
    let rects: Vec<_> = s
        .extract()
        .into_iter()
        .filter(|q| matches!(q.content, QuadContent::Backdrop { .. }))
        .map(|q| q.rect.expect("resolved"))
        .collect();
    assert_eq!(rects.len(), 9);
    // The background, 2 root units in from the 50..150 rect.
    assert_eq!((rects[0].left, rects[0].right), (52.0, 148.0));
    // LEFT: 8 wide, flush with the rect, between the 8-tall corners.
    let left = rects[1];
    assert_eq!((left.left, left.right), (50.0, 58.0));
    assert_eq!((left.bottom, left.top), (58.0, 92.0));
}

/// `0x96c210`: the thumb is a texture of the frame, so at 0.5 a 32-long thumb draws 16 long and
/// `(track / scale - thumb) * fraction` lands it at the track's end.
#[test]
fn a_scaled_sliders_thumb_is_half_as_long_and_reaches_the_tracks_end() {
    let mut s = root();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "Sl")
        sl:SetScale(0.5)
        sl:SetOrientation("HORIZONTAL")
        sl:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        sl:SetWidth(200); sl:SetHeight(32); sl:EnableMouse(true)
        sl:SetThumbTexture("Interface\\Buttons\\UI-SliderBar-Button-Horizontal")
        local t = sl:GetThumbTexture()
        t:SetWidth(32); t:SetHeight(16)
        sl:SetMinMaxValues(0, 100)
        sl:SetValue(0)
    "#,
    )
    .unwrap();
    s.resolve();

    // Track x 50..150, y 50..66.
    assert_eq!(quad(&s, "Button"), (50.0, 66.0, 54.0, 62.0));
    s.run("Sl:SetValue(100)").unwrap();
    assert_eq!(quad(&s, "Button"), (134.0, 150.0, 54.0, 62.0));
    s.run("Sl:SetValue(50)").unwrap();
    assert_eq!(quad(&s, "Button"), (92.0, 108.0, 54.0, 62.0));
}

/// `0x96c320` subtracts the own-unit thumb from the scaled track, and centres on it.
#[test]
fn a_scaled_sliders_drag_keeps_the_own_unit_thumb() {
    let mut s = root();
    s.run(
        r#"
        local sl = CreateFrame("Slider", "Sl")
        sl:SetScale(0.5)
        sl:SetOrientation("HORIZONTAL")
        sl:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        sl:SetWidth(200); sl:SetHeight(32); sl:EnableMouse(true)
        sl:SetThumbTexture("Interface\\Buttons\\UI-SliderBar-Button-Horizontal")
        local t = sl:GetThumbTexture()
        t:SetWidth(32); t:SetHeight(16)
        sl:SetMinMaxValues(0, 100)
        sl:SetValueStep(0)
        sl:SetValue(0)
    "#,
    )
    .unwrap();
    s.resolve();

    // Off the thumb, 60 along the 100 track: (60 - 32 / 2) / (100 - 32).
    s.mouse_move(110.0, 58.0);
    s.mouse_button(110.0, 58.0, "LeftButton", true);
    near(num(&s, "Sl:GetValue()"), 100.0 * 44.0 / 68.0);
    s.mouse_button(110.0, 58.0, "LeftButton", false);
}

/// `0x96e0b0`, `0x96e170`: own-unit offsets on textures of the frame, so the markers draw at their
/// own size times the scale, on the point the wheel's scaled rect names.
#[test]
fn a_scaled_colour_pickers_markers_are_half_size() {
    let mut s = root();
    let xml = r#"<Ui>
        <ColorSelect name="Cs" parent="UIParent" enableMouse="true">
            <Size><AbsDimension x="365" y="200"/></Size>
            <Anchors><Anchor point="BOTTOMLEFT">
                <Offset><AbsDimension x="200" y="200"/></Offset>
            </Anchor></Anchors>
            <ColorWheelTexture name="CsWheel">
                <Size><AbsDimension x="128" y="128"/></Size>
                <Anchors><Anchor point="TOPLEFT">
                    <Offset><AbsDimension x="16" y="-32"/></Offset>
                </Anchor></Anchors>
            </ColorWheelTexture>
            <ColorWheelThumbTexture file="WheelMarker">
                <Size><AbsDimension x="10" y="10"/></Size>
            </ColorWheelThumbTexture>
            <ColorValueTexture>
                <Size><AbsDimension x="32" y="128"/></Size>
                <Anchors><Anchor point="LEFT" relativeTo="CsWheel" relativePoint="RIGHT">
                    <Offset><AbsDimension x="24" y="0"/></Offset>
                </Anchor></Anchors>
            </ColorValueTexture>
            <ColorValueThumbTexture file="ValueMarker">
                <Size><AbsDimension x="48" y="14"/></Size>
            </ColorValueThumbTexture>
        </ColorSelect>
    </Ui>"#;
    let doc = crate::framexml::parse(xml).unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    s.run("Cs:SetScale(0.5) Cs:SetColorRGB(0.5, 0.5, 0.5)")
        .unwrap();
    s.resolve();

    // The wheel's rect is x 108..172, y 120..184: S 0 centres the marker on it, 5 wide.
    assert_eq!(quad(&s, "WheelMarker"), (137.5, 142.5, 149.5, 154.5));
    // The strip's is x 184..200: `V` of the wheel's 64 tall, 24 by 7.
    let (l, r, b, t) = quad(&s, "ValueMarker");
    assert_eq!((l, r, t - b), (180.0, 204.0, 7.0));
    let v = num(&s, "(Cs:GetColorRGB())");
    near(f64::from(b + t) * 0.5, 120.0 + v * 64.0);
}

/// `0x61b2e0`: the anchor is the cursor over the tooltip's scale, so it lands under the cursor.
#[test]
fn a_scaled_world_tooltip_stays_on_the_cursor() {
    let mut s = root();
    s.run(r#"CreateFrame("GameTooltip", "GameTooltip", UIParent)"#)
        .unwrap();
    s.set_ui_scale(0.5);
    let anchor = "select(4, GameTooltip:GetPoint()), select(5, GameTooltip:GetPoint())";
    assert!(s.minimap_tooltip("Stormwind", 400.0, 300.0, false));
    let got = s.eval::<(f64, f64)>(&format!("return {anchor}")).unwrap();
    assert_eq!(got, (800.0, 600.0), "own units, doubled back at layout");

    s.world_tooltip_move(500.0, 200.0);
    let got = s.eval::<(f64, f64)>(&format!("return {anchor}")).unwrap();
    assert_eq!(got, (1000.0, 400.0));
}

/// `0x57ed70` reads the offsets against the widget's unscaled size, a pure fraction of it, which
/// the app's root-unit geometry reaches through the frame's scale.
#[test]
fn a_ping_is_the_same_fraction_of_a_scaled_minimap() {
    let mut s = root();
    s.run(
        r#"
        local m = CreateFrame("Minimap", "Mm")
        m:SetWidth(140); m:SetHeight(140)
        m:SetPoint("CENTER")
        m:SetScale(0.5)
        m:PingLocation(70, -35)
    "#,
    )
    .unwrap();
    assert_eq!(s.take_minimap_ping_request(), Some((35.0, -17.5)));
}

/// Advances and pitch in root units, as the host's seam answers them.
struct Mono;

impl TextMeasure for Mono {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let w = req.text.len() as f32 * 7.0;
        (w, 14.0, w)
    }

    fn editbox_advances(
        &mut self,
        req: &EditBoxAdvanceRequest,
    ) -> Option<(Vec<f32>, Vec<usize>, f32)> {
        let cum = (0..=req.text.len()).map(|i| i as f32 * 7.0).collect();
        let breaks = req.text.match_indices('\n').map(|(i, _)| i + 1);
        Some((cum, std::iter::once(0).chain(breaks).collect(), 7.0))
    }
}

/// `0x964a90`: the caret's `x` and the row pitch reach `OnCursorChanged` in the box's own units.
#[test]
fn a_scaled_edit_boxs_cursor_arguments_are_in_its_own_units() {
    let mut s = root();
    s.set_text_measurer(Box::new(Mono));
    s.run(
        r#"
        local e = CreateFrame("EditBox", "E")
        e:SetScale(0.5)
        e:SetAutoFocus(false)
        e:SetMultiLine(true)
        e:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        e:SetWidth(400); e:SetHeight(100)
        e:SetFontObject(GameFontNormal)
        e:SetScript("OnCursorChanged", function(self, x, y, w, h) CURSOR = { x, y, w, h } end)
        e:SetText("ab\ncdef")
        e:SetCursorPosition(5)
    "#,
    )
    .unwrap();
    s.resolve();
    for _ in 0..3 {
        s.tick(0.016);
        s.resolve();
    }
    let got = s
        .eval::<(f64, f64, f64, f64)>("return unpack(CURSOR)")
        .unwrap();
    // Row 1 at root advance 14 past its start, pitch 7: doubled, with `w` the constant 4.
    assert_eq!(got, (28.0, -14.0, 4.0, 14.0));
}
