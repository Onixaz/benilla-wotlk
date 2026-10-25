//! `macros-cache.txt`, 12340's grammar both ways: the account's in account-data slot 4, the
//! character's in slot 5. CRLF, no header:
//!
//! ```text
//! MACRO 16777217 "Heal" Spell_Holy_Heal
//! /cast Lesser Heal
//! END
//! ```
//!
//! - Write (`SaveMacrosCallback` `0x5668f0`): `MACRO %d "%s" %s` (id, name, icon), the body's
//!   non-empty lines, `END`. The character set's ids carry the base `0x1000000` (`0xa0f834`); the
//!   icon is the bare texture name under `Interface\Icons\`.
//! - Read (`LoadMacrosCallback` `0x567c10`): outside a macro a line must open with `MACRO`, any
//!   case, and name an id not 0 and not taken (one under the set's base gains it), a quoted name
//!   and an optional icon; inside one, a line that is `END`, any case, closes it and every other
//!   non-empty line is body. 36 per set at most; a malformed line is skipped.

use benilla_ui::script::MacroView;

/// The path prefix the client prepends to a stored icon basename (`0x84ca64`).
pub(super) const ICON_PREFIX: &str = "Interface\\Icons\\";

/// The id base of the character set (`s_macroIDBase` `0xa0f834`); the account set's is 0.
pub(super) const CHARACTER_BASE: u32 = 0x100_0000;

/// The reader's cap per set (`0x567c10`, `0x24`).
const READ_MAX: usize = 36;

/// The name's and the icon's buffers (`+0x20`, 64 bytes; `+0x60`, 256 bytes).
const NAME_MAX: usize = 63;
const ICON_MAX: usize = 255;

/// One scope's file as its macros, in id order.
pub(super) fn parse(text: &str, base: u32) -> Vec<MacroView> {
    let mut out: Vec<(u32, MacroView)> = Vec::new();
    let mut open: Option<(u32, String, Option<String>, Vec<&str>)> = None;
    for line in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        if let Some((id, name, icon, body)) = open.as_mut() {
            if line.eq_ignore_ascii_case("END") {
                let m = MacroView {
                    name: std::mem::take(name),
                    texture: icon.take().map(icon_path),
                    body: body.join("\n"),
                    local_only: false,
                };
                out.push((*id, m));
                open = None;
            } else {
                body.push(line);
            }
            continue;
        }
        if out.len() >= READ_MAX {
            continue;
        }
        let Some((id, name, icon)) = parse_header(line) else {
            continue;
        };
        let id = if id < base { id + base } else { id };
        if out.iter().any(|(i, _)| *i == id) {
            continue;
        }
        open = Some((id, name, icon, Vec::new()));
    }
    // A macro the file never closes keeps no body (the body is set at `END`).
    if let Some((id, name, icon, _)) = open {
        out.push((
            id,
            MacroView {
                name,
                texture: icon.map(icon_path),
                body: String::new(),
                local_only: false,
            },
        ));
    }
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, m)| m).collect()
}

/// A header line, `MACRO <id> "<name>" [icon]`: the keyword any case, an id that is not 0, a
/// non-empty quoted name cut to its buffer, the icon the next token.
fn parse_header(line: &str) -> Option<(u32, String, Option<String>)> {
    let line = line.trim_start();
    let (keyword, rest) = line.split_once(' ')?;
    if !keyword.eq_ignore_ascii_case("MACRO") {
        return None;
    }
    let rest = rest.trim_start();
    let (id, rest) = rest.split_once(' ')?;
    let id: u32 = id.parse().ok().filter(|&i| i != 0)?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let (name, rest) = rest.split_once('"')?;
    if name.is_empty() {
        return None;
    }
    let icon = rest.split_whitespace().next().map(|i| cap(i, ICON_MAX));
    Some((id, cap(name, NAME_MAX), icon))
}

