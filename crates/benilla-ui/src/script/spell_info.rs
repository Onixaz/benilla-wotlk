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
    /// `GetCastTime` as an unsigned dword, 0 under `Attributes & 0x40`.
    pub cast_ms: u32,
    /// `(minRange, maxRange)`; `None` without the player or the caster, which ends the returns
    /// at seven.
    pub range: Option<(f32, f32)>,
}

/// The app's answer for one spell id, against the player, or with `pet` against its pet.
pub trait SpellInfoSource {
    fn spell_info(&self, spell_id: u32, pet: bool) -> Option<SpellInfo>;

    /// `Spell_C_IsCurrentSpell` (`0x806030`) for `spell_id`, the player's cast state as the call
    /// is made.
    fn is_current_spell(&self, _spell_id: u32) -> bool {
        false
    }
}

impl super::UiScript {
    /// Install the source `GetSpellInfo` reads.
    pub fn set_spell_info_source(&mut self, source: Rc<dyn SpellInfoSource>) {
        self.model_mut().spell_info = Some(source);
    }
}

/// `GetSlotFromLua`'s spell (`0x540670`), `Ok(None)` for a name the book lacks: a number and a
/// string are a book slot below 1024, else the binding raises; a string is a book name, with the
/// second argument its subtext unless it names a unit. The error names the binding, `verb`.
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

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
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
