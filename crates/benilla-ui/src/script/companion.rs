//! The companion bindings, over the two lists the app pushes (`CGSpellBook::m_companions`,
//! `0xbe8df4`): critters and mounts, each resolved to creature, name, icon, active flag and
//! cooldown. `CallCompanion` and `SummonRandomCritter` queue the spell's cast; `DismissCompanion`
//! queues an intent the app drains ([`CompanionIntent`]).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{queue_cursor_update, CursorPayload, CursorSpell};
use super::{CastAt, ScriptCall};

/// Which list, in the order of the type tokens `GetLuaCompanionType` (`0x53b600`) matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompanionType {
    Critter,
    Mount,
}

impl CompanionType {
    /// The token FrameXML passes and `COMPANION_UPDATE` carries (`0xacccc4`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Critter => "CRITTER",
            Self::Mount => "MOUNT",
        }
    }
}

/// One companion as `GetCompanionInfo` returns it, every field resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompanionView {
    /// `EffectMiscValue[0]`, the creature the spell summons or rides.
    pub creature_id: u32,
    /// The creature's cached name; `None` until the creature query answers.
    pub name: Option<String>,
    pub spell_id: u32,
    /// The active icon while active, else the spell's own (`0x53e3d0`).
    pub icon: Option<String>,
    /// `Spell_C_IsActiveSpell` (`0x802cb0`): the critter out, or the mount aura up.
    pub active: bool,
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, as a book slot carries it.
    pub cooldown: Option<(i64, u32, bool)>,
}

/// The two lists, each in the reference's order (by creature name once every name is cached,
/// `0x53cec0`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompanionState {
    pub critters: Vec<CompanionView>,
    pub mounts: Vec<CompanionView>,
    /// The known spells of skill line 778, the pool `SummonRandomCritter` draws from
    /// (`CGPlayer_C::m_companionSpells`, filled at `0x6e7bc4`).
    pub random_pool: Vec<u32>,
}

impl CompanionState {
    pub fn list(&self, ty: CompanionType) -> &[CompanionView] {
        match ty {
            CompanionType::Critter => &self.critters,
            CompanionType::Mount => &self.mounts,
        }
    }

    /// The list and 0-based slot holding `spell_id` (`FindCompanionSlotByID` `0x53d990`).
    pub fn find(&self, spell_id: u32) -> Option<(CompanionType, usize)> {
        [CompanionType::Critter, CompanionType::Mount]
            .into_iter()
            .find_map(|ty| {
                let at = self.list(ty).iter().position(|c| c.spell_id == spell_id)?;
                Some((ty, at))
            })
    }
}

/// What a companion verb asks of the app beyond a cast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompanionIntent {
    /// `DismissCompanion`: cancel this active companion (`Spell_C_CancelAura` `0x802f80`).
    Dismiss(u32),
}

impl super::UiScript {
    /// Replace the lists; the app fires the `COMPANION_*` events itself.
    pub fn set_companions(&mut self, state: CompanionState) {
        if self.model_ref().companions != state {
            self.model_mut().companions = state;
        }
    }

    /// Take the queued dismissals and random summons, in call order.
    pub fn take_companion_intents(&mut self) -> Vec<CompanionIntent> {
        drain_field!(self, companion_intents)
    }
}

/// `GetLuaCompanionType` (`0x53b600`): argument 1 through `lua_tolstring`, matched to a token
/// case-insensitively; anything else is `None`, which every verb answers with its `Usage:`.
fn companion_type(lua: &Lua, v: &Value) -> Option<CompanionType> {
    let token = match v {
        Value::String(_) | Value::Integer(_) | Value::Number(_) => lua
            .coerce_string(v.clone())
            .ok()
            .flatten()
            .map(|s| s.to_string_lossy())?,
        _ => return None,
    };
    [CompanionType::Critter, CompanionType::Mount]
        .into_iter()
        .find(|ty| ty.token().eq_ignore_ascii_case(&token))
}

