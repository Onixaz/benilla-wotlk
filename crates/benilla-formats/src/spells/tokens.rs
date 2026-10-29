//! The spell-text parser the 3.3.5a client runs over `Spell.dbc` description and aura text
//! (`0x57abc0`): the description's variables from `SpellDescriptionVariables.dbc` (`0x579d60`),
//! then each `$` token (`0x579e50`): a `${…}` expression ([`super::expr`]), a `$<name>` variable, a
//! `$?cond[a][b]` conditional, or a letter with its optional `$/N;` or `$*N;` scale, cross-spell id
//! and slot digit. A token the parser cannot read leaves a `$` and the text after it.

use super::expr::{self, ExprEnv, MathVar, PlayerValue};
use super::soft_float;
use super::{
    SpellDescriptionVariablesCatalog, SpellDisplay, SpellDurationCatalog, SpellRadiusCatalog,
    SpellRangeCatalog,
};

/// The caster's spell-modifier appliers over its tables: the integer, the FPU float and the
/// software float forms, each returning `value` unchanged when no modifier matches.
pub trait SpellMods {
    fn apply_int(&self, d: &SpellDisplay, op: u8, value: i32) -> i32;
    fn apply_float(&self, d: &SpellDisplay, op: u8, value: f32) -> f32;
    fn apply_soft(&self, d: &SpellDisplay, op: u8, value: f32) -> f32;
}

/// What the parser reads of the active player beyond its level and gender.
pub trait Caster {
    /// `IsSpellKnown` `0x7260e0`: the spell is in the player's known set.
    fn knows_spell(&self, spell: u32) -> bool;
    /// The aura test (`0x7282a0`): an aura of the spell is on the player.
    fn has_aura(&self, spell: u32) -> bool;
    /// An expression operand read off the player's fields.
    fn value(&self, value: PlayerValue) -> f32;
    /// The power cost and cost per second (`0x7ff100`) for the view's caster.
    fn power_cost(&self, d: &SpellDisplay) -> i32;
    fn power_cost_per_second(&self, d: &SpellDisplay) -> i32;
    /// `UNIT_MOD_CAST_SPEED`, which scales a hasted period.
    fn cast_speed(&self) -> f32;
}

/// A value supplied to a `%` hole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TokenNumber {
    Int(i64),
    Float(f64),
}

/// The inputs one substitution runs over. `lookup` resolves cross-spell references (`$1234s1`).
pub struct TokenContext<'a> {
    pub durations: &'a SpellDurationCatalog,
    pub radii: &'a SpellRadiusCatalog,
    pub ranges: Option<&'a SpellRangeCatalog>,
    /// `SpellDescriptionVariables.dbc`, the `$name=` definitions a description's `$<name>` reads.
    pub variables: Option<&'a SpellDescriptionVariablesCatalog>,
    /// The active player's skill in a spell's `SkillLineAbility` line, bonuses included, by spell
    /// id; 0 without a player or a line. Every per-level term scales by the level derived from it
    /// (`0x7ff070`, [`SpellDisplay::skill_level`]).
    pub skill: &'a dyn Fn(u32) -> u32,
    pub lookup: &'a dyn Fn(u32) -> Option<&'a SpellDisplay>,
    /// The caster's live spell-modifier tables; absent for contexts without a caster.
    pub mods: Option<&'a dyn SpellMods>,
    /// The parser's no-modifiers flag, set by the aura tooltip: the effect points and the
    /// duration skip their modifiers.
    pub unmodified_points: bool,
    /// The active player's gender, `UNIT_FIELD_BYTES_0` byte 2, which `$g`/`$G` branch on.
    /// Asked only where a branch expands.
    pub gender: &'a dyn Fn() -> u8,
    /// The `$z` token: the home-bind area's name; `None` reads `HOME_INN`. Asked only where a
    /// `$z` expands, so a caller can tell which texts read the bind point.
    pub home_area: &'a dyn Fn() -> Option<&'a str>,
    /// The active player's other state; `None` without one, where every expression operand reads
    /// 0 (`0x578303`) and every condition is false.
    pub caster: Option<&'a dyn Caster>,
    /// The global-string lookup (`0x819d40`): a `GlobalStrings` template by key. `None` leaves the
    /// token unexpanded.
    pub global: &'a dyn Fn(&str) -> Option<String>,
    /// The format call: a template's `%` holes filled, as the CRT formats them.
    pub printf: &'a dyn Fn(&str, &[TokenNumber]) -> String,
}

/// `GetText(key, -1)`: the plural index of -1 is 1, so the `_P1` twin wins where one exists.
fn text_of(ctx: &TokenContext, key: &str) -> Option<String> {
    (ctx.global)(&format!("{key}_P1"))
        .filter(|s| !s.is_empty())
        .or_else(|| (ctx.global)(key))
}

/// A `GlobalStrings` template, filled.
fn keyed(ctx: &TokenContext, key: &str, args: &[TokenNumber]) -> Option<String> {
    Some((ctx.printf)(&text_of(ctx, key)?, args))
}

fn fixed(ctx: &TokenContext, decimals: u8, v: f64) -> String {
    (ctx.printf)(&format!("%.{decimals}f"), &[TokenNumber::Float(v)])
}

