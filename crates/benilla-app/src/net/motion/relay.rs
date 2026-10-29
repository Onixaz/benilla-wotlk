//! When a remote unit's relayed move replays: the reference's per-unit replay chain.
//!
//! An inbound `MSG_MOVE_*` gets a fire-time and waits in the unit's queue until then. A moving unit
//! replays on the sender's cadence, `fire = prev fire + (wire stamp - prev wire stamp)`; only an
//! idle mover with nothing queued re-bases, by the worst lateness of the last 32 packets. The
//! offset from arrival stays in `[-500, +1000]` ms. The state is five cells on the unit's
//! `CMovement`: `+0xa8`, `+0xac`, `+0xbc[32]`, `+0x13c`, `+0x140`.

use benilla_protocol::{JumpInfo, RelayVerb, TransportPose};

/// One relayed move as it came off the wire, before [`RelayChain`] schedules it.
#[derive(Clone)]
pub(crate) struct RelayMove {
    /// The server's ms clock at receipt, one clock for every mover (vmangos `Object.cpp:64`,
    /// written back at `Object.cpp:194`).
    pub(crate) wire_ms: u32,
    pub(crate) position: [f32; 3],
    pub(crate) orientation: f32,
    pub(crate) flags: u32,
    pub(crate) pitch: f32,
    pub(crate) fall_time: u32,
    pub(crate) jump: Option<JumpInfo>,
    pub(crate) transport: Option<TransportPose>,
    /// What the opcode means on top of the pose.
    pub(crate) verb: RelayVerb,
}

impl RelayMove {
    /// Whether this move arms the pre-fire facing and position blends: every relay but a teleport
    /// (node tag `0x26`); a heartbeat blends like any other.
    pub(crate) fn reconciles(&self) -> bool {
        self.verb != RelayVerb::Teleport
    }
}

/// A scheduled relayed move, the reference's queued node (fire-time at `node[+8]`, pose at
/// `node[+0x10]`).
#[derive(Clone)]
pub(crate) struct PendingMove {
    /// On the real-time ms clock.
    pub(crate) fire_ms: f64,
    pub(crate) mv: RelayMove,
}

/// The skew clamp (`skew + 0x1f4` saturated into `[0, 0x5dc]`).
const SKEW_MIN_MS: f64 = -500.0;
const SKEW_MAX_MS: f64 = 1000.0;

/// The reference's ring at `CMovement+0xbc`, cursor at `+0x13c` masked `& 0x1f`.
const LATENESS_WINDOW: usize = 32;

/// The mid-motion mask that blocks a re-base (`test [esi+0x40],0x20ff`): direction, turn and pitch
/// bits and `FALLING`.
const BUSY_MASK: u32 = 0x20ff;

/// A remote unit's replay chain, one per mover: two movers on different paths carry different
/// buffers.
#[derive(Clone, Default)]
pub(crate) struct RelayChain {
    /// `CMovement+0x40` bit 31, set with the first seed.
    seeded: bool,
    /// `+0xa8`: the previous packet's fire-time.
    last_fire_ms: f64,
    /// `+0xac`: the previous wire stamp, advanced only by a forward step.
    last_wire_ms: u32,
    /// `+0xbc[32]`: each packet's lateness stored as `base + lateness` at the time, so a spike
    /// already in the base is not charged twice.
    ring: [f64; LATENESS_WINDOW],
    /// `+0x13c`.
    ring_idx: usize,
    /// `+0x140`: the buffer the chain holds.
    base_ms: f64,
}

impl RelayChain {
    /// The move's fire-time; `flags` and `queue_empty` are the mover's state before it applies
    /// (`[esi+0x40]`, `[esi+0x150]`). The fire is stored; one at or before `now_ms` is due now.
    pub(crate) fn schedule(
        &mut self,
        wire_ms: u32,
        now_ms: f64,
        flags: u32,
        queue_empty: bool,
    ) -> f64 {
        // The first packet fires at arrival.
        if !self.seeded {
            self.seeded = true;
            self.last_wire_ms = wire_ms;
            self.last_fire_ms = now_ms;
        }
        // The server's `u32` ms clock wraps; a non-forward step adds nothing (-`c2`).
        let step = wire_ms.wrapping_sub(self.last_wire_ms) as i32;
        let wire_delta = if step > 0 {
            self.last_wire_ms = wire_ms;
            f64::from(step)
        } else {
            0.0
        };
        // `skew` makes `now + skew == last_fire + wire_delta`.
        let arrival_delta = now_ms - self.last_fire_ms;
        let mut skew = wire_delta - arrival_delta;
        let window_max = self.record_lateness(arrival_delta - wire_delta);
        // Only a standing mover with nothing queued re-bases (the re-base-`d41`), never stepping
        // back past the previous fire.
        if flags & BUSY_MASK == 0 && queue_empty {
            skew = (skew + window_max - self.base_ms).clamp(SKEW_MIN_MS, SKEW_MAX_MS);
            if now_ms + skew < self.last_fire_ms {
                skew = self.last_fire_ms - now_ms;
            }
            self.base_ms = window_max;
        }
        let fire_ms = now_ms + skew.clamp(SKEW_MIN_MS, SKEW_MAX_MS);
        self.last_fire_ms = fire_ms;
        fire_ms
    }

    /// `MSG_MOVE_TIME_SKIPPED`: `[CMovement+0xac] += lag` and nothing else; skipped, the next
    /// packet would fire `lag_ms` late.
    pub(crate) fn skip_time(&mut self, lag_ms: u32) {
        if self.seeded {
            self.last_wire_ms = self.last_wire_ms.wrapping_add(lag_ms);
        }
    }

    /// Stores `base + lateness` and returns the max of all 32 slots, the new one included; an
    /// unfilled slot is zero, as in a fresh `CMovement`.
    fn record_lateness(&mut self, lateness_ms: f64) -> f64 {
        self.ring[self.ring_idx] = self.base_ms + lateness_ms;
        self.ring_idx = (self.ring_idx + 1) % LATENESS_WINDOW;
        self.ring.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }

    /// `fire - arrival`, for the trace; negative means the move applied at arrival.
    pub(crate) fn lead_ms(&self, now_ms: f64) -> f64 {
        self.last_fire_ms - now_ms
    }
}
