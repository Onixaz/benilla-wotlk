//! The tutorial verbs. The banks are the app's: it pushes the acknowledged bank for
//! `TutorialsEnabled` and drains the writes. `TriggerTutorial(n)` raises tutorial `n` through the
//! app's fire-once trigger, as the client's own sites do. `ClearTutorials()` sets every
//! bit of both banks and sends `CMSG_TUTORIAL_CLEAR`; `ResetTutorials()` clears them and sends
//! `CMSG_TUTORIAL_RESET`. None of the three writers returns a value.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, number_arg};

/// `FlagTutorial`'s bound, `0 ≤ n − 1 < 0x32`: the fifty tutorials `GlobalStrings.lua` names.
const TUTORIAL_IDS: i32 = 0x32;

/// `TriggerTutorial`'s bound, `n − 1 < 0x3c` unsigned (`0x5308f2`).
const TRIGGER_IDS: i32 = 0x3c;

impl super::UiScript {
    /// The acknowledged bank's bytes, `None` before `SMSG_TUTORIAL_FLAGS`.
    pub fn set_tutorial_bank(&mut self, bank: Option<Vec<u8>>) {
        set_field!(self, tutorial_bank, bank);
    }

    /// The completed list, in completion order, `id + 1` each.
    pub fn set_tutorials_completed(&mut self, completed: Vec<u32>) {
        set_field!(self, tutorials_completed, completed);
    }

    /// `FlagTutorial(n)` calls since the last drain, as 0-based ids (`n − 1`).
    pub fn take_tutorial_flag_requests(&mut self) -> Vec<u32> {
        drain_field!(self, tutorial_flag_requests)
    }

    /// `TriggerTutorial(n)` calls since the last drain, as 0-based ids (`n − 1`).
    pub fn take_tutorial_triggers(&mut self) -> Vec<u32> {
        drain_field!(self, tutorial_triggers)
    }

    /// `ClearTutorials()` calls since the last drain.
    pub fn take_tutorial_clears(&mut self) -> u32 {
        drain_field!(self, tutorial_clears)
    }

