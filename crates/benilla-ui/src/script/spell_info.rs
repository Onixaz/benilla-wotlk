//! `GetSpellInfo(spellId | slot, bookType | "name"[, subtext])` (`0x540a30`): any `Spell.dbc`
//! row by id, or a book spell by slot or name, as name, rank, icon, cost, isFunnel, powerType,
//! castTime and, with a caster, minRange and maxRange. The app answers each id synchronously
//! through a [`SpellInfoSource`], since a row's cost, cast time and range read the caster's
//! state as the call is made.

use std::rc::Rc;

use mlua::{Lua, MultiValue, Value};

use super::spellbook::{book_slot, resolve_spell_by_name, BOOKTYPE_PET, BOOKTYPE_SPELL};

/// One spell as `GetSpellInfo` returns it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellInfo {
    pub name: String,
    /// `NameSubtext`, "" for none.
    pub rank: String,
    /// The melee auto-attack's weapon icon, a ranged shot's ranged-weapon icon, else the
    /// `SpellIcon.dbc` path or ""; `None` where the weapon lookup has nothing, which pushes nil.
    pub texture: Option<String>,
    /// The cost in display units, or the per-second cost for a funnel.
    pub cost: i64,
    pub funnel: bool,
    pub power_type: i64,
    /// The spell's cast time as an unsigned dword, 0 under `Attributes & 0x40`.
    pub cast_ms: u32,
    /// `(minRange, maxRange)`; `None` without the player or the caster, which ends the returns
    /// at seven.
    pub range: Option<(f32, f32)>,
}

/// One spell effect as the item stat summary reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpellStatEffect {
    pub effect: u32,
    pub aura: u32,
    pub misc_value: i32,
    pub base_points: i32,
}

/// A spell's stat-bearing fields (`0x61b5e0`), and whether it takes a reagent (`0x5173e0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpellStatEffects {
    pub attributes: u32,
    pub effects: [SpellStatEffect; 3],
    pub has_reagents: bool,
    /// The first effect's created item, a recipe's product (`0x598830`).
    pub item_type: u32,
    /// The helpful-or-harmful test's two answers (`0x7fe1b0`).
    pub helpful: bool,
    pub harmful: bool,
}

/// The app's answer for one spell id, against the player, or with `pet` against its pet.
pub trait SpellInfoSource {
    fn spell_info(&self, spell_id: u32, pet: bool) -> Option<SpellInfo>;

    /// The current-spell test (`0x806030`) for `spell_id`, the player's cast state as the call is
    /// made.
    fn is_current_spell(&self, _spell_id: u32) -> bool {
        false
    }

    /// The range lookup with no target answers a nonzero bound (`0x541af0`): the player's, or with
    /// `pet` the pet's.
    fn spell_has_range(&self, _spell_id: u32, _pet: bool) -> bool {
        false
    }

    /// The range check (`0x809610`) toward `target`: `None` where the spell cannot take that unit.
    fn spell_in_range(&self, _spell_id: u32, _pet: bool, _target: u64) -> Option<bool> {
        None
    }

    /// The spell's stat-bearing fields, `None` for no row.
    fn spell_stat_effects(&self, _spell_id: u32) -> Option<SpellStatEffects> {
        None
    }

    /// The spell a talent's `rank` teaches, 0-based, and the talent's rank count.
    fn talent_rank_spell(&self, _talent_id: u32, _rank: i32) -> Option<(u32, u32)> {
        None
    }

    /// `GetSpellLink`'s reads (`0x5408e0`): the name, and the `|Htrade:` link of a linkable
    /// trade-skill opener; `skill` answers a line's rank and max. `None` for a row with no link.
    fn spell_link(
        &self,
        _spell_id: u32,
        _skill: &dyn Fn(u32) -> (u32, u32),
    ) -> Option<(String, Option<String>)> {
        None
    }
}

impl super::UiScript {
    /// Install the source `GetSpellInfo` reads.
    pub fn set_spell_info_source(&mut self, source: Rc<dyn SpellInfoSource>) {
        self.model_mut().spell_info = Some(source);
    }
}

