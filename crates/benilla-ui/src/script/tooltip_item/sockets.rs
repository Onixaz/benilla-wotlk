//! The enchant and socket lines of an item object or a link block (`0x6277f0`): the twelve
//! enchant slots in order, an empty socket where a slot 2..=4 the item has holds no gem, then the
//! socket bonus, green when every gem fits its socket.

use mlua::{Lua, Table};

use crate::script::tooltip::{add_texture, append_line};
use crate::script::ItemTemplateView;
use crate::strings::{fill, Arg};

use super::names::*;
use super::render::{charges_phrase, enchant_time_left, ItemInstance};

/// The enchant slots: 0 permanent, 1 temporary, 2..=4 sockets, 5 the socket bonus, 6 prismatic,
/// 7..=11 the random properties.
const ENCHANT_SLOTS: u8 = 12;
const PRISMATIC_SLOT: u8 = 6;

/// Whether a gem of `gem_type` fits a socket of `socket_color` (`0x5c48d0`): an uncoloured socket
/// takes anything, a coloured one any gem sharing one of its four colour bits.
fn gem_matches_socket(socket_color: u32, gem_type: Option<u32>) -> bool {
    socket_color == 0 || gem_type.is_some_and(|t| t & socket_color & 0xf != 0)
}

/// The colour names a condition test reads, by operand type (`0x61c0b0`); 0 has none.
const GEM_COLOR_TOKENS: [&str; 5] = ["", "META_GEM", "RED_GEM", "YELLOW_GEM", "BLUE_GEM"];

/// The condition text (`0x61c0b0`): one `"  Requires …"` line per test, white when it
/// holds and gray when not, each after a line break when `newline` is set or a line is already
/// out; and whether every test held. Counts are meta, red, yellow, blue.
pub(super) fn condition_text(
    cond: &crate::script::EnchantConditionView,
    counts: [i32; 4],
    newline: bool,
    get: &dyn Fn(&str) -> Option<String>,
) -> (String, bool) {
    let key = |k: &str| get(k).unwrap_or_default();
    let name = |ty: u8| match GEM_COLOR_TOKENS.get(usize::from(ty)) {
        Some(token) if !token.is_empty() => key(token),
        Some(_) => String::new(),
        None => key("UNKNOWN"),
    };
    let count = |ty: u8| match ty {
        1..=4 => counts[usize::from(ty) - 1],
        _ => 0,
    };
    let mut out = String::new();
    let mut all_met = true;
    for t in cond {
        if newline || !out.is_empty() {
            out.push('\n');
        }
        let (left, left_name) = (count(t.left_type), name(t.left_type));
        let (text, met) = if t.right_type != 0 {
            let (right, right_name) = (count(t.right_type), name(t.right_type));
            let pair = |k: &str, a: &str, b: &str| fill(&key(k), &[Arg::S(a), Arg::S(b)]);
            match t.operator {
                0 => (
                    pair("ENCHANT_CONDITION_EQUAL_COMPARE", &left_name, &right_name),
                    left == right,
                ),
                1 => (
                    pair(
                        "ENCHANT_CONDITION_NOT_EQUAL_COMPARE",
                        &left_name,
                        &right_name,
                    ),
                    left != right,
                ),
                2 => (
                    pair("ENCHANT_CONDITION_MORE_COMPARE", &right_name, &left_name),
                    right > left,
                ),
                3 => (
                    pair("ENCHANT_CONDITION_MORE_COMPARE", &left_name, &right_name),
                    left > right,
                ),
                4 => (
                    pair(
                        "ENCHANT_CONDITION_MORE_EQUAL_COMPARE",
                        &right_name,
                        &left_name,
                    ),
                    right >= left,
                ),
                5 => (
                    pair(
                        "ENCHANT_CONDITION_MORE_EQUAL_COMPARE",
                        &left_name,
                        &right_name,
                    ),
                    left >= right,
                ),
                _ => (String::new(), true),
            }
        } else {
            let value =
                |k: &str, v: i32| fill(&key(k), &[Arg::D(i64::from(v)), Arg::S(&left_name)]);
            let v = t.right_value;
            match t.operator {
                0 => (value("ENCHANT_CONDITION_EQUAL_VALUE", v), left == v),
                1 => (value("ENCHANT_CONDITION_NOT_EQUAL_VALUE", v), left != v),
                2 => (value("ENCHANT_CONDITION_LESS_VALUE", v), left < v),
                // The value prints one higher, and the test is that count reached.
                3 => (value("ENCHANT_CONDITION_MORE_VALUE", v + 1), left > v),
                4 => (value("ENCHANT_CONDITION_LESS_VALUE", v + 1), left < v + 1),
                5 => (value("ENCHANT_CONDITION_MORE_VALUE", v), left >= v),
                _ => (String::new(), true),
            }
        };
        let color = if met { "|cffffffff" } else { "|cff808080" };
        out.push_str(&format!(
            "  {color}{}{text}|r",
            key("ENCHANT_CONDITION_REQUIRES")
        ));
        all_met &= met;
        if t.logic == 0 {
            break;
        }
    }
    (out, all_met)
}

