//! Shape C: a numeric position the reference reads with a bare `lua_tonumber` and no `lua_isnumber`
//! gate, so nil, a table or a string is 0.0 and the call completes. Shape B, not tested here, is a
//! pre-staged default, such as every colour setter's alpha.

use super::common::script;

#[test]
fn every_shape_c_colour_position_takes_nil_as_zero_and_never_raises() {
    let s = script();
    s.run(
        r#"
        F = CreateFrame("Frame", "F")
        T = F:CreateTexture(nil, "ARTWORK")
        FS = F:CreateFontString(nil, "ARTWORK")
        SB = CreateFrame("StatusBar", "SB")
        CS = CreateFrame("ColorSelect", "CS")
        "#,
    )
    .unwrap();

    // `Texture:SetVertexColor` (`2=C 3=C 4=C`), as stock `QuestLogFrame.lua:337` calls it.
    s.run("T:SetVertexColor(nil, nil, nil)")
        .expect("SetVertexColor(nil,nil,nil) is three bare lua_tonumbers, not a raise");
    s.run(
        r#"
        local r, g, b = T:GetVertexColor()
        assert(r == 0 and g == 0 and b == 0, "a nil channel stores 0.0, it does not keep the old one")
        "#,
    )
    .unwrap();
    // A table and a string are 0.0 too: `lua_tonumber` tests nothing.
    s.run("T:SetVertexColor({}, \"abc\", true)")
        .expect("no tag is rejected at a shape-C position");

    // `Texture:SetTexCoord`: every coordinate C; only the arity raises.
    s.run("T:SetTexCoord(nil, nil, nil, nil)")
        .expect("four nil coordinates are four zeroes");
    s.run("T:SetTexCoord(0, 1, 0, 1, 0, 1, 0, 1)")
        .expect("the 8-corner form still takes numbers");
    s.run("T:SetTexCoord(1, 2, 3)")
        .expect_err("but 3 args is neither 4 nor 8, and the arity DOES raise");

    // `FontString:SetShadowColor` and `StatusBar:SetStatusBarColor`.
    s.run("FS:SetShadowColor(nil, nil, nil)")
        .expect("FontString:SetShadowColor is shape C on r/g/b");
    s.run("SB:SetStatusBarColor(nil, nil, nil)")
        .expect("StatusBar:SetStatusBarColor is shape C on r/g/b");
    s.run(
        r#"
        local r, g, b = SB:GetStatusBarColor()
        assert(r == 0 and g == 0 and b == 0, "and it stored the zeroes")
        "#,
    )
    .unwrap();

    // `ColorSelect:SetColorRGB`: C on all three; it fires OnColorSelect either way.
    s.run("CS:SetColorRGB(nil, nil, nil)")
        .expect("ColorSelect:SetColorRGB is shape C on r/g/b");

    // `Texture:SetGradient` and `SetGradientAlpha`.
    s.run("T:SetGradient(\"HORIZONTAL\", nil, nil, nil, nil, nil, nil)")
        .expect("SetGradient's six stops are all C");
    s.run("T:SetGradientAlpha(\"VERTICAL\", nil, nil, nil, nil, nil, nil, nil, nil)")
        .expect("SetGradientAlpha's eight are C/B");

    // `FontString:SetTextColor`.
    s.run("FS:SetTextColor(nil, nil, nil)")
        .expect("FontString:SetTextColor is shape C on r/g/b");
}
