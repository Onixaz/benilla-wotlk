//! The box's flush in its place in the tick's walk: after the box's own `OnUpdate`,
//! `OnCursorChanged` then `OnTextChanged`, on the dirty word `[E+0x31c]`.

use crate::script::{
    EditAction, EditBoxAdvanceRequest, EditUnit, MeasureRequest, TextMeasure, UiScript,
};

/// A stand-in font engine: each byte 7 wide and each line 14 tall, answering the advance table
/// inline as the host's engine does. A multi-line box's rows start after each newline, a trailing
/// one opening an empty row, as the reference's break routine adds one.
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
        let Some(_) = req.wrap_width else {
            return Some((cum, vec![0], 0.0));
        };
        let breaks = req.text.match_indices('\n').map(|(i, _)| i + 1);
        let rows = std::iter::once(0).chain(breaks).collect();
        Some((cum, rows, 14.0))
    }
}

fn script() -> UiScript {
    let mut s = UiScript::new().expect("construct UiScript");
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(Mono));
    s
}

/// The `LOG` entries since the last call, joined by spaces.
fn take_log(s: &UiScript) -> String {
    s.eval::<String>("local l = table.concat(LOG, ' ') LOG = {} return l")
        .unwrap()
}

/// A 200×32 box `E` at `(100, 50)` with nothing focused, logging its `OnUpdate`, `OnCursorChanged`
/// (with `x`) and `OnTextChanged` (with the text) into `LOG`, between a frame made before it and
/// one made after, whose `OnUpdate`s log too; its first-show fires are drained.
fn logged_box(s: &mut UiScript) {
    s.run(
        r#"
        LOG = {}
        EARLY = CreateFrame("Frame", "Early")
        EARLY:SetScript("OnUpdate", function() table.insert(LOG, "early") end)
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(200); E:SetHeight(32)
        E:SetScript("OnUpdate", function() table.insert(LOG, "update") end)
        E:SetScript("OnCursorChanged", function(self, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) table.insert(LOG, "cursor@" .. arg1) end)
        E:SetScript("OnTextChanged", function() table.insert(LOG, "text:" .. this:GetText()) end)
        LATE = CreateFrame("Frame", "Late")
        LATE:SetScript("OnUpdate", function() table.insert(LOG, "late") end)
    "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.016);
    s.resolve();
    take_log(s);
}

/// The box's update fires its own `OnUpdate`, then the flush, whose caret leg fires
/// `OnCursorChanged` before `OnTextChanged` fires; a frame walked after the box updates after all
/// three.
#[test]
fn the_flush_follows_the_boxs_own_on_update_cursor_then_text() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("abc")"#).unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update cursor@21 text:abc late",
        "one tick, each box flushing in its place in the walk"
    );
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update late",
        "a quiet tick fires nothing"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// An edit a box makes in its own `OnUpdate` is flushed by the same update, so its events fire in
