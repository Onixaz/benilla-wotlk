//! Measure and fit, in the emit pass's own space ([`super`]): the width law, the wrap walk and the
//! ellipsis seam. A width is a pure function of face, raster size, outline and characters, which
//! lets a measure answer from inside a Lua call where no shaper reaches.

use bevy::math::Rect;

use benilla_ui::script::{JustifyH, JustifyV, TextWrap};

use super::super::engine::{TextEngine, UiFontAtlas};
use super::super::markup::{
    fontstring_lines, icon_index, icons_of, parse_markup_icons, ColorRun, InlineIcon,
};
use super::wrap::{cut_line, fitting_prefix, greedy_pack_rows, tokenize_words};
use super::{client_step, overflow, FontSpec, Justify, HEIGHT_LIMIT_MIN, WRAP_MIN_WIDTH};

/// A [`FontSpec`] resolved once, so measure and draw agree on face and raster size.
#[derive(Clone, Copy)]
pub(super) struct Resolved {
    /// Index into the engine's faces.
    pub(super) face: usize,
    /// The exact integer device-pixel size ([`TextEngine::ppem`]).
    pub(super) ppem: u16,
    /// The logical height `ppem` draws at, not the requested one: the pitch and every block em.
    pub(super) size: f32,
    /// The outline cell variant (`0` = plain).
    pub(super) radius: u8,
    /// The step law's per-glyph bias ([`super::step_extra_of`]).
    pub(super) step_extra: f32,
}

/// Resolve a spec without touching the caches ([`TextEngine::ensure_metrics`] covers the text).
pub(super) fn resolve(e: &mut TextEngine, font: &FontSpec) -> Resolved {
    let face = e.face_for(font.path);
    let ppem = e.ppem(font.height.unwrap_or(super::super::DEFAULT_FONT_SIZE));
    Resolved {
        face,
        ppem,
        size: e.logical_size(ppem),
        radius: super::super::outline::radius_of(font.outline),
        step_extra: super::step_extra_of(font.outline),
    }
}

/// Each embedded texture's `[width, height]` in logical px at this font, whole device px each, by
/// the spec parser's sizing (`0x6c0e80`) at the drawn font height. Deviation: the reference also
/// multiplies explicit sizes by the frame's layout scale on top of the font height that already
/// carries it; benilla scales an icon with its font alone.
pub(super) fn icon_sizes(e: &TextEngine, r: Resolved, icons: &[InlineIcon]) -> Vec<[f32; 2]> {
    if icons.is_empty() {
        return Vec::new();
    }
    let pixel_scale = benilla_ui::markup::texture_pixel_scale(r.size, e.aspect());
    let dpi = e.dpi();
    icons
        .iter()
        .map(|i| {
            let [w, h] = i.geometry.size(r.size, pixel_scale);
            [
                (w * dpi).round().max(0.0) / dpi,
                (h * dpi).round().max(0.0) / dpi,
            ]
        })
        .collect()
}

/// One line's width in logical px, the sum of its characters' steps ([`client_step`]) and its
/// icons' widths as the reference's extent (`0x6c1430`) sums them; every measure, the one inside a
/// Lua call ([`benilla_ui::script::TextMeasure`]) included, runs through here. The caller ensures
/// `text`'s metrics first: this is `&`-only, and a character not in the cache counts 0.
pub(super) fn measure_line_width(
    e: &TextEngine,
    r: Resolved,
    step_extra: f32,
    text: &str,
    icons: &[[f32; 2]],
) -> f32 {
    // Summed in device px and divided once, as the pen does: a sum of quotients is another `f32`,
    // and measure must equal render to the bit.
    let dpi = e.dpi();
    let steps: f32 = text
        .chars()
        .map(|c| match icon_index(c) {
            Some(k) => icons.get(k).map_or(0.0, |s| (s[0] * dpi).round()),
            None => e
                .char_cell(r.face, r.ppem, c)
                .map_or(0.0, |c| c.floor_sum + step_extra * c.glyphs.len() as f32),
        })
        .sum();
    steps / dpi
}

/// The width law itself, for the differential test that pins it to a shaped sum.
#[cfg(test)]
pub(crate) fn measure_line_width_for_test(
    e: &mut TextEngine,
    face: usize,
    ppem: u16,
    step_extra: f32,
    text: &str,
) -> f32 {
    e.ensure_metrics(face, ppem, text);
    let r = Resolved {
        face,
        ppem,
        size: e.logical_size(ppem),
        radius: 0,
        step_extra,
    };
    measure_line_width(e, r, step_extra, text, &[])
}

