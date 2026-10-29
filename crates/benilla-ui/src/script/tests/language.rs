//! The `|4` grammar pass at every text store that runs it (the font string's text setter with its
//! process flag, `0x483910`): font strings, button labels, tooltip lines and message lines, but not
//! the edit box, whose display passes the flag clear.

use super::common::script;
use crate::script::QuadContent;

fn drawn_text(s: &crate::script::UiScript, needle: &str) -> Option<String> {
    s.extract().iter().find_map(|q| match &q.content {
        QuadContent::Text { text: Some(t), .. } if t.contains(needle) => Some(t.clone()),
        _ => None,
    })
}

#[test]
fn a_font_string_stores_the_picked_form() {
    let s = script();
    s.run(
        r#"F = CreateFrame("Frame", "F")
           FS = F:CreateFontString("FS", "ARTWORK") FS:SetFont('Fonts/FRIZQT__.TTF', 12)
           FS:SetText("12 |4hat:hats; left")"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return FS:GetText()").unwrap(),
        "12 hats left"
    );
    s.run(r#"FS:SetFormattedText("%d |4hat:hats; left", 1)"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return FS:GetText()").unwrap(),
        "1 hat left"
    );
}

#[test]
fn a_button_label_and_a_tooltip_line_pick_too() {
    let s = script();
    s.run(
        r#"B = CreateFrame("Button", "B")
           B:SetText("2 |4box:boxes;")
           TT = CreateFrame("GameTooltip", "TT")
           TT:SetOwner(B, "ANCHOR_NONE")
           TT:AddLine("1 |4box:boxes;")
           TT:AddDoubleLine("3 |4cat:cats;", "1 |4dog:dogs;")"#,
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return B:GetText()").unwrap(), "2 boxes");
    assert_eq!(
        s.eval::<(String, String, String)>(
            "return TTTextLeft1:GetText(), TTTextLeft2:GetText(), TTTextRight2:GetText()"
        )
        .unwrap(),
        ("1 box".into(), "3 cats".into(), "1 dog".into())
    );
}

#[test]
fn message_lines_pick_and_the_edit_box_keeps_the_escape() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"M = CreateFrame("ScrollingMessageFrame", "M")
           M:SetPoint("BOTTOMLEFT", 32, 90) M:SetWidth(430) M:SetHeight(34)
           M:AddMessage("7 |4wolf:wolves; howl")
           E = CreateFrame("EditBox", "E")
           E:SetText("2 |4x:y;")"#,
    )
    .unwrap();
    s.resolve();
    let rows: Vec<(u32, u32, u16, u64)> = s
        .message_lines_needing_measure()
        .iter()
        .map(|r| (r.frame, r.index, 1, r.key))
        .collect();
    s.set_message_line_rows(&rows);
    assert_eq!(drawn_text(&s, "howl").as_deref(), Some("7 wolves howl"));
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "2 |4x:y;");
}