/// the same tick.
#[test]
fn an_edit_in_the_boxs_own_on_update_fires_its_events_the_same_tick() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnUpdate", function()
            table.insert(LOG, "update")
            if not EDITED then EDITED = true this:SetText("hello") end
        end)
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@35 text:hello late");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The ctor raises dirty bit 0, so a new box fires `OnTextChanged` once at its first flush, with
/// its text unchanged; a hidden box is not walked, so its bit waits for the show.
#[test]
fn a_new_box_fires_on_text_changed_once_at_its_first_flush_once_shown() {
    let mut s = script();
    s.run(
        r#"
        SHOWN, HIDDEN = 0, 0
        local a = CreateFrame("EditBox", "A")
        a:SetAutoFocus(false)
        a:SetScript("OnTextChanged", function() SHOWN = SHOWN + 1 end)
        local b = CreateFrame("EditBox", "B")
        b:SetAutoFocus(false)
        b:Hide()
        b:SetScript("OnTextChanged", function() HIDDEN = HIDDEN + 1 end)
    "#,
    )
    .unwrap();
    let counts = |s: &UiScript| s.eval::<(i64, i64)>("return SHOWN, HIDDEN").unwrap();
    s.tick(0.016);
    assert_eq!(counts(&s), (1, 0), "the shown box's first flush");
    s.tick(0.016);
    assert_eq!(counts(&s), (1, 0), "once");
    s.run("B:Show()").unwrap();
    s.tick(0.016);
    assert_eq!(
        counts(&s),
        (1, 1),
        "the hidden box's bit waited for the show"
    );
    s.tick(0.016);
    assert_eq!(counts(&s), (1, 1));
    assert_eq!(s.eval::<String>("return B:GetText()").unwrap(), "");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Every run of the text seat on a box with a rect ends in `or eax,7`, so its next flush fires
/// `OnTextChanged` once, with the text unchanged: from a resize (the `OnSizeChanged` override),
/// `SetTextInsets`, `GetTextInsets`, `SetMultiLine` and a font change.
#[test]
fn each_re_seat_of_the_text_fires_one_on_text_changed_at_the_next_flush() {
    let mut s = script();
    s.run(
        r#"
        N = 0
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetPoint("BOTTOMLEFT", 100, 50)
        E:SetWidth(200); E:SetHeight(32)
        E:SetText("same")
        E:SetScript("OnTextChanged", function() N = N + 1 end)
    "#,
    )
    .unwrap();
    s.resolve();
    s.tick(0.016);
    let mut fires = Vec::new();
    for (what, lua, resolve) in [
        ("a resize", "E:SetWidth(250)", true),
        ("SetTextInsets", "E:SetTextInsets(4, 4, 2, 2)", false),
        ("GetTextInsets", "E:GetTextInsets()", false),
        (
            "a font change",
            r#"E:SetFont("Fonts\\FRIZQT__.TTF", 14)"#,
            false,
        ),
        ("SetMultiLine", "E:SetMultiLine(true)", false),
    ] {
        s.run(&format!("N = 0 {lua}")).unwrap();
        if resolve {
            s.resolve();
        }
        s.tick(0.016);
        fires.push((what, s.eval::<i64>("return N").unwrap()));
    }
    assert_eq!(
        fires,
        [
            ("a resize", 1),
            ("SetTextInsets", 1),
            ("GetTextInsets", 1),
            ("a font change", 1),
            ("SetMultiLine", 1),
        ]
    );
    assert_eq!(s.eval::<String>("return E:GetText()").unwrap(), "same");

    // A box with no rect yet returns before the seat and raises nothing.
    s.run(
        r#"
        M = 0
        F = CreateFrame("EditBox", "F")
        F:SetAutoFocus(false)
        F:SetScript("OnTextChanged", function() M = M + 1 end)
    "#,
    )
    .unwrap();
    s.tick(0.016);
    s.run("F:SetTextInsets(4, 4, 2, 2)").unwrap();
    s.tick(0.016);
    assert_eq!(
        s.eval::<i64>("return M").unwrap(),
        1,
        "the ctor's fire alone"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A handler's edit raises the bits the flush has already cleared, so an `OnTextChanged` that sets
/// its own text fires once more, at the next frame's flush, and never loops inside one; `SetText`
/// of an equal text raises nothing, which ends it.
#[test]
fn an_on_text_changed_that_sets_its_own_text_fires_once_more_next_frame() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnTextChanged", function()
            table.insert(LOG, "text:" .. this:GetText())
            this:SetText(strupper(this:GetText()))
        end)
        E:SetText("abc")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@21 text:abc late");
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@21 text:ABC late");
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update late");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Bit 2 clears after the caret leg, so a caret an `OnCursorChanged` handler moves is not reported
/// again; the text it inserts keeps bit 0 for the next frame.
#[test]
fn a_caret_moved_by_on_cursor_changed_is_cleared_with_the_bit() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnCursorChanged", function(self, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9)
            table.insert(LOG, "cursor@" .. arg1)
            if not EDITED then EDITED = true this:Insert("!") end
        end)
        E:SetText("ab")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@14 text:ab! late");
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update text:ab! late",
        "the insert's text bit, without its caret bit"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The focus transition raises bit 2 on both boxes: each fires `OnCursorChanged` at its next flush,
/// the one that lost the keyboard included.
#[test]
fn a_focus_change_runs_the_caret_leg_of_both_boxes() {
    let mut s = script();
    s.run(
        r#"
        LOG = {}
        for _, name in ipairs({ "A", "B" }) do
            local f = CreateFrame("EditBox", name)
            f:SetAutoFocus(false)
            f:SetScript("OnCursorChanged", function() table.insert(LOG, this:GetName()) end)
        end
    "#,
    )
    .unwrap();
    s.tick(0.016);
    take_log(&s);
    s.run("A:SetFocus()").unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "A");
    s.run("B:SetFocus()").unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "A B");
    s.run("B:ClearFocus()").unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "B");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The move helpers step the caret only while it can (`cursor < len`, `cursor > 0`), and only the
