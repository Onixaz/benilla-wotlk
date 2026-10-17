//! 3.3.5a's character-sheet verbs, the stat panes' feed past the 1.12 set in [`super::char_stats`]:
//! the combat ratings, the crit chances, spell power, regeneration and expertise, read from the
//! player's combat-stats snapshot. As in the reference, the player-implicit verbs answer 0 before
//! the snapshot streams, and the unit-taking ones answer for the player or the pet and 0 for any
//! other token, the gate `IsActivePlayer || UNIT_FIELD_PETNUMBER != 0` (`0x60e130`).

use mlua::{Lua, Value};

use super::char_stats::UnitCombatStats;

/// The 25 combat ratings; a 1-based index past them raises (`0x60ddb0`, `cmp 0x18`).
const COMBAT_RATINGS: i64 = 25;

/// Our own wording for the reference's raises; its strings are not carried.
const RATING_INDEX_ERROR: &str = "rating index must be 1 .. 25";
const SCHOOL_ERROR: &str = "spell school must be 1 .. 7";
const UNIT_ERROR: &str = "expected a unit token";
const STAT_ERROR: &str = "expected a stat index and a stat value";

fn player_stats<T>(lua: &Lua, absent: T, f: impl FnOnce(&UnitCombatStats) -> T) -> T {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    model.player_combat_stats.as_ref().map_or(absent, f)
}

/// The player's or the pet's snapshot behind a unit-taking verb; any other token is absent.
fn unit_stats<T>(lua: &Lua, token: &str, absent: T, f: impl FnOnce(&UnitCombatStats) -> T) -> T {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let pushed = if token.eq_ignore_ascii_case("player") {
        model.player_combat_stats.as_ref()
    } else if token.eq_ignore_ascii_case("pet") {
        model.pet_combat_stats.as_ref()
    } else {
        None
    };
    pushed.map_or(absent, f)
}

/// A 1-based rating index as `GetCombatRating` takes it: a number rounded by `fistp`, then range
/// checked; anything else raises.
fn rating_index(lua: &Lua, v: Value) -> mlua::Result<usize> {
    let n = lua
        .coerce_number(v)?
        .ok_or_else(|| mlua::Error::runtime(RATING_INDEX_ERROR))?;
    let i = n.round_ties_even() as i64 - 1;
    if !(0..COMBAT_RATINGS).contains(&i) {
        return Err(mlua::Error::runtime(RATING_INDEX_ERROR));
    }
    Ok(i as usize)
}

/// A 1-based school as `GetSpellCritChance` takes it: `lua_tonumber` then `_ftol2` (`0x88b9c0`),
/// so a non-number is school 0 and raises.
fn school_index(lua: &Lua, v: Value) -> mlua::Result<usize> {
    let n = lua.coerce_number(v)?.unwrap_or(0.0);
    let i = n as i64 - 1;
    if !(0..7).contains(&i) {
        return Err(mlua::Error::runtime(SCHOOL_ERROR));
    }
    Ok(i as usize)
}

