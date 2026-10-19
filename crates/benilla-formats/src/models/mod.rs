//! Model loading over `benilla-m2` and `benilla-wmo`: a model becomes one [`RenderSubmesh`] per
//! render batch, each with its own vertices, texture and blend, in model space (Z up, WoW axes).

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::Chain;

mod anim;
mod anim_summary;
mod art_extent;
mod bounds;
mod collision;
mod key_anim;
mod m2_batches;
mod m2_combiner;
mod mat_anim;
mod records;
mod tex_anim;
mod types;
mod wmo;
pub use anim::*;
pub use anim_summary::*;
pub use art_extent::*;
pub use bounds::*;
pub use collision::*;
pub(crate) use key_anim::{bake_track, SeqSlot};
pub use key_anim::{KeyAnim, SeqLoops};
pub use m2_batches::*;
pub use mat_anim::{AlphaAnim, AlphaSeq, RgbAnim, ScalarAnim};
pub use records::*;
pub use tex_anim::{rotation_2x2, uv_transform, UvAnim, UvRotAnim};
pub use types::*;
pub use wmo::*;

pub use benilla_m2::{
    anim_paths, md20_version, relayout_264, skin_path, AnimFiles, Relayout, VERSION_264,
};

/// The M2 at `path` in the 1.12.1 layout every reader here takes: a version-264 model is written
/// in it with its skin profile 0 and its `.anim` files ([`relayout_264`]), whose
/// [`Relayout::dropped`] comes back beside.
pub fn read_m2(chain: &Chain, path: &str) -> Result<(Vec<u8>, Vec<String>)> {
    let bytes = chain.read(path)?;
    if md20_version(&bytes) != Some(VERSION_264) {
        return Ok((bytes, Vec::new()));
    }
    let skin = chain.read(&skin_path(path))?;
    let to_err = |e| anyhow::anyhow!("{path}: {e}");
    let anims: AnimFiles = anim_paths(&bytes, path)
        .map_err(to_err)?
        .into_iter()
        .filter_map(|(seq, file)| Some((seq, chain.read(&file).ok()?)))
        .collect();
    let out = relayout_264(&bytes, &skin, &anims).map_err(to_err)?;
    Ok((out.bytes, out.dropped))
}

/// The 3.3.5a model at `raw_path` (`.mdx`/`.mdl` read as `.m2`) with its chain path, read
/// through [`read_m2`] so `parse_m2` can take it: the version-264 file with its skin profile 0
/// and `.anim` files. A sequence whose `.anim` does not read is left without keys.
fn read_model(chain: &Chain, raw_path: &str) -> Result<(String, Vec<u8>)> {
    let path = model_path(raw_path);
    let (bytes, _dropped) = read_m2(chain, &path).with_context(|| format!("reading M2 {path}"))?;
    Ok((path, bytes))
}

/// Build a compact submesh from global vertex indices. Also returns each local vertex's global
/// index, from which the M2 path fills [`RenderSubmesh::joints`].
fn remap_submesh(
    global_indices: impl Iterator<Item = u32>,
    vertex: impl Fn(u32) -> ([f32; 3], [f32; 3], [f32; 2], [f32; 4]),
    texture: Option<String>,
    blend: ModelBlend,
    two_sided: bool,
    interior: bool,
    emissive: bool,
) -> (RenderSubmesh, Vec<u32>) {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let (mut positions, mut normals, mut uvs, mut indices, mut vertex_colors) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut globals: Vec<u32> = Vec::new();
    for g in global_indices {
        let local = *map.entry(g).or_insert_with(|| {
            let (p, n, uv, c) = vertex(g);
            positions.push(p);
            normals.push(n);
            uvs.push(uv);
            vertex_colors.push(c);
            globals.push(g);
            (positions.len() - 1) as u32
        });
        indices.push(local);
    }
    (
        RenderSubmesh {
            positions,
            normals,
            uvs,
            indices,
            texture,
            // The per-batch fields start at their defaults; the M2 and WMO paths set their own.
            skin_slot: None,
            geoset_id: 0,
            char_slot: None,
            icon_slot: false,
            blend,
            // WMO keeps repeat; the M2 batch loop sets both from the texture record's flags.
            wrap_x: true,
            wrap_y: true,
            two_sided,
            vertex_colors,
            joints: Vec::new(),
            weights: Vec::new(),
            interior,
            emissive,
            sidn: None,
            window: false,
            wmo_unified: None,
            additive: false,
            no_depth_write: false,
            no_depth_test: false,
            fog_policy: FogPolicy::Scene,
            billboard: None,
            welded_billboard: false,
            alpha_anim: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            rgb_anim: None,
            rgb_seq: None,
            wmo_batch: None,
            env_map: false,
            section: None,
            combiner: None,
            uvs2: Vec::new(),
            priority_plane: 0,
        },
        globals,
    )
}

/// Normalize a model path for the chain: lowercase, with `.mdx`/`.mdl` as `.m2`.
pub(crate) fn model_path(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    match lower
        .strip_suffix(".mdx")
        .or_else(|| lower.strip_suffix(".mdl"))
    {
        Some(stem) => format!("{stem}.m2"),
        None => lower,
    }
}

/// Panicking little-endian reads at a byte offset, for the raw walks not on
/// `benilla_bytes::ByteExt`.
pub(crate) fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub(crate) fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Load a GameObject display model by path: a `.wmo` through [`load_wmo`], anything else as an M2.
pub fn load_object_model(chain: &mut Chain, raw_path: &str) -> Result<Vec<RenderSubmesh>> {
    if raw_path.to_ascii_lowercase().ends_with(".wmo") {
        load_wmo(chain, raw_path)
    } else {
        load_m2_mesh(chain, raw_path)
    }
}
