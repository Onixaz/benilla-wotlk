//! The language pass text runs through as it is set (`0x482110`): font strings (`0x483925`),
//! buttons, message frames, SimpleHTML, tooltips, chat bubbles (`0x56c801`) and overhead names
//! (`0x7e57ca`). It runs under every locale.
//!
//! - `|1first;second;` picks a Korean particle by the sound before it (`0x481a00`).
//! - `|2` writes the French "of" in front of the next word (`0x481c80`).
//! - `|3-N(word)` puts the word in case `N` (`0x481e90`) through [`crate::declension`].
//! - `|4singular:plural;` picks a form by the number before it (`0x481fc0`).

use std::borrow::Cow;

/// The script layer's plural index under the running locale's rule (`0x817d10`, set at
/// `0x40642a`): koKR, frFR, zhCN and zhTW take rule 1, ruRU rule 2, the rest rule 0.
pub fn plural_index(count: i32) -> usize {
    match crate::locale::index() {
        1 | 2 | 4 | 5 => usize::from(count > 1),
        8 => match (count % 100, count % 10) {
            (11..=14, _) => 2,
            (_, 1) => 0,
            (_, 2..=4) => 1,
            _ => 2,
        },
        _ => usize::from(count != 1),
    }
}

/// The language pass over `text`, borrowed when it holds no rule token. The first `|` before a
/// number from 1 to 4 runs that rule over the whole text, and the scan starts again on the result
/// until none is left; `||` is skipped.
pub fn process(text: &str) -> Cow<'_, str> {
    let mut text = Cow::Borrowed(text);
    while let Some(rule) = next_rule(text.as_bytes()) {
        let done = match rule {
            1 => rule1(&text),
            2 => rule2(&text),
            3 => rule3(&text),
            _ => rule4(&text),
        };
        // Deviation: 12340 scans again forever when a rule leaves its token, as `|01` reads as
        // rule 1 to the scan and is no `|1` to the rule.
        if done == *text {
            break;
        }
        text = Cow::Owned(done);
    }
    text
}

/// The first token's rule (`0x482110`): a `|` and the number its digits read.
fn next_rule(b: &[u8]) -> Option<i32> {
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'|' {
            i += 1;
            continue;
        }
        i += 1;
        match b.get(i) {
            Some(b'|') => i += 1,
            Some(d) if d.is_ascii_digit() => {
                let number = b[i..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .fold(0i32, |n, c| {
                        n.wrapping_mul(10).wrapping_add(i32::from(c - b'0'))
                    });
                if (1..=4).contains(&number) {
                    return Some(number);
                }
            }
            _ => {}
        }
    }
    None
}

/// [`process`] over an owned string, kept as it is when it holds no rule token.
pub fn process_owned(text: String) -> String {
    match process(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(done) => done,
    }
}

/// Whether a token's digit stands alone: one followed by another digit is copied as text.
fn lone(next: Option<&char>) -> bool {
    !next.is_some_and(char::is_ascii_digit)
}

/// Digits the `|1` chooser counts as ending in a consonant, bit `n` for `n` (`0x4819b9`).
const DIGIT_CODA: u16 = 0b1_1100_1011;
/// Latin letters the `|1` chooser counts as ending in a consonant, bit `n` for the `n`th letter
/// (`0x4819e5`).
const LETTER_CODA: u32 = 0b1_1011_1100_0000_0110;

/// The `|1` rule (`0x481a00`): `|1first;second;` becomes the first form after a consonant and the
/// second after a vowel, judged by the last digit, Latin letter or Hangul syllable copied before
/// it. The eight characters after `|c` and the one after any other `|` are not counted.
fn rule1(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut last = 0u32;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        i += 1;
        if ch != '|' {
            if ch.is_ascii_alphanumeric() || ('\u{ac00}'..='\u{d7a3}').contains(&ch) {
                last = u32::from(ch);
            }
            out.push(ch);
            continue;
        }
        let Some(&next) = c.get(i) else {
            out.push('|');
            break;
        };
        i += 1;
        if next != '1' || !lone(c.get(i)) {
            out.push('|');
            out.push(next);
            if next == 'c' || next == 'C' {
                let colour = (c.len() - i).min(8);
                out.extend(&c[i..i + colour]);
                i += colour;
            }
            continue;
        }
        // Each form runs through its `;`.
        let form_end = |from: usize| {
            c[from..]
                .iter()
                .position(|&x| x == ';')
                .map_or(c.len(), |p| from + p + 1)
        };
        let first = i;
        let second = form_end(first);
        i = form_end(second);
        let from = if takes_first(last, c.get(first).copied()) {
            first
        } else {
            second
        };
        out.extend(c[from..].iter().take_while(|&&x| x != ';'));
    }
    out
}

