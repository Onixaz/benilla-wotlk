//! `SetTexCoord` and font objects.

use super::common::script;
use crate::script::*;

#[test]
fn set_tex_coord_changes_extracted_uv() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "TcF")
        f:SetWidth(100); f:SetHeight(100); f:SetPoint("CENTER", 0, 0)
        t = f:CreateTexture("TcTex", "ARTWORK")
        t:SetTexture("Interface\\Foo")
        t:SetTexCoord(0.1, 0.6, 0.2, 0.8)
    "#,
    )
    .unwrap();
    s.resolve();
    let tex = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Foo")))
        .expect("the Foo texture quad");
    assert!(
        matches!(&tex.content, QuadContent::Texture { tex_coords: Some(TexCoords::Rect(tc)), .. } if *tc == [0.1, 0.6, 0.2, 0.8]),
        "SetTexCoord surfaces on the extracted quad, got {:?}",
        tex.content
    );

    // `GetTexCoord` returns eight values in `SetTexCoord`'s corner order, `ULx, ULy, LLx, LLy,
    // URx, URy, LRx, LRy`: the reference has no 4-value getter.
    let got: (f32, f32, f32, f32, f32, f32, f32, f32) = s.eval("return t:GetTexCoord()").unwrap();
    assert_eq!(got, (0.1, 0.2, 0.1, 0.8, 0.6, 0.2, 0.6, 0.8));

    // The 8-argument form (UL, LL, UR, LR) lands in screen order [TL, TR, BR, BL]; a 90° turn here.
    s.run("t:SetTexCoord(0,1, 1,1, 0,0, 1,0)").unwrap();
    s.resolve();
    let tex = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Foo")))
        .unwrap();
    assert!(
        matches!(
            &tex.content,
            QuadContent::Texture { tex_coords: Some(TexCoords::Corners(c)), .. }
                if *c == [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]
        ),
        "the affine form carries corners in screen winding, got {:?}",
        tex.content
    );
    // A no-arg reset returns to the full texture.
    s.run("t:SetTexCoord()").unwrap();
    s.resolve();
    let tex = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Foo")))
        .unwrap();
    assert!(matches!(
        &tex.content,
        QuadContent::Texture {
            tex_coords: None,
            ..
        }
    ));
}

#[test]
fn set_font_object_repoints_fontstring() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "Big",
        FontObject {
            font: Some("Fonts\\MORPHEUS.TTF".into()),
            height: Some(18.0),
            color: Some([0.0, 0.0, 0.0, 1.0]),
            outline: Outline::None,
            justify_h: None,
            justify_v: None,
            shadow: None,
            spacing: None,
        },
    );
    s.register_font_object(
        "Small",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(10.0),
            color: Some([1.0, 1.0, 1.0, 1.0]),
            outline: Outline::None,
            justify_h: None,
            justify_v: None,
            shadow: None,
            spacing: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "FoF")
        f:SetWidth(120); f:SetHeight(30); f:SetPoint("CENTER", 0, 0)
        fs = f:CreateFontString("FoText", "ARTWORK")
        fs:SetFontObject("Big")
        fs:SetText("Hi")
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return fs:GetFontObject():GetName()")
            .unwrap(),
        "Big"
    );

    let resolved = |s: &UiScript| -> (Option<String>, Option<f32>, Option<[f32; 4]>) {
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text {
                    font,
                    font_height,
                    color,
                    ..
                } => Some((font, font_height, color)),
                _ => None,
            })
            .expect("a text quad")
    };
    s.resolve();
    assert_eq!(
        resolved(&s),
        (
            Some("Fonts\\MORPHEUS.TTF".into()),
            Some(18.0),
            Some([0.0, 0.0, 0.0, 1.0])
        )
    );

    s.run("fs:SetFontObject('Small')").unwrap();
    s.resolve();
    assert_eq!(
        resolved(&s),
        (
            Some("Fonts\\FRIZQT__.TTF".into()),
            Some(10.0),
            Some([1.0, 1.0, 1.0, 1.0])
        )
    );

    assert!(s.run("fs:SetFontObject('Nope')").is_err());
}

