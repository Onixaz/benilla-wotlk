//! The wire audit: rules every 3.3.5a client keeps on the movement wire, checked on the packets as
//! they cross the socket. The read thread feeds what the server granted and where it put each
//! unit; the write thread checks each movement packet under the guid it is stamped with. A broken
//! rule is a `warn!` and an `audit` trace line, and an unattended run that broke one exits non-zero
//! ([`exit_code`]).
//!
//! - **Granted modes**: a packet carries root, water walk, feather fall, hover, can-fly or gravity
//!   off for a guid only while the server holds that mode on it: granted and acked, or written by
//!   the server itself (a create, a movement block, a relayed pose of ours, a spline mode).
//!   `FLYING` needs can-fly.
//! - **One body's pose**: a packet for one unit never carries the pose last sent for another, as
//!   the server relocates the named unit to whatever a packet claims.
//! - **Owed acks**: a mode, speed, knockback, teleport or collision-height change with a counter is
//!   acked, once, with its guid and counter, within [`ACK_DEADLINE`], while that unit is ours to
//!   move; an ack the server never asked for is a rule broken too.
//!
//! The state is per world session: a new writer starts it over.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use benilla_protocol::messages::{
    MoveMode, CLASSIC_CAN_FLY, CLASSIC_DISABLE_GRAVITY, CLASSIC_FLYING,
};
use benilla_protocol::SessionEvent;

use super::{ClientCommand, VehicleCommand};

/// The movement-flag bits only the server grants: root, water walk, feather fall, hover, can-fly
/// and gravity off.
const MODE_BITS: u32 = 0x0000_1000
    | 0x1000_0000
    | 0x2000_0000
    | 0x4000_0000
    | CLASSIC_CAN_FLY
    | CLASSIC_DISABLE_GRAVITY;

/// How long a counted change may wait for its ack. vmangos and AzerothCore act on a missing ack
/// within seconds (anticheat, a held change).
pub(crate) const ACK_DEADLINE: Duration = Duration::from_secs(5);

/// A pose within this of another unit's last sent pose is that unit's.
const SAME_POSE: f32 = 0.05;
/// A pose within this of the unit's own last one is its own, whatever else sits there.
const OWN_POSE: f32 = 5.0;
/// Each rule's `warn!` lines, capped per session; the count is not.
const WARN_CAP: u32 = 20;

/// The rules, for the report.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Rule {
    UngrantedMode,
    FlyingWithoutCanFly,
    OtherBodysPose,
    MissingAck,
    UnaskedAck,
}

/// What kind of counted change an ack answers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum AckKind {
    Mode,
    Speed,
    KnockBack,
    Teleport,
    CollisionHeight,
}

#[derive(Default)]
pub(crate) struct Audit {
    /// Each unit's server-held mode bits.
    modes: HashMap<u64, u32>,
    /// Each unit's last pose, as sent by us or written by the server.
    poses: HashMap<u64, [f32; 3]>,
    /// The last pose we sent for each unit; only a sent pose can turn up under another guid.
    sent: HashMap<u64, [f32; 3]>,
    /// Counted changes not yet acked, with their arrival.
    owed: HashMap<(u64, u32, AckKind), Instant>,
    /// When each unit was last claimed as our mover.
    claimed: HashMap<u64, Instant>,
    /// The guid movement packets are stamped with now.
    mover: u64,
    counts: HashMap<Rule, u32>,
    total: u32,
}

static AUDIT: Mutex<Option<Audit>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Audit) -> R) -> R {
    let mut guard = AUDIT.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Audit::default))
}

/// A new world writer: the counted changes, claims and sent poses of the last session are void.
/// The modes stay, as a create re-states them, and a stale one can only hide a break.
pub(crate) fn reset(mover: u64) {
    with(|a| {
        a.owed.clear();
        a.claimed.clear();
        a.sent.clear();
        a.mover = mover;
        if mover != 0 {
            a.claimed.insert(mover, Instant::now());
        }
    });
}

/// The summary line, and the exit code an unattended run owes: 3 for any broken rule.
pub(crate) fn exit_code() -> i32 {
    let (total, counts) = with(|a| (a.total, a.counts.clone()));
    if total == 0 {
        bevy::log::info!("wire-audit: clean");
        return 0;
    }
    let mut rows: Vec<String> = counts.iter().map(|(r, n)| format!("{r:?} {n}")).collect();
    rows.sort();
    bevy::log::warn!("wire-audit: {total} rule(s) broken — {}", rows.join(", "));
    3
}