/// step raises bit 2: Right at the end and Home at 0 fire nothing.
#[test]
fn a_caret_move_that_goes_nowhere_raises_nothing() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("ab") E:SetFocus()"#).unwrap();
    s.tick(0.016);
    take_log(&s);
    let mut logs = Vec::new();
    for (what, unit, back) in [
        ("Right at the end", EditUnit::Char, false),
        ("Home from the end", EditUnit::Edge, true),
        ("Home at 0", EditUnit::Edge, true),
    ] {
        assert!(s.editbox_action(EditAction::Move {
            unit,
            back,
            extend: false,
        }));
        s.tick(0.016);
        logs.push((what, take_log(&s)));
    }
    assert_eq!(
        logs,
        [
            ("Right at the end", "early update late".to_string()),
            (
                "Home from the end",
                "early update cursor@0 late".to_string()
            ),
            ("Home at 0", "early update late".to_string()),
        ]
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnKeyDown` flushes the box before it handles the key, and a char's key-down comes before it:
/// keys typed within one frame fire their events per key, ahead of the next key's own script, so
/// `OnEnterPressed` sees the `OnTextChanged` of the text it ends.
#[test]
fn a_key_flushes_the_box_before_it_acts() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnEnterPressed", function() table.insert(LOG, "enter") end)
        E:SetFocus()
    "#,
    )
    .unwrap();
    s.tick(0.016);
    take_log(&s);
    assert!(s.char_input("a"));
    assert!(s.char_input("b"));
    assert!(s.key_input("ENTER"));
    assert_eq!(
        take_log(&s),
        "cursor@7 text:a cursor@14 text:ab enter",
        "one frame, no tick between the keys"
    );
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update late",
        "nothing left for the walk"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `HighlightText` writes only the selection, and Ctrl+A is that call: the caret bit stays down, so
/// no `OnCursorChanged` fires at the flush.
#[test]
fn highlight_text_and_ctrl_a_fire_no_on_cursor_changed() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("hello") E:SetFocus()"#).unwrap();
    s.tick(0.016);
    take_log(&s);
    assert!(s.editbox_action(EditAction::Move {
        unit: EditUnit::Edge,
        back: true,
        extend: false,
    }));
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update cursor@0 late");
    s.run("E:HighlightText()").unwrap();
    s.tick(0.016);
    assert!(s.editbox_action(EditAction::SelectAll));
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update late early update late",
        "the caret stayed at 0"
    );
    s.char_input("X");
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "X",
        "and the selection is all of it"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A focus change raises bit 2, and the caret leg restarts the blink and shows the caret in the
/// focused box: a box that takes the keyboard back starts solid, whatever phase it lost it in.
#[test]
fn taking_the_keyboard_restarts_the_caret_blink() {
    let mut s = script();
    logged_box(&mut s);
    s.run(r#"E:SetText("ab") E:SetFocus()"#).unwrap();
    s.tick(0.016);
    s.tick(0.6);
    s.resolve();
    assert!(
        !s.focused_editbox_text_ui().unwrap().caret_on,
        "past the half-period: off"
    );
    s.run("E:ClearFocus()").unwrap();
    s.tick(0.3);
    s.run("E:SetFocus()").unwrap();
    s.tick(0.016);
    assert!(
        s.focused_editbox_text_ui().unwrap().caret_on,
        "the focus's flush shows it"
    );
    s.tick(0.45);
    assert!(
        s.focused_editbox_text_ui().unwrap().caret_on,
        "with a whole half-period ahead"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// UP and DOWN in a multi-line box move a row, and `OnCursorChanged` reports it a row pitch down; a
/// single-line box would recall history instead.
#[test]
fn up_and_down_move_a_multi_line_boxs_caret_by_rows() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnCursorChanged", function(self, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9)
            table.insert(LOG, format("cursor@%d,%d", arg1, arg2))
        end)
        E:SetMultiLine(true)
        E:SetText("abc\ndefg")
        E:SetFocus()
    "#,
    )
    .unwrap();
    // The flush sizes the box to its text; the drain after the walk re-seats the resized text,
    // which the next flush reports once more.
    s.tick(0.016);
    s.tick(0.016);
    take_log(&s);
    let row = |back| EditAction::Move {
        unit: EditUnit::Row,
        back,
        extend: false,
    };
    assert!(s.editbox_action(row(true)));
    s.tick(0.016);
    assert!(s.editbox_action(row(false)));
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update cursor@21,0 late early update cursor@21,-14 late",
        "column 4 holds before the newline above, then comes back down to column 3"
    );
    s.char_input("|");
    assert_eq!(
        s.eval::<String>("return E:GetText()").unwrap(),
        "abc\ndef||g"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `OnCursorChanged`'s `y` starts at 0 and loses one row pitch per row above the caret: a
/// single-line box, and a multi-line box's first row, report +0, which Lua prints as "0", never
/// "-0".
#[test]
fn on_cursor_changed_reports_the_first_row_at_plus_zero() {
    let mut s = script();
    logged_box(&mut s);
    s.run(
        r#"
        E:SetScript("OnCursorChanged", function(self, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) table.insert(LOG, tostring(arg2)) end)
        E:SetText("ab")
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "early update 0 text:ab late", "single-line");
    s.run(r#"E:SetMultiLine(true) E:SetText("a\nb")"#).unwrap();
    s.tick(0.016);
    s.run(r#"E:SetText("c")"#).unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "early update -14 text:a\nb late early update 0 text:c late",
        "multi-line, the second row and then the first"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// [`logged_box`] logging `OnTextChanged`'s one argument, focused, its first-show fires drained.
fn user_input_box(s: &mut UiScript) {
    logged_box(s);
    s.run(
        r#"
        E:SetScript("OnUpdate", nil)
        E:SetScript("OnCursorChanged", nil)
        E:SetScript("OnTextChanged", function(self, userInput)
            table.insert(LOG, this:GetText() .. "=" .. tostring(userInput))
        end)
        EARLY:SetScript("OnUpdate", nil)
        LATE:SetScript("OnUpdate", nil)
        E:SetFocus()
    "#,
    )
    .unwrap();
    s.tick(0.016);
    take_log(s);
}

/// Typing, the delete keys, cut and a Lua `Insert` pass 0 to the insert and delete (`0x965960`,
/// `0x962f90`), so `OnTextChanged` gets a real `true`.
#[test]
fn on_text_changed_gets_true_for_keyed_edits_and_a_lua_insert() {
    let mut s = script();
    user_input_box(&mut s);
    assert!(s.char_input("a"));
    s.tick(0.016);
    assert_eq!(take_log(&s), "a=true", "typing");
    s.run(r#"E:Insert("b")"#).unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "ab=true", "a script's Insert");
    s.editbox_action(EditAction::Delete {
        unit: EditUnit::Char,
        back: true,
    });
    s.tick(0.016);
    assert_eq!(take_log(&s), "a=true", "Backspace");
    s.run("E:HighlightText(0, -1)").unwrap();
    assert_eq!(s.editbox_cut().as_deref(), Some("a"));
    s.tick(0.016);
    assert_eq!(take_log(&s), "=true", "cut");
    assert!(s.paste("c"));
    s.tick(0.016);
    assert_eq!(take_log(&s), "c=true", "paste");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetText` passes 1 to both (`0x9665ca`, `0x9665d5`), so `OnTextChanged` gets `false`, and the
/// last writer before the flush wins: typing then `SetText` is `false`, `SetText` then typing
/// `true`.
#[test]
fn on_text_changed_gets_false_for_set_text_and_the_last_writer_wins() {
    let mut s = script();
    user_input_box(&mut s);
    s.run(r#"E:SetText("x")"#).unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "x=false", "SetText");
    assert!(s.char_input("y"));
    s.run(r#"E:SetText("z")"#).unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "z=false",
        "typing, then SetText, in one frame"
    );
    // A character's key-down flushes first, so the `SetText` reports on its own.
    s.run(r#"E:SetText("p")"#).unwrap();
    assert!(s.char_input("q"));
    s.tick(0.016);
    assert_eq!(take_log(&s), "p=false pq=true", "SetText, then typing");
    s.run(r#"E:SetText("r") E:Insert("s")"#).unwrap();
    s.tick(0.016);
    assert_eq!(
        take_log(&s),
        "rs=true",
        "SetText, then Insert, in one frame"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A `SetText` the `numeric` gate refuses returns from the insert before its write of bit 3
/// (`0x965a9c`), so the typing ahead of it still reads `true`.
#[test]
fn a_refused_numeric_set_text_leaves_user_input_as_it_stood() {
    let mut s = script();
    user_input_box(&mut s);
    s.run("E:SetNumeric(true)").unwrap();
    assert!(s.char_input("7"));
    s.run(r#"E:SetText("x")"#).unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "=true");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A re-seat raises bits 0-2 and not bit 3 (`0x963740`), so a resize fires `false`.
#[test]
fn a_resize_fires_on_text_changed_with_false() {
    let mut s = script();
    user_input_box(&mut s);
    s.run("E:SetWidth(300)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(take_log(&s), "=false");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The ctor sets bit 0 alone (`0x966b70`), so the first show passes `false`, and the flush clears
/// bit 3 after the script (`0x9670cd`): an `Insert` from the handler raises it and loses it, so its
/// own `OnTextChanged`, next frame, reads `false`.
#[test]
fn the_first_show_is_false_and_the_flush_clears_the_bit_after_the_script() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        LOG = {}
        E = CreateFrame("EditBox", "E")
        E:SetAutoFocus(false)
        E:SetScript("OnTextChanged", function(self, userInput)
            table.insert(LOG, this:GetText() .. "=" .. tostring(userInput))
            if not ADDED and this:GetText() ~= "" then ADDED = true this:Insert("!") end
        end)
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "=false", "the ctor's bit");
    s.run(r#"E:Insert("a")"#).unwrap();
    s.tick(0.016);
    assert_eq!(take_log(&s), "a=true", "the insert the script reports");
    s.tick(0.016);
    assert_eq!(take_log(&s), "a!=false", "the handler's own insert");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
