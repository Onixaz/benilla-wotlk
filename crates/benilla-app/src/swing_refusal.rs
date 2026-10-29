//! The melee swing refusal: what the client does with the server's answers to a
//! `CMSG_ATTACKSWING` it will not honour.
//!
//! Six opcodes, four arms: `0x145` NOTINRANGE and `0x146` BADFACING latch a message; `0x148`
//! DEADTARGET, `0x149` CANT_ATTACK and `0x14e` `SMSG_CANCEL_COMBAT` (whose body is arm 4's byte for
//! byte) stop the attack silently; `0x147` NOTSTANDING is never registered, and vmangos never sends
//! it.
//!
//! The server sends a refusal only on its edge (vmangos `Unit.cpp:406-472`), so the repeat the
//! player sees is the client's own latch and cooldown:
//!
//! ```text
//! (0x145 NOTINRANGE)  ecx = 1 ; call; latch code 1
//! (0x146 BADFACING)   ecx = 2 ; call; latch code 2
//! (0x148 DEADTARGET / 0x149 CANT_ATTACK)          ; no message, StopAttack only
//!
//! = code ; = OsGetAsyncTimeMs()    ; the code + "show at"
//!
//! The reference the local player's tick:
//!             this is the active player      else out
//!             [this+0xc48] != 0      else out   ; an attack target is set
//!             now - >= 0     else out
//! == 1 -> DisplayError(0xd8) | == 2 -> DisplayError(0xd7) | else out
//! = now + 0xfa0                                  ; re-arm, 4000 ms
//! ```
//!
//! So the first line shows on the next tick and repeats every 4 s while the latch stands. A landed
//! swing clears it: `SMSG_ATTACKERSTATEUPDATE`'s arm calls when the attacker is the active player
//! and the victim resolves, and its first act. A second refusal rewrites the deadline to now,
//! defeating the cooldown, and the re-arm precedes the print, so it happens even when the string is
//! missing. Both edges ride one message ([`SwingRefusalEdge`]) so they are consumed in packet
//! order.

use std::time::Duration;

use benilla_protocol::messages::AttackSwingError;
use bevy::prelude::*;

use crate::creature_anim::{AttackSeam, Engaged};
use crate::net::SelfPlayer;
use crate::ui_action::{UiError, UiErrorKeys};

/// The reference's re-arm interval.
const REPEAT: Duration = Duration::from_millis(0xfa0);

/// One edge of the swing-refusal seam, off the net drain in packet order.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) enum SwingRefusalEdge {
    /// The server refused our swing (`0x145`/`0x146`/`0x148`/`0x149`).
    Refused(AttackSwingError),
    /// `SMSG_CANCEL_COMBAT` (`0x14e`): the server forced our attack to stop (`Unit::CombatStop`,
    /// `Unit::StopAttackFaction`, `Unit::InterruptAttacksOnMe`, a resisted feign death).
    CombatCancelled,
    /// One of our swings landed on a victim we can. The write site applies both gates: the attacker
    /// is the local player and the victim resolves as a unit; benilla checks presence only, which
    /// is equivalent since the victim is always a unit or player.
    Landed,
}

/// The latched refusal: the reference's (the code) and (the absolute time the next line is due).
/// `None` is code 0, whose timestamp the tick never reads.
#[derive(Resource, Default)]
struct SwingRefusal(Option<(Latched, Duration)>);

/// The two refusals that latch; arm 4 writes no latch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Latched {
    /// Code 1: `DisplayError(0xd8)`, `ERR_BADATTACKPOS`.
    NotInRange,
    /// Code 2: `DisplayError(0xd7)`, `ERR_BADATTACKFACING`.
    BadFacing,
}

impl Latched {
    /// The `GlobalStrings.lua` key the reference's error id resolves to.
    const fn key(self) -> &'static str {
        match self {
            Self::NotInRange => "ERR_BADATTACKPOS",
            Self::BadFacing => "ERR_BADATTACKFACING",
        }
    }
}

impl SwingRefusal {
    /// `(code)`: latch the refusal, due now.
    fn latch(&mut self, code: Latched, now: Duration) {
        self.0 = Some((code, now));
    }

    /// .
    fn clear(&mut self) {
        self.0 = None;
    }
}

