//! The spellbook's per-slot predicates and lookups no stock file calls: `FindSpellBookSlotByID`,
//! `GetSpellCount`, and `IsAttackSpell`, `IsAutoRepeatSpell`, `IsConsumableSpell`,
//! `IsHelpfulSpell` and `IsHarmfulSpell`. Each but the first takes a book slot and book type or a
//! spell name through the slot parser (`0x540670`), returns nothing when that fails, and reads the
//! slot's spell, 0 past the book's spells.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, number_arg};
use super::item_search::held_count;
use super::spell_info::{spell_or_zero, SpellStatEffects};

/// `Spell.dbc`'s `Reagent` slot the reagent-uses count skips besides 0 (`0x801770`).
const REAGENT_SKIP: u32 = -2i32 as u32;

/// `SPELL_EFFECT_ATTACK`, the melee auto-attack's first effect.
const EFFECT_ATTACK: u32 = 78;

/// The spell's row as the predicates read it, `None` for spell 0 or no row.
fn row(lua: &Lua, spell_id: u32) -> Option<SpellStatEffects> {
    let source = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .spell_info
        .clone()?;
    source.spell_stat_effects(spell_id)
}

/// A predicate over the slot's row: 1 or nil, nothing for a failed parse.
fn row_predicate(
    lua: &Lua,
    args: &MultiValue,
    verb: &str,
    test: impl Fn(&SpellStatEffects) -> bool,
) -> mlua::Result<MultiValue> {
    let Some((spell_id, _)) = spell_or_zero(lua, args, verb)? else {
        return Ok(MultiValue::new());
    };
    let hit = row(lua, spell_id).is_some_and(|r| test(&r));
    Ok(MultiValue::from_vec(vec![flag(hit)]))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `FindSpellBookSlotByID(spellID[, isPet])` (`0x53b6c0`): the 0-based index of the last book
    // slot holding the spell (`0x53b4e0` searches from the end), one below the slot the book
    // verbs take, or nil. A non-number or an id below 1 raises.
    g.set(
        "FindSpellBookSlotByID",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: FindSpellBookSlotByID(spellID[, isPet])";
            let id = number_arg(lua, args.front().cloned().unwrap_or(Value::Nil), USAGE)?;
            if id <= 0 {
                return Err(mlua::Error::runtime(USAGE));
            }
            let pet = bool_or_default(args.get(1), false);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // The pet's book is read at the call, as every pet book verb reads it.
            let pet_book;
            let slots = if pet {
                pet_book = super::spellbook::live_pet_book(&model);
                &pet_book.slots
            } else {
                &model.spellbook.slots
            };
            Ok(slots
                .iter()
                .rposition(|s| s.spell_id == id as u32)
                .map_or(Value::Nil, |i| Value::Integer(i as i64)))
        })?,
    )?;

    // `GetSpellCount(slot, bookType | "name")` (`0x540df0`): how many casts the carried reagents
    // pay for (`0x801770`), the least over each reagent of the bags' count over the count a cast
    // takes; 0 for a spell with none.
    g.set(
        "GetSpellCount",
        lua.create_function(|lua, args: MultiValue| {
            let Some((spell_id, _)) = spell_or_zero(lua, &args, "GetSpellCount")? else {
                return Ok(MultiValue::new());
            };
            let uses = row(lua, spell_id).map_or(0, |r| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                r.reagents
                    .iter()
                    .filter(|&&(id, n)| id != 0 && id != REAGENT_SKIP && n as i32 > 0)
                    .map(|&(id, n)| held_count(&model, id, false, false) / n)
                    .min()
                    .unwrap_or(0)
            });
            Ok(MultiValue::from_vec(vec![Value::Number(f64::from(uses))]))
        })?,
    )?;

    // `IsAttackSpell` (`0x541420`): the first effect is the auto-attack.
    g.set(
        "IsAttackSpell",
        lua.create_function(|lua, args: MultiValue| {
            row_predicate(lua, &args, "IsAttackSpell", |r| {
                r.effects[0].effect == EFFECT_ATTACK
            })
        })?,
    )?;
    // `IsConsumableSpell` (`0x5419f0`): a reagent with a positive count.
    g.set(
        "IsConsumableSpell",
        lua.create_function(|lua, args: MultiValue| {
            row_predicate(lua, &args, "IsConsumableSpell", |r| r.has_reagents)
        })?,
    )?;
    // `IsHelpfulSpell` (`0x541800`) and `IsHarmfulSpell` (`0x5418f0`): the helpful-or-harmful test
    // (`0x7fe1b0`) answering 1 or 2.
    g.set(
        "IsHelpfulSpell",
        lua.create_function(|lua, args: MultiValue| {
            row_predicate(lua, &args, "IsHelpfulSpell", |r| r.helpful)
        })?,
    )?;
    g.set(
        "IsHarmfulSpell",
        lua.create_function(|lua, args: MultiValue| {
            row_predicate(lua, &args, "IsHarmfulSpell", |r| r.harmful)
        })?,
    )?;

    // `IsAutoRepeatSpell` (`0x5415d0`): a player-book spell, not 0, that is the auto-repeating or
    // auto-ranged-combat spell; a pet slot is always nil.
    g.set(
        "IsAutoRepeatSpell",
        lua.create_function(|lua, args: MultiValue| {
            let Some((spell_id, pet)) = spell_or_zero(lua, &args, "IsAutoRepeatSpell")? else {
                return Ok(MultiValue::new());
            };
            let source = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .spell_info
                .clone();
            let hit =
                spell_id != 0 && !pet && source.is_some_and(|s| s.is_auto_repeat_spell(spell_id));
            Ok(MultiValue::from_vec(vec![flag(hit)]))
        })?,
    )?;
    Ok(())
}
