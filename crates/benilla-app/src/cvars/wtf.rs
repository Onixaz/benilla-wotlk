//! The `.wtf` CVar file, 12340's grammar both ways: `Config.wtf`, the account and character
//! `config-cache.wtf`, `RunOnce*.wtf` and `realmlist.wtf` all hold `SET name "value"` lines.
//!
//! - Write (`0x766640`): `SET %s "%s"` and a bare LF per row, the value raw, no escaping and no
//!   BOM.
//! - Read (`0x766400`): a UTF-8 BOM skipped, lines split at CR or LF, blank ones skipped, and only
//!   a line opening `SET ` (any case) runs; the `SET` handler (`0x7681f0`) tokenizes name and value
//!   on ` ,;\t"\r\n`, a value that opens with `"` running to the next `"`, so no value can hold a
//!   quote.

/// Longest name and value the tokenizer keeps (`0x7681f0`'s 64- and 2048-byte buffers).
const NAME_MAX: usize = 63;
const VALUE_MAX: usize = 2047;

/// The token delimiters of `SET` (`0xa38544`).
fn delimiter(c: char) -> bool {
    matches!(c, ' ' | ',' | ';' | '\t' | '"' | '\r' | '\n')
}

/// One token off the front of `s`, quote-aware, and what is left after it.
fn token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start_matches(|c: char| delimiter(c) && c != '"');
    if let Some(quoted) = s.strip_prefix('"') {
        let end = quoted.find('"').unwrap_or(quoted.len());
        let rest = quoted.get(end + 1..).unwrap_or("");
        return Some((&quoted[..end], rest));
    }
    if s.is_empty() {
        return None;
    }
    let end = s.find(delimiter).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

/// Cut `s` to at most `max` bytes, on a character boundary.
fn capped(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Every `SET` in `text`, in file order; a later line for a name the file already set is kept as
/// its own entry, as each runs in turn. A `SET` with no value sets nothing.
pub(crate) fn parse(text: &str) -> Vec<(String, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    for line in text.split(['\r', '\n']) {
        let Some(head) = line.get(..4) else {
            continue;
        };
        if !head.eq_ignore_ascii_case("SET ") {
            continue;
        }
        let Some((name, rest)) = token(&line[4..]) else {
            continue;
        };
        let Some((value, _)) = token(rest) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        out.push((
            capped(name, NAME_MAX).to_string(),
            capped(value, VALUE_MAX).to_string(),
        ));
    }
    out
}

/// One row as the reference's CVar save writes it.
pub(crate) fn line(name: &str, value: &str) -> String {
    format!("SET {name} \"{value}\"\n")
}

/// The rows, in order, as a file.
pub(crate) fn compose<'a>(rows: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    rows.into_iter().map(|(n, v)| line(n, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(text: &str) -> Vec<(String, String)> {
        parse(text)
    }

    #[test]
    fn a_saved_file_reads_back_row_for_row() {
        let rows = [
            ("locale", "enUS"),
            ("Sound_OutputDriverName", "System Default"),
            ("gxResolution", "1920x1080"),
            ("emptyOne", ""),
        ];
        let text = compose(rows);
        assert_eq!(
            text,
            "SET locale \"enUS\"\nSET Sound_OutputDriverName \"System Default\"\n\
             SET gxResolution \"1920x1080\"\nSET emptyOne \"\"\n"
        );
        let back = pairs(&text);
        assert_eq!(
            back.iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
            rows
        );
    }

    #[test]
    fn the_reader_takes_what_the_set_command_takes() {
        let text = "\u{feff}set realmlist 192.168.1.89\r\n\r\nSET a \"x, y\"\r\nSETX b 1\n\
                    # comment\nSET c 4;5\nSET d\nSET e\t\"tabbed\"";
        assert_eq!(
            pairs(text),
            [
                ("realmlist".into(), "192.168.1.89".into()),
                ("a".into(), "x, y".into()),
                ("c".into(), "4".into()),
                ("e".into(), "tabbed".into()),
            ]
        );
    }

    #[test]
    fn raw_bytes_in_a_value_survive_both_ways() {
        let tutorials = "v##M##\u{1}##%";
        let text = line("flaggedTutorials", tutorials);
        assert_eq!(
            pairs(&text),
            [("flaggedTutorials".into(), tutorials.into())]
        );
    }
}
