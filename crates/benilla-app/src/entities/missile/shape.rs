//! A missile's flight shape off its `SpellVisual` rows: where it leaves from and where it lands
//! (columns 16 and 10 with the cast and impact offsets), how it hugs the ground (columns 17-20),
//! the motion script and targeting kit it carries (columns 21-22), and how its model turns with
//! its own motion.

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::VisualStages;

/// The attachment a cast offset with no column 16 leaves from, SpellHandDirected (`0x720c86`).
pub(super) const SPELL_HAND_DIRECTED: u16 = 38;

/// The lowest hover height and drop rate a flight takes (`0x7332f5`).
const MIN_HEIGHT: f32 = 0.25;
const MIN_RATE: f32 = 0.01;

/// The longest step one follow takes, and how far below the hover height its down ray reaches
/// (`0x6fffc0`).
const MAX_STEP: f32 = 50.0;
const SCAN_RANGE: f32 = 20.0;

/// A step shorter than this does not turn the model (`0x701230`).
const STILL: f32 = 2.384_185_8e-7;

/// The launch override (`0x720bf0`): column 16's attachment plus the cast offset, or with no
/// column 16 the cast offset alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MissileSource {
    /// Column 16's attachment; `None` leaves from SpellHandDirected, else the model's origin.
    pub(crate) tag: Option<u16>,
    /// Columns 26-28 in Bevy model space.
    pub(crate) offset: Vec3,
}

/// Columns 17-20 as the flight reads them (`0x7332f5`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FollowGround {
    /// The hover height over the ground, yards.
    pub(crate) height: f32,
    /// The share of the gap to the hover height closed per yard flown.
    pub(crate) rate: f32,
    /// The horizontal progress past which the flight blends into its aim.
    pub(crate) start: f32,
    pub(crate) flags: u32,
}

impl FollowGround {
    /// Flag 4: the approach blends toward the straight-ahead point, not the aim (`0x70026a`).
    fn ahead(&self) -> bool {
        self.flags & 4 != 0
    }

    /// Flag 8: the aim and the hit kit sit at the hover height over the target's feet (`0x6ff7e8`).
    pub(crate) fn hugs_target(&self) -> bool {
        self.flags & 8 != 0
    }
}

/// A spell's flight columns beyond the model, as a new missile takes them (`0x732ff0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MissileShape {
    /// The launch override, from the spell's own row (`0x720c1d`).
    pub(crate) source: Option<MissileSource>,
    /// Columns 29-31 in Bevy model space, from the row the cast plays (`0x73339c`).
    pub(crate) impact_offset: Vec3,
    /// Columns 17-20 when the own row's follow flag 1 is set (`0x733373`).
    pub(crate) follow: Option<FollowGround>,
    /// Column 21 of the own row, 0 for none (`0x733674`).
    pub(crate) motion: u32,
    /// Column 22 of `Spell.dbc`'s first visual, read past the violence pick (`0x702570`).
    pub(crate) targeting_kit: u32,
}

impl MissileShape {
    /// `own` is the spell's own row (`0x7fa290`), `played` the row the cast plays after the ranged
    /// merge (`0x720f80`), `first` the spell's first visual whatever the violence level.
    pub(crate) fn of(
        own: Option<&VisualStages>,
        played: Option<&VisualStages>,
        first: Option<&VisualStages>,
    ) -> Self {
        Self {
            source: own.and_then(source_of),
            impact_offset: played.map_or(Vec3::ZERO, |s| wow_to_bevy(s.impact_offset())),
            follow: own.and_then(follow_of),
            motion: own.map_or(0, |s| s.missile_motion),
            targeting_kit: first
                .map(|s| s.missile_targeting_kit)
                .filter(|&k| k as i32 > 0)
                .unwrap_or(0),
        }
    }
}

/// The launch override of a row (`0x720bf0`): none for column 16's -1 with no cast offset.
fn source_of(s: &VisualStages) -> Option<MissileSource> {
    let offset = wow_to_bevy(s.cast_offset());
    if s.missile_source_attach == u32::MAX {
        return (offset != Vec3::ZERO).then_some(MissileSource { tag: None, offset });
    }
    // An index past the table reads adjacent data in the reference; here, no override.
    Some(MissileSource {
        tag: Some(s.missile_source_attachment()?),
        offset,
    })
}

/// A row's ground following, set by its follow flag 1 (`0x733373`), floors applied (`0x7332f5`).
fn follow_of(s: &VisualStages) -> Option<FollowGround> {
    (s.follow_ground_flags & 1 != 0).then(|| FollowGround {
        height: (s.follow_ground_height as f32 * 0.001).max(MIN_HEIGHT),
        rate: (s.follow_ground_drop_speed as f32 * 0.001).max(MIN_RATE),
        start: (s.follow_ground_approach as f32 * 0.001).clamp(0.0, 1.0),
        flags: s.follow_ground_flags,
    })
}

