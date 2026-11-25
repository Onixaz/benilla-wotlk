//! `SendChatMessage`'s line passes past its tutorial flag, in 12340's order (`0x50d6ac`-`0x50d7ae`):
//! the unit tokens, the drunk slur, the newline cut and the escape walk.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

/// The two line buffers the passes write, 256 bytes with their NUL (`0x50d69f`, `0x50d6d9`).
const LINE_BUFFER: usize = 0x100;

/// The walk's Lua error (`0x50d7b1`).
pub(super) const INVALID_ESCAPE: &str = "SendChatMessage(): Invalid escape code in chat message";

/// The line `SendChatMessage` sends for `text`, or `None` for the escape walk's Lua error. Bytes
/// throughout: a 255-byte cut can end inside a UTF-8 sequence, and 12340 sends it so.
pub(super) fn sent_line(world: &mut World, vm: &UiScript, text: &[u8]) -> Option<Vec<u8>> {
    // The tokens (`0x50d6ac` -> `0x577d00`).
    let parsed = {
        let world: &World = world;
        let focus = vm.focus_guid();
        let target = world
            .get_resource::<crate::target::Selection>()
            .and_then(|s| s.guid)
            .unwrap_or(0);
        parse_tokens(text, |token| match token {
            Token::Focus => token_name(world, vm, focus, "FOCUS_TOKEN_NOT_FOUND"),
            Token::Target => token_name(world, vm, target, "TARGET_TOKEN_NOT_FOUND"),
        })
    };
    // The drunk slur, while the drunk test passes (`0x50d6c5` -> `0x4ffd80`, `0x50d6e8`).
    let mut line = match self_drunkness(world) {
        Some(drunk) => {
            let slurred = combat_string(vm, "SLURRED_SPEECH");
            let mut filter = world.get_resource_mut::<crate::text_filter::TextFilter>();
            slur(
                &parsed,
                drunk,
                rand::random::<f32>,
                // The word test forces the profanity filter on (`0x578124` -> `0x509a50`).
                |word| filter.as_mut().is_some_and(|f| f.mask(true, word)),
                |copy| super::super::away::fill_one_bytes(slurred.as_bytes(), copy),
            )
        }
        None => parsed,
    };
    // The newline cut (`0x50d6f7`), then the escape walk (`0x50d6ff`-`0x50d7ae`).
    line.truncate(benilla_ui::script::new_line_cut(&line));
    benilla_ui::script::chat_escapes_valid(&mut line).then_some(line)
}

/// A unit token the parser names (`0x577d91`, cases at `0x577d9f` and `0x577e46`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// `%f` and `%F`: the focus.
    Focus,
    /// `%t`, `%T`, `%n` and `%N`: the target.
    Target,
}

/// The parse (`0x577d00`): each `%` and the byte after it read as a token; a known one packs its
/// unit's name, any other packs the `%` alone and the byte stays text (`0x577ed1`). Packed into the
/// 256-byte buffer.
fn parse_tokens(src: &[u8], mut name: impl FnMut(Token) -> String) -> Vec<u8> {
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(at) = src[start..]
        .iter()
        .position(|&b| b == b'%')
        .map(|p| start + p)
    {
        pack(&mut out, &src[start..at]);
        start = match src.get(at + 1) {
            Some(b'F' | b'f') => {
                pack(&mut out, name(Token::Focus).as_bytes());
                at + 2
            }
            Some(b'N' | b'T' | b'n' | b't') => {
                pack(&mut out, name(Token::Target).as_bytes());
                at + 2
            }
            _ => {
                pack(&mut out, b"%");
                at + 1
            }
        };
    }
    pack(&mut out, &src[start..]);
    out
}

/// The buffer append (`0x76ef70`): what fits under the buffer's 255 bytes.
fn pack(out: &mut Vec<u8>, src: &[u8]) {
    let room = (LINE_BUFFER - 1).saturating_sub(out.len());
    out.extend_from_slice(&src[..src.len().min(room)]);
}

