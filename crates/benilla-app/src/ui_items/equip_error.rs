//! The wire `InventoryResult` to the `GlobalStrings` key of its red error line.
//!
//! Reason 59 (`EQUIP_ERR_NONE`) only clears a pending lock (`ItemHandler.cpp:865`, `:963`), and
//! after a failed item use rides beside the real refusal (`SpellHandler.cpp:130`). The reference
//! maps it like any reason to an error key with no 1.12 string, which the error sink's
//! empty-string guard drops. Reason 0, the one coded suppression, is dropped earlier, by
//! `benilla_protocol::events`.

/// A refusal reason's key, total: [`inventory_error_key`], with a result that shows nothing as
/// the empty key the error sink drops. Reason 1's string carries a `%d` for the required level.
pub(crate) fn equip_error_key(reason: u8) -> &'static str {
    inventory_error_key(u32::from(reason)).unwrap_or("")
}

/// An `InventoryResult`'s key in 12340's table (`0x753bb0`): `None` for the results it maps to id
/// `0x2da`, one past the catalog, which shows nothing (`0x5216fc`); `ERR_BAG_FULL` past 91.
pub(crate) fn inventory_error_key(result: u32) -> Option<&'static str> {
    Some(match result {
        0 | 81 | 83 => return None,
        1 => "ERR_CANT_EQUIP_LEVEL_I",
        2 => "ERR_CANT_EQUIP_SKILL",
        3 => "ERR_WRONG_SLOT",
        // BAG_FULL and its BAG_FULL3/4/6 aliases; 51 is not one.
        4 | 53 | 56 | 62 | 74 => "ERR_BAG_FULL",
        5 => "ERR_BAG_IN_BAG",
        6 => "ERR_TRADE_EQUIPPED_BAG",
        7 => "ERR_AMMO_ONLY",
        8 => "ERR_PROFICIENCY_NEEDED",
        9 | 12 | 18 => "ERR_NO_SLOT_AVAILABLE",
        10 | 11 => "ERR_CANT_EQUIP_EVER",
        13 => "ERR_2HANDED_EQUIPPED",
        14 => "ERR_2HSKILLNOTFOUND",
        15 | 16 => "ERR_WRONG_BAG_TYPE",
        17 => "ERR_ITEM_MAX_COUNT",
        19 | 55 => "ERR_CANT_STACK",
        20 => "ERR_NOT_EQUIPPABLE",
        21 => "ERR_CANT_SWAP",
        22 => "ERR_SLOT_EMPTY",
        23 | 54 => "ERR_ITEM_NOT_FOUND",
        24 => "ERR_DROP_BOUND_ITEM",
        25 => "ERR_OUT_OF_RANGE",
        26 => "ERR_TOO_FEW_TO_SPLIT",
        27 => "ERR_SPLIT_FAILED",
        28 => "ERR_SPELL_FAILED_REAGENTS_GENERIC",
        29 => "ERR_NOT_ENOUGH_MONEY",
        30 => "ERR_NOT_A_BAG",
        31 => "ERR_DESTROY_NONEMPTY_BAG",
        32 => "ERR_NOT_OWNER",
        33 => "ERR_ONLY_ONE_QUIVER",
        34 => "ERR_NO_BANK_SLOT",
        35 => "ERR_NO_BANK_HERE",
        36 => "ERR_ITEM_LOCKED",
        37 => "ERR_GENERIC_STUNNED",
        38 => "ERR_PLAYER_DEAD",
        39 => "ERR_CLIENT_LOCKED_OUT",
        40 | 73 => "ERR_INTERNAL_BAG_ERROR",
        // ERR_ONLY_ONE_BOLT's 1.12 string also says "quiver".
        41 => "ERR_ONLY_ONE_BOLT",
        42 => "ERR_ONLY_ONE_AMMO",
        43 => "ERR_CANT_WRAP_STACKABLE",
        44 => "ERR_CANT_WRAP_EQUIPPED",
        45 => "ERR_CANT_WRAP_WRAPPED",
        46 => "ERR_CANT_WRAP_BOUND",
        47 => "ERR_CANT_WRAP_UNIQUE",
        48 => "ERR_CANT_WRAP_BAGS",
        49 => "ERR_LOOT_GONE",
        50 => "ERR_INV_FULL",
        // EQUIP_ERR_BANK_FULL: the reference sets errorId 1, `ERR_BANK_FULL`, though vmangos tags
        // it `ERR_BAG_FULL`.
        51 => "ERR_BANK_FULL",
        52 | 57 => "ERR_VENDOR_SOLD_OUT",
        58 => "ERR_OBJECT_IS_BUSY",
        // EQUIP_ERR_NONE, the lock-clear sentinel: a real key with no 1.12 string.
        59 => "ERR_CANT_BE_DISENCHANTED",
        60 => "ERR_NOT_IN_COMBAT",
        61 => "ERR_NOT_WHILE_DISARMED",
        63 => "ERR_CANT_EQUIP_RANK",
        64 => "ERR_CANT_EQUIP_REPUTATION",
        65 => "ERR_TOO_MANY_SPECIAL_BAGS",
        66 => "ERR_LOOT_CANT_LOOT_THAT_NOW",
        67 => "ERR_ITEM_UNIQUE_EQUIPPABLE",
        68 => "ERR_VENDOR_MISSING_TURNINS",
        69 => "ERR_NOT_ENOUGH_HONOR_POINTS",
        70 => "ERR_NOT_ENOUGH_ARENA_POINTS",
        71 => "ERR_ITEM_MAX_COUNT_SOCKETED",
        72 => "ERR_MAIL_BOUND_ITEM",
        75 => "ERR_ITEM_MAX_COUNT_EQUIPPED_SOCKETED",
        76 => "ERR_ITEM_UNIQUE_EQUIPPABLE_SOCKETED",
        77 => "ERR_TOO_MUCH_GOLD",
        78 => "ERR_NOT_DURING_ARENA_MATCH",
        79 => "ERR_TRADE_BOUND_ITEM",
        80 => "ERR_CANT_EQUIP_RATING",
        82 => "ERR_NOT_SAME_ACCOUNT",
        84 => "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS",
        85 => "ERR_ITEM_MAX_LIMIT_CATEGORY_SOCKETED_EXCEEDED_IS",
        86 => "ERR_SCALING_STAT_ITEM_LEVEL_EXCEEDED",
        87 => "ERR_PURCHASE_LEVEL_TOO_LOW",
        88 => "ERR_CANT_EQUIP_NEED_TALENT",
        89 => "ERR_ITEM_MAX_LIMIT_CATEGORY_EQUIPPED_EXCEEDED_IS",
        90 => "ERR_SHAPESHIFT_FORM_CANNOT_EQUIP",
        91 => "ERR_ITEM_INVENTORY_FULL_SATCHEL",
        // The default arm (`0x753dde`).
        _ => "ERR_BAG_FULL",
    })
}

