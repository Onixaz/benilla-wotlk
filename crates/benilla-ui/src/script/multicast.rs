//! `GetMultiCastTotemSpells` (`0x5a8330`), `SetMultiCastSpell` (`0x5ab8a0`), `IsSpellKnown`.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::action::ACTION_KIND_SPELL;
use super::binding_abi::number_arg;
use super::Model;

/// Action slot 133 in Lua (wire 132), the first of three pages of four element slots.
pub const FIRST_TOTEM_ACTION: u32 = 133;

/// What the app resolves for the bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiCastView {
    /// The element lists, fire, earth, water, air (`0xbe8ea0`).
    pub lists: [Vec<u32>; 4],
    /// Every spell's `TotemCategory` pair that names an element row, for the slot test.
    pub spell_categories: HashMap<u32, [u32; 2]>,
    /// The element rows' names, `ERR_MULTI_CAST_ACTION_TOTEM_S`'s argument.
    pub category_names: HashMap<u32, String>,
}

impl super::UiScript {
    /// Replace the bar's lists and tables; the app fires `UPDATE_MULTI_CAST_ACTIONBAR` itself.
    pub fn set_multicast(&mut self, view: MultiCastView) {
        set_field!(self, multicast, view);
    }

    /// Drain the keyed refusals that carry one `%s`; the app shows each.
    pub fn take_ui_errors_s(&mut self) -> Vec<(&'static str, String)> {
        drain_field!(self, ui_errors_s)
    }
}

/// The tool row a totem action asks for (`0x5a7ae0`), for Lua actions 133..144.
pub(crate) fn action_category(action: u32) -> Option<u32> {
    (FIRST_TOTEM_ACTION..FIRST_TOTEM_ACTION + 12)
        .contains(&action)
        .then(|| [4, 2, 5, 3][((action - FIRST_TOTEM_ACTION) % 4) as usize])
}

/// Whether `spell`'s tool rows hold totem `action`'s element (`0x5a7bc0`); `None` off the pages.
pub(crate) fn totem_fits(model: &Model, action: u32, spell: u32) -> Option<bool> {
    let category = action_category(action)?;
    Some(
        model
            .multicast
            .spell_categories
            .get(&spell)
            .is_some_and(|cats| cats.contains(&category)),
    )
}