/// A token's name (`0x577dbf`-`0x577e2b`): a unit in view reads its own name, one out of view the
/// player name cache's record with no query, and no guid or no record the global `missing`.
fn token_name(world: &World, vm: &UiScript, guid: u64, missing: &str) -> String {
    let entity = (guid != 0)
        .then(|| world.get_resource::<crate::net::GuidIndex>())
        .flatten()
        .and_then(|i| i.0.get(&guid).copied());
    let unit = entity.filter(|&e| {
        world
            .get::<crate::net::ObjectStore>(e)
            .is_some_and(crate::net::ObjectStore::is_unit)
    });
    if let Some(unit) = unit {
        return unit_name(world, vm, unit, guid, true);
    }
    let names = world.get_resource::<crate::names::NameCache>();
    (guid != 0 && benilla_protocol::guid::is_player(guid))
        .then(|| names.and_then(|n| n.peek(guid)))
        .flatten()
        .map(str::to_string)
        .unwrap_or_else(|| combat_string(vm, missing))
}

/// A unit's name (`0x72a000`): with `follow`, the first aura in slot order whose applied effect is
/// aura 279 names its caster instead (`0x72a043`-`0x72a0a9`), from the caster's object with no
/// follow (`0x72a16c`) or, out of view, by guid (`0x72a18a`); else the unit's own name, asked on a
/// miss, and `UNKNOWNOBJECT` meanwhile.
fn unit_name(world: &World, vm: &UiScript, unit: Entity, guid: u64, follow: bool) -> String {
    let spells = world.get_resource::<crate::ui_action::Spells>();
    let caster = follow
        .then(|| world.get::<crate::unit_auras::UnitAuras>(unit))
        .flatten()
        .and_then(|auras| {
            image_caster(auras, |id| {
                spells
                    .and_then(|s| s.catalog.get(id))
                    .map(|d| d.effect_apply_aura)
            })
        });
    let (guid, store) = match caster {
        Some(caster) => {
            let in_view = world
                .get_resource::<crate::net::GuidIndex>()
                .and_then(|i| i.0.get(&caster).copied())
                .filter(|&e| {
                    world
                        .get::<crate::net::ObjectStore>(e)
                        .is_some_and(crate::net::ObjectStore::is_unit)
                });
            if let Some(e) = in_view {
                return unit_name(world, vm, e, caster, false);
            }
            (caster, None)
        }
        None => (guid, world.get::<crate::net::ObjectStore>(unit)),
    };
    world
        .get_resource::<crate::names::NameCache>()
        .zip(world.get_resource::<crate::net::NetCommands>())
        .and_then(|(names, net)| super::super::combat::object_name(guid, store, names, net))
        .unwrap_or_else(|| combat_string(vm, "UNKNOWNOBJECT"))
}

/// The caster of the first aura in slot order whose applied effect is aura 279; a spell with no
/// record walks on (`0x72a087`-`0x72a0cc`).
fn image_caster(
    auras: &crate::unit_auras::UnitAuras,
    effect_auras: impl Fn(u32) -> Option<[u32; 3]>,
) -> Option<u64> {
    auras
        .iter()
        .find(|a| {
            effect_auras(a.spell_id).is_some_and(|effects| {
                (0..3).any(|i| effects[i] == AURA_INITIALIZE_IMAGES && a.flags & (1 << i) != 0)
            })
        })
        .map(|a| a.caster)
}

/// `SPELL_AURA` 279, whose holder is named as the aura's caster (`0x72a092`).
const AURA_INITIALIZE_IMAGES: u32 = 279;

/// The global string `key` off the calling VM; "" when unset.
fn combat_string(vm: &UiScript, key: &str) -> String {
    super::super::combat::global_string(vm, key).unwrap_or_default()
}