#[cfg(test)]
mod tests {
    use super::{equip_error_key, inventory_error_key};

    /// Stunned and dead are 37 and 38 at 5875; 39 and 40 are "right now" and the bag error.
    #[test]
    fn equip_error_table_matches_the_5875_enum() {
        assert_eq!(equip_error_key(50), "ERR_INV_FULL");
        assert_eq!(equip_error_key(37), "ERR_GENERIC_STUNNED");
        assert_eq!(equip_error_key(38), "ERR_PLAYER_DEAD");
        assert_eq!(equip_error_key(39), "ERR_CLIENT_LOCKED_OUT");
        assert_eq!(equip_error_key(40), "ERR_INTERNAL_BAG_ERROR");
        assert_eq!(equip_error_key(1), "ERR_CANT_EQUIP_LEVEL_I");
    }

    /// The reference's table maps 51 to `ERR_BANK_FULL`, against vmangos's `ItemDefines.h` tag.
    #[test]
    fn a_full_bank_says_bank_not_bag() {
        assert_eq!(equip_error_key(51), "ERR_BANK_FULL");
        for alias in [4u8, 53, 56, 62] {
            assert_eq!(equip_error_key(alias), "ERR_BAG_FULL", "reason {alias}");
        }
    }

    /// A mount used in cat form answers `SPELL_FAILED_NO_ITEMS_WHILE_SHAPESHIFTED` and reason 59
    /// (`SpellHandler.cpp:130`); 59 must print nothing, or the refusal shows twice.
    #[test]
    fn the_lock_clear_sentinel_maps_to_the_stringless_key() {
        assert_eq!(equip_error_key(59), "ERR_CANT_BE_DISENCHANTED");
    }

    /// The drain swaps 16's line for the bag-family one when the bag resolves.
    #[test]
    fn the_wrong_bag_reasons_share_the_generic_key() {
        assert_eq!(equip_error_key(15), "ERR_WRONG_BAG_TYPE");
        assert_eq!(equip_error_key(16), "ERR_WRONG_BAG_TYPE");
    }

    /// `0x753de8`: 3.3.5a's rows past 66, the three that show nothing, and the default past 91.
    #[test]
    fn the_table_runs_to_91_and_clamps_past_it() {
        assert_eq!(equip_error_key(67), "ERR_ITEM_UNIQUE_EQUIPPABLE");
        assert_eq!(equip_error_key(79), "ERR_TRADE_BOUND_ITEM");
        assert_eq!(
            equip_error_key(84),
            "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS"
        );
        assert_eq!(equip_error_key(91), "ERR_ITEM_INVENTORY_FULL_SATCHEL");
        for shows_nothing in [0, 81, 83] {
            assert_eq!(inventory_error_key(shows_nothing), None);
        }
        assert_eq!(equip_error_key(92), "ERR_BAG_FULL");
        assert_eq!(equip_error_key(0xFF), "ERR_BAG_FULL");
    }

    /// Every key resolves in the shipped `GlobalStrings.lua` except reason 59's, which must not: a
    /// typo'd key would swallow a refusal in silence. Skips without client data.
    #[test]
    fn every_equip_error_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        // Reason 0 never reaches this table (gated at `events::decode`); 81 and 83 show nothing.
        for reason in (1..=91u8).filter(|r| ![81, 83].contains(r)) {
            let key = equip_error_key(reason);
            let text = g(key).unwrap_or_default();
            if reason == 59 {
                // Empty, which the drain's `is_empty()` skip drops as the reference's empty-string
                // guard does.
                assert!(
                    text.is_empty(),
                    "reason 59's {key} resolved to {text:?} — the duplicate line is back"
                );
                continue;
            }
            assert!(!text.is_empty(), "{key} (reason {reason}) missing");
        }
        assert_eq!(g(equip_error_key(50)).unwrap(), "Inventory is full.");
        assert_eq!(
            g(equip_error_key(1)).unwrap().replace("%d", "30"),
            "You must reach level 30 to use that item."
        );
        assert_eq!(g(equip_error_key(51)).unwrap(), "Your bank is full");
        assert_eq!(g(equip_error_key(92)).unwrap(), "That bag is full.");
        // Reason 16's generic line, and the bag-family one the drain substitutes.
        assert_eq!(
            g(equip_error_key(16)).unwrap(),
            "That item doesn't go in that container."
        );
        assert_eq!(
            g("ERR_WRONG_BAG_TYPE_SUBCLASS")
                .unwrap()
                .replace("%s", "Arrows"),
            "Only Arrows can be placed in that."
        );
    }
}