/// The spell level (`0x7ff070`) for the active player.
fn skill_level(ctx: &TokenContext, d: &SpellDisplay) -> u32 {
    d.skill_level((ctx.skill)(d.id))
}

/// The effect's modifier type (`0x7fd440`, jump tables `0x7fd498` and `0x7fd53c`): whether the
/// effect's points round to whole numbers, whether its damage is periodic, and whether it takes a
/// damage op at all.
fn classify(effect: u32, aura: u32) -> (bool, bool, bool) {
    match effect {
        2 | 9 | 10 | 17 | 31 | 58 | 67 | 75 | 121 => (true, false, true),
        6 | 27 | 35 | 65 | 119 | 128 | 129 | 143 => match aura {
            3 | 8 | 20 | 53 | 62 | 89 => (true, true, true),
            15 | 43 => (true, false, true),
            21 | 24 | 63 | 64 | 162 => (true, false, false),
            _ => (false, false, false),
        },
        8 | 30 | 62 | 146 => (true, false, false),
        _ => (false, false, false),
    }
}

/// The min-max points (`0x7ff770`): the slot's `(min, max)` as soft floats. With `level` 0 the
/// spell's own; less `baseLevel` when that is positive, floored at 0. The bounds are
/// `BasePoints + 1` and `BasePoints + DieSides`, each plus `RealPointsPerLevel × level`. Unless
/// modifiers are off: op 8, the slot's own op (3, 12, 23), the damage op (22 periodic, else 0), and
/// op 2 for auras 10, 103 and 183. Both quantize to 1/128; a rounding effect floors the minimum and
/// ceils the maximum.
fn effect_points(d: &SpellDisplay, slot: usize, ctx: &TokenContext, level: u32) -> (f32, f32) {
    let level = if level == 0 {
        skill_level(ctx, d)
    } else {
        level
    } as i32;
    let base_level = d.base_level as i32;
    let delta = if base_level > 0 {
        level.wrapping_sub(base_level)
    } else {
        level
    }
    .max(0);
    let base = d.effect_base_points[slot];
    let real = soft_float::mul_f32(
        d.effect_real_points_per_level[slot],
        soft_float::int_to_float(delta),
    );
    let mut min = soft_float::add_f32(soft_float::int_to_float(base.wrapping_add(1)), real);
    let mut max = soft_float::add_f32(
        soft_float::int_to_float(base.wrapping_add(d.effect_die_sides[slot])),
        real,
    );
    let aura = d.effect_apply_aura[slot];
    let (rounds, periodic, damage) = classify(d.effects[slot], aura);
    if let Some(mods) = ctx.mods.filter(|_| !ctx.unmodified_points) {
        let slot_op = [3, 12, 23][slot];
        let damage_op = damage.then_some(if periodic { 22 } else { 0 });
        let aura_op = matches!(aura, 10 | 103 | 183).then_some(2);
        for op in [Some(8), Some(slot_op), damage_op, aura_op]
            .into_iter()
            .flatten()
        {
            min = mods.apply_soft(d, op, min);
            max = mods.apply_soft(d, op, max);
        }
    }
    let (min, max) = (soft_float::quantize(min), soft_float::quantize(max));
    if rounds {
        soft_float::floor_ceil(min, max)
    } else {
        (min, max)
    }
}

fn modify_int(ctx: &TokenContext, d: &SpellDisplay, op: u8, value: i32) -> i32 {
    ctx.mods.map_or(value, |m| m.apply_int(d, op, value))
}

fn modify_float(ctx: &TokenContext, d: &SpellDisplay, op: u8, value: f32) -> f32 {
    ctx.mods.map_or(value, |m| m.apply_float(d, op, value))
}

/// `AttributesExE` `0x2000`: a periodic spell's duration and tick scale with cast speed.
const ATTR_EX5_HASTE_AFFECTS_DURATION: u32 = 0x2000;
/// `AttributesExE` `0x200`: the over-time total counts one tick more.
const ATTR_EX5_START_PERIODIC_AT_APPLY: u32 = 0x200;
/// `AttributesExC` `0x20000000`: no haste.
const ATTR_EX3_IGNORE_HASTE: u32 = 0x2000_0000;
/// `AttributesExB` `0x1000000`: a referenced spell scales by the player's level.
const ATTR_EX2_PLAYER_LEVEL: u32 = 0x100_0000;

/// A tick or duration scaled by cast speed, as the duration read and `$t` scale them: the spell is
/// periodic, opts in and is not exempt, and the player's speed is at least 0.001; rounded to
/// nearest. Deviation: the duration's haste (`0x800a70`) also opts in a spell an aura of type 316
/// reaches, which the modifier tables here do not report.
fn hasted(ctx: &TokenContext, d: &SpellDisplay, ms: i32) -> i32 {
    let Some(caster) = ctx.caster else { return ms };
    let periodic = d.effect_amplitude.iter().any(|&p| p != 0);
    let speed = caster.cast_speed();
    if periodic
        && d.attributes_ex5 & ATTR_EX5_HASTE_AFFECTS_DURATION != 0
        && d.attributes_ex3 & ATTR_EX3_IGNORE_HASTE == 0
        && speed >= 0.001
    {
        (ms as f32 * speed).round_ties_even() as i32
    } else {
        ms
    }
}