/// Greedy wrap of one markup line into rows no wider than `max_width` px, the reference's wrap
/// kernel: break at the last opportunity, else force-break at the last fitting glyph; inter-word
/// whitespace stays verbatim and only the separator at a break drops.
///
/// Break opportunities are whitespace only: the reference's classifier also breaks after `-`, `:`
/// and `/` and by CJK kinsoku classes, and its `nonspacewrap` flag (ui `0x1000`) is read by the
/// one-line fit walk alone ([`one_line_fit_rows`]); after a cut word here it is not (#1468).
pub(super) fn wrap_line(
    e: &TextEngine,
    r: Resolved,
    line: &[ColorRun],
    max_width: f32,
    icons: &[[f32; 2]],
) -> Vec<Vec<ColorRun>> {
    wrap_line_rows(e, r, line, max_width, max_width, icons)
}

/// [`wrap_line`] with its first row `first_width` wide and every later one `rest_width`.
fn wrap_line_rows(
    e: &TextEngine,
    r: Resolved,
    line: &[ColorRun],
    first_width: f32,
    rest_width: f32,
    icons: &[[f32; 2]],
) -> Vec<Vec<ColorRun>> {
    let words = tokenize_words(line);
    if words.is_empty() {
        // A blank or all-whitespace line still takes a row.
        return vec![line.to_vec()];
    }
    greedy_pack_rows(words, first_width, rest_width, |t| {
        measure_line_width(e, r, r.step_extra, t, icons)
    })
}

/// The indented word wrap's step in device px: 15 screen pixels whatever the font, scale or
/// window, as the window-size handler stores it (`0x6be1c6`).
const INDENT_DEVICE_PX: f32 = 15.0;

/// The step an indented string's continuation rows take, in logical px; 0 when not indented.
pub(super) fn indent_of(e: &TextEngine, wrap: TextWrap) -> f32 {
    if wrap.indented {
        INDENT_DEVICE_PX / e.dpi()
    } else {
        0.0
    }
}

/// The rows the markup `lines` lay out into at `width`, as the string's geometry pass lays them
/// (`0x6c7b10`). Word wrap off draws one line and stops (`0x6c7f79`), cut at the last character
/// within a pinned width (`0x6c2103`); a blank line before it only steps a row (`0x6c7ced`), so it
/// draws below and indented. Wrapped, every row after the first, a `\n` line's first row included,
/// wraps narrower by the indent (`0x6c7ca6`). An unpinned width wraps nothing.
pub(super) fn layout_rows(
    e: &TextEngine,
    r: Resolved,
    lines: Vec<Vec<ColorRun>>,
    width: f32,
    wrap: TextWrap,
    icons: &[[f32; 2]],
) -> Vec<Vec<ColorRun>> {
    let pinned = width > WRAP_MIN_WIDTH;
    if !wrap.word_wrap {
        let mut rows: Vec<Vec<ColorRun>> = Vec::new();
        let mut lines = lines.into_iter().peekable();
        while let Some(line) = lines.next() {
            let blank = line.iter().all(|run| run.text.is_empty());
            // A line opening with the newline code adds a row and steps y (`0x6c7cf2`).
            if blank && lines.peek().is_some() {
                rows.push(line);
                continue;
            }
            if !pinned {
                rows.push(line);
                break;
            }
            // From the second row on the indent narrows it (`0x6c7cad`).
            let w = if rows.is_empty() {
                width
            } else {
                width - indent_of(e, wrap)
            };
            let (row, kept) = cut_line(&line, w, |t| {
                measure_line_width(e, r, r.step_extra, t, icons)
            });
            // Not one character fits: the pass stops before drawing a line.
            if kept > 0 || blank {
                rows.push(row);
            }
            break;
        }
        return rows;
    }
    if !pinned {
        return lines;
    }
    let rest = width - indent_of(e, wrap);
    let mut out: Vec<Vec<ColorRun>> = Vec::new();
    for line in &lines {
        let first = if out.is_empty() { width } else { rest };
        out.extend(wrap_line_rows(e, r, line, first, rest, icons));
    }
    out
}

