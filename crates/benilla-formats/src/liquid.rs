//! ADT liquid to per-chunk liquid meshes, in raw WoW coords (+X north, +Y west, +Z up).
//!
//! 12340 makes an MCNK's liquids from two sources (`0x7c5690`): one per MCLQ block, each a 9×9
//! grid over the whole chunk of `LiquidType` header bit + 1, and one per MH2O instance, each a
//! tile rectangle of its own type and vertex format. Both reach the same [`LiquidMesh`]: a vertex
//! grid on the terrain's lattice and two triangles per wet tile.

use benilla_adt::{MclqChunk, Mh2oInstance};

use crate::liquid_type::{LiquidTypes, VertexFormat};
use crate::terrain::{snap_to_lattice, UNIT_SIZE};

/// A liquid's animated texture set and render path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiquidKind {
    /// Still water, `XTextures\river\lake_a.*` (30 frames): lakes, ponds, slow rivers, WMO canals.
    Still,
    /// Rapids, `XTextures\river\fast_a.*` (16 frames), nibble 8 in the name table `0x86a000`. No
    /// shipped MLIQ cell carries 8, and the ADT path cannot reach it.
    Rapids,
    /// Ocean, `XTextures\ocean\ocean_h.*` (30 frames).
    Ocean,
    /// Magma, `XTextures\lava\lava.*`: opaque and unlit, but fogged like every liquid batch, since
    /// no liquid setup overrides the device's fog-on default (`0x593d18`). Its ADT UVs are
    /// authored per vertex under vertex format 1.
    Magma,
    /// Slime, `XTextures\slime\slime.*`: magma's render path with its own texture (one handler,
    /// `0x6b68f0`).
    Slime,
}

impl LiquidKind {
    /// The kind a WMO tile nibble selects: it indexes the animated-texture name table `0x86a000`
    /// directly, with 4, 6 and 7 as variants of 0, 2 and 3.
    pub fn from_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 0xf {
            0 | 4 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 | 6 => Some(LiquidKind::Magma),
            3 | 7 => Some(LiquidKind::Slime),
            8 => Some(LiquidKind::Rapids),
            _ => None, // 5, 9..=0xf: empty name-table slots and the 0xf hole
        }
    }

    /// Whether this kind is opaque and unlit, its texture the body colour (magma, slime); water and
    /// ocean take the `ocean0_s.bls` depth-swatch path. Unlit is not unfogged (`0x593d18`).
    pub fn is_fullbright(self) -> bool {
        matches!(self, LiquidKind::Magma | LiquidKind::Slime)
    }
}

/// One liquid surface, an MCLQ block, an MH2O instance or a WMO group's MLIQ: a regular vertex
/// grid in raw WoW coords and the triangles over its wet cells. Queries read the grid, as the
/// reference samples liquid height bilinearly over the containing cell's corners (`0x7ce0b0`).
/// Drawn two-sided, as every reference liquid pass turns culling off (`0x6851da`, `0x6b6302`).
#[derive(Debug, Clone)]
pub struct LiquidMesh {
    /// Vertex counts `[cols, rows]`, cols fastest: `[9, 9]` for MCLQ, `[width+1, height+1]` for
    /// MH2O, the header's `xverts`/`yverts` for MLIQ. Every per-vertex array is this grid,
    /// row-major.
    pub grid: [u32; 2],
    /// Per-cell coverage over `(cols−1)·(rows−1)`: the cell exists and its corners are real. The
    /// containment truth, since the grid's bounds span dry cells.
    pub wet: Vec<bool>,
    /// Per cell, the MLIQ tile flag's `0x80`: another group claims the cell too. The reference
    /// draws it only for the group winning a per-frame 2-colouring of the portal graph, so the
    /// translucent overlap draws once; applied in `benilla_assets::wmo`, `false` on the ADT path.
    pub shared: Vec<bool>,
    /// Vertex positions in WoW yards, `cols·rows` of them.
    pub positions: Vec<[f32; 3]>,
    /// Texture UVs: generated at ¼ per cell, world-continuous (WMO liquid in model space, same
    /// period); authored per vertex under vertex format 1 (`u = s · 3/256`, `0x7ce390`).
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex swatch coord V (0..1) from the depth byte through the type's depth table
    /// (`0x79e3c0`). The reference indexes the depth swatch with this one V for both the body
    /// colour and the opacity ramp. 0 where the type reads no depth.
    pub depths: Vec<f32>,
    /// Triangle indices, 6 per wet cell in row-major order, derived from [`Self::wet`].
    pub indices: Vec<u32>,
    /// The `LiquidType.dbc` id: the MH2O instance's, MCLQ's header bit + 1 (`0x7c5690`), or the
    /// WMO group's as `0x7d7310` resolves it. Its row picks the ambient loop (`0x4d2390`).
    pub liquid_type: u32,
    /// WMO only: the MLIQ header's `materialId`, an index into the root's MOMT, whose `diffColor`
    /// is the reference's interior water body colour. `None` on the ADT path.
    pub material_id: Option<u16>,
    pub kind: LiquidKind,
}

