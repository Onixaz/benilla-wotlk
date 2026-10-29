use crate::messages::MovementInfo;
use crate::wire::Vector3d;

pub(super) const MOVEMENT_FLAG_FORWARD: u32 = 0x1;

/// Ms since start for the `MovementInfo` time, as the 1.12 client's `GetTickCount()`, never 0:
/// vmangos pauses extrapolation on 0 and otherwise uses only deltas, so a full `u32` wrap is safe.
pub(super) fn client_uptime_ms() -> u32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    (START.get_or_init(Instant::now).elapsed().as_millis() as u32).max(1)
}

/// Build a `MovementInfo` stamped with [`client_uptime_ms`].
pub(super) fn movement_info(pos: [f32; 3], orientation: f32, flags: u32) -> MovementInfo {
    MovementInfo {
        flags,
        timestamp: client_uptime_ms(),
        position: Vector3d {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        },
        orientation,
        // Written only under ON_TRANSPORT, which benilla does not send.
        transport: None,
        // Written only under SWIMMING; `send_movement` supplies the live pitch then.
        pitch: 0.0,
        fall_time: 0,
        jump: None,
        flags2: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Never 0, and a stamp taken right after is not smaller; it wraps at `u32::MAX` ms.
    #[test]
    fn client_uptime_ms_is_nonzero_and_back_to_back_calls_do_not_decrease() {
        let a = client_uptime_ms();
        let b = client_uptime_ms();
        assert!(a >= 1, "stamp is non-zero: {a}");
        assert!(b >= a, "the next stamp is not smaller: {a} -> {b}");
    }

    #[test]
    fn movement_info_is_stamped_nonzero() {
        let info = movement_info([1.0, 2.0, 3.0], 0.5, MOVEMENT_FLAG_FORWARD);
        assert_ne!(
            info.timestamp, 0,
            "a movement packet must carry a non-zero time"
        );
    }
}
