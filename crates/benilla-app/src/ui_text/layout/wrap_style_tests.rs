//! The wrap style bits in the measure and the draw: word wrap off lays one line cut at the width
//! (`0x6c7f79`, `0x6c2103`), and the indented wrap narrows and steps in every row after the first
//! (`0x6c7ca6`, `0x6c7e25`, `0x6c7dc0`) by 15 screen pixels (`0x6be1c6`).

use super::*;
use crate::ui_text::engine::test_engine;

const FACE: &str = "Fonts\\FRIZQT__.TTF";

/// The chat line, long enough to wrap at [`WIDTH`].
const LINE: &str = "[One] says: a long line that runs past the width of the window and wraps again";

const WIDTH: f32 = 180.0;

fn spec(wrap: TextWrap) -> FontSpec<'static> {
    FontSpec {
        path: Some(FACE),
        height: Some(14.0),
        outline: Outline::None,
        alpha_gradient: None,
        spacing: 0.0,
        wrap,
    }
}

fn indented() -> TextWrap {
    TextWrap {
        indented: true,
        ..TextWrap::default()
    }
}

fn one_line(non_space_wrap: bool) -> TextWrap {
    TextWrap {
        word_wrap: false,
        non_space_wrap,
        ..TextWrap::default()
    }
}

/// Each drawn row's left ink edge and baseline-ish top, in row order.
fn row_lefts(quads: &[UiQuad]) -> Vec<(f32, f32)> {
    let mut rows: Vec<(f32, f32)> = Vec::new();
    for q in quads {
        match rows
            .iter_mut()
            .find(|(_, top)| (top - q.rect.min.y).abs() < 6.0)
        {
            Some(row) => row.0 = row.0.min(q.rect.min.x),
            None => rows.push((q.rect.min.x, q.rect.min.y)),
        }
    }
    rows.sort_by(|a, b| a.1.total_cmp(&b.1));
    rows
}

fn draw(e: &mut TextEngine, text: &str, rect: Rect, h: JustifyH, wrap: TextWrap) -> Vec<UiQuad> {
    layout_text_quads(
        e,
        text,
        rect,
        [1.0; 4],
        Justify {
            h,
            v: JustifyV::Top,
        },
        0,
        spec(wrap),
        TextSeat::UiGrid,
    )
}

/// The indent is 15 device pixels whatever the DPI: a continuation row's ink starts that much
/// right of the first row's, and the first row does not move.
#[test]
fn an_indented_continuation_row_steps_in_fifteen_screen_pixels() {
    for dpi in [1.0, 2.0] {
        let Some(mut e) = test_engine(dpi) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        let rect = Rect::new(10.0, 10.0, 10.0 + WIDTH, 400.0);
        let plain = row_lefts(&draw(
            &mut e,
            LINE,
            rect,
            JustifyH::Left,
            TextWrap::default(),
        ));
        let stepped = row_lefts(&draw(&mut e, LINE, rect, JustifyH::Left, indented()));
        assert!(plain.len() >= 2 && stepped.len() >= 2, "the line wraps");
        assert_eq!(stepped[0].0, plain[0].0, "the first row stays at the edge");
        // The same first glyph opens both rows' ink only if they start with the same letter, so
        // compare against the unindented layout's own second row.
        let indent = 15.0 / dpi;
        let shift = stepped[1].0 - plain[1].0;
        assert!(
            (shift - indent).abs() < 1.0 / dpi + 1e-3,
            "dpi {dpi}: the second row steps in {indent}, stepped {shift}"
        );
    }
}

/// A right-justified continuation row stands the indent in from the right edge; a centred one
/// stays centred.
#[test]
fn the_indent_moves_a_right_row_left_and_leaves_a_centred_row() {
    let Some(mut e) = test_engine(1.0) else {
        eprintln!("skipping: no install / font chain");
        return;
    };
    let rect = Rect::new(0.0, 0.0, WIDTH, 400.0);
    let right_edge = |quads: &[UiQuad], row: usize| {
        let rows = row_lefts(quads);
        let top = rows[row].1;
        quads
            .iter()
            .filter(|q| (q.rect.min.y - top).abs() < 6.0)
            .map(|q| q.rect.max.x)
            .fold(f32::MIN, f32::max)
    };
    let plain = draw(&mut e, LINE, rect, JustifyH::Right, TextWrap::default());
    let stepped = draw(&mut e, LINE, rect, JustifyH::Right, indented());
    assert_eq!(right_edge(&stepped, 0), right_edge(&plain, 0));
    assert!(
        right_edge(&stepped, 1) <= WIDTH - 15.0 + 1.0,
        "a right row ends the indent short of the edge"
    );
    let centred = draw(&mut e, "ab\nab", rect, JustifyH::Center, indented());
    let rows = row_lefts(&centred);
    assert_eq!(rows[0].0, rows[1].0, "a centred row does not step");
}

