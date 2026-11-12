//! Shared fixtures for the script test modules.

use crate::script::{PaperDollSlotInfo, UiScript};

pub(super) fn script() -> UiScript {
    let script = UiScript::new().expect("construct UiScript");
    script.set_paperdoll_item_frame(paperdoll_rows());
    script
}

/// A synthetic slot table for the tests, which run without the install: the slot buttons they name,
/// each with a made-up texture.
pub(crate) fn paperdoll_rows() -> Vec<PaperDollSlotInfo> {
    let buttons = [
        ("AmmoSlot", 0),
        ("HeadSlot", 1),
        ("Trinket0Slot", 13),
        ("BackSlot", 15),
        ("MainHandSlot", 16),
        ("RangedSlot", 18),
        ("TabardSlot", 19),
        ("Bag0Slot", 20),
        ("Bag1Slot", 21),
    ]
    .map(|(name, slot)| (name.to_string(), slot));
    let bank = (1..=12).map(|n| (format!("Bag{n}"), 63 + n));
    buttons
        .into_iter()
        .chain(bank)
        .map(|(name, slot)| PaperDollSlotInfo {
            texture: format!("Test\\Slot\\{name}"),
            name,
            slot,
        })
        .collect()
}