/// The duration read (`0x800a70`): 0 with no row, else the row's base plus its per-level term at
/// the spell's own level less `baseLevel`, unfloored, capped at the row's maximum, then op 1 unless
/// modifiers are off, then cast speed when `haste` asks for it.
fn duration_ms(d: &SpellDisplay, ctx: &TokenContext, haste: bool) -> i32 {
    let Some(row) = ctx.durations.get(d.duration_index) else {
        return 0;
    };
    let levels = (skill_level(ctx, d) as i32).wrapping_sub(d.base_level as i32);
    let resolved = row
        .base_ms
        .wrapping_add(levels.wrapping_mul(row.per_level_ms))
        .min(row.max_ms);
    let modified = if ctx.unmodified_points {
        resolved
    } else {
        modify_int(ctx, d, 1, resolved)
    };
    if haste {
        hasted(ctx, d, modified)
    } else {
        modified
    }
}

/// The time-fraction test (`0x61a960`): the time in its largest unit, its float copy less the floor
/// of the double, under `fraction`.
fn time_fraction_below(ms: f32, fraction: f32) -> bool {
    let t = f64::from(ms);
    let value = if ms >= 86_400_000.0 {
        t * f64::from(1.157_407_4e-8_f32)
    } else if ms >= 3_600_000.0 {
        t * f64::from(2.777_778e-7_f32)
    } else if ms >= 60_000.0 {
        t * f64::from(1.666_666_7e-5_f32)
    } else {
        t * f64::from(0.001_f32)
    };
    f64::from(value as f32) - value.floor() < f64::from(fraction)
}

/// The `$d` text: no positive duration is `SPELL_DURATION_UNTIL_CANCELLED`; a whole unit takes the
/// duration string (`0x61a9e0`) over `INT_SPELL_DURATION`, a fractional one the float duration
/// string (`0x61aee0`) over `SPELL_DURATION`.
fn duration_text(ms: i32, ctx: &TokenContext) -> Option<String> {
    if ms <= 0 {
        return text_of(ctx, "SPELL_DURATION_UNTIL_CANCELLED");
    }
    let unit = |ms: f64| -> (&'static str, f64) {
        if ms >= 86_400_000.0 {
            ("DAYS", 86_400_000.0)
        } else if ms >= 3_600_000.0 {
            ("HOURS", 3_600_000.0)
        } else if ms >= 60_000.0 {
            ("MIN", 60_000.0)
        } else {
            ("SEC", 1_000.0)
        }
    };
    if time_fraction_below(ms as f32, 0.01) {
        let (suffix, per) = unit(f64::from(ms));
        let n = (f64::from(ms) / per) as i64;
        keyed(
            ctx,
            &format!("INT_SPELL_DURATION_{suffix}"),
            &[TokenNumber::Int(n)],
        )
    } else {
        let t = ms as f32;
        let (suffix, value) = if t >= 86_400_000.0 {
            ("DAYS", t / 86_400_000.0)
        } else if t >= 3_600_000.0 {
            ("HOURS", t / 3_600_000.0)
        } else if t >= 60_000.0 {
            ("MIN", t * 1.666_666_7e-5)
        } else {
            ("SEC", t * 0.001)
        };
        keyed(
            ctx,
            &format!("SPELL_DURATION_{suffix}"),
            &[TokenNumber::Float(f64::from(value.max(0.0)))],
        )
    }
}

/// The points arm's integer read of a scaled bound: the bound truncated, and whether it counts as
/// whole, within 0.001 of its floor or its ceiling. A bound just under its ceiling reads as the
/// ceiling.
fn whole(v: f32) -> (i32, bool) {
    let x = f64::from(v);
    let eps = f64::from(0.001f32);
    let truncated = x.floor() as i32;
    let near_ceil = x.ceil() - x < eps;
    if x - x.floor() < eps || near_ceil {
        let up = near_ceil && truncated != x.ceil() as i32;
        (truncated + i32::from(up), true)
    } else {
        (truncated, false)
    }
}

/// A fractional number keys the `$l` plural as 2, a whole one as itself.
fn plural_key(v: f64) -> i32 {
    if v.fract() == 0.0 {
        v as i32
    } else {
        2
    }
}

/// `$name=` definitions resolved by `0x579d60`: a 56-byte entry each of name, text and values.
#[derive(Clone, Debug, Default)]
struct Variable {
    name: String,
    text: String,
    int_value: i32,
    value: f32,
}

/// The CRT's `atoi`: an optional sign, then decimal digits up to the first other character;
/// nothing parsed is 0.
fn atoi(bytes: &[u8]) -> i32 {
    let (negative, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let n = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0i32, |n, &b| {
            n.wrapping_mul(10).wrapping_add(i32::from(b - b'0'))
        });
    if negative {
        n.wrapping_neg()
    } else {
        n
    }
}

fn find(text: &str, from: usize, c: u8) -> Option<usize> {
    let from = from.min(text.len());
    text.as_bytes()[from..]
        .iter()
        .position(|&b| b == c)
        .map(|i| from + i)
}

fn byte(text: &str, at: usize) -> u8 {
    text.as_bytes().get(at).copied().unwrap_or(0)
}

