//! `SecureCmdOptionParse("options")` (`0x564ae0` → `0x5f0df0`), the macro-condition parser every
//! secure slash command runs its argument through: `/cast [target=player] Lesser Heal; Smite`.
//!
//! The grammar, as `0x5f0df0` scans it: options separated by `;`; each may lead with `[...]`
//! groups, which pass if any one does (the first that passes supplies the target); an option with
//! no group, or with a passing one, is the answer: its text after the last `]`, trimmed, and the
//! group's target, nil when it named none. Inside a group (`0x5f0bb0`), items split on `,` and
//! are trimmed: `target=X` or `@X` names the unit, anything else is a condition that must hold
//! (`0x5ef5c0`). A condition (`0x5f0040`) may lead with `no` (negated, the name longer than 2), is
//! named exactly, case and all, from the table at `0xad14d8`, and takes arguments after `:`, split
//! on `/` and trimmed. An unknown name posts `ERR_UNKNOWN_MACRO_OPTION_S` and holds.
//!
//! A boolean condition given an argument holds when its truth equals the argument's number
//! (`0x5eeb00`): `[combat:0]` is out of combat.
//!
//! Deviation: `flyable`, `flying`, `indoors`, `outdoors`, `stealth`, `equipped`/`worn`,
//! `vehicleui` and `unithasvehicleui` never hold, for state benilla does not track; `spec` reads
//! the primary talent group, as benilla has no dual specialization; `pet:` matches the pet'"'"'s name,
//! never its family.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The player's state the conditions read that no unit snapshot carries, pushed by the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptionState {
    /// `swimming`: the player's `MOVEMENTFLAG_SWIMMING` (`[unit+0xa30] & 0x200000`, `0x5efa63`).
    pub swimming: bool,
    /// `mounted`: the player's mount display (`[unit+0x9c0]`, `0x5efb23`).
    pub mounted: bool,
}

impl super::UiScript {
    /// Push the player state [`OptionState`] carries.
    pub fn set_option_state(&mut self, state: OptionState) {
        set_field!(self, option_state, state);
    }

    /// Drain the condition names `SecureCmdOptionParse` did not know, for the app to post as
    /// `ERR_UNKNOWN_MACRO_OPTION_S` (`0x5f011a`).
    pub fn take_unknown_macro_options(&mut self) -> Vec<String> {
        drain_field!(self, unknown_macro_options)
    }
}

/// Trim the reference's way: spaces only.
fn trim(s: &str) -> &str {
    s.trim_matches(' ')
}

/// `0x76e760` with a length: an ASCII case-insensitive prefix compare.
fn has_prefix_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// The leading digits of an argument as a number (`0x5eeb00`); none reads 0.
fn number(arg: &str) -> i64 {
    arg.bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, b| {
            n.saturating_mul(10).saturating_add(i64::from(b - b'0'))
        })
}

/// One parsed condition.
struct Condition<'a> {
    negated: bool,
    name: &'a str,
    args: Vec<&'a str>,
}

fn parse_condition(item: &str) -> Condition<'_> {
    let (negated, rest) = if item.len() > 2 && has_prefix_ci(item, "no") {
        (true, &item[2..])
    } else {
        (false, item)
    };
    let (name, args) = match rest.find(':') {
        Some(colon) => {
            let tail = &rest[colon + 1..];
            let args = if tail.is_empty() {
                Vec::new()
            } else {
                tail.split('/').map(trim).collect()
            };
            (&rest[..colon], args)
        }
        None => (rest, Vec::new()),
    };
    Condition {
        negated,
        name,
        args,
    }
}

/// A group's target and conditions (`0x5f0bb0`).
fn parse_group(content: &str) -> (Option<&str>, Vec<Condition<'_>>) {
    let mut target = None;
    let mut conditions = Vec::new();
    for item in content.split(',').map(trim) {
        if item.len() >= 7 && has_prefix_ci(item, "target=") {
            target = Some(trim(&item[7..]));
        } else if let Some(t) = item.strip_prefix('@') {
            target = Some(trim(t));
        } else {
            conditions.push(parse_condition(item));
        }
    }
    (target, conditions)
}