/// `text`'s laid-out size, the widest line by all lines' height, wrapping at `wrap_width` by the
/// render's own [`wrap_line`] pass; the engine's measure round-trip sizes height-less FontStrings
/// by it ([`benilla_ui::script::UiScript::fontstrings_needing_measure`]).
pub(crate) fn measure_text(
    e: &mut TextEngine,
    text: &str,
    wrap_width: Option<f32>,
    font: FontSpec,
) -> (f32, f32) {
    let r = resolve(e, &font);
    // Metrics only: a string measured and never drawn costs no raster.
    e.ensure_metrics(r.face, r.ppem, text);
    let e = &*e;

    let (lines, icons) = fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]);
    let icons = icon_sizes(e, r, &icons);
    // The wrapped height's walk (`0x6bd900`) counts the rows the draw lays out: one with word wrap
    // off (`0x6bda3d`), the continuation rows narrower when indented (`0x6bd979`).
    let width_of = |rows: &[Vec<ColorRun>]| {
        rows.iter()
            .map(|line| {
                line.iter()
                    .filter(|run| !run.text.is_empty())
                    .map(|run| measure_line_width(e, r, r.step_extra, &run.text, &icons))
                    .sum::<f32>()
            })
            .fold(0.0f32, f32::max)
    };
    let (max_w, render_lines) = match wrap_width {
        Some(w) => {
            let rows = layout_rows(e, r, lines, w, font.wrap, &icons);
            (width_of(&rows), rows)
        }
        None => {
            // An unpinned string is as wide as its widest line under any wrap bits: the extent
            // walk (`0x6bce60`, called at `0x483f1b`) spans every newline. Its height walks the
            // rows laid out at that width, where an indented continuation line wraps again; the
            // draw lays them out in the same box (`0x486be4`).
            let natural = width_of(&lines);
            let w = if font.wrap.indented && font.wrap.word_wrap {
                natural.ceil() + 1.0
            } else {
                0.0
            };
            let rows = layout_rows(e, r, lines, w, font.wrap, &icons);
            (natural, rows)
        }
    };
    // Deviation: one pixel of headroom over the reference's measured width, kept because dropping
    // it narrows every auto-sized FontString, a look change. Height: N lines at the font em with no
    // outline pad (`N·S + (N-1)·gap`); an outlined ring pokes past it, as in the reference.
    (
        max_w.ceil() + 1.0,
        super::text_block_h(block_rows(&render_lines, font.wrap), r.size, font.spacing),
    )
}

/// The rows a measure counts: with word wrap off the wrapped height's walk ends after its first
/// row (`0x6bda3d`), a blank one included; the draw's block counts every row (`0x6c61a8`).
fn block_rows(rows: &[Vec<ColorRun>], wrap: TextWrap) -> usize {
    if wrap.word_wrap {
        rows.len()
    } else {
        rows.len().min(1)
    }
}

/// How many rows `text` wraps into at `wrap_width`, by the render's own [`layout_rows`] pass: the
/// engine's ScrollingMessageFrame allocates `rows × font height` per line from it
/// ([`benilla_ui::script::UiScript::message_lines_needing_measure`]), each line's string taking
/// the frame font's indent (`0x969fa0`). Never 0.
pub(crate) fn measure_wrapped_rows(
    e: &mut TextEngine,
    text: &str,
    wrap_width: f32,
    font: FontSpec,
) -> u16 {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, text);
    let rows = wrapped_rows(e, r, text, wrap_width, font.wrap);
    rows.clamp(1, usize::from(u16::MAX)) as u16
}

/// The wrapped row count of `text` at `wrap_width`; an unconstrained width counts the `\n` lines.
fn wrapped_rows(e: &TextEngine, r: Resolved, text: &str, wrap_width: f32, wrap: TextWrap) -> usize {
    wrapped_rows_capped(e, r, text, wrap_width, wrap, usize::MAX)
}