/// The continuation rows wrap the indent narrower, so a line whose second row just fits the full
/// width takes a third row indented, in the measure and the message rows alike.
#[test]
fn an_indented_string_wraps_its_continuation_rows_narrower() {
    let Some(mut e) = test_engine(1.0) else {
        eprintln!("skipping: no install / font chain");
        return;
    };
    // Find a width where the indent costs a row.
    let mut found = None;
    for w in (120..400).map(|w| w as f32) {
        let plain = measure_wrapped_rows(&mut e, LINE, w, spec(TextWrap::default()));
        let stepped = measure_wrapped_rows(&mut e, LINE, w, spec(indented()));
        assert!(stepped >= plain, "the indent never saves a row at {w}");
        if stepped > plain {
            found = Some((w, plain, stepped));
            break;
        }
    }
    let (w, plain, stepped) = found.expect("some width where the indent costs a row");
    // The FontString measure agrees with the message rows.
    let size = measure_text(&mut e, LINE, Some(w), spec(indented())).1;
    let plain_h = measure_text(&mut e, LINE, Some(w), spec(TextWrap::default())).1;
    assert!(
        size > plain_h,
        "{w}: {stepped} rows against {plain} must measure taller"
    );
    // The draw lays the same rows.
    let rect = Rect::new(0.0, 0.0, w, 1000.0);
    assert_eq!(
        row_lefts(&draw(&mut e, LINE, rect, JustifyH::Left, indented())).len(),
        usize::from(stepped)
    );
}

/// A `\n` line's first row is a continuation row too: it steps in.
#[test]
fn a_new_lines_first_row_steps_in() {
    let Some(mut e) = test_engine(1.0) else {
        eprintln!("skipping: no install / font chain");
        return;
    };
    let rect = Rect::new(0.0, 0.0, 400.0, 400.0);
    let rows = row_lefts(&draw(&mut e, "Hi\nHi", rect, JustifyH::Left, indented()));
    assert_eq!(rows.len(), 2);
    assert!(
        (rows[1].0 - rows[0].0 - 15.0).abs() < 1.0 + 1e-3,
        "{rows:?}"
    );
}

/// Word wrap off draws the first line alone, cut at its last fitting character, not at a word,
/// and measures one line tall, as wide as its widest line.
#[test]
fn word_wrap_off_draws_one_line_cut_at_the_width() {
    let Some(mut e) = test_engine(1.0) else {
        eprintln!("skipping: no install / font chain");
        return;
    };
    let rect = Rect::new(0.0, 0.0, WIDTH, 400.0);
    let wrapped = draw(&mut e, LINE, rect, JustifyH::Left, TextWrap::default());
    let cut = draw(&mut e, LINE, rect, JustifyH::Left, one_line(false));
    assert!(row_lefts(&wrapped).len() >= 2);
    assert_eq!(row_lefts(&cut).len(), 1, "one line");
    let ink = |q: &[UiQuad]| q.iter().map(|q| q.rect.max.x).fold(0.0f32, f32::max);
    assert!(ink(&cut) <= WIDTH + 1.0, "the line is cut at the width");
    // A word cut ends past where the word wrap broke: the cut keeps every glyph that fits.
    let first_row_end = {
        let rows = row_lefts(&wrapped);
        wrapped
            .iter()
            .filter(|q| (q.rect.min.y - rows[0].1).abs() < 6.0)
            .map(|q| q.rect.max.x)
            .fold(0.0f32, f32::max)
    };
    assert!(
        ink(&cut) > first_row_end,
        "the cut is mid-word, past the word break at {first_row_end}"
    );
    // A second `\n` line is not drawn.
    assert_eq!(
        row_lefts(&draw(
            &mut e,
            "ab\ncd",
            rect,
            JustifyH::Left,
            one_line(false)
        ))
        .len(),
        1
    );
    // The measure is one line tall and, unpinned, as wide as the widest line, not the drawn one:
    // the extent walk spans every newline whatever the wrap bits (`0x483f1b`, `0x6bce60`).
    let (w1, h1) = measure_text(&mut e, "ab\ncdefghijkl", None, spec(one_line(false)));
    let (wa, h0) = measure_text(&mut e, "ab", None, spec(TextWrap::default()));
    let (wide, _) = measure_text(&mut e, "cdefghijkl", None, spec(TextWrap::default()));
    assert!(wide > wa + 20.0, "the lines differ clearly: {wa} vs {wide}");
    assert_eq!((w1, h1), (wide, h0));
    let (_, hw) = measure_text(&mut e, LINE, Some(WIDTH), spec(one_line(false)));
    assert_eq!(hw, h0, "one line at a pinned width too");
}

