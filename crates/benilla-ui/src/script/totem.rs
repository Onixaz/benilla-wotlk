//! `GetTotemInfo` (`0x51d330`), `GetTotemTimeLeft` (`0x517c60`), `DestroyTotem` (`0x51d520`), which
//! read and write the totem table (`0xbd0b00`) through the lent view when called.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::ScriptValue;

/// The slots `GetTotemInfo` accepts.
pub const MAX_TOTEM_SLOTS: usize = 4;

/// One live slot of the totem table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TotemView {
    /// The totem unit's name (`+0x10`), `None` until its object streams.
    pub name: Option<String>,
    /// When `SMSG_TOTEM_CREATED` arrived (`+0x18`), on the `GetTime` clock in seconds.
    pub start: f64,
    pub duration_ms: u32,
    /// The summoning spell's icon path.
    pub icon: Option<String>,
}

/// The live slot `slot` (0-based) through the lent view.
fn live_totem(slot: usize) -> Option<TotemView> {
    super::live::read_live(|l| l.totem(slot)).flatten()
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

/// `PLAYER_TOTEM_UPDATE` for the 0-based `slot`, fired inside the call (`0x51986a`).
pub(crate) fn totem_update(lua: &Lua, slot: usize) {
    super::tick::fire_event_into(
        lua,
        "PLAYER_TOTEM_UPDATE",
        vec![ScriptValue::Int(slot as i64 + 1)],
    );
}

pub(super) fn install_tooltip_method(lua: &Lua, m: &mlua::Table) -> mlua::Result<()> {
    // GameTooltip:SetTotem(slot) (`0x6205c0`): the totem's name and its time left, nothing for an
    // empty slot.
    m.set(
        "SetTotem",
        lua.create_function(|lua, (this, slot): (mlua::Table, Value)| {
            let h = super::object::frame_handle_of(lua, &this)?;
            let slot = slot_arg(lua, slot, "Usage: GameTooltip:SetTotem(slot)")?;
            let now = super::clock::now(lua);
            let Some(TotemView {
                name,
                start,
                duration_ms,
                ..
            }) = live_totem(slot)
            else {
                return Ok(());
            };
            super::tooltip::clear_content(&mut super::model_mut_of(lua), h);
            super::tooltip::fire_cleared(lua, h);
            let white = [1.0, 1.0, 1.0, 1.0];
            let gold = [1.0, 210.0 / 255.0, 0.0, 1.0];
            super::tooltip::append_line(
                lua,
                &this,
                (name.unwrap_or_default(), white),
                None,
                false,
            )?;
            let left = (start + f64::from(duration_ms) / 1000.0 - now).max(0.0);
            let get = |k: &str| crate::strings::global(lua, k);
            if let Some(text) = super::tooltip::duration_text(
                (left * 1000.0) as u32,
                "SPELL_TIME_REMAINING",
                true,
                &get,
            ) {
                super::tooltip::append_line(lua, &this, (text, gold), None, false)?;
            }
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // Live: `true, name, start, duration, icon`; empty: the tool flag, "", 0, 0, "". The flag is
    // the bag search for the slot's tool row with the player held (`0x51d42f`, `0x51d457`-`0x51d496`).
    g.set(
        "GetTotemInfo",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: GetTotemInfo(slot)")?;
            let opt_str = |s: &Option<String>| -> mlua::Result<Value> {
                Ok(match s {
                    Some(s) => Value::String(lua.create_string(s)?),
                    None => Value::Nil,
                })
            };
            let empty = || lua.create_string("").map(Value::String);
            let values = match &live_totem(slot) {
                Some(t) => vec![
                    Value::Boolean(true),
                    opt_str(&t.name)?,
                    Value::Integer(whole_seconds((t.start * 1000.0) as i64)),
                    Value::Integer(whole_seconds(i64::from(t.duration_ms))),
                    opt_str(&t.icon)?,
                ],
                None => vec![
                    Value::Boolean(super::live::read_live(|l| l.totem_tool(slot)).unwrap_or(false)),
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
            Ok(live_totem(slot).map_or(0, |t| {
                let end_ms = (t.start * 1000.0) as i64 + i64::from(t.duration_ms);
                whole_seconds(end_ms - (now * 1000.0) as i64)
            }))
        })?,
    )?;

    // A live slot is sent, cleared and announced inside the call (`0x51d562` -> `0x5197d0`); an
    // empty one does nothing. Behind a queued script call it keeps its turn, so its send follows
    // that call's.
    g.set(
        "DestroyTotem",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: DestroyTotem(slot)")?;
            if !crate::script::calls::none_waiting(lua) {
                crate::script::model_mut_of(lua)
                    .script_calls
                    .push(super::ScriptCall::DestroyTotem(slot as u8));
            } else if super::live::write_live(|l| l.destroy_totem(slot)) == Some(true) {
                totem_update(lua, slot);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::live::TestView;
    use crate::script::UiScript;

    use super::*;

    fn script_with(
        live: [Option<TotemView>; MAX_TOTEM_SLOTS],
        tools: [bool; MAX_TOTEM_SLOTS],
    ) -> (UiScript, TestView) {
        let mut s = UiScript::new().unwrap();
        s.set_now(100.0);
        let view = TestView {
            totems: std::cell::RefCell::new(live),
            totem_tools: tools,
            ..Default::default()
        };
        (s, view)
    }

    fn searing() -> TotemView {
        TotemView {
            name: Some("Searing Totem".into()),
            start: 95.5,
            duration_ms: 30_000,
            icon: Some("Interface\\Icons\\Spell_Fire_SearingTotem".into()),
        }
    }

    fn one(slot: usize, t: TotemView) -> [Option<TotemView>; MAX_TOTEM_SLOTS] {
        let mut live: [Option<TotemView>; MAX_TOTEM_SLOTS] = Default::default();
        live[slot] = Some(t);
        live
    }

    #[test]
    fn a_live_slot_answers_its_totem_in_whole_seconds() {
        let (s, mut view_v) = script_with(one(0, searing()), [false; 4]);
        crate::script::live::lending(&mut view_v, || {
            let (have, name, start, duration, icon): (bool, String, f64, f64, String) =
                s.eval("return GetTotemInfo(1)").unwrap();
            assert!(have);
            assert_eq!(name, "Searing Totem");
            assert_eq!((start, duration), (95.0, 30.0));
            assert_eq!(icon, "Interface\\Icons\\Spell_Fire_SearingTotem");
            // 95.5 + 30 - 100 = 25.5 s left, truncated.
            assert_eq!(s.eval::<f64>("return GetTotemTimeLeft(1)").unwrap(), 25.0);
        });
    }

    #[test]
    fn an_empty_slot_answers_whether_its_tool_is_carried() {
        let (s, mut view_v) = script_with(Default::default(), [false, true, false, false]);
        crate::script::live::lending(&mut view_v, || {
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
        });
    }

    #[test]
    fn an_unnamed_totem_answers_nil_for_its_name() {
        let (s, mut view_v) = script_with(
            one(
                3,
                TotemView {
                    name: None,
                    ..searing()
                },
            ),
            [false; 4],
        );
        crate::script::live::lending(&mut view_v, || {
            assert!(s
                .eval::<bool>("local _, name = GetTotemInfo(4) return name == nil")
                .unwrap());
        });
    }

    #[test]
    fn an_expired_totem_has_no_time_left() {
        let (s, mut view_v) = script_with(
            one(
                0,
                TotemView {
                    start: 50.0,
                    ..searing()
                },
            ),
            [false; 4],
        );
        crate::script::live::lending(&mut view_v, || {
            assert_eq!(s.eval::<f64>("return GetTotemTimeLeft(1)").unwrap(), 0.0);
        });
    }

    #[test]
    fn slots_outside_one_to_four_and_non_numbers_raise() {
        let (s, mut view_v) = script_with(Default::default(), [false; 4]);
        crate::script::live::lending(&mut view_v, || {
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
        });
    }

    /// `DestroyTotem` sends, clears and fires `PLAYER_TOTEM_UPDATE` before it returns, so the same
    /// chunk reads the slot empty and a handler of the event has run (`0x5197d0`).
    #[test]
    fn destroying_a_live_totem_sends_clears_and_announces_it_inside_the_call() {
        let (s, mut view_v) = script_with(one(2, searing()), [false; 4]);
        crate::script::live::lending(&mut view_v, || {
            s.run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_TOTEM_UPDATE')
             f:SetScript('OnEvent', function(_, _, slot) SEEN[#SEEN + 1] = slot end)",
            )
            .unwrap();
            let (live, seen): (bool, i64) = s
                .eval("DestroyTotem(3) return (GetTotemInfo(3)), #SEEN")
                .unwrap();
            assert!(!live);
            assert_eq!(seen, 1);
            assert_eq!(s.eval::<i64>("return SEEN[1]").unwrap(), 3);
            assert_eq!(
                crate::script::live::test_view(|v| v.totem_destroys.borrow().clone()),
                [2]
            );

            // An empty slot sends and fires nothing.
            s.run("DestroyTotem(1)").unwrap();
            assert_eq!(
                crate::script::live::test_view(|v| v.totem_destroys.borrow().len()),
                1
            );
            assert_eq!(s.eval::<i64>("return #SEEN").unwrap(), 1);
        });
    }

    /// Behind a queued script call the destroy keeps its turn: it is queued, and nothing is sent
    /// or fired in the call.
    #[test]
    fn a_destroy_behind_a_queued_call_keeps_its_turn() {
        let (mut s, mut view_v) = script_with(one(0, searing()), [false; 4]);
        crate::script::live::lending(&mut view_v, || {
            s.run("TargetUnit('player') DestroyTotem(1)").unwrap();
            assert!(crate::script::live::test_view(|v| v
                .totem_destroys
                .borrow()
                .is_empty()));
            let calls = s.take_script_calls();
            assert!(
                matches!(
                    calls.last(),
                    Some(crate::script::ScriptCall::DestroyTotem(0))
                ),
                "{calls:?}"
            );
        });
    }
}