/// A `(type, index)` pair: a bad type or an index reading 0.0 raises `usage`; the index is
/// truncated and made 0-based, so 0.5 and a negative wrap past every list (`0x53e30a`).
fn type_and_index(
    lua: &Lua,
    ty: &Value,
    index: Value,
    usage: &'static str,
) -> mlua::Result<(CompanionType, usize)> {
    let n = super::binding_abi::coerced_number(lua, Some(index));
    match companion_type(lua, ty) {
        Some(ty) if n != 0.0 => {
            let slot = (super::binding_abi::ftol(n) as u32).wrapping_sub(1);
            Ok((ty, slot as usize))
        }
        _ => Err(mlua::Error::runtime(usage)),
    }
}

/// Register the companion globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumCompanions(type) (`0x53c2a0`): the list's length.
    g.set(
        "GetNumCompanions",
        lua.create_function(|lua, ty: Value| {
            let Some(ty) = companion_type(lua, &ty) else {
                return Err(mlua::Error::runtime("Usage: GetNumCompanions(type)"));
            };
            let model = crate::script::model_mut_of(lua);
            Ok(model.companions.list(ty).len() as i64)
        })?,
    )?;

    // GetCompanionInfo(type, index) (`0x53e2c0`): `creatureID, creatureName, spellID, icon,
    // active`, the name nil until cached and active 1 or nil; past the list, nothing.
    g.set(
        "GetCompanionInfo",
        lua.create_function(|lua, (ty, index): (Value, Value)| {
            let (ty, slot) =
                type_and_index(lua, &ty, index, "Usage: GetCompanionInfo(type, index)")?;
            let model = crate::script::model_mut_of(lua);
            let Some(c) = model.companions.list(ty).get(slot) else {
                return Ok(MultiValue::new());
            };
            let text = |s: &Option<String>| -> mlua::Result<Value> {
                Ok(match s {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(c.creature_id)),
                text(&c.name)?,
                Value::Integer(i64::from(c.spell_id)),
                text(&c.icon)?,
                flag(c.active),
            ]))
        })?,
    )?;

    // GetCompanionCooldown(type, index) (`0x53e490`): `start, duration, enable` of the spell's
    // cooldown; with no player, or past the list, nothing.
    g.set(
        "GetCompanionCooldown",
        lua.create_function(|lua, (ty, index): (Value, Value)| {
            let (ty, slot) =
                type_and_index(lua, &ty, index, "Usage: GetCompanionCooldown(type, index)")?;
            let now = crate::script::clock::now(lua);
            let model = crate::script::model_mut_of(lua);
            let Some(c) = model.companions.list(ty).get(slot) else {
                return Ok(MultiValue::new());
            };
            let (start, duration, enable) = match c.cooldown {
                Some((start_ms, duration_ms, enabled)) => {
                    let (start, duration) =
                        (start_ms as f64 / 1000.0, f64::from(duration_ms) / 1000.0);
                    if start + duration > now || !enabled {
                        (start, duration, i64::from(enabled))
                    } else {
                        (0.0, 0.0, 1)
                    }
                }
                None => (0.0, 0.0, 1),
            };
            Ok(MultiValue::from_vec(vec![
                Value::Number(start),
                Value::Number(duration),
                Value::Integer(enable),
            ]))
        })?,
    )?;

    // CallCompanion(type, index) (`0x53e5e0`): the spell casts through `0x80da40` at the null
    // guid, past the book's cancel forks; past the list, nothing.
    g.set(
        "CallCompanion",
        lua.create_function(|lua, (ty, index): (Value, Value)| {
            let (ty, slot) = type_and_index(lua, &ty, index, "Usage: CallCompanion(type, index)")?;
            let mut model = crate::script::model_mut_of(lua);
            if let Some(spell_id) = model.companions.list(ty).get(slot).map(|c| c.spell_id) {
                model.script_calls.push(ScriptCall::CastSpell {
                    spell_id,
                    at: CastAt::Selection,
                    via_book: false,
                });
            }
            Ok(())
        })?,
    )?;

    // DismissCompanion(type) (`0x53e6b0`): every active companion of the list is cancelled.
    g.set(
        "DismissCompanion",
        lua.create_function(|lua, ty: Value| {
            let Some(ty) = companion_type(lua, &ty) else {
                return Err(mlua::Error::runtime("Usage: DismissCompanion(type)"));
            };
            let mut model = crate::script::model_mut_of(lua);
            let active: Vec<u32> = model
                .companions
                .list(ty)
                .iter()
                .filter(|c| c.active)
                .map(|c| c.spell_id)
                .collect();
            model
                .companion_intents
                .extend(active.into_iter().map(CompanionIntent::Dismiss));
            Ok(())
        })?,
    )?;

    // PickupCompanion(type, index) (`0x53c310` → `0x53bbc0`): the spell goes on the cursor; the
    // same spell already held drops instead.
    //
    // Deviation: a cursor holding anything else refuses, as `PickupSpell` does here, where the
    // reference clears it first.
    g.set(
        "PickupCompanion",
        lua.create_function(|lua, (ty, index): (Value, Value)| {
            let (ty, slot) =
                type_and_index(lua, &ty, index, "Usage: PickupCompanion(type, index)")?;
            let mut model = crate::script::model_mut_of(lua);
            let Some(c) = model.companions.list(ty).get(slot).cloned() else {
                return Ok(());
            };
            match &model.cursor {
                Some(CursorPayload::Spell(held)) if held.spell_id == c.spell_id => {
                    model.cursor = None;
                }
                Some(_) => return Ok(()),
                None => {
                    model.cursor = Some(CursorPayload::Spell(CursorSpell {
                        book_slot: 0,
                        book_type: "spell".to_string(),
                        spell_id: c.spell_id,
                        texture: c.icon,
                        passive: false,
                    }));
                }
            }
            queue_cursor_update(&mut model);
            Ok(())
        })?,
    )?;

    // SummonRandomCritter() (`0x53e7e0` → `0x6d7b30`): a random spell of the pool casts as
    // `CallCompanion` does (`rand() % count`); an empty pool casts nothing.
    g.set(
        "SummonRandomCritter",
        lua.create_function(|lua, ()| {
            use std::hash::BuildHasher;
            let mut model = crate::script::model_mut_of(lua);
            let pool = &model.companions.random_pool;
            if pool.is_empty() {
                return Ok(());
            }
            let roll = std::collections::hash_map::RandomState::new().hash_one(pool.len());
            let spell_id = pool[(roll % pool.len() as u64) as usize];
            model.script_calls.push(ScriptCall::CastSpell {
                spell_id,
                at: CastAt::Selection,
                via_book: false,
            });
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{SpellBookState, SpellSlotView, UiScript};

    fn view(spell_id: u32, creature_id: u32, name: Option<&str>, active: bool) -> CompanionView {
        CompanionView {
            creature_id,
            name: name.map(Into::into),
            spell_id,
            icon: Some(format!("Icon{spell_id}")),
            active,
            cooldown: None,
        }
    }

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_companions(CompanionState {
            critters: vec![
                view(10673, 7385, Some("Bombay Cat"), true),
                view(4055, 2671, None, false),
            ],
            mounts: vec![view(458, 284, Some("Brown Horse"), false)],
            random_pool: vec![4055],
        });
        s
    }

    /// `GetCompanionInfo` (`0x53e2c0`): five values, the uncached name nil, active 1 or nil, and
    /// nothing past the list; the type token is case-insensitive.
    #[test]
    fn companion_info_answers_five_values_and_nothing_past_the_list() {
        let s = script();
        assert_eq!(
            s.eval::<i64>("return GetNumCompanions('critter')").unwrap(),
            2
        );
        assert_eq!(
            s.eval::<i64>("return GetNumCompanions('MOUNT')").unwrap(),
            1
        );
        assert!(s
            .eval::<bool>(
                "local c, n, id, icon, a = GetCompanionInfo('CRITTER', 1) \
                 return c == 7385 and n == 'Bombay Cat' and id == 10673 and icon == 'Icon10673' \
                 and a == 1"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local c, n, id, icon, a = GetCompanionInfo('CRITTER', 2) \
                 return c == 2671 and n == nil and a == nil and select('#', \
                 GetCompanionInfo('CRITTER', 2)) == 5"
            )
            .unwrap());
        assert_eq!(
            s.eval::<i64>("return select('#', GetCompanionInfo('MOUNT', 2))")
                .unwrap(),
            0
        );
    }

    /// A bad type, or an index reading 0, raises the verb's `Usage:`.
    #[test]
    fn a_bad_type_or_a_zero_index_raises() {
        let s = script();
        for (call, usage) in [
            ("GetNumCompanions('PET')", "Usage: GetNumCompanions(type)"),
            (
                "GetCompanionInfo('MOUNT', 0)",
                "Usage: GetCompanionInfo(type, index)",
            ),
            (
                "CallCompanion('CRITTER')",
                "Usage: CallCompanion(type, index)",
            ),
            ("DismissCompanion(nil)", "Usage: DismissCompanion(type)"),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(usage), "{call}: {err}");
        }
    }

    /// `CallCompanion` casts past the book's forks at the null guid; `DismissCompanion` asks for
    /// every active companion of its list; `PickupCompanion` holds the spell.
    #[test]
    fn call_dismiss_and_pickup_queue_their_effects() {
        let mut s = script();
        s.run("CallCompanion('MOUNT', 1) CallCompanion('MOUNT', 5)")
            .unwrap();
        assert_eq!(
            s.take_script_calls(),
            vec![ScriptCall::CastSpell {
                spell_id: 458,
                at: CastAt::Selection,
                via_book: false,
            }]
        );
        s.run("DismissCompanion('CRITTER') DismissCompanion('MOUNT') SummonRandomCritter()")
            .unwrap();
        assert_eq!(
            s.take_companion_intents(),
            vec![CompanionIntent::Dismiss(10673)]
        );
        assert_eq!(
            s.take_script_calls(),
            vec![ScriptCall::CastSpell {
                spell_id: 4055,
                at: CastAt::Selection,
                via_book: false,
            }],
            "the pool's one spell"
        );
        s.run("PickupCompanion('CRITTER', 2)").unwrap();
        assert!(s
            .eval::<bool>(
                "local kind, index, ty = GetCursorInfo() \
                 return kind == 'companion' and index == 2 and ty == 'CRITTER'"
            )
            .unwrap());
        s.run("PickupCompanion('CRITTER', 2)").unwrap();
        assert_eq!(
            s.eval::<i64>("return select('#', GetCursorInfo())")
                .unwrap(),
            0,
            "the same companion again drops it"
        );
    }

    /// `/cast` reaches a mount by name though no tab lists it (`0x542699`), and `GetActionInfo`
    /// answers a barred companion as `"companion"` (`0x5a8fa8`).
    #[test]
    fn a_mount_casts_by_name_and_answers_as_a_companion_on_the_bar() {
        let mut s = script();
        s.set_spellbook(SpellBookState {
            unlisted: vec![SpellSlotView {
                spell_id: 458,
                name: "Brown Horse".into(),
                ..SpellSlotView::default()
            }],
            ..SpellBookState::default()
        });
        s.run("CastSpellByName('Brown Horse') CastSpellByID(458)")
            .unwrap();
        let calls = s.take_script_calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(
            s.eval::<i64>("return GetNumSpellTabs()").unwrap(),
            0,
            "no tab lists it"
        );
        s.set_action(
            1,
            Some(crate::script::ActionSlot {
                kind: crate::script::action::ACTION_KIND_SPELL,
                action: 458,
                ..Default::default()
            }),
        );
        assert!(s
            .eval::<bool>(
                "local kind, index, ty, id = GetActionInfo(1) \
                 return kind == 'companion' and index == 1 and ty == 'MOUNT' and id == 458"
            )
            .unwrap());
    }
}
