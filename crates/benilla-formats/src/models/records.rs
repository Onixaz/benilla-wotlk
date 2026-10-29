//! Authored records in raw WoW model space: M2 lights, the WMO root's MOLT lights, and the M2
//! camera table.

use benilla_bytes::ByteExt;
use benilla_m2::{M2LightDef, M2Model, M2Track};

/// An M2 light (the MD20 lights array), `position` relative to `bone` (`-1`
/// for the origin). The reference commits a point light on a placed prop as a hardware `GL_LIGHT`
/// with the fixed attenuation `1 / (0.7·d + 0.03·d²)`, diffuse only; the glue scenes light their
/// ambient through an ambient-only light, so both pairs are read. Colour and intensity are each
/// track's first value, where the reference animates them; the attenuation range is a cull hint.
#[derive(Debug, Clone, Copy)]
pub struct M2Light {
    pub light_type: u16,
    pub bone: i16,
    pub position: [f32; 3],
    pub ambient_color: [f32; 3],
    pub ambient_intensity: f32,
    pub diffuse_color: [f32; 3],
    pub diffuse_intensity: f32,
    pub attenuation_start: f32,
    pub attenuation_end: f32,
    /// The light bone's +Z axis in model space at the track origin, a directional light's basis:
    /// the reference reads row 2 of the bone's pose matrix, never the def position.
    pub bone_z: [f32; 3],
    /// Both runtime gate bytes load as 1 and the animate loop rewrites the visibility byte only
    /// when the track has keys, so a light is dark only when its first visibility key is 0. The
    /// values are bytes (: `+0xec = visibilityValue[k0]`).
    pub visibility_off: bool,
}

impl M2Light {
    /// A point light; type 0 feeds the ambient accumulator, not a GL light.
    pub fn is_point(&self) -> bool {
        self.light_type == 1
    }

    /// The spawn gate: a point light its visibility track does not hold dark.
    pub fn casts(&self) -> bool {
        self.is_point() && !self.visibility_off
    }
}

/// A WMO MOLT light (`SMOLight`, stride `0x30`), in WMO model space. The 1.12 client commits them
/// to the scene's dynamic lights every frame for every visible WMO, as `colour × intensity` point
/// lights at the fixed 0/0.7/0.03 falloff over the baked MOCV.
#[derive(Debug, Clone, Copy)]
pub struct WmoLight {
    pub light_type: u8,
    pub use_atten: bool,
    pub color: [f32; 3],
    pub position: [f32; 3],
    pub intensity: f32,
    pub attenuation_start: f32,
    pub attenuation_end: f32,
}

impl WmoLight {
    /// An omni light, the interior fixtures' type (1 is spot, 2 directional, 3 ambient).
    pub fn is_omni(&self) -> bool {
        self.light_type == 0
    }
}

/// A track's first value.
fn first<V: Copy>(track: &M2Track<V>) -> Option<V> {
    track.keys.first().map(|&(_, v)| v)
}

