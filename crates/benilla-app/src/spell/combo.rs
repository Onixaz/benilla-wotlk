//! Combo points, which 3.3.5a keeps off the descriptor: `SMSG_UPDATE_COMBO_POINTS` banks ours on
//! one unit in the game UI, `SMSG_PET_UPDATE_COMBO_POINTS` the pet's in the pet info (`0x5d34b0`).
//! Every packet fires `UNIT_COMBO_POINTS`, changed or not; the UI feed reads the epochs.

use benilla_protocol::SessionEvent;
use bevy::prelude::*;

/// One bank of combo points and the unit they sit on.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Banked {
    pub(crate) target: u64,
    pub(crate) points: u8,
}

impl Banked {
    /// `GetComboPoints(target)` (`0x513920`, the pet's `0x5d3520`): the points while `target`, or
    /// the current target for 0, is the banked unit; 0 otherwise.
    pub(crate) fn on(self, target: u64, current_target: u64) -> u8 {
        let target = if target == 0 { current_target } else { target };
        if target == self.target {
            self.points
        } else {
            0
        }
    }
}

/// The game UI's combo bank (`0xbd08a8` target, `0xbd084d` count) and the pet info's, each with a
/// count of the packets applied.
#[derive(Resource, Default, Debug)]
pub(crate) struct ComboPoints {
    pub(crate) player: Banked,
    pub(crate) pet: Banked,
    pub(crate) player_epoch: u64,
    pub(crate) pet_epoch: u64,
}

/// Registers the two combo handlers.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    use benilla_protocol::SessionEventKind as K;
    app.net_handler(K::ComboPoints, on_combo_points)
        .net_handler(K::PetComboPoints, on_combo_points);
}

/// Both packets. The pet's is kept only for the pet the pet info holds (`0x5d36a0`).
fn on_combo_points(
    In(ev): In<SessionEvent>,
    mut combo: ResMut<ComboPoints>,
    pet_bar: Res<crate::ui_pet::PetBar>,
) {
    match ev {
        SessionEvent::ComboPoints { target, points } => {
            debug!("net: combo points {points} on {target:#x}");
            combo.player = Banked { target, points };
            combo.player_epoch += 1;
        }
        SessionEvent::PetComboPoints {
            pet,
            target,
            points,
        } if pet_bar.has_bar() && pet_bar.spells.pet_guid == pet => {
            debug!("net: pet combo points {points} on {target:#x}");
            combo.pet = Banked { target, points };
            combo.pet_epoch += 1;
        }
        _ => {}
    }
}

/// The session's start zeroes both banks (`0x52b550`, `0x5d6d90`).
pub(super) fn clear_combo_points(mut combo: ResMut<ComboPoints>) {
    combo.player = Banked::default();
    combo.pet = Banked::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_read_only_on_the_banked_unit_and_zero_means_the_target() {
        let banked = Banked {
            target: 7,
            points: 3,
        };
        assert_eq!(banked.on(7, 0), 3);
        assert_eq!(banked.on(8, 7), 0, "another unit");
        assert_eq!(banked.on(0, 7), 3, "0 reads the current target");
        assert_eq!(banked.on(0, 8), 0);
        assert_eq!(Banked::default().on(0, 0), 0);
    }
}