/// The `|1` chooser (`0x481960`): whether the sound before the token ends in a consonant.
fn takes_first(last: u32, first_form: Option<char>) -> bool {
    match last {
        0xac00..=0xd7a3 => {
            // A syllable ending in rieul takes the second form when the first opens with `으`
            // (`0x48197b`).
            if (last as i32 - 0xac08) % 28 == 0 && first_form == Some('\u{c73c}') {
                return false;
            }
            !(last - 0xac00).is_multiple_of(28)
        }
        0x30..=0x39 => DIGIT_CODA & (1 << (last - 0x30)) != 0,
        0x41..=0x5a => LETTER_CODA & (1 << (last - 0x41)) != 0,
        0x61..=0x7a => LETTER_CODA & (1 << (last - 0x61)) != 0,
        // Nothing counted yet (`0x4819fb`).
        _ => true,
    }
}

/// A vowel to the `|2` rule (`0x481bc0`): ASCII `aeiou` in either case and the accented Latin-1
/// vowels.
fn vowel(c: Option<&char>) -> bool {
    c.is_some_and(|&c| {
        matches!(
            c,
            'A' | 'E'
                | 'I'
                | 'O'
                | 'U'
                | 'a'
                | 'e'
                | 'i'
                | 'o'
                | 'u'
                | '\u{c0}'..='\u{c6}'
                | '\u{c8}'..='\u{cf}'
                | '\u{d2}'..='\u{d6}'
                | '\u{d8}'..='\u{dc}'
                | '\u{e0}'..='\u{e6}'
                | '\u{e8}'..='\u{ef}'
                | '\u{f2}'..='\u{f6}'
                | '\u{f8}'..='\u{fc}'
        )
    })
}

/// The `|2` rule (`0x481c80`): the spaces after the token drop, then the next word picks the
/// French "of": `L'`, `Le `, `La ` and `Les ` become `de l'`, `du `, `de la ` and `des `; a vowel,
/// `h`, or `y` before a vowel takes `d'` (`0x481e31`); anything else takes `de ` (`0x481e1c`).
/// The character after the token, or after any other `|`, is copied without a look.
fn rule2(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        i += 1;
        if ch != '|' {
            out.push(ch);
            continue;
        }
        if c.get(i) == Some(&'2') && lone(c.get(i + 1)) {
            i += 1;
            while c.get(i) == Some(&' ') {
                i += 1;
            }
            let starts = |s: &str| s.chars().enumerate().all(|(k, x)| c.get(i + k) == Some(&x));
            let (article, skip) = if starts("L'") {
                ("de l'", 2)
            } else if starts("Le ") {
                ("du ", 3)
            } else if starts("La ") {
                ("de la ", 3)
            } else if starts("Les ") {
                ("des ", 4)
            } else if vowel(c.get(i))
                || matches!(c.get(i), Some('h' | 'H'))
                || (matches!(c.get(i), Some('y' | 'Y')) && vowel(c.get(i + 1)))
            {
                ("d'", 0)
            } else {
                ("de ", 0)
            };
            out.push_str(article);
            i += skip;
        } else {
            out.push('|');
        }
        if let Some(&next) = c.get(i) {
            out.push(next);
            i += 1;
        }
    }
    out
}