/// The active player's drunk fraction while the drunk test passes, the drunk byte or the fake
/// level nonzero (`0x4ffd80`): the larger of the two, capped at 100, as a byte times 0.01
/// (`0x4f7290`).
fn self_drunkness(world: &mut World) -> Option<f32> {
    let (byte, fake) = world
        .query_filtered::<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>()
        .iter(world)
        .next()
        .map(|s| {
            (
                s.0.player_drunk_byte().unwrap_or(0),
                s.0.player_fake_inebriation(),
            )
        })?;
    if byte == 0 && fake == 0 {
        return None;
    }
    let level = i32::from(byte).max(fake).min(100) as u8;
    Some((f64::from(level) * f64::from(DRUNK_SCALE)) as f32)
}

/// 0.01, the drunk fraction's scale (`0x4f72bc`).
const DRUNK_SCALE: f32 = f32::from_bits(0x3c23_d70a);

/// The slur (`0x577f60`), into a 256-byte buffer: a word-start `s` or `S` not followed by a space
/// or another `s` takes an `h` on a draw under `drunk` unless `sh` and the word's rest is profane
/// (`0x5780a2`-`0x57814a`); a word-final one takes it on a draw (`0x57815c`-`0x578189`); a `|c` run
/// to its `|r` copies whole (`0x577fcb`-`0x57800b`). Then, on a draw under a quarter of `drunk`,
/// the line becomes `SLURRED_SPEECH` over it (`0x578020`-`0x578088`).
fn slur(
    text: &[u8],
    drunk: f32,
    mut draw: impl FnMut() -> f32,
    mut profane: impl FnMut(&mut String) -> bool,
    slurred: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(LINE_BUFFER);
    let mut remaining = LINE_BUFFER;
    let mut i = 0;
    let mut word_start = true;
    while i < text.len() && remaining > 1 {
        let c = text[i];
        out.push(c);
        remaining -= 1;
        i += 1;
        // The NUL past the line's end reads as a byte that is no space and no `s`.
        let next = text.get(i).copied().unwrap_or(0);
        match c {
            b' ' => {
                word_start = true;
                continue;
            }
            b'S' | b's' if word_start => {
                if remaining > 1 && !matches!(next, b' ' | b'S' | b's') && draw() < drunk {
                    let rest = text[i..].split(|&b| b == b' ').next().unwrap_or(&[]);
                    let mut word = String::from("sh");
                    word.push_str(&String::from_utf8_lossy(rest));
                    if !profane(&mut word) {
                        out.push(b'h');
                        remaining -= 1;
                    }
                }
            }
            b'S' | b's' => {
                if remaining > 1 && next == b' ' && draw() < drunk {
                    out.push(b'h');
                    remaining -= 1;
                }
            }
            b'|' if next == b'c' => {
                if let Some(end) = text[i..].windows(2).position(|w| w == b"|r") {
                    let end = i + end;
                    let length = (end - i + 2).min(remaining - 1);
                    out.extend_from_slice(&text[i..i + length]);
                    remaining -= length;
                    i = end + 2;
                }
            }
            _ => {}
        }
        word_start = false;
    }
    if drunk * 0.25 > draw() {
        let mut line = slurred(&out);
        line.truncate(LINE_BUFFER - 1);
        return line;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(token: Token) -> String {
        match token {
            Token::Focus => "Fay".into(),
            Token::Target => "Tom".into(),
        }
    }

    /// Each known token packs its unit's name; any other `%` packs itself and leaves the next byte
    /// as text, so `%%t` is a `%` and the target (`0x577ed1`, `0x577ee4`).
    #[test]
    fn the_parse_names_the_tokens_and_keeps_other_percents() {
        let parse = |t: &str| String::from_utf8(parse_tokens(t.as_bytes(), names)).unwrap();
        assert_eq!(parse("hi %t and %F"), "hi Tom and Fay");
        assert_eq!(parse("%N%n%T%f"), "TomTomTomFay");
        assert_eq!(parse("50% off %x"), "50% off %x");
        assert_eq!(parse("%%t"), "%Tom");
        assert_eq!(parse("end%"), "end%");
        assert_eq!(parse("plain"), "plain");
    }

    /// The parse packs into 256 bytes, 255 of text (`0x577d65`).
    #[test]
    fn the_parse_caps_the_line_at_255_bytes() {
        let long = "a".repeat(300);
        assert_eq!(parse_tokens(long.as_bytes(), names).len(), 255);
        let near = format!("{}%t", "b".repeat(253));
        assert_eq!(
            String::from_utf8(parse_tokens(near.as_bytes(), names)).unwrap(),
            format!("{}To", "b".repeat(253))
        );
    }

    /// The 255-byte cut (`0x577d65`) lands inside a two-byte character and keeps its lead byte:
    /// the line stays bytes to the wire.
    #[test]
    fn the_cut_keeps_a_partial_character() {
        let text = format!("{}\u{e9}", "a".repeat(254));
        let out = parse_tokens(text.as_bytes(), names);
        assert_eq!(out.len(), 255);
        assert_eq!(out[254], 0xC3);
        assert!(String::from_utf8(out).is_err());
    }

    /// A word-start `s` takes its `h` on a draw under the level unless the word is profane; one
    /// before a space takes it too; one followed by a space or an `s` at a word start does not
    /// draw; a colour run copies whole.
    #[test]
    fn the_slur_follows_the_draws() {
        let run = |text: &str, draws: &[f32], profane: bool| {
            let mut draws = draws.iter().copied();
            String::from_utf8(slur(
                text.as_bytes(),
                0.5,
                || draws.next().unwrap_or(1.0),
                |_| profane,
                |c| [c, b" ...hic!"].concat(),
            ))
            .unwrap()
        };
        // `sit` draws 0.1 (h), `yes` draws 0.1 before the space (h), the end draws 1.0 (no slur).
        assert_eq!(run("sit yes ok", &[0.1, 0.1, 1.0], false), "shit yesh ok");
        assert_eq!(
            run("sit", &[0.1, 1.0], true),
            "sit",
            "a profane word keeps its s"
        );
        assert_eq!(run("ss so", &[0.1, 0.1, 1.0], false), "ssh sho");
        assert_eq!(
            run("s", &[0.1, 1.0], false),
            "sh",
            "the line's end is no space"
        );
        assert_eq!(run("sit", &[0.9, 1.0], false), "sit");
        assert_eq!(
            run("|cffffffffs s|r s", &[0.1, 1.0], false),
            "|cffffffffs s|r sh"
        );
        // The last draw under a quarter of the level wraps the line.
        assert_eq!(run("ok", &[0.1], false), "ok ...hic!");
    }

    /// The first aura in slot order with an applied aura-279 effect names its caster; an unapplied
    /// effect bit, another aura or a spell with no record does not.
    #[test]
    fn the_image_caster_is_the_first_applied_279_in_slot_order() {
        use crate::unit_auras::UnitAuras;
        let effects = |id: u32| match id {
            100 => Some([0, AURA_INITIALIZE_IMAGES, 0]),
            101 => Some([AURA_INITIALIZE_IMAGES, 0, 0]),
            200 => Some([AURA_INITIALIZE_IMAGES - 1, 0, 0]),
            _ => None,
        };
        assert_eq!(
            image_caster(&UnitAuras::of(7, &[(0, 100, 0x1)]), effects),
            None
        );
        assert_eq!(
            image_caster(&UnitAuras::of(7, &[(0, 200, 0x1)]), effects),
            None
        );
        assert_eq!(
            image_caster(&UnitAuras::of(9, &[(4, 100, 0x2)]), effects),
            Some(9)
        );
        let ordered = UnitAuras::of_casters(&[(5, 101, 0x50), (2, 101, 0x20), (1, 300, 0x10)]);
        assert_eq!(image_caster(&ordered, effects), Some(0x20));
    }

    /// The slur writes into 256 bytes, so a full line takes no `h` past 255.
    #[test]
    fn the_slur_stops_at_the_buffer() {
        let text = format!("{} s", "a".repeat(253));
        let out = slur(text.as_bytes(), 1.0, || 0.0, |_| false, |c| c.into());
        assert_eq!(out.len(), 255);
    }
}