/// 8×8 tiles per MCNK, 9×9 MCLQ vertices.
const CELLS: usize = 8;
/// Cell-flag low nibble meaning "dry / do not render".
const DRY_NIBBLE: u8 = 0x0f;
/// The reference's scale on a vertex's authored `u16` texcoords, `3/256` (`0x7ce390`).
const TEXCOORD_SCALE: f32 = 0.011_718_75;
/// Verts under no liquid carry FLT_MAX (`0x7F7FFFFF`), which is finite, so gate on magnitude.
const HEIGHT_SENTINEL: f32 = 1.0e9;
/// The map whose third MCLQ header bit is `LiquidType` 15 rather than 3 (`0x7c5764`): Outland.
const OUTLAND_MAP: u32 = 530;
/// Outland's MCLQ magma type (`0x7c5769`).
const OUTLAND_MCLQ_MAGMA: u32 = 15;

/// The `LiquidType` id 12340 gives the MCLQ block in header `slot` 0–3 on `map_id` (`0x7c5690`).
pub fn mclq_liquid_type(slot: u8, map_id: u32) -> u32 {
    if map_id == OUTLAND_MAP && slot == 2 {
        OUTLAND_MCLQ_MAGMA
    } else {
        u32::from(slot) + 1
    }
}

/// One liquid's vertex grid as the builder reads it: `cols`×`rows` vertices whose first sits
/// `(row0, col0)` cells into the chunk, and per-vertex accessors.
struct Grid<'a> {
    cols: usize,
    rows: usize,
    row0: usize,
    col0: usize,
    /// Per cell, `(cols−1)·(rows−1)`, before the corner check.
    exists: Vec<bool>,
    height: &'a dyn Fn(usize) -> f32,
    depth: &'a dyn Fn(usize) -> u8,
    texcoord: &'a dyn Fn(usize) -> [u16; 2],
    format: VertexFormat,
    /// The fill for a vertex whose height is the dry sentinel.
    min_height: f32,
}