/// [`wrapped_rows`] as the fit count walks them (`0x6bd390`), stopping at `cap` as the reference's
/// fit walk lays out only `min(needed, fits + 1)` lines: an overflow test passes `allowed + 1`,
/// the chat band's real count [`usize::MAX`]. Word wrap off walks [`one_line_fit_rows`].
fn wrapped_rows_capped(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    wrap: TextWrap,
    cap: usize,
) -> usize {
    let (lines, icons) = fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]);
    if !wrap.word_wrap {
        let icons = icon_sizes(e, r, &icons);
        return one_line_fit_rows(e, r, &lines, wrap_width, wrap, &icons, cap);
    }
    if wrap_width <= WRAP_MIN_WIDTH {
        return lines.len().min(cap);
    }
    let icons = icon_sizes(e, r, &icons);
    // An indented string's rows after the first are narrower (`0x6bd46d`).
    let rest = wrap_width - indent_of(e, wrap);
    let mut rows = 0usize;
    for line in &lines {
        let first = if rows == 0 { wrap_width } else { rest };
        rows += wrap_line_rows(e, r, line, first, rest, &icons).len();
        if rows >= cap {
            return cap;
        }
    }
    rows
}

/// The fit walk's row count for a string with word wrap off (`0x6bd390`): it draws one line, but
/// the walk lays the whole text out, each line cut at its last fitting character (`0x6c2103`).
/// After a cut the walk goes on only with the non-space wrap bit (`0x6c2124`) and otherwise stops
/// short (`0x6bd50b`), answered as `cap`, an overflow; a line ended by its `\n` or the text's end
/// goes on.
fn one_line_fit_rows(
    e: &TextEngine,
    r: Resolved,
    lines: &[Vec<ColorRun>],
    width: f32,
    wrap: TextWrap,
    icons: &[[f32; 2]],
    cap: usize,
) -> usize {
    if width <= WRAP_MIN_WIDTH {
        return lines.len().min(1);
    }
    let rest = width - indent_of(e, wrap);
    let measure = |t: &str| measure_line_width(e, r, r.step_extra, t, icons);
    let mut rows = 0usize;
    for line in lines {
        let mut text: String = line.iter().map(|run| run.text.as_str()).collect();
        loop {
            let w = if rows == 0 { width } else { rest };
            let kept = fitting_prefix(&text, w, measure);
            rows += 1;
            if rows >= cap {
                return cap;
            }
            if kept == text.len() {
                break;
            }
            if !wrap.non_space_wrap || kept == 0 {
                return cap;
            }
            // The next line starts past the whitespace at the cut.
            text = text[kept..].trim_start().to_string();
            if text.is_empty() {
                break;
            }
        }
    }
    rows
}

/// [`wrapped_rows`], for the tests.
#[cfg(test)]
pub(super) fn wrapped_rows_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
) -> usize {
    wrapped_rows(e, r, text, wrap_width, TextWrap::default())
}

/// [`wrapped_rows_capped`] under the wrap bits `wrap`, for the tests.
#[cfg(test)]
pub(super) fn wrapped_rows_capped_styled_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    wrap: TextWrap,
    cap: usize,
) -> usize {
    wrapped_rows_capped(e, r, text, wrap_width, wrap, cap)
}

/// [`wrapped_rows_capped`], for the tests.
#[cfg(test)]
pub(super) fn wrapped_rows_capped_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    cap: usize,
) -> usize {
    wrapped_rows_capped(e, r, text, wrap_width, TextWrap::default(), cap)
}

/// The display string under the reference's height-gated ellipsis truncate of the FontString:
/// wrapped past the box, the text backs off a char at a time behind `"..."` until it fits; `None`
/// draws it raw. The gate is `boxW > 0 && boxH > 0` (`maxLines` is not modelled), so an auto-height
/// rect never truncates. Region FontStrings only: edit boxes, message lines and world text never
/// reach in the reference either.
pub(crate) fn ellipsize_to_fit(
    atlas: &mut UiFontAtlas,
    region: benilla_ui::widget::RegionHandle,
    text: &str,
    rect: Rect,
    font: FontSpec,
) -> Option<String> {
    if rect.width() <= WRAP_MIN_WIDTH || rect.height() <= HEIGHT_LIMIT_MIN {
        return None;
    }
    let (box_w, box_h) = (rect.width(), rect.height());
    // The remembered answer ([`super::super::EllipsisMemo`], the reference's `[string+0xf8]`): the
    // paint runs every frame, and this is by far its costliest step.
    if let Some(hit) = atlas.ellipsis.get(region, text, box_w, box_h, &font) {
        return hit.clone();
    }
    let display = {
        let mut e = atlas.lock();
        let r = resolve(&mut e, &font);
        // Every candidate is a prefix of `text` plus "...", so these two cover the whole walk.
        e.ensure_metrics(r.face, r.ppem, text);
        e.ensure_metrics(r.face, r.ppem, "...");
        let e = &*e;
        // Overflow is decided one row past the box, the reference's bound (`wrapped_rows_capped`).
        // A gap between lines fits `N` where `N·S + (N-1)·gap` does: the pitch with one gap more.
        let (gap_h, pitch) = (box_h + font.spacing, r.size + font.spacing);
        let cap = overflow::lines_fitting(gap_h.max(pitch), pitch) + 1;
        overflow::ellipsize_in_box(text, gap_h, pitch, |candidate| {
            wrapped_rows_capped(e, r, candidate, box_w, font.wrap, cap)
        })
    };
    atlas
        .ellipsis
        .put(region, text, box_w, box_h, &font, display.clone());
    display
}

