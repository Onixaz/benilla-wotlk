//! Collision geometry: the M2 collision hull and the WMO group gathers for walking and the camera.

use std::collections::HashMap;
use std::io::Cursor;

use anyhow::{Context, Result};
use benilla_m2::M2Model;
use benilla_wmo::{parse_wmo, ParsedWmo};

use crate::Chain;

use super::read_model;

/// A collision triangle list in raw model-local coordinates, the space of
/// [`super::RenderSubmesh::positions`]; empty when the source has no collision geometry.
#[derive(Debug, Clone, Default)]
pub struct CollisionMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl CollisionMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// An M2 doodad's collision hull, the header's `bounding_vertices` (`C3Vector`) and
/// `bounding_triangles` (`u16` indices): a coarse solid far smaller than the render mesh, so the
/// trunk blocks and the canopy does not. A doodad with no hull does not collide.
pub fn load_m2_collision_hull(chain: &mut Chain, raw_path: &str) -> Result<CollisionMesh> {
    let (_, model) = read_model(chain, raw_path)?;
    Ok(parse_m2_collision_hull(&model))
}

/// [`load_m2_collision_hull`] from a model already in hand; empty when there is no hull.
pub fn parse_m2_collision_hull(model: &M2Model) -> CollisionMesh {
    let rd = &model.raw_data;
    let positions: Vec<[f32; 3]> = rd
        .bounding_vertices
        .as_chunks::<12>()
        .0
        .iter()
        .map(|c| {
            [
                f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                f32::from_le_bytes([c[8], c[9], c[10], c[11]]),
            ]
        })
        .collect();
    let n = positions.len() as u32;
    let indices: Vec<u32> = rd
        .bounding_triangles
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
        .collect();
    // A hull with a partial triangle or an index past its vertices counts as no hull.
    if indices.is_empty() || !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i >= n) {
        return CollisionMesh::default();
    }
    CollisionMesh { positions, indices }
}

/// MOPY `DETAIL`, the face bit the walking gather skips. The reference builds a reject mask from a
/// per-query class word: walking, `0x10_0111`, gives `0x84`, tested at the box-query leaf; camera
/// and line of sight, `0x10_0171`, give `0x82`, tested at the segment-query leaf. The same code
/// builds both masks, and `0x80` is the query's transient visited bit, so walking skips `DETAIL`
/// and the camera `NOCAMCOLLIDE`.
const MOPY_DETAIL: u8 = 0x04;
/// MOPY `NOCAMCOLLIDE`, the face bit the camera and line-of-sight gather skips.
const MOPY_NOCAMCOLLIDE: u8 = 0x02;
/// The face bits the mouse pick's world trace skips. The WMO ignore-mask builder (`0x7ae140`)
/// turns the trace's class word `0x100_0124` into the ignore mask `0x188`; the segment query's
/// face test (`0x7c6c30`) checks its low byte against the face's MOPY flag byte: MOPY `0x08`
/// (the collision-only faces, such as the blockers along a bank counter) and `0x80`, which the
/// query also ORs in as its visited bit. `DETAIL` is not in it, so a detail face occludes the pick.
const MOPY_PICK_SKIP: u8 = 0x88;

/// A WMO's walking-collidable triangles across every group file, in raw WMO-local coordinates: a
/// face collides unless it is `DETAIL` ([`MOPY_DETAIL`]). Wider than the vmangos vmap rule
/// `COLLISION || (RENDER && !DETAIL)`: faces with flags `0x00` or only `0x40` collide too. Every
/// group is gathered, with no whole-group skip by MOGP flags.
pub fn load_wmo_collision_tris(chain: &mut Chain, raw_path: &str) -> Result<CollisionMesh> {
    let root_path = raw_path.to_ascii_lowercase();
    let bytes = chain
        .read_file(&root_path)
        .with_context(|| format!("reading WMO {root_path}"))?;
    let ParsedWmo::Root(root) = parse_wmo(&mut Cursor::new(&bytes))
        .map_err(|e| anyhow::anyhow!("parsing {root_path}: {e}"))?
    else {
        anyhow::bail!("{root_path} is not a WMO root file");
    };
    let stem = root_path.strip_suffix(".wmo").unwrap_or(&root_path);
    let (mut positions, mut indices): (Vec<[f32; 3]>, Vec<u32>) = (Vec::new(), Vec::new());
    for gi in 0..root.n_groups {
        let group_path = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read_file(&group_path) else {
            continue;
        };
        accumulate_wmo_group_collision(&gbytes, &mut positions, &mut indices);
    }
    Ok(CollisionMesh { positions, indices })
}

