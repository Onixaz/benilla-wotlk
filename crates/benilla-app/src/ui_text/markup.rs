/// The `|H<link>|h<text>|h` hyperlink a run sits in: the payload (`item:2000:0:0:0`) and the
/// rebuilt markup, `OnHyperlinkClick`'s `arg2`. Every run of the link's text shares one, so spans
/// group by pointer identity.
#[derive(Debug, PartialEq)]
pub(crate) struct LinkInfo {
    pub(crate) link: String,
    pub(crate) markup: String,
}

/// The character an embedded texture stands in as inside a run's text: the `k`th icon of a string
/// is `ICON_BASE + k`, from Supplementary Private Use Area-A, which no client face carries, so the
/// wrap, the pen and the caret walk an icon as one character.
const ICON_BASE: u32 = 0xF0000;

/// One past the last icon character.
const ICON_END: u32 = 0xFFFFE;

/// The stand-in character of the `k`th icon, `None` past the range.
fn icon_char(k: usize) -> Option<char> {
    u32::try_from(k)
        .ok()
        .and_then(|k| ICON_BASE.checked_add(k))
        .filter(|&c| c < ICON_END)
        .and_then(char::from_u32)
}

/// The icon index a run character stands for, `None` for an ordinary character.
pub(super) fn icon_index(c: char) -> Option<usize> {
    let c = u32::from(c);
    (ICON_BASE..ICON_END)
        .contains(&c)
        .then(|| (c - ICON_BASE) as usize)
}

/// One `|T…|t` embedded texture of a string, in token order: its path and its spec's geometry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InlineIcon {
    pub(crate) path: String,
    pub(crate) geometry: benilla_ui::markup::TextureGeometry,
}

/// The `|T` span's icon, `None` when the spec parser refuses it: the token then draws nothing.
fn inline_icon(spec: &str) -> Option<InlineIcon> {
    benilla_ui::markup::TextureSpec::parse(spec).map(|s| InlineIcon {
        path: s.path.to_string(),
        geometry: s.geometry,
    })
}

/// `input`'s embedded textures in token order, the table its icon characters index; empty, and no
/// walk, for a string with no `|T`.
pub(super) fn icons_of(input: &str) -> Vec<InlineIcon> {
    if !input.contains("|T") {
        return Vec::new();
    }
    benilla_ui::markup::tokens(input)
        .filter_map(|(_, t)| match t.kind {
            benilla_ui::markup::TokenKind::Texture { spec } => inline_icon(spec),
            _ => None,
        })
        .collect()
}

/// One colour run of a line: its text, its straight-alpha RGBA in 0..1 (as
/// [`crate::ui_pass::UiQuad::color`]), and its link. An embedded texture is one character of the
/// text ([`icon_index`]).
#[derive(Clone)]
pub(super) struct ColorRun {
    pub(super) text: String,
    pub(super) color: [f32; 4],
    pub(super) link: Option<std::sync::Arc<LinkInfo>>,
}

/// [`parse_markup_icons`]'s lines alone, for the tests.
#[cfg(test)]
pub(super) fn parse_markup(input: &str, base_color: [f32; 4]) -> Vec<Vec<ColorRun>> {
    parse_markup_icons(input, base_color).0
}