/// The Lua spell-argument resolver's spell (`0x540670`), `Ok(None)` for a name the book lacks: a
/// number and a string are a book slot below 1024, else the binding raises; a string is a book
/// name, with the second argument its subtext unless it names a unit. The error names the binding,
/// `verb`.
pub(super) fn spell_of(
    lua: &Lua,
    args: &MultiValue,
    verb: &str,
) -> mlua::Result<Option<(u32, bool)>> {
    let invalid = || mlua::Error::runtime(format!("{verb}(): Invalid spell slot"));
    let first = args.front().cloned().unwrap_or(Value::Nil);
    let second = args.get(1).cloned().unwrap_or(Value::Nil);
    let is_string =
        |v: &Value| matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_));
    let number = lua.coerce_number(first.clone())?;
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    if let (Some(n), true) = (number, is_string(&second)) {
        let slot = super::binding_abi::ftol(n - 1.0) as u32;
        if slot > 1023 {
            return Err(invalid());
        }
        let book = lua
            .coerce_string(second)?
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let pet = book.eq_ignore_ascii_case(BOOKTYPE_PET);
        let book = if pet { BOOKTYPE_PET } else { BOOKTYPE_SPELL };
        return Ok(book_slot(&model, slot + 1, book).map(|s| (s.spell_id, pet)));
    }
    if !is_string(&first) {
        return Err(invalid());
    }
    let name = lua
        .coerce_string(first)?
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let subtext = match &second {
        Value::String(_) | Value::Integer(_) | Value::Number(_) => {
            let s = lua
                .coerce_string(second)?
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            (super::parse_unit_token(&s) == super::UnitTokenParse::Unknown).then_some(s)
        }
        _ => None,
    };
    let query = match subtext {
        Some(sub) => format!("{name}({sub})"),
        None => name,
    };
    Ok(resolve_spell_by_name(&model.spellbook, &query).map(|s| (s.spell_id, false)))
}

/// The argument the slot parser (`0x540670`) leaves the unit at, 1-based: 3 after a book slot or a
/// name with a subtext, else 2.
fn unit_arg(lua: &Lua, args: &MultiValue) -> mlua::Result<Value> {
    let first = args.front().cloned().unwrap_or(Value::Nil);
    let second = args.get(1).cloned().unwrap_or(Value::Nil);
    let is_string =
        |v: &Value| matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_));
    let third = if lua.coerce_number(first)?.is_some() && is_string(&second) {
        true
    } else if is_string(&second) {
        let s = lua
            .coerce_string(second.clone())?
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        super::parse_unit_token(&s) == super::UnitTokenParse::Unknown
    } else {
        false
    };
    Ok(if third {
        args.get(2).cloned().unwrap_or(Value::Nil)
    } else {
        second
    })
}

/// The guid a range verb's unit argument names, `None` for a token naming nobody.
pub(super) fn range_unit(lua: &Lua, unit: &Value) -> Option<u64> {
    let token = super::binding_abi::optional_string(lua, unit)?;
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    model.unit_guids.resolve(&token).ok().flatten()
}

