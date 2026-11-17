//! A movie's captions: `<movie>.sbt` beside it in the archives, read as `0x95e880` reads it.
//!
//! The file is text, optionally behind a UTF-8 byte order mark, split into lines on CR and LF
//! with empty lines skipped. A line beginning `0` is a time line,
//! `HH:MM:SS:FF - HH:MM:SS:FF`, and the line after it is that caption's text. Each field is read
//! as a decimal number at its fixed column, and `FF` counts 41 ms (`imul 0x29` at `0x95e9b2`).

/// One caption: shown from `start_ms` until `end_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caption {
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
}

/// The number at column `at` of `line`: its leading decimal digits, 0 for none or past the end.
fn number_at(line: &[u8], at: usize) -> u32 {
    line.get(at..)
        .unwrap_or_default()
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .fold(0u32, |n, &c| {
            n.wrapping_mul(10).wrapping_add(u32::from(c - b'0'))
        })
}

/// `HH:MM:SS:FF` from column `at` in milliseconds.
fn time_at(line: &[u8], at: usize) -> u32 {
    let h = number_at(line, at);
    let m = number_at(line, at + 3);
    let s = number_at(line, at + 6);
    let f = number_at(line, at + 9);
    ((h.wrapping_mul(60).wrapping_add(m))
        .wrapping_mul(60)
        .wrapping_add(s))
    .wrapping_mul(1000)
    .wrapping_add(f.wrapping_mul(41))
}

/// Every caption of an `.sbt` file, in file order.
pub fn parse(bytes: &[u8]) -> Vec<Caption> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut lines = bytes
        .split(|&c| c == b'\r' || c == b'\n')
        .filter(|l| !l.is_empty());
    let mut out = Vec::new();
    while let Some(line) = lines.next() {
        if line[0] != b'0' {
            continue;
        }
        let text = lines.next().unwrap_or_default();
        out.push(Caption {
            start_ms: time_at(line, 0),
            end_ms: time_at(line, 14),
            text: String::from_utf8_lossy(text).into_owned(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_time_lines_and_the_text_after_each() {
        let file = b"\xef\xbb\xbf00:00:14:00 - 00:00:16:00\r\nFirst line\r\n\r\n\
                     00:01:02:50 - 01:00:00:01\r\nSecond, with \xc3\xa9\r\n\r\n\
                     not a time line\r\nignored\r\n";
        assert_eq!(
            parse(file),
            vec![
                Caption {
                    start_ms: 14_000,
                    end_ms: 16_000,
                    text: "First line".into(),
                },
                Caption {
                    start_ms: 62_000 + 50 * 41,
                    end_ms: 3_600_000 + 41,
                    text: "Second, with é".into(),
                },
            ]
        );
    }

    #[test]
    fn a_trailing_time_line_has_empty_text_and_short_lines_read_zeros() {
        assert_eq!(
            parse(b"00:00:01:00\n"),
            vec![Caption {
                start_ms: 1000,
                end_ms: 0,
                text: String::new(),
            }]
        );
        assert!(parse(b"").is_empty());
    }
}
