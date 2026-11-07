//! The range check the range verbs ask (`0x809610`): whether a spell can take a unit at all, by its
//! target word and the unit's state, and then the distance against the range lookup's bounds. The
//! relations are read from the player toward the unit; a pet's book reads the pet's position and
//! reach against them.

use benilla_formats::{RangeTargets, RangeUnit, SpellDisplay, SpellRange};
use bevy::math::Vec3;

use super::SpellModifiers;

/// `UNIT_FIELD_FLAGS & 0x10000`, which only `AttributesEx6 & 0x1000000` lets past (`0x809610`).
const UNIT_FLAG_NON_ATTACKABLE_2: u32 = 0x0001_0000;
/// `UNIT_FIELD_FLAGS & 0x100`, the plain unit word's refusal.
const UNIT_FLAG_NOT_ATTACKABLE_1: u32 = 0x0000_0100;
/// `AttributesEx5 & 0x800`: the check runs on the unit's own target when this one cannot take it.
const ATTR_EX5_IMPLIED_TARGET: u32 = 0x800;
const ATTR_EX6_IGNORES_FLAG: u32 = 0x0100_0000;

const WORD_UNIT: u32 = 0x0002;
const WORD_RAID: u32 = 0x0004;
const WORD_PARTY: u32 = 0x0008;
const WORD_ENEMY: u32 = 0x0080;
const WORD_ASSIST: u32 = 0x0100;
const WORD_CORPSE_ENEMY: u32 = 0x0200;
const WORD_DEAD_ONLY: u32 = 0x0400;
const WORD_GAMEOBJECT: u32 = 0x0800;
const WORD_ITEM_GAMEOBJECT: u32 = 0x4000;
const WORD_CORPSE_ALLY: u32 = 0x8000;
const WORD_ANY_ALLY: u32 = WORD_RAID | WORD_PARTY | WORD_ASSIST;

/// One unit as the check reads it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RangeFacts {
    pub(crate) unit: RangeUnit,
    pub(crate) pos: Vec3,
    pub(crate) flags: u32,
    pub(crate) alive: bool,
    /// `UNIT_DYNAMIC_FLAGS & 0x20`.
    pub(crate) dead_flag: bool,
    pub(crate) creature_type: u32,
    pub(crate) pet_number: u32,
    pub(crate) attackable: bool,
    pub(crate) in_party: bool,
    pub(crate) in_group: bool,
    /// `UNIT_FIELD_TARGET`, the implied target's read.
    pub(crate) target: Option<u64>,
}

/// The spell's target word with its implicit targets folded in, and whether one is the pet.
fn range_word(def: &SpellDisplay) -> (u32, bool) {
    let mut word = def.targets & 0xffff;
    let mut pet = false;
    for implicit in def.effect_implicit_target_a {
        match implicit {
            1 => word &= !WORD_DEAD_ONLY,
            5 => {
                pet = true;
                word &= !WORD_CORPSE_ALLY;
            }
            6 | 53 => word |= WORD_ENEMY,
            21 | 45 => word |= WORD_ASSIST,
            23 => word |= WORD_GAMEOBJECT,
            25 | 63..=71 | 74 | 75 => word |= WORD_UNIT,
            26 => word |= WORD_ITEM_GAMEOBJECT,
            35 => word |= WORD_PARTY,
            57 | 61 => word |= WORD_RAID,
            _ => {}
        }
    }
    (word, pet)
}

/// The implied target (`0x7ffc40`): an ally spell on an enemy reads the enemy's own target when
/// that is an ally the word takes, an enemy spell on an ally the ally's target when attackable.
fn implied_target(
    unit: RangeFacts,
    word: u32,
    facts: &impl Fn(u64) -> Option<RangeFacts>,
) -> Option<RangeFacts> {
    let own_target = || unit.target.and_then(facts);
    if word & WORD_ANY_ALLY != 0 {
        if !unit.attackable {
            return Some(unit);
        }
        let implied = own_target().filter(|t| t.unit.assist)?;
        let taken = word & WORD_ASSIST != 0
            || (word & WORD_PARTY != 0 && implied.in_party)
            || (word & WORD_RAID != 0 && implied.in_group);
        return taken.then_some(implied);
    }
    if word & WORD_ENEMY != 0 && unit.unit.assist {
        return own_target().filter(|t| t.attackable);
    }
    Some(unit)
}

