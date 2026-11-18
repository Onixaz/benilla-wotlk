//! A sky model's bones, evaluated on the CPU on the sky's own clock.
//!
//! 12340 draws every sky as an ordinary animated M2 in a scene of its own (`0x7f30c0`), so the
//! lane samples the bones on that scene's clock. The bone law is the model animator's
//! (`0x82f0f0`): a bone with flags `& 0x280` composes `T(pivot + t) · R · S · T(−pivot)`
//! onto its parent, the rotation lerped and normalised between keys (`0x82ad50`); any other
//! bone takes its parent's.

use benilla_m2::{M2Model, M2Track};

use super::key_anim::{bake_track, KeyAnim, SeqSlot};

/// Bone flags `0x200 | 0x80`: the bone composes a local matrix (`0x82f0f0`).
const BONE_ANIMATED: u32 = 0x280;

/// A bone's affine model-space matrix: the linear part (rows) and the translation, WoW space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneMatrix {
    pub linear: [[f32; 3]; 3],
    pub translation: [f32; 3],
}

impl BoneMatrix {
    pub const IDENTITY: Self = Self {
        linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        translation: [0.0; 3],
    };

    /// `T(pivot + t) · R(q) · S(s) · T(−pivot)`, `q` normalised.
    fn bone_local(pivot: [f32; 3], t: [f32; 3], q: [f32; 4], s: [f32; 3]) -> Self {
        let r = quat_rows(q);
        let mut linear = [[0.0; 3]; 3];
        for (i, row) in linear.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = r[i][j] * s[j];
            }
        }
        let lp = mul3(&linear, pivot);
        Self {
            linear,
            translation: [
                pivot[0] + t[0] - lp[0],
                pivot[1] + t[1] - lp[1],
                pivot[2] + t[2] - lp[2],
            ],
        }
    }

    /// `self · rhs`: `rhs` first, then `self`.
    pub fn then_parent(&self, rhs: &Self) -> Self {
        let mut linear = [[0.0; 3]; 3];
        for (i, row) in linear.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.linear[i][k] * rhs.linear[k][j]).sum();
            }
        }
        let t = mul3(&self.linear, rhs.translation);
        Self {
            linear,
            translation: [
                t[0] + self.translation[0],
                t[1] + self.translation[1],
                t[2] + self.translation[2],
            ],
        }
    }

    /// The matrix applied to a point.
    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let v = mul3(&self.linear, p);
        [
            v[0] + self.translation[0],
            v[1] + self.translation[1],
            v[2] + self.translation[2],
        ]
    }
}

fn mul3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// The rotation matrix of `q = [x, y, z, w]`, normalised first; a degenerate one is the identity.
fn quat_rows(q: [f32; 4]) -> [[f32; 3]; 3] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n <= 1e-6 {
        return BoneMatrix::IDENTITY.linear;
    }
    let (x, y, z, w) = (q[0] / n, q[1] / n, q[2] / n, q[3] / n);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// One bone's baked tracks on the armed sequence's band (or their global sequences).
#[derive(Clone, Debug, PartialEq)]
struct SkyBone {
    parent: Option<usize>,
    pivot: [f32; 3],
    /// `flags & 0x280`: compose a local matrix at all.
    animated: bool,
    translation: Option<KeyAnim<[f32; 3]>>,
    rotation: Option<KeyAnim<[f32; 4]>>,
    scale: Option<KeyAnim<[f32; 3]>>,
}

/// A sky model's skeleton on its armed sequence: the loader's arm, animation id 0, which for every
/// shipped sky is file slot 0 and its only sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyRig {
    /// The armed sequence's length in seconds (`0x8266b0`); 0 for a model with no sequence.
    pub length: f32,
    /// Whether the armed sequence loops (sequence flags bit 0 clear); a one-shot holds its tail.
    pub looping: bool,
    /// The armed sequence's file slot, which the batches' per-sequence material loops index.
    pub slot: Option<usize>,
    bones: Vec<SkyBone>,
}

