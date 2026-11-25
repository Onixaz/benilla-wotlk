//! The wrap style bits reach the host's measure and draw: a FontString's word wrap off (`0x40`),
//! non-space wrap (`0x1000`) and indented wrap (`0x20000`, its own or its font's), and a message
//! frame's font indent, which each line's string takes (`0x969fa0`).

use super::common::script;
use crate::script::*;

/// The wrap bits on the text quad of `text`.
fn quad_wrap(s: &UiScript, text: &str) -> TextWrap {
    s.extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                wrap,
                ..
            } if t == text => Some(wrap),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a text quad for {text:?}"))
}

/// Each setter re-asks the measure under a new key carrying its bit, and the draw carries it too.
#[test]
fn the_wrap_setters_re_measure_and_reach_the_draw() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local w = CreateFrame("Frame", "Win")
        w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(400); w:SetHeight(300)
        local t = w:CreateFontString("Label", "ARTWORK") t:SetFont('Fonts/FRIZQT__.TTF', 12)
        t:SetText("a label"); t:SetWidth(120)
        t:SetPoint("TOPLEFT", 10, -10)
    "#,
    )
    .unwrap();
    s.resolve();
    let first = s.fontstrings_needing_measure();
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].wrap,
        TextWrap::default(),
        "the constructor's 0x212"
    );
    s.set_measured_text_unwrapped(&[(first[0].id, 50.0, 14.0, first[0].key)]);
    s.resolve();
    assert!(s.fontstrings_needing_measure().is_empty(), "cache warm");
    assert_eq!(quad_wrap(&s, "a label"), TextWrap::default());

    let mut keys = vec![first[0].key];
    for (lua, want) in [
        (
            "Label:SetWordWrap(false)",
            TextWrap {
                word_wrap: false,
                ..TextWrap::default()
            },
        ),
        (
            "Label:SetNonSpaceWrap(true)",
            TextWrap {
                word_wrap: false,
                non_space_wrap: true,
                indented: false,
            },
        ),
        // 12340 registers the FontString's setter under the getter's name (`0x48e600`).
        (
            "Label:GetIndentedWordWrap(true)",
            TextWrap {
                word_wrap: false,
                non_space_wrap: true,
                indented: true,
            },
        ),
    ] {
        s.run(lua).unwrap();
        s.resolve();
        let reqs = s.fontstrings_needing_measure();
        assert_eq!(reqs.len(), 1, "{lua} re-asks the measure");
        assert_eq!(reqs[0].wrap, want, "{lua}");
        assert!(!keys.contains(&reqs[0].key), "{lua} moves the key");
        keys.push(reqs[0].key);
        s.set_measured_text_unwrapped(&[(reqs[0].id, 50.0, 14.0, reqs[0].key)]);
        s.resolve();
        assert_eq!(quad_wrap(&s, "a label"), want, "{lua} reaches the draw");
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A FontString without its own indent bit takes its font's, live: a later
/// `SetIndentedWordWrap` on the font re-measures and redraws its dependents, and the string's own
/// setting wins over the font's.
#[test]
fn a_font_strings_indent_follows_its_font_until_set() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        IndentFont = CreateFont("IndentFont")
        IndentFont:SetFont('Fonts/FRIZQT__.TTF', 12)
        local w = CreateFrame("Frame", "Win")
        w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(400); w:SetHeight(300)
        local t = w:CreateFontString("Dep", "ARTWORK") t:SetFontObject(IndentFont)
        t:SetText("dependent"); t:SetWidth(120); t:SetPoint("TOPLEFT", 10, -10)
    "#,
    )
    .unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert!(!reqs[0].wrap.indented);
    s.set_measured_text_unwrapped(&[(reqs[0].id, 50.0, 14.0, reqs[0].key)]);
    s.resolve();
    s.run("IndentFont:SetIndentedWordWrap(true)").unwrap();
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(reqs.len(), 1, "the font's bit re-asks its dependent");
    assert!(reqs[0].wrap.indented);
    assert!(quad_wrap(&s, "dependent").indented);
    assert_eq!(
        s.eval::<Option<i64>>("return Dep:SetIndentedWordWrap()")
            .unwrap(),
        Some(1),
        "the getter (registered as the setter) answers the inherited bit"
    );
    s.run("Dep:GetIndentedWordWrap(false)").unwrap();
    s.resolve();
    assert!(
        !quad_wrap(&s, "dependent").indented,
        "the string's own bit wins"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A message frame's font indent rides every line's row request and draw, and toggling it asks
/// the rows again: the chat window's scroll and page counts follow the new row counts.
#[test]
fn a_message_frames_indent_reaches_its_rows_and_lines() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        "local f = CreateFrame('ScrollingMessageFrame', 'CF')\n\
         f:SetPoint('BOTTOMLEFT', 32, 90)\n\
         f:SetWidth(300)\n\
         f:SetHeight(120)\n\
         f:AddMessage('one', 1, 1, 1)\n\
         f:AddMessage('two', 1, 1, 1)",
    )
    .unwrap();
    s.resolve();
    let reqs = s.message_lines_needing_measure();
    assert_eq!(reqs.len(), 2);
    assert!(reqs.iter().all(|r| !r.indented));
    let answers: Vec<_> = reqs.iter().map(|r| (r.frame, r.index, 1, r.key)).collect();
    s.set_message_line_rows(&answers);
    s.resolve();
    assert!(s.message_lines_needing_measure().is_empty(), "rows warm");

    s.run("CF:SetIndentedWordWrap(true)").unwrap();
    s.resolve();
    let reqs = s.message_lines_needing_measure();
    assert_eq!(reqs.len(), 2, "the indent asks every line's rows again");
    assert!(reqs.iter().all(|r| r.indented));
    let answers: Vec<_> = reqs.iter().map(|r| (r.frame, r.index, 1, r.key)).collect();
    s.set_message_line_rows(&answers);
    s.resolve();
    assert!(quad_wrap(&s, "two").indented, "the line draws indented");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A font found by a mixed-case name of any length gives its dependents its indent: the lookup
/// lowercases short names on the stack and a name past the buffer the long way.
#[test]
fn a_mixed_case_font_name_of_any_length_reaches_its_indent() {
    for name in ["IndentFont", &format!("Long{}Font", "X".repeat(140))] {
        let mut s = script();
        s.set_screen_size(800.0, 600.0);
        s.run(&format!(
            r#"
            local f = CreateFont("{name}")
            f:SetFont('Fonts/FRIZQT__.TTF', 12) f:SetIndentedWordWrap(true)
            local w = CreateFrame("Frame", "Win")
            w:SetPoint("TOPLEFT", 0, -100); w:SetWidth(400); w:SetHeight(300)
            local t = w:CreateFontString("Dep", "ARTWORK") t:SetFontObject("{name}")
            t:SetText("dependent"); t:SetWidth(120); t:SetPoint("TOPLEFT", 10, -10)
        "#
        ))
        .unwrap();
        s.resolve();
        assert!(quad_wrap(&s, "dependent").indented, "{} bytes", name.len());
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
