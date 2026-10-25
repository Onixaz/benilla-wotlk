//! `bindings-cache.wtf`, 12340's grammar both ways, the account set in account-data slot 2 and the
//! character's in slot 3. CRLF, no header:
//!
//! ```text
//! BINDINGMODE 0
//! bind F JUMP
//! bind SPACE NONE
//! modifiedclick CTRL SELFCAST
//! ```
//!
//! - Write (`CGUIBindings::SaveBindings` `0x55f1b0`): a `BINDINGMODE` line before a mode's first
//!   key, then `bind <key> <command>` for each key the set holds off the defaults, `NONE` for a
//!   default key it unbinds; then `modifiedclick <binding> <action>` for each action off its
//!   default.
//! - Read (`0x5641c0`): the defaults first, then line by line, `BINDINGMODE n` (n <= 3) switching
//!   the mode, `bind` taking a key token of at most 31 bytes and the rest of the line as its
//!   command, `modifiedclick` the same; any other line is ignored.
//!
//! benilla binds mode 0 alone, the one the stock UI uses; a key in another mode is read past.

use benilla_ui::script::keybind::KeyBinding;

/// A key bound to nothing (`Bind` `0x562ed0` stores the literal).
const NONE: &str = "NONE";

/// The reader's key buffer, 32 bytes with the terminator (`0x5641c0`).
const KEY_MAX: usize = 31;

/// The set as its file: the keys off the defaults, in the set's order, then the defaults' keys the
/// set unbinds; then the modified clicks off theirs, `(action, binding)`.
pub(crate) fn encode(
    set: &[KeyBinding],
    defaults: &[KeyBinding],
    clicks: &[(String, String)],
) -> String {
    let default_of = |key: &str| defaults.iter().find(|(k, _)| k == key).map(|(_, c)| c);
    let mut binds: Vec<(&str, &str)> = set
        .iter()
        .filter(|(k, c)| default_of(k).is_none_or(|d| !d.eq_ignore_ascii_case(c)))
        .map(|(k, c)| (k.as_str(), c.as_str()))
        .collect();
    binds.extend(
        defaults
            .iter()
            .filter(|(k, _)| !set.iter().any(|(s, _)| s == k))
            .map(|(k, _)| (k.as_str(), NONE)),
    );
    let mut out = String::new();
    if !binds.is_empty() {
        out.push_str("BINDINGMODE 0\r\n");
        for (key, command) in binds {
            out.push_str(&format!("bind {key} {command}\r\n"));
        }
    }
    for (action, binding) in clicks {
        out.push_str(&format!("modifiedclick {binding} {action}\r\n"));
    }
    out
}

/// A file back into its set over the defaults, and its modified clicks as `(action, binding)`.
pub(crate) fn decode(
    text: &str,
    defaults: &[KeyBinding],
) -> (Vec<KeyBinding>, Vec<(String, String)>) {
    let mut set = defaults.to_vec();
    let mut clicks = Vec::new();
    let mut mode = 0u32;
    for line in text.split(['\r', '\n']) {
        let line = line.trim_start_matches([' ', '\t']);
        if let Some(n) = strip_ci(line, "BINDINGMODE ") {
            if let Ok(n) = n.trim().parse::<u32>() {
                if n <= 3 {
                    mode = n;
                }
            }
        } else if let Some(rest) = strip_ci(line, "bind ") {
            let Some((key, command)) = split_token(rest) else {
                continue;
            };
            if mode != 0 {
                continue;
            }
            set.retain(|(k, _)| k != key);
            if !command.eq_ignore_ascii_case(NONE) {
                set.push((key.to_string(), command.to_string()));
            }
        } else if let Some(rest) = strip_ci(line, "modifiedclick ") {
            if let Some((binding, action)) = split_token(rest) {
                clicks.push((action.to_string(), binding.to_string()));
            }
        }
    }
    (set, clicks)
}

fn strip_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let head = line.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &line[prefix.len()..])
}

/// A space-ended token of at most [`KEY_MAX`] bytes, then the rest of the line, both non-empty.
fn split_token(rest: &str) -> Option<(&str, &str)> {
    let (token, tail) = rest.split_once(' ')?;
    let tail = tail.trim();
    (!token.is_empty() && token.len() <= KEY_MAX && !tail.is_empty()).then_some((token, tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(pairs: &[(&str, &str)]) -> Vec<KeyBinding> {
        pairs
            .iter()
            .map(|(k, c)| (k.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn the_defaults_write_nothing() {
        let d = kb(&[("W", "MOVEFORWARD"), ("SPACE", "JUMP")]);
        assert_eq!(encode(&d, &d, &[]), "");
    }

    #[test]
    fn a_rebind_and_an_unbind_round_trip_in_the_references_lines() {
        let d = kb(&[
            ("W", "MOVEFORWARD"),
            ("SPACE", "JUMP"),
            ("Z", "TOGGLESHEATH"),
        ]);
        let set = kb(&[
            ("W", "MOVEFORWARD"),
            ("F", "JUMP"),
            ("SPACE", "TOGGLEAUTORUN"),
        ]);
        let clicks = vec![("SELFCAST".to_string(), "CTRL".to_string())];
        let text = encode(&set, &d, &clicks);
        assert_eq!(
            text,
            "BINDINGMODE 0\r\nbind F JUMP\r\nbind SPACE TOGGLEAUTORUN\r\nbind Z NONE\r\n\
             modifiedclick CTRL SELFCAST\r\n"
        );
        let (back, back_clicks) = decode(&text, &d);
        let mut back = back;
        back.sort();
        let mut want = set;
        want.sort();
        assert_eq!(back, want);
        assert_eq!(back_clicks, clicks);
    }

    #[test]
    fn the_reader_takes_what_the_reference_takes() {
        let d = kb(&[("SPACE", "JUMP")]);
        let text =
            "  bind F JUMP\r\nBINDINGMODE 9\r\njunk\r\nbind TOOLONGKEYNAMEXXXXXXXXXXXXXXXXXX X\r\n\
                    BINDINGMODE 1\r\nbind G MOVEBACKWARD\r\nbindingmode 0\r\nBIND H SITORSTAND\r\n";
        let (set, _) = decode(text, &d);
        assert!(set.contains(&("F".into(), "JUMP".into())));
        assert!(
            set.contains(&("H".into(), "SITORSTAND".into())),
            "case-blind keywords"
        );
        assert!(!set.iter().any(|(k, _)| k == "G"), "mode 1 is not ours");
        assert!(
            !set.iter().any(|(_, c)| c == "X"),
            "a key past 31 bytes is refused"
        );
        assert!(
            set.contains(&("SPACE".into(), "JUMP".into())),
            "the defaults stand"
        );
    }
}