/// Whether a boolean condition holds: its truth, or with an argument, its truth against the
/// argument's number.
fn boolean(truth: bool, args: &[&str]) -> bool {
    match args.first() {
        Some(arg) => i64::from(truth) == number(arg),
        None => truth,
    }
}

/// Any numeric argument equal to `value`; no argument holds as `empty` does.
fn any_number(value: i64, args: &[&str], empty: bool) -> bool {
    if args.is_empty() {
        empty
    } else {
        args.iter().any(|a| number(a) == value)
    }
}

/// The button name a number stands for (`0x5eec20`).
fn button_name(n: i64) -> String {
    match n {
        1 => "LeftButton".into(),
        2 => "RightButton".into(),
        3 => "MiddleButton".into(),
        n => format!("Button{n}"),
    }
}

/// Evaluate one condition against `token`, the group's unit (`target` when it named none).
fn holds(model: &Model, token: &str, c: &Condition<'_>) -> Option<bool> {
    let unit = model.unit(token);
    let guid = model.unit_guids.guid_of(token);
    let args = &c.args[..];
    let in_party = |g: u64| g == model.unit_guids.player || model.unit_guids.party.contains(&g);
    let in_raid = |g: u64| model.unit_guids.raid.contains(&g);
    Some(match c.name {
        // Table row 0, `""`, and any unknown name's default: holds (`0x8c8de0`).
        "" => true,
        // The player or the pet in combat (`0x5ef710`).
        "combat" => boolean(
            ["player", "pet"]
                .iter()
                .any(|t| model.unit(t).is_some_and(|u| u.exists && u.in_combat)),
            args,
        ),
        "exists" => boolean(unit.is_some_and(|u| u.exists), args),
        "help" => boolean(unit.is_some_and(|u| u.exists && u.can_assist), args),
        "harm" => boolean(unit.is_some_and(|u| u.exists && u.can_attack), args),
        "dead" => boolean(unit.is_some_and(|u| u.exists && (u.dead || u.ghost)), args),
        "party" => boolean(guid.is_some_and(in_party), args),
        "raid" => boolean(guid.is_some_and(|g| in_party(g) || in_raid(g)), args),
        // `group`, `group:party`, `group:raid` (`0x5eeed0`).
        "group" => {
            let party = model.unit_guids.party.iter().any(|&g| g != 0);
            let raid = !model.unit_guids.raid.is_empty();
            if args.is_empty() {
                party || raid
            } else {
                args.iter().any(|a| match *a {
                    "party" => party,
                    "raid" => raid,
                    _ => false,
                })
            }
        }
        // The form's 1-based stance index, shifted only (`0x5eefb0` → `0x53d4b0(1)`); no
        // argument: any form.
        "stance" | "form" => {
            let current = i64::from(super::shapeshift::current_form_index(model, true));
            any_number(current, args, current != 0)
        }
        // A pet, by name when arguments say (`0x5efbd0`).
        "pet" => {
            let pet = model.unit("pet").filter(|u| u.exists);
            match pet {
                None => false,
                Some(_) if args.is_empty() => true,
                Some(p) => args
                    .iter()
                    .any(|a| p.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(a))),
            }
        }
        // Each argument through `IsModifiedClick`'s test (`0x5ef0a2` → `0x55f940`).
        "modifier" | "mod" => {
            if args.is_empty() {
                super::modified_click::is_modified_click(model, None)
            } else {
                args.iter()
                    .any(|a| super::modified_click::is_modified_click(model, Some(a)))
            }
        }
        // The click's button, `LeftButton` outside a click (`0x5ef0f8`).
        "button" | "btn" => {
            let current = model.click_button.as_deref().unwrap_or("LeftButton");
            args.iter().any(|a| {
                let want = if a.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                    button_name(number(a))
                } else {
                    (*a).to_string()
                };
                want.eq_ignore_ascii_case(current)
            })
        }
        // The main bar's page (`[0xc1e598] + 1`, `0x5ef204`).
        "actionbar" | "bar" => any_number(i64::from(model.action_bar_page) + 1, args, false),
        // The bonus bar offset (`[0xc1e59c]`).
        "bonusbar" => any_number(i64::from(model.bonus_bar_offset), args, false),
        // The player's running channel, by spell name when arguments say (`0x5efeb0`).
        "channeling" => {
            let channel = model
                .unit_casts
                .channel_of(model.unit_guids.player)
                .filter(|c| model.now * 1000.0 < c.end_ms);
            match channel {
                None => false,
                Some(_) if args.is_empty() => true,
                Some(c) => args.iter().any(|a| c.name.eq_ignore_ascii_case(a)),
            }
        }
        "cursor" => boolean(model.cursor.is_some(), args),
        "swimming" => boolean(model.option_state.swimming, args),
        "mounted" => boolean(model.option_state.mounted, args),
        "spec" => any_number(1, args, true),
        "flyable" | "flying" | "indoors" | "outdoors" | "stealth" | "equipped" | "worn"
        | "vehicleui" | "unithasvehicleui" => false,
        _ => return None,
    })
}