/// An empty socket: its `EMPTY_SOCKET_*` line and icon, or the colourless pair for a prismatic one.
pub(super) fn empty_socket(
    lua: &Lua,
    this: &Table,
    color: Option<&str>,
    line_color: [f32; 4],
    get: &dyn Fn(&str) -> Option<String>,
) -> mlua::Result<()> {
    let (key, icon) = match color {
        Some(name) => (
            format!("EMPTY_SOCKET_{}", name.to_uppercase()),
            format!("Interface\\ItemSocketingFrame\\UI-EmptySocket-{name}.blp"),
        ),
        None => (
            "EMPTY_SOCKET_NO_COLOR".to_string(),
            "Interface\\ItemSocketingFrame\\UI-EmptySocket.blp".to_string(),
        ),
    };
    let Some(text) = get(&key) else {
        return Ok(());
    };
    append_line(lua, this, (text, line_color), None, false)?;
    add_texture(lua, this, Some(icon), None, WHITE)
}

/// The slot walk over an instance's enchants. Not built: a gem's icon and the enchant and
/// prismatic skill and level requirements.
pub(super) fn socket_lines(
    lua: &Lua,
    this: &Table,
    v: &ItemTemplateView,
    inst: &ItemInstance,
    get: &dyn Fn(&str) -> Option<String>,
) -> mlua::Result<()> {
    // The worn gems a meta gem's condition counts.
    let counts = crate::script::socket::equipped_gem_counts(&crate::script::model_mut_of(lua));
    // A signable item's ids read as zero.
    let enchants = match v.flags & 0x2000 != 0 {
        true => &[][..],
        false => inst.enchants.as_slice(),
    };
    // The template's coloured sockets, and those a prismatic enchant adds (`0x7094e0`).
    let prismatic = enchants
        .iter()
        .find(|e| e.slot == PRISMATIC_SLOT)
        .map_or(0, |e| e.prismatic_sockets as usize);
    let sockets = v.socket_colors.iter().filter(|&&c| c != 0).count() + prismatic;
    let mut all_match = true;
    let preview = inst.socket_preview.as_ref();
    for slot in 0..ENCHANT_SLOTS {
        let socket = slot.checked_sub(2).filter(|&s| s <= 2).map(usize::from);
        let current = enchants.iter().find(|e| e.slot == slot);
        // The socket preview (`0x6277f0`): a placed gem's enchant stands in its socket, and a
        // socket left empty prints nothing.
        let swapped;
        let shown = match (preview, socket) {
            (Some(placed), Some(s)) => match &placed[s] {
                Some(gem) => {
                    if !gem_matches_socket(v.socket_colors[s], gem.gem.as_ref().map(|g| g.gem_type))
                    {
                        all_match = false;
                    }
                    match &gem.gem {
                        Some(g) => {
                            swapped = crate::script::EnchantView {
                                slot,
                                id: g.enchant,
                                gem_type: Some(g.gem_type),
                                name: g.enchant_name.clone().unwrap_or_default(),
                                condition: g.condition,
                                ..Default::default()
                            };
                            Some(&swapped)
                        }
                        None => current,
                    }
                }
                None => {
                    if current.is_none() && v.socket_colors[s] != 0 {
                        all_match = false;
                    }
                    current
                }
            },
            _ => current,
        };
        let Some(e) = shown else {
            if preview.is_some() {
                continue;
            }
            let Some(socket) = socket.filter(|&s| s < sockets) else {
                continue;
            };
            match socket_color_name(v.socket_colors[socket]) {
                Some(name) => {
                    all_match = false;
                    empty_socket(lua, this, Some(name), GRAY, get)?;
                }
                None => empty_socket(lua, this, None, GRAY, get)?,
            }
            continue;
        };
        // The prismatic enchant and the socket bonus's own enchant print no line, nor does a
        // nameless one, which the line core drops.
        if slot == PRISMATIC_SLOT || (v.socket_bonus != 0 && e.id == v.socket_bonus) {
            continue;
        }
        if e.name.is_empty() {
            continue;
        }
        if let Some(socket) = socket {
            if !gem_matches_socket(v.socket_colors[socket], e.gem_type) {
                all_match = false;
            }
        }
        // Slots 0, 1 and 5 are green, or bright red for a negative id; the rest are white.
        let color = match (matches!(slot, 0 | 1 | 5), e.negative) {
            (true, false) => GREEN,
            (true, true) => ENCHANT_RED,
            (false, _) => WHITE,
        };
        // A temporary enchant's countdown replaces its name, in the same line and colour; the
        // time is `SMSG_ITEM_ENCHANT_TIME_UPDATE`'s.
        let mut text = match e.remaining_ms {
            // A missing template skips the slot, not falling back to the bare name: the
            // reference formats an empty string, which `AddLine` drops.
            Some(ms) => match enchant_time_left(&e.name, ms, get) {
                Some(t) => t,
                None => continue,
            },
            None => e.name.clone(),
        };
        // The slot's charges: ITEM_SPELL_CHARGES inside the literal `" (%s)"`.
        if let Some(charges) = charges_phrase(e.charges, get) {
            text.push_str(&format!(" ({charges})"));
        }
        // A gem's line carries its own colour, white while its condition holds and gray when not,
        // its condition lines appended (`0x629ce5`). Not built: the red for an unmet enchant skill
        // or level requirement.
        if socket.is_some() {
            let (cond, met) = e.condition.as_ref().map_or((String::new(), true), |c| {
                condition_text(c, counts, true, get)
            });
            let code = if met { "|cffffffff" } else { "|cff808080" };
            append_line(
                lua,
                this,
                (format!("{code}{text}|r{cond}"), GOLD),
                None,
                false,
            )?;
            continue;
        }
        append_line(lua, this, (text, color), None, false)?;
    }
    if let (Some(bonus), Some(t)) = (&v.socket_bonus_name, get("ITEM_SOCKET_BONUS")) {
        let color = if all_match { GREEN } else { GRAY };
        append_line(lua, this, (fill(&t, &[Arg::S(bonus)]), color), None, false)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{condition_text, gem_matches_socket};
    use crate::script::EnchantConditionTest;

    fn strings(key: &str) -> Option<String> {
        Some(
            match key {
                "ENCHANT_CONDITION_REQUIRES" => "Requires ",
                "ENCHANT_CONDITION_MORE_VALUE" => "at least %d %s gems",
                "ENCHANT_CONDITION_MORE_COMPARE" => "more %s gems than %s gems",
                "RED_GEM" => "Red",
                "YELLOW_GEM" => "Yellow",
                "BLUE_GEM" => "Blue",
                _ => return None,
            }
            .to_string(),
        )
    }

    /// A count test past its value (op 3 adds one) and a colour compare, white when held and gray
    /// when not, every line led by a break when `newline` is set.
    #[test]
    fn a_condition_reads_the_equipped_counts() {
        let test = |left_type, operator, right_type, right_value, logic| EnchantConditionTest {
            left_type,
            operator,
            right_type,
            right_value,
            logic,
        };
        let cond = [
            test(2, 3, 0, 1, 1),
            test(4, 3, 3, 0, 0),
            test(3, 3, 0, 9, 0),
            test(0, 0, 0, 0, 0),
            test(0, 0, 0, 0, 0),
        ];
        // Meta, red, yellow, blue: 2 red, 1 yellow, 0 blue.
        let (text, met) = condition_text(&cond, [0, 2, 1, 0], true, &strings);
        assert_eq!(
            text,
            "
  |cffffffffRequires at least 2 Red gems|r
  |cff808080Requires more Blue gems than Yellow gems|r"
        );
        assert!(
            !met,
            "the second test fails, and the third is past the logic stop"
        );
        // Without `newline` the first line has no break; a lone test that holds is met.
        let single = [
            test(2, 3, 0, 1, 0),
            test(0, 0, 0, 0, 0),
            test(0, 0, 0, 0, 0),
        ];
        let single = [single[0], single[1], single[2], single[1], single[1]];
        let (text, met) = condition_text(&single, [0, 2, 0, 0], false, &strings);
        assert_eq!(text, "  |cffffffffRequires at least 2 Red gems|r");
        assert!(met);
    }

    #[test]
    fn a_gem_matches_a_socket_by_any_shared_colour_bit() {
        assert!(gem_matches_socket(0, None), "no colour takes anything");
        assert!(gem_matches_socket(2, Some(2)));
        assert!(gem_matches_socket(2, Some(6)), "orange fits red");
        assert!(gem_matches_socket(8, Some(14)), "prismatic fits blue");
        assert!(!gem_matches_socket(4, Some(2)));
        assert!(!gem_matches_socket(1, Some(2)), "red is no meta");
        assert!(
            !gem_matches_socket(2, None),
            "an unknown gem fits no colour"
        );
    }
}