/// The `|3` rule (`0x481e90`): `|3-N(word)` becomes the word in case `N`. Without `-N` the `3`
/// itself reads as the case. The word runs to `)` or the end, cut at 1023 bytes.
fn rule3(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'|' {
            i += 1;
            continue;
        }
        let token = i;
        i += 1;
        if b.get(i) != Some(&b'3') || b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            // Any other `|` takes the byte after it along.
            i += 1;
            continue;
        }
        if b.get(i + 1) == Some(&b'-') && b.get(i + 2).is_some_and(u8::is_ascii_digit) {
            i += 2;
        }
        let mut case = 0u32;
        while let Some(d) = b.get(i).filter(|d| d.is_ascii_digit()) {
            case = case.wrapping_mul(10).wrapping_add(u32::from(d - b'0'));
            i += 1;
        }
        if b.get(i) == Some(&b'(') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i] != b')' {
            i += 1;
        }
        let mut cut = i.min(start + 1023);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push_str(&text[copied..token]);
        out.push_str(&crate::declension::decline(&text[start..cut], case as i32));
        i += 1;
        copied = i.min(b.len());
    }
    out.push_str(&text[copied..]);
    out
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

    #[test]
    fn a_hangul_syllable_picks_by_its_final_consonant() {
        // 책 ends in a consonant, 사 in a vowel.
        assert_eq!(process("책|1이;가; 있다"), "책이 있다");
        assert_eq!(process("사|1이;가; 있다"), "사가 있다");
        // 길 ends in rieul: the second form before 으, the first before anything else.
        assert_eq!(process("길|1으로;로;"), "길로");
        assert_eq!(process("길|1을;를;"), "길을");
    }

    #[test]
    fn digits_and_latin_letters_pick_by_their_table() {
        assert_eq!(process("3|1이;가;"), "3이");
        assert_eq!(process("2|1이;가;"), "2가");
        assert_eq!(process("L|1이;가;"), "L이");
        assert_eq!(process("l|1이;가;"), "l이");
        assert_eq!(process("a|1이;가;"), "a가");
        // Punctuation is not counted: the `a` before it decides.
        assert_eq!(process("a! |1이;가;"), "a! 가");
    }

    #[test]
    fn nothing_counted_takes_the_first_form() {
        assert_eq!(process("|1이;가;"), "이");
        assert_eq!(process("! |1이;가;"), "! 이");
    }

    #[test]
    fn a_colour_and_an_escape_are_not_counted() {
        // The `0` ending the colour would take the first form; the `2` before it decides.
        assert_eq!(process("2|cff00ff00|r|1이;가;"), "2|cff00ff00|r가");
        // The character after another `|` is not counted either.
        assert_eq!(process("2|n|1이;가;"), "2|n가");
    }

    #[test]
    fn a_korean_form_runs_through_its_semicolon() {
        assert_eq!(process("책|1이;가"), "책이");
        assert_eq!(process("사|1이;가"), "사가");
        assert_eq!(process("사|1이"), "사");
    }

    #[test]
    fn the_four_french_articles_contract() {
        assert_eq!(process("Peau |2 L'arbre"), "Peau de l'arbre");
        assert_eq!(process("Peau |2 Le loup"), "Peau du loup");
        assert_eq!(process("Peau |2 La louve"), "Peau de la louve");
        assert_eq!(process("Peau |2 Les loups"), "Peau des loups");
        // Only a capital article contracts.
        assert_eq!(process("|2le loup"), "de le loup");
    }

    #[test]
    fn a_vowel_an_h_or_a_y_before_a_vowel_elides() {
        assert_eq!(process("|2 Elwynn"), "d'Elwynn");
        assert_eq!(process("|2 \u{c9}caille"), "d'\u{c9}caille");
        assert_eq!(process("|2 hache"), "d'hache");
        assert_eq!(process("|2 Hurlevent"), "d'Hurlevent");
        assert_eq!(process("|2 Yeux"), "d'Yeux");
        assert_eq!(process("|2 Ymiron"), "de Ymiron");
        assert_eq!(process("|2 Tanaris"), "de Tanaris");
        assert_eq!(process("|2"), "de ");
    }

    #[test]
    fn a_following_digit_cancels_a_token() {
        assert_eq!(process("|21 Le loup"), "|21 Le loup");
        assert_eq!(process("책|12"), "책|12");
        assert_eq!(process("|31(chat)"), "|31(chat)");
        assert_eq!(process("||2 Le loup"), "||2 Le loup");
    }

    #[test]
    fn a_case_token_outside_ru_ru_leaves_the_bare_word() {
        assert_eq!(process("|3-1(chat)"), "chat");
        assert_eq!(process("de |3(chat) noir"), "de chat noir");
        assert_eq!(process("|3-1(chat"), "chat");
        assert_eq!(
            process("a |3-2(|cffffffff|Hunit:1|h[chat]|h|r) b"),
            "a |cffffffff|Hunit:1|h[chat]|h|r b"
        );
    }

    #[test]
    fn every_rule_runs_until_no_token_is_left() {
        assert_eq!(
            process("2 |4ours:ours; |2 Le bois, |3-1(chat) et 책|1이;가;"),
            "2 ours du bois, chat et 책이"
        );
        // A token that reads as a rule only to the scan stops the pass.
        assert_eq!(process("|01 |2 Le loup"), "|01 |2 Le loup");
        assert!(matches!(process("|5 |0 ||1"), Cow::Borrowed(_)));
    }
}