fn cap(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// One scope's macros as its file, ids from `base + 1` in list order.
pub(super) fn write(macros: &[MacroView], base: u32) -> String {
    let mut out = String::new();
    for (i, m) in macros.iter().enumerate() {
        let icon = m.texture.as_deref().map(icon_token).unwrap_or_default();
        out.push_str(&format!(
            "MACRO {} \"{}\" {}\r\n",
            base + i as u32 + 1,
            m.name,
            icon
        ));
        for line in m.body.split(['\r', '\n']).filter(|l| !l.is_empty()) {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str("END\r\n");
    }
    out
}

/// A stored icon token to its texture path; a token with a path separator is taken whole.
fn icon_path(token: String) -> String {
    if token.contains('\\') || token.contains('/') {
        token
    } else {
        format!("{ICON_PREFIX}{token}")
    }
}

/// A texture path to its stored token: the basename in the icons folder, else the whole path.
fn icon_token(path: &str) -> &str {
    path.strip_prefix(ICON_PREFIX).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(name: &str, icon: Option<&str>, body: &str) -> MacroView {
        MacroView {
            name: name.into(),
            texture: icon.map(|i| format!("{ICON_PREFIX}{i}")),
            body: body.into(),
            local_only: false,
        }
    }

    #[test]
    fn the_writer_is_the_references_shape() {
        let m = vec![
            view(
                "f",
                Some("Ability_Ambush"),
                ".cheat fly on\n\n.modify aspeed 10",
            ),
            view("bare", None, ""),
        ];
        assert_eq!(
            write(&m, 0),
            "MACRO 1 \"f\" Ability_Ambush\r\n.cheat fly on\r\n.modify aspeed 10\r\nEND\r\n\
             MACRO 2 \"bare\" \r\nEND\r\n"
        );
        assert!(write(&m, CHARACTER_BASE).starts_with("MACRO 16777217 \"f\""));
        let back = parse(&write(&m, CHARACTER_BASE), CHARACTER_BASE);
        assert_eq!(
            back[0].body, ".cheat fly on\n.modify aspeed 10",
            "empty lines dropped"
        );
        assert_eq!(back[1], m[1]);
    }

    #[test]
    fn the_reader_orders_by_id_and_bases_a_small_character_id() {
        let text = "MACRO 3 \"ns\" Ability_BackStab\r\n/a\r\nEND\r\n\
                    macro 2 \"w\" Ability_Creature_Cursed_05\r\n/b\r\nend\r\n";
        let account = parse(text, 0);
        assert_eq!(
            account.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["w", "ns"]
        );
        // The character file's ids under the base gain it, and still sort.
        let character = parse(text, CHARACTER_BASE);
        assert_eq!(character[0].name, "w");
    }

    #[test]
    fn malformed_lines_lose_only_themselves() {
        let text = "garbage\r\nMACRO 0 \"zero\" X\r\nEND\r\nMACRO 5 \"\" X\r\nEND\r\n\
                    MACRO 7 \"good\" Ability_Ambush\r\n/say hi\r\nMACRO 8 \"inside\" X\r\nEND\r\n\
                    MACRO 7 \"dup\" X\r\nEND\r\nMACRO 9 \"open\" X\r\n/say bye\r\n";
        let m = parse(text, 0);
        assert_eq!(
            m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["good", "open"]
        );
        assert_eq!(
            m[0].body, "/say hi\nMACRO 8 \"inside\" X",
            "only END closes a macro"
        );
        assert_eq!(m[1].body, "", "an unclosed macro keeps no body");
    }

    #[test]
    fn a_full_path_icon_token_is_taken_whole() {
        let text = "MACRO 1 \"custom\" Interface\\Buttons\\UI-Panel-Button\r\nEND\r\n";
        let macros = parse(text, 0);
        assert_eq!(
            macros[0].texture.as_deref(),
            Some("Interface\\Buttons\\UI-Panel-Button")
        );
        assert_eq!(write(&macros, 0), text, "and writes back unchanged");
    }

    #[test]
    fn the_reader_stops_at_thirty_six() {
        let text: String = (1..=40)
            .map(|i| format!("MACRO {i} \"m{i}\" Ability_Ambush\r\nEND\r\n"))
            .collect();
        assert_eq!(parse(&text, 0).len(), 36);
    }
}
