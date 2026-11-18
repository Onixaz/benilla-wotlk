//! The verbs the stock `StaticPopup.lua` dialogs call for the instance-boot and area spirit healer clocks, each read live at the call; the battlefield port answer is `battlefield_queue`'s.

use mlua::Lua;

/// The area spirit healer's aura (`0xA18`), the only cancel-aura spell that also fires
/// `AREA_SPIRIT_HEALER_OUT_OF_RANGE`.
pub const AREA_SPIRIT_HEALER_SPELL: u32 = 2584;

/// What the healer poll (`0x524010`) did through its healer set (`0x523eb0`): a pending wave cancelled, a new healer asked for.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SpiritPoll {
    /// The wave's aura cancel (`0x523eea`), which fires `AREA_SPIRIT_HEALER_OUT_OF_RANGE` ahead of its send.
    pub cancel_aura: bool,
    /// `CMSG_AREA_SPIRIT_HEALER_QUERY` for the healer newly held.
    pub query: Option<u64>,
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetInstanceBootTimeRemaining",
        lua.create_function(|_, ()| {
            Ok(i64::from(
                super::read_live(|l| l.instance_boot_secs()).unwrap_or(0),
            ))
        })?,
    )?;

    // The poll first, its cancel's event inside the call, then the queue send naming the healer it left (`0x524b60`).
    g.set(
        "AcceptAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            let Some(poll) = super::write_live(|l| l.area_spirit_poll()).flatten() else {
                return Ok(());
            };
            if poll.cancel_aura {
                super::tick::fire_event_into(lua, "AREA_SPIRIT_HEALER_OUT_OF_RANGE", Vec::new());
            }
            super::read_live(|l| l.accept_area_spirit_heal(poll));
            Ok(())
        })?,
    )?;
    // The reference runs the generic cancel-aura: the event, then `0x136` with `u32 2584`, no guid.
    // Its refusal gate (`AttributesEx` bit 13 set, bit 2 clear) never trips for 2584 in the shipped
    // `Spell.dbc` (the app's `spell_2584_never_trips_the_cancel_gate`), so the send is
    // unconditional.
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
    g.set(
        "GetAreaSpiritHealerTime",
        lua.create_function(|_, ()| {
            Ok(i64::from(
                super::read_live(|l| l.area_spirit_healer_secs()).unwrap_or(0),
            ))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_clocks_read_zero_with_no_view_lent() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetInstanceBootTimeRemaining()")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return GetAreaSpiritHealerTime()").unwrap(),
            0
        );
        s.run("AcceptAreaSpiritHeal()").unwrap();
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
}
