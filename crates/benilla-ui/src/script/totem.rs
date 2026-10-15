//! `GetTotemInfo` (`0x51d330`), `GetTotemTimeLeft` (`0x517c60`), `DestroyTotem` (`0x51d520`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::ScriptValue;

/// The slots `GetTotemInfo` accepts.
pub const MAX_TOTEM_SLOTS: usize = 4;

/// One live totem, as the app resolves it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TotemView {
    /// The totem unit's name, `None` until its object streams.
    pub name: Option<String>,
    /// When `SMSG_TOTEM_CREATED` arrived, on the `GetTime` clock in seconds.
    pub start: f64,
    pub duration_ms: u32,
    /// The summoning spell's icon path.
    pub icon: Option<String>,
}

/// The whole table: each slot's live totem, and whether the player carries its tool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TotemSlots {
    pub live: [Option<TotemView>; MAX_TOTEM_SLOTS],
    /// The slot's `TotemCategory` row is covered by a carried item (`0x7548f0`, flag 8).
    pub tools: [bool; MAX_TOTEM_SLOTS],
}

impl super::UiScript {
    /// Replace the totem table; the app fires `PLAYER_TOTEM_UPDATE` itself.
    pub fn set_totems(&mut self, slots: TotemSlots) {
        set_field!(self, totems, slots);
    }

    /// Take the `DestroyTotem` calls, as 0-based slots; the app sends each.
    pub fn take_totem_destroys(&mut self) -> Vec<u8> {
        drain_field!(self, totem_destroys)
    }
}

/// The 0-based slot argument; a non-number raises the usage line, a slot past 1..4 the range one.
fn slot_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<usize> {
    let slot = number_arg(lua, v, usage)?;
    usize::try_from(slot - 1)
        .ok()
        .filter(|&s| s < MAX_TOTEM_SLOTS)
        .ok_or_else(|| {
            mlua::Error::runtime(format!(
                "Totem slot must be in range of 1 to {MAX_TOTEM_SLOTS}"
            ))
        })
}