/// One run of the parser over one spell: `0x57abc0`'s level and the shared state its pieces
/// write, the variable table (`0xbeb668`) and the number `$l` keys on (`0xbeb660`).
struct Parser<'c, 'a> {
    ctx: &'c TokenContext<'a>,
    spell: &'c SpellDisplay,
    level: u32,
    variables: Vec<Variable>,
    last_number: i32,
}

/// The expression environment of one `${…}`: the parser's variables and spell.
struct Env<'p, 'c, 'a> {
    parser: &'p Parser<'c, 'a>,
}

impl ExprEnv for Env<'_, '_, '_> {
    fn variable(&self, name: &str) -> f32 {
        if name.is_empty() {
            return 0.0;
        }
        self.parser
            .variables
            .iter()
            .find(|v| v.name == name)
            .map_or(0.0, |v| v.value)
    }

    fn value(&self, spell: Option<u32>, var: MathVar) -> Option<f32> {
        let p = self.parser;
        let d = match spell {
            Some(id) => (p.ctx.lookup)(id)?,
            None => p.spell,
        };
        let Some(caster) = p.ctx.caster else {
            return Some(0.0);
        };
        Some(operand(p.ctx, d, p.level, caster, var))
    }
}

/// The variable value (`0x5782d0`) for one named operand.
fn operand(
    ctx: &TokenContext,
    d: &SpellDisplay,
    level: u32,
    caster: &dyn Caster,
    var: MathVar,
) -> f32 {
    let s = |slot: u8| usize::from(slot);
    match var {
        MathVar::MinPoints(slot) => effect_points(d, s(slot), ctx, level).0,
        MathVar::MaxPoints(slot) => effect_points(d, s(slot), ctx, level).1,
        MathVar::Radius(slot) => ctx
            .radii
            .get(d.effect_radius_index[s(slot)])
            .map_or(0.0, |r| modify_float(ctx, d, 6, r.radius)),
        MathVar::Duration => duration_ms(d, ctx, true) as f32 * 0.001,
        MathVar::PowerCost => caster.power_cost(d) as f32,
        MathVar::PowerCostPerSecond => caster.power_cost_per_second(d) as f32,
        MathVar::ChainTargets(slot) => {
            modify_int(ctx, d, 17, d.effect_chain_targets[s(slot)] as i32) as f32
        }
        MathVar::AuraPeriod(slot) => {
            if d.proc_flags & 1 != 0 {
                5000.0
            } else {
                modify_int(ctx, d, 19, d.effect_amplitude[s(slot)] as i32) as f32
            }
        }
        MathVar::ProcChance => modify_int(ctx, d, 18, d.proc_chance as i32) as f32,
        MathVar::ProcCharges => modify_int(ctx, d, 4, d.proc_charges as i32) as f32,
        MathVar::PointsPerCombo(slot) => d.effect_points_per_combo_point[s(slot)],
        MathVar::StackAmount => d.stack_amount as i32 as f32,
        MathVar::MaxTargetLevel => d.max_target_level as i32 as f32,
        MathVar::MaxTargets => d.max_affected_targets as i32 as f32,
        MathVar::MultipleValue(slot) => modify_float(ctx, d, 27, d.effect_multiple_value[s(slot)]),
        MathVar::DamageMultiplier(slot) => d.damage_multiplier[s(slot)],
        // Deviation: the reference indexes these from the `f` operands' base, so they read 0; no
        // shipped text uses them, and the slot is read here.
        MathVar::MiscValue(slot) => d.effect_misc_value[s(slot)] as f32,
        MathVar::BonusCoefficient(slot) => {
            modify_float(ctx, d, 24, d.effect_bonus_multiplier[s(slot)])
        }
        MathVar::Player(v) => caster.value(v),
    }
}

impl Parser<'_, '_> {
    /// The sub-text parse (`0x57aaf0`): `text` with each token replaced; false when one failed,
    /// which also leaves a `$` in its place.
    fn sub_text(&mut self, text: &str, out: &mut String) -> bool {
        let mut ok = true;
        let mut start = 0;
        let mut cursor = find(text, 0, b'$');
        while let Some(at) = cursor {
            out.push_str(&text[start..at]);
            let mut pos = at + 1;
            if !self.replace(text, &mut pos, out) {
                out.push('$');
                ok = false;
            }
            start = pos.min(text.len());
            cursor = find(text, start, b'$');
        }
        out.push_str(&text[start..]);
        ok
    }