/// The fit walk with word wrap off: a cut line stops the walk unless the non-space wrap bit lets
/// it go on (`0x6bd50b`, `0x6c2124`), so a box two lines tall ellipsizes the string with the bit
/// clear and draws it raw, cut, with the bit set.
#[test]
fn a_cut_line_ellipsizes_unless_the_non_space_wrap_bit_walks_on() {
    let Some(mut e) = test_engine(1.0) else {
        eprintln!("skipping: no install / font chain");
        return;
    };
    let r = measure::resolve(&mut e, &spec(TextWrap::default()));
    e.ensure_metrics(r.face, r.ppem, LINE);
    e.ensure_metrics(r.face, r.ppem, "...");
    let box_h = 4.0 * r.size;
    let cap = overflow::lines_fitting(box_h, r.size) + 1;
    let fit = |wrap: TextWrap| {
        overflow::ellipsize_in_box(LINE, box_h, r.size, |candidate| {
            measure::wrapped_rows_capped_styled_for_test(&e, r, candidate, WIDTH, wrap, cap)
        })
    };
    let clear = fit(one_line(false)).expect("the bit clear stops the walk at the cut");
    assert!(clear.ends_with("..."), "{clear:?}");
    assert_eq!(
        measure::wrapped_rows_capped_styled_for_test(&e, r, &clear, WIDTH, one_line(false), cap),
        1,
        "{clear:?} fits the one line"
    );
    assert_eq!(
        fit(one_line(true)),
        None,
        "the bit set walks the rest: it fits four lines"
    );
    // A string that fits its line never ellipsizes, whatever the bit.
    let fits = |wrap: TextWrap| {
        overflow::ellipsize_in_box("[One] says", box_h, r.size, |candidate| {
            measure::wrapped_rows_capped_styled_for_test(&e, r, candidate, WIDTH, wrap, cap)
        })
    };
    assert_eq!(fits(one_line(false)), None);
}

/// Word wrap off with a blank line first: the newline only steps a row (`0x6c7ced`), so the
/// first non-blank line draws one row down, cut narrower and stepped in by the indent since the
/// line count is already 1 (`0x6c7cad`, `0x6c7e25`); the measure stays one row (`0x6bda3d`).
#[test]
fn word_wrap_off_draws_the_first_line_past_leading_blank_lines() {
    let Some(mut e) = test_engine(1.0) else {
        eprintln!("skipping: no install / font chain");
        return;
    };
    let rect = Rect::new(0.0, 0.0, 400.0, 400.0);
    let plain = row_lefts(&draw(&mut e, "ab", rect, JustifyH::Left, one_line(false)));
    assert_eq!(plain.len(), 1);
    let pitch = {
        let two = row_lefts(&draw(
            &mut e,
            "ab\nab",
            rect,
            JustifyH::Left,
            TextWrap::default(),
        ));
        two[1].1 - two[0].1
    };
    for (text, down) in [("\nab", 1.0), ("\n\nab", 2.0), ("|nab", 1.0)] {
        let rows = row_lefts(&draw(&mut e, text, rect, JustifyH::Left, one_line(false)));
        assert_eq!(rows.len(), 1, "{text:?} draws one line");
        assert!(
            (rows[0].1 - plain[0].1 - down * pitch).abs() < 1e-3,
            "{text:?} sits {down} rows down: {rows:?} vs {plain:?}"
        );
        assert!(
            (rows[0].0 - plain[0].0).abs() < 1e-3,
            "{text:?} not indented"
        );
        let wrap = TextWrap {
            indented: true,
            ..one_line(false)
        };
        let stepped = row_lefts(&draw(&mut e, text, rect, JustifyH::Left, wrap));
        assert!(
            (stepped[0].0 - plain[0].0 - 15.0).abs() < 1.0 + 1e-3,
            "{text:?} indented steps in: {stepped:?}"
        );
        let (w, h) = measure_text(&mut e, text, None, spec(one_line(false)));
        assert_eq!(
            (w, h),
            measure_text(&mut e, "ab", None, spec(TextWrap::default())),
            "{text:?} measures one row, as wide as its widest line"
        );
    }
    // A blank string still has its one row and draws nothing.
    assert!(draw(&mut e, "\n", rect, JustifyH::Left, one_line(false)).is_empty());
}
