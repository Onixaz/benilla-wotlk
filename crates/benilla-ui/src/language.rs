//! The grammar pass a font string runs over text as it is set (its text setter `0x483910` with its
//! process flag, into the language pass `0x482110`): `|4singular:plural;` picks a form by the
//! number before it. The enUS strings carry no `|1`..`|3` (Korean particles, French elision,
//! Russian declension), which pass through unchanged.

use std::borrow::Cow;

/// The script layer's plural index (`0x817d10`) under enUS's rule 0 (`0x40642a`): 1 for any count
/// but one.
pub fn plural_index(count: i32) -> usize {
    usize::from(count != 1)
}

/// The language pass over `text`, borrowed when it holds no `|4`.
pub fn process(text: &str) -> Cow<'_, str> {
    if !text.as_bytes().windows(2).any(|w| w == b"|4") {
        return Cow::Borrowed(text);
    }
    Cow::Owned(rule4(text))
}

/// [`process`] over an owned string, kept as it is when it holds no `|4`.
pub fn process_owned(text: String) -> String {
    match process(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(done) => done,
    }
}

/// The `|4` rule (`0x481fc0`): each `|4` not followed by a digit becomes the form its count picks,
/// the count being the last digit run anywhere before it. Leading spaces of the form are skipped;
/// the text from the form's end to the `;` is dropped. Any other `|` and the byte after it are
/// copied as they are, so `||4` stays escaped.
fn rule4(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut number: i32 = 0;
    let mut in_digits = false;
    let mut i = 0;
    while i < b.len() {
        let ch = b[i];
        let digit = ch.is_ascii_digit();
        if digit {
            let d = i32::from(ch - b'0');
            number = if in_digits {
                number.wrapping_mul(10).wrapping_add(d)
            } else {
                d
            };
        }
        if ch != b'|' {
            out.push(ch);
            i += 1;
            in_digits = digit;
            continue;
        }
        i += 1;
        let opens = b.get(i) == Some(&b'4') && !b.get(i + 1).is_some_and(u8::is_ascii_digit);
        if !opens {
            out.push(b'|');
            if let Some(&next) = b.get(i) {
                out.push(next);
                i += 1;
            }
            in_digits = false;
            continue;
        }
        i += 1;
        // Skip to the chosen form: one `:` per index, stopping at a `;` or the end.
        for _ in 0..plural_index(number) {
            while i < b.len() && b[i] != b':' && b[i] != b';' {
                i += 1;
            }
            if i >= b.len() || b[i] == b';' {
                break;
            }
            i += 1;
        }
        while b.get(i) == Some(&b' ') {
            i += 1;
        }
        while i < b.len() && b[i] != b':' && b[i] != b';' {
            out.push(b[i]);
            i += 1;
        }
        while i < b.len() && b[i] != b';' {
            i += 1;
        }
        // Past the `;`; an unterminated escape ends the text (`0x4820aa`, `0x4820ce`).
        i += 1;
        in_digits = false;
    }
    // Every cut lands on an ASCII byte, so the bytes stay UTF-8.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_before_the_escape_picks_the_form() {
        assert_eq!(process("12 |4hat:hats; left"), "12 hats left");
        assert_eq!(process("1 |4hat:hats; left"), "1 hat left");
        assert_eq!(process("0 |4hat:hats;"), "0 hats");
    }

    #[test]
    fn the_count_is_the_last_digit_run_even_across_words() {
        assert_eq!(
            process("21 apples and a |4box:boxes;"),
            "21 apples and a boxes"
        );
        assert_eq!(process("2 or 1 |4x:y;"), "2 or 1 x");
        // No digit before reads a count of 0.
        assert_eq!(process("|4one:many;"), "many");
    }

    #[test]
    fn a_missing_form_falls_back_and_spaces_lead_off() {
        // Only a singular: the plural walk stops on the `;` and prints nothing (`0x48206c`).
        assert_eq!(process("3 |4only;"), "3 ");
        assert_eq!(process("1 |4only;"), "1 only");
        assert_eq!(process("3 |4a:  b;"), "3 b");
        assert_eq!(process("3 |4a:b:c; end"), "3 b end");
    }

    #[test]
    fn other_escapes_and_digit_suffixes_pass_through() {
        // A colour's digits are digits too: `002` is the count.
        assert_eq!(process("|cff00ff002|r |4x:y;"), "|cff00ff002|r y");
        assert_eq!(process("2 ||4x:y;"), "2 ||4x:y;");
        assert_eq!(process("2 |41 stays"), "2 |41 stays");
        assert!(matches!(process("plain |cffffffff|r"), Cow::Borrowed(_)));
    }

    #[test]
    fn an_unterminated_escape_ends_the_text() {
        assert_eq!(process("2 |4x:y"), "2 y");
        assert_eq!(process("2 |4x"), "2 ");
        assert_eq!(process("trailing |"), "trailing |");
    }

    #[test]
    fn multibyte_text_survives() {
        assert_eq!(
            process("2 |4\u{e9}t\u{e9}:\u{e9}t\u{e9}s; \u{2014}"),
            "2 \u{e9}t\u{e9}s \u{2014}"
        );
    }
}