    /// The token replace (`0x579e50`) with `pos` just past the `$`.
    fn replace(&mut self, text: &str, pos: &mut usize, out: &mut String) -> bool {
        match byte(text, *pos) {
            b'{' => {
                if let Some(end) = find(text, *pos, b'}') {
                    return match self.expression(text, pos, end) {
                        Some((shown, int, _)) => {
                            out.push_str(&shown);
                            self.last_number = int;
                            true
                        }
                        None => false,
                    };
                }
            }
            b'<' => {
                if let Some(end) = find(text, *pos, b'>') {
                    return self.variable(text, pos, end, out);
                }
            }
            _ => {}
        }
        let mut scale = 1.0f32;
        let op = byte(text, *pos);
        if op == b'/' || op == b'*' {
            if let Some(semi) = find(text, *pos, b';') {
                let digits = &text.as_bytes()[*pos + 1..semi.min(*pos + 8)];
                // A zero leaves 1.0; `/` takes the reciprocal on the x87 at double precision.
                scale = match atoi(digits) {
                    0 => 1.0,
                    n if op == b'/' => (1.0 / f64::from(n)) as f32,
                    n => n as f32,
                };
                *pos = semi + 1;
            }
        }
        let mut spell = self.spell;
        let mut level = self.level;
        let digits = text.as_bytes()[*pos..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits > 0 {
            let id = atoi(&text.as_bytes()[*pos..*pos + digits.min(7)]);
            *pos += digits;
            // Deviation: the reference first swaps in the row for the instance's difficulty
            // (`0x802850`); outside a dungeon or raid that is the same id.
            let Some(loaded) = (self.ctx.lookup)(id as u32) else {
                return false;
            };
            spell = loaded;
            if let Some(caster) = self
                .ctx
                .caster
                .filter(|_| loaded.attributes_ex2 & ATTR_EX2_PLAYER_LEVEL != 0)
            {
                level = caster.value(PlayerValue::Level) as u32;
            }
            if loaded.max_level > 0 && level >= loaded.max_level {
                level = loaded.max_level;
            }
        }
        let letter = byte(text, *pos);
        let slot_digit = byte(text, *pos + 1);
        let (slot, has_slot) = if (b'1'..=b'9').contains(&slot_digit) {
            let s = usize::from(slot_digit - b'1');
            (if s >= 3 { 0 } else { s }, true)
        } else {
            (0, false)
        };
        let ctx = self.ctx;
        let shown = match letter {
            b'?' => {
                *pos += 1;
                return self.conditional(text, pos, out, None);
            }
            b'l' | b'L' => {
                *pos += 1;
                self.plural(text, pos, out);
                return true;
            }
            b'g' | b'G' => {
                *pos += 1;
                if ctx.caster.is_none() {
                    return false;
                }
                gender_branch(text, pos, (ctx.gender)(), out);
                if *pos < text.len() {
                    *pos += 1;
                }
                return true;
            }
            b'a' | b'A' => {
                let r = ctx.radii.get(spell.effect_radius_index[slot]);
                let v = r.map_or(0, |r| modify_int(ctx, spell, 6, r.radius as i32));
                self.key(v);
                v.to_string()
            }
            b'b' | b'B' => {
                let v = spell.effect_points_per_combo_point[slot] as i32;
                self.last_number = v;
                v.to_string()
            }
            b'c' | b'C' => {
                let v = ctx.caster.map_or(0, |c| c.power_cost(spell));
                self.key(v);
                v.to_string()
            }
            b'd' | b'D' => {
                let ms = duration_ms(spell, ctx, true);
                self.key(ms.max(0) / 1000);
                match duration_text(ms, ctx) {
                    Some(t) => t,
                    None => return false,
                }
            }
            b'e' | b'E' => {
                let v = f64::from(modify_float(
                    ctx,
                    spell,
                    27,
                    spell.effect_multiple_value[slot],
                )) * f64::from(scale);
                self.key(plural_key(v));
                fixed(ctx, 1, v)
            }
            b'F' => {
                // ±0.5 by the sign, then chopped.
                let v =
                    (f64::from(scale) * f64::from(spell.damage_multiplier[slot])).round() as i32;
                self.key(v);
                v.to_string()
            }
            b'f' => {
                let v = f64::from(scale) * f64::from(spell.damage_multiplier[slot]);
                self.key(plural_key(v));
                fixed(ctx, 1, v)
            }
            b'h' | b'H' => {
                let v = modify_int(ctx, spell, 18, spell.proc_chance as i32);
                self.last_number = v;
                v.to_string()
            }
            b'i' | b'I' => {
                let v = spell.max_affected_targets as i32;
                self.last_number = v;
                v.to_string()
            }
            b'm' | b'M' | b'o' | b'O' | b's' | b'S' => {
                let Some((text, n)) = self.points(letter, slot, spell, scale, level) else {
                    return false;
                };
                self.last_number = n;
                text
            }
            b'n' | b'N' => {
                let v = modify_int(ctx, spell, 4, spell.proc_charges as i32);
                self.last_number = v;
                v.to_string()
            }
            b'p' | b'P' => {
                let v = ctx.caster.map_or(0, |c| c.power_cost_per_second(spell));
                self.key(v);
                v.to_string()
            }
            b'q' | b'Q' => {
                let v = spell.effect_misc_value[slot];
                self.last_number = v;
                v.to_string()
            }
            b'r' | b'R' => {
                // The row at or below index 1 is row 1; its first maximum through op 5.
                let row = ctx.ranges.and_then(|r| r.get(spell.range_index.max(1)));
                let v = row.map_or(0.0, |r| modify_float(ctx, spell, 5, r.max));
                self.last_number = v.ceil() as i32;
                (v as i32).to_string()
            }
            b't' | b'T' => {
                let ms = if spell.proc_flags & 1 != 0 {
                    5000
                } else {
                    hasted(
                        ctx,
                        spell,
                        modify_int(ctx, spell, 19, spell.effect_amplitude[slot] as i32),
                    )
                };
                if time_fraction_below(ms as f32, 0.01) {
                    self.key(ms / 1000);
                    (ms / 1000).to_string()
                } else {
                    self.key(2);
                    fixed(ctx, 2, f64::from(ms as f32 * 0.001))
                }
            }
            b'u' | b'U' => {
                let v = spell.stack_amount as i32;
                self.last_number = v;
                v.to_string()
            }
            b'v' | b'V' => {
                let v = spell.max_target_level as i32;
                self.last_number = v;
                v.to_string()
            }
            b'x' | b'X' => {
                let v = modify_int(ctx, spell, 17, spell.effect_chain_targets[slot] as i32);
                self.last_number = v;
                v.to_string()
            }
            b'z' | b'Z' => match (ctx.home_area)() {
                Some(name) => name.to_string(),
                None => match text_of(ctx, "HOME_INN") {
                    Some(t) => t,
                    None => return false,
                },
            },
            _ => return false,
        };
        out.push_str(&shown);
        if *pos < text.len() {
            *pos += 1;
            if has_slot {
                *pos += 1;
            }
        }
        true
    }

    /// Deviation: every numeric arm keys the `$l` plural. The reference's `$a`, `$c`, `$d`, `$e`,
    /// `$f`, `$F`, `$p` and `$t` arms leave `0xbeb660` alone, so its "$s1 … every $t1
    /// $lsecond:seconds;" keys on the damage; we print the grammar the text means.
    fn key(&mut self, n: i32) {
        self.last_number = n;
    }

    /// The `$s $S $m $M $o $O` arm: the effect points times the parser's scale of 1, spread over
    /// the duration for `$o` (unhasted, a period of 0 read as 5000, one tick more under
    /// `AttributesExE` `0x200`), then made positive and scaled, and printed by the letter's
    /// rule. The text and the number `$l` keys on: `m` its whole minimum, the others their whole
    /// maximum, a fraction 2.
    fn points(
        &self,
        letter: u8,
        slot: usize,
        d: &SpellDisplay,
        scale: f32,
        level: u32,
    ) -> Option<(String, i32)> {
        let ctx = self.ctx;
        let (mut min, mut max) = effect_points(d, slot, ctx, level);
        if letter.eq_ignore_ascii_case(&b'o') {
            let amplitude = d.effect_amplitude[slot] as i32;
            let period = if amplitude == 0 { 5000 } else { amplitude };
            let duration = if period > 0 {
                duration_ms(d, ctx, false)
            } else {
                0
            };
            (min, max) = if duration > 0 {
                let extra = if d.attributes_ex5 & ATTR_EX5_START_PERIODIC_AT_APPLY != 0 {
                    period
                } else {
                    0
                };
                let total = f64::from(duration.wrapping_add(extra));
                let over = |v: f32| (total * f64::from(v) / f64::from(period)) as f32;
                (over(min), over(max))
            } else {
                (0.0, 0.0)
            };
        }
        let scaled = |v: f32| (f64::from(v.abs()) * f64::from(scale)) as f32;
        let (lo, hi) = (scaled(min), scaled(max));
        let ((lo_int, lo_whole), (hi_int, hi_whole)) = (whole(lo), whole(hi));
        let plural = match letter {
            b'm' if lo_whole => lo_int,
            b'm' => 2,
            _ if hi_whole => hi_int,
            _ => 2,
        };
        let int = |n: i32| TokenNumber::Int(i64::from(n));
        let text = match letter {
            b'm' if lo_whole => lo_int.to_string(),
            b'm' => fixed(ctx, 1, f64::from(lo)),
            b'M' if hi_whole => hi_int.to_string(),
            b'M' => fixed(ctx, 1, f64::from(hi)),
            // Equal bounds print one number, `%d` unless `$S` holds a fraction.
            _ if min == max && (lo_whole || letter != b'S') => lo_int.to_string(),
            _ if min == max => fixed(ctx, 1, f64::from(lo)),
            _ if lo_whole && hi_whole => keyed(
                ctx,
                "INT_SPELL_POINTS_SPREAD_TEMPLATE",
                &[int(lo_int), int(hi_int)],
            )?,
            b'S' => keyed(
                ctx,
                "SPELL_POINTS_SPREAD_TEMPLATE",
                &[
                    TokenNumber::Float(f64::from(lo)),
                    TokenNumber::Float(f64::from(hi)),
                ],
            )?,
            // A fractional spread under `s`/`o`/`O`: the minimum truncated, a fractional maximum
            // truncated plus one.
            _ => {
                let top = if hi_whole { hi_int } else { hi as i32 + 1 };
                keyed(
                    ctx,
                    "INT_SPELL_POINTS_SPREAD_TEMPLATE",
                    &[int(lo as i32), int(top)],
                )?
            }
        };
        Some((text, plural))
    }

    /// The expression token (`0x579980`) over `text[pos+1..end]`: the cursor moves past the `}` and
    /// an optional `.1`..`.9` precision first, so a failed expression drops its text. The shown
    /// value (`%.0f` or `%.Nf`), its integer (`fistp`, round to nearest) and the value.
    fn expression(&self, text: &str, pos: &mut usize, end: usize) -> Option<(String, i32, f32)> {
        let body = &text[*pos + 1..end];
        *pos = end + 1;
        let mut decimals = 0;
        if byte(text, *pos) == b'.' && (b'1'..=b'9').contains(&byte(text, *pos + 1)) {
            decimals = byte(text, *pos + 1) - b'0';
            *pos += 2;
        }
        let env = Env { parser: self };
        let program = expr::compile(body, &env)?;
        let value = expr::evaluate(&program, &env)?;
        let shown = fixed(self.ctx, decimals, f64::from(value));
        Some((shown, value.round_ties_even() as i32, value))
    }

    /// The variable token (`0x577c40`): `$<name>`, the name cut at 14 characters, prints its
    /// variable's text and keys `$l` on its integer; an unknown name fails.
    fn variable(&mut self, text: &str, pos: &mut usize, end: usize, out: &mut String) -> bool {
        let name = &text.as_bytes()[*pos + 1..end.min(*pos + 15)];
        let Some(v) = self.variables.iter().find(|v| v.name.as_bytes() == name) else {
            return false;
        };
        out.push_str(&v.text);
        self.last_number = v.int_value;
        *pos = end + 1;
        true
    }

    /// The plural conditional (`0x576ef0`): `$l` over `a:b[:c…];`, the form the plural index of the
    /// last number picks, leading and trailing spaces trimmed. With no `;` nothing is consumed.
    fn plural(&self, text: &str, pos: &mut usize, out: &mut String) {
        while byte(text, *pos) == b' ' {
            *pos += 1;
        }
        let Some(semi) = find(text, *pos, b';') else {
            return;
        };
        let forms = &text[*pos..semi];
        *pos = semi + 1;
        // enUS's plural rule (`0x817d10`): the second form for any number but one.
        let index = usize::from(self.last_number != 1);
        // Past the last form the walk stops on the `;`: an empty pick.
        let form = forms.split(':').nth(index).unwrap_or("");
        out.push_str(form.trim_start_matches(' ').trim_end_matches(' '));
    }

    /// The conditional (`0x579ba0`) with `pos` past the `?`. The condition picks its arm, or the
    /// next condition of a `]?` chain, else the last `[…]`. Into `var` (`0x579d60`'s definitions)
    /// the arm's first `${…}` is evaluated as that variable; otherwise the arm is parsed as text. A
    /// conditional without its brackets consumes nothing.
    fn conditional(
        &mut self,
        text: &str,
        pos: &mut usize,
        out: &mut String,
        var: Option<&str>,
    ) -> bool {
        while byte(text, *pos) == b' ' {
            *pos += 1;
        }
        if *pos >= text.len() {
            return true;
        }
        let Some(arms) = verify_conditional(text, *pos) else {
            return true;
        };
        let (start, stop) = if self.condition(text, pos, arms.true_start) {
            (arms.true_start + 1, arms.true_end)
        } else if let Some(mut next) = arms.else_at {
            let ok = self.conditional(text, &mut next, out, var);
            *pos = next;
            return ok;
        } else {
            (arms.false_start + 1, arms.false_end)
        };
        let arm = &text[start..stop];
        let mut ok = true;
        match var {
            Some(name) => {
                if let Some(dollar) = find(arm, 0, b'$') {
                    if let Some(open) = find(arm, dollar, b'{') {
                        if let Some(close) = find(arm, open, b'}') {
                            let mut at = open;
                            self.define(name, arm, &mut at, close);
                        }
                    }
                }
            }
            None => {
                if !self.sub_text(arm, out) {
                    out.push('$');
                    ok = false;
                }
            }
        }
        *pos = arms.false_end + 1;
        ok
    }

    /// The condition parse (`0x5797f0`) from `pos` up to `end`: `s<id>` known, `a<id>` on the
    /// player, `!` negating the next term, `&`, `|` short-circuiting, brackets nesting.
    fn condition(&self, text: &str, pos: &mut usize, end: usize) -> bool {
        let mut cond = false;
        if byte(text, *pos) == 0 {
            return false;
        }
        while *pos < end {
            match byte(text, *pos) {
                b'!' => {
                    *pos += 1;
                    while *pos < end && byte(text, *pos) != 0 {
                        let c = byte(text, *pos);
                        if b"sSaA".contains(&c) {
                            cond = !self.condition_term(text, pos);
                            break;
                        }
                        if c == b'(' {
                            cond = !self.condition(text, pos, end);
                            skip_bracket(text, pos, end);
                            break;
                        }
                        *pos += 1;
                    }
                }
                b'&' => {
                    if !cond {
                        return false;
                    }
                    *pos += 1;
                    cond = self.condition(text, pos, end);
                }
                b'(' => {
                    *pos += 1;
                    cond = self.condition(text, pos, end);
                    skip_bracket(text, pos, end);
                }
                b')' => return cond,
                b'A' | b'S' | b'a' | b's' => cond = self.condition_term(text, pos),
                b'|' => {
                    if cond {
                        return true;
                    }
                    *pos += 1;
                    cond = self.condition(text, pos, end);
                }
                _ => *pos += 1,
            }
            if byte(text, *pos) == 0 {
                return cond;
            }
        }
        cond
    }

    /// The condition read (`0x5796c0`): the letter, then the spell id; false without digits or a
    /// player.
    fn condition_term(&self, text: &str, pos: &mut usize) -> bool {
        let kind = byte(text, *pos);
        *pos += 1;
        let digits = text.as_bytes()[(*pos).min(text.len())..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            return false;
        }
        let id = atoi(&text.as_bytes()[*pos..*pos + digits.min(10)]) as u32;
        *pos += digits;
        let Some(caster) = self.ctx.caster else {
            return false;
        };
        match kind {
            b'a' | b'A' => caster.has_aura(id),
            b's' | b'S' => caster.knows_spell(id),
            _ => false,
        }
    }

    /// The variables block (`0x579d60`) over a `SpellDescriptionVariables.dbc` text: each `$name=`
    /// followed by a `${…}` or a `$?…` defines `name`.
    fn define_all(&mut self, defs: &str) {
        let mut cursor = find(defs, 0, b'$');
        while let Some(at) = cursor {
            let name_at = at + 1;
            let mut cur = name_at;
            if let Some(eq) = find(defs, name_at, b'=') {
                let mut cut = eq.min(name_at + 14);
                while !defs.is_char_boundary(cut) {
                    cut -= 1;
                }
                let name = &defs[name_at..cut];
                let Some(dollar) = find(defs, name_at, b'$') else {
                    break;
                };
                cur = dollar + 1;
                match byte(defs, cur) {
                    b'?' => {
                        cur += 1;
                        let mut sink = String::new();
                        self.conditional(defs, &mut cur, &mut sink, Some(name));
                    }
                    b'{' => {
                        if let Some(close) = find(defs, cur, b'}') {
                            self.define(name, defs, &mut cur, close);
                        }
                    }
                    _ => {}
                }
            }
            cursor = find(defs, cur, b'$');
        }
    }

    /// The variable expression (`0x579b30`): a new entry, named only once its expression evaluates.
    fn define(&mut self, name: &str, text: &str, pos: &mut usize, close: usize) {
        let entry = match self.expression(text, pos, close) {
            Some((shown, int_value, value)) => Variable {
                name: name.to_string(),
                text: shown,
                int_value,
                value,
            },
            None => Variable::default(),
        };
        self.variables.push(entry);
    }
}

/// The three bracket pairs of a conditional (`0x577030`).
struct Arms {
    true_start: usize,
    true_end: usize,
    else_at: Option<usize>,
    false_start: usize,
    false_end: usize,
}

/// The first `[…]` from `at`; a `]?` continues the chain, whose last `[…]` is the false arm.
fn verify_conditional(text: &str, at: usize) -> Option<Arms> {
    let true_start = find(text, at, b'[')?;
    let true_end = find(text, true_start, b']')?;
    if byte(text, true_end + 1) == b'?' {
        let else_at = true_end + 2;
        let rest = verify_conditional(text, else_at)?;
        return Some(Arms {
            true_start,
            true_end,
            else_at: Some(else_at),
            false_start: rest.false_start,
            false_end: rest.false_end,
        });
    }
    let false_start = find(text, true_end, b'[')?;
    let false_end = find(text, false_start, b']')?;
    Some(Arms {
        true_start,
        true_end,
        else_at: None,
        false_start,
        false_end,
    })
}

/// The bracket skip (`0x576ff0`): past the `)` that closes the open bracket.
fn skip_bracket(text: &str, pos: &mut usize, end: usize) {
    let mut depth = 1;
    while *pos < end && byte(text, *pos) != 0 {
        match byte(text, *pos) {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    *pos += 1;
                    return;
                }
            }
            _ => {}
        }
        *pos += 1;
    }
}