/// One step of a ground-following flight (`0x6fffc0`): `distance` along `dir` from `pos`, over the
/// ground height `ground` reads, then the approach blend past [`FollowGround::start`].
///
/// Deviation: the reference rays the step against the world (up to four forward rays, then one
/// down); this reads the terrain heightfield once at the step's end, so the flight climbs and
/// drops with the terrain but passes over buildings and doodads, a cost a per-frame ray would add.
pub(crate) fn follow_ground(
    pos: Vec3,
    dir: Vec3,
    distance: f32,
    follow: &FollowGround,
    h_progress: f32,
    impact: Vec3,
    ground: impl Fn(Vec3) -> Option<f32>,
) -> Vec3 {
    let distance = distance.clamp(0.0, MAX_STEP);
    let ahead = pos + dir * distance * 2.0;
    // The flat heading; straight up or down flies the reference's +X, Bevy's −Z.
    let flat = if dir.y.abs() <= 0.999 {
        Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z)
    } else {
        Vec3::NEG_Z
    };
    let mut out = pos + flat * distance;
    match ground(out) {
        // The ground rises above the flight: climb onto it at the hover height.
        Some(g) if g > pos.y => out.y = g + follow.height,
        // Else drop toward the hover height over ground within reach below, never under it.
        g => {
            if let Some(g) = g.filter(|&g| g >= pos.y - (follow.height + SCAN_RANGE)) {
                let floor = g + follow.height;
                out.y = (out.y - (out.y - floor) * follow.rate * distance).max(floor);
            }
        }
    }
    if follow.start < h_progress {
        let t = (h_progress - follow.start) / (1.0 - follow.start);
        let toward = if follow.ahead() { ahead } else { impact };
        out = out.lerp(toward, t * t);
    }
    out
}

/// The flight's progress after a step (`0x6ffed0`): `1 − remaining / launch-to-impact` in 3D and
/// `1 − (pos to impact) / (launch to impact)` over the ground, neither falling below `prev`.
pub(crate) fn progress(
    launch: Vec3,
    impact: Vec3,
    pos: Vec3,
    remaining: f32,
    prev: (f32, f32),
) -> (f32, f32) {
    let total = launch.distance(impact);
    let p = if total <= 1e-6 {
        1.0
    } else {
        1.0 - remaining / total
    };
    let flat = |v: Vec3| Vec2::new(v.x, v.z).length();
    let total_h = flat(impact - launch);
    let h = if total_h <= 1e-6 {
        1.0
    } else {
        1.0 - flat(impact - pos) / total_h
    };
    (p.max(prev.0), h.max(prev.1))
}

/// A model that turns with its own motion (`0x701230`, missile flag `0x800`): it faces its last
/// step, a step under 0.01 eased into the previous heading, else the aim.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FaceMotion {
    last: Vec3,
    smooth: Option<Vec3>,
}

impl FaceMotion {
    /// The first step's heading is the aim's: the last point starts behind the launch (`0x70250b`).
    pub(crate) fn new(launch: Vec3, impact: Vec3) -> Self {
        Self {
            last: launch - (impact - launch),
            smooth: None,
        }
    }