/// A FontString's `justifyV` defaults to MIDDLE, the client's default.
#[test]
fn justify_v_defaults_middle_and_overrides() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "JvF")
        f:SetWidth(100); f:SetHeight(30); f:SetPoint("CENTER", 0, 0)
        fs = f:CreateFontString("JvText", "ARTWORK") fs:SetFont('Fonts/FRIZQT__.TTF', 12)
        fs:SetText("Hi")
    "#,
    )
    .unwrap();
    let justify_v = |s: &UiScript| {
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text { justify_v, .. } => Some(justify_v),
                _ => None,
            })
            .expect("a text quad")
    };
    s.resolve();
    assert_eq!(justify_v(&s), JustifyV::Middle, "the client default");

    s.run("fs:SetJustifyV('BOTTOM')").unwrap();
    s.resolve();
    assert_eq!(justify_v(&s), JustifyV::Bottom);

    // A font object's justification applies only on an axis the string has not set itself: a
    // local setter clears that axis's inherit bit (`+0x124`) for good. XML is unaffected, as the
    // loader applies `inherits=` before the element's own `justifyV=`.
    s.register_font_object(
        "TopFont",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(12.0),
            color: None,
            outline: Outline::None,
            justify_h: None,
            justify_v: Some(JustifyV::Top),
            shadow: None,
            spacing: None,
        },
    );
    s.run("fs:SetFontObject('TopFont')").unwrap();
    s.resolve();
    assert_eq!(justify_v(&s), JustifyV::Bottom, "severance is permanent");

    // A string that never set the axis takes the object's justification.
    s.run(
        r#"
        fresh = f:CreateFontString(nil, "ARTWORK")
        fresh:SetFontObject('TopFont')
        fresh:SetText("Fresh")
    "#,
    )
    .unwrap();
    s.resolve();
    let fresh = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                justify_v,
                ..
            } if t == "Fresh" => Some(justify_v),
            _ => None,
        })
        .expect("a text quad");
    assert_eq!(fresh, JustifyV::Top);
}

/// `Texture:SetRotation(angle[, cx, cy])` (`0x48c860` → `CSimpleTexture::SetRotation 0x483120`):
/// the corners on a unit radius about the centre from `angle - π/4`, read back by `GetTexCoord` in
/// UL, LL, UR, LR order; the host's compass setter writes the same corners.
#[test]
fn set_rotation_turns_the_corners_on_a_unit_radius() {
    let mut s = script();
    s.run(
        r#"
        f = CreateFrame("Frame", "RotF")
        t = f:CreateTexture("MinimapCompassTexture")
        t:SetRotation(0)
    "#,
    )
    .unwrap();
    let r = std::f32::consts::FRAC_1_SQRT_2;
    let got: (f32, f32, f32, f32, f32, f32, f32, f32) = s.eval("return t:GetTexCoord()").unwrap();
    let want = (
        0.5 - r,
        0.5 - r,
        0.5 - r,
        0.5 + r,
        0.5 + r,
        0.5 - r,
        0.5 + r,
        0.5 + r,
    );
    for (g, w) in [
        (got.0, want.0),
        (got.1, want.1),
        (got.2, want.2),
        (got.3, want.3),
        (got.4, want.4),
        (got.5, want.5),
        (got.6, want.6),
        (got.7, want.7),
    ] {
        assert!((g - w).abs() < 1e-5, "{got:?} vs {want:?}");
    }
    // A quarter turn about (0,0) moves UL to (s, -c) of 45°: (r, -r).
    s.run("t:SetRotation(math.pi / 2, 0, 0)").unwrap();
    let ul: (f32, f32) = s.eval("return t:GetTexCoord()").unwrap();
    assert!((ul.0 - r).abs() < 1e-5 && (ul.1 + r).abs() < 1e-5, "{ul:?}");
    assert!(
        s.run("t:SetRotation('x')").is_err(),
        "a non-number angle raises"
    );
    s.set_minimap_compass_rotation(0.0);
    let again: (f32, f32) = s.eval("return t:GetTexCoord()").unwrap();
    assert!((again.0 - (0.5 - r)).abs() < 1e-5, "{again:?}");
}
