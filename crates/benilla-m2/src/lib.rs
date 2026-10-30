//! An M2 (MD20) model reader for WoW 3.3.5a. A model is MD20 version 264: a header of `M2Array`s
//! (`count`, file `offset`), the arrays they name, its skin profiles in `<Model>NN.skin` and the
//! keys of some sequences in `<Model><id>-<sub>.anim`. [`parse_m2`] reads the `.m2` with skin
//! profile 0 and those `.anim` files, its tracks onto one timeline ([`Tracks264`]). Particle and
//! ribbon emitters are not read here; benilla-formats reads them from [`M2Model::bytes`] through
//! [`M2Model::tracks`].

mod error;
mod model;
mod skin;
mod timeline;
mod track;

#[cfg(any(test, feature = "fixture"))]
pub mod fixture;

pub use error::Error;
pub use model::{
    find_sequence_slot, M2ArrayString, M2Attachment, M2BlendMode, M2Bone, M2BoneFlags, M2Camera,
    M2Event, M2EventMarker, M2Header, M2LightDef, M2Material, M2Model, M2RawData, M2RenderFlags,
    M2Sequence, M2Texture, M2TextureTransform, M2TextureType, M2Vertex, C2, C3,
};
pub use skin::{Skin, SkinBatch, SkinSection};
pub use timeline::{anim_paths, md20_version, skin_path, AnimFiles, Tracks264, VERSION_264};
pub use track::{
    CubicValue, M2QuatTrack, M2ScalarSplineTrack, M2ScalarTrack, M2SplineKey, M2Track,
    M2Vec3SplineTrack, M2Vec3Track,
};

use std::ffi::CString;

use benilla_bytes::ByteExt;

use error::Result;
use timeline::{arr, get, get_n, h264, u16_at, u32_at, TRACK_264};

/// A `C3Vector` (3×f32) at `o`.
fn rd_c3(b: &[u8], o: usize) -> Result<C3> {
    Ok(C3 {
        x: b.f32_at(o).ok_or(Error::Truncated)?,
        y: b.f32_at(o + 4).ok_or(Error::Truncated)?,
        z: b.f32_at(o + 8).ok_or(Error::Truncated)?,
    })
}

fn rd_vec3(b: &[u8], o: usize) -> Result<[f32; 3]> {
    rd_c3(b, o).map(|c| [c.x, c.y, c.z])
}

fn f32_at(b: &[u8], o: usize) -> Result<f32> {
    b.f32_at(o).ok_or(Error::Truncated)
}

/// The records of the array at header `at`, `size` bytes each, through `record(offset)`; the
/// whole array must lie in the file.
fn records<T>(
    b: &[u8],
    at: usize,
    size: usize,
    mut record: impl FnMut(usize) -> Result<T>,
) -> Result<Vec<T>> {
    let (count, ofs) = arr(b, at)?;
    get_n(b, ofs, count, size)?;
    (0..count).map(|i| record(ofs + i * size)).collect()
}

/// The `u16` array at header `at`.
fn u16s(b: &[u8], at: usize) -> Result<Vec<u16>> {
    records(b, at, 2, |o| u16_at(b, o))
}