/// Resolve `input`'s inline markup, whose grammar is [`benilla_ui::markup`]'s, into lines of
/// [`ColorRun`]s and the string's [`InlineIcon`]s, which its icon characters index. `|n` breaks the
/// line in every box. The reference skips it in a single-line box (`K & 0x200`, set by
/// `SetMultiLine`), though its cursor model still parses it; no stock string carries `|n`, and a
/// typed `|` becomes `||`.
pub(super) fn parse_markup_icons(
    input: &str,
    base_color: [f32; 4],
) -> (Vec<Vec<ColorRun>>, Vec<InlineIcon>) {
    use benilla_ui::markup::TokenKind as T;

    let mut icons: Vec<InlineIcon> = Vec::new();
    let mut lines: Vec<Vec<ColorRun>> = Vec::new();
    let mut runs: Vec<ColorRun> = Vec::new();
    let mut color = base_color;
    let mut cur = String::new();
    // The open hyperlink: payload, visible text, and the runs flushed under it, which get the
    // `Arc` built at the closing `|h`.
    let mut link: Option<(String, String, Vec<usize>)> = None;

    for (_, token) in benilla_ui::markup::tokens(input) {
        match token.kind {
            T::Color(rgba) => {
                flush(&mut runs, &mut cur, color, &mut link);
                // At the string's alpha: the escape's `AA` is discarded and the emitter patches in
                // the FontString's own, so a fading link fades.
                color = rgba.to_f32_at(base_color[3]);
            }
            T::ColorReset => {
                flush(&mut runs, &mut cur, color, &mut link);
                color = base_color;
            }
            T::LineBreak => {
                flush(&mut runs, &mut cur, color, &mut link);
                lines.push(std::mem::take(&mut runs));
            }
            T::EscapedPipe => push_visible('|', &mut cur, &mut link),
            T::LinkOpen { payload } => {
                flush(&mut runs, &mut cur, color, &mut link);
                link = Some((payload.to_string(), String::new(), Vec::new()));
            }
            T::LinkClose => {
                flush(&mut runs, &mut cur, color, &mut link);
                // A close with no open is a stray token: nothing to back-patch, nothing drawn.
                if let Some((payload, visible, idxs)) = link.take() {
                    let info = std::sync::Arc::new(LinkInfo {
                        markup: format!("|H{payload}|h{visible}|h"),
                        link: payload,
                    });
                    for idx in idxs {
                        runs[idx].link = Some(info.clone());
                    }
                }
            }
            // A character in the icon range is not one the faces draw; dropped, it cannot pose
            // as an icon.
            T::Char(c) if icon_index(c).is_some() => {}
            T::Char(c) => push_visible(c, &mut cur, &mut link),
            T::Texture { spec } => {
                if let Some(ch) = inline_icon(spec).and_then(|i| {
                    let ch = icon_char(icons.len())?;
                    icons.push(i);
                    Some(ch)
                }) {
                    cur.push(ch);
                    if let Some((_, visible, _)) = &mut link {
                        visible.push_str(&format!("|T{spec}|t"));
                    }
                }
            }
            T::TextureClose => {}
        }
    }
    // An unterminated link's runs stay plain text: the `|H` is consumed, the text still shows.
    flush(&mut runs, &mut cur, color, &mut link);
    lines.push(runs);
    (lines, icons)
}

/// [`parse_markup_icons`] under the FontString line-count law of `GxuFont_GetTextBlockHeight`: a
/// line's terminating break is consumed with it, so a trailing break opens no line (a `<BR/>` block,
/// `"\n"`, is one blank line), and an empty string is zero lines of height 0. Not the EditBox's
/// law, hence a seam of its own: a multiline EditBox ending in a break shows the empty row its
/// caret sits on, through `line_rows`.
pub(super) fn fontstring_lines(
    input: &str,
    base_color: [f32; 4],
) -> (Vec<Vec<ColorRun>>, Vec<InlineIcon>) {
    if input.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let (mut lines, icons) = parse_markup_icons(input, base_color);
    if lines.len() > 1
        && lines
            .last()
            .is_some_and(|l| l.iter().all(|r| r.text.is_empty()))
    {
        lines.pop();
    }
    (lines, icons)
}

/// Accumulate one drawn char into the current run, and into the open link's visible text.
fn push_visible(c: char, cur: &mut String, link: &mut Option<(String, String, Vec<usize>)>) {
    if let Some((_, visible, _)) = link {
        visible.push(c);
    }
    cur.push(c);
}

fn flush(
    runs: &mut Vec<ColorRun>,
    cur: &mut String,
    color: [f32; 4],
    link: &mut Option<(String, String, Vec<usize>)>,
) {
    if !cur.is_empty() {
        if let Some((_, _, idxs)) = link {
            idxs.push(runs.len());
        }
        runs.push(ColorRun {
            text: std::mem::take(cur),
            color,
            link: None, // back-patched at the closing |h
        });
    }
}

