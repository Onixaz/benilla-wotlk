//! The bank bindings: verbs that read the purchased slots and the banker at the call (`0x57bab0`, `0x57bb30`), `PurchaseSlot`'s send (`0x57bbc0`) and `CloseBankFrame`'s close (`0x57b9c0`); the vault is containers `-1` and `5..=11` (`BankFrame.xml`).

use mlua::{Lua, MultiValue, Value};

/// Whether a banker is open (`0xbeb9a0`), through the lent view; none lent, none open.
pub(crate) fn bank_open() -> bool {
    super::read_live(|l| l.banker().is_some()).unwrap_or(false)
}

/// The purchased bank bag slots through the lent view, 0 with no active player (`0x57bae1`).
fn bank_bag_slots() -> u8 {
    super::read_live(|l| l.bank_bag_slots())
        .flatten()
        .unwrap_or(0)
}

/// `CloseBankFrame` (`0x57b9c0`), the banker setter with no banker (`0x57b8d0`): `BANKFRAME_CLOSED` with the banker still set, then, with the interact target the banker, the interaction close's bank leg fires it again (`0x57b8f1` -> `0x518d50` -> `0x512fa7`), then the banker clears.
fn close_bank_frame(lua: &Lua) {
    super::tick::fire_event_into(lua, "BANKFRAME_CLOSED", Vec::new());
    if super::write_live(|l| l.close_bank_interaction()).unwrap_or(false) {
        super::tick::fire_event_into(lua, "BANKFRAME_CLOSED", Vec::new());
    }
    super::write_live(|l| l.clear_banker());
}

impl super::UiScript {
    /// The interaction close's bank leg from a window opened in the client (`0x51fd1c` -> `0x512fa7`), taken only while the interact target is the banker: `BANKFRAME_CLOSED` with the banker still set, then the banker clears.
    pub fn close_bank_interaction(&mut self) {
        if super::write_live(|l| l.close_bank_interaction()).unwrap_or(false) {
            self.fire_event("BANKFRAME_CLOSED", Vec::new());
            super::write_live(|l| l.clear_banker());
        }
    }
}

