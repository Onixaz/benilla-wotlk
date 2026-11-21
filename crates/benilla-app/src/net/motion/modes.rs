//! The movement modes the server grants a unit we do not control: the `SMSG_SPLINE_MOVE_*` state.

use benilla_protocol::SplineMode;
use bevy::prelude::*;

use crate::creature_anim::move_flags;

/// A streamed unit's server-granted movement modes as `MOVEMENTFLAGS` bits: root, water-walk,
/// feather-fall, hover, walk-mode and swim. The reference keeps one flags dword per unit
/// (the movement object at unit `+0x788`, word `+0x44`); the granted half is kept apart from
/// [`super::RemoteMotion::flags`] because the relay merge rewrites that word from every pose and a
/// creature sends none. Every unit's create or movement block seeds it with the block's standing
/// bits ([`standing_bits`], `0x6f1520` zeroes the word and merges), a creature's as a player's.
/// Absent means no modes; once inserted it lives as long as the entity, as the reference's word
/// lives as long as the unit.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct UnitMoveModes(pub(crate) u32);

/// The input latches `0x8000..=0x10_0000` and spline-enabled `0x40_0000`, in the 1.12.1-bit word.
const LATCHES_AND_SPLINE: u32 = 0x1f_8000 | 0x40_0000;

/// A flags word without its motion bits: what a path leaves (`0x98c770` writes the direction).
pub(crate) const fn standing_bits(flags: u32) -> u32 {
    flags
        & !(move_flags::INTEGRATED
            | move_flags::FALLING_FAR
            | LATCHES_AND_SPLINE
            | move_flags::ON_TRANSPORT)
}

impl UnitMoveModes {
    /// `apply` is the bit's direction; the run/walk opcodes' inversion is folded at the parse
    /// ([`SplineMode::WalkMode`]).
    pub(crate) fn set(&mut self, mode: SplineMode, apply: bool) {
        if apply {
            self.0 |= mode.flag();
        } else {
            self.0 &= !mode.flag();
        }
    }

    /// `MOVEFLAG_ROOT`: the unit cannot move, jump or be splined; the reference's server-position
    /// apply refuses while the bit is set.
    pub(crate) fn rooted(self) -> bool {
        self.0 & move_flags::ROOT != 0
    }

    /// `MOVEFLAG_HOVER`: the body rests [`crate::player::HOVER_HEIGHT`] above the floor.
    pub(crate) fn hovering(self) -> bool {
        self.0 & move_flags::HOVER != 0
    }

    /// `MOVEFLAG_WATERWALKING`: the liquid surface counts as walkable ground.
    pub(crate) fn water_walking(self) -> bool {
        self.0 & move_flags::WATER_WALKING != 0
    }
}

/// A unit we do not control holds the second flags word's swim-to-fly bit (`0x4000`): from
/// `MSG_MOVE_UPDATE_CAN_TRANSITION_BETWEEN_SWIM_AND_FLY` (`0x6ef1c0`) or its acked grant. Only the
/// active mover's collision query reads it (`0x75e3d0`), so it is kept, not used.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[component(storage = "SparseSet")]
pub(crate) struct SwimFlyTransition;

// `MOVEFLAG_SAFE_FALL` has no accessor: only our mover (`MoveModes::feather_fall`) and relayed
// players (their own pose flags) fall; a creature rides its spline and never does.

/// What the root apply clears from the flags word, as an AND mask: the direction, turn and pitch
/// bits (`0xff`), the `0x8000` latch and the input latches (`0x1f0000`); `ROOT` survives. A
/// one-shot wipe, not a gate: with no direction bits the unit fails the integration test
/// (`move_flags::INTEGRATED`) and stops rather than coasting. 3.3.5a's apply (`0x9884b8`,
/// `and 0xff203f00`) also clears ascending and descending.
pub(crate) const ROOT_APPLY_WIPE: u32 =
    0xffe0_7f00 & !(move_flags::ASCENDING | move_flags::DESCENDING);