impl SkyRig {
    /// Bake `model`'s bones on the sequence the loader arms: animation id 0 through the lookup,
    /// else file slot 0.
    pub fn new(model: &M2Model) -> Self {
        let slot_index =
            benilla_m2::find_sequence_slot(&model.animation_lookup, &model.sequences, 0)
                .or((!model.sequences.is_empty()).then_some(0));
        let slot = slot_index.and_then(|index| {
            let s = model.sequences.get(index)?;
            Some(SeqSlot {
                index,
                band: (s.start, s.end),
                looping: s.flags & 1 == 0,
            })
        });
        let gseqs = &model.global_sequences;
        // Keep every keyed track, a constant one too: a held rotation still turns the bone, and an
        // identity hold costs nothing.
        fn bake<T: Copy, V: super::key_anim::Lerp + PartialEq>(
            track: &M2Track<T>,
            gseqs: &[u32],
            slot: Option<SeqSlot>,
            proj: impl Fn(T) -> V,
        ) -> Option<KeyAnim<V>> {
            bake_track(track, gseqs, slot, proj, |_| false, |_| false)
        }
        let bones = model
            .bones
            .iter()
            .map(|b| SkyBone {
                parent: usize::try_from(b.parent).ok(),
                pivot: [b.pivot.x, b.pivot.y, b.pivot.z],
                animated: b.flags.bits() & BONE_ANIMATED != 0,
                translation: bake(&b.translation, gseqs, slot, |v| v),
                rotation: bake(&b.rotation, gseqs, slot, |v| v),
                scale: bake(&b.scale, gseqs, slot, |v| v),
            })
            .collect();
        Self {
            length: slot.map_or(0.0, |s| s.band.1.saturating_sub(s.band.0) as f32 / 1000.0),
            looping: slot.is_none_or(|s| s.looping),
            slot: slot_index,
            bones,
        }
    }

    /// The bone count.
    pub fn bone_count(&self) -> usize {
        self.bones.len()
    }

    /// Whether no bone ever moves: every matrix is the identity at every time.
    pub fn is_static(&self) -> bool {
        self.bones.iter().all(|b| {
            !b.animated || (b.translation.is_none() && b.rotation.is_none() && b.scale.is_none())
        })
    }

    /// Per bone: can its matrix move? A bone that composes a keyed track, or rides one that does.
    /// A constant track counts: it is cheap to over-include, and wrong to miss.
    pub fn moving_bones(&self) -> Vec<bool> {
        let mut out: Vec<bool> = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let own = b.animated
                && (b.translation.is_some() || b.rotation.is_some() || b.scale.is_some());
            let inherited = b.parent.and_then(|p| out.get(p).copied()).unwrap_or(false);
            out.push(own || inherited);
        }
        out
    }

    /// Every bone's model-space matrix (WoW model space) at `band_t` seconds into the armed
    /// sequence, `gseq_now` being the model's global-sequence cursor in seconds. A parent listed
    /// after its child (none ships) reads the identity.
    pub fn pose(&self, band_t: f32, gseq_now: f64) -> Vec<BoneMatrix> {
        let mut out: Vec<BoneMatrix> = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let parent = b
                .parent
                .and_then(|p| out.get(p).copied())
                .unwrap_or(BoneMatrix::IDENTITY);
            if !b.animated {
                out.push(parent);
                continue;
            }
            let t = b.translation.as_ref().map_or([0.0; 3], |a| {
                a.sample_or(a.clock(band_t, gseq_now), [0.0; 3])
            });
            // The component lerp the bake's sampler does, normalised in `quat_rows` (`0x982630`).
            let q = b.rotation.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |a| {
                a.sample_or(a.clock(band_t, gseq_now), [0.0, 0.0, 0.0, 1.0])
            });
            let s = b.scale.as_ref().map_or([1.0; 3], |a| {
                a.sample_or(a.clock(band_t, gseq_now), [1.0; 3])
            });
            out.push(parent.then_parent(&BoneMatrix::bone_local(b.pivot, t, q, s)));
        }
        out
    }
}