/// What the server sent, before the game thread sees it.
pub(crate) fn inbound(ev: &SessionEvent) {
    with(|a| a.inbound(ev, Instant::now()));
}

/// A command about to be written, stamped with the writer's mover.
pub(crate) fn outbound(cmd: &ClientCommand, stamped: u64) {
    with(|a| a.outbound(cmd, stamped, Instant::now()));
}

impl Audit {
    fn inbound(&mut self, ev: &SessionEvent, now: Instant) {
        match ev {
            SessionEvent::MoveMode {
                guid,
                counter,
                mode,
                ..
            } => {
                if *mode != MoveMode::SwimFlyTransition {
                    self.owed.insert((*guid, *counter, AckKind::Mode), now);
                }
            }
            SessionEvent::ForceSpeedChange { guid, counter, .. } => {
                self.owed.insert((*guid, *counter, AckKind::Speed), now);
            }
            SessionEvent::KnockBack { guid, counter, .. } => {
                self.owed.insert((*guid, *counter, AckKind::KnockBack), now);
            }
            SessionEvent::Teleport {
                guid,
                counter,
                position,
                ..
            } => {
                self.owed.insert((*guid, *counter, AckKind::Teleport), now);
                self.poses.insert(*guid, *position);
                self.sent.remove(guid);
            }
            SessionEvent::MoveSetCollisionHeight { guid, counter, .. } => {
                self.owed
                    .insert((*guid, *counter, AckKind::CollisionHeight), now);
            }
            SessionEvent::SplineMoveMode { guid, mode, apply } => {
                let word = self.modes.entry(*guid).or_default();
                if *apply {
                    *word |= mode.flag() & MODE_BITS;
                } else {
                    *word &= !mode.flag();
                }
            }
            SessionEvent::ObjectCreate {
                guid,
                position,
                mover: Some(m),
                ..
            } => self.server_pose(*guid, *position, m.flags),
            SessionEvent::ObjectMove {
                guid,
                position,
                mover,
                ..
            } => self.server_pose(*guid, *position, mover.flags),
            SessionEvent::UnitMove {
                guid,
                position,
                flags,
                ..
            } => self.server_pose(*guid, *position, *flags),
            SessionEvent::MonsterMove { guid, .. } => {
                // The server drives the unit along a path: wherever it ends is its own.
                self.sent.remove(guid);
                self.poses.remove(guid);
            }
            _ => {}
        }
    }

    /// A pose and flags word the server wrote for `guid`: its modes and where it is.
    fn server_pose(&mut self, guid: u64, position: [f32; 3], flags: u32) {
        *self.modes.entry(guid).or_default() |= flags & MODE_BITS;
        self.poses.insert(guid, position);
        self.sent.remove(&guid);
    }

    fn outbound(&mut self, cmd: &ClientCommand, stamped: u64, now: Instant) {
        self.mover = stamped;
        match cmd {
            ClientCommand::SetActiveMover { guid } => {
                self.mover = *guid;
                self.claimed.insert(*guid, now);
            }
            ClientCommand::Move { flags, pos, .. }
            | ClientCommand::MoveSplineDone { flags, pos, .. } => {
                self.pose_packet(stamped, *flags, *pos);
            }
            ClientCommand::NotActiveMover {
                guid, flags, pos, ..
            } => self.pose_packet(*guid, *flags, *pos),
            ClientCommand::Vehicle(VehicleCommand::Pitch {
                mover, flags, pos, ..
            })
            | ClientCommand::Vehicle(VehicleCommand::Controlled {
                mover, flags, pos, ..
            }) => self.pose_packet(*mover, *flags, *pos),
            ClientCommand::MoveModeAck {
                guid,
                counter,
                mode,
                apply,
                flags,
                pos,
                ..
            } => {
                self.acked(*guid, *counter, AckKind::Mode);
                let word = self.modes.entry(*guid).or_default();
                if *apply {
                    *word |= mode.flag() & MODE_BITS;
                } else {
                    *word &= !mode.flag();
                }
                self.pose_packet(*guid, *flags, *pos);
            }
            ClientCommand::ForceSpeedAck {
                guid,
                counter,
                flags,
                pos,
                ..
            } => {
                self.acked(*guid, *counter, AckKind::Speed);
                self.pose_packet(*guid, *flags, *pos);
            }
            ClientCommand::KnockBackAck {
                guid,
                counter,
                flags,
                pos,
                ..
            } => {
                self.acked(*guid, *counter, AckKind::KnockBack);
                self.pose_packet(*guid, *flags, *pos);
            }
            ClientCommand::CollisionHeightAck {
                guid,
                counter,
                flags,
                pos,
                ..
            } => {
                self.acked(*guid, *counter, AckKind::CollisionHeight);
                self.pose_packet(*guid, *flags, *pos);
            }
            ClientCommand::TeleportAck { guid, counter } => {
                self.acked(*guid, *counter, AckKind::Teleport);
            }
            _ => {}
        }
        self.overdue(now);
    }