/// A range answer as Lua reads it: 1, 0, or nil where the check does not apply.
pub(super) fn range_answer(answer: Option<bool>) -> Value {
    answer.map_or(Value::Nil, |near| Value::Integer(i64::from(near)))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // SpellHasRange(spell [, book]) (`0x541af0`): 1 or nil.
    lua.globals().set(
        "SpellHasRange",
        lua.create_function(|lua, args: MultiValue| {
            let Some((spell, pet)) = spell_of(lua, &args, "SpellHasRange")? else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let source = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .spell_info
                .clone();
            let has = source.is_some_and(|s| s.spell_has_range(spell, pet));
            let out = if has { Value::Integer(1) } else { Value::Nil };
            Ok(MultiValue::from_vec(vec![out]))
        })?,
    )?;

    // IsSpellInRange(spell, [book,] unit) (`0x541c60`): 1, 0, or nil where the spell cannot take
    // the unit.
    lua.globals().set(
        "IsSpellInRange",
        lua.create_function(|lua, args: MultiValue| {
            let Some((spell, pet)) = spell_of(lua, &args, "IsSpellInRange")? else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let target = range_unit(lua, &unit_arg(lua, &args)?);
            let source = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .spell_info
                .clone();
            let near = source
                .zip(target)
                .and_then(|(s, guid)| s.spell_in_range(spell, pet, guid));
            Ok(MultiValue::from_vec(vec![range_answer(near)]))
        })?,
    )?;

    // GetSpellLink(id | slot, bookType | "name") (`0x5408e0`): the link and the trade link, nil
    // for each without; an unknown name answers nothing.
    lua.globals().set(
        "GetSpellLink",
        lua.create_function(|lua, args: MultiValue| {
            let by_id = args.len() == 1 && lua.coerce_number(args[0].clone())?.is_some();
            let spell = if by_id {
                let n = lua.coerce_number(args[0].clone())?.unwrap_or(0.0);
                Some((super::binding_abi::ftol(n) as u32, false))
            } else {
                spell_of(lua, &args, "GetSpellLink")?
            };
            let Some((spell_id, _)) = spell else {
                return Ok(MultiValue::new());
            };
            let (source, skills) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                (model.spell_info.clone(), model.skills.entries.clone())
            };
            let skill = |line: u32| {
                skills
                    .iter()
                    .find(|e| e.skill_id == line)
                    .map_or((0, 0), |e| (e.value, e.max))
            };
            let Some((name, trade)) = source.and_then(|s| s.spell_link(spell_id, &skill)) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Nil]));
            };
            let link = format!("|cff71d5ff|Hspell:{spell_id}|h[{name}]|h|r");
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&link)?),
                match trade {
                    Some(t) => Value::String(lua.create_string(&t)?),
                    None => Value::Nil,
                },
            ]))
        })?,
    )?;

    lua.globals().set(
        "GetSpellInfo",
        lua.create_function(|lua, args: MultiValue| {
            let by_id = args.len() == 1
                && matches!(
                    args.front(),
                    Some(Value::Integer(_) | Value::Number(_) | Value::String(_))
                )
                && lua.coerce_number(args[0].clone())?.is_some();
            let spell = if by_id {
                let n = lua.coerce_number(args[0].clone())?.unwrap_or(0.0);
                Some((super::binding_abi::ftol(n) as u32, false))
            } else {
                spell_of(lua, &args, "GetSpellInfo")?
            };
            let Some((spell_id, pet)) = spell else {
                return Ok(MultiValue::new());
            };
            let source = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.spell_info.clone()
            };
            let Some(info) = source.and_then(|s| s.spell_info(spell_id, pet)) else {
                return Ok(MultiValue::new());
            };
            let text =
                |s: &str| -> mlua::Result<Value> { Ok(Value::String(lua.create_string(s)?)) };
            let mut out = vec![
                text(&info.name)?,
                text(&info.rank)?,
                match &info.texture {
                    Some(t) => text(t)?,
                    None => Value::Nil,
                },
                Value::Integer(info.cost),
                Value::Boolean(info.funnel),
                Value::Integer(info.power_type),
                Value::Integer(i64::from(info.cast_ms)),
            ];
            if let Some((min, max)) = info.range {
                out.push(Value::Number(f64::from(min)));
                out.push(Value::Number(f64::from(max)));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{SpellBookState, SpellSlotView, SpellTabView, UiScript};

    struct Fixed;

    impl SpellInfoSource for Fixed {
        fn spell_info(&self, spell_id: u32, pet: bool) -> Option<SpellInfo> {
            (spell_id == 133 || spell_id == 17).then(|| SpellInfo {
                name: if pet { "Bite" } else { "Fireball" }.into(),
                rank: "Rank 1".into(),
                texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
                cost: 30,
                power_type: 0,
                cast_ms: 1500,
                range: (spell_id == 133).then_some((0.0, 35.0)),
                ..Default::default()
            })
        }

        fn is_current_spell(&self, spell_id: u32) -> bool {
            spell_id == 63645 || spell_id == 133
        }

        fn spell_has_range(&self, spell_id: u32, _pet: bool) -> bool {
            spell_id == 133
        }

        fn spell_in_range(&self, spell_id: u32, _pet: bool, target: u64) -> Option<bool> {
            (spell_id == 133 && target != ALLY).then_some(target == NEAR)
        }
    }

    const NEAR: u64 = 7;
    const FAR: u64 = 8;
    const ALLY: u64 = 9;

    /// The argument forms of the slot parser: a name then a unit, a book slot, then a book name,
    /// then a unit. A token naming nobody and a unit the spell cannot take answer nil.
    #[test]
    fn the_spell_range_verbs_read_the_unit_past_the_slot() {
        let mut s = script();
        s.set_unit_guids(&crate::script::UnitGuids {
            target: NEAR,
            pet: FAR,
            player: ALLY,
            ..Default::default()
        });
        let is = |call: &str, want: &str| {
            assert!(
                s.eval::<bool>(&format!("return {call} == {want}")).unwrap(),
                "{call}"
            );
        };
        is("IsSpellInRange('Fireball', 'target')", "1");
        is("IsSpellInRange('Fireball', 'pet')", "0");
        is("IsSpellInRange(1, 'spell', 'target')", "1");
        is("IsSpellInRange(1, 'spell', 'pet')", "0");
        is("IsSpellInRange('Fireball', 'Rank 1', 'pet')", "0");
        is("IsSpellInRange('Fireball', 'player')", "nil");
        is("IsSpellInRange('Fireball', 'party1')", "nil");
        is("SpellHasRange('Fireball')", "1");
        is("SpellHasRange(1, 'spell')", "1");
        is("SpellHasRange('Nope')", "nil");
        assert!(s.eval::<mlua::Value>("return IsSpellInRange()").is_err());
    }

    /// `IsCurrentSpell` asks the cast state by id, so a spell the book lacks answers too; a book
    /// slot or name resolves first, and a name the book lacks answers nothing.
    #[test]
    fn is_current_spell_asks_the_cast_state_by_id() {
        let s = script();
        let n = |call: &str| {
            s.eval::<i64>(&format!("return select('#', {call})"))
                .unwrap()
        };
        assert_eq!(s.eval::<i64>("return IsCurrentSpell(63645)").unwrap(), 1);
        assert_eq!(n("IsCurrentSpell(2)"), 1);
        assert!(s.eval::<bool>("return IsCurrentSpell(2) == nil").unwrap());
        assert_eq!(
            s.eval::<i64>("return IsCurrentSpell('Fireball')").unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>("return IsCurrentSpell(1, 'spell')").unwrap(),
            1
        );
        assert_eq!(n("IsCurrentSpell('Nope')"), 0);
    }

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_spell_info_source(Rc::new(Fixed));
        s.set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Fire".into(),
                offset: 0,
                num_spells: 1,
                ..Default::default()
            }],
            slots: vec![SpellSlotView {
                spell_id: 133,
                name: "Fireball".into(),
                rank: Some("Rank 1".into()),
                ..Default::default()
            }],
            ..Default::default()
        });
        s
    }

    #[test]
    fn an_id_answers_nine_or_seven_and_a_missing_row_nothing() {
        let s = script();
        let n = |s: &UiScript, call: &str| {
            s.eval::<i64>(&format!("return select('#', {call})"))
                .unwrap()
        };
        assert_eq!(n(&s, "GetSpellInfo(133)"), 9);
        assert_eq!(n(&s, "GetSpellInfo(17)"), 7, "no caster: no range");
        assert_eq!(n(&s, "GetSpellInfo(99999)"), 0);
        let got: (String, String, String, i64, bool, i64, i64, f64, f64) =
            s.eval("return GetSpellInfo(133)").unwrap();
        assert_eq!(
            got,
            (
                "Fireball".into(),
                "Rank 1".into(),
                "Interface\\Icons\\Spell_Fire_FlameBolt".into(),
                30,
                false,
                0,
                1500,
                0.0,
                35.0
            )
        );
    }

    /// A slot reads the book, a name the player's book with its rank, an unknown name nothing; a
    /// slot past the book's 1024 raises.
    #[test]
    fn a_slot_and_a_name_read_the_book() {
        let s = script();
        assert_eq!(
            s.eval::<String>("return (GetSpellInfo(1, 'spell'))")
                .unwrap(),
            "Fireball"
        );
        assert_eq!(
            s.eval::<String>("return (GetSpellInfo('Fireball'))")
                .unwrap(),
            "Fireball"
        );
        assert_eq!(
            s.eval::<String>("return (GetSpellInfo('Fireball', 'Rank 1'))")
                .unwrap(),
            "Fireball"
        );
        assert_eq!(
            s.eval::<String>("return (GetSpellInfo('Fireball', 'player'))")
                .unwrap(),
            "Fireball",
            "a unit token is the cast target, not the subtext"
        );
        assert_eq!(
            s.eval::<i64>("return select('#', GetSpellInfo('Frostbolt'))")
                .unwrap(),
            0
        );
        assert!(s.run("GetSpellInfo(2000, 'spell')").is_err());
        assert!(s.run("GetSpellInfo()").is_err());
    }

    /// A handler may ask every frame: the reads leave the mutation count standing.
    #[test]
    fn reads_leave_the_mutation_count_standing() {
        let s = script();
        let ask = "GetSpellInfo(133) GetSpellInfo(1, 'spell') GetSpellInfo('Fireball')                    GetSpellInfo('Frostbolt') GetSpellInfo(99999)";
        s.run(ask).unwrap();
        let settled = s.visual_epoch();
        s.run(ask).unwrap();
        assert_eq!(s.visual_epoch(), settled);
    }
}