/// Skin one vertex by up to four `(bone, weight)` influences (weights summing to 1): the
/// reference's palette blend, `Σ wᵢ · Mᵢ · v`. Influences on a missing bone read the identity.
pub fn skin_point(
    pose: &[BoneMatrix],
    joints: [u16; 4],
    weights: [f32; 4],
    p: [f32; 3],
) -> [f32; 3] {
    let mut acc = [0.0f32; 3];
    let mut total = 0.0;
    for i in 0..4 {
        let w = weights[i];
        if w <= 0.0 {
            continue;
        }
        let m = pose
            .get(usize::from(joints[i]))
            .copied()
            .unwrap_or(BoneMatrix::IDENTITY);
        let v = m.transform_point(p);
        for k in 0..3 {
            acc[k] += v[k] * w;
        }
        total += w;
    }
    if total <= 0.0 {
        return p;
    }
    [acc[0] / total, acc[1] / total, acc[2] / total]
}

/// The sky's band time in seconds at `elapsed` seconds into the armed sequence: wrapped for a
/// looping sequence, held at its tail for a one-shot.
pub fn sky_band_time(rig: &SkyRig, elapsed: f64) -> f32 {
    if rig.length <= 0.0 {
        return 0.0;
    }
    let len = f64::from(rig.length);
    if rig.looping {
        elapsed.rem_euclid(len) as f32
    } else {
        elapsed.clamp(0.0, len) as f32
    }
}

/// The time-of-day sync (`0x7ecf20`): on a minute jump of two or more, a flag-`0x1` sky seeks to
/// the day fraction and plays one sequence per game day; `last_minutes` is stamped on every call.
pub fn sync_sky_anim(
    rig: &SkyRig,
    flags: u32,
    minutes: u32,
    last_minutes: &mut u32,
) -> Option<SkySync> {
    let length_ms = (f64::from(rig.length) * 1000.0).round() as u32;
    if length_ms == 0 {
        return None;
    }
    let jumped = minutes.abs_diff(*last_minutes) >= 2;
    *last_minutes = minutes;
    (jumped && flags & crate::SKYBOX_FLAG_SYNC_TIME != 0).then_some(SkySync {
        // `length · minutes / 1440`, in 12340's float order.
        seek_ms: (length_ms as f32 * (minutes as f32 * 0.000_694_444_46)) as u32,
        speed: 0.000_000_011_574_074 * length_ms as f32,
    })
}