/// `input`'s drawn text, as [`parse_markup_icons`] draws it, with the raw byte offset of every
/// boundary: `bounds[k]` is the raw offset before drawn byte `k`, and
/// `bounds.len() == drawn.len() + 1`. The EditBox stores the raw string and draws only its visible
/// text, so its advance table charges
/// escape bytes zero width: a glyph ending at drawn byte `e` files its width at raw `bounds[e]`,
/// just past the glyph and before any escape after it, and `||` draws one byte from two.
pub(super) fn visible_map(input: &str) -> (String, Vec<usize>) {
    use benilla_ui::markup::TokenKind as T;

    let mut drawn = String::new();
    let mut bounds: Vec<usize> = Vec::new();
    let mut last_end = 0usize;
    let mut icons = 0usize;
    for (at, token) in benilla_ui::markup::tokens(input) {
        let c = match token.kind {
            T::Char(c) if icon_index(c).is_some() => continue,
            T::Char(c) => c,
            T::EscapedPipe => '|',
            T::LineBreak => '\n',
            // An icon draws as its stand-in, numbered as [`parse_markup_icons`] numbers it.
            T::Texture { spec } => match inline_icon(spec).and_then(|_| icon_char(icons)) {
                Some(ch) => {
                    icons += 1;
                    ch
                }
                None => continue,
            },
            // Zero-width: no drawn byte, so no boundary of its own.
            T::Color(_) | T::ColorReset | T::LinkOpen { .. } | T::LinkClose | T::TextureClose => {
                continue
            }
        };
        // The boundary before this char is where the previous drawn token ended, before any escape
        // in between; a multi-byte char's interior boundaries are its own raw bytes.
        bounds.push(last_end);
        bounds.extend((1..c.len_utf8()).map(|k| at + k));
        last_end = at + token.byte_len;
        drawn.push(c);
    }
    bounds.push(last_end);
    (drawn, bounds)
}