/// Mesh one ADT liquid at the MCNK header's raw WoW `position`; `None` when no cell is wet or the
/// type has no row.
fn build_grid_mesh(
    grid: Grid<'_>,
    position: [f32; 3],
    liquid_type: u32,
    types: &LiquidTypes,
) -> Option<LiquidMesh> {
    let kind = types.kind(liquid_type)?;
    let ramp = types.get(liquid_type)?.depth_ramp;
    let (cols, rows) = (grid.cols, grid.rows);
    let real = |i: usize| {
        let h = (grid.height)(i);
        h.is_finite() && h.abs() < HEIGHT_SENTINEL
    };

    let mut indices = Vec::with_capacity((cols - 1) * (rows - 1) * 6);
    let mut wet = vec![false; (cols - 1) * (rows - 1)];
    for row in 0..rows - 1 {
        for col in 0..cols - 1 {
            if !grid.exists[row * (cols - 1) + col] {
                continue;
            }
            let tl = (row * cols + col) as u32;
            let tr = tl + 1;
            let bl = ((row + 1) * cols + col) as u32;
            let br = bl + 1;
            // A wet cell with a sentinel or NaN corner is a data anomaly; skip it.
            if [tl, tr, bl, br].iter().any(|&i| !real(i as usize)) {
                continue;
            }
            // One gate for `wet` and the triangles, so the queries and the drawing agree.
            wet[row * (cols - 1) + col] = true;
            indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
        }
    }
    if indices.is_empty() {
        return None;
    }

    // Rows step south (−X) and columns east (−Y) like the MCVT outer grid, on the terrain's
    // lattice (`0x7cdf80`); Z is the absolute liquid height.
    let [wx, wy, _wz] = position;
    let authored_uvs = grid.format == VertexFormat::HeightUv;
    let mut positions = Vec::with_capacity(cols * rows);
    let mut uvs = Vec::with_capacity(cols * rows);
    let mut depths = Vec::with_capacity(cols * rows);
    for n in 0..cols * rows {
        let row = (grid.row0 + n / cols) as f32;
        let col = (grid.col0 + n % cols) as f32;
        // A dry vert's sentinel would stretch the AABB to ~1e38 and get a partly wet chunk culled.
        let h = if real(n) {
            (grid.height)(n)
        } else {
            grid.min_height
        };
        positions.push([
            snap_to_lattice(wx - row * UNIT_SIZE),
            snap_to_lattice(wy - col * UNIT_SIZE),
            h,
        ]);
        uvs.push(if authored_uvs {
            // Unsigned: the reference widens the `u16`, so 0xffff is 65535, never −1.
            let [s, t] = (grid.texcoord)(n);
            [f32::from(s) * TEXCOORD_SCALE, f32::from(t) * TEXCOORD_SCALE]
        } else {
            [col * 0.25, row * 0.25]
        });
        depths.push(ramp.map_or(0.0, |r| r.v((grid.depth)(n))));
    }

    Some(LiquidMesh {
        grid: [cols as u32, rows as u32],
        shared: vec![false; wet.len()],
        wet,
        positions,
        uvs,
        depths,
        indices,
        liquid_type,
        material_id: None, // ADT liquid has no MOMT to index
        kind,
    })
}

/// Mesh one MCLQ block of `LiquidType` `liquid_type` ([`mclq_liquid_type`]). A cell is wet where
/// its nibble is not dry and shares the type's class (`0x7ce180`); the vertex bytes follow the
/// type's material format (`0x8a3200`–`0x8a3250`).
pub(crate) fn build_mclq_mesh(
    mclq: &MclqChunk,
    position: [f32; 3],
    liquid_type: u32,
    types: &LiquidTypes,
) -> Option<LiquidMesh> {
    if mclq.vertices.len() < (CELLS + 1) * (CELLS + 1) {
        return None;
    }
    let format = types.get(liquid_type)?.vertex_format?;
    let class = liquid_type.wrapping_sub(1) & 3;
    let exists = mclq
        .tile_flags
        .iter()
        .map(|&f| {
            let nibble = f & 0xf;
            nibble != DRY_NIBBLE && (u32::from(nibble) ^ class) & 3 == 0
        })
        .collect();
    let height = |i: usize| {
        if format.has_height() {
            mclq.vertices[i].height
        } else {
            0.0
        }
    };
    let depth = |i: usize| match format {
        VertexFormat::HeightDepth | VertexFormat::Depth => mclq.vertices[i].depth_byte(),
        _ => 0,
    };
    let texcoord = |i: usize| match format {
        VertexFormat::HeightUv => mclq.vertices[i].texcoords(),
        _ => [0, 0],
    };
    build_grid_mesh(
        Grid {
            cols: CELLS + 1,
            rows: CELLS + 1,
            row0: 0,
            col0: 0,
            exists,
            height: &height,
            depth: &depth,
            texcoord: &texcoord,
            format,
            min_height: mclq.min_height,
        },
        position,
        liquid_type,
        types,
    )
}

