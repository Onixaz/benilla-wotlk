//! The parsed M2 record types and the [`M2Model`] that `crate::parse_m2` builds.

use std::ffi::CString;

use crate::skin::Skin;
use crate::timeline::{AnimFiles, Tracks264};
use crate::track::{
    M2QuatTrack, M2ScalarSplineTrack, M2ScalarTrack, M2Vec3SplineTrack, M2Vec3Track,
};

/// A 3-float vector (position / normal / pivot).
#[derive(Clone, Copy)]
pub struct C3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
/// A 2-float vector (texture coordinate).
#[derive(Clone, Copy)]
pub struct C2 {
    pub x: f32,
    pub y: f32,
}

/// M2 texture type: `Hardcoded` names its file; `Monster*` slots take the creature display's skins.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum M2TextureType {
    Hardcoded,
    Monster1,
    Monster2,
    Monster3,
    Other(u32),
}

impl M2TextureType {
    pub(crate) fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Hardcoded,
            11 => Self::Monster1,
            12 => Self::Monster2,
            13 => Self::Monster3,
            other => Self::Other(other),
        }
    }
}

/// A texture's filename.
pub struct M2ArrayString {
    pub string: CString,
}

/// One M2 texture definition.
pub struct M2Texture {
    pub texture_type: M2TextureType,
    /// Address mode on U (`flags & 0x1`): set repeats, clear clamps to edge.
    pub wrap_x: bool,
    /// Address mode on V (`flags & 0x2`).
    pub wrap_y: bool,
    pub filename: M2ArrayString,
}

/// M2 material render flags (`& 0x01` = unlit/emissive, `& 0x04` = two-sided).
#[derive(Clone, Copy)]
pub struct M2RenderFlags(pub(crate) u16);
impl M2RenderFlags {
    pub fn bits(&self) -> u16 {
        self.0
    }
}
/// M2 material blend mode (0 opaque, 1 alpha-key, 2 alpha, 3/4 additive).
#[derive(Clone, Copy)]
pub struct M2BlendMode(pub(crate) u16);
impl M2BlendMode {
    pub fn bits(&self) -> u16 {
        self.0
    }
}

/// One M2 material (MD20 "render flags" entry).
pub struct M2Material {
    pub flags: M2RenderFlags,
    pub blend_mode: M2BlendMode,
}