/// A sync's result: seek the armed sequence to `seek_ms` and play it at `speed` × real time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkySync {
    pub seek_ms: u32,
    pub speed: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < eps)
    }

    fn rig(bones: Vec<SkyBone>, length: f32) -> SkyRig {
        SkyRig {
            length,
            looping: true,
            slot: Some(0),
            bones,
        }
    }

    fn spin_bone(parent: Option<usize>, pivot: [f32; 3]) -> SkyBone {
        // A quarter turn about +z at t = 1 s, identity at t = 0, on a 2 s band.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        SkyBone {
            parent,
            pivot,
            animated: true,
            translation: None,
            rotation: Some(KeyAnim {
                period: 2.0,
                step: false,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [0.0, 0.0, h, h])],
            }),
            scale: None,
        }
    }

    #[test]
    fn a_bone_turns_about_its_pivot_and_a_child_rides_its_parent() {
        let r = rig(
            vec![
                spin_bone(None, [1.0, 0.0, 0.0]),
                SkyBone {
                    parent: Some(0),
                    pivot: [0.0; 3],
                    animated: false,
                    translation: None,
                    rotation: None,
                    scale: None,
                },
            ],
            2.0,
        );
        let pose = r.pose(1.0, 0.0);
        // (2, 0, 0) is one unit from the pivot along +x; a quarter turn takes it to (1, 1, 0).
        let p = pose[0].transform_point([2.0, 0.0, 0.0]);
        assert!(close(p, [1.0, 1.0, 0.0], 1e-5), "{p:?}");
        assert_eq!(
            pose[1], pose[0],
            "an unanimated child takes its parent's matrix"
        );
        let id = r.pose(0.0, 0.0)[0];
        assert!(close(
            id.transform_point([3.0, 4.0, 5.0]),
            [3.0, 4.0, 5.0],
            1e-6
        ));
    }

    #[test]
    fn the_rotation_nlerps_between_keys() {
        let r = rig(vec![spin_bone(None, [0.0; 3])], 2.0);
        // Half way: the component lerp normalised, which is exactly the 45° turn here.
        let p = r.pose(0.5, 0.0)[0].transform_point([1.0, 0.0, 0.0]);
        let a = std::f32::consts::FRAC_PI_4;
        assert!(close(p, [a.cos(), a.sin(), 0.0], 1e-3), "{p:?}");
    }

    #[test]
    fn a_child_composes_onto_its_parent_and_scale_applies_before_rotation() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let quarter = BoneMatrix::bone_local([0.0; 3], [0.0; 3], [0.0, 0.0, h, h], [1.0; 3]);
        let stretch = BoneMatrix::bone_local(
            [0.0; 3],
            [5.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [2.0, 1.0, 1.0],
        );
        // parent · child: stretch x by 2 and move +5 x, then turn a quarter:
        // (1, 0, 0) → (7, 0, 0) → (0, 7, 0).
        let m = quarter.then_parent(&stretch);
        assert!(close(
            m.transform_point([1.0, 0.0, 0.0]),
            [0.0, 7.0, 0.0],
            1e-5
        ));
        // A pivot: a quarter turn about (1, 0, 0) takes (2, 0, 0) to (1, 1, 0).
        let about = BoneMatrix::bone_local([1.0, 0.0, 0.0], [0.0; 3], [0.0, 0.0, h, h], [1.0; 3]);
        assert!(close(
            about.transform_point([2.0, 0.0, 0.0]),
            [1.0, 1.0, 0.0],
            1e-5
        ));
    }

    #[test]
    fn a_bone_moves_with_its_own_tracks_or_its_parents() {
        let still = |parent| SkyBone {
            parent,
            pivot: [0.0; 3],
            animated: true,
            translation: None,
            rotation: None,
            scale: None,
        };
        let r = rig(
            vec![
                still(None),
                spin_bone(None, [0.0; 3]),
                still(Some(1)),
                still(Some(0)),
            ],
            2.0,
        );
        assert_eq!(r.moving_bones(), [false, true, true, false]);
    }

    #[test]
    fn skinning_blends_by_weight() {
        let up = BoneMatrix {
            translation: [0.0, 0.0, 10.0],
            ..BoneMatrix::IDENTITY
        };
        let pose = [BoneMatrix::IDENTITY, up];
        let p = skin_point(&pose, [0, 1, 0, 0], [0.5, 0.5, 0.0, 0.0], [1.0, 2.0, 3.0]);
        assert_eq!(p, [1.0, 2.0, 8.0]);
        let q = skin_point(&pose, [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
        assert_eq!(q, [0.0, 0.0, 10.0]);
    }

    #[test]
    fn the_time_sync_seeks_to_the_day_fraction_only_on_a_jump_and_only_with_bit_one() {
        let r = rig(vec![], 160.0);
        let mut last = 0;
        // First call at 12:00: a jump from the initial 0, flag 1 set.
        let s = sync_sky_anim(&r, 1, 720, &mut last).expect("synced");
        assert_eq!(s.seek_ms, 80_000, "noon is half the 160 s sequence");
        assert!((s.speed - 160_000.0 / 86_400_000.0).abs() < 1e-9);
        assert_eq!(last, 720);
        // A minute later: no sync, but the stamp moves.
        assert_eq!(sync_sky_anim(&r, 1, 721, &mut last), None);
        assert_eq!(last, 721);
        // A server time jump: synced again.
        assert!(sync_sky_anim(&r, 1, 1000, &mut last).is_some());
        // Bit 1 clear: never synced, stamp still kept.
        let mut last0 = 0;
        assert_eq!(sync_sky_anim(&r, 2, 720, &mut last0), None);
        assert_eq!(last0, 720);
        // Seen first at 00:01: |1 − 0| < 2, so it never syncs while the minutes tick by one.
        let mut early = 0;
        assert_eq!(sync_sky_anim(&r, 1, 1, &mut early), None);
        assert_eq!(sync_sky_anim(&r, 1, 2, &mut early), None);
    }

    #[test]
    fn the_band_time_wraps_a_loop_and_holds_a_one_shot() {
        let mut r = rig(vec![], 10.0);
        assert!((sky_band_time(&r, 25.0) - 5.0).abs() < 1e-6);
        r.looping = false;
        assert_eq!(sky_band_time(&r, 25.0), 10.0);
    }
}