/// Mesh one MH2O instance: its tile rectangle, exists bitmap and its own vertex format
/// (`0x7c5690`, `0x8a3130`–`0x8a31b0`).
pub(crate) fn build_mh2o_mesh(
    inst: &Mh2oInstance,
    position: [f32; 3],
    types: &LiquidTypes,
) -> Option<LiquidMesh> {
    let (w, h) = (usize::from(inst.width), usize::from(inst.height));
    let (x0, y0) = (usize::from(inst.x_offset), usize::from(inst.y_offset));
    if w == 0 || h == 0 || x0 + w > CELLS || y0 + h > CELLS || inst.exists.len() < w * h {
        return None;
    }
    let height = |i: usize| inst.height_at(i);
    let depth = |i: usize| inst.depth_at(i);
    let texcoord = |i: usize| inst.texcoord_at(i);
    build_grid_mesh(
        Grid {
            cols: w + 1,
            rows: h + 1,
            row0: y0,
            col0: x0,
            exists: inst.exists.clone(),
            height: &height,
            depth: &depth,
            texcoord: &texcoord,
            format: VertexFormat::from_raw(u32::from(inst.vertex_format)),
            min_height: inst.min_height,
        },
        position,
        u32::from(inst.liquid_type),
        types,
    )
}

impl LiquidMesh {
    /// Drop the cells `keep` rejects (indexed like [`Self::wet`]) and rebuild [`Self::indices`];
    /// the vertex grid stays whole. Applies [`Self::shared`] once the model has every group.
    pub fn retain_cells(&mut self, keep: impl Fn(usize) -> bool) {
        let (cols, rows) = (self.grid[0] as usize, self.grid[1] as usize);
        let (xt, yt) = (cols.saturating_sub(1), rows.saturating_sub(1));
        let mut dropped = false;
        for c in 0..self.wet.len() {
            if self.wet[c] && !keep(c) {
                self.wet[c] = false;
                dropped = true;
            }
        }
        if !dropped {
            return;
        }
        self.indices.clear();
        for ty in 0..yt {
            for tx in 0..xt {
                if !self.wet[ty * xt + tx] {
                    continue;
                }
                let tl = (ty * cols + tx) as u32;
                let (tr, bl) = (tl + 1, ((ty + 1) * cols + tx) as u32);
                let br = bl + 1;
                self.indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use benilla_adt::{parse_adt, ParsedAdt};

    use super::*;
    use crate::liquid_type::{DepthRamp, LiquidTypeRow};

    /// Every liquid mesh a real tile builds, with the chunk index it came from.
    fn tile_liquids(
        chain: &mut crate::Chain,
        map: &str,
        tx: u32,
        ty: u32,
    ) -> Vec<((u32, u32), LiquidMesh)> {
        let tile = crate::load_tile_mesh(chain, map, tx, ty).expect("load tile");
        tile.chunks
            .into_iter()
            .flat_map(|c| {
                let at = (c.index_x, c.index_y);
                c.liquids.into_iter().map(move |m| (at, m))
            })
            .collect()
    }

    /// The surface height where a mesh's wet cells cover a raw WoW `(x, y)`.
    fn wet_at(m: &LiquidMesh, x: f32, y: f32) -> Option<f32> {
        let cols = m.grid[0] as usize;
        for (c, _) in m.wet.iter().enumerate().filter(|(_, &w)| w) {
            let (row, col) = (c / (cols - 1), c % (cols - 1));
            let corners = [
                row * cols + col,
                row * cols + col + 1,
                (row + 1) * cols + col,
                (row + 1) * cols + col + 1,
            ];
            let xs: Vec<f32> = corners.iter().map(|&i| m.positions[i][0]).collect();
            let ys: Vec<f32> = corners.iter().map(|&i| m.positions[i][1]).collect();
            let (x0, x1) = (
                xs.iter().cloned().fold(f32::MAX, f32::min),
                xs.iter().cloned().fold(f32::MIN, f32::max),
            );
            let (y0, y1) = (
                ys.iter().cloned().fold(f32::MAX, f32::min),
                ys.iter().cloned().fold(f32::MIN, f32::max),
            );
            if (x0..=x1).contains(&x) && (y0..=y1).contains(&y) {
                return Some(corners.iter().map(|&i| m.positions[i][2]).sum::<f32>() / 4.0);
            }
        }
        None
    }

    fn water_row(ramp: DepthRamp, lvf: VertexFormat, class: u32) -> LiquidTypeRow {
        LiquidTypeRow {
            flags: 0,
            class,
            sound_id: 1,
            vertex_format: Some(lvf),
            depth_ramp: (lvf != VertexFormat::HeightUv).then_some(ramp),
        }
    }

    fn synthetic_types() -> LiquidTypes {
        LiquidTypes::from_rows([
            (1, water_row(DepthRamp::River, VertexFormat::HeightDepth, 0)),
            (2, water_row(DepthRamp::Ocean, VertexFormat::HeightDepth, 1)),
            (3, water_row(DepthRamp::River, VertexFormat::HeightUv, 2)),
        ])
    }

    /// A 3×2 instance at tile (2, 5) spans 4×3 verts there, and its exists bitmap holes a tile.
    #[test]
    fn an_mh2o_instance_meshes_its_own_rectangle_and_bitmap() {
        // One chunk header pointing at one instance, its bitmap and its format-0 vertex data.
        let (w, h) = (3u8, 2u8);
        let n = (usize::from(w) + 1) * (usize::from(h) + 1);
        let mut d = vec![0u8; 256 * 12];
        let inst_at = d.len();
        d[0..4].copy_from_slice(&(inst_at as u32).to_le_bytes());
        d[4..8].copy_from_slice(&1u32.to_le_bytes());
        let bitmap_at = inst_at + 24;
        let verts_at = bitmap_at + 1;
        let mut inst = vec![0u8; 24];
        inst[0..2].copy_from_slice(&1u16.to_le_bytes()); // LiquidType 1
        inst[4..8].copy_from_slice(&50.0f32.to_le_bytes());
        inst[8..12].copy_from_slice(&50.0f32.to_le_bytes());
        inst[12] = 2; // x_offset
        inst[13] = 5; // y_offset
        inst[14] = w;
        inst[15] = h;
        inst[16..20].copy_from_slice(&(bitmap_at as u32).to_le_bytes());
        inst[20..24].copy_from_slice(&(verts_at as u32).to_le_bytes());
        d.extend(inst);
        d.push(0b11_1101); // tile (1, 0) is dry
        for _ in 0..n {
            d.extend(50.0f32.to_le_bytes());
        }
        d.extend(std::iter::repeat_n(21u8, n));

        let mut adt = Vec::new();
        adt.extend(b"O2HM");
        adt.extend((d.len() as u32).to_le_bytes());
        adt.extend(&d);
        let ParsedAdt::Root(root) = parse_adt(&mut std::io::Cursor::new(&adt[..])).unwrap();
        let inst = &root.mh2o_instances(0, 0)[0];
        let m = build_mh2o_mesh(inst, [1000.0, 2000.0, 0.0], &synthetic_types()).unwrap();

        assert_eq!(m.grid, [4, 3]);
        assert_eq!(m.wet, vec![true, false, true, true, true, true]);
        assert_eq!(m.indices.len(), 5 * 6);
        // The first vertex sits 5 rows south and 2 columns east of the chunk corner.
        let [x, y, z] = m.positions[0];
        assert!((x - (1000.0 - 5.0 * UNIT_SIZE)).abs() < 0.01, "{x}");
        assert!((y - (2000.0 - 2.0 * UNIT_SIZE)).abs() < 0.01, "{y}");
        assert_eq!(z, 50.0);
        assert!(
            (m.depths[0] - 0.5).abs() < 1e-3,
            "depth 21 on the river ramp"
        );
        assert_eq!(m.uvs[0], [0.5, 1.25], "world-continuous cell UVs");
        assert_eq!(m.liquid_type, 1);
        assert_eq!(m.kind, LiquidKind::Still);
    }

    /// MCLQ wets a cell only where its nibble shares the block type's class (`0x7ce180`).
    #[test]
    fn an_mclq_block_keeps_only_cells_of_its_own_class() {
        let mut tile_flags = [0u8; 64];
        tile_flags[1] = 1; // an ocean nibble under a water block
        tile_flags[2] = 0xf;
        tile_flags[3] = 4; // slow water: class 0
        let mclq = MclqChunk {
            slot: 0,
            min_height: 10.0,
            max_height: 10.0,
            vertices: vec![
                benilla_adt::LiquidVertex {
                    union_data: [0; 4],
                    height: 10.0
                };
                81
            ],
            tile_flags,
        };
        let m = build_mclq_mesh(&mclq, [0.0; 3], 1, &synthetic_types()).unwrap();
        assert!(m.wet[0] && !m.wet[1] && !m.wet[2] && m.wet[3]);
        assert_eq!(mclq_liquid_type(2, 0), 3);
        assert_eq!(mclq_liquid_type(2, OUTLAND_MAP), OUTLAND_MCLQ_MAGMA);
        assert_eq!(mclq_liquid_type(1, OUTLAND_MAP), 2);
    }

    /// The Hillsbrad river mouth: one chunk holds a format-0 river over a format-2 sea, whose
    /// vertices carry no height and read 0 (`0x8a3130`).
    #[test]
    fn a_river_mouth_chunk_builds_its_river_over_its_heightless_sea() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Maps\\Azeroth\\Azeroth_33_33.adt")
            .expect("read adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");
        let insts = root.mh2o_instances(4, 10);
        assert_eq!(insts.len(), 2, "two instances in chunk (4, 10)");
        assert_eq!(insts[0].vertex_format, 0);
        assert_eq!(insts[1].vertex_format, 2);

        let meshes: Vec<(u32, LiquidMesh)> = tile_liquids(&mut chain, "Azeroth", 33, 33)
            .into_iter()
            .filter(|(at, _)| *at == (4, 10))
            .map(|(_, m)| (m.liquid_type, m))
            .collect();
        assert_eq!(meshes.len(), 2, "both instances mesh");
        let (river, sea) = (&meshes[0].1, &meshes[1].1);
        assert_eq!(river.kind, LiquidKind::Still);
        assert_eq!(sea.kind, LiquidKind::Ocean);
        assert!(sea.positions.iter().all(|p| p[2] == 0.0), "the sea is at 0");
        let rz = river.positions[river.indices[0] as usize][2];
        assert!(rz > 1.0, "the stream ({rz}) sits above sea level");
        assert!(
            sea.depths.iter().any(|&v| v > 0.0 && v < 1.0),
            "the sea's depth bytes ride the ocean ramp"
        );
    }

    /// Stormwind's tile: its terrain water, Slow Water (5) at format 0, meshes as still water.
    #[test]
    fn stormwinds_tile_builds_its_terrain_water() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let meshes = tile_liquids(&mut chain, "Azeroth", 30, 48);
        assert!(!meshes.is_empty(), "Azeroth_30_48 carries MH2O water");
        for (_, m) in &meshes {
            assert_eq!(m.liquid_type, 5);
            assert_eq!(m.kind, LiquidKind::Still);
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
            for &i in &m.indices {
                let z = m.positions[i as usize][2];
                assert!((0.0..300.0).contains(&z), "water elevation {z} plausible");
            }
        }
    }

    /// The Westfall coast: open sea, `LiquidType` 2 at format 2, sits at z 0 under the pin.
    #[test]
    fn westfalls_coast_is_heightless_ocean_at_sea_level() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let meshes = tile_liquids(&mut chain, "Azeroth", 28, 51);
        assert!(meshes.iter().all(|(_, m)| m.kind == LiquidKind::Ocean));
        let (px, py) = (-10_150.0f32, 2_115.0);
        let z = meshes
            .iter()
            .find_map(|(_, m)| wet_at(m, px, py))
            .expect("the pin is over the sea");
        assert_eq!(z, 0.0);
    }

    /// Outland is MCLQ-only: its magma blocks are `LiquidType` 15 (`0x7c5690`) with authored UVs.
    #[test]
    fn outlands_mclq_magma_is_liquid_type_15() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let mut seen = 0;
        for (tx, ty) in [(27, 37), (27, 38), (28, 37)] {
            let Ok(tile) = crate::load_tile_mesh(&mut chain, "Expansion01", tx, ty) else {
                continue;
            };
            for m in tile.chunks.iter().flat_map(|c| &c.liquids) {
                if m.kind == LiquidKind::Magma {
                    assert_eq!(m.liquid_type, OUTLAND_MCLQ_MAGMA);
                    seen += 1;
                }
            }
        }
        assert!(seen > 0, "an Outland magma block in the sampled tiles");
    }
}
