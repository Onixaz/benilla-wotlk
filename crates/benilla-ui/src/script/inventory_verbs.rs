//! Four verbs of the reference's inventory registrar beside the paper-doll getters in
//! [`super::char_stats`]: the keyring's slot map, the doll's sell cursor, the doll portrait setter
//! and the alert recompute.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{number_arg, string_arg};
use super::char_stats::{alert_events, inventory_slot_arg, recompute_inventory_alerts};
use super::container::UiCursorMode;
use super::region::{clear_texture, this_texture};

/// Register the four globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // KeyRingButtonIDToInvSlotID(buttonID) (`0x5e7700`): the truncated id plus 86 (`0x5e7739`), so
    // key 1 is inventory slot 87, with no range check; a non-number raises.
    g.set(
        "KeyRingButtonIDToInvSlotID",
        lua.create_function(|lua, id: Value| {
            let id = number_arg(lua, id, "Usage: KeyRingButtonIDToInvSlotID(buttonID)")?;
            Ok(i64::from(id.wrapping_add(86)))
        })?,
    )?;

    // ShowInventorySellCursor(slot): the Buy cursor over a worn item. The armed-spell and
    // base-cursor gates run before the argument is read; past them a slot the reader refuses, the
    // ammo slot, an empty slot or a locked item leave the cursor as it was (`0x5ea930`). Zero returns. The stock doll's call is commented out
    // (`PaperDollFrame.lua:754-756`).
    g.set(
        "ShowInventorySellCursor",
        lua.create_function(|lua, slot: Value| {
            {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                // Repair mode and the targeting arm move the base.
                if model.spell_targeting || model.repair_mode {
                    return Ok(());
                }
            }
            let Some(slot0) = inventory_slot_arg(lua, &slot)?.filter(|&s| s != -1) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            let sellable = usize::try_from(slot0 + 1)
                .ok()
                .and_then(|slot| model.inv_slot("player", slot))
                .is_some_and(|v| v.item_id != 0 && !v.locked);
            if sellable {
                model.ui_cursor = Some(UiCursorMode::Buy);
                model.ui_cursor_dirty = true;
            }
            Ok(())
        })?,
    )?;

    // SetInventoryPortraitTexture(texture, unit, slot) (`0x5ea9b0`): the texture is cleared before
    // anything else is read; a non-string unit raises, a refused slot or a unit that is no player
    // returns blank, else the worn item's icon (the ammo for slot 0) is painted round (`0x5eaadb`).
    g.set(
        "SetInventoryPortraitTexture",
        lua.create_function(|lua, (texture, unit, slot): (Value, Value, Value)| {
            let rh = this_texture(lua, &texture)?;
            clear_texture(&mut crate::script::model_mut_of(lua), rh);
            let token = string_arg(
                lua,
                unit,
                "Usage: SetInventoryPortraitTexture(texture, unit, slot)",
            )?;
            let Some(slot0) = inventory_slot_arg(lua, &slot)? else {
                return Ok(());
            };
            let icon = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                // The player-type test on the resolved unit (`0x5eaa59`).
                if !model.unit(&token).is_some_and(|u| u.is_player) {
                    return Ok(());
                }
                // Slot -1 finds the active player's ammo in the unit's bags, only ours (`0x5eaa7a`).
                let view = match usize::try_from(slot0 + 1) {
                    Ok(0) if !token.eq_ignore_ascii_case("player") => None,
                    Ok(live) => model.inv_slot(&token, live),
                    Err(_) => None,
                };
                view.filter(|v| v.item_id != 0).and_then(|v| v.icon)
            };
            let mut model = crate::script::model_mut_of(lua);
            let data = model.region_data.entry(rh).or_default();
            data.texture = icon;
            data.circular = true;
            Ok(())
        })?,
    )?;

    // UpdateInventoryAlertStatus() (`0x5eac00`): the alert recompute (`0x5e90d0`) and its events;
    // zero returns.
    g.set(
        "UpdateInventoryAlertStatus",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            let changed = recompute_inventory_alerts(&mut model);
            for event in alert_events(changed) {
                model.pending_events.push((event.to_string(), Vec::new()));
            }
            Ok(MultiValue::new())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{
        ContainerSlot, ContainerState, InvSlotView, QuadContent, UiCursorMode, UiScript, UnitState,
    };

    fn keyring_with_key() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            2,
            ContainerSlot {
                item_id: 5396,
                texture: Some("Interface\\Icons\\INV_Misc_Key_03".into()),
                count: 1,
                link: Some("|cffffffff|Hitem:5396:0:0:0|h[Key to Searing Gorge]|h|r".into()),
                ..Default::default()
            },
        );
        ContainerState {
            bag_family: 0,
            name: None,
            num_slots: 4,
            slots,
        }
    }

    #[test]
    fn keyring_button_ids_map_past_the_buyback_band() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(1)")
                .unwrap(),
            87
        );
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(32)")
                .unwrap(),
            118
        );
        // No range check, a truncating cast and a numeric string, as `0x5e7700` reads them.
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(0)")
                .unwrap(),
            86
        );
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID(-2.9)")
                .unwrap(),
            84
        );
        assert_eq!(
            s.eval::<i64>("return KeyRingButtonIDToInvSlotID('3')")
                .unwrap(),
            89
        );
        let err = s
            .run("KeyRingButtonIDToInvSlotID()")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: KeyRingButtonIDToInvSlotID(buttonID)"),
            "{err}"
        );
    }

    /// The keyring band answers the inventory API from the keyring container, so the stock hover
    /// (`ContainerFrame.lua:617`) reads a key through `GetInventoryItem*` and the tooltip.
    #[test]
    fn a_key_reads_through_its_inventory_slot() {
        let mut s = UiScript::new().unwrap();
        s.set_container(-2, Some(keyring_with_key()));
        assert_eq!(
            s.eval::<Option<String>>(
                "return GetInventoryItemTexture('player', KeyRingButtonIDToInvSlotID(2))"
            )
            .unwrap()
            .as_deref(),
            Some("Interface\\Icons\\INV_Misc_Key_03")
        );
        assert_eq!(
            s.eval::<Option<String>>("return GetInventoryItemTexture('player', 87)")
                .unwrap(),
            None,
            "key slot 1 is empty"
        );
        assert_eq!(
            s.eval::<i64>("return GetInventoryItemCount('player', 88)")
                .unwrap(),
            1
        );
    }

    #[test]
    fn inventory_sell_cursor_buys_over_a_worn_item_only() {
        let mut s = UiScript::new().unwrap();
        s.set_paperdoll_item_frame(crate::script::tests::common::paperdoll_rows());
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[16] = Some(InvSlotView {
            item_id: 2361,
            ..Default::default()
        });
        slots[17] = Some(InvSlotView {
            item_id: 2362,
            locked: true,
            ..Default::default()
        });
        s.set_inventory_slots(slots);

        for quiet in ["0", "1", "17"] {
            s.run(&format!("ShowInventorySellCursor({quiet})")).unwrap();
            assert_eq!(s.ui_cursor(), None, "slot {quiet}: ammo, empty or locked");
        }
        s.run("ShowInventorySellCursor(16)").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
        assert_eq!(
            s.eval::<i64>(
                "local n = function(...) return arg.n end return n(ShowInventorySellCursor(16))"
            )
            .unwrap(),
            0
        );

        // A slot the reader refuses returns, and a name is its row number read 0-based.
        s.run("ResetCursor()").unwrap();
        for refused in ["24", "39", "75", "119", "-1", "'MainHandSlot'"] {
            s.run(&format!("ShowInventorySellCursor({refused})"))
                .unwrap();
            assert_eq!(s.ui_cursor(), None, "{refused}");
        }
        s.run("ShowInventorySellCursor('BackSlot')").unwrap();
        assert_eq!(s.ui_cursor(), Some(UiCursorMode::Buy));
    }

    /// The two cursor gates come before the argument read.
    #[test]
    fn inventory_sell_cursor_under_repair_mode_is_silent() {
        let mut s = UiScript::new().unwrap();
        s.set_merchant(Some(crate::script::MerchantState {
            can_repair: true,
            ..Default::default()
        }));
        s.run("ShowRepairCursor()").unwrap();
        s.run("ShowInventorySellCursor(999)").unwrap();
        assert_eq!(s.ui_cursor(), None);
    }

    fn portrait_script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"local f = CreateFrame("Frame", "PortHost", UIParent)
               f:SetPoint("TOPLEFT", 0, 0)
               f:SetWidth(64); f:SetHeight(64)
               PortTex = f:CreateTexture("PortTex")
               PortTex:SetAllPoints(f)
               PortText = f:CreateFontString("PortText")"#,
        )
        .unwrap();
        s.set_screen_size(800.0, 600.0);
        let mut slots: crate::script::InventorySlots = Default::default();
        slots[0] = Some(InvSlotView {
            item_id: 2512,
            icon: Some("Interface\\Icons\\INV_Ammo_Arrow_02".into()),
            ..Default::default()
        });
        slots[1] = Some(InvSlotView {
            item_id: 10050,
            icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
            ..Default::default()
        });
        s.set_inventory_slots(slots);
        for (token, is_player) in [("player", true), ("target", false)] {
            s.set_unit(
                token,
                Some(UnitState {
                    exists: true,
                    is_player,
                    ..Default::default()
                }),
            );
        }
        s
    }

    /// Paint a stale icon, run `call`, and read the drawn quad's path and round mask back.
    fn portrait_after(s: &mut UiScript, call: &str) -> (Option<String>, bool) {
        s.run(r#"PortTex:SetTexture("Interface\\Icons\\INV_Misc_Bag_08")"#)
            .unwrap();
        s.run(call).unwrap();
        s.resolve();
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Texture { path, circular, .. } => Some((path, circular)),
                _ => None,
            })
            .unwrap_or((None, false))
    }

    #[test]
    fn inventory_portrait_paints_the_worn_item_round() {
        let mut s = portrait_script();
        assert_eq!(
            portrait_after(&mut s, "SetInventoryPortraitTexture(PortTex, 'player', 1)"),
            (Some("Interface\\Icons\\INV_Helmet_01".into()), true)
        );
        assert_eq!(
            portrait_after(&mut s, "SetInventoryPortraitTexture(PortTex, 'player', 0)"),
            (Some("Interface\\Icons\\INV_Ammo_Arrow_02".into()), true),
            "slot 0 is the ammo leg"
        );
        assert_eq!(
            s.arity("SetInventoryPortraitTexture(PortTex, 'player', 1)")
                .unwrap(),
            0
        );
        assert!(
            s.eval::<bool>("return SetInventoryPortaitTexture == nil")
                .unwrap(),
            "1.12.1's spelling is not a 3.3.5a global"
        );
    }

    /// The clear runs first, so every quiet return leaves the texture blank.
    #[test]
    fn inventory_portrait_quiet_returns_leave_it_blank() {
        let mut s = portrait_script();
        for call in [
            "SetInventoryPortraitTexture(PortTex, 'player', 30)",
            "SetInventoryPortraitTexture(PortTex, 'player', 75)",
            "SetInventoryPortraitTexture(PortTex, 'target', 1)",
            "SetInventoryPortraitTexture(PortTex, 'focus', 1)",
            "SetInventoryPortraitTexture(PortTex, 'player', 2)",
            "SetInventoryPortraitTexture(PortTex, 'target', 0)",
        ] {
            assert_eq!(portrait_after(&mut s, call).0, None, "{call}");
        }
    }

    #[test]
    fn inventory_portrait_raises_after_the_clear() {
        let s = portrait_script();
        s.run(r#"PortTex:SetTexture("Interface\\Icons\\INV_Misc_Bag_08")"#)
            .unwrap();
        let err = s
            .run("SetInventoryPortraitTexture(PortTex, nil, 1)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: SetInventoryPortraitTexture(texture, unit, slot)"),
            "{err}"
        );
        assert_eq!(
            s.eval::<Option<String>>("return PortTex:GetTexture()")
                .unwrap(),
            None
        );

        for (call, message) in [
            (
                "SetInventoryPortraitTexture(1, 'player', 1)",
                "non-table object",
            ),
            (
                "SetInventoryPortraitTexture({}, 'player', 1)",
                "non-framescript object",
            ),
            (
                "SetInventoryPortraitTexture(PortText, 'player', 1)",
                "Wrong object type for member function",
            ),
            (
                "SetInventoryPortraitTexture(PortHost, 'player', 1)",
                "Wrong object type for member function",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(message), "{call}: {err}");
        }
    }

    #[test]
    fn update_inventory_alert_status_fires_alerts_on_a_change_and_durability_always() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"SEEN = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("UPDATE_INVENTORY_ALERTS")
               f:RegisterEvent("UPDATE_INVENTORY_DURABILITY")
               f:SetScript("OnEvent", function() SEEN[event] = (SEEN[event] or 0) + 1 end)"#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<i64>(
                "local n = function(...) return arg.n end return n(UpdateInventoryAlertStatus())"
            )
            .unwrap(),
            0
        );
        s.tick(0.0);
        let counts = |s: &UiScript| {
            s.eval::<(i64, i64)>(
                "return SEEN.UPDATE_INVENTORY_ALERTS or 0, SEEN.UPDATE_INVENTORY_DURABILITY or 0",
            )
            .unwrap()
        };
        // Nothing equipped: every status stays 0, so only the durability event.
        assert_eq!(counts(&s), (0, 1));

        // The model behind the verb, a broken helm seated without the setter's own recompute.
        s.model_mut().inventory_slots[1] = Some(InvSlotView {
            item_id: 7,
            durability: Some((0, 40)),
            ..Default::default()
        });
        s.run("UpdateInventoryAlertStatus()").unwrap();
        s.tick(0.0);
        assert_eq!(counts(&s), (1, 2));
        s.run("UpdateInventoryAlertStatus()").unwrap();
        s.tick(0.0);
        assert_eq!(counts(&s), (1, 3));
    }
}
