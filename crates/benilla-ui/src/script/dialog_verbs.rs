//! The verbs the stock `StaticPopup.lua` dialogs call: the instance-boot and area spirit healer
//! clocks, the meeting stone pair and the pet untrain pair; the battlefield port answer is
//! `battlefield_queue`'s.

use mlua::{Lua, Value};

use super::binding_abi::flag;

/// The area spirit healer's aura (`0xA18`), the only cancel-aura spell that also fires
/// `AREA_SPIRIT_HEALER_OUT_OF_RANGE` (`0x6e70b6`).
pub const AREA_SPIRIT_HEALER_SPELL: u32 = 2584;

impl super::UiScript {
    /// `ConfirmPetUnlearn()` calls since the last drain.
    pub fn take_pet_unlearn_confirms(&mut self) -> u32 {
        drain_field!(self, pet_unlearn_confirms)
    }

    /// What `CheckPetUntrainerDist()` answers: a pending question whose trainer is in reach.
    pub fn set_pet_untrainer_pending(&mut self, pending: bool) {
        set_field!(self, pet_untrainer_pending, pending);
    }

    /// `GetInstanceBootTimeRemaining()`'s answer, whole seconds (0 idle).
    pub fn set_instance_boot_secs(&mut self, secs: u32) {
        set_field!(self, instance_boot_secs, secs);
    }

    /// The area spirit healer: whether one is cached, and the seconds to its next wave.
    pub fn set_area_spirit_healer(&mut self, cached: bool, secs: u32) {
        {
            let m = self.model_ref();
            if m.area_spirit_healer_cached == cached && m.area_spirit_secs == secs {
                return;
            }
        }
        let mut model = self.model_mut();
        model.area_spirit_healer_cached = cached;
        model.area_spirit_secs = secs;
    }

    /// `AcceptAreaSpiritHeal()` calls since the last drain, each a `0x2E3` to send.
    pub fn take_area_spirit_accepts(&mut self) -> u32 {
        drain_field!(self, area_spirit_accepts)
    }

    /// `CancelMeetingStoneRequest()` calls since the last drain.
    pub fn take_meeting_stone_cancels(&mut self) -> u32 {
        drain_field!(self, meeting_stone_cancels)
    }