    /// `ResetTutorials()` calls since the last drain.
    pub fn take_tutorial_resets(&mut self) -> u32 {
        drain_field!(self, tutorial_resets)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `TutorialsEnabled()`: 1 if any of the bank's bytes is not 0xFF, the whole bank and not just
    // the fifty ids; else nil, as with no bank.
    g.set(
        "TutorialsEnabled",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let enabled = model
                .tutorial_bank
                .as_ref()
                .is_some_and(|b| b.iter().any(|&byte| byte != 0xFF));
            Ok(flag(enabled))
        })?,
    )?;

    // `CanResetTutorials()` (`0x530700`): 1 if any word of the bank is non-zero, else nil.
    g.set(
        "CanResetTutorials",
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let any = model
                .tutorial_bank
                .as_ref()
                .is_some_and(|b| b.iter().any(|&byte| byte != 0));
            Ok(flag(any))
        })?,
    )?;

    // `FlagTutorial(n)`: a non-number raises the reference's usage text, which asks for a string;
    // truncated, an `n − 1` outside `0..0x32` is a silent no-op.
    g.set(
        "FlagTutorial",
        lua.create_function(|lua, n: Value| {
            let n = number_arg(lua, n, "Usage: FlagTutorial(\"tutorial\")")?;
            let id = n.wrapping_sub(1);
            if !(0..TUTORIAL_IDS).contains(&id) {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            model.tutorial_flag_requests.push(id as u32);
            Ok(())
        })?,
    )?;

    // `TriggerTutorial(n)` (`0x5308d0`): a non-number raises the reference's usage text, which
    // names `Trigger`; an `n − 1` outside `0..0x3c` is a silent no-op. Inside, the client's
    // trigger `0x530840` runs with no delay: fire once, the popup cue, `TUTORIAL_TRIGGER(n)`.
    g.set(
        "TriggerTutorial",
        lua.create_function(|lua, n: Value| {
            let n = number_arg(lua, n, "Usage: Trigger(\"tutorial\")")?;
            let id = n.wrapping_sub(1);
            if !(0..TRIGGER_IDS).contains(&id) {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            model.tutorial_triggers.push(id as u32);
            Ok(())
        })?,
    )?;

    // `IsTutorialFlagged(n)` (`0x5307a0`): bank B's bit, 1 or nil; nothing past the sixty.
    g.set(
        "IsTutorialFlagged",
        lua.create_function(|lua, n: Value| {
            let n = number_arg(lua, n, "Usage: Trigger(\"tutorial\")")?;
            let Ok(id) = u32::try_from(n.wrapping_sub(1)) else {
                return Ok(MultiValue::new());
            };
            if id >= TRIGGER_IDS as u32 {
                return Ok(MultiValue::new());
            }
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let set = model
                .tutorial_bank
                .as_ref()
                .and_then(|b| b.get((id >> 3) as usize))
                .is_some_and(|byte| byte & (1 << (id & 7)) != 0);
            Ok(MultiValue::from_vec(vec![flag(set)]))
        })?,
    )?;

    // `GetNextCompleatedTutorial(n)` (`0x5301d0`): the tutorial completed after `n`; none at the
    // end, or for a next of 60, the end's own marker.
    g.set(
        "GetNextCompleatedTutorial",
        lua.create_function(|lua, n: Value| {
            let n = number_arg(lua, n, "Usage: GetNextCompleatedTutorial(\"tutorial\")")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let list = &model.tutorials_completed;
            let next = list
                .iter()
                .position(|&id| id as i32 == n)
                .and_then(|i| list.get(i + 1))
                .copied()
                .filter(|&id| id != TRIGGER_IDS as u32);
            Ok(MultiValue::from_vec(
                next.map(|id| Value::Integer(i64::from(id)))
                    .into_iter()
                    .collect(),
            ))
        })?,
    )?;

    // `GetPrevCompleatedTutorial(n)` (`0x530240`): the tutorial completed before `n`.
    g.set(
        "GetPrevCompleatedTutorial",
        lua.create_function(|lua, n: Value| {
            let n = number_arg(lua, n, "Usage: GetPrevCompleatedTutorial(\"tutorial\")")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let list = &model.tutorials_completed;
            let prev = list
                .iter()
                .position(|&id| id as i32 == n)
                .and_then(|i| i.checked_sub(1))
                .map(|i| list[i]);
            Ok(MultiValue::from_vec(
                prev.map(|id| Value::Integer(i64::from(id)))
                    .into_iter()
                    .collect(),
            ))
        })?,
    )?;

    g.set(
        "ClearTutorials",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.tutorial_clears += 1;
            Ok(())
        })?,
    )?;

    g.set(
        "ResetTutorials",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.tutorial_resets += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// `0x530140`, `0x530190`: the walk is completion order, not id order; 60 reads as the end.
    #[test]
    fn the_completed_walk_is_in_completion_order() {
        let mut s = UiScript::new().unwrap();
        s.set_tutorials_completed(vec![7, 3, 12, 60]);
        let next: i32 = s.eval("return GetNextCompleatedTutorial(7)").unwrap();
        assert_eq!(next, 3);
        let prev: i32 = s.eval("return GetPrevCompleatedTutorial(12)").unwrap();
        assert_eq!(prev, 3);
        let count = |lua: &str| {
            s.eval::<i32>(&format!("return select('#', {lua})"))
                .unwrap()
        };
        assert_eq!(count("GetNextCompleatedTutorial(12)"), 0, "the next is 60");
        assert_eq!(
            count("GetPrevCompleatedTutorial(7)"),
            0,
            "nothing before the first"
        );
        assert_eq!(count("GetNextCompleatedTutorial(5)"), 0, "not completed");
        assert!(s.eval::<()>("GetNextCompleatedTutorial('x')").is_err());
    }

    /// `0x5307a0`: bank B's bit; past the sixty, no value at all.
    #[test]
    fn is_tutorial_flagged_reads_the_acknowledged_bank() {
        let mut s = UiScript::new().unwrap();
        s.set_tutorial_bank(Some(vec![0b100; 8]));
        let on: Option<i32> = s.eval("return IsTutorialFlagged(3)").unwrap();
        assert_eq!(on, Some(1));
        let off: Option<i32> = s.eval("return IsTutorialFlagged(4)").unwrap();
        assert_eq!(off, None);
        let n: i32 = s.eval("return select('#', IsTutorialFlagged(61))").unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn tutorials_enabled_scans_the_whole_acknowledged_bank() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("TutorialsEnabled()").unwrap(), 1);
        assert!(
            s.eval::<bool>("return TutorialsEnabled() == nil").unwrap(),
            "no bank"
        );
        s.set_tutorial_bank(Some(vec![0xFF; 32]));
        assert!(
            s.eval::<bool>("return TutorialsEnabled() == nil").unwrap(),
            "every bit acknowledged: disabled"
        );
        let mut bank = vec![0xFF; 32];
        bank[31] = 0x7F; // a bit past the fifty real ids
        s.set_tutorial_bank(Some(bank));
        assert_eq!(
            s.eval::<i64>("return TutorialsEnabled()").unwrap(),
            1,
            "the scan covers the whole bank, not the 50 ids"
        );
        s.set_tutorial_bank(Some(vec![0; 32]));
        assert_eq!(s.eval::<i64>("return TutorialsEnabled()").unwrap(), 1);
    }

    #[test]
    fn flag_tutorial_takes_a_one_based_number_and_bails_silently_off_the_fifty() {
        let mut s = UiScript::new().unwrap();
        s.run("FlagTutorial(1) FlagTutorial(50) FlagTutorial('42') FlagTutorial(7.9) FlagTutorial(0) FlagTutorial(51) FlagTutorial(-3)")
            .unwrap();
        assert_eq!(
            s.take_tutorial_flag_requests(),
            vec![0, 49, 41, 6],
            "0-based, truncated toward zero, 0/51/−3 dropped"
        );
        assert!(s.take_tutorial_flag_requests().is_empty(), "drained");
        for call in ["FlagTutorial()", "FlagTutorial(nil)", "FlagTutorial({})"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(
                err.contains("Usage: FlagTutorial(\"tutorial\")"),
                "{call}: {err}"
            );
        }
        s.run("ClearTutorials() ClearTutorials() ResetTutorials()")
            .unwrap();
        assert_eq!(s.take_tutorial_clears(), 2);
        assert_eq!(s.take_tutorial_resets(), 1);
        assert_eq!(s.take_tutorial_clears(), 0);
    }

    /// `TriggerTutorial(n)` (`0x5308d0`): `n − 1` in `0..0x3c` reaches the trigger, the rest is a
    /// silent no-op; a non-number raises the reference's `Trigger` usage text.
    #[test]
    fn trigger_tutorial_queues_the_zero_based_id_in_range() {
        let mut s = UiScript::new().unwrap();
        s.run("TriggerTutorial(59) TriggerTutorial(58) TriggerTutorial(60) TriggerTutorial(61) TriggerTutorial(0)")
            .unwrap();
        assert_eq!(s.take_tutorial_triggers(), vec![58, 57, 59]);
        assert_eq!(s.arity("TriggerTutorial(1)").unwrap(), 0);
        let err = s.run("TriggerTutorial()").unwrap_err();
        assert!(format!("{err}").contains("Usage: Trigger(\"tutorial\")"));
    }
}