/// Consume this frame's edges in packet order: latch a refusal, clear on a landing, and stop the
/// attack for the silent arm.
fn apply_swing_refusals(
    mut edges: MessageReader<SwingRefusalEdge>,
    time: Res<Time>,
    mut refusal: ResMut<SwingRefusal>,
    mut seam: AttackSeam,
    engaged: Query<(), (With<SelfPlayer>, With<Engaged>)>,
) {
    for edge in edges.read() {
        match edge {
            SwingRefusalEdge::Refused(AttackSwingError::NotInRange) => {
                debug!("swing refused: out of range");
                refusal.latch(Latched::NotInRange, time.elapsed());
            }
            SwingRefusalEdge::Refused(AttackSwingError::BadFacing) => {
                debug!("swing refused: bad facing");
                refusal.latch(Latched::BadFacing, time.elapsed());
            }
            // No latch, no message, only the stop, whose own engaged guard decides whether it sends
            // anything.
            SwingRefusalEdge::Refused(AttackSwingError::DeadOrUnattackable)
            | SwingRefusalEdge::CombatCancelled => {
                debug!("swing stopped by the server — silently ({edge:?})");
                seam.stop(!engaged.is_empty());
            }
            SwingRefusalEdge::Landed => refusal.clear(),
        }
    }
}

/// The reference's per-player tick, gated in its order: an attack target set ([`Engaged`] is
/// `[+0xc48]`), the cooldown, then the code; it re-arms only when a line shows. It runs once per
/// rendered frame, as an `Update` system does.
fn show_swing_refusal(
    time: Res<Time>,
    mut refusal: ResMut<SwingRefusal>,
    engaged: Query<(), (With<SelfPlayer>, With<Engaged>)>,
    mut errors: ResMut<UiErrorKeys>,
) {
    let Some((code, due)) = refusal.0 else {
        return;
    };
    if engaged.is_empty() {
        return; // `[+0xc48] == 0`: no attack target
    }
    let now = time.elapsed();
    if now < due {
        return;
    }
    refusal.0 = Some((code, now + REPEAT));
    errors.0.push(UiError::key(code.key()));
}

/// Entering the world drops the latch: the reference's, from `ClientInitializeGame`, which runs
/// once per world entry.
fn on_world_enter(mut refusal: ResMut<SwingRefusal>) {
    refusal.clear();
}

pub(crate) struct SwingRefusalPlugin;

impl Plugin for SwingRefusalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SwingRefusal>()
            .add_message::<SwingRefusalEdge>()
            .add_systems(
                Update,
                // After the drain that writes the edges and before the unit feeds, so a queued
                // key shows the same frame; chained so a refusal is latched before the tick.
                (apply_swing_refusals, show_swing_refusal)
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(crate::ui_unit::UnitFeed),
            )
            .add_systems(
                OnEnter(crate::char_select::ClientState::InWorld),
                on_world_enter,
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// The latch makes the line due immediately; the re-arm is 4 s later.
    #[test]
    fn the_first_line_is_immediate_and_the_repeat_is_four_seconds() {
        let mut r = SwingRefusal::default();
        r.latch(Latched::NotInRange, secs(100));
        assert_eq!(
            r.0,
            Some((Latched::NotInRange, secs(100))),
            "due now, not in 4 s"
        );
        assert_eq!(secs(100) + REPEAT, Duration::from_millis(104_000));
    }

    /// A second refusal defeats the 4 s cooldown: the reference writes the deadline
    /// unconditionally.
    #[test]
    fn a_second_refusal_packet_defeats_the_cooldown() {
        let mut r = SwingRefusal::default();
        r.latch(Latched::NotInRange, secs(100));
        // The tick shows the line and re-arms for 104 s,
        r.0 = Some((Latched::NotInRange, secs(100) + REPEAT));
        // and a second packet one second later puts it due now.
        r.latch(Latched::BadFacing, secs(101));
        assert_eq!(r.0, Some((Latched::BadFacing, secs(101))));
    }

    /// A landed swing clears it.
    #[test]
    fn a_landed_swing_clears_the_latch() {
        let mut r = SwingRefusal::default();
        r.latch(Latched::BadFacing, secs(5));
        r.clear();
        assert_eq!(r.0, None);
    }

    /// The keys are the catalog's error rows `0xe7` and `0xe6`.
    #[test]
    fn the_two_keys_are_the_reference_error_ids() {
        let pos = benilla_ui::messages::by_key(Latched::NotInRange.key()).expect("0xe7 is a row");
        let facing = benilla_ui::messages::by_key(Latched::BadFacing.key()).expect("0xe6 is a row");
        assert_eq!(pos.id, 0xe7);
        assert_eq!(facing.id, 0xe6);
        // Both go to the red error line, by the catalog's own `kind`.
        assert_eq!(pos.kind, benilla_ui::messages::MsgKind::Error);
        assert_eq!(facing.kind, benilla_ui::messages::MsgKind::Error);
    }
}