    /// The heading for the model drawn at `model`; `aim` when it did not move.
    pub(crate) fn face(&mut self, model: Vec3, aim: Vec3) -> Vec3 {
        let mut delta = model - self.last;
        self.last = model;
        let len = delta.length();
        if len <= STILL {
            return aim;
        }
        if len < 0.01 {
            if let Some(s) = self.smooth {
                let w = (len * 100.0).max(0.05);
                delta = delta * w + s * (1.0 - w);
            }
        }
        self.smooth = Some(delta);
        delta.normalize_or(aim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn follow(flags: u32) -> FollowGround {
        FollowGround {
            height: 1.0,
            rate: 0.3,
            start: 0.75,
            flags,
        }
    }

    /// The `0x7332f5` floors and the `0x720bf0` source rules.
    #[test]
    fn the_shape_reads_the_reference_floors_and_source_rules() {
        let row = VisualStages {
            missile_source_attach: u32::MAX,
            follow_ground_height: 100,
            follow_ground_drop_speed: 5,
            follow_ground_approach: 1500,
            follow_ground_flags: 7,
            missile_cast_offset: [0.0, 2.0, 1.0],
            missile_impact_offset: [3.0, 0.0, 0.0],
            missile_motion: 13,
            missile_targeting_kit: 8729,
            ..Default::default()
        };
        let shape = MissileShape::of(Some(&row), Some(&row), Some(&row));
        let f = shape.follow.expect("flag 1 follows");
        assert_eq!((f.height, f.rate, f.start), (0.25, 0.01, 1.0));
        assert_eq!(
            shape.source,
            Some(MissileSource {
                tag: None,
                // Model +Z up is Bevy +Y; the column's y is negated, then model +Y is Bevy −X.
                offset: Vec3::new(2.0, 1.0, 0.0),
            })
        );
        assert_eq!(
            shape.impact_offset,
            Vec3::new(0.0, 0.0, -3.0),
            "model +X is Bevy −Z"
        );
        assert_eq!((shape.motion, shape.targeting_kit), (13, 8729));

        let quiet = VisualStages {
            missile_source_attach: u32::MAX,
            follow_ground_flags: 6,
            ..Default::default()
        };
        let shape = MissileShape::of(Some(&quiet), None, None);
        assert_eq!(
            shape.source, None,
            "-1 with no offset keeps the event's point"
        );
        assert_eq!(shape.follow, None, "no flag 1, no following");
        let hand = VisualStages {
            missile_source_attach: 4,
            ..Default::default()
        };
        assert_eq!(
            MissileShape::of(Some(&hand), None, None).source,
            Some(MissileSource {
                tag: Some(0x16),
                offset: Vec3::ZERO
            })
        );
    }

    /// Over flat ground the flight sinks toward the hover height at the drop rate per yard, and
    /// a rise lifts it onto the slope.
    #[test]
    fn the_flight_sinks_to_its_hover_height_and_climbs_a_rise() {
        let flat = |_: Vec3| Some(0.0);
        let at = follow_ground(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::NEG_Z,
            2.0,
            &follow(1),
            0.0,
            Vec3::new(0.0, 1.0, -40.0),
            flat,
        );
        // 5 − (5 − 1)·0.3·2 = 2.6.
        assert!((at.y - 2.6).abs() < 1e-5, "{at}");
        assert!((at.z + 2.0).abs() < 1e-5, "it steps along its heading");
        let low = follow_ground(
            Vec3::new(0.0, 1.2, 0.0),
            Vec3::NEG_Z,
            10.0,
            &follow(1),
            0.0,
            Vec3::ZERO,
            flat,
        );
        assert_eq!(low.y, 1.0, "never under the hover height");
        let rise = follow_ground(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::NEG_Z,
            2.0,
            &follow(1),
            0.0,
            Vec3::ZERO,
            |_| Some(3.0),
        );
        assert_eq!(rise.y, 4.0, "onto the rise at the hover height");
        let void = follow_ground(
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::NEG_Z,
            2.0,
            &follow(1),
            0.0,
            Vec3::ZERO,
            flat,
        );
        assert_eq!(
            void.y, 50.0,
            "ground out of the ray's reach holds the height"
        );
    }

    /// Past the approach start the flight blends into the aim, or flag 4 the point ahead.
    #[test]
    fn the_approach_blends_into_the_aim_past_its_start() {
        let none = |_: Vec3| None;
        let impact = Vec3::new(0.0, 0.0, -10.0);
        let pos = Vec3::new(4.0, 0.0, 0.0);
        let at = follow_ground(pos, Vec3::NEG_Z, 1.0, &follow(1), 1.0, impact, none);
        assert_eq!(at, impact, "full progress lands on the aim");
        let ahead = follow_ground(pos, Vec3::NEG_Z, 1.0, &follow(5), 1.0, impact, none);
        assert_eq!(
            ahead,
            pos + Vec3::NEG_Z * 2.0,
            "flag 4 blends to twice the step ahead"
        );
        let before = follow_ground(pos, Vec3::NEG_Z, 1.0, &follow(1), 0.5, impact, none);
        assert_eq!(before, pos + Vec3::NEG_Z, "no blend before the start");
    }

    #[test]
    fn progress_never_falls() {
        let launch = Vec3::ZERO;
        let impact = Vec3::new(0.0, 0.0, -10.0);
        let (p, h) = progress(launch, impact, Vec3::new(0.0, 0.0, -4.0), 6.0, (0.0, 0.0));
        assert!((p - 0.4).abs() < 1e-6 && (h - 0.4).abs() < 1e-6);
        let back = progress(launch, impact, Vec3::new(0.0, 0.0, -1.0), 9.0, (p, h));
        assert_eq!(back, (p, h));
    }

    #[test]
    fn the_model_faces_its_own_step_and_eases_a_tiny_one() {
        let mut face = FaceMotion::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0));
        let aim = Vec3::NEG_Z;
        assert_eq!(
            face.face(Vec3::ZERO, aim),
            aim,
            "the first step heads at the aim"
        );
        assert_eq!(face.face(Vec3::new(1.0, 0.0, 0.0), aim), Vec3::X);
        let eased = face.face(Vec3::new(1.0, 0.001, 0.0), aim);
        assert!(
            eased.x > 0.99,
            "a tiny step keeps most of the old heading: {eased}"
        );
        assert_eq!(face.face(Vec3::new(1.0, 0.001, 0.0), aim), aim, "still");
    }
}