/// M2 bone flags (`& 0x08` spherical billboard, `& 0x10/0x20/0x40` cylindrical billboard).
#[derive(Clone, Copy)]
pub struct M2BoneFlags(pub(crate) u32);
impl M2BoneFlags {
    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// One M2 bone (`M2CompBone`, `0x58` bytes): keyBoneId i32 `+0x00`, flags `+0x04`, parent i16
/// `+0x08`, the translation, rotation and scale tracks at `+0x10`, `+0x24`, `+0x38`, pivot C3
/// `+0x4c`.
pub struct M2Bone {
    /// `KeyBoneID`, `-1` for none: 0/1 arms L/R, 2/3 shoulders L/R, 4 spine low, 5 waist, 6 head…
    pub key_bone: i16,
    pub flags: M2BoneFlags,
    /// Parent bone index, `-1` for a root.
    pub parent: i16,
    pub pivot: C3,
    pub translation: M2Vec3Track,
    /// The rotation, unpacked from four `i16`s to `[x, y, z, w]`.
    pub rotation: M2QuatTrack,
    pub scale: M2Vec3Track,
}
impl M2Bone {
    /// Whether this bone billboards (any of the spherical/cylindrical bits).
    pub fn is_billboard(&self) -> bool {
        self.flags.0 & (0x08 | 0x10 | 0x20 | 0x40) != 0
    }
}

/// One M2 vertex, 48 bytes: position @0, weights @0x0c, indices @0x10, normal @0x14, UV @0x20,
/// second UV @0x28.
pub struct M2Vertex {
    pub position: C3,
    /// Per-vertex bone influence weights (`/255`), paired with [`Self::bone_indices`].
    pub bone_weights: [u8; 4],
    pub bone_indices: [u8; 4],
    pub normal: C3,
    pub tex_coords: C2,
    /// The second UV set, which a texture stage reads as `T2` (`Diffuse_T1_T2`, `Diffuse_T2`).
    pub tex_coords2: C2,
}

/// One sequence (`M2Sequence`, `0x40` bytes), its band on the model's one timeline.
#[derive(Clone, Copy, Debug)]
pub struct M2Sequence {
    /// `AnimationData.dbc` id (`+0x00`).
    pub id: u16,
    /// The variation index (`+0x02`).
    pub sub: u16,
    /// The band `[start, end]` its tracks key on, ms; `end - start` is the length (`+0x04`).
    pub start: u32,
    pub end: u32,
    /// `moveSpeed` (`+0x08`, yd/s).
    pub move_speed: f32,
    /// `+0x0c`: bit 0 clear loops, `0x20` keys in the `.m2`, `0x40` an alias.
    pub flags: u32,
    /// `frequency` (`+0x10`): this variation's weight in the per-play roll.
    pub frequency: u16,
    /// The replay range (`+0x14`/`+0x18`).
    pub min_replay: u32,
    pub max_replay: u32,
    /// `blendTime` (`+0x1c`, ms).
    pub blend_time: u32,
    /// The box (`+0x20`/`+0x2c`) and the sphere radius (`+0x38`).
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub bounds_radius: f32,
    /// `variationNext` (`+0x3c`).
    pub variation_next: i16,
    /// `aliasNext` (`+0x3e`).
    pub alias_next: u16,
}

/// One animation event (`M2Event`, `0x24` bytes): a 4CC, its data, a bone-relative point and its
/// times on the timeline.
#[derive(Clone, Debug)]
pub struct M2Event {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    /// `+0x04`: a SoundEntries id for `$SND`/`$DSL`/`$DSO`, else 0.
    pub data: u32,
    /// `+0x08` as the file holds it, which can be out of the bone range.
    pub bone: u16,
    /// `+0x0c`, model space.
    pub position: [f32; 3],
    /// The `M2TrackBase` at `+0x18`: its global sequence (`0xffff` for none), a window per
    /// sequence and the times.
    pub gseq: u16,
    pub ranges: Vec<(u32, u32)>,
    pub times: Vec<u32>,
}

/// One M2 light (`M2Light`, `0x9c` bytes): type `+0x00`, bone `+0x02` (`-1` for none), position
/// `+0x04`, then the seven tracks from `+0x10`.
#[derive(Clone, Debug)]
pub struct M2LightDef {
    pub light_type: u16,
    pub bone: i16,
    pub position: [f32; 3],
    pub ambient_color: M2Vec3Track,
    pub ambient_intensity: M2ScalarTrack,
    pub diffuse_color: M2Vec3Track,
    pub diffuse_intensity: M2ScalarTrack,
    pub attenuation_start: M2ScalarTrack,
    pub attenuation_end: M2ScalarTrack,
    /// One byte a key, as `0..=255`.
    pub visibility: M2ScalarTrack,
}

/// The texture lookup table and the collision hull's raw bytes.
pub struct M2RawData {
    pub texture_lookup_table: Vec<u16>,
    /// Collision-hull triangle indices, raw bytes (`count × 2`, `u16` each).
    pub bounding_triangles: Vec<u8>,
    /// Collision-hull vertices, raw bytes (`count × 12`, `C3Vector` each).
    pub bounding_vertices: Vec<u8>,
}

/// The MD20 header's authored bounds: the bounding box and sphere are the render and cull volume
/// over every animation, the collision box and sphere the tight hull.
pub struct M2Header {
    pub bounding_box_min: [f32; 3],
    pub bounding_box_max: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub collision_box_min: [f32; 3],
    pub collision_box_max: [f32; 3],
    pub collision_sphere_radius: f32,
}

/// One attachment point (`M2Attachment`, `0x28` bytes), `id` narrowed to `u16`. On character
/// models `position` equals the attach bone's pivot. Ids: 0 shield (left forearm), 1 right hand,
/// 2 left hand, 26/27 right/left back sheath, 28 shield back, 30/31 lower back, 32/33 hips.
#[derive(Clone, Copy)]
pub struct M2Attachment {
    pub id: u16,
    pub bone: u16,
    /// Model-space position, raw WoW axes like [`M2Bone::pivot`].
    pub position: [f32; 3],
}

/// The positional half of an animation-event record: found by 4CC, first match, and moved by
/// `bone`'s matrix. Cast launch points `$CSL`/`$CSR`/`$CST`, bow anchors `$WTT`/`$WTB`, ranged
/// release `$BWR`.
#[derive(Clone, Copy)]
pub struct M2EventMarker {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    pub bone: u16,
    /// Model-space position, raw WoW axes.
    pub position: [f32; 3],
}

/// One M2 texture transform (header `0x60`, stride `0x3c`), its UV-animation tracks at `+0x00`,
/// `+0x14`, `+0x28`.
pub struct M2TextureTransform {
    pub translation: M2Vec3Track,
    pub rotation: M2QuatTrack,
    pub scaling: M2Vec3Track,
}

/// One M2 camera record (header `0x110`, stride `0x64`): a `Cameras\*.m2` fly-by's path, and the
/// framing of the portrait bake and `<Model>` panes. The camera adds each track to its base
/// (`eye = position_base + positions(t)`), so a keyless track leaves the base.
#[derive(Clone)]
pub struct M2Camera {
    /// The `type` word (`+0x00`): 0 portrait, 1 the `<PlayerModel>` pane, `-1` on every fly-by.
    pub camera_type: i32,
    /// Diagonal field of view, radians (`+0x04`): the projection takes the half-angle as
    /// `(fov/2)/√(aspect²+1)`, not `fov/2`.
    pub fov: f32,
    /// Far clip (`+0x08`); every fly-by carries `27.777779` (1000/36).
    pub far_clip: f32,
    /// Near clip (`+0x0c`); every fly-by carries `0.22222222` (8/36).
    pub near_clip: f32,
    /// The eye path (`+0x10`), relative to [`Self::position_base`].
    pub positions: M2Vec3SplineTrack,
    /// The eye's base position, model space (`+0x24`).
    pub position_base: [f32; 3],
    /// The look-at path (`+0x30`), relative to [`Self::target_base`].
    pub target: M2Vec3SplineTrack,
    /// The look-at base position, model space (`+0x44`).
    pub target_base: [f32; 3],
    /// Roll about the view axis, radians (`+0x50`).
    pub roll: M2ScalarSplineTrack,
}

/// A parsed version-264 model with its skin profile 0.
pub struct M2Model {
    /// The sequences in file order, the slot every track's key window is indexed by.
    pub sequences: Vec<M2Sequence>,
    pub vertices: Vec<M2Vertex>,
    pub textures: Vec<M2Texture>,
    pub materials: Vec<M2Material>,
    /// Each `M2Color`'s alpha track (header `0x48`), picked directly by a batch's `colorIndex`; no
    /// keys means no factor. It gates visibility and feeds the material combine.
    pub color_alpha_tracks: Vec<M2ScalarTrack>,
    /// Each `M2Color`'s RGB track: the batch tint multiplied into the vertex colour, none without
    /// keys. Neutral glow cards (`GenericGlow_Alpha_128`) take all their colour from it.
    pub color_rgb_tracks: Vec<M2Vec3Track>,
    /// Each `M2TextureWeight`'s weight track (header `0x58`), via [`Self::transparency_lookup`].
    pub transparency_tracks: Vec<M2ScalarTrack>,
    /// The transparency lookup (header `0x90`, u16): `weight_combo_index` to
    /// [`Self::transparency_tracks`].
    pub transparency_lookup: Vec<u16>,
    /// The texture-unit lookup (header `0x88`), by `texture_coord_combo_index`: a UV channel or a
    /// generated environment coordinate (see [`Self::stage_is_env_mapped`]).
    pub texture_unit_lookup: Vec<u16>,
    /// The texture transforms, reached through [`Self::texture_transform_lookup`].
    pub texture_transforms: Vec<M2TextureTransform>,
    /// The texture-animation lookup (header `0x98`, u16): `texture_transform_combo_index` to
    /// [`Self::texture_transforms`], `0xffff` for none.
    pub texture_transform_lookup: Vec<u16>,
    /// Global-sequence durations (header `0x14`, ms): the clocks `gseq` tracks wrap on.
    pub global_sequences: Vec<u32>,
    pub bones: Vec<M2Bone>,
    /// The camera records (header `0x110`): one in a `Cameras\*.m2` fly-by, none in most models.
    pub cameras: Vec<M2Camera>,
    /// CameraLookup (header `0x118`): a camera purpose to its [`Self::cameras`] slot. The portrait
    /// bake reads it; the `<Model>` pane indexes cameras directly.
    pub camera_lookup: Vec<u16>,
    /// The lights (header `0x108`), in file order.
    pub lights: Vec<M2LightDef>,
    pub raw_data: M2RawData,
    pub header: M2Header,
    /// Model-space Z of attachment id 17, the follow camera's pivot height.
    pub pivot_attach_z: Option<f32>,
    /// The surviving attachment records; address them by id via [`Self::attachment`], never scan.
    pub attachments: Vec<M2Attachment>,
    /// AttachLookup (header `0xf8`) as [`Self::attachments`] indices, `0xffff` for none.
    pub attach_lookup: Vec<u16>,
    /// Every animation event, in file order.
    pub events: Vec<M2Event>,
    /// The events a bone poses, in file order; a query takes the first ident match.
    pub event_markers: Vec<M2EventMarker>,
    /// AnimationLookup (header `0x24`): row `i` is the first sequence slot for
    /// `AnimationData.dbc` id `i`, `0xffff` for none; it ends past the highest authored id.
    pub animation_lookup: Vec<u16>,
    /// The header's global flags (`+0x10`); `0x8` makes the batch shader word a
    /// [`Self::texture_combiner_combos`] index (`CM2Shared::SubstituteSimpleShaders 0x836980`).
    pub global_flags: u32,
    /// The texture combiner ops (header `0x130`), read only under global flag `0x8`.
    pub texture_combiner_combos: Vec<u16>,
    /// Skin profile 0, the `<Model>00.skin` file.
    pub skin: Skin,
    /// What the timeline could not hold ([`Tracks264::dropped`]), one entry per kind.
    pub dropped: Vec<String>,
    /// The `.m2` and its `.anim` files, which the emitter readers walk.
    pub(crate) bytes: Vec<u8>,
    pub(crate) anims: AnimFiles,
}

impl M2Model {
    /// Whether `batch`'s stage `stage` takes generated environment coordinates: a
    /// [`Self::texture_unit_lookup`] entry above 2 (art: `0xffff`), and an out-of-range index, is
    /// the sphere map of the view-space reflection, which no texture transform drives.
    pub fn stage_is_env_mapped(&self, batch: &crate::SkinBatch, stage: u16) -> bool {
        let idx = batch.texture_coord_combo_index as usize + stage as usize;
        self.texture_unit_lookup.get(idx).is_none_or(|&v| v > 2)
    }