/// Register the bank globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumBankSlots() → numSlots, full: the active player's byte, banker or not, `full` 1 from seven up (`0x57bab0`).
    g.set(
        "GetNumBankSlots",
        lua.create_function(|_, ()| {
            let n = bank_bag_slots();
            let full = if n >= 7 {
                Value::Integer(1)
            } else {
                Value::Nil
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(i64::from(n)),
                full,
            ]))
        })?,
    )?;

    // GetBankSlotCost(numSlots): the argument is unread; the price row after the purchased count (`0x57bb30`).
    g.set(
        "GetBankSlotCost",
        lua.create_function(|_, _n: Option<Value>| {
            let cost = super::read_live(|l| {
                l.bank_bag_slots()
                    .map_or(0, |n| l.bank_slot_cost(u32::from(n) + 1))
            })
            .unwrap_or(0);
            Ok(i64::from(cost))
        })?,
    )?;

    // PurchaseSlot(): the `CONFIRM_BUY_BANK_SLOT` popup's accept (`StaticPopup.lua:52`); its refusal line shows from the call (`0x57bc2c`).
    g.set(
        "PurchaseSlot",
        lua.create_function(|lua, ()| {
            if let Some(key) = super::read_live(|l| l.purchase_bank_slot()).flatten() {
                super::ui_errors::show_now(lua, key);
            }
            Ok(())
        })?,
    )?;

    g.set(
        "CloseBankFrame",
        lua.create_function(|lua, ()| {
            close_bank_frame(lua);
            Ok(())
        })?,
    )?;

    // BankButtonIDToInvSlotID(id, isBag) (`0x57b940`): `id + 39` for an item button, `id + 63`
    // for a bag button, whose id is the container id 5..=11 (`BankFrame.xml`), the same answer as
    // `ContainerIDToInventoryID`'s bank-bag arm. There is no range check. `isBag` is
    // `lua_isnumber(L, 2)` (`0x57b982`), not truthiness: `true` takes the item arm, `0` and `"1"`
    // the bag arm; stock sets `self.isBag = 1` (`BankFrame.lua:20`). Argument 1 truncates toward
    // zero (`0x88b9c0`), and its usage string names only `buttonID`.
    g.set(
        "BankButtonIDToInvSlotID",
        lua.create_function(|lua, (id, is_bag): (Value, Option<Value>)| {
            let n = super::binding_abi::number_arg(
                lua,
                id,
                "Usage: BankButtonIDToInvSlotID(buttonID)",
            )?;
            let is_bag = is_bag
                .and_then(|v| lua.coerce_number(v).ok().flatten())
                .is_some();
            let step = if is_bag { 63 } else { 39 };
            Ok(i64::from(n.wrapping_add(step)))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// A view with `slots` purchased, the two price rows past it, and `banker` open.
    fn bank_view(slots: Option<u8>, banker: Option<u64>) -> crate::script::live::TestView {
        crate::script::live::TestView {
            bank_slots: slots,
            bank_costs: [(3, 100_000), (7, 250_000), (8, 999_999_999)].into(),
            banker,
            bank_interacting: banker.is_some(),
            ..Default::default()
        }
    }

    /// The count and price read the active player at the call, banker or not (`0x57bab0`, `0x57bb30`).
    #[test]
    fn bank_slots_and_cost_read_the_player_at_the_call() {
        let s = UiScript::new().unwrap();
        let counts = |s: &UiScript| {
            s.eval::<String>("local n, full = GetNumBankSlots() return n .. ',' .. tostring(full) .. ',' .. GetBankSlotCost(0)").unwrap()
        };
        assert_eq!(counts(&s), "0,nil,0", "no view, no player");
        {
            let _l = crate::script::live::lend_test(bank_view(None, None));
            assert_eq!(counts(&s), "0,nil,0", "no active player");
        }
        {
            let _l = crate::script::live::lend_test(bank_view(Some(2), None));
            assert_eq!(
                counts(&s),
                "2,nil,100000",
                "no banker: the count and the next row still answer"
            );
        }
        {
            let _l = crate::script::live::lend_test(bank_view(Some(6), Some(0x42)));
            assert_eq!(counts(&s), "6,nil,250000", "six is not yet full");
        }
        {
            let _l = crate::script::live::lend_test(bank_view(Some(7), Some(0x42)));
            assert_eq!(
                counts(&s),
                "7,1,999999999",
                "seven is full; the row past it is the sentinel"
            );
        }
    }

    /// `CloseBankFrame` fires `BANKFRAME_CLOSED` with the banker still set, again from the interaction close while the interact target is the banker, then clears it (`0x57b8d0`, `0x512fa7`).
    #[test]
    fn close_bank_frame_fires_twice_with_the_banker_set_then_clears_it() {
        let s = UiScript::new().unwrap();
        s.run("SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('BANKFRAME_CLOSED') f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = tostring(BankOpenNow()) end)").unwrap();
        s.lua()
            .globals()
            .set(
                "BankOpenNow",
                s.lua()
                    .create_function(|_, ()| Ok(super::bank_open()))
                    .unwrap(),
            )
            .unwrap();
        let seen = |s: &UiScript| {
            s.eval::<String>("local t = table.concat(SEEN, ',') SEEN = {} return t")
                .unwrap()
        };
        let mut loan = crate::script::live::lend_test(bank_view(Some(1), Some(0x42)));
        s.run("CloseBankFrame()").unwrap();
        assert_eq!(seen(&s), "true,true");
        assert!(!super::bank_open(), "the banker cleared in the call");
        s.run("CloseBankFrame()").unwrap();
        assert_eq!(seen(&s), "false", "with no bank, the lone fire");
        drop(loan);
        // A banker whose interact target already went: one fire.
        loan = crate::script::live::lend_test(crate::script::live::TestView {
            bank_interacting: false,
            ..bank_view(Some(1), Some(0x42))
        });
        s.run("CloseBankFrame()").unwrap();
        assert_eq!(seen(&s), "true");
        drop(loan);
    }

    /// A window opened in the client takes the interact target: the bank's leg fires `BANKFRAME_CLOSED` once with the banker set, then clears it; with the interact target gone, or no banker, it does nothing (`0x51fd1c` -> `0x512fa7`).
    #[test]
    fn a_client_open_closes_the_bank_only_while_its_banker_holds_the_target() {
        let mut s = UiScript::new().unwrap();
        s.lua()
            .globals()
            .set(
                "BankOpenNow",
                s.lua()
                    .create_function(|_, ()| Ok(super::bank_open()))
                    .unwrap(),
            )
            .unwrap();
        s.run("SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('BANKFRAME_CLOSED') f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = tostring(BankOpenNow()) end)").unwrap();
        let seen = |s: &UiScript| {
            s.eval::<String>("local t = table.concat(SEEN, ',') SEEN = {} return t")
                .unwrap()
        };
        let loan = crate::script::live::lend_test(bank_view(Some(1), Some(0x42)));
        s.close_bank_interaction();
        assert_eq!(seen(&s), "true", "one fire, the banker still set");
        assert!(!super::bank_open(), "then the banker clears");
        s.close_bank_interaction();
        assert_eq!(seen(&s), "", "nothing left to close");
        drop(loan);
        let loan = crate::script::live::lend_test(crate::script::live::TestView {
            bank_interacting: false,
            ..bank_view(Some(1), Some(0x42))
        });
        s.close_bank_interaction();
        assert_eq!(seen(&s), "", "the interact target is no longer the banker");
        assert!(super::bank_open(), "the banker stays");
        drop(loan);
    }

    /// The bank bags are inventory slots 68..=74, as the reference's bank reads them
    /// (`BankFrame.lua:2`).
    #[test]
    fn bank_bag_slots_read_through_the_inventory_api() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 68) == nil")
            .unwrap());

        let mut bags: crate::script::BankBagSlots = Default::default();
        bags[0] = Some(crate::script::InvSlotView {
            item_id: 4500,
            icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            link: Some("|cffffffff|Hitem:4500:0:0:0|h[Traveler\'s Backpack]|h|r".into()),
            equip_slots: vec![20, 21, 22, 23],
            ..Default::default()
        });
        s.set_bank_bag_slots(bags);

        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(5, 1)")
                .unwrap(),
            68,
            "BankFrameBag1's own id is 5 — the container id, not a bag number"
        );
        assert_eq!(
            s.eval::<String>("return GetInventoryItemTexture(\"player\", 68)")
                .unwrap(),
            "Interface\\Icons\\INV_Misc_Bag_08"
        );
        assert_eq!(
            s.eval::<i64>(
                "local _, _, id = string.find(GetInventoryItemLink(\"player\", 68), \"item:(%d+)\") \
                 return tonumber(id)"
            )
            .unwrap(),
            4500
        );
        // Slot 69 is empty; neither it nor the last bag slot, 74, falls through to the doll.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 69) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 74) == nil")
            .unwrap());
    }

    /// The reference paints the vault through the inventory API (`BankFrame.lua:35`) while the app
    /// feeds it as container `-1`; if the two disagree the bank draws empty.
    #[test]
    fn the_bank_band_reads_the_vault_through_the_inventory_api() {
        let mut s = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            3,
            super::super::ContainerSlot {
                item_id: 4496,
                count: 7,
                texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
                quality: Some(2),
                link: Some("|cff1eff00|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
                ..Default::default()
            },
        );
        s.set_container(
            -1,
            Some(super::super::ContainerState {
                bag_family: 0,
                name: Some("Bank".into()),
                num_slots: 28,
                slots,
            }),
        );

        // Vault slot 3 is inventory id 42.
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(3)").unwrap(),
            42
        );
        assert_eq!(
            s.eval::<String>("return GetInventoryItemTexture(\"player\", 42)")
                .unwrap(),
            "Interface\\Icons\\INV_Misc_Bag_08"
        );
        assert_eq!(
            s.eval::<i64>("return GetInventoryItemCount(\"player\", 42)")
                .unwrap(),
            7
        );
        assert_eq!(
            s.eval::<i64>(
                "local _, _, id = string.find(GetInventoryItemLink(\"player\", 42), \"item:(%d+)\") \
                 return tonumber(id)"
            )
            .unwrap(),
            4496
        );
        // An empty vault slot answers nil rather than falling through to `inventory_slots`.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 43) == nil")
            .unwrap());
        // The doll is untouched either side of the band.
        assert!(s
            .eval::<bool>("return GetInventoryItemTexture(\"player\", 16) == nil")
            .unwrap());
    }

    /// Item buttons 1..=28 map to 40..=67 and bag buttons, whose ids are the container ids 5..=11,
    /// to 68..=74; the wire slot is one less.
    #[test]
    fn bank_button_to_inv_slot() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(1)").unwrap(),
            40
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(28)").unwrap(),
            67
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(5, 1)")
                .unwrap(),
            68,
            "BankFrameBag1 carries id 5"
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(11, 1)")
                .unwrap(),
            74,
            "BankFrameBag7 carries id 11"
        );
        // The same map as `ContainerIDToInventoryID` for a bank bag; a drift would put the bag row
        // and the bag windows on different slots.
        for id in 5..=11 {
            assert_eq!(
                s.eval::<i64>(&format!("return BankButtonIDToInvSlotID({id}, 1)"))
                    .unwrap(),
                s.eval::<i64>(&format!("return ContainerIDToInventoryID({id})"))
                    .unwrap()
            );
        }
    }

    #[test]
    fn bank_button_to_inv_slot_follows_the_carved_abi() {
        let s = UiScript::new().unwrap();

        // `isBag` is `lua_isnumber(L, 2)`: `true` takes the item arm as nil and false do, while
        // `0` and `"1"` take the bag arm.
        for (call, want) in [
            ("BankButtonIDToInvSlotID(5, true)", 44),
            ("BankButtonIDToInvSlotID(5, false)", 44),
            ("BankButtonIDToInvSlotID(5, nil)", 44),
            ("BankButtonIDToInvSlotID(5, {})", 44),
            ("BankButtonIDToInvSlotID(5)", 44),
            ("BankButtonIDToInvSlotID(5, 0)", 68),
            ("BankButtonIDToInvSlotID(5, \"1\")", 68),
        ] {
            assert_eq!(
                s.eval::<i64>(&format!("return {call}")).unwrap(),
                want,
                "{call}"
            );
        }

        // No range check: the only compares in `0x57b940` test `lua_isnumber`'s result.
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(25)").unwrap(),
            64
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(-100)")
                .unwrap(),
            -61
        );

        // Argument 1 truncates toward zero (`0x88b9c0`), and the usage string names only
        // `buttonID`.
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(-2.9)")
                .unwrap(),
            37,
            "trunc toward zero gives -2, not -3"
        );
        assert_eq!(
            s.eval::<i64>("return BankButtonIDToInvSlotID(\"7\")")
                .unwrap(),
            46,
            "a numeric string passes the gate"
        );
        let err = s
            .eval::<i64>("return BankButtonIDToInvSlotID({})")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: BankButtonIDToInvSlotID(buttonID)"),
            "the reference's own usage string, verbatim: {err}"
        );
    }
}