/// `CGPlayer_C::GetAttackPowerContributionFromStat` (`0x6d7070`): strength gives two attack power
/// per point past 10, or one for a class whose damage bonus stat is agility; agility gives one per
/// point past 10 to that class, plus the whole value in a form with flag `0x20`.
fn attack_power_for_stat(s: &UnitCombatStats, stat: i32, value: i32) -> i32 {
    let Some(bonus_stat) = s.damage_bonus_stat else {
        return 0;
    };
    let agility_class = bonus_stat == 1;
    let past_ten = (value - 10).max(0);
    match stat {
        0 if agility_class => past_ten,
        0 => past_ten * 2,
        1 => {
            let base = if agility_class { past_ten } else { 0 };
            base + if s.form_agility_attack_power {
                value
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// Register the 3.3.5a sheet verbs.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetCombatRating(index) → the rating's points (`+0x10ec`).
    g.set(
        "GetCombatRating",
        lua.create_function(|lua, i: Value| {
            let i = rating_index(lua, i)?;
            Ok(player_stats(lua, 0, |s| i64::from(s.combat_ratings[i])))
        })?,
    )?;

    // GetCombatRatingBonus(index) → the rating's percent bonus (`0x60de70`).
    g.set(
        "GetCombatRatingBonus",
        lua.create_function(|lua, i: Value| {
            let i = rating_index(lua, i)?;
            Ok(player_stats(lua, 0.0, |s| {
                f64::from(s.combat_rating_bonus[i])
            }))
        })?,
    )?;

    // GetMaxCombatRatingBonus(index) → the cap: the three crit-taken ratings (0-based 14..=16)
    // answer the client's constant, every other -1 (`0x6082c0`).
    g.set(
        "GetMaxCombatRatingBonus",
        lua.create_function(|lua, i: Value| {
            let i = rating_index(lua, i)?;
            Ok(if (14..=16).contains(&i) {
                f64::from_bits(0x4040_8000_0b00_0000)
            } else {
                -1.0
            })
        })?,
    )?;

    // GetCritChance(), GetRangedCritChance() → the player's percent (`+0xdc4`, `+0xdc8`).
    g.set(
        "GetCritChance",
        lua.create_function(|lua, ()| Ok(player_stats(lua, 0.0, |s| f64::from(s.crit_percent))))?,
    )?;
    g.set(
        "GetRangedCritChance",
        lua.create_function(|lua, ()| {
            Ok(player_stats(lua, 0.0, |s| f64::from(s.ranged_crit_percent)))
        })?,
    )?;

    // GetSpellCritChance(school) → that school's percent (`+0xdd0 + 4 * (school - 1)`).
    g.set(
        "GetSpellCritChance",
        lua.create_function(|lua, school: Value| {
            let i = school_index(lua, school)?;
            Ok(player_stats(lua, 0.0, |s| {
                f64::from(s.spell_crit_percent[i])
            }))
        })?,
    )?;

    // GetSpellBonusDamage(school) → the school's positive plus negative damage mod (`0x60e310`).
    g.set(
        "GetSpellBonusDamage",
        lua.create_function(|lua, school: Value| {
            let i = school_index(lua, school)?;
            Ok(player_stats(lua, 0, |s| i64::from(s.spell_damage_bonus[i])))
        })?,
    )?;

    // GetSpellBonusHealing() → `+0x1050`.
    g.set(
        "GetSpellBonusHealing",
        lua.create_function(|lua, ()| {
            Ok(player_stats(lua, 0, |s| i64::from(s.spell_healing_bonus)))
        })?,
    )?;

    // GetSpellPenetration() → `-[+0x105c]`, the target resistance mod negated.
    g.set(
        "GetSpellPenetration",
        lua.create_function(|lua, ()| {
            Ok(player_stats(lua, 0, |s| {
                -i64::from(s.target_resistance_mod)
            }))
        })?,
    )?;

    // GetArmorPenetration() → rating 24's bonus, at most 100 (`0x60e4e0`).
    g.set(
        "GetArmorPenetration",
        lua.create_function(|lua, ()| {
            Ok(player_stats(lua, 0.0, |s| {
                let bonus = f64::from(s.combat_rating_bonus[24]);
                if bonus <= 100.0 {
                    bonus
                } else {
                    100.0
                }
            }))
        })?,
    )?;

    // GetShieldBlock() → `+0xdec`, unsigned.
    g.set(
        "GetShieldBlock",
        lua.create_function(|lua, ()| Ok(player_stats(lua, 0, |s| i64::from(s.shield_block))))?,
    )?;

    // GetExpertise() → main hand, off hand (`+0xdbc`, `+0xdc0`, unsigned).
    g.set(
        "GetExpertise",
        lua.create_function(|lua, ()| {
            Ok(player_stats(lua, (0, 0), |s| {
                (i64::from(s.expertise[0]), i64::from(s.expertise[1]))
            }))
        })?,
    )?;

    // GetExpertisePercent() → each hand's expertise as a float × 0.25 (`0x612cb0`).
    g.set(
        "GetExpertisePercent",
        lua.create_function(|lua, ()| {
            Ok(player_stats(lua, (0.0, 0.0), |s| {
                let pct = |e: u32| f64::from(e as i32 as f32 * 0.25);
                (pct(s.expertise[0]), pct(s.expertise[1]))
            }))
        })?,
    )?;

    // GetManaRegen() → (base, casting): a mana user's regen floats + 0.001, else 0 and 0
    // (`0x612a90`).
    g.set(
        "GetManaRegen",
        lua.create_function(|lua, ()| {
            Ok(player_stats(lua, (0.0, 0.0), |s| {
                s.mana_regen.map_or((0.0, 0.0), |[base, casting]| {
                    (f64::from(base) + 0.001, f64::from(casting) + 0.001)
                })
            }))
        })?,
    )?;

    // GetAttackPowerForStat(stat, value) → the attack power `value` points of 0-based stat
    // `stat - 1` give (`0x60e560`); both arguments must be numbers.
    g.set(
        "GetAttackPowerForStat",
        lua.create_function(|lua, (stat, value): (Value, Value)| {
            let (Some(stat), Some(value)) = (lua.coerce_number(stat)?, lua.coerce_number(value)?)
            else {
                return Err(mlua::Error::runtime(STAT_ERROR));
            };
            Ok(player_stats(lua, 0, |s| {
                i64::from(attack_power_for_stat(s, stat as i32 - 1, value as i32))
            }))
        })?,
    )?;

    // GetPetSpellBonusDamage() (`0x60e410`): the player's `PLAYER_PET_SPELL_POWER`, 0 without
    // a player.
    g.set(
        "GetPetSpellBonusDamage",
        lua.create_function(|lua, ()| Ok(player_stats(lua, 0, |s| i64::from(s.pet_spell_power))))?,
    )?;

    // The unit-taking verbs: the player or the pet, 0 for anything else; a non-string raises.
    // `GetUnitHealthModifier` (`0x6127f0`) and `GetUnitPowerModifier` (`0x612900`) answer the
    // player or a unit with a pet number. Deviation: a party member's pet reads 0, as no snapshot
    // of it is pushed.
    for (name, pick) in [
        ("GetCritChanceFromAgility", 0u8),
        ("GetSpellCritChanceFromIntellect", 1),
        ("GetUnitHealthRegenRateFromSpirit", 2),
        ("GetUnitManaRegenRateFromSpirit", 3),
        ("GetUnitMaxHealthModifier", 4),
        ("GetUnitHealthModifier", 5),
        ("GetUnitPowerModifier", 6),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, unit: Value| {
                let token = super::binding_abi::string_arg(lua, unit, UNIT_ERROR)?;
                Ok(unit_stats(lua, &token, 0.0, |s| match pick {
                    0 => f64::from(s.crit_from_agility),
                    1 => f64::from(s.spell_crit_from_intellect),
                    2 => f64::from(s.health_regen_from_spirit),
                    // `0x612a00` adds 0.001 to the rate.
                    3 => f64::from(s.mana_regen_from_spirit) + 0.001,
                    // `0x612870` adds 1.0 to the modifier, in f32.
                    4 => f64::from(s.max_health_modifier + 1.0),
                    5 => f64::from(s.health_modifier),
                    _ => f64::from(s.power_modifier),
                }))
            })?,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::attack_power_for_stat;
    use crate::script::{UiScript, UnitCombatStats};

    fn sheet() -> UnitCombatStats {
        let mut s = UnitCombatStats {
            crit_percent: 5.5,
            ranged_crit_percent: 4.25,
            spell_healing_bonus: 410,
            target_resistance_mod: -35,
            shield_block: 95,
            expertise: [12, 7],
            mana_regen: Some([10.0, 2.5]),
            damage_bonus_stat: Some(0),
            crit_from_agility: 1.5,
            spell_crit_from_intellect: 2.25,
            health_regen_from_spirit: 3.0,
            mana_regen_from_spirit: 4.0,
            max_health_modifier: 0.5,
            ..Default::default()
        };
        s.spell_crit_percent[1] = 6.75;
        s.spell_damage_bonus[2] = 120;
        s.combat_ratings[5] = 18;
        s.combat_rating_bonus[5] = 0.5;
        s.combat_rating_bonus[24] = 140.0;
        s
    }

    #[test]
    fn every_verb_answers_zero_before_the_snapshot_streams() {
        let s = UiScript::new().unwrap();
        for call in [
            "GetCombatRating(6)",
            "GetCombatRatingBonus(6)",
            "GetCritChance()",
            "GetRangedCritChance()",
            "GetSpellCritChance(2)",
            "GetSpellBonusDamage(2)",
            "GetSpellBonusHealing()",
            "GetSpellPenetration()",
            "GetArmorPenetration()",
            "GetShieldBlock()",
            "GetAttackPowerForStat(1, 50)",
            r#"GetCritChanceFromAgility("player")"#,
            r#"GetUnitMaxHealthModifier("player")"#,
        ] {
            assert_eq!(
                s.eval::<f64>(&format!("return {call}")).unwrap(),
                0.0,
                "{call}"
            );
        }
        assert_eq!(
            s.eval::<(f64, f64)>("return GetManaRegen()").unwrap(),
            (0.0, 0.0)
        );
        assert_eq!(
            s.eval::<(i64, i64)>("return GetExpertise()").unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn the_player_implicit_verbs_read_the_snapshot() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(sheet()));
        assert_eq!(s.eval::<i64>("return GetCombatRating(6)").unwrap(), 18);
        // The index rounds to nearest: 5.6 is rating 6.
        assert_eq!(s.eval::<i64>("return GetCombatRating(5.6)").unwrap(), 18);
        assert_eq!(
            s.eval::<f64>("return GetCombatRatingBonus(6)").unwrap(),
            0.5
        );
        assert_eq!(s.eval::<f64>("return GetCritChance()").unwrap(), 5.5);
        assert_eq!(s.eval::<f64>("return GetRangedCritChance()").unwrap(), 4.25);
        assert_eq!(s.eval::<f64>("return GetSpellCritChance(2)").unwrap(), 6.75);
        assert_eq!(s.eval::<i64>("return GetSpellBonusDamage(3)").unwrap(), 120);
        assert_eq!(s.eval::<i64>("return GetSpellBonusHealing()").unwrap(), 410);
        assert_eq!(s.eval::<i64>("return GetSpellPenetration()").unwrap(), 35);
        // Capped at 100.
        assert_eq!(
            s.eval::<f64>("return GetArmorPenetration()").unwrap(),
            100.0
        );
        assert_eq!(s.eval::<i64>("return GetShieldBlock()").unwrap(), 95);
        assert_eq!(
            s.eval::<(i64, i64)>("return GetExpertise()").unwrap(),
            (12, 7)
        );
        assert_eq!(
            s.eval::<(f64, f64)>("return GetExpertisePercent()")
                .unwrap(),
            (3.0, 1.75)
        );
        assert_eq!(
            s.eval::<(f64, f64)>("return GetManaRegen()").unwrap(),
            (10.001, 2.501)
        );
        // A rage user's regen is 0 and 0.
        s.set_player_combat_stats(Some(UnitCombatStats {
            mana_regen: None,
            ..sheet()
        }));
        assert_eq!(
            s.eval::<(f64, f64)>("return GetManaRegen()").unwrap(),
            (0.0, 0.0)
        );
    }

    #[test]
    fn the_unit_verbs_answer_for_the_player_and_the_pet_only() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(sheet()));
        assert_eq!(
            s.eval::<f64>(r#"return GetCritChanceFromAgility("player")"#)
                .unwrap(),
            1.5
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetSpellCritChanceFromIntellect("player")"#)
                .unwrap(),
            2.25
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetUnitHealthRegenRateFromSpirit("player")"#)
                .unwrap(),
            3.0
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetUnitManaRegenRateFromSpirit("player")"#)
                .unwrap(),
            4.001
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetUnitMaxHealthModifier("player")"#)
                .unwrap(),
            1.5
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetCritChanceFromAgility("target")"#)
                .unwrap(),
            0.0
        );
        assert_eq!(
            s.eval::<f64>(r#"return GetCritChanceFromAgility("pet")"#)
                .unwrap(),
            0.0
        );
        s.set_pet_combat_stats(Some(UnitCombatStats {
            crit_from_agility: 7.0,
            ..Default::default()
        }));
        assert_eq!(
            s.eval::<f64>(r#"return GetCritChanceFromAgility("pet")"#)
                .unwrap(),
            7.0
        );
        assert!(s
            .eval::<f64>("return GetCritChanceFromAgility(nil)")
            .is_err());
    }

    #[test]
    fn out_of_range_indices_raise() {
        let mut s = UiScript::new().unwrap();
        s.set_player_combat_stats(Some(sheet()));
        for call in [
            "GetCombatRating(0)",
            "GetCombatRating(26)",
            "GetCombatRating('x')",
            "GetCombatRatingBonus(0)",
            "GetMaxCombatRatingBonus(26)",
            "GetSpellCritChance(0)",
            "GetSpellCritChance(8)",
            "GetSpellBonusDamage(nil)",
            "GetAttackPowerForStat(1)",
        ] {
            assert!(s.eval::<f64>(&format!("return {call}")).is_err(), "{call}");
        }
        assert_eq!(
            s.eval::<f64>("return GetMaxCombatRatingBonus(1)").unwrap(),
            -1.0
        );
        assert!(s.eval::<f64>("return GetMaxCombatRatingBonus(16)").unwrap() > 0.0);
    }

    #[test]
    fn attack_power_from_a_stat_follows_the_class_s_damage_bonus_stat() {
        let strength_class = UnitCombatStats {
            damage_bonus_stat: Some(0),
            ..Default::default()
        };
        let agility_class = UnitCombatStats {
            damage_bonus_stat: Some(1),
            ..Default::default()
        };
        // Strength: two per point past 10, or one for an agility class; never below 0.
        assert_eq!(attack_power_for_stat(&strength_class, 0, 50), 80);
        assert_eq!(attack_power_for_stat(&agility_class, 0, 50), 40);
        assert_eq!(attack_power_for_stat(&strength_class, 0, 4), 0);
        // Agility: one per point past 10 for an agility class only.
        assert_eq!(attack_power_for_stat(&agility_class, 1, 50), 40);
        assert_eq!(attack_power_for_stat(&strength_class, 1, 50), 0);
        // A form with flag 0x20 adds the whole value.
        let cat = UnitCombatStats {
            form_agility_attack_power: true,
            ..strength_class.clone()
        };
        assert_eq!(attack_power_for_stat(&cat, 1, 50), 50);
        // Stamina and the rest give none; no class row gives none.
        assert_eq!(attack_power_for_stat(&strength_class, 2, 50), 0);
        assert_eq!(attack_power_for_stat(&UnitCombatStats::default(), 0, 50), 0);
    }
}