    /// The meeting stone's two globals: the queued area id (`0xb72038`, 0 for none) and the
    /// cached status text (`0xb7203c`, `None` outside the world), which the app rebuilds.
    pub fn set_meeting_stone(&mut self, area: u32, status_text: Option<String>) {
        let mut model = self.model_mut();
        model.meeting_stone_area = area;
        model.meeting_stone_status_text = status_text;
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x48b620`: `max(deadline - now, 0) / 1000`, truncated.
    g.set(
        "GetInstanceBootTimeRemaining",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.instance_boot_secs))
        })?,
    )?;

    // `0x48df20`: silent with no healer cached, else `0x2E3` with the cached guid.
    g.set(
        "AcceptAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if model.area_spirit_healer_cached {
                model.area_spirit_accepts += 1;
            }
            Ok(())
        })?,
    )?;
    // `0x48df30` runs the generic cancel-aura (`0x6e7040`): the event, then `0x136` with
    // `u32 2584`, no guid. Its refusal gate (`AttributesEx` bit 13 set, bit 2 clear, then
    // `0x5ee290`) never trips for 2584 in the shipped `Spell.dbc` (the app's
    // `spell_2584_never_trips_the_cancel_gate`), so the send is unconditional.
    g.set(
        "CancelAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model
                .pending_events
                .push(("AREA_SPIRIT_HEALER_OUT_OF_RANGE".to_string(), Vec::new()));
            model.cancel_aura_requests.push(AREA_SPIRIT_HEALER_SPELL);
            Ok(())
        })?,
    )?;
    // Whole seconds: `max(0, [0xb4e338] - now) / 1000`.
    g.set(
        "GetAreaSpiritHealerTime",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.area_spirit_secs))
        })?,
    )?;

    // `0x4ca5a0`: an empty `0x293` gated on party leadership (the app's); only the reply clears.
    g.set(
        "CancelMeetingStoneRequest",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.meeting_stone_cancels += 1;
            Ok(())
        })?,
    )?;

    // `0x4ca570`: the number `1` or nil, never `0` (truthy, it would pin the icon) or a boolean.
    g.set(
        "IsInMeetingStoneQueue",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.meeting_stone_area != 0))
        })?,
    )?;

    // `0x4ca5b0`: a string or nil, never `""` for nothing.
    g.set(
        "GetMeetingStoneStatusText",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(match &model.meeting_stone_status_text {
                Some(text) => Value::String(lua.create_string(text)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // `0x48d1c0`: `1` or nil, never `0`; the app tests the range to the latched trainer.
    g.set(
        "CheckPetUntrainerDist",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.pet_untrainer_pending))
        })?,
    )?;
    // `0x48dca0`: the app holds the latch and the money gate, and sends `0x2F0`.
    g.set(
        "ConfirmPetUnlearn",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.pet_unlearn_confirms += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_clocks_read_what_the_app_feeds() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetInstanceBootTimeRemaining()")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return GetAreaSpiritHealerTime()").unwrap(),
            0
        );
        s.set_instance_boot_secs(42);
        s.set_area_spirit_healer(true, 17);
        assert_eq!(
            s.eval::<i64>("return GetInstanceBootTimeRemaining()")
                .unwrap(),
            42
        );
        assert_eq!(
            s.eval::<i64>("return GetAreaSpiritHealerTime()").unwrap(),
            17
        );
    }

    #[test]
    fn accept_area_spirit_heal_is_silent_without_a_cached_healer() {
        let mut s = UiScript::new().unwrap();
        s.run("AcceptAreaSpiritHeal()").unwrap();
        assert_eq!(
            s.take_area_spirit_accepts(),
            0,
            "no healer cached: nothing sent"
        );
        s.set_area_spirit_healer(true, 5);
        s.run("AcceptAreaSpiritHeal() AcceptAreaSpiritHeal()")
            .unwrap();
        assert_eq!(s.take_area_spirit_accepts(), 2);
    }

    #[test]
    fn cancel_area_spirit_heal_cancels_the_aura_and_fires_out_of_range() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"F = CreateFrame("Frame") F:RegisterEvent("AREA_SPIRIT_HEALER_OUT_OF_RANGE")
               F:SetScript("OnEvent", function() GOT = event end) CancelAreaSpiritHeal()"#,
        )
        .unwrap();
        s.tick(0.0);
        assert_eq!(
            s.eval::<String>("return GOT").unwrap(),
            "AREA_SPIRIT_HEALER_OUT_OF_RANGE"
        );
        assert_eq!(
            s.take_cancel_aura_requests(),
            vec![super::AREA_SPIRIT_HEALER_SPELL]
        );
    }

    #[test]
    fn the_meeting_stone_pair_answers_one_or_nil_and_string_or_nil() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("IsInMeetingStoneQueue()").unwrap(), 1);
        assert!(s
            .eval::<bool>("return IsInMeetingStoneQueue() == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetMeetingStoneStatusText() == nil")
            .unwrap());
        s.set_meeting_stone(1519, Some("Looking for more for Stormwind City".into()));
        assert_eq!(s.eval::<i64>("return IsInMeetingStoneQueue()").unwrap(), 1);
        assert_eq!(
            s.eval::<String>("return GetMeetingStoneStatusText()")
                .unwrap(),
            "Looking for more for Stormwind City"
        );
        s.set_meeting_stone(0, Some("Unknown".into()));
        assert!(
            s.eval::<bool>("return IsInMeetingStoneQueue() == nil")
                .unwrap(),
            "area 0: not queued, whatever the text says"
        );
        assert_eq!(
            s.eval::<String>("return GetMeetingStoneStatusText()")
                .unwrap(),
            "Unknown"
        );
    }

    #[test]
    fn the_pet_pair_counts_and_answers_one_or_nil() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return CheckPetUntrainerDist() == nil")
            .unwrap());
        s.set_pet_untrainer_pending(true);
        assert_eq!(s.eval::<i64>("return CheckPetUntrainerDist()").unwrap(), 1);
        s.run("ConfirmPetUnlearn() ConfirmPetUnlearn() CancelMeetingStoneRequest()")
            .unwrap();
        assert_eq!(s.take_pet_unlearn_confirms(), 2);
        assert_eq!(s.take_meeting_stone_cancels(), 1);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