#[cfg(test)]
mod markup_tests {
    use super::*;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    #[test]
    fn plain_text_is_one_run() {
        let lines = parse_markup("hello", WHITE);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "hello");
        assert_eq!(lines[0][0].color, WHITE);
    }

    #[test]
    fn color_escape_switches_and_resets() {
        let lines = parse_markup("a|cffff0000b|rc", WHITE);
        assert_eq!(lines[0].len(), 3);
        assert_eq!(lines[0][0].text, "a");
        assert_eq!(lines[0][0].color, WHITE);
        assert_eq!(lines[0][1].text, "b");
        assert_eq!(lines[0][1].color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(lines[0][2].text, "c");
        assert_eq!(lines[0][2].color, WHITE);
    }

    /// The string `ui_loot::receive_line` emits, on the line's LOOT green (0, 170, 0).
    #[test]
    fn a_loot_line_draws_its_item_name_in_the_quality_color_and_the_count_in_the_line_color() {
        const LOOT_GREEN: [f32; 4] = [0.0, 170.0 / 255.0, 0.0, 1.0];
        let lines = parse_markup(
            "You receive loot: |cff9d9d9d|Hitem:7092:0:0:0|h[Chipped Claw]|h|rx2.",
            LOOT_GREEN,
        );
        assert_eq!(lines[0].len(), 3);
        assert_eq!(lines[0][0].text, "You receive loot: ");
        assert_eq!(lines[0][0].color, LOOT_GREEN);
        assert_eq!(lines[0][1].text, "[Chipped Claw]");
        let grey = 0x9d as f32 / 255.0;
        assert_eq!(lines[0][1].color, [grey, grey, grey, 1.0]);
        assert_eq!(
            lines[0][1].link.as_ref().expect("linked run").link,
            "item:7092:0:0:0"
        );
        assert_eq!(lines[0][2].text, "x2.");
        assert_eq!(lines[0][2].color, LOOT_GREEN);
        assert!(lines[0][2].link.is_none());
    }

    #[test]
    fn newline_splits_lines() {
        let lines = parse_markup("one\ntwo", WHITE);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].text, "one");
        assert_eq!(lines[1][0].text, "two");
    }

    /// An embedded texture is one character of its run, standing for the string's icon table;
    /// the stock `OBJECTIVES_LABEL` reads `Objectives` and one icon.
    #[test]
    fn an_inline_texture_is_one_icon_character_of_its_run() {
        let (lines, icons) = parse_markup_icons(
            "Objectives|TInterface\\OptionsFrame\\UI-OptionsFrame-NewFeatureIcon:0:0:0:-1|t",
            WHITE,
        );
        assert_eq!(lines[0].len(), 1);
        let text: Vec<char> = lines[0][0].text.chars().collect();
        assert_eq!(text.len(), 11);
        assert_eq!(text[..10].iter().collect::<String>(), "Objectives");
        assert_eq!(icon_index(text[10]), Some(0));
        assert_eq!(icons.len(), 1);
        assert_eq!(
            icons[0].path,
            "Interface\\OptionsFrame\\UI-OptionsFrame-NewFeatureIcon"
        );
        assert_eq!(icons[0].geometry.offset, [0.0, -1.0]);
        // Icons number in token order.
        let (lines, icons) = parse_markup_icons("|Ta:0|t x |Tb:0|t", WHITE);
        let ks: Vec<Option<usize>> = lines[0][0].text.chars().map(icon_index).collect();
        assert_eq!(ks, [Some(0), None, None, None, Some(1)]);
        assert_eq!(icons[1].path, "b");
        assert_eq!(icons_of("|Ta:0|t x |Tb:0|t"), icons);
    }

    /// A span the spec parser refuses and a stray `|t` draw nothing; a span the decoder refuses
    /// draws as text.
    #[test]
    fn a_refused_texture_draws_nothing_and_an_undecoded_one_draws_as_text() {
        let (lines, icons) = parse_markup_icons("a|Tfoo|tb|tc", WHITE);
        assert_eq!(lines[0][0].text, "abc");
        assert!(icons.is_empty());
        assert_eq!(parse_markup("a|Tfoo", WHITE)[0][0].text, "a|Tfoo");
        // A character in the icon range from the input cannot pose as an icon.
        assert_eq!(parse_markup("a\u{F0000}b", WHITE)[0][0].text, "ab");
    }

    #[test]
    fn hyperlink_runs_carry_the_link_and_strip_the_markers() {
        let lines = parse_markup("|cff1eff00|Hitem:2000:0:0:0|h[Another Helm]|h|r ok", WHITE);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[0][0].text, "[Another Helm]");
        assert_eq!(lines[0][0].color, [0x1e as f32 / 255.0, 1.0, 0.0, 1.0]);
        let info = lines[0][0].link.as_ref().expect("linked run");
        assert_eq!(info.link, "item:2000:0:0:0");
        assert_eq!(info.markup, "|Hitem:2000:0:0:0|h[Another Helm]|h");
        assert_eq!(lines[0][1].text, " ok");
        assert!(lines[0][1].link.is_none());
        assert_eq!(lines[0][1].color, WHITE);
    }

    #[test]
    fn color_change_inside_a_link_still_shares_one_link() {
        let lines = parse_markup("|Hplayer:Bob|h[|cffff0000Bob|r]|h", WHITE);
        assert_eq!(lines[0].len(), 3);
        let first = lines[0][0].link.as_ref().expect("linked");
        for run in &lines[0] {
            let l = run.link.as_ref().expect("all runs linked");
            assert!(std::sync::Arc::ptr_eq(first, l));
        }
        assert_eq!(first.link, "player:Bob");
        assert_eq!(first.markup, "|Hplayer:Bob|h[Bob]|h");
    }

    // ── visible_map ──

    /// Checks `visible_map` against `parse_markup` and the boundary map's invariants.
    fn check_map(raw: &str, expect_drawn: &str) -> Vec<usize> {
        let (drawn, bounds) = visible_map(raw);
        assert_eq!(drawn, expect_drawn, "drawn text of {raw:?}");
        let from_runs: String = parse_markup(raw, WHITE)
            .iter()
            .map(|l| l.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(drawn, from_runs, "visible_map agrees with parse_markup");
        assert_eq!(
            bounds.len(),
            drawn.len() + 1,
            "one boundary per drawn byte, plus the end"
        );
        for w in bounds.windows(2) {
            assert!(w[0] <= w[1], "monotonic: {bounds:?}");
        }
        for &b in &bounds {
            assert!(
                raw.is_char_boundary(b),
                "boundary {b} of {raw:?} is mid-char"
            );
        }
        bounds
    }

    /// A shift-clicked item link in the chat edit box: the last drawn glyph's boundary is just past
    /// the `]`, with `|h|r` still to come, so an end caret lands on the drawn width.
    #[test]
    fn an_item_link_maps_its_drawn_boundaries_onto_the_raw_buffer() {
        let raw = "|cffa335ee|Hitem:13984:0:0:0|h[The Plague Bearer]|h|r";
        let bounds = check_map(raw, "[The Plague Bearer]");
        // Before the leading escapes, where the reachable cursor set puts it (`|c`/`|H` absorb
        // forward).
        assert_eq!(bounds[0], 0);
        // The `]` glyph ends at drawn byte 19; its raw boundary is just past the `]`.
        let end = bounds[19];
        assert_eq!(&raw[end - 1..end], "]");
        assert_eq!(&raw[end..], "|h|r");
    }

    /// An escape between two visible chars: the boundary after `b` is 2, not 12.
    #[test]
    fn a_boundary_stops_before_a_following_escape() {
        assert_eq!(check_map("ab|cffff0000cd", "abcd"), vec![0, 1, 2, 13, 14]);
    }

    /// Text typed after a link sits past its `|r`, so its advances follow the drawn name.
    #[test]
    fn text_after_a_links_reset_maps_past_the_escape() {
        let raw = "|cffa335ee|Hitem:13984:0:0:0|h[The Plague Bearer]|h|rds";
        let bounds = check_map(raw, "[The Plague Bearer]ds");
        assert_eq!(&raw[bounds[19]..], "|h|rds");
        assert_eq!(bounds[bounds.len() - 1], raw.len());
    }

    #[test]
    fn visible_map_handles_plain_text_and_newlines() {
        assert_eq!(check_map("hello", "hello"), vec![0, 1, 2, 3, 4, 5]);
        // A newline draws as its own byte, and the map keeps crossing it.
        assert_eq!(
            check_map("ab\n|cffff0000cd|r", "ab\ncd"),
            vec![0, 1, 2, 3, 14, 15]
        );
        // A string that draws nothing has its one boundary at the start.
        assert_eq!(visible_map("|cffff0000|r"), (String::new(), vec![0]));
        assert_eq!(visible_map(""), (String::new(), vec![0]));
    }

    /// `||` draws one `|` from two raw bytes, `|n` and `\r\n` break the line, and `|T…|t` draws as
    /// one icon character.
    #[test]
    fn the_tokens_the_engine_grammar_added() {
        // `a||b`: the drawn `|` spans raw 1..3, so the boundary after it is 3.
        assert_eq!(check_map("a||b", "a|b"), vec![0, 1, 3, 4]);
        assert_eq!(parse_markup("a||b", WHITE)[0][0].text, "a|b");
        assert_eq!(parse_markup("a|nb", WHITE).len(), 2);
        assert_eq!(parse_markup("a\r\nb", WHITE).len(), 2);
        assert_eq!(check_map("a\r\nb", "a\nb"), vec![0, 1, 3, 4]);
        assert_eq!(check_map("a|Tfoo|tb", "ab"), vec![0, 1, 9]);
        let icon = icon_char(0).expect("the first icon");
        let bounds = check_map("a|Tfoo:0|tb", &format!("a{icon}b"));
        // The icon's four drawn bytes map to its whole raw span.
        assert_eq!(bounds, vec![0, 1, 2, 3, 4, 10, 11]);
    }

    /// The line-count law where it differs from a split, and where it does not.
    #[test]
    fn a_trailing_break_does_not_open_a_line() {
        let n = |t: &str| fontstring_lines(t, [1.0, 1.0, 1.0, 1.0]).0.len();
        assert_eq!(n("\n"), 1, "a <BR/> block is ONE blank line");
        assert_eq!(n("a\n"), 1);
        // The item tooltip's set spacer, the reference's literal.
        assert_eq!(n(" \n"), 1, "the set's blank gold spacer is one row");
        assert_eq!(n(""), 0, "an empty string is zero lines, height 0.0");
        // As a split counts them.
        assert_eq!(n("a"), 1);
        assert_eq!(n("a\nb"), 2);
        assert_eq!(
            n("\nbody\n"),
            2,
            "the reader's own padding: a blank line, then the body"
        );
        assert_eq!(n("\n\n"), 2);
        assert_eq!(n("a\n\nb"), 3);
        // `|n` is the same break token.
        assert_eq!(n("a|n"), 1);
        assert_eq!(n("a|nb"), 2);
    }

    #[test]
    fn a_malformed_escape_stays_visible_in_the_map() {
        check_map("a|cffzzb", "a|cffzzb");
        // A well-formed open with no close: no span, and the text still draws.
        check_map("|Hitem:1|h[Broken", "[Broken");
    }

    #[test]
    fn unterminated_link_degrades_to_plain_text() {
        let lines = parse_markup("|Hitem:1|h[Broken", WHITE);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "[Broken");
        assert!(lines[0][0].link.is_none());
    }
}
