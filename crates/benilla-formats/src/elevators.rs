//! `TransportAnimation.dbc` and `TransportRotation.dbc`, the keyframe paths and orientation keys
//! of type-11 transports (elevators and trams), and the reference's cycle evaluators over them, in
//! WoW coordinates. The server sends one anchor, the create block's `UPDATE_FLAG_TRANSPORT` clock
//! (time since creation, modulo the period), and the client animates the car: bracket the
//! keyframes around `(anchor + elapsed) % period`, lerp their offsets, turn them by
//! `GAMEOBJECT_PARENTROTATION` and add the spawn position (`0x70daa0`); with two or more
//! orientation keys, slerp them and turn the result by the same parent rotation (`0x70dc10`).
//! `TransportID` and `GameObjectsID` are the `gameobject_template` entry; `SequenceID`, the car's
//! animation per span, is not read.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const TRANSPORT_ANIMATION: &str = "DBFilesClient\\TransportAnimation.dbc";
const TRANSPORT_ROTATION: &str = "DBFilesClient\\TransportRotation.dbc";

/// One `TransportAnimation.dbc` row, a keyframe on a type-11 transport's local path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevatorKeyframe {
    /// Cumulative cycle time in ms; the last frame's is the period.
    pub time_ms: u32,
    /// Offset from the spawn point in WoW axes, before the spawn rotation.
    pub pos: [f32; 3],
}

/// `TransportAnimation.dbc` grouped by `TransportID`, each path's frames sorted by `TimeIndex`.
pub struct ElevatorPaths {
    by_entry: HashMap<u32, Vec<ElevatorKeyframe>>,
}

impl ElevatorPaths {
    /// A template entry's keyframes in time order: at least two, the first at `time_ms == 0`.
    pub fn entry(&self, template_entry: u32) -> Option<&[ElevatorKeyframe]> {
        self.by_entry.get(&template_entry).map(Vec::as_slice)
    }

    /// The number of transport entries with a path.
    pub fn len(&self) -> usize {
        self.by_entry.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_entry.is_empty()
    }
}