    /// One packet claiming `guid` stands at `pos` with `flags`.
    fn pose_packet(&mut self, guid: u64, flags: u32, pos: [f32; 3]) {
        let held = self.modes.get(&guid).copied().unwrap_or(0);
        let ungranted = flags & MODE_BITS & !held;
        if ungranted != 0 {
            self.broke(
                Rule::UngrantedMode,
                format!("{guid:#x} sent mode bits {ungranted:#x} the server never granted (flags {flags:#x})"),
            );
        }
        if flags & CLASSIC_FLYING != 0 && held & CLASSIC_CAN_FLY == 0 {
            self.broke(
                Rule::FlyingWithoutCanFly,
                format!("{guid:#x} sent FLYING without can-fly (flags {flags:#x})"),
            );
        }
        let own = self
            .sent
            .get(&guid)
            .or_else(|| self.poses.get(&guid))
            .map(|p| dist(*p, pos));
        if own.is_some_and(|d| d > OWN_POSE) {
            if let Some((&other, _)) = self
                .sent
                .iter()
                .find(|(&g, p)| g != guid && dist(**p, pos) <= SAME_POSE)
            {
                self.broke(
                    Rule::OtherBodysPose,
                    format!(
                        "{guid:#x} sent the pose last sent for {other:#x} ({:.1} yd from its own): \
                         [{:.2}, {:.2}, {:.2}]",
                        own.unwrap_or(0.0),
                        pos[0],
                        pos[1],
                        pos[2]
                    ),
                );
            }
        }
        self.sent.insert(guid, pos);
        self.poses.insert(guid, pos);
    }

    fn acked(&mut self, guid: u64, counter: u32, kind: AckKind) {
        if self.owed.remove(&(guid, counter, kind)).is_none() {
            self.broke(
                Rule::UnaskedAck,
                format!("{kind:?} ack for {guid:#x} counter {counter} the server never asked for"),
            );
        }
    }

    /// Changes past their deadline on a unit claimed as ours since they arrived.
    fn overdue(&mut self, now: Instant) {
        let late: Vec<(u64, u32, AckKind)> = self
            .owed
            .iter()
            .filter(|(_, &at)| now.duration_since(at) > ACK_DEADLINE)
            .map(|(k, _)| *k)
            .collect();
        for key in late {
            let at = self.owed.remove(&key).expect("listed above");
            let ours = key.0 == self.mover || self.claimed.get(&key.0).is_some_and(|&c| c >= at);
            if ours {
                self.broke(
                    Rule::MissingAck,
                    format!(
                        "{:?} change for {:#x} counter {} never acked",
                        key.2, key.0, key.1
                    ),
                );
            }
        }
    }

    fn broke(&mut self, rule: Rule, what: String) {
        let n = self.counts.entry(rule).or_default();
        *n += 1;
        self.total += 1;
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line("audit", &format!("{rule:?} {what}"));
        }
        if *n <= WARN_CAP {
            bevy::log::warn!("wire-audit: {rule:?}: {what}");
        }
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MoveKind;

    const ME: u64 = 0x405;
    const EYE: u64 = 0xF130_006F_5F41_59F3;

    fn audit() -> Audit {
        Audit {
            mover: ME,
            ..Default::default()
        }
    }

    fn mv(flags: u32, pos: [f32; 3]) -> ClientCommand {
        ClientCommand::Move {
            kind: MoveKind::Heartbeat,
            flags,
            pos,
            orientation: 0.0,
            pitch: 0.0,
            fall_time: 0,
            jump: None,
            transport: None,
        }
    }

    fn grant(guid: u64, counter: u32, mode: MoveMode, apply: bool) -> SessionEvent {
        SessionEvent::MoveMode {
            guid,
            counter,
            mode,
            apply,
        }
    }

    fn ack(guid: u64, counter: u32, mode: MoveMode, apply: bool, flags: u32) -> ClientCommand {
        ClientCommand::MoveModeAck {
            guid,
            counter,
            mode,
            apply,
            flags,
            pos: [0.0; 3],
            orientation: 0.0,
            pitch: 0.0,
            fall_time: 0,
            jump: None,
            transport: None,
        }
    }