/// The EditBox overlays' line origin (x0 under `justifyH`, where caret offsets start), one-line
/// cell top and height, by [`super::layout_text_quads`]'s own step law and snap, and the row pitch,
/// height plus spacing (`0x964db0`).
pub(crate) fn line_origin(
    e: &mut TextEngine,
    drawn: &str,
    rect: Rect,
    justify: Justify,
    font: FontSpec,
) -> (f32, f32, f32, f32) {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, drawn);
    let e = &*e;
    let x0 = match justify.h {
        JustifyH::Left => rect.min.x,
        JustifyH::Center | JustifyH::Right => {
            let (lines, icons) = fontstring_lines(drawn, [1.0, 1.0, 1.0, 1.0]);
            let icons = icon_sizes(e, r, &icons);
            let w = lines.first().map_or(0.0, |line| {
                line.iter()
                    .map(|run| measure_line_width(e, r, r.step_extra, &run.text, &icons))
                    .sum()
            });
            if matches!(justify.h, JustifyH::Center) {
                rect.min.x + ((rect.width() - w) * 0.5).max(0.0)
            } else {
                rect.max.x - w
            }
        }
    };
    // The draw's one-line `v_offset` and snap, so the caret cell sits on the drawn glyphs.
    let top = if rect.height() > f32::EPSILON {
        let v_offset = match justify.v {
            JustifyV::Top => 0.0,
            JustifyV::Middle => (rect.height() - r.size) * 0.5,
            JustifyV::Bottom => rect.height() - r.size,
        };
        super::snap_block_top(rect.min.y + v_offset)
    } else {
        rect.min.y
    };
    (x0, top, r.size, r.size + font.spacing)
}

/// The EditBox advance table ([`benilla_ui::script::UiScript::set_editbox_advances`]): the drawn
/// width at each raw byte boundary of `text`, `len + 1` entries from 0; a char's inner bytes read
/// as its start. Measured over the drawn string ([`crate::ui_text::markup::visible_map`]), so
/// markup costs nothing; measuring the raw string puts the caret right of a shift-clicked link.
pub(crate) fn line_advances(e: &mut TextEngine, text: &str, font: FontSpec) -> Vec<f32> {
    let mut cum = vec![0.0f32; text.len() + 1];
    if text.is_empty() {
        return cum;
    }
    let (drawn, bounds) = crate::ui_text::markup::visible_map(text);
    if drawn.is_empty() {
        return cum; // pure markup draws nothing: every boundary sits at x = 0
    }
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, &drawn);
    let e = &*e;
    let dpi = e.dpi();
    let icons = icon_sizes(e, r, &icons_of(text));

    let mut written = vec![false; text.len() + 1];
    written[0] = true;
    // The render's device-px pen, so a caret boundary is the x the glyph before it was drawn at.
    let mut x_dev = 0.0f32;
    for (off, ch) in drawn.char_indices() {
        if let Some(k) = icon_index(ch) {
            x_dev += icons.get(k).map_or(0.0, |s| (s[0] * dpi).round());
        } else if let Some(c) = e.char_cell(r.face, r.ppem, ch) {
            for g in &c.glyphs {
                x_dev += client_step(g.advance, r.step_extra);
            }
        }
        // The character's end in drawn bytes, mapped to its raw boundary.
        let end = bounds[(off + ch.len_utf8()).min(drawn.len())];
        cum[end] = x_dev / dpi;
        written[end] = true;
    }
    // Unwritten slots (char interiors, markup bytes) carry the previous boundary's value.
    for i in 1..cum.len() {
        if !written[i] {
            cum[i] = cum[i - 1];
        }
    }
    cum
}