    /// Whether the model authors `AnimationData.dbc` id `anim_id`: a bounds-checked
    /// [`Self::animation_lookup`] read, so an id past the end is not owned.
    pub fn owns_animation(&self, anim_id: u16) -> bool {
        self.animation_lookup
            .get(anim_id as usize)
            .is_some_and(|&slot| slot != 0xffff)
    }

    /// The sequence slot `anim_id` resolves to through [`Self::animation_lookup`].
    pub fn sequence_slot(&self, anim_id: u16) -> Option<usize> {
        let slot = usize::from(*self.animation_lookup.get(usize::from(anim_id))?);
        (slot < self.sequences.len()).then_some(slot)
    }

    /// The record attach id `id` resolves to through [`Self::attach_lookup`]. An unresolved id
    /// hangs nothing: its users skip the sentinel. A scan of [`Self::attachments`] disagrees where
    /// weapons repeat ids.
    pub fn attachment(&self, id: u16) -> Option<&M2Attachment> {
        let idx = *self.attach_lookup.get(id as usize)?;
        (idx != 0xffff)
            .then(|| self.attachments.get(idx as usize))
            .flatten()
    }

    /// The `.m2` bytes, which the particle and ribbon readers walk.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The particle emitter records' `(count, offset)` (header `0x128`, `0x1dc` bytes each).
    pub fn particle_emitters(&self) -> (usize, usize) {
        crate::timeline::arr(&self.bytes, crate::timeline::h264::PARTICLE_EMITTERS)
            .unwrap_or((0, 0))
    }

    /// The ribbon emitter records' `(count, offset)` (header `0x120`, `0xb0` bytes each).
    pub fn ribbon_emitters(&self) -> (usize, usize) {
        crate::timeline::arr(&self.bytes, crate::timeline::h264::RIBBON_EMITTERS).unwrap_or((0, 0))
    }

    /// The model's tracks on its timeline, for the records this crate does not parse.
    pub fn tracks(&self) -> Tracks264<'_> {
        Tracks264::new(&self.bytes, &self.anims).expect("parse_m2 built this timeline")
    }
}
