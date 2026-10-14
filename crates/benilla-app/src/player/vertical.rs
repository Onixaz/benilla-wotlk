//! 3.3.5a's vertical channel. The `JUMP` and `SITORSTAND` keys hold the input control word's
//! ascend (`0x2000`) and descend (`0x4000`) bits, and each change of a held bit resyncs the mover
//! (`0x5fbbc0`), whose vertical leg (`0x5face0`) drives the mover's `ASCENDING` and `DESCENDING`
//! flags while it swims or flies, through move commands 6, 7 and 8 (executor `0x6ef860`). Either
//! flag moves the mover straight up or down at its current speed, or at 45° along its level
//! heading when it also translates (`0x987b50`); an ascent near the surface jumps out
//! (`0x730d10`).

use bevy::math::Vec3;

use crate::creature_anim::move_flags;
use crate::net::MoveKind;

/// The surface-hop band (yd, `[0xa12098]` = 1/3): an ascending swimmer whose depth over the feet
/// is at most the swim depth plus this jumps (`0x730e5c`-`0x730e7a`).
const SURFACE_HOP_BAND: f32 = 1.0 / 3.0;

/// The channel's two halves: the mover's flags and the input side's latch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Vertical {
    /// `MOVEFLAG_ASCENDING` (3.3.5a `0x40_0000`).
    ascending: bool,
    /// `MOVEFLAG_DESCENDING` (3.3.5a `0x80_0000`).
    descending: bool,
    /// The input control's `0x100000`: the last resync left a command standing, so a resync that
    /// finds the same direction sends nothing.
    active: bool,
    /// The command a resync executed this frame, for the move stream.
    sent: Option<MoveKind>,
}

impl Vertical {
    /// The vertical leg of the resync `0x5fbbc0`, run on a frame a held input bit changed: `up`
    /// and `down` are the two held bits, `swim_or_fly` the mover's `SWIMMING | FLYING` at the call
    /// and `may_move` its move predicate (`0x5fac90`: alive, unrooted, not stand state 7).
    pub(super) fn resync(&mut self, up: bool, down: bool, swim_or_fly: bool, may_move: bool) {
        if !may_move {
            // `0x5fbd2f`: a mover that may not move stops the channel (command 8).
            if self.active {
                self.stop(swim_or_fly);
                self.active = false;
            }
            return;
        }
        // `0x5facf3`: neither swimming nor flying, the latch drops and nothing is sent.
        if !swim_or_fly {
            self.active = false;
            return;
        }
        match i8::from(up) - i8::from(down) {
            0 => {
                if self.active {
                    self.stop(swim_or_fly);
                    self.active = false;
                }
            }
            // A standing command is re-sent only over the opposite flag.
            1 if !self.active || self.descending => {
                self.ascending = true;
                self.sent = Some(MoveKind::StartAscend);
                self.active = true;
            }
            -1 if !self.active || self.ascending => {
                self.descending = true;
                self.sent = Some(MoveKind::StartDescend);
                self.active = true;
            }
            _ => {}
        }
    }

    /// StopAscend (`0x989940`): clears both flags and sends `STOP_ASCEND`, only while swimming or
    /// flying.
    fn stop(&mut self, swim_or_fly: bool) {
        if swim_or_fly {
            self.ascending = false;
            self.descending = false;
            self.sent = Some(MoveKind::StopAscend);
        }
    }

    /// The flags end without a packet of their own: StopSwim (`0x98bff0`) and StartFalling
    /// (`0x988370`) mask them off with SWIMMING, and the root apply keeps neither (`0x9884b8`,
    /// `and 0xff203f00`). The input latch stays, so only a held-bit change re-arms the ascent.
    pub(super) fn end(&mut self) {
        self.ascending = false;
        self.descending = false;
    }

    /// The command this frame's resync executed, taken by the move stream.
    pub(super) fn take_sent(&mut self) -> Option<MoveKind> {
        self.sent.take()
    }

    /// The two flags in the flag word's bits.
    pub(super) fn flags(&self) -> u32 {
        let mut f = 0;
        if self.ascending {
            f |= move_flags::ASCENDING;
        }
        if self.descending {
            f |= move_flags::DESCENDING;
        }
        f
    }

    /// The vertical sign: `0x987b50`'s pure arm negates only without ASCENDING, so a mover holding
    /// both rises.
    pub(super) fn direction(&self) -> f32 {
        if self.ascending {
            1.0
        } else if self.descending {
            -1.0
        } else {
            0.0
        }
    }

    /// The swim decision's hop (`0x730e52`-`0x730e7a`): ascending, with the depth over the feet
    /// `depth` at most `0.75·h + 1/3`, the mover jumps (`0x72eb80`).
    pub(super) fn surface_hop(&self, depth: f32, h: f32) -> bool {
        self.ascending && depth - super::swim::swim_enter_depth(h) <= SURFACE_HOP_BAND
    }
}