/// Hamilton product `a·b` of two `[x, y, z, w]` quaternions (the vanilla M2 key layout).
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// `q · (0,0,1) · q⁻¹`, the +Z axis rotated by `q`, in closed form.
fn quat_rotate_z(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    [
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

/// A bone's model-space +Z at the track origin, its first rotation keys composed up the parent
/// chain. Bone flag `0x04` keeps the root's orientation. Rotation animation is not applied.
fn bone_z_axis(model: &M2Model, bone: i16) -> [f32; 3] {
    // Local rotations leaf to root, bounded by the bone count.
    let mut chain: Vec<[f32; 4]> = Vec::new();
    let mut idx = bone;
    for _ in 0..=model.bones.len() {
        let Some(b) = usize::try_from(idx).ok().and_then(|i| model.bones.get(i)) else {
            break;
        };
        if let Some(q) = first(&b.rotation) {
            chain.push(q);
        }
        if b.flags.bits() & 0x4 != 0 {
            break; // the bone keeps the model root's orientation
        }
        idx = b.parent;
    }
    let mut q = [0.0, 0.0, 0.0, 1.0];
    for local in chain.iter().rev() {
        q = quat_mul(q, *local); // root-first: global = root ∘ … ∘ local
    }
    quat_rotate_z(q)
}

/// One M2 light as the scene commits it: colours and intensities their tracks' first values, the
/// attenuation range, and the visibility gate.
fn m2_light(model: &M2Model, l: &M2LightDef) -> M2Light {
    M2Light {
        light_type: l.light_type,
        bone: l.bone,
        bone_z: bone_z_axis(model, l.bone),
        position: l.position,
        ambient_color: first(&l.ambient_color).unwrap_or([1.0; 3]),
        ambient_intensity: first(&l.ambient_intensity).unwrap_or(0.0),
        diffuse_color: first(&l.diffuse_color).unwrap_or([1.0; 3]),
        diffuse_intensity: first(&l.diffuse_intensity).unwrap_or(1.0),
        attenuation_start: first(&l.attenuation_start).unwrap_or(0.0),
        attenuation_end: first(&l.attenuation_end).unwrap_or(0.0),
        visibility_off: first(&l.visibility) == Some(0.0),
    }
}

/// The M2 lights, in file order.
pub fn parse_m2_lights(model: &M2Model) -> Vec<M2Light> {
    model.lights.iter().map(|l| m2_light(model, l)).collect()
}

/// The model's portrait camera, which the 1.12 client renders every unit-frame portrait through:
/// the bake takes camera `cameraLookup[0]` and builds `lookAt(position, target, up-from-roll)` with
/// the diagonal-FOV perspective (half-angle `(fov/2)/√(aspect²+1)`) at a fixed 4:3, a vertical
/// half-angle of `0.3·fov`, with nothing on top. Model space, radians.
#[derive(Debug, Clone, Copy)]
pub struct M2PortraitCamera {
    /// The diagonal opening angle, not fovy.
    pub fov: f32,
    pub far_clip: f32,
    pub near_clip: f32,
    /// The eye, like `target` its base plus its track's first key; `roll` is a first key alone.
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub roll: f32,
}

/// The portrait camera, `cameraLookup[0]` into the camera table as the reference selects it;
/// `None` without one, the `0xffff` sentinel included.
pub fn parse_m2_portrait_camera(model: &M2Model) -> Option<M2PortraitCamera> {
    let idx = *model.camera_lookup.first()? as usize;
    parse_m2_camera(model, idx)
}

/// One camera record by table index, the glue screens' `Model:SetCamera(idx)` path: their one
/// camera has the `0xffff` sentinel in its lookup slot, so the client indexes the table directly.
/// Key 0 of each track; a moving camera is sampled through [`M2PaneCamera::at`].
pub fn parse_m2_camera(model: &M2Model, index: usize) -> Option<M2PortraitCamera> {
    let cam = model.cameras.get(index)?;
    let base_plus_key = |base: [f32; 3], track: &benilla_m2::M2Vec3SplineTrack| {
        let k = track.keys.first().map_or([0.0; 3], |(_, k)| k.value);
        [base[0] + k[0], base[1] + k[1], base[2] + k[2]]
    };
    Some(M2PortraitCamera {
        fov: cam.fov,
        far_clip: cam.far_clip,
        near_clip: cam.near_clip,
        position: base_plus_key(cam.position_base, &cam.positions),
        target: base_plus_key(cam.target_base, &cam.target),
        roll: cam.roll.keys.first().map_or(0.0, |(_, k)| k.value),
    })
}

/// One `SMOLight` at a bounds-checked `r`: type@0, useAtten@1, colour (BGRA)@4, position@8,
/// intensity@0x14, attenStart@0x28, attenEnd@0x2c. The attenuation the reference uses is the
/// tail; `+0x18..+0x28` hold four other floats (≈0, −0, −1, −0.5 on every vanilla light).
fn read_wmo_light(b: &[u8], r: usize) -> Option<WmoLight> {
    Some(WmoLight {
        light_type: b.u8_at(r)?,
        use_atten: b.u8_at(r + 1)? != 0,
        // CImVector is BGRA in memory.
        color: [
            f32::from(b.u8_at(r + 6)?) / 255.0,
            f32::from(b.u8_at(r + 5)?) / 255.0,
            f32::from(b.u8_at(r + 4)?) / 255.0,
        ],
        position: [b.f32_at(r + 8)?, b.f32_at(r + 12)?, b.f32_at(r + 16)?],
        intensity: b.f32_at(r + 0x14)?,
        attenuation_start: b.f32_at(r + 0x28)?,
        attenuation_end: b.f32_at(r + 0x2c)?,
    })
}

/// A WMO root file's MOLT lights, which `benilla-wmo` does not expose: the chunk list scanned for
/// `MOLT` (stored reversed, `TLOM`). The walk must not stop at a zero-size chunk: `MOVV(0)` and
/// `MOVB(0)` sit right before MOLT in vanilla roots. Hand-rolled, not `benilla_bytes::chunks()`:
/// each record is bounded by the whole buffer, not by a clamped chunk payload.
pub fn parse_wmo_lights(root_bytes: &[u8]) -> Vec<WmoLight> {
    let b = root_bytes;
    let mut o = 0usize;
    while o + 8 <= b.len() {
        let Some(size) = b.u32_at(o + 4).map(|v| v as usize) else {
            break;
        };
        let data = o + 8;
        if &b[o..o + 4] == b"TLOM" || &b[o..o + 4] == b"MOLT" {
            let n = size / 0x30;
            let mut out = Vec::with_capacity(n.min(256));
            for i in 0..n {
                let Some(r) = data.checked_add(i * 0x30).filter(|&r| r + 0x30 <= b.len()) else {
                    break;
                };
                match read_wmo_light(b, r) {
                    Some(light) => out.push(light),
                    None => break, // unreachable after the filter above
                }
            }
            return out;
        }
        o = match data.checked_add(size) {
            Some(x) => x,
            None => break,
        };
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2-bone chain: the root turned 90° about +X, the child 90° about its own +Z.
    fn two_bone_chain(rotations: bool) -> benilla_m2::M2Model {
        use benilla_m2::fixture::{self, header as h, packed_quat, sequence, Model};
        let mut m = Model::new();
        m.array(h::SEQUENCES, 1, &sequence(0, 100, 0x20));
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let rot = |m: &mut Model, q: [f32; 4]| match rotations {
            true => m.track(1, 0xffff, &[&[(0, packed_quat(q))]]),
            false => m.no_track(),
        };
        // Bone 0 (root): rotation 90° about +X. Bone 1: parent 0, local rotation 90° about +Z.
        let r0 = rot(&mut m, [s, 0.0, 0.0, s]);
        let r1 = rot(&mut m, [0.0, 0.0, s, s]);
        let (t, sc) = (m.no_track(), m.no_track());
        let b0 = fixture::bone(-1, 0, [0.0; 3], [&t, &r0, &sc]);
        let b1 = fixture::bone(0, 0, [0.0; 3], [&t, &r1, &sc]);
        m.array(h::BONES, 2, &[b0, b1].concat());
        m.parse(&fixture::empty_skin()).unwrap()
    }

    /// Root alone: 90° about +X sends +Z to −Y. The child's turn about +Z leaves its own +Z, so
    /// the chain gives the root's axis.
    #[test]
    fn bone_z_axis_composes_parent_then_local() {
        // Packed rotations carry 1/32767 steps, hence the tolerance.
        let m = two_bone_chain(true);
        let root = bone_z_axis(&m, 0);
        assert!((root[0]).abs() < 1e-4 && (root[1] + 1.0).abs() < 1e-4 && root[2].abs() < 1e-4);
        let child = bone_z_axis(&m, 1);
        for (c, r) in child.iter().zip(root) {
            assert!((c - r).abs() < 1e-4, "child {child:?} vs root {root:?}");
        }
        // No rotation keys anywhere: identity +Z.
        assert_eq!(bone_z_axis(&two_bone_chain(false), 0), [0.0, 0.0, 1.0]);
    }
}

/// One camera table record by raw index, what `Model:SetCamera(n)` selects on a `<Model>` widget:
/// the reference reads the count off `MD20+0x124` and the record at
/// `[model+0x3c4] + idx·0x84 + 0x80` without consulting `cameraLookup` (for the portrait bake), so
/// the index decides and [`Self::camera_type`] never does. Every shipped character, creature and
/// interface camera is its [`Self::still`] rig, each track one key of zero; [`Self::at`] samples
/// the `Cameras\*.m2` fly-bys.
#[derive(Debug, Clone)]
pub struct M2PaneCamera {
    /// The `type` word (`+0x00`): 0 portrait, 1 characterinfo, -1 on every shipped fly-by and the
    /// six glue scenes.
    pub camera_type: i32,
    /// The rig at rest: the bases plus each track's first key.
    pub still: M2PortraitCamera,
    /// The authored tracks, kept only when one moves (more than one distinct key).
    pub tracks: Option<Box<M2CameraTracks>>,
}

/// A moving camera's tracks and bases, the publish pass's inputs:
/// `eye = position_base + positions(t)`, likewise the target.
#[derive(Debug, Clone)]
pub struct M2CameraTracks {
    pub positions: benilla_m2::M2Vec3SplineTrack,
    pub position_base: [f32; 3],
    pub target: benilla_m2::M2Vec3SplineTrack,
    pub target_base: [f32; 3],
    pub roll: benilla_m2::M2ScalarSplineTrack,
}

impl M2PaneCamera {
    /// The rig at absolute file-timeline `ms`, through [`benilla_m2::M2Track::sample_ms`] (the
    /// reference's four-way `interp` dispatch, both ends clamped; the fly-bys author Bézier). A
    /// pane's `ms` is its play head plus the armed sequence's band start.
    pub fn at(&self, ms: u32) -> M2PortraitCamera {
        let Some(t) = self.tracks.as_deref() else {
            return self.still;
        };
        let add = |b: [f32; 3], v: Option<[f32; 3]>| {
            let v = v.unwrap_or([0.0; 3]);
            [b[0] + v[0], b[1] + v[1], b[2] + v[2]]
        };
        M2PortraitCamera {
            position: add(t.position_base, t.positions.sample_ms(ms)),
            target: add(t.target_base, t.target.sample_ms(ms)),
            roll: t.roll.sample_ms(ms).unwrap_or(self.still.roll),
            ..self.still
        }
    }
}

/// The whole camera table in file order, the index space of `Model:SetCamera(n)`; empty for most
/// models.
pub fn parse_m2_pane_cameras(model: &M2Model) -> Vec<M2PaneCamera> {
    model
        .cameras
        .iter()
        .map(|cam| {
            let key0 = |t: &benilla_m2::M2Vec3SplineTrack| {
                t.keys.first().map_or([0.0; 3], |(_, k)| k.value)
            };
            let base_plus = |b: [f32; 3], t: &benilla_m2::M2Vec3SplineTrack| {
                let k = key0(t);
                [b[0] + k[0], b[1] + k[1], b[2] + k[2]]
            };
            let still = M2PortraitCamera {
                fov: cam.fov,
                far_clip: cam.far_clip,
                near_clip: cam.near_clip,
                position: base_plus(cam.position_base, &cam.positions),
                target: base_plus(cam.target_base, &cam.target),
                roll: cam.roll.keys.first().map_or(0.0, |(_, k)| k.value),
            };
            // A track that never leaves its first key is the still rig at every instant.
            let moves = |n: usize, same: bool| n > 1 && !same;
            let v3_moves = |t: &benilla_m2::M2Vec3SplineTrack| {
                let first = key0(t);
                moves(t.keys.len(), t.keys.iter().all(|(_, k)| k.value == first))
            };
            let roll_moves = {
                let first = cam.roll.keys.first().map_or(0.0, |(_, k)| k.value);
                moves(
                    cam.roll.keys.len(),
                    cam.roll.keys.iter().all(|(_, k)| k.value == first),
                )
            };
            let tracks =
                (v3_moves(&cam.positions) || v3_moves(&cam.target) || roll_moves).then(|| {
                    Box::new(M2CameraTracks {
                        positions: cam.positions.clone(),
                        position_base: cam.position_base,
                        target: cam.target.clone(),
                        target_base: cam.target_base,
                        roll: cam.roll.clone(),
                    })
                });
            M2PaneCamera {
                camera_type: cam.camera_type,
                still,
                tracks,
            }
        })
        .collect()
}