/// Parse the version-264 model `bytes` with its skin profile 0, `skin`, and the [`anim_paths`]
/// files in `anims`; a sequence whose `.anim` is missing keeps no keys, which
/// [`M2Model::dropped`] names.
pub fn parse_m2(bytes: Vec<u8>, skin: &[u8], anims: AnimFiles) -> Result<M2Model> {
    let skin = Skin::parse(skin)?;
    let b = &bytes[..];
    let mut t = Tracks264::new(b, &anims)?;
    let global_flags = u32_at(b, h264::FLAGS)?;
    let global_sequences = records(b, h264::GLOBAL_SEQUENCES, 4, |o| u32_at(b, o))?;

    let bands = t.sequences();
    let sequences = records(b, h264::SEQUENCES, 0x40, |r| {
        Ok((
            u16_at(b, r)?,
            u16_at(b, r + 2)?,
            f32_at(b, r + 0x08)?,
            u32_at(b, r + 0x0c)?,
            u16_at(b, r + 0x10)?,
            (u32_at(b, r + 0x14)?, u32_at(b, r + 0x18)?),
            u32_at(b, r + 0x1c)?,
            (
                rd_vec3(b, r + 0x20)?,
                rd_vec3(b, r + 0x2c)?,
                f32_at(b, r + 0x38)?,
            ),
            (u16_at(b, r + 0x3c)? as i16, u16_at(b, r + 0x3e)?),
        ))
    })?
    .into_iter()
    .zip(&bands)
    .map(
        |(
            (id, sub, move_speed, flags, frequency, replay, blend_time, bounds, next),
            &(band, _),
        )| {
            M2Sequence {
                id,
                sub,
                start: band.0,
                end: band.1,
                move_speed,
                flags,
                frequency,
                min_replay: replay.0,
                max_replay: replay.1,
                blend_time,
                bounds_min: bounds.0,
                bounds_max: bounds.1,
                bounds_radius: bounds.2,
                variation_next: next.0,
                alias_next: next.1,
            }
        },
    )
    .collect();
    let animation_lookup = u16s(b, h264::SEQUENCE_LOOKUP)?;

    // Bones: the reference zeroes a NaN pivot component.
    let bones = records(b, h264::BONES, 0x58, |r| {
        let mut pivot = rd_c3(b, r + 0x4c)?;
        for c in [&mut pivot.x, &mut pivot.y, &mut pivot.z] {
            if c.is_nan() {
                *c = 0.0;
            }
        }
        Ok(M2Bone {
            key_bone: u32_at(b, r)? as i32 as i16,
            flags: M2BoneFlags(u32_at(b, r + 4)?),
            parent: u16_at(b, r + 8)? as i16,
            pivot,
            translation: t.vec3(r + 0x10, [0.0; 3])?,
            rotation: t.packed_quat(r + 0x24)?,
            scale: t.vec3(r + 0x38, [1.0; 3])?,
        })
    })?;

    let vertices = records(b, h264::VERTICES, 48, |r| {
        let v = get(b, r, 48)?;
        Ok(M2Vertex {
            position: rd_c3(v, 0)?,
            bone_weights: [v[12], v[13], v[14], v[15]],
            bone_indices: [v[16], v[17], v[18], v[19]],
            normal: rd_c3(v, 20)?,
            tex_coords: C2 {
                x: f32_at(v, 32)?,
                y: f32_at(v, 36)?,
            },
            tex_coords2: C2 {
                x: f32_at(v, 40)?,
                y: f32_at(v, 44)?,
            },
        })
    })?;

    // M2Color: an RGB track then a fixed16 alpha track.
    let colors = records(b, h264::COLORS, 2 * TRACK_264, |r| {
        Ok((t.vec3(r, [1.0; 3])?, t.fixed16(r + TRACK_264, 0x7fff)?))
    })?;
    let (color_rgb_tracks, color_alpha_tracks) = colors.into_iter().unzip();

    // Textures: type, flags (`+0x04`, the address mode: bit 0 repeats U, bit 1 V, clear clamps to
    // edge), filename. Cutout cards put UVs outside `0..1` to clamp to their transparent edge.
    let textures = records(b, h264::TEXTURES, 16, |r| {
        let flags = u32_at(b, r + 4)?;
        let (count, ofs) = arr(b, r + 8)?;
        let raw = b.bytes_at(ofs, count).unwrap_or(&[]);
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        Ok(M2Texture {
            texture_type: M2TextureType::from_u32(u32_at(b, r)?),
            wrap_x: flags & 0x1 != 0,
            wrap_y: flags & 0x2 != 0,
            filename: M2ArrayString {
                string: CString::new(&raw[..end]).unwrap_or_default(),
            },
        })
    })?;
    let transparency_tracks = records(b, h264::TEXTURE_WEIGHTS, TRACK_264, |r| {
        t.fixed16(r, 0x7fff)
    })?;
    let texture_transforms = records(b, h264::TEXTURE_TRANSFORMS, 3 * TRACK_264, |r| {
        Ok(M2TextureTransform {
            translation: t.vec3(r, [0.0; 3])?,
            rotation: t.quat(r + TRACK_264)?,
            scaling: t.vec3(r + 2 * TRACK_264, [1.0; 3])?,
        })
    })?;
    let materials = records(b, h264::MATERIALS, 4, |r| {
        Ok(M2Material {
            flags: M2RenderFlags(u16_at(b, r)?),
            blend_mode: M2BlendMode(u16_at(b, r + 2)?),
        })
    })?;
    let texture_lookup_table = u16s(b, h264::TEXTURE_LOOKUP)?;
    let texture_unit_lookup = u16s(b, h264::TEXTURE_UNIT_LOOKUP)?;
    let transparency_lookup = u16s(b, h264::TEXTURE_WEIGHT_LOOKUP)?;
    let texture_transform_lookup = u16s(b, h264::TEXTURE_TRANSFORM_LOOKUP)?;

    // Authored bounds: AABB (min C3, max C3) then sphere radius, then the collision hull's.
    let o = h264::BOUNDS;
    let header = M2Header {
        bounding_box_min: rd_vec3(b, o)?,
        bounding_box_max: rd_vec3(b, o + 12)?,
        bounding_sphere_radius: f32_at(b, o + 24)?,
        collision_box_min: rd_vec3(b, o + 28)?,
        collision_box_max: rd_vec3(b, o + 40)?,
        collision_sphere_radius: f32_at(b, o + 52)?,
    };
    let (n, ofs) = arr(b, h264::COLLISION_TRIANGLES)?;
    let bounding_triangles = get_n(b, ofs, n, 2)?.to_vec();
    let (n, ofs) = arr(b, h264::COLLISION_VERTICES)?;
    let bounding_vertices = get_n(b, ofs, n, 12)?.to_vec();

    // Attachments: id `u32`, bone `u16`, position `+0x08`. A record whose id overflows `u16`, or
    // whose bone is out of range, is dropped; real ids stop at 36.
    let raw_attachments = records(b, h264::ATTACHMENTS, 0x28, |r| {
        Ok((u32_at(b, r)?, u16_at(b, r + 4)?, rd_vec3(b, r + 8)?))
    })?;
    let mut attachments = Vec::new();
    // AttachLookup indexes the file's records; a dropped one shifts ours, so map file to emitted.
    let emitted_at: Vec<u16> = raw_attachments
        .iter()
        .map(|&(id, bone, position)| match u16::try_from(id) {
            Ok(id) if usize::from(bone) < bones.len() => {
                attachments.push(M2Attachment { id, bone, position });
                u16::try_from(attachments.len() - 1).unwrap_or(0xffff)
            }
            _ => 0xffff,
        })
        .collect();
    let raw_lookup = u16s(b, h264::ATTACHMENT_LOOKUP)?;
    // The follow camera's pivot height: attachment id 17's file record, through the lookup.
    let pivot_attach_z = raw_lookup
        .get(17)
        .and_then(|&i| raw_attachments.get(usize::from(i)))
        .map(|a| a.2[2]);
    // AttachLookup: attachment id to its one record, the reference's only id map. Shipped weapons
    // repeat ids (`Stave_2H_Long_D_05`: 0-3 twice) and the lookup names one.
    let attach_lookup = raw_lookup
        .iter()
        .map(|&i| emitted_at.get(usize::from(i)).copied().unwrap_or(0xffff))
        .collect();

    // Events: 4CC, data, bone `u16`, position `+0x0c`, an `M2TrackBase` at `+0x18`.
    let events: Vec<M2Event> = records(b, h264::EVENTS, 0x24, |r| {
        let (ranges, times) = t.times(r + 0x18)?;
        Ok(M2Event {
            ident: get(b, r, 4)?.try_into().map_err(|_| Error::Truncated)?,
            data: u32_at(b, r + 4)?,
            bone: u16_at(b, r + 8)?,
            position: rd_vec3(b, r + 0x0c)?,
            gseq: u16_at(b, r + 0x18 + 2)?,
            ranges,
            times,
        })
    })?;
    let event_markers = events
        .iter()
        .filter(|e| usize::from(e.bone) < bones.len())
        .map(|e| M2EventMarker {
            ident: e.ident,
            bone: e.bone,
            position: e.position,
        })
        .collect();

    let lights = records(b, h264::LIGHTS, 0x10 + 7 * TRACK_264, |r| {
        let at = |i: usize| r + 0x10 + i * TRACK_264;
        Ok(M2LightDef {
            light_type: u16_at(b, r)?,
            bone: u16_at(b, r + 2)? as i16,
            position: rd_vec3(b, r + 4)?,
            ambient_color: t.vec3(at(0), [0.0; 3])?,
            ambient_intensity: t.float(at(1), 0.0)?,
            diffuse_color: t.vec3(at(2), [0.0; 3])?,
            diffuse_intensity: t.float(at(3), 0.0)?,
            attenuation_start: t.float(at(4), 0.0)?,
            attenuation_end: t.float(at(5), 0.0)?,
            visibility: t.byte(at(6), 0)?,
        })
    })?;

    let cameras = records(b, h264::CAMERAS, 0x64, |r| {
        Ok(M2Camera {
            camera_type: u32_at(b, r)? as i32,
            fov: f32_at(b, r + 0x04)?,
            far_clip: f32_at(b, r + 0x08)?,
            near_clip: f32_at(b, r + 0x0c)?,
            positions: t.spline_vec3(r + 0x10)?,
            position_base: rd_vec3(b, r + 0x24)?,
            target: t.spline_vec3(r + 0x30)?,
            target_base: rd_vec3(b, r + 0x44)?,
            roll: t.spline_f32(r + 0x50)?,
        })
    })?;
    let camera_lookup = u16s(b, h264::CAMERA_LOOKUP)?;

    // The texture combiner ops, present under flag `0x8` alone.
    let texture_combiner_combos = if global_flags & 8 != 0 {
        u16s(b, h264::TEXTURE_COMBINER_COMBOS)?
    } else {
        Vec::new()
    };
    let dropped = t.dropped();

    Ok(M2Model {
        sequences,
        vertices,
        textures,
        materials,
        color_alpha_tracks,
        color_rgb_tracks,
        transparency_tracks,
        transparency_lookup,
        texture_unit_lookup,
        texture_transforms,
        texture_transform_lookup,
        global_sequences,
        bones,
        cameras,
        camera_lookup,
        lights,
        raw_data: M2RawData {
            texture_lookup_table,
            bounding_triangles,
            bounding_vertices,
        },
        header,
        pivot_attach_z,
        attachments,
        attach_lookup,
        events,
        event_markers,
        animation_lookup,
        global_flags,
        texture_combiner_combos,
        skin,
        dropped,
        bytes,
        anims,
    })
}

#[cfg(test)]
mod tests;