/// The swim stroke's unit direction under the channel (`0x987b50`): straight up or down alone
/// (cases `0x10`/`0x12`), else the level heading `level` and the vertical at 45° each
/// (`0x987700`, both × `0.70710677`); the pitch is not read.
pub(super) fn stroke(level: Vec3, direction: f32) -> Vec3 {
    let level = Vec3::new(level.x, 0.0, level.z).normalize_or_zero();
    (level + Vec3::Y * direction).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resynced(v: &mut Vertical, up: bool, down: bool) -> Option<MoveKind> {
        v.resync(up, down, true, true);
        v.take_sent()
    }

    /// Space in water starts the ascent, its release stops it, and X mirrors it downward.
    #[test]
    fn each_half_starts_on_its_bit_and_the_one_stop_ends_either() {
        let mut v = Vertical::default();
        assert_eq!(resynced(&mut v, true, false), Some(MoveKind::StartAscend));
        assert_eq!(v.flags(), move_flags::ASCENDING);
        assert_eq!(v.direction(), 1.0);
        assert_eq!(resynced(&mut v, true, false), None, "the latch holds");
        assert_eq!(resynced(&mut v, false, false), Some(MoveKind::StopAscend));
        assert_eq!(v.flags(), 0);
        assert_eq!(resynced(&mut v, false, false), None, "nothing to stop");
        assert_eq!(resynced(&mut v, false, true), Some(MoveKind::StartDescend));
        assert_eq!(v.direction(), -1.0);
        assert_eq!(resynced(&mut v, false, false), Some(MoveKind::StopAscend));
        assert_eq!(v.flags(), 0, "STOP_ASCEND clears descending too");
    }

    /// Both held net to zero, which stops; releasing one then starts the other.
    #[test]
    fn both_keys_cancel_and_the_survivor_restarts() {
        let mut v = Vertical::default();
        resynced(&mut v, true, false);
        assert_eq!(resynced(&mut v, true, true), Some(MoveKind::StopAscend));
        assert_eq!(resynced(&mut v, false, true), Some(MoveKind::StartDescend));
        assert_eq!(v.flags(), move_flags::DESCENDING);
    }

    /// On land the channel is inert: no command and no flag, and the latch drops.
    #[test]
    fn on_land_the_channel_sends_nothing() {
        let mut v = Vertical::default();
        v.resync(true, false, false, true);
        assert_eq!(v.take_sent(), None);
        assert_eq!(v.flags(), 0);
        // Into the water with Space still down: no resync, no ascent, until a held bit changes.
        assert_eq!(v, Vertical::default());
    }

    /// A surface hop ends the flags but not the latch, so a re-entry with Space still held does not
    /// rise again until a held bit changes; a fresh press then re-arms it.
    #[test]
    fn a_hop_leaves_the_latch_so_only_a_new_press_rearms() {
        let mut v = Vertical::default();
        resynced(&mut v, true, false);
        v.end();
        assert_eq!(v.flags(), 0);
        assert_eq!(
            resynced(&mut v, true, false),
            None,
            "the latch still stands, so an unrelated key's resync starts nothing"
        );
        resynced(&mut v, false, false);
        assert_eq!(resynced(&mut v, true, false), Some(MoveKind::StartAscend));
    }

    /// A mover that may not move stops a standing command at its next resync.
    #[test]
    fn a_gated_mover_stops_the_channel() {
        let mut v = Vertical::default();
        resynced(&mut v, true, false);
        v.resync(true, false, true, false);
        assert_eq!(v.take_sent(), Some(MoveKind::StopAscend));
        assert_eq!(v.flags(), 0);
    }

    /// The hop band: at most 1/3 yd under the swim depth, and only ascending.
    #[test]
    fn the_hop_waits_for_the_last_third_of_a_yard() {
        let h = 2.0;
        let rest = super::super::swim::swim_enter_depth(h);
        let mut v = Vertical::default();
        assert!(!v.surface_hop(rest, h), "not ascending");
        resynced(&mut v, true, false);
        assert!(v.surface_hop(rest, h));
        assert!(v.surface_hop(rest + SURFACE_HOP_BAND - 0.001, h));
        assert!(!v.surface_hop(rest + SURFACE_HOP_BAND + 0.001, h));
    }

    /// Alone the stroke is vertical; with a heading it is 45°, whatever the heading's own pitch.
    #[test]
    fn the_stroke_is_vertical_or_forty_five_degrees() {
        assert_eq!(stroke(Vec3::ZERO, 1.0), Vec3::Y);
        assert_eq!(stroke(Vec3::ZERO, -1.0), Vec3::NEG_Y);
        let s = stroke(Vec3::new(0.0, 0.5, -1.0), 1.0);
        let k = std::f32::consts::FRAC_1_SQRT_2;
        assert!((s - Vec3::new(0.0, k, -k)).length() < 1e-6, "{s}");
    }
}