/// Append one WMO group file's walking-collidable triangles: every face but `DETAIL`.
pub fn accumulate_wmo_group_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(group_bytes, MOPY_DETAIL, 0, positions, indices);
}

/// Append one WMO group file's camera and line-of-sight triangles: every face but `NOCAMCOLLIDE`,
/// so a `DETAIL` overhang the player walks under (forge pipes) still stops the camera.
pub fn accumulate_wmo_group_camera_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(group_bytes, MOPY_NOCAMCOLLIDE, 0, positions, indices);
}

/// Append one WMO group file's camera-only triangles (`DETAIL` set, `NOCAMCOLLIDE` clear): the
/// camera set minus the walking set, for the down-ray's fallback once the walking leg misses.
pub fn accumulate_wmo_group_camera_only_collision(
    group_bytes: &[u8],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces(
        group_bytes,
        MOPY_NOCAMCOLLIDE,
        MOPY_DETAIL,
        positions,
        indices,
    );
}

/// Append one WMO group file's faces past MOPY `0x88` ([`MOPY_PICK_SKIP`]) in one walk, split by
/// material class: the opaque ones are the mouse pick's occluders, the blended ones go apart. The
/// pick trace's segment test (`0x7c6c30`) classes a face opaque (`0x200`) when its material id is
/// `0xFF` or its MOMT `blendMode` is 0, else blended (`0x100`), and its mask `0x188` carries only
/// `0x100`. The ground-decal gather's box and frustum tests (`0x7c9b10`, `0x7c7660`) read the flag
/// byte alone under the mask `0x88`, so a decal drapes both halves. `blended` is
/// [`super::WmoRoot::material_blended`], by MOPY material id; an id past it counts as opaque.
pub fn accumulate_wmo_group_pick_collision(
    group_bytes: &[u8],
    blended: &[bool],
    (positions, indices): (&mut Vec<[f32; 3]>, &mut Vec<u32>),
    (blended_positions, blended_indices): (&mut Vec<[f32; 3]>, &mut Vec<u32>),
) {
    accumulate_wmo_group_faces_routed(
        group_bytes,
        |flags, material| {
            (flags & MOPY_PICK_SKIP == 0).then(|| {
                let is_blended = material != 0xFF
                    && blended.get(usize::from(material)).copied().unwrap_or(false);
                usize::from(is_blended)
            })
        },
        [(positions, indices), (blended_positions, blended_indices)],
    );
}

/// Append every triangle whose MOPY flags miss `skip_mask` and carry all of `require_mask`, its
/// vertices remapped per group (the same index in two groups is two vertices).
fn accumulate_wmo_group_faces(
    group_bytes: &[u8],
    skip_mask: u8,
    require_mask: u8,
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces_where(
        group_bytes,
        |flags, _| flags & skip_mask == 0 && flags & require_mask == require_mask,
        positions,
        indices,
    );
}

/// Append every triangle `keep(flags, material_id)` accepts, its vertices remapped per group.
fn accumulate_wmo_group_faces_where(
    group_bytes: &[u8],
    keep: impl Fn(u8, u8) -> bool,
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    accumulate_wmo_group_faces_routed(
        group_bytes,
        |flags, material| keep(flags, material).then_some(0),
        [(positions, indices)],
    );
}