/// `ERR_MULTI_CAST_ACTION_TOTEM_S` with the action's element name (`0x5ab24b`, `0x5ab98c`).
pub(crate) fn refuse_totem(model: &mut Model, action: u32) {
    let Some(category) = action_category(action) else {
        return;
    };
    if let Some(name) = model.multicast.category_names.get(&category).cloned() {
        model
            .ui_errors_s
            .push(("ERR_MULTI_CAST_ACTION_TOTEM_S", name));
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The constant 6.0 at `0xa0b708` (`0x5b71e0`): stock pages land on actions 133..144.
    g.set(
        "GetMultiCastBarOffset",
        lua.create_function(|_, ()| Ok(6.0))?,
    )?;

    // The element list of slot `(n - 1) & 3`, each id a return.
    g.set(
        "GetMultiCastTotemSpells",
        lua.create_function(|lua, v: Value| {
            let n = number_arg(lua, v, "Usage: GetMultiCastTotemSpells(slot)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let list = &model.multicast.lists[((n - 1) & 3) as usize];
            Ok(MultiValue::from_vec(
                list.iter().map(|&s| Value::Integer(i64::from(s))).collect(),
            ))
        })?,
    )?;

    // Usage only when neither argument is a number; spell 0 clears, a wrong element refuses.
    g.set(
        "SetMultiCastSpell",
        lua.create_function(|lua, (action, spell): (Value, Value)| {
            if !matches!(action, Value::Integer(_) | Value::Number(_))
                && !matches!(spell, Value::Integer(_) | Value::Number(_))
            {
                return Err(mlua::Error::runtime(
                    "Usage: SetMultiCastSpell(slot, spellID)",
                ));
            }
            let action = super::binding_abi::coerced_number(lua, Some(action)) as i64;
            let spell = super::binding_abi::coerced_number(lua, Some(spell)) as i64;
            let (Ok(action), Ok(spell)) = (u32::try_from(action), u32::try_from(spell)) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            match totem_fits(&model, action, spell) {
                None => {}
                Some(_) if spell == 0 => {
                    model.actions.remove(&action);
                    model.action_sets.push((action, 0));
                }
                Some(true) => {
                    let texture = model
                        .spellbook
                        .slots
                        .iter()
                        .find(|s| s.spell_id == spell)
                        .and_then(|s| s.texture.clone());
                    model.actions.insert(
                        action,
                        crate::script::ActionSlot {
                            texture,
                            kind: ACTION_KIND_SPELL,
                            action: spell,
                            count: 0,
                            consumable: false,
                            stackable: false,
                            pet_spell: None,
                        },
                    );
                    let packed = (u32::from(ACTION_KIND_SPELL) << 24) | (spell & 0x00FF_FFFF);
                    model.action_sets.push((action, packed));
                }
                Some(false) => refuse_totem(&mut model, action),
            }
            Ok(())
        })?,
    )?;

    // The player's book (`0x53b4e0`), or the pet's book then its bar (`0x5d3560`).
    g.set(
        "IsSpellKnown",
        lua.create_function(|lua, (v, is_pet): (Value, Option<Value>)| {
            let usage = "Usage: IsSpellKnown(spellID[, isPet])";
            let id = number_arg(lua, v, usage)?;
            let Ok(id) =
                u32::try_from(id)
                    .map_err(|_| ())
                    .and_then(|i| if i > 0 { Ok(i) } else { Err(()) })
            else {
                return Err(mlua::Error::runtime(usage));
            };
            let is_pet = is_pet.is_some_and(|v| !matches!(v, Value::Nil | Value::Boolean(false)));
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if is_pet {
                model.pet_book.slots.iter().any(|s| s.spell_id == id)
                    || model
                        .pet_bar
                        .slots
                        .iter()
                        .any(|s| !s.view.is_token && s.view.spell_id == Some(id))
            } else {
                model.spellbook.slots.iter().any(|s| s.spell_id == id)
            })
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{SpellSlotView, UiScript};

    use super::*;

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_multicast(MultiCastView {
            lists: [vec![3599], vec![8071, 2484], vec![], vec![]],
            spell_categories: [(3599, [4, 0]), (8071, [2, 0]), (2484, [2, 0])]
                .into_iter()
                .collect(),
            category_names: [
                (4, "Fire Totem".to_string()),
                (2, "Earth Totem".to_string()),
            ]
            .into_iter()
            .collect(),
        });
        s
    }

    #[test]
    fn each_slot_answers_its_element_list() {
        let s = script();
        assert_eq!(
            s.eval::<f64>("return GetMultiCastBarOffset()").unwrap(),
            6.0
        );
        let earth: (i64, i64) = s.eval("return GetMultiCastTotemSpells(2)").unwrap();
        assert_eq!(earth, (8071, 2484));
        // `(slot - 1) & 3`: slot 6 is earth again.
        let again: (i64, i64) = s.eval("return GetMultiCastTotemSpells(6)").unwrap();
        assert_eq!(again, earth);
        assert_eq!(
            s.eval::<i64>("return select('#', GetMultiCastTotemSpells(3))")
                .unwrap(),
            0
        );
        assert!(s.run("GetMultiCastTotemSpells('x')").is_err());
    }

    #[test]
    fn a_totem_of_the_slots_element_is_set_and_another_refused() {
        let mut s = script();
        // Lua action 134 is page 1's earth slot.
        s.run("SetMultiCastSpell(134, 8071)").unwrap();
        assert!(s.eval::<bool>("return HasAction(134)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(134, 8071)]);
        s.run("SetMultiCastSpell(133, 8071)").unwrap();
        assert!(
            !s.eval::<bool>("return HasAction(133)").unwrap(),
            "earth is not fire"
        );
        assert_eq!(
            s.take_ui_errors_s(),
            vec![("ERR_MULTI_CAST_ACTION_TOTEM_S", "Fire Totem".to_string())]
        );
        s.run("SetMultiCastSpell(134, 0)").unwrap();
        assert!(
            !s.eval::<bool>("return HasAction(134)").unwrap(),
            "0 clears"
        );
        assert_eq!(s.take_action_sets(), vec![(134, 0)]);
        s.run("SetMultiCastSpell(12, 8071)").unwrap();
        assert!(
            s.take_action_sets().is_empty(),
            "a main-bar action is ignored"
        );
        assert!(s.run("SetMultiCastSpell('a', 'b')").is_err());
    }

    #[test]
    fn a_dropped_spell_lands_on_its_own_element_and_is_refused_elsewhere() {
        use crate::script::cursor::{CursorPayload, CursorSpell};
        let held = |id| {
            CursorPayload::Spell(CursorSpell {
                book_slot: 1,
                book_type: "spell".into(),
                spell_id: id,
                texture: None,
                passive: false,
            })
        };
        let mut s = script();
        s.set_cursor_for_test(held(3599));
        assert!(
            !s.eval::<bool>("return PlaceAction(134)").unwrap(),
            "fire on earth"
        );
        assert_eq!(
            s.take_ui_errors_s(),
            vec![("ERR_MULTI_CAST_ACTION_TOTEM_S", "Earth Totem".to_string())]
        );
        assert!(
            s.eval::<bool>("return CursorHasSpell()").unwrap(),
            "still held"
        );
        assert!(
            s.eval::<bool>("return PlaceAction(133)").unwrap(),
            "fire on fire"
        );
        assert!(s.eval::<bool>("return HasAction(133)").unwrap());
        // A main-bar slot takes the same spell as before.
        s.set_cursor_for_test(held(8071));
        assert!(s.eval::<bool>("return PlaceAction(5)").unwrap());
        assert!(s.take_ui_errors_s().is_empty());
    }

    #[test]
    fn is_spell_known_reads_the_book() {
        let s = script();
        s.model_mut().spellbook.slots.push(SpellSlotView {
            spell_id: 8071,
            ..Default::default()
        });
        assert!(s.eval::<bool>("return IsSpellKnown(8071)").unwrap());
        assert!(!s.eval::<bool>("return IsSpellKnown(3599)").unwrap());
        assert!(
            !s.eval::<bool>("return IsSpellKnown(8071, true)").unwrap(),
            "not the pet's"
        );
        assert!(s.run("IsSpellKnown(0)").is_err());
        assert!(s.run("IsSpellKnown()").is_err());
    }
}