/// Whether the word takes `unit`.
fn word_takes(word: u32, unit: &RangeFacts) -> bool {
    let assist = unit.unit.assist;
    let dead = !unit.alive;
    (word & WORD_PARTY != 0 && unit.in_party && assist)
        || (word & WORD_RAID != 0 && unit.in_group && assist)
        || (word & WORD_ASSIST != 0 && assist)
        || (word & WORD_ENEMY != 0 && unit.attackable)
        || (word & WORD_UNIT != 0 && unit.flags & UNIT_FLAG_NOT_ATTACKABLE_1 == 0)
        || (word & WORD_CORPSE_ALLY != 0
            && dead
            && assist
            && (unit.unit.player || unit.pet_number != 0))
        || (word & WORD_CORPSE_ENEMY != 0 && dead && unit.unit.player && !assist)
}

/// `Some(in range)` for a unit the spell can take, else `None`, which `IsSpellInRange` reads as
/// nil. `caster` is the player, or the pet for a pet book's spell.
pub(crate) fn range_check(
    def: &SpellDisplay,
    row: Option<&SpellRange>,
    mods: &SpellModifiers,
    caster: (RangeUnit, Vec3),
    target: u64,
    pet: Option<u64>,
    facts: impl Fn(u64) -> Option<RangeFacts>,
) -> Option<bool> {
    let (mut word, on_pet) = range_word(def);
    let mut unit = facts(target)?;
    if on_pet && word & WORD_UNIT == 0 {
        unit = facts(pet?)?;
        word |= WORD_UNIT;
    }
    if def.attributes_ex5 & ATTR_EX5_IMPLIED_TARGET != 0 {
        unit = implied_target(unit, word, &facts)?;
    }
    if unit.flags & UNIT_FLAG_NON_ATTACKABLE_2 != 0
        && def.attributes_ex6 & ATTR_EX6_IGNORES_FLAG == 0
    {
        return None;
    }
    if !def.admits_creature_type(unit.creature_type) {
        return None;
    }
    if !unit.alive
        && !unit.dead_flag
        && word & (WORD_DEAD_ONLY | WORD_CORPSE_ALLY | WORD_CORPSE_ENEMY) == 0
        && !def.allows_dead_target()
    {
        return None;
    }
    if unit.alive && word & WORD_DEAD_ONLY != 0 {
        return None;
    }
    if !word_takes(word, &unit) {
        return None;
    }
    let targets = RangeTargets {
        target: Some(unit.unit),
        attack_target: None,
    };
    // The self row's bounds are the pad alone: the lookup's max stays 0 before the reach adds.
    let (min, max) = mods
        .min_max_range(def, row, caster.0, targets)
        .unwrap_or((0.0, caster.0.reach + unit.unit.reach));
    let d2 = caster.1.distance_squared(unit.pos);
    Some(min * min <= d2 && d2 <= max * max)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u64 = 1;
    const FRIEND: u64 = 2;
    const FOE: u64 = 3;

    const HOLY_SHOCK: SpellRange = SpellRange {
        min: 0.0,
        max: 20.0,
        friendly_min: 0.0,
        friendly_max: 40.0,
        flags: 0,
    };

    fn fact(guid: u64, yards: f32) -> RangeFacts {
        RangeFacts {
            unit: RangeUnit {
                assist: guid != FOE,
                ..RangeUnit::still(0.0)
            },
            pos: Vec3::new(yards, 0.0, 0.0),
            flags: 0,
            alive: true,
            dead_flag: false,
            creature_type: 0,
            pet_number: 0,
            attackable: guid == FOE,
            in_party: false,
            in_group: false,
            target: None,
        }
    }

    fn check(def: &SpellDisplay, target: u64, yards: f32) -> Option<bool> {
        check_row(def, &HOLY_SHOCK, target, yards)
    }

    fn check_row(def: &SpellDisplay, row: &SpellRange, target: u64, yards: f32) -> Option<bool> {
        let mods = SpellModifiers::default();
        let caster = (RangeUnit::still(0.0), Vec3::ZERO);
        range_check(def, Some(row), &mods, caster, target, None, |guid| {
            (guid == target).then(|| fact(guid, yards))
        })
    }

    /// A spell taking a friend or a foe by its word, as Holy Shock's pair is read: the friendly
    /// pair toward an ally, the hostile toward an enemy, and the self answers at distance 0.
    #[test]
    fn a_self_a_friend_and_a_foe_read_their_own_pair() {
        let shock = SpellDisplay {
            targets: WORD_ENEMY | WORD_ASSIST,
            ..Default::default()
        };
        assert_eq!(check(&shock, ME, 0.0), Some(true));
        assert_eq!(check(&shock, FRIEND, 30.0), Some(true), "within 40");
        assert_eq!(check(&shock, FRIEND, 41.0), Some(false));
        assert_eq!(check(&shock, FOE, 20.0), Some(true), "at 20 exactly");
        assert_eq!(check(&shock, FOE, 30.0), Some(false), "past the hostile 20");
    }

    /// A spell whose word has no arm for the unit answers nil, in or out of range.
    #[test]
    fn a_word_that_cannot_take_the_unit_answers_nil() {
        let heal = SpellDisplay {
            targets: WORD_ASSIST,
            ..Default::default()
        };
        assert_eq!(check(&heal, FOE, 1.0), None);
        let bolt = SpellDisplay {
            targets: WORD_ENEMY,
            ..Default::default()
        };
        assert_eq!(check(&bolt, FRIEND, 1.0), None);
        let no_word = SpellDisplay::default();
        assert_eq!(check(&no_word, FOE, 1.0), None);
    }

    /// The min fails before the max: inside a row's minimum reads 0.
    #[test]
    fn inside_the_minimum_is_out_of_range() {
        let shot = SpellRange {
            min: 8.0,
            max: 35.0,
            friendly_min: 8.0,
            friendly_max: 35.0,
            flags: 0,
        };
        let aimed = SpellDisplay {
            targets: WORD_ENEMY,
            ..Default::default()
        };
        assert_eq!(check_row(&aimed, &shot, FOE, 5.0), Some(false));
        assert_eq!(check_row(&aimed, &shot, FOE, 20.0), Some(true));
    }

    /// A dead unit needs the dead flag, a word admitting the dead or `AttributesEx2 & 1`, and the
    /// dead-only word refuses a living one.
    #[test]
    fn the_dead_gates_answer_nil() {
        let mods = SpellModifiers::default();
        let caster = (RangeUnit::still(0.0), Vec3::ZERO);
        let ask = |word, alive| {
            let def = SpellDisplay {
                targets: word,
                ..Default::default()
            };
            range_check(&def, Some(&HOLY_SHOCK), &mods, caster, FRIEND, None, |_| {
                Some(RangeFacts {
                    alive,
                    ..fact(FRIEND, 1.0)
                })
            })
        };
        assert_eq!(ask(WORD_ASSIST, false), None);
        assert_eq!(ask(WORD_ASSIST, true), Some(true));
        assert_eq!(ask(WORD_DEAD_ONLY | WORD_ASSIST, true), None);
        assert_eq!(ask(WORD_DEAD_ONLY | WORD_ASSIST, false), Some(true));
    }

    /// An implicit pet target with no unit word checks the pet, not the unit asked about.
    #[test]
    fn a_pet_implicit_checks_the_pet() {
        let def = SpellDisplay {
            effect_implicit_target_a: [5, 0, 0],
            ..Default::default()
        };
        let mods = SpellModifiers::default();
        let caster = (RangeUnit::still(0.0), Vec3::ZERO);
        let answer = range_check(
            &def,
            Some(&HOLY_SHOCK),
            &mods,
            caster,
            FOE,
            Some(FRIEND),
            |guid| Some(fact(guid, if guid == FRIEND { 10.0 } else { 90.0 })),
        );
        assert_eq!(answer, Some(true));
    }
}
