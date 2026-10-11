//! Action-bar messages: the 144-slot login snapshot, the one-slot write, and the bar toggles.
//! The bar is client-authoritative: the server stores the slots, sends them at login, and takes
//! one `CMSG_SET_ACTION_BUTTON` per local change.

use std::io::{self};

use crate::wire::{read_u32_le, read_u8};

/// The slot words of a 12340 `SMSG_ACTION_BUTTONS`: the handler reads `0x90` (`0x6d87be`).
pub const ACTION_BUTTON_SLOTS: usize = 144;

/// The action-button kind, bits 24-31 of a slot word (`ActionButtonType`, vmangos `Player.h`).
pub const ACTION_KIND_SPELL: u8 = 0x00;
pub const ACTION_KIND_MACRO: u8 = 0x40;
pub const ACTION_KIND_ITEM: u8 = 0x80;

/// An occupied slot of `SMSG_ACTION_BUTTONS`'s packed `u32`s; a zero word is an empty slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionButton {
    /// The bar slot index (0..143). Slots 0–11 are the main bar's buttons 1–12.
    pub slot: u8,
    /// The spell/macro/item id (bits 0–23).
    pub action: u32,
    /// The kind byte (bits 24-31), an `ACTION_KIND_*`; the enum's 0x01 ("click?") is carried raw.
    pub kind: u8,
}

/// Read `SMSG_ACTION_BUTTONS` as the 12340 handler does (`0x6d8750`): a `u8` state, then for
/// state 0 or 1 the [`ACTION_BUTTON_SLOTS`] slot words, dropping empty ones. Only state 1 puts the
/// words on the bar (`0x6d87d3`); state 0 stores them, and state 2 carries no words. `None` is a
/// body that sets no buttons.
pub(super) fn read_action_buttons(r: &mut &[u8]) -> io::Result<Option<Vec<ActionButton>>> {
    let state = read_u8(r)?;
    if state > 1 {
        return Ok(None);
    }
    let mut buttons = Vec::new();
    for slot in 0..ACTION_BUTTON_SLOTS as u8 {
        let packed = read_u32_le(r)?;
        if packed != 0 {
            buttons.push(ActionButton {
                slot,
                action: packed & 0x00FF_FFFF,
                kind: (packed >> 24) as u8,
            });
        }
    }
    Ok((state == 1).then_some(buttons))
}

/// `CMSG_SET_ACTION_BUTTON` (296): `button u8`, `packetData u32` packed as in [`ActionButton`]
/// (`Server/Packets/Misc.cpp:87-90`). Zero clears the slot (no reply); a drag-swap is two sends.
pub fn set_action_button(button: u8, packed: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(5);
    body.push(button);
    body.extend_from_slice(&packed.to_le_bytes());
    body
}

/// `CMSG_SET_ACTIONBAR_TOGGLES`: one `u8`, the whole body (`SetActionBarToggles 0x4e76e0`).
/// The byte is server-owned: the reference never writes its copy (`PLAYER_FIELD_BYTES` byte 2),
/// which changes only when `SMSG_UPDATE_OBJECT` echoes it, with no change event; read it with
/// [`super::ObjectFields::player_action_bar_toggles`]. The reference packs only bits 0..3, so a
/// set clears the high nibble; this encoder does not mask. Bits name bars only in FrameXML.
pub fn set_actionbar_toggles(toggles: u8) -> Vec<u8> {
    vec![toggles]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The head of AzerothCore's login body, captured live: state 1, then slot words.
    #[test]
    fn the_login_snapshot_puts_144_slots_on_the_bar() {
        let mut body = vec![0x01];
        body.extend(0x19cbu32.to_le_bytes()); // 6603 Auto Attack
        body.extend(0x0b9du32.to_le_bytes()); // 2973 Raptor Strike
        body.extend(0x004bu32.to_le_bytes()); // 75 Auto Shot
        body.resize(1 + 4 * ACTION_BUTTON_SLOTS, 0);
        let mut r = &body[..];
        let buttons = read_action_buttons(&mut r)
            .unwrap()
            .expect("state 1 sets the bar");
        assert!(r.is_empty());
        assert_eq!(
            buttons
                .iter()
                .map(|b| (b.slot, b.action))
                .collect::<Vec<_>>(),
            [(0, 6603), (1, 2973), (2, 75)]
        );
        // A short body is an error, not a partial bar.
        assert!(read_action_buttons(&mut &body[..100]).is_err());
        // State 0 stores without setting; state 2 carries nothing.
        body[0] = 0;
        assert_eq!(read_action_buttons(&mut &body[..]).unwrap(), None);
        assert_eq!(read_action_buttons(&mut &[2u8][..]).unwrap(), None);
    }
}