/// Append each triangle to the output `route(flags, material_id)` names (`None` drops it), in one
/// parse of the group, its vertices remapped per group and per output.
fn accumulate_wmo_group_faces_routed<const N: usize>(
    group_bytes: &[u8],
    route: impl Fn(u8, u8) -> Option<usize>,
    mut outs: [(&mut Vec<[f32; 3]>, &mut Vec<u32>); N],
) {
    let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(group_bytes)) else {
        return;
    };
    let n_tri = group.vertex_indices.len() / 3;
    let mut maps: [HashMap<u16, u32>; N] = std::array::from_fn(|_| HashMap::new());
    for t in 0..n_tri {
        let Some(mopy) = group.material_info.get(t) else {
            continue;
        };
        let Some(o) = route(mopy.flags, mopy.material_id) else {
            continue;
        };
        let ((positions, indices), map) = (&mut outs[o], &mut maps[o]);
        for k in 0..3 {
            let Some(&vidx) = group.vertex_indices.get(t * 3 + k) else {
                continue;
            };
            let Some(p) = group.vertex_positions.get(vidx as usize) else {
                continue;
            };
            let global = *map.entry(vidx).or_insert_with(|| {
                positions.push([p.x, p.y, p.z]);
                (positions.len() - 1) as u32
            });
            indices.push(global);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(reversed_magic: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = reversed_magic.to_vec();
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    /// A group file with one triangle per `(flags, material)` face, each over its own vertices.
    fn group(faces: &[(u8, u8)]) -> Vec<u8> {
        let mut mogp = vec![0u8; 0x44];
        let (mut vert, mut idx, mut mopy) = (Vec::new(), Vec::new(), Vec::new());
        for (i, &(flags, material)) in faces.iter().enumerate() {
            for k in 0..3u16 {
                let x = i as f32 * 10.0 + f32::from(k);
                vert.extend([x, 0.0, f32::from(k)].map(f32::to_le_bytes).concat());
                idx.extend((i as u16 * 3 + k).to_le_bytes());
            }
            mopy.extend([flags, material]);
        }
        mogp.extend(chunk(b"YPOM", &mopy));
        mogp.extend(chunk(b"TVOM", &vert));
        mogp.extend(chunk(b"IVOM", &idx));
        chunk(b"PGOM", &mogp)
    }

    /// The (pick occluder, blended decal-only) triangle counts for `faces`.
    fn pick_split(faces: &[(u8, u8)], blended: &[bool]) -> (usize, usize) {
        let (mut pos, mut idx) = (Vec::new(), Vec::new());
        let (mut bpos, mut bidx) = (Vec::new(), Vec::new());
        accumulate_wmo_group_pick_collision(
            &group(faces),
            blended,
            (&mut pos, &mut idx),
            (&mut bpos, &mut bidx),
        );
        (idx.len() / 3, bidx.len() / 3)
    }

    fn pick_faces(faces: &[(u8, u8)], blended: &[bool]) -> usize {
        pick_split(faces, blended).0
    }

    /// The pick passes Dalaran's counter blockers (MOPY `0x48`, no material) and a blended pane,
    /// and stops on an opaque render face and on a DETAIL face, which the walking gather drops.
    #[test]
    fn the_pick_set_drops_collision_only_and_blended_faces_and_keeps_detail() {
        let blended = [false, true];
        assert_eq!(
            pick_faces(&[(0x48, 0xFF)], &blended),
            0,
            "collision-only blocker"
        );
        assert_eq!(pick_faces(&[(0x80, 0)], &blended), 0, "MOPY 0x80");
        assert_eq!(pick_faces(&[(0x20, 1)], &blended), 0, "blended material");
        assert_eq!(pick_faces(&[(0x20, 0)], &blended), 1, "opaque render face");
        assert_eq!(
            pick_faces(&[(0x24, 0)], &blended),
            1,
            "DETAIL occludes the pick"
        );
        assert_eq!(
            pick_faces(&[(0x20, 0xFF)], &blended),
            1,
            "no material is opaque"
        );
        assert_eq!(
            pick_faces(&[(0x20, 7)], &blended),
            1,
            "an id past MOMT is opaque"
        );

        // The decal half: a blended face past `0x88` goes apart, a collision-only one nowhere.
        assert_eq!(pick_split(&[(0x20, 1)], &blended), (0, 1), "blended");
        assert_eq!(pick_split(&[(0x24, 1)], &blended), (0, 1), "blended DETAIL");
        assert_eq!(
            pick_split(&[(0x48, 1)], &blended),
            (0, 0),
            "blended blocker"
        );
        assert_eq!(
            pick_split(&[(0x20, 0), (0x20, 1), (0x24, 0)], &blended),
            (2, 1),
            "one walk splits a mixed group"
        );

        let (mut pos, mut idx) = (Vec::new(), Vec::new());
        accumulate_wmo_group_collision(&group(&[(0x24, 0), (0x48, 0xFF)]), &mut pos, &mut idx);
        assert_eq!(
            idx.len() / 3,
            1,
            "the walking gather keeps the blocker, drops DETAIL"
        );
    }
}
