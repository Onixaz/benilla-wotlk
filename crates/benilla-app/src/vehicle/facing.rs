//! A vehicle's legal facing: a record with flag `0x200000` turns only within its limits either side
//! of the facing it was created with (`0x6eaa50`). Every facing write on the mover clamps into it:
//! a key turn runs to the limit and stops there (`0x6f0f70`, `0x6ef3d0`), and a mouse-look sync
//! holds the camera's yaw at the limit with the body (`0x6023d0`, through `0x71c1e0`).

use std::f32::consts::TAU;

use benilla_formats::VehicleRec;

/// The record's facing-limit flag.
const FACING_LIMITED: u32 = 0x20_0000;

/// The record flags that fix a vehicle's rendered facing (`0x73dab0`): `0x200` or `0x1000`.
const FIXED_RENDER_FACING: u32 = 0x200 | 0x1000;

/// Whether `rec`'s model turns with its facing at once (`0x73dab0`), with no stationary chase and no
/// twist of its key bones. Both Isle of Conquest siege engines set `0x1000`.
pub(crate) fn fixed_render_facing(rec: &VehicleRec) -> bool {
    rec.flags & FIXED_RENDER_FACING != 0
}

/// `0x6eaa50`: the facings `rec` allows about `raw_facing` (the vehicle's own, set when it became
/// a vehicle), right limit below and left above, or `None` for an unlimited record.
pub(crate) fn legal_range(rec: &VehicleRec, raw_facing: f32) -> Option<(f32, f32)> {
    (rec.flags & FACING_LIMITED != 0).then_some((
        raw_facing - rec.facing_limit_right,
        raw_facing + rec.facing_limit_left,
    ))
}

/// `0x6e9290`: `facing` moved by whole turns into `[min, max]`, or, when no turn lands it inside,
/// to whichever side's nearest copy is closer to its limit.
pub(crate) fn wrap_to_range(mut facing: f32, min: f32, max: f32) -> f32 {
    while min > facing {
        facing += TAU;
    }
    if max > facing {
        return facing;
    }
    let mut wrapped = facing;
    let mut previous;
    loop {
        previous = wrapped;
        wrapped = previous - TAU;
        if wrapped <= max {
            break;
        }
    }
    if wrapped <= min && previous - max < min - wrapped {
        previous
    } else {
        wrapped
    }
}

/// `0x71c1e0`: the limit `facing` crosses, or `None` while it lies within the range.
pub(crate) fn clamp(facing: f32, (min, max): (f32, f32)) -> Option<f32> {
    let wrapped = wrap_to_range(facing, min, max);
    if wrapped < min {
        Some(min)
    } else if max < wrapped {
        Some(max)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(flags: u32, right: f32, left: f32) -> VehicleRec {
        VehicleRec {
            flags,
            facing_limit_right: right,
            facing_limit_left: left,
            ..VehicleRec::default()
        }
    }

    #[test]
    fn only_a_limited_record_has_a_range() {
        assert_eq!(legal_range(&rec(0, 0.5, 0.5), 1.0), None);
        assert_eq!(
            legal_range(&rec(FACING_LIMITED, 0.25, 0.5), 1.0),
            Some((0.75, 1.5))
        );
    }

    #[test]
    fn a_facing_inside_stays_and_one_past_a_limit_stops_on_it() {
        let range = (0.75, 1.5);
        assert_eq!(clamp(1.0, range), None);
        assert_eq!(clamp(1.0 + TAU, range), None, "a whole turn away is inside");
        assert_eq!(clamp(1.6, range), Some(1.5));
        assert_eq!(clamp(0.7, range), Some(0.75));
    }

    #[test]
    fn a_facing_behind_goes_to_the_nearer_limit() {
        // The range spans 0.75..1.5; its far side is centred on 1.125 + π.
        let range = (0.75, 1.5);
        assert_eq!(clamp(1.125 + std::f32::consts::PI - 0.1, range), Some(1.5));
        assert_eq!(clamp(1.125 + std::f32::consts::PI + 0.1, range), Some(0.75));
    }

    #[test]
    fn a_record_fixes_its_render_facing_under_either_bit() {
        let rec = |flags| VehicleRec {
            flags,
            ..Default::default()
        };
        assert!(!fixed_render_facing(&rec(0x477)));
        assert!(fixed_render_facing(&rec(0x200)));
        assert!(
            fixed_render_facing(&rec(0x5e8d_d477)),
            "the Catapult's record (438)"
        );
        assert!(
            fixed_render_facing(&rec(0x5f0d_9677)),
            "the Demolisher's record (509)"
        );
    }
}