/// The multiline EditBox row table: the raw byte where each wrapped row of `text` starts at
/// `wrap_width`, by the render's own [`wrap_line`] pass, and the row pitch, the font em plus its
/// spacing. Never empty (`[0]` for empty text).
pub(crate) fn line_rows(
    e: &mut TextEngine,
    text: &str,
    wrap_width: f32,
    font: FontSpec,
) -> (Vec<usize>, f32) {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, text);
    let e = &*e;
    let mut rows = Vec::new();
    let mut base = 0usize; // byte offset of the current '\n' segment within `text`
    for seg in text.split('\n') {
        let (lines, icons) = parse_markup_icons(seg, [1.0, 1.0, 1.0, 1.0]);
        let icons = icon_sizes(e, r, &icons);
        let sub: Vec<Vec<ColorRun>> = if wrap_width > WRAP_MIN_WIDTH {
            lines
                .iter()
                .flat_map(|line| wrap_line(e, r, line, wrap_width, &icons))
                .collect()
        } else {
            lines
        };
        segment_row_starts(seg, &sub, base, &mut rows);
        base += seg.len() + 1; // + the '\n' itself (swallowed by the split)
    }
    if rows.is_empty() {
        rows.push(0);
    }
    (rows, r.size + font.spacing)
}

/// [`line_rows`]'s walk for one `\n` segment at `base`: each row starts past the previous row's
/// bytes and the whitespace its break dropped (none after a force-break); a blank segment still
/// takes a row.
fn segment_row_starts(seg: &str, sub: &[Vec<ColorRun>], base: usize, rows: &mut Vec<usize>) {
    if sub.is_empty() {
        rows.push(base);
        return;
    }
    // Rows hold drawn text, but a row start is a raw byte, the space the cursor and
    // [`line_advances`] use: walk drawn bytes and map each start back.
    let (drawn, bounds) = crate::ui_text::markup::visible_map(seg);
    let mut p = 0usize; // byte cursor within `drawn`
    for (j, line) in sub.iter().enumerate() {
        if j > 0 {
            while let Some(c) = drawn[p.min(drawn.len())..].chars().next() {
                if c.is_whitespace() {
                    p += c.len_utf8();
                } else {
                    break;
                }
            }
        }
        rows.push(base + bounds[p.min(drawn.len())]);
        p += line.iter().map(|r| r.text.len()).sum::<usize>();
    }
}

#[cfg(test)]
mod row_start_tests {
    use super::*;

    fn runs(texts: &[&str]) -> Vec<Vec<ColorRun>> {
        texts
            .iter()
            .map(|t| {
                vec![ColorRun {
                    text: (*t).to_string(),
                    color: [1.0; 4],
                    link: None,
                }]
            })
            .collect()
    }

    fn starts(seg: &str, sub: &[Vec<ColorRun>], base: usize) -> Vec<usize> {
        let mut out = Vec::new();
        segment_row_starts(seg, sub, base, &mut out);
        out
    }

    #[test]
    fn a_word_break_swallows_its_separator() {
        assert_eq!(starts("hello world", &runs(&["hello", "world"]), 0), [0, 6]);
    }

    #[test]
    fn a_force_broken_word_swallows_nothing() {
        assert_eq!(
            starts(
                "supercalifragilistic",
                &runs(&["supercali", "fragilistic"]),
                0
            ),
            [0, 9]
        );
    }

    #[test]
    fn a_double_space_separator_is_swallowed_whole() {
        assert_eq!(starts("one  two", &runs(&["one", "two"]), 0), [0, 5]);
    }

    #[test]
    fn inner_whitespace_stays_verbatim_inside_a_row() {
        assert_eq!(
            starts("a b  c d", &runs(&["a b  c", "d"]), 0),
            [0, 7],
            "only the BREAK's separator is dropped"
        );
    }

    #[test]
    fn a_blank_segment_still_occupies_a_row_and_base_offsets() {
        assert_eq!(starts("", &[], 12), [12]);
        assert_eq!(starts("hi there", &runs(&["hi", "there"]), 100), [100, 103]);
    }
}