/// Whole seconds, as the bindings divide milliseconds by 1000 in integers.
fn whole_seconds(ms: i64) -> i64 {
    ms.max(0) / 1000
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // Live: `true, name, start, duration, icon`; empty: the tool flag, "", 0, 0, "".
    g.set(
        "GetTotemInfo",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: GetTotemInfo(slot)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let opt_str = |s: &Option<String>| -> mlua::Result<Value> {
                Ok(match s {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            let empty = || lua.create_string("").map(Value::String);
            let values = match &model.totems.live[slot] {
                Some(t) => vec![
                    Value::Boolean(true),
                    opt_str(&t.name)?,
                    Value::Integer(whole_seconds((t.start * 1000.0) as i64)),
                    Value::Integer(whole_seconds(i64::from(t.duration_ms))),
                    opt_str(&t.icon)?,
                ],
                None => vec![
                    Value::Boolean(model.totems.tools[slot]),
                    empty()?,
                    Value::Integer(0),
                    Value::Integer(0),
                    empty()?,
                ],
            };
            Ok(MultiValue::from_vec(values))
        })?,
    )?;

    // Whole seconds to expiry (`0x513f50`), 0 for an empty or expired slot.
    g.set(
        "GetTotemTimeLeft",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: GetTotemTimeLeft(slot)")?;
            let now = crate::script::clock::now(lua);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.totems.live[slot].as_ref().map_or(0, |t| {
                let end_ms = (t.start * 1000.0) as i64 + i64::from(t.duration_ms);
                whole_seconds(end_ms - (now * 1000.0) as i64)
            }))
        })?,
    )?;

    // A live slot is cleared, announced and sent (`0x5197d0`); an empty one does nothing.
    g.set(
        "DestroyTotem",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: DestroyTotem(slot)")?;
            let mut model = crate::script::model_mut_of(lua);
            if model.totems.live[slot].take().is_some() {
                model.totem_destroys.push(slot as u8);
                model.pending_events.push((
                    "PLAYER_TOTEM_UPDATE".into(),
                    vec![ScriptValue::Int(slot as i64 + 1)],
                ));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{ScriptValue, UiScript};

    use super::*;

    fn script_with(slots: TotemSlots) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_now(100.0);
        s.set_totems(slots);
        s
    }

    fn searing() -> TotemView {
        TotemView {
            name: Some("Searing Totem".into()),
            start: 95.5,
            duration_ms: 30_000,
            icon: Some("Interface\\Icons\\Spell_Fire_SearingTotem".into()),
        }
    }

    #[test]
    fn a_live_slot_answers_its_totem_in_whole_seconds() {
        let mut slots = TotemSlots::default();
        slots.live[0] = Some(searing());
        let s = script_with(slots);
        let (have, name, start, duration, icon): (bool, String, f64, f64, String) =
            s.eval("return GetTotemInfo(1)").unwrap();
        assert!(have);
        assert_eq!(name, "Searing Totem");
        assert_eq!((start, duration), (95.0, 30.0));
        assert_eq!(icon, "Interface\\Icons\\Spell_Fire_SearingTotem");
        // 95.5 + 30 - 100 = 25.5 s left, truncated.
        assert_eq!(s.eval::<f64>("return GetTotemTimeLeft(1)").unwrap(), 25.0);
    }

    #[test]
    fn an_empty_slot_answers_whether_its_tool_is_carried() {
        let mut slots = TotemSlots::default();
        slots.tools[1] = true;
        let s = script_with(slots);
        let (have, name, start, duration, icon): (bool, String, f64, f64, String) =
            s.eval("return GetTotemInfo(2)").unwrap();
        assert!(have, "the earth tool is carried");
        assert_eq!(
            (name.as_str(), start, duration, icon.as_str()),
            ("", 0.0, 0.0, "")
        );
        let (have,): (bool,) = s.eval("return GetTotemInfo(3)").unwrap();
        assert!(!have);
        assert_eq!(s.eval::<f64>("return GetTotemTimeLeft(2)").unwrap(), 0.0);
    }

    #[test]
    fn an_unnamed_totem_answers_nil_for_its_name() {
        let mut slots = TotemSlots::default();
        slots.live[3] = Some(TotemView {
            name: None,
            ..searing()
        });
        let s = script_with(slots);
        assert!(s
            .eval::<bool>("local _, name = GetTotemInfo(4) return name == nil")
            .unwrap());
    }

    #[test]
    fn an_expired_totem_has_no_time_left() {
        let mut slots = TotemSlots::default();
        slots.live[0] = Some(TotemView {
            start: 50.0,
            ..searing()
        });
        let s = script_with(slots);
        assert_eq!(s.eval::<f64>("return GetTotemTimeLeft(1)").unwrap(), 0.0);
    }

    #[test]
    fn slots_outside_one_to_four_and_non_numbers_raise() {
        let s = script_with(TotemSlots::default());
        for call in [
            "GetTotemInfo(0)",
            "GetTotemInfo(5)",
            "GetTotemTimeLeft(9)",
            "DestroyTotem(0)",
            "GetTotemInfo()",
            "GetTotemInfo('fire')",
        ] {
            assert!(s.run(call).is_err(), "{call} raises");
        }
    }

    #[test]
    fn destroying_a_live_totem_clears_queues_and_announces_it() {
        let mut slots = TotemSlots::default();
        slots.live[2] = Some(searing());
        let mut s = script_with(slots);
        s.run("DestroyTotem(3)").unwrap();
        assert!(!s.eval::<bool>("return (GetTotemInfo(3))").unwrap());
        assert_eq!(s.take_totem_destroys(), vec![2]);
        assert!(s
            .model_ref()
            .pending_events
            .iter()
            .any(|(e, a)| e == "PLAYER_TOTEM_UPDATE" && a == &vec![ScriptValue::Int(3)]));

        // An empty slot sends nothing.
        s.run("DestroyTotem(1)").unwrap();
        assert!(s.take_totem_destroys().is_empty());
    }
}