    /// The Eye of Acherus leak (#1186): the eye's flight on our own body after the hand-back.
    #[test]
    fn flight_on_a_body_never_granted_it_breaks_the_rule() {
        let now = Instant::now();
        let mut a = audit();
        a.inbound(&grant(EYE, 1, MoveMode::CanFly, true), now);
        a.outbound(&ClientCommand::SetActiveMover { guid: EYE }, ME, now);
        a.outbound(
            &ack(EYE, 1, MoveMode::CanFly, true, CLASSIC_CAN_FLY),
            EYE,
            now,
        );
        a.outbound(&mv(CLASSIC_CAN_FLY | CLASSIC_FLYING, [0.0; 3]), EYE, now);
        assert_eq!(a.total, 0, "the eye holds can-fly");
        a.outbound(&ClientCommand::SetActiveMover { guid: ME }, EYE, now);
        a.outbound(&mv(CLASSIC_FLYING, [0.0; 3]), ME, now);
        assert_eq!(a.counts.get(&Rule::FlyingWithoutCanFly), Some(&1));
        a.outbound(&mv(CLASSIC_CAN_FLY, [0.0; 3]), ME, now);
        assert_eq!(a.counts.get(&Rule::UngrantedMode), Some(&1));
    }

    /// A revoke takes the mode off once acked; before the ack the bit may still ride.
    #[test]
    fn a_mode_holds_from_its_ack_to_its_revoke_ack() {
        let now = Instant::now();
        let mut a = audit();
        a.inbound(&grant(ME, 1, MoveMode::Hover, true), now);
        a.outbound(&ack(ME, 1, MoveMode::Hover, true, 0x4000_0000), ME, now);
        a.inbound(&grant(ME, 2, MoveMode::Hover, false), now);
        a.outbound(&mv(0x4000_0000, [0.0; 3]), ME, now);
        assert_eq!(a.total, 0, "the revoke is not ours until acked");
        a.outbound(&ack(ME, 2, MoveMode::Hover, false, 0), ME, now);
        a.outbound(&mv(0x4000_0000, [0.0; 3]), ME, now);
        assert_eq!(a.counts.get(&Rule::UngrantedMode), Some(&1));
    }

    /// A mode the server wrote into a pose (a spline mode, a create) is held without an ack.
    #[test]
    fn a_server_written_mode_needs_no_ack() {
        let now = Instant::now();
        let mut a = audit();
        a.inbound(
            &SessionEvent::SplineMoveMode {
                guid: EYE,
                mode: benilla_protocol::messages::SplineMode::Root,
                apply: true,
            },
            now,
        );
        a.outbound(&mv(0x1000, [0.0; 3]), EYE, now);
        assert_eq!(a.total, 0);
    }

    /// The park after a hand-back (#1186): a stop for us at the eye's last pose.
    #[test]
    fn a_packet_carrying_another_body_s_pose_breaks_the_rule() {
        let now = Instant::now();
        let mut a = audit();
        a.outbound(&mv(0, [2323.2, -5662.4, 426.05]), ME, now);
        a.outbound(&mv(0, [1758.0, -5876.8, 166.9]), EYE, now);
        a.outbound(&mv(0, [1758.0, -5876.8, 166.9]), ME, now);
        assert_eq!(a.counts.get(&Rule::OtherBodysPose), Some(&1));
        // Walking up to where another unit stood is our own pose.
        a.outbound(&mv(0, [1758.5, -5876.8, 166.9]), ME, now);
        assert_eq!(a.total, 1);
    }

    /// A counted change for our mover goes unacked past the deadline; one for another unit that
    /// never becomes ours owes nothing.
    #[test]
    fn a_counted_change_on_our_mover_must_be_acked() {
        let t0 = Instant::now();
        let mut a = audit();
        a.inbound(&grant(ME, 4, MoveMode::Root, true), t0);
        a.inbound(&grant(EYE, 5, MoveMode::Root, true), t0);
        let later = t0 + ACK_DEADLINE + Duration::from_secs(1);
        a.outbound(&mv(0, [0.0; 3]), ME, later);
        assert_eq!(a.counts.get(&Rule::MissingAck), Some(&1));
        assert!(a.owed.is_empty());
    }

    /// An ack for a change the server never sent.
    #[test]
    fn an_ack_nobody_asked_for_breaks_the_rule() {
        let now = Instant::now();
        let mut a = audit();
        a.outbound(&ack(EYE, 9, MoveMode::Root, false, 0), ME, now);
        assert_eq!(a.counts.get(&Rule::UnaskedAck), Some(&1));
    }
}