/// The gender conditional (`0x576cd0`) with `pos` past the `g`: `male:female[:selector];`, the
/// first form on gender 0, else the second, each trimmed; `pos` lands on the `;`. A `:c` or `:r`
/// selector switches the reference to the class's or race's display gender, which is the player's
/// own here.
fn gender_branch(text: &str, pos: &mut usize, gender: u8, out: &mut String) {
    while byte(text, *pos) == b' ' {
        *pos += 1;
    }
    let Some(colon) = find(text, *pos, b':') else {
        return;
    };
    let Some(semi) = find(text, colon, b';') else {
        return;
    };
    let selector = find(text, colon + 1, b':').filter(|&s| s + 1 < semi && s < semi);
    let form = if gender == 0 {
        &text[*pos..colon]
    } else {
        let mut start = colon + 1;
        while byte(text, start) == b' ' {
            start += 1;
        }
        &text[start.min(semi)..selector.unwrap_or(semi)]
    };
    out.push_str(form.trim_end_matches(' '));
    *pos = semi;
}

/// Substitute every `$`-token in `text`, the description or aura text of `spell`.
pub fn substitute(text: &str, spell: &SpellDisplay, ctx: &TokenContext) -> String {
    let mut parser = Parser {
        ctx,
        spell,
        level: skill_level(ctx, spell),
        variables: Vec::new(),
        last_number: 0,
    };
    if let Some(defs) = ctx
        .variables
        .and_then(|v| v.get(spell.description_variables_id))
    {
        parser.define_all(defs);
    }
    let mut out = String::with_capacity(text.len());
    parser.sub_text(text, &mut out);
    out
}

#[cfg(test)]
mod tests;