/// One group (`0x5ef5c0`): every condition holds, each negated by `no`, against the group's unit.
/// `Some(target)` when it passes. An unknown condition name is queued for its message and holds.
fn group_passes<'a>(model: &mut Model, content: &'a str) -> Option<Option<&'a str>> {
    let (target, conditions) = parse_group(content);
    // An empty or absent target is the selection (`0x60b029`).
    let token = target.filter(|t| !t.is_empty()).unwrap_or("target");
    let token: String = token.chars().take(31).collect();
    for c in &conditions {
        let truth = match holds(model, &token, c) {
            Some(t) => t,
            None => {
                model.unknown_macro_options.push(c.name.to_string());
                true
            }
        };
        if truth == c.negated {
            return None;
        }
    }
    Some(target)
}

/// Parse `options` as `0x5f0df0` does: the chosen option's text and its group's target, or
/// `None` when no option passes.
pub(crate) fn parse(model: &mut Model, options: &str) -> Option<(String, Option<String>)> {
    let bytes = options.as_bytes();
    let mut i = 0;
    loop {
        let mut text_start = i;
        let mut seen_group = false;
        let mut passed: Option<Option<&str>> = None;
        // Scan one option: groups, then text, up to `;` or the end.
        while i < bytes.len() && bytes[i] != b';' {
            if bytes[i] == b'[' {
                seen_group = true;
                let open = i + 1;
                let close = bytes[open..]
                    .iter()
                    .position(|&b| b == b']')
                    .map_or(bytes.len(), |p| open + p);
                if passed.is_none() {
                    passed = group_passes(model, trim(&options[open..close]));
                }
                i = (close + 1).min(bytes.len());
                text_start = i;
            } else {
                i += 1;
            }
        }
        if !seen_group || passed.is_some() {
            let text = trim(&options[text_start..i]).to_string();
            let target = passed
                .flatten()
                .filter(|t| !t.is_empty())
                .map(str::to_string);
            return Some((text, target));
        }
        if i >= bytes.len() {
            return None;
        }
        i += 1;
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // SecureCmdOptionParse("options"): the option text and its target (nil when none), or nothing
    // when no option passes (`0x564b3f`). A non-string raises (`0x564b00`).
    lua.globals().set(
        "SecureCmdOptionParse",
        lua.create_function(|lua, options: Value| {
            let options = super::binding_abi::string_arg(
                lua,
                options,
                r#"Usage: SecureCmdOptionParse("options")"#,
            )?;
            let parsed = {
                let mut model = crate::script::model_mut_of(lua);
                parse(&mut model, &options)
            };
            let Some((text, target)) = parsed else {
                return Ok(MultiValue::new());
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&text)?),
                match target {
                    Some(t) => Value::String(lua.create_string(&t)?),
                    None => Value::Nil,
                },
            ]))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{UiScript, UnitGuids, UnitState};

    const ME: u64 = 0x10;
    const MOB: u64 = 0xF130_0000_0000_0001;

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: MOB,
            ..Default::default()
        });
        s
    }

    fn parse(s: &UiScript, options: &str) -> String {
        s.eval::<String>(&format!(
            "local a, t = SecureCmdOptionParse({options:?}) return tostring(a) .. '|' .. tostring(t)"
        ))
        .unwrap()
    }

    #[test]
    fn plain_text_and_targets() {
        let s = script();
        assert_eq!(parse(&s, "Lesser Heal"), "Lesser Heal|nil");
        assert_eq!(parse(&s, "  Lesser Heal  "), "Lesser Heal|nil");
        assert_eq!(
            parse(&s, "[target=player] Lesser Heal"),
            "Lesser Heal|player"
        );
        assert_eq!(parse(&s, "[@player] Lesser Heal"), "Lesser Heal|player");
        assert_eq!(parse(&s, "[ TARGET= focus ] Heal"), "Heal|focus");
        assert_eq!(parse(&s, "[] Heal"), "Heal|nil", "an empty group passes");
        assert_eq!(parse(&s, ""), "|nil");
    }

    #[test]
    fn options_fall_through_to_the_first_that_passes() {
        let mut s = script();
        s.set_unit(
            "target",
            Some(UnitState {
                exists: true,
                can_attack: true,
                ..Default::default()
            }),
        );
        assert_eq!(parse(&s, "[help] Heal; [harm] Smite"), "Smite|nil");
        assert_eq!(parse(&s, "[help] Heal; [noharm] Smite; Wand"), "Wand|nil");
        assert_eq!(
            parse(&s, "[help] Heal"),
            "nil|nil",
            "no option passes: nothing returned"
        );
        assert_eq!(
            parse(&s, "[help][harm,@target] Smite"),
            "Smite|target",
            "groups are OR'd; the passing one names the target"
        );
        assert_eq!(parse(&s, "[harm:1] A; B"), "A|nil", "a numeric argument");
        assert_eq!(parse(&s, "[harm:0] A; B"), "B|nil");
    }

    #[test]
    fn stance_counts_only_while_shifted() {
        let mut s = script();
        let form = |active| crate::script::ShapeshiftFormView {
            active,
            ..Default::default()
        };
        // A lit paladin aura leaves the form byte clear: no stance.
        s.set_shapeshift_forms(vec![form(true), form(false)]);
        assert_eq!(parse(&s, "[stance] A; B"), "B|nil");
        assert_eq!(parse(&s, "[stance:1] A; B"), "B|nil");
        assert_eq!(parse(&s, "[nostance] A; B"), "A|nil");

        s.set_shapeshift_forms(vec![form(false), form(true)]);
        s.set_shapeshift_form_byte(18);
        assert_eq!(parse(&s, "[stance:2] A; B"), "A|nil");
        assert_eq!(parse(&s, "[form:1/2] A; B"), "A|nil");
        assert_eq!(parse(&s, "[stance] A; B"), "A|nil");
    }

    #[test]
    fn an_unknown_condition_holds_and_is_reported() {
        let mut s = script();
        assert_eq!(parse(&s, "[bogus] A; B"), "A|nil");
        assert_eq!(parse(&s, "[nobogus] A; B"), "B|nil");
        assert_eq!(
            parse(&s, "[Combat] A; B"),
            "A|nil",
            "names match case-sensitively"
        );
        assert_eq!(
            s.take_unknown_macro_options(),
            vec!["bogus".to_string(), "bogus".into(), "Combat".into()]
        );
    }

    #[test]
    fn modifier_and_button_and_bars() {
        let mut s = script();
        assert_eq!(parse(&s, "[mod] A; B"), "B|nil");
        s.set_modifiers(true, false, false);
        assert_eq!(parse(&s, "[mod] A; B"), "A|nil");
        assert_eq!(parse(&s, "[mod:shift] A; B"), "A|nil");
        assert_eq!(parse(&s, "[mod:ctrl] A; B"), "B|nil");
        assert_eq!(
            parse(&s, "[button:1] A; B"),
            "A|nil",
            "LeftButton outside a click"
        );
        assert_eq!(parse(&s, "[button:2] A; B"), "B|nil");
        assert_eq!(parse(&s, "[bar:1] A; B"), "A|nil");
        assert_eq!(parse(&s, "[bar] A; B"), "B|nil", "no argument never holds");
        assert_eq!(parse(&s, "[bonusbar:0] A; B"), "A|nil");
    }
}