pub(crate) fn transport_animation_schema() -> Schema {
    let mut s = Schema::new("TransportAnimation");
    for name in ["ID", "TransportID", "TimeIndex"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in ["PosX", "PosY", "PosZ"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s.add_field(SchemaField::new("SequenceID", FieldType::UInt32));
    s
}

/// Read `TransportAnimation.dbc` off the patch chain, silently dropping, as the reference does, a
/// path that cannot drive a cycle (under 2 frames, a zero period, a first frame past 0); its GO
/// stays at its spawn point.
pub fn load_elevator_paths(chain: &mut Chain) -> Result<ElevatorPaths> {
    let bytes = chain
        .read_file(TRANSPORT_ANIMATION)
        .context("reading TransportAnimation.dbc")?;
    let rs = parse(&bytes, transport_animation_schema(), "TransportAnimation")?;
    let mut by_entry: HashMap<u32, Vec<ElevatorKeyframe>> = HashMap::new();
    for r in rs.records() {
        let Some(entry) = u32_at(r, 1) else { continue };
        let Some(time_ms) = u32_at(r, 2) else {
            continue;
        };
        let (Some(x), Some(y), Some(z)) = (f32_at(r, 3), f32_at(r, 4), f32_at(r, 5)) else {
            continue;
        };
        by_entry.entry(entry).or_default().push(ElevatorKeyframe {
            time_ms,
            pos: [x, y, z],
        });
    }
    for frames in by_entry.values_mut() {
        frames.sort_by_key(|f| f.time_ms);
    }
    by_entry.retain(|_, f| f.len() >= 2 && f[0].time_ms == 0 && f[f.len() - 1].time_ms > 0);
    Ok(ElevatorPaths { by_entry })
}

/// The cycle period in ms, the last keyframe's time.
pub fn elevator_period_ms(frames: &[ElevatorKeyframe]) -> u32 {
    frames.last().map_or(1, |f| f.time_ms).max(1)
}

/// The reference's type-11 evaluator: the car's world position at `cycle_ms`, from the spawn
/// position and its `GAMEOBJECT_PARENTROTATION` quaternion `(x, y, z, w)`, and whether the
/// bracketing span moves. The offset turns by `R(parent)`, the 3×3 the reference builds from the
/// parent rotation (`0x70daa0`), never by the object's own rotation, which only orients the model.
pub fn elevator_sample(
    frames: &[ElevatorKeyframe],
    spawn_pos: [f32; 3],
    parent_rotation: [f32; 4],
    cycle_ms: u32,
) -> ([f32; 3], bool) {
    debug_assert!(frames.len() >= 2 && frames[0].time_ms == 0);
    let period = elevator_period_ms(frames);
    let target = cycle_ms % period;

    // The reference's bracket, found by search rather than its ring cursor.
    let hi = frames.partition_point(|f| f.time_ms <= target);
    let (prev, next) = (frames[hi - 1], frames[hi.min(frames.len() - 1)]);

    let local = if prev.pos == next.pos || next.time_ms == prev.time_ms {
        prev.pos
    } else {
        let frac = (target - prev.time_ms) as f32 / (next.time_ms - prev.time_ms) as f32;
        [
            prev.pos[0] + (next.pos[0] - prev.pos[0]) * frac,
            prev.pos[1] + (next.pos[1] - prev.pos[1]) * frac,
            prev.pos[2] + (next.pos[2] - prev.pos[2]) * frac,
        ]
    };

    let rotated = rotate_by_quat(local, parent_rotation);
    (
        [
            spawn_pos[0] + rotated[0],
            spawn_pos[1] + rotated[1],
            spawn_pos[2] + rotated[2],
        ],
        prev.pos != next.pos,
    )
}

/// One `TransportRotation.dbc` row, an orientation key on a type-11 transport's cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotationKey {
    /// Cycle time in ms the key holds from.
    pub time_ms: u32,
    /// Orientation `(x, y, z, w)` before the parent rotation.
    pub rotation: [f32; 4],
}

/// `TransportRotation.dbc` grouped by `GameObjectsID`, each entry's keys in file order.
pub struct RotationPaths {
    by_entry: HashMap<u32, Vec<RotationKey>>,
}

impl RotationPaths {
    /// A template entry's orientation keys, in file order.
    pub fn entry(&self, template_entry: u32) -> Option<&[RotationKey]> {
        self.by_entry.get(&template_entry).map(Vec::as_slice)
    }

    /// The number of transport entries with orientation keys.
    pub fn len(&self) -> usize {
        self.by_entry.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_entry.is_empty()
    }
}

pub(crate) fn transport_rotation_schema() -> Schema {
    let mut s = Schema::new("TransportRotation");
    for name in ["ID", "GameObjectsID", "TimeIndex"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in ["RotX", "RotY", "RotZ", "RotW"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s
}

/// Read `TransportRotation.dbc` off the patch chain. The reference binary-searches the
/// `GameObjectsID`-sorted table for an entry's first row and takes the rows after it while the id
/// holds (`0x70c930`, `0x713820`), so an entry's keys are its first contiguous run, in file order.
pub fn load_rotation_paths(chain: &mut Chain) -> Result<RotationPaths> {
    let bytes = chain
        .read_file(TRANSPORT_ROTATION)
        .context("reading TransportRotation.dbc")?;
    let rs = parse(&bytes, transport_rotation_schema(), "TransportRotation")?;
    let mut by_entry: HashMap<u32, Vec<RotationKey>> = HashMap::new();
    let mut run: Option<u32> = None;
    for r in rs.records() {
        let (Some(entry), Some(time_ms)) = (u32_at(r, 1), u32_at(r, 2)) else {
            continue;
        };
        let (Some(x), Some(y), Some(z), Some(w)) =
            (f32_at(r, 3), f32_at(r, 4), f32_at(r, 5), f32_at(r, 6))
        else {
            continue;
        };
        // A second run of an entry already read is past the reference's reach.
        if run != Some(entry) && by_entry.contains_key(&entry) {
            continue;
        }
        run = Some(entry);
        by_entry.entry(entry).or_default().push(RotationKey {
            time_ms,
            rotation: [x, y, z, w],
        });
    }
    Ok(RotationPaths { by_entry })
}

/// The reference's type-11 orientation at `cycle_ms` (`0x70dc10`): the key whose span holds the
/// time, slerped toward the next (ring order) and turned by `parent_rotation`, as `slerp · parent`.
/// A span ends at the next key's time, or at `cycle_end_ms` (the last keyframe's time) when that
/// key's time is 0; its fraction is taken against the next key's time even then, as an unsigned
/// difference, so the wrapping span divides by nearly 2^32 and holds the last key. `None` under
/// two keys, where the
/// car keeps its own rotation; also `None` when no span holds the time, where the reference's
/// walk never ends.
pub fn elevator_rotation(
    keys: &[RotationKey],
    cycle_end_ms: u32,
    parent_rotation: [f32; 4],
    cycle_ms: u32,
) -> Option<[f32; 4]> {
    let n = keys.len();
    if n <= 1 {
        return None;
    }
    // The reference walks a ring cursor; the spans are disjoint, so a search finds the same one.
    let (current, following) = (0..n)
        .map(|i| (keys[i], keys[(i + 1) % n]))
        .find(|(c, f)| {
            let end = if f.time_ms == 0 {
                cycle_end_ms
            } else {
                f.time_ms
            };
            cycle_ms >= c.time_ms && cycle_ms < end
        })?;
    // An unsigned difference, as the reference converts it: the wrapping span's is near 2^32.
    let span = following.time_ms.wrapping_sub(current.time_ms) as f32;
    // Two keys at one time divide by zero in the reference; hold the first.
    let t = if span == 0.0 {
        0.0
    } else {
        (cycle_ms - current.time_ms) as f32 / span
    };
    let turned = slerp(t, current.rotation, following.rotation);
    Some(quat_mul(turned, parent_rotation))
}

/// The reference's slerp (`0x982460`): the shorter arc, `a` itself when the two are within
/// `sin < 2^-21` of each other, and no clamp on `t`.
fn slerp(t: f32, a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let mut cosine = a[2] * b[2] + a[1] * b[1] + a[0] * b[0] + b[3] * a[3];
    let mut sign = 1.0;
    if cosine < 0.0 {
        sign = -1.0;
        cosine = -cosine;
    }
    let sine = (1.0 - cosine * cosine).abs().sqrt();
    if sine.abs() < 0.000_000_476_837_16 {
        return a;
    }
    let angle = sine.atan2(cosine);
    let scale = 1.0 / sine;
    let wa = ((1.0 - t) * angle).sin() * scale;
    let wb = scale * (t * angle).sin() * sign;
    [
        b[0] * wb + a[0] * wa,
        wb * b[1] + a[1] * wa,
        a[2] * wa + wb * b[2],
        wb * b[3] + wa * a[3],
    ]
}

/// The Hamilton product `l · r` of two `(x, y, z, w)` quaternions (`0x4f4320`).
fn quat_mul(l: [f32; 4], r: [f32; 4]) -> [f32; 4] {
    [
        l[3] * r[0] + r[2] * l[1] + r[3] * l[0] - l[2] * r[1],
        r[3] * l[1] + l[3] * r[1] + l[2] * r[0] - r[2] * l[0],
        r[2] * l[3] + r[3] * l[2] + l[0] * r[1] - r[0] * l[1],
        r[3] * l[3] - l[0] * r[0] - l[1] * r[1] - r[2] * l[2],
    ]
}

/// `R(q)·v` for a unit quaternion `(x, y, z, w)`.
fn rotate_by_quat(v: [f32; 3], q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    // t = 2·(q.xyz × v); v' = v + w·t + q.xyz × t
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames() -> Vec<ElevatorKeyframe> {
        vec![
            ElevatorKeyframe {
                time_ms: 0,
                pos: [0.0, 0.0, 0.0],
            },
            ElevatorKeyframe {
                time_ms: 5000,
                pos: [0.0, 0.0, 0.0],
            },
            ElevatorKeyframe {
                time_ms: 10000,
                pos: [0.0, 0.0, -10.0],
            },
            ElevatorKeyframe {
                time_ms: 30000,
                pos: [0.0, 0.0, 0.0],
            },
        ]
    }

    const IDENT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    #[test]
    fn dwell_then_lerp_then_wrap() {
        let f = frames();
        let base = [100.0, 200.0, 50.0];
        let (p, moving) = elevator_sample(&f, base, IDENT, 2500);
        assert_eq!(p, base);
        assert!(!moving);
        // 7500 is halfway through the 5000 to 10000 span: z offset -5.
        let (p, moving) = elevator_sample(&f, base, IDENT, 7500);
        assert_eq!(p, [100.0, 200.0, 45.0]);
        assert!(moving);
        let (p0, _) = elevator_sample(&f, base, IDENT, 0);
        let (pw, _) = elevator_sample(&f, base, IDENT, 30000);
        assert_eq!(p0, pw);
        let (p, moving) = elevator_sample(&f, base, IDENT, 29999);
        assert!(moving);
        assert!((p[2] - 50.0).abs() < 0.01, "z = {}", p[2]);
    }

    /// A 90° parent yaw turns `(x, y)` into `(-y, x)`.
    #[test]
    fn the_parent_rotation_turns_the_local_offset() {
        let half = std::f32::consts::FRAC_PI_4; // 90°/2
        let q = [0.0, 0.0, half.sin(), half.cos()];
        let f = vec![
            ElevatorKeyframe {
                time_ms: 0,
                pos: [10.0, 0.0, 0.0],
            },
            ElevatorKeyframe {
                time_ms: 1000,
                pos: [10.0, 0.0, 0.0],
            },
        ];
        let (p, _) = elevator_sample(&f, [0.0; 3], q, 500);
        assert!(
            (p[0] - 0.0).abs() < 1e-4 && (p[1] - 10.0).abs() < 1e-4,
            "{p:?}"
        );
    }

    fn yaw(rad: f32) -> [f32; 4] {
        let (s, c) = (rad * 0.5).sin_cos();
        [0.0, 0.0, s, c]
    }

    fn key(time_ms: u32, rotation: [f32; 4]) -> RotationKey {
        RotationKey { time_ms, rotation }
    }

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// One key or none keeps the car's own rotation (`0x70dc10`'s early return).
    #[test]
    fn under_two_keys_the_car_keeps_its_own_rotation() {
        assert_eq!(elevator_rotation(&[], 1000, IDENT, 10), None);
        assert_eq!(
            elevator_rotation(&[key(0, yaw(1.0))], 1000, IDENT, 10),
            None
        );
    }

    /// Halfway through a span is halfway around the arc; the parent turns the result.
    #[test]
    fn a_span_slerps_its_two_keys_then_turns_by_the_parent() {
        let keys = [key(0, IDENT), key(1000, yaw(1.0)), key(2000, IDENT)];
        let q = elevator_rotation(&keys, 2000, IDENT, 500).unwrap();
        assert!(close(q, yaw(0.5)), "{q:?}");
        let q = elevator_rotation(&keys, 2000, yaw(0.25), 500).unwrap();
        assert!(close(q, yaw(0.75)), "{q:?}");
        let q = elevator_rotation(&keys, 2000, IDENT, 1500).unwrap();
        assert!(close(q, yaw(0.5)), "{q:?}");
    }

    /// `slerp · parent`, not `parent · slerp`: a roll key under a yaw parent.
    #[test]
    fn the_parent_is_the_right_hand_factor() {
        let half = 0.5f32.sin_cos();
        let roll = [half.0, 0.0, 0.0, half.1];
        let keys = [key(0, roll), key(1000, roll)];
        let parent = yaw(1.0);
        let q = elevator_rotation(&keys, 2000, parent, 10).unwrap();
        assert!(close(q, quat_mul(roll, parent)), "{q:?}");
        assert!(!close(q, quat_mul(parent, roll)));
    }

    /// The wrapping span ends at the cycle's end but divides by the unsigned `0 - 1000`, near
    /// 2^32: the fraction is all but 0 and the car holds the last key (`0x70dc10`).
    #[test]
    fn the_wrapping_span_holds_the_last_key() {
        let keys = [key(0, IDENT), key(1000, yaw(1.0))];
        let q = elevator_rotation(&keys, 2000, IDENT, 1500).unwrap();
        assert!(close(q, yaw(1.0)), "{q:?}");
    }

    /// No span holds a time before the first key when it is past 0; the reference never returns.
    #[test]
    fn a_time_no_span_holds_keeps_the_own_rotation() {
        let keys = [key(1600, IDENT), key(1633, IDENT), key(1667, IDENT)];
        assert_eq!(elevator_rotation(&keys, 1667, IDENT, 100), None);
    }

    /// The reference's slerp: the shorter arc, and `a` for two keys within `2^-21`.
    #[test]
    fn slerp_takes_the_shorter_arc() {
        let a = yaw(0.2);
        let b = yaw(-0.2).map(|c| -c);
        assert!(close(slerp(0.5, a, b), IDENT));
        assert_eq!(slerp(0.7, a, a), a);
    }

    /// The 12340 table: the Ulduar tram turns on its own keys over its keyframe cycle.
    #[test]
    fn real_transport_rotation_keys_the_ulduar_tram() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let rotations = load_rotation_paths(&mut chain).expect("load TransportRotation.dbc");
        let paths = load_elevator_paths(&mut chain).expect("load TransportAnimation.dbc");
        // Template 194675, the Ulduar tram: 14 keys, the first at 0 and the last at its
        // keyframe period, turning a half circle and back.
        let tram = rotations.entry(194675).expect("Ulduar tram keys");
        assert_eq!(tram.len(), 14);
        assert_eq!(tram[0].time_ms, 0);
        let end = elevator_period_ms(paths.entry(194675).expect("Ulduar tram path"));
        assert_eq!(tram[tram.len() - 1].time_ms, end);
        let far = elevator_rotation(tram, end, IDENT, 80_000).unwrap();
        assert!(far[2].abs() > 0.99, "turned around at the far end: {far:?}");
        assert!(rotations.entry(999_999).is_none());
    }

    /// The 5875 table: layout and load guarantees on three real paths.
    #[test]
    fn real_transport_animation_layout_sanity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let paths = load_elevator_paths(&mut chain).expect("load TransportAnimation.dbc");
        // Thunder Bluff's Mesa Elevator cars are templates 4170 and 4171, Undercity's 152614.
        let top = paths.entry(4170).expect("Mesa Elevator 4170");
        assert_eq!(elevator_period_ms(top), 30033);
        assert_eq!(top[0].time_ms, 0);
        assert_eq!(top[0].pos, [0.0, 0.0, 0.0]);
        assert!(top.windows(2).all(|w| w[0].time_ms <= w[1].time_ms));
        // The car moves on z only (x and y are ~2e-6 yd of noise), 61.24 yd down.
        assert!(top
            .iter()
            .all(|f| f.pos[0].abs() < 1e-3 && f.pos[1].abs() < 1e-3));
        assert!((top.iter().map(|f| f.pos[2]).fold(0.0f32, f32::min) + 61.244).abs() < 1e-3);
        let bottom = paths.entry(4171).expect("Mesa Elevator 4171");
        assert_eq!(elevator_period_ms(bottom), 30000);
        assert!(paths.entry(152614).is_some(), "Undercity elevator");
        assert!(paths.entry(999_999).is_none());
    }
}
