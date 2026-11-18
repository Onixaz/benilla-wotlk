//! The verbs the stock `StaticPopup.lua` dialogs call for the instance-boot and area spirit healer clocks, each read live at the call; the battlefield port answer is `battlefield_queue`'s.

use mlua::Lua;

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

    // The poll up to its drop's cancel, the cancel's event inside the call while the deadline still reads, then the rest of the poll and the queue send naming the healer it left (`0x524b60`).
    g.set(
        "AcceptAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            let Some(cancelled) = super::write_live(|l| l.area_spirit_poll_drop()).flatten() else {
                return Ok(());
            };
            if cancelled {
                super::tick::fire_event_into(lua, "AREA_SPIRIT_HEALER_OUT_OF_RANGE", Vec::new());
            }
            super::write_live(|l| l.accept_area_spirit_heal(cancelled));
            Ok(())
        })?,
    )?;
    // The reference's aura cancel for 2584 (`0x522fa0` -> `0x51f710`): nothing without our body
    // (`0x51f75c`), else the event inside the call (`0x803290`), then `0x136` with `u32 2584`, the
    // wave deadline untouched. Its refusal gate (`AttributesEx` bit 13 set, bit 2 clear) never trips
    // for 2584 in the shipped `Spell.dbc` (the app's `spell_2584_never_trips_the_cancel_gate`).
    g.set(
        "CancelAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            if !super::read_live(|l| l.spirit_cancel_has_body()).unwrap_or(false) {
                return Ok(());
            }
            super::tick::fire_event_into(lua, "AREA_SPIRIT_HEALER_OUT_OF_RANGE", Vec::new());
            super::read_live(|l| l.cancel_area_spirit_heal());
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

    /// A view that holds our body or not.
    struct Body(bool);

    impl crate::script::LiveState for Body {
        fn unit_flags(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn unit_health(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn active_player(&self) -> Option<u64> {
            self.0.then_some(1)
        }

        fn has_control(&self) -> bool {
            true
        }

        fn far_sight(&self) -> bool {
            false
        }

        fn spirit_cancel_has_body(&self) -> bool {
            self.0
        }
    }

    /// `CancelAreaSpiritHeal` once under a view holding our body or not; the event as the handler saw it by the verb's return.
    fn cancel_with_body(body: bool) -> Option<String> {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"F = CreateFrame("Frame") F:RegisterEvent("AREA_SPIRIT_HEALER_OUT_OF_RANGE")
               F:SetScript("OnEvent", function() GOT = event end)"#,
        )
        .unwrap();
        s.with_live(&mut Body(body), |s| {
            s.run("CancelAreaSpiritHeal() AT_RETURN = GOT").unwrap()
        });
        s.eval::<Option<String>>("return AT_RETURN").unwrap()
    }

    #[test]
    fn cancel_area_spirit_heal_fires_out_of_range_inside_its_call() {
        assert_eq!(
            cancel_with_body(true).as_deref(),
            Some("AREA_SPIRIT_HEALER_OUT_OF_RANGE"),
            "the event has run by the time the verb returns, no tick between"
        );
    }

    /// `0x51f710` skips the aura cancel when it finds no player object (`0x51f75c`).
    #[test]
    fn cancel_area_spirit_heal_does_nothing_without_our_body() {
        assert_eq!(cancel_with_body(false), None);
    }
}
