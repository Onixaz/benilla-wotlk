//! The animated liquid surfaces: one shared material per (frame sheet, [`LiquidPath`], scroll) over
//! a `texture_2d_array` of the sheet's frames, the sheet the surface's `LiquidType` row names, the two spawn paths (an ADT chunk's MCLQ, a WMO
//! group's MLIQ) and the flat mesh. The 24 fps frame flip and the lava scroll run in the shader
//! off the wall clock, so no CPU mutates a material; a deterministic run bakes the clock off
//! (frame 0, scroll 0). Day and night follow server time.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;

use super::query::{wet_footprint, FoamPatch, LiquidSource, WmoPool};
use crate::collision::liquid_layers;
use crate::lighting::WATER_SHININESS;
use crate::terrain_stream::{build_collider_task, PendingCollider};
use avian3d::prelude::{Collider, RigidBody};
use benilla_assets::coords::wow_to_bevy;
use benilla_assets::materials::{LiquidExt, LiquidMaterial};
use benilla_assets::LockRecover;
use benilla_assets::{liquid_frame_array, LiquidTables, RenderConfig, WorldAssets};
use benilla_formats::{
    read_texture_mip_chain, BlpMipChain, LiquidKind, LiquidMesh, LiquidTypeRow, LiquidTypes,
};

/// The shared liquid materials by [`LiquidKey`]; absent without client data.
#[derive(Resource, Default)]
pub(crate) struct LiquidAssets {
    materials: HashMap<LiquidKey, LiquidEntry>,
    /// Each `LiquidType` id's own sheet, with the kind it was built for: the reference binds the
    /// row's `Texture[0]` (`0x8a28f0`), so Outland's magma (15) is green, not the base lava.
    type_sheets: HashMap<u32, (LiquidKind, usize)>,
    /// Each kind's base sheet ([`FRAME_SETS`]), for a type with no row or no sheet of its own.
    kind_sheets: HashMap<LiquidKind, usize>,
    /// The `LiquidType` rows, which decide how a liquid draws ([`TypeSettings`]).
    types: LiquidTypes,
}

/// What a liquid's `LiquidType` row decides about how it draws: a WMO pool's environment
/// (`0x793d20`), its depth texture and the sheet's texture matrix, terrain water's too (`0x8a5590`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TypeSettings {
    /// `Flags & 0x200`: the exterior environment in any group.
    exterior: bool,
    /// `Texture[1]` is a depth swatch, colour and alpha both by V.
    swatch: bool,
    /// The sheet's texture-coordinate matrix, rows `a` and `b`: `RotZ(Float[1]·57.29578)` scaled
    /// by `Float[0]`, applied as `uv·M` (`0x8a5590`, constants `c[9..12]` of `vsLiquidWater`).
    sheet: [[f32; 2]; 2],
    /// `Float[2]`, the depth texture's V factor (`0x8a5590`), on the water program's surfaces only.
    depth_v: f32,
}

impl Default for TypeSettings {
    /// A type with no row: an interior-capable pool on the band texture, its sheet unscaled.
    fn default() -> Self {
        TypeSettings {
            exterior: false,
            swatch: false,
            sheet: [[1.0, 0.0], [0.0, 1.0]],
            depth_v: 1.0,
        }
    }
}

impl TypeSettings {
    pub(crate) fn from_row(row: &LiquidTypeRow) -> Self {
        let (sin, cos) = row.sheet_rotation.sin_cos();
        let scale = row.sheet_scale;
        TypeSettings {
            exterior: row.exterior_pool(),
            swatch: row.depth_texture.is_some_and(|t| t.is_swatch()),
            sheet: [[cos * scale, sin * scale], [-sin * scale, cos * scale]],
            depth_v: row.depth_v_scale,
        }
    }

    /// `uv` through the sheet matrix, row-vector order as the vertex program applies it.
    fn sheet_uv(&self, [u, v]: [f32; 2]) -> [f32; 2] {
        let [a, b] = self.sheet;
        [u * a[0] + v * b[0], u * a[1] + v * b[1]]
    }
}

/// Which of the reference's three liquid renderers draws a surface: its dispatch sends the type
/// nibble to a category, and category 0 (nibbles 0/4/8) splits on the group's `MOGP.flags & 0x48`.
/// Magma and slime share one renderer per source, WMO or ADT, whatever the flags, so for them the
/// path only picks the fog block. Fog is per pass: the WMO liquid pass re-submits the smoothed
/// interior fog block under the same gate as the WMO geometry, so a pool fogs like its walls, while
/// ADT liquid submits none and draws under the scene block.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum LiquidPath {
    /// An ADT chunk's MCLQ: two stages, the depth swatch at `tc0 = (0.5, LUT[depthByte])` and the
    /// sheet at `tc1 = (col·¼, row·¼)`; no vertex colour. Also an exterior WMO pool whose type binds
    /// a depth swatch (WMO Ocean): 12340 draws both with the one water program (`0x8a5590`).
    Adt,
    /// A WMO group's MLIQ, exterior or exterior-lit (`MOGP.flags & 0x48 != 0`): one stage, a
    /// 9-float vertex with an up normal and a colour dword, `MapObjExtWater0.bls` bound.
    WmoExterior,
    /// A WMO group's MLIQ, a true interior (`MOGP.flags & 0x48 == 0`): one stage, a 6-float vertex
    /// with no normal, lighting off, no pixel program.
    WmoInterior,
}

impl LiquidPath {
    /// A WMO pool's renderer: interior only in an interior group (`MOGP.flags & 0x48 == 0`) and for
    /// a type without `LiquidType.Flags & 0x200`, the reference's exterior test (`0x793d20`); an
    /// exterior pool whose type binds a depth swatch draws the swatch, as terrain water does.
    pub(crate) fn wmo(group_interior: bool, pool: TypeSettings) -> Self {
        if group_interior && !pool.exterior {
            Self::WmoInterior
        } else if pool.swatch {
            Self::Adt
        } else {
            Self::WmoExterior
        }
    }

    fn interior_fog(self) -> bool {
        matches!(self, Self::WmoInterior)
    }

    /// The renderer selector `liquid.wgsl` branches on (`LiquidParams.path.x`).
    fn shader_id(self) -> f32 {
        match self {
            Self::Adt => 0.0,
            Self::WmoExterior => 1.0,
            Self::WmoInterior => 2.0,
        }
    }

    /// Does this arm's pool take its body colour from its own MOMT `diffColor`? Only the interior
    /// one: the exterior reads a live `Light.dbc` band, and ADT liquid has no MOMT.
    fn takes_material_body_color(self) -> bool {
        matches!(self, Self::WmoInterior)
    }
}

/// Which shared material a surface takes. The scroll is per type and per path, which
/// [`LiquidKind`] collapses (2 and 6 are both `Magma`), so it keys the material too.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct LiquidKey {
    /// The frame sheet, built for one kind ([`LiquidAssets::sheet`]).
    sheet: usize,
    path: LiquidPath,
    /// This surface takes the animated stage-0 texture matrix ([`scrolls`]).
    scroll: bool,
}

/// Does a WMO MLIQ surface of this type nibble take the reference's animated stage-0 texture
/// matrix, the lava/slime scroll? Only 6 and 7: the WMO magma/slime kernel gates on
/// `and esi,0xc; cmp esi,4` over the nibble and builds an identity but for element 13, the
/// v-translate. Nibbles 2 and 3 reach the same kernel and stay still (the Great Forge is 2,
/// Blackrock 6). ADT liquid never scrolls, though open-world lava is nibble 6: its kernel zeroes
/// element 13, so the caller gates the path. The phase runs off the reference's uptime clock; only
/// its rate and period are reproducible. Keyed by `LiquidType` id, the nibble + 1.
/// The `LiquidType` a WMO pool draws as: an interior pool of a water type (1, 5, 9, 13, 17: ids
/// below 21 with `(id − 1) & 3 == 0`) takes WMO Water - Interior's row, 17, for its material
/// (`0x793d20`); every other pool, and every pool's sound and swim, keep the authored type.
fn drawn_type(path: LiquidPath, liquid_type: u32) -> u32 {
    if path == LiquidPath::WmoInterior
        && (1..21).contains(&liquid_type)
        && (liquid_type - 1) & 3 == 0
    {
        17
    } else {
        liquid_type
    }
}

fn scrolls(liquid_type: u32) -> bool {
    matches!(liquid_type, 7 | 8)
}

struct LiquidEntry {
    material: Handle<LiquidMaterial>,
}

impl LiquidAssets {
    /// The sheet a surface of `liquid_type` and `kind` draws: the type's own where it was built for
    /// that kind, else the kind's base sheet.
    fn sheet(&self, liquid_type: u32, kind: LiquidKind) -> Option<usize> {
        match self.type_sheets.get(&liquid_type) {
            Some(&(built_for, sheet)) if built_for == kind => Some(sheet),
            _ => self.kind_sheets.get(&kind).copied(),
        }
    }

    /// The shared material for a surface's type and kind, renderer and scroll, if one was built and
    /// its frames loaded.
    pub(crate) fn material(
        &self,
        liquid_type: u32,
        kind: LiquidKind,
        path: LiquidPath,
        scroll: bool,
    ) -> Option<Handle<LiquidMaterial>> {
        let sheet = self.sheet(liquid_type, kind)?;
        self.materials
            .get(&LiquidKey {
                sheet,
                path,
                scroll,
            })
            .map(|e| e.material.clone())
    }

    /// What a liquid of `liquid_type` takes from its row; the default for a type with no row.
    fn type_settings(&self, liquid_type: u32) -> TypeSettings {
        self.types
            .get(liquid_type)
            .map_or_else(TypeSettings::default, TypeSettings::from_row)
    }
}

/// Marks a spawned liquid surface: one per MCNK liquid layer or WMO group pool.
#[derive(Component)]
pub(crate) struct LiquidSurface;

/// `WOW_NO_LIQUID`: hide every liquid surface and only the surface, since the swim grid, foam and
/// sound ride sibling components. An override run after both per-frame `Visibility` owners (the
/// exterior cull for ADT surfaces, `apply_model_visibility` for WMO pools), so it wins the frame.
pub(super) fn hide_liquid_surfaces(mut surfaces: Query<&mut Visibility, With<LiquidSurface>>) {
    for mut vis in &mut surfaces {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
    }
}

/// The surface's `LiquidType` id, whose row names the ambient loop (`0x4d2390`), on every liquid
/// surface; the driver reads the geometry off the same entity's `WaterChunkInfo`.
#[derive(Component)]
pub(crate) struct LiquidSoundSource {
    pub(crate) liquid_type: u32,
}

/// Spawn an ADT tile's liquid surfaces into `entities`, to despawn with the tile.
pub(crate) fn spawn_liquids<'a>(
    commands: &mut Commands,
    liquids: impl Iterator<Item = &'a LiquidMesh>,
    liquid_assets: Option<&LiquidAssets>,
    meshes: &mut Assets<Mesh>,
    entities: &mut Vec<Entity>,
) {
    let Some(liquid) = liquid_assets else {
        return;
    };
    for lq in liquids {
        // The scene fog (the ADT passes submit none and draw under the scene submit), and never a
        // scroll, though ADT magma is nibble 6: its kernel zeroes the v-translate.
        let Some(material) = liquid.material(lq.liquid_type, lq.kind, LiquidPath::Adt, false)
        else {
            continue; // this kind's frames failed to load (warned at setup)
        };
        let info = wet_footprint(lq, &Transform::IDENTITY, LiquidSource::AdtChunk);
        let foam = !lq.kind.is_fullbright(); // white surf is a water thing
        let water = foam.then(|| liquid.type_settings(lq.liquid_type));
        entities.push(
            commands
                .spawn((
                    // Water through its type's sheet and depth matrices (Orange Slime's sheet is ×2,
                    // its depth V ×8.5); magma keeps its own coordinates.
                    Mesh3d(meshes.add(liquid_bevy_mesh(
                        lq,
                        None,
                        water.as_ref(),
                        water.map_or(1.0, |t| t.depth_v),
                    ))),
                    MeshMaterial3d(material),
                    Transform::IDENTITY,
                    LiquidSurface,
                    // Exterior scene: the ADT liquid producer is called only from the per-window
                    // populate, like ADT terrain and doodads, one entity per MCNK layer. The
                    // exterior cull is its only `Visibility` writer.
                    crate::exterior_cull::ExteriorScene,
                    info,
                    LiquidSoundSource {
                        liquid_type: lq.liquid_type,
                    },
                ))
                .id(),
        );
        if foam {
            commands
                .entity(*entities.last().expect("just pushed"))
                .insert(FoamPatch);
        }
        // The waterline for the camera sweep under `cameraWaterCollision`; nothing else queries it.
        // Built off-thread like the terrain's own trimesh: a tile carries up to 256 layers.
        if let Some((verts, tris)) = liquid_collider_data(lq) {
            commands
                .entity(*entities.last().expect("just pushed"))
                .insert(PendingCollider::new(
                    build_collider_task(verts, tris),
                    Some(liquid_layers()),
                    true,
                ));
        }
    }
}

/// The collision trimesh input for one [`LiquidMesh`]: the render mesh's own wet-cell triangles,
/// `None` when every cell is dry.
fn liquid_collider_data(lq: &LiquidMesh) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let tris: Vec<[u32; 3]> = lq
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    if tris.is_empty() {
        return None;
    }
    let verts: Vec<Vec3> = lq.positions.iter().map(|p| wow_to_bevy(*p)).collect();
    Some((verts, tris))
}

/// [`liquid_collider_data`] built inline, for a WMO group's few pools.
fn liquid_collider(lq: &LiquidMesh) -> Option<Collider> {
    liquid_collider_data(lq).map(|(verts, tris)| Collider::trimesh(verts, tris))
}

/// The render mesh for one [`LiquidMesh`] in its own space (absolute for MCLQ, model-local for WMO
/// liquid): a flat up normal, the tiling UVs (through a water type's `sheet` matrix), and the
/// per-vertex depth `V` in UV1.x, times `depth_v` (the water program's depth-texture matrix).
fn liquid_bevy_mesh(
    lq: &LiquidMesh,
    body_color: Option<[f32; 3]>,
    sheet: Option<&TypeSettings>,
    depth_v: f32,
) -> Mesh {
    let positions: Vec<[f32; 3]> = lq
        .positions
        .iter()
        .map(|p| wow_to_bevy(*p).to_array())
        .collect();
    let n = positions.len();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    // WoW up (0, 0, 1) is Bevy up (0, 1, 0).
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n]);
    let uvs = match sheet {
        Some(pool) => lq.uvs.iter().map(|&uv| pool.sheet_uv(uv)).collect(),
        None => lq.uvs.clone(),
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    let uv1: Vec<[f32; 2]> = lq.depths.iter().map(|&d| [d * depth_v, 0.0]).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
    // An interior pool's `MOMT.diffColor` rides the vertex colour, where the reference's interior
    // vertex carries it, keeping one material per lane; other lanes take the shader's white.
    if let Some([red, green, blue]) = body_color {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[red, green, blue, 1.0]; n]);
    }
    mesh.insert_indices(Indices::U32(lq.indices.clone()));
    mesh
}

/// Spawn a WMO group's liquid surfaces at the building's placement `transform`, pushed onto
/// `entities` to despawn with the placement. `interior` is the group's `MOGI & 0x48 == 0` class,
/// which with the pool's type picks the renderer and the fog block ([`LiquidPath::wmo`]); `pool`
/// scopes each surface to its room and floor.
pub(crate) fn spawn_wmo_liquids<'a>(
    commands: &mut Commands,
    liquids: impl Iterator<Item = &'a LiquidMesh>,
    liquid_assets: Option<&LiquidAssets>,
    meshes: &mut Assets<Mesh>,
    transform: Transform,
    interior: bool,
    pool: WmoPool,
    // The owning root's MOMT `diffColor` table: MOMT lives in the root, not the group file.
    material_diff_color: &[[f32; 3]],
    entities: &mut Vec<Entity>,
) {
    let Some(liquid) = liquid_assets else {
        return;
    };
    for lq in liquids {
        let path = LiquidPath::wmo(interior, liquid.type_settings(lq.liquid_type));
        // The type the pool draws as: an interior water pool takes 17's row (`0x793d20`).
        let draw_type = drawn_type(path, lq.liquid_type);
        let type_settings = liquid.type_settings(draw_type);
        // The one path that can scroll, decided by the type, not the kind (`scrolls`).
        let scroll = scrolls(draw_type);
        // The interior arm's body colour via the pool's MLIQ `materialId`; none when fullbright.
        let body_color = (path.takes_material_body_color() && !lq.kind.is_fullbright())
            .then(|| {
                lq.material_id
                    .and_then(|id| material_diff_color.get(usize::from(id)))
                    .copied()
            })
            .flatten();
        let Some(material) = liquid.material(draw_type, lq.kind, path, scroll) else {
            continue; // this kind's frames failed to load (warned at setup)
        };
        if scroll {
            debug!(
                "liquid: {:?} LiquidType {} takes the scroll lane",
                lq.kind, lq.liquid_type
            );
        }
        let surface = commands
            .spawn((
                // The sheet matrix is the water material's; magma and slime keep their own UVs.
                // The depth matrix is the water program's (`LiquidPath::Adt`); the WMO programs read
                // the depth as their alpha ramp, untransformed.
                Mesh3d(meshes.add(liquid_bevy_mesh(
                    lq,
                    body_color,
                    (!lq.kind.is_fullbright()).then_some(&type_settings),
                    if path == LiquidPath::Adt {
                        type_settings.depth_v
                    } else {
                        1.0
                    },
                ))),
                MeshMaterial3d(material),
                transform,
                LiquidSurface,
                // Bit 30 is the interior-fog lane (`liquid.wgsl`'s `room_fog`), written each frame
                // by the `Visibility` authority off the pool's room; spawned clear.
                bevy::mesh::MeshTag(0),
                // Every kind, the lava and slime hum too.
                LiquidSoundSource {
                    liquid_type: lq.liquid_type,
                },
            ))
            .id();
        // Every kind carries the swim grid, so lava and slime swim; their damage is not modelled.
        commands.entity(surface).insert(wet_footprint(
            lq,
            &transform,
            LiquidSource::WmoGroup(pool),
        ));
        if !lq.kind.is_fullbright() {
            commands.entity(surface).insert(FoamPatch);
        }
        // The camera's waterline, model-local under the entity's placement `transform`.
        if let Some(collider) = liquid_collider(lq) {
            commands
                .entity(surface)
                .insert((collider, RigidBody::Static, liquid_layers()));
        }
        entities.push(surface);
    }
}

/// Each kind's base sheet, the `Texture[0]` of its base `LiquidType` row (1–4, and 9 for rapids), as
/// `(kind, template)`; a type whose row names a sheet draws that one instead ([`LiquidAssets`]).
const FRAME_SETS: &[(LiquidKind, &str)] = &[
    (LiquidKind::Still, "XTextures\\river\\lake_a.%d.blp"),
    (LiquidKind::Rapids, "XTextures\\river\\fast_a.%d.blp"),
    (LiquidKind::Ocean, "XTextures\\ocean\\ocean_h.%d.blp"),
    // Fullbright: the sheet is the opaque, unlit, fogged body. Magma comes from both the WMO pools
    // and the ADT magma queue; the reference has no ADT queue for slime.
    (LiquidKind::Magma, "XTextures\\lava\\lava.%d.blp"),
    (LiquidKind::Slime, "XTextures\\slime\\slime.%d.blp"),
];

/// The frames `0x8a2450` loads for a `%d` sheet: 1 to 30.
const SHEET_FRAMES: u32 = 30;

pub(super) fn setup_liquid(
    mut commands: Commands,
    config: Option<Res<RenderConfig>>,
    world_assets: Option<ResMut<WorldAssets>>,
    tables: Option<Res<LiquidTables>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<LiquidMaterial>>,
) {
    let (Some(_config), Some(mut world_assets)) = (config, world_assets) else {
        return; // no client data → no terrain, so no water either
    };
    // Light, fog and the water swatches come off the shared global-light buffer, as for terrain.
    let mut assets = LiquidAssets::default();
    if let Some(tables) = &tables {
        assets.types = tables.0.types.clone();
    }
    let mut builder = SheetBuilder {
        world_assets: &mut world_assets,
        images: &mut images,
        materials: &mut materials,
        built: HashMap::new(),
        frames: HashMap::new(),
    };
    for &(kind, template) in FRAME_SETS {
        match builder.sheet(&mut assets, template, kind) {
            Some(sheet) => {
                assets.kind_sheets.insert(kind, sheet);
            }
            None => warn!("liquid: no frames for {template} — {kind:?} water will not render"),
        }
    }
    // Each type's own `Texture[0]` (`0x8a27c0`), built for the kind its class draws.
    if let Some(tables) = tables {
        let types = &tables.0.types;
        for (id, row) in types.iter() {
            let (Some(template), Some(kind)) = (row.sheet.as_deref(), types.kind(id)) else {
                continue;
            };
            match builder.sheet(&mut assets, template, kind) {
                Some(sheet) => {
                    assets.type_sheets.insert(id, (kind, sheet));
                }
                None => {
                    warn!("liquid: no frames for {template} — LiquidType {id} takes {kind:?}'s")
                }
            }
        }
    }
    info!(
        "liquid: loaded {} frame sheet(s) for {} liquid type(s)",
        builder.built.values().flatten().count(),
        assets.type_sheets.len()
    );
    commands.insert_resource(assets);
}

/// Builds each distinct (sheet, kind) once: its frame array and its materials, one per renderer
/// and scroll lane the kind can take.
struct SheetBuilder<'a> {
    world_assets: &'a mut WorldAssets,
    images: &'a mut Assets<Image>,
    materials: &'a mut Assets<LiquidMaterial>,
    /// By lowercased template and kind; `None` where the frames failed to load.
    built: HashMap<(String, LiquidKind), Option<usize>>,
    /// The frame arrays by lowercased template and DC flattening, shared across kinds.
    frames: HashMap<(String, bool), Option<FrameArray>>,
}

/// A sheet's frame array and how many frames it holds.
type FrameArray = (Handle<Image>, u32);

impl SheetBuilder<'_> {
    fn sheet(
        &mut self,
        assets: &mut LiquidAssets,
        template: &str,
        kind: LiquidKind,
    ) -> Option<usize> {
        let name = template.to_ascii_lowercase();
        if let Some(&built) = self.built.get(&(name.clone(), kind)) {
            return built;
        }
        // Deviation: water frames have their per-frame mean flattened (`flatten_frame_dc`), because
        // mipping turns the shipped frames' drifting means into a flicker on distant water; magma
        // and slime keep theirs, the body's intended pulse.
        let normalize_dc = !kind.is_fullbright();
        let frames = self
            .frames
            .entry((name.clone(), normalize_dc))
            .or_insert_with(|| {
                load_frame_array(self.world_assets, self.images, template, normalize_dc)
            })
            .clone();
        let sheet = frames.map(|(frames, frame_count)| {
            let sheet = self.built.values().flatten().count();
            self.add_materials(assets, sheet, kind, &frames, frame_count);
            sheet
        });
        self.built.insert((name, kind), sheet);
        sheet
    }

    fn add_materials(
        &mut self,
        assets: &mut LiquidAssets,
        sheet: usize,
        kind: LiquidKind,
        frames: &Handle<Image>,
        frame_count: u32,
    ) {
        // Blend per kind, over the device defaults every reference setter pushes and pops against.
        // Water and ocean: blend mode 2 (`SRC_ALPHA / INV_SRC_ALPHA`), depth write off (which is
        // not modelled). Magma and slime: blend stays 0 (disabled) with depth test and write on
        // (ids `0x10`/`0x12`), as the ADT lava pass sets and the WMO arm leaves it, so they are
        // opaque. All four liquid passes turn culling off at entry; `glFrontFace` is never
        // imported.
        let alpha_mode = if kind.is_fullbright() {
            AlphaMode::Opaque
        } else {
            AlphaMode::Blend
        };
        // One material per renderer and scroll the kind can take, all on one frame array; only
        // magma and slime (nibbles 6 and 7) can scroll.
        let scroll_lanes: &[bool] = if kind.is_fullbright() {
            &[false, true]
        } else {
            &[false]
        };
        for (path, &scroll) in [
            LiquidPath::Adt,
            LiquidPath::WmoExterior,
            LiquidPath::WmoInterior,
        ]
        .into_iter()
        .flat_map(|p| scroll_lanes.iter().map(move |s| (p, s)))
        {
            let material = self.materials.add(ExtendedMaterial {
                base: StandardMaterial {
                    // The shader does the WoW lighting.
                    unlit: true,
                    alpha_mode,
                    cull_mode: None,
                    double_sided: true,
                    // Water takes the water-pass slot of the reference's interleave
                    // (`sky_order::WATER_BIAS`); the opaque kinds take no rung.
                    depth_bias: if kind.is_fullbright() {
                        0.0
                    } else {
                        crate::sky_order::WATER_BIAS
                    },
                    ..default()
                },
                extension: LiquidExt {
                    frames: frames.clone(),
                    // x = fullbright (the sheet as body, still fogged); y = the ocean swatch;
                    // z = the interior fog block; w = water's sun-sheen exponent.
                    kind: Vec4::new(
                        if kind.is_fullbright() { 1.0 } else { 0.0 },
                        if kind == LiquidKind::Ocean { 1.0 } else { 0.0 },
                        if path.interior_fog() { 1.0 } else { 0.0 },
                        WATER_SHININESS,
                    ),
                    // Which of the reference's three liquid renderers `liquid.wgsl` runs.
                    path: Vec4::new(path.shader_id(), 0.0, 0.0, 0.0),
                    // x reserved; y = frame count; z = scroll; w = clock enable (0 on a
                    // deterministic run: frame 0, scroll 0).
                    anim: Vec4::new(
                        0.0,
                        frame_count as f32,
                        if scroll { 1.0 } else { 0.0 },
                        if crate::dev_state::deterministic_run() {
                            0.0
                        } else {
                            1.0
                        },
                    ),
                    light_buf: self.world_assets.shared_light.clone(),
                },
            });
            assets.materials.insert(
                LiquidKey {
                    sheet,
                    path,
                    scroll,
                },
                LiquidEntry { material },
            );
        }
    }
}

/// Decode a `%d` sheet's frames `1..=30` with their authored mips into one repeating, anisotropic
/// array, stopping at the first missing, non-square or mis-sized frame; returns it and the count
/// loaded.
fn load_frame_array(
    world_assets: &mut WorldAssets,
    images: &mut Assets<Image>,
    template: &str,
    normalize_dc: bool,
) -> Option<FrameArray> {
    let mut frames: Vec<BlpMipChain> = Vec::new();
    let mut size = 0u32;
    for i in 1..=SHEET_FRAMES {
        let path = template.replacen("%d", &i.to_string(), 1);
        let Ok(chain) = read_texture_mip_chain(&mut world_assets.chain.lock_recover(), &path)
        else {
            break;
        };
        if chain.width != chain.height {
            break; // water frames are square; bail rather than build a ragged array
        }
        if size == 0 {
            size = chain.width;
        } else if chain.width != size {
            break; // a frame at a different resolution can't share the array
        }
        frames.push(chain);
    }
    if frames.is_empty() {
        return None;
    }
    let loaded = frames.len() as u32;
    Some((images.add(liquid_frame_array(frames, normalize_dc)), loaded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nibbles_six_and_seven_scroll() {
        // The ids are the nibble + 1. Blackrock's lava and Stratholme's slime.
        assert!(scrolls(7), "magma variant 6 takes the animated matrix");
        assert!(scrolls(8), "slime variant 7 takes the animated matrix");

        // Same kernel, same sheet, still static: the Great Forge (2) and Undercity's slime (3).
        assert!(!scrolls(3), "the Great Forge's lava is static");
        assert!(!scrolls(4), "nibble-3 slime is static");

        // 4 passes a bare `nibble & 0xc == 4` test but is a river variant, never the kernel's.
        assert!(!scrolls(5), "nibble 4 is lake_a, not a hazard liquid");

        // Everything else in the table, including the hole nibble.
        for n in [0u32, 1, 5, 8, 0xf] {
            assert!(!scrolls(n + 1), "nibble {n} must not scroll");
        }
    }

    /// Only the fullbright kinds have nibbles that scroll, so only they get a scroll lane.
    #[test]
    fn only_fullbright_kinds_can_take_a_scroll_lane() {
        for &(kind, ..) in FRAME_SETS {
            let wants_lane = kind.is_fullbright();
            assert_eq!(
                wants_lane,
                [2u8, 3, 6, 7]
                    .iter()
                    .any(|&n| scrolls(u32::from(n) + 1) && LiquidKind::from_nibble(n) == Some(kind)),
                "{kind:?}: scroll lane and the nibbles that can ask for one must agree"
            );
        }
    }

    /// The smallest MCLQ sheet `spawn_liquids` builds: 2×2 vertices at `at`.
    fn one_sheet(at: [f32; 3]) -> LiquidMesh {
        let [x, y, z] = at;
        LiquidMesh {
            grid: [2, 2],
            wet: vec![true],
            shared: vec![false],
            positions: vec![
                [x, y, z],
                [x + 8.0, y, z],
                [x, y + 8.0, z],
                [x + 8.0, y + 8.0, z],
            ],
            uvs: vec![[0.0, 0.0]; 4],
            depths: vec![1.0; 4],
            indices: vec![0, 1, 2, 1, 3, 2],
            liquid_type: 1,
            material_id: None,
            kind: LiquidKind::Still,
        }
    }

    /// Only the ADT still-water material, without which `spawn_liquids` spawns nothing.
    fn adt_assets() -> LiquidAssets {
        LiquidAssets {
            materials: HashMap::from([(
                LiquidKey {
                    sheet: 0,
                    path: LiquidPath::Adt,
                    scroll: false,
                },
                LiquidEntry {
                    material: Handle::default(),
                },
            )]),
            type_sheets: HashMap::new(),
            kind_sheets: HashMap::from([(LiquidKind::Still, 0)]),
            types: LiquidTypes::default(),
        }
    }

    /// The reference's exterior test (`0x793d20`): an exterior group's pool is exterior, and so is
    /// an interior group's pool whose type carries `LiquidType.Flags & 0x200`. An exterior pool on a
    /// depth swatch draws the swatch.
    #[test]
    fn a_wmo_pool_is_interior_only_in_an_interior_group_without_the_exterior_flag() {
        let pool = |exterior, swatch| TypeSettings {
            exterior,
            swatch,
            ..TypeSettings::default()
        };
        assert_eq!(
            LiquidPath::wmo(true, pool(false, false)),
            LiquidPath::WmoInterior
        );
        assert_eq!(
            LiquidPath::wmo(true, pool(true, false)),
            LiquidPath::WmoExterior
        );
        assert_eq!(
            LiquidPath::wmo(false, pool(false, false)),
            LiquidPath::WmoExterior
        );
        assert_eq!(
            LiquidPath::wmo(false, pool(true, false)),
            LiquidPath::WmoExterior
        );
        assert_eq!(LiquidPath::wmo(true, pool(true, true)), LiquidPath::Adt);
        assert_eq!(LiquidPath::wmo(false, pool(false, true)), LiquidPath::Adt);
    }

    /// An interior water pool draws as 17 (`0x793d20`); ocean, magma and slime, any exterior pool,
    /// and an id past the base twenty keep their own type.
    #[test]
    fn an_interior_water_pool_draws_as_wmo_water_interior() {
        for id in [1, 5, 9, 13, 17] {
            assert_eq!(drawn_type(LiquidPath::WmoInterior, id), 17, "type {id}");
            assert_eq!(drawn_type(LiquidPath::WmoExterior, id), id, "type {id}");
        }
        for id in [0, 2, 3, 4, 14, 19, 20, 21, 41, 181] {
            assert_eq!(drawn_type(LiquidPath::WmoInterior, id), id, "type {id}");
        }
    }

    /// The sheet matrix is `RotZ(θ)·s` in row-vector order: a quarter turn at scale ½ sends
    /// `(1, 0)` to `(0, ½)` and `(0, 1)` to `(−½, 0)`.
    #[test]
    fn the_sheet_matrix_rotates_and_scales_uvs_row_vector_first() {
        let row = LiquidTypeRow {
            flags: 0,
            class: 1,
            sound_id: 0,
            vertex_format: None,
            depth_ramp: None,
            sheet: None,
            depth_texture: None,
            sheet_scale: 0.5,
            sheet_rotation: std::f32::consts::FRAC_PI_2,
            depth_v_scale: 1.0,
        };
        let pool = TypeSettings::from_row(&row);
        let close =
            |[a, b]: [f32; 2], [c, d]: [f32; 2]| (a - c).abs() < 1e-6 && (b - d).abs() < 1e-6;
        assert!(close(pool.sheet_uv([1.0, 0.0]), [0.0, 0.5]));
        assert!(close(pool.sheet_uv([0.0, 1.0]), [-0.5, 0.0]));
        assert_eq!(TypeSettings::default().sheet_uv([3.0, 4.0]), [3.0, 4.0]);
    }

    /// A type draws its own row's sheet (Outland's green magma, 15), but only for the kind it was
    /// built for; a type with no sheet of its own, or a kind it was not built for, takes the kind's.
    #[test]
    fn a_liquid_type_draws_its_own_sheet() {
        const BASE_LAVA: usize = 0;
        const GREEN_LAVA: usize = 1;
        const BASE_WATER: usize = 2;
        let assets = LiquidAssets {
            materials: HashMap::new(),
            type_sheets: HashMap::from([(15, (LiquidKind::Magma, GREEN_LAVA))]),
            kind_sheets: HashMap::from([
                (LiquidKind::Magma, BASE_LAVA),
                (LiquidKind::Still, BASE_WATER),
            ]),
            types: LiquidTypes::default(),
        };
        assert_eq!(assets.sheet(15, LiquidKind::Magma), Some(GREEN_LAVA));
        assert_eq!(assets.sheet(3, LiquidKind::Magma), Some(BASE_LAVA));
        assert_eq!(assets.sheet(15, LiquidKind::Still), Some(BASE_WATER));
        assert_eq!(
            assets.sheet(15, LiquidKind::Slime),
            None,
            "no slime sheet loaded"
        );
    }

    /// An ADT surface carries `ExteriorScene` and none of the components `UnownedSceneFilter`
    /// excludes, so the exterior cull reaches it.
    #[test]
    fn an_adt_liquid_surface_is_tagged_exterior_scene() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<LiquidMaterial>();
        let sheets = [one_sheet([100.0, 100.0, 5.0])];
        let assets = adt_assets();
        let mut spawned = Vec::new();
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>| {
                    let mut ents = Vec::new();
                    spawn_liquids(
                        &mut commands,
                        sheets.iter(),
                        Some(&assets),
                        &mut meshes,
                        &mut ents,
                    );
                    ents
                },
            )
            .map(|ents| spawned = ents)
            .expect("the spawn system ran");
        assert_eq!(spawned.len(), 1, "one sheet in, one surface out");
        let e = app.world().entity(spawned[0]);
        assert!(
            e.contains::<crate::exterior_cull::ExteriorScene>(),
            "an open-world liquid surface is exterior scene — untagged, the window cull never \
             queries it and the lake draws through a sealed ceiling"
        );
        assert!(
            e.contains::<LiquidSurface>(),
            "…and is still a liquid surface"
        );
        // The three exclusions in `UnownedSceneFilter`.
        assert!(!e.contains::<crate::model_render::ModelPart>());
        assert!(!e.contains::<crate::wmo_portal::WmoGroupVis>());
        assert!(!e.contains::<crate::world_unit::WorldUnit>());
    }
}

/// The scroll and fog-block rules against the real client files.
#[cfg(test)]
mod real_data {
    use super::{LiquidKind, LiquidPath, TypeSettings};
    use benilla_formats::{parse_wmo_root, wmo_group_liquid_mesh, WmoLiquidRule};
    use std::collections::HashMap;

    /// Daggercap Bay's cave pool is the one interior-group pool of type 14 (WMO Ocean) in the
    /// install, so it draws exterior on the ocean swatch; Drak'Tharon's interior water (13) stays interior.
    #[test]
    fn the_daggercap_cave_pool_draws_exterior_on_the_ocean_swatch() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let types = benilla_formats::load_liquid_types(&chain).expect("LiquidType loads");
        let mut pool_path = |root_path: &str, gi: usize| -> (u32, LiquidPath) {
            let bytes = chain.read_file(root_path).expect("root readable");
            let root = parse_wmo_root(&bytes).expect("parse root");
            let stem = root_path.strip_suffix(".wmo").expect("a .wmo path");
            let gb = chain
                .read_file(&format!("{stem}_{gi:03}.wmo"))
                .expect("group readable");
            let lq = wmo_group_liquid_mesh(&gb, WmoLiquidRule::for_root(&root)).expect("a pool");
            let interior = root.group_infos().get(gi).expect("group info").interior;
            (
                lq.liquid_type,
                LiquidPath::wmo(
                    interior,
                    types
                        .get(lq.liquid_type)
                        .map_or_else(TypeSettings::default, TypeSettings::from_row),
                ),
            )
        };
        assert_eq!(
            pool_path("world\\wmo\\northrend\\howlingfjord\\daggercapcave.wmo", 0),
            (14, LiquidPath::Adt),
            "the cave's ocean is exterior water on the ocean swatch; the interior arm adds its \
             white MOMT colour and saturates it",
        );
        assert_eq!(
            pool_path("world\\wmo\\dungeon\\draktharon\\draktharon.wmo", 2),
            (13, LiquidPath::WmoInterior),
            "interior WMO water keeps the interior arm",
        );
    }

    /// The scroll gate over the shipped pools, where both behaviours share buildings: Ironforge has
    /// 9 static nibble-2 and 2 scrolling nibble-6 magma groups, Blackrock is nibble 6 throughout,
    /// Undercity's 38 slime canals are nibble 3 and still, and Stratholme's are nibble 7 and flow.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn only_the_nibble_six_and_seven_pools_scroll_in_the_shipped_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        // Every liquid-bearing group of a WMO, as `(LiquidType, kind)` → count.
        let mut census = |root_path: &str| -> HashMap<(u32, LiquidKind), usize> {
            let bytes = chain.read_file(root_path).expect("root readable");
            let root = parse_wmo_root(&bytes).expect("parse root");
            let rule = WmoLiquidRule::for_root(&root);
            let stem = root_path
                .strip_suffix(".wmo")
                .unwrap_or(root_path)
                .to_string();
            let mut tally = HashMap::new();
            for gi in 0..root.group_count() as usize {
                let Ok(gb) = chain.read_file(&format!("{stem}_{gi:03}.wmo")) else {
                    continue;
                };
                if let Some(m) = wmo_group_liquid_mesh(&gb, rule) {
                    *tally.entry((m.liquid_type, m.kind)).or_default() += 1;
                }
            }
            tally
        };

        let ironforge = census("world\\wmo\\khazmodan\\cities\\ironforge\\ironforge.wmo");
        assert_eq!(
            ironforge,
            HashMap::from([
                ((2, LiquidKind::Magma), 9),
                ((4, LiquidKind::Still), 2),
                ((6, LiquidKind::Magma), 2),
            ]),
            "Ironforge's liquid census moved",
        );

        let blackrock = census("world\\wmo\\dungeon\\az_blackrock\\blackrock.wmo");
        assert_eq!(
            blackrock,
            HashMap::from([((6, LiquidKind::Magma), 2)]),
            "Blackrock Mountain's lava — the `.go xyz -7531.21 -1123.64 172.58` pin — is all nibble 6",
        );

        let undercity = census("world\\wmo\\lorderon\\undercity\\undercity.wmo");
        assert_eq!(
            undercity,
            HashMap::from([((3, LiquidKind::Slime), 38)]),
            "Undercity's slime is nibble 3 throughout",
        );

        let stratholme = census("world\\wmo\\dungeon\\ld_stratholme\\stratholme.wmo");
        assert_eq!(
            stratholme,
            HashMap::from([((7, LiquidKind::Slime), 3)]),
            "Stratholme's slime is nibble 7 throughout",
        );

        // The gate's verdict over all of it.
        assert_eq!(
            ironforge
                .iter()
                .filter(|((n, _), _)| super::scrolls(*n))
                .map(|(_, c)| c)
                .sum::<usize>(),
            2,
            "exactly 2 of Ironforge's 11 magma pools scroll",
        );
        for (label, tally) in [("blackrock", &blackrock), ("stratholme", &stratholme)] {
            assert!(
                tally.keys().all(|(n, _)| super::scrolls(*n)),
                "{label}: every pool scrolls",
            );
        }
        assert!(
            undercity.keys().all(|(n, _)| !super::scrolls(*n)),
            "Undercity's slime stands still",
        );
    }

    /// A WMO pool's fog block follows its group's `MOGI & 0x48 == 0` class: Undercity's 38 liquid
    /// groups are interior but for group 7, and Stormwind's 22 are all exterior.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn a_wmo_pools_fog_block_follows_its_groups_interior_class() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        // Which groups of a WMO carry liquid, and whether each is an interior group.
        let liquid_groups = |chain: &mut benilla_formats::Chain, root_path: &str| {
            let bytes = chain.read_file(root_path).expect("root readable");
            let root = parse_wmo_root(&bytes).expect("parse root");
            let rule = WmoLiquidRule::for_root(&root);
            let stem = root_path
                .strip_suffix(".wmo")
                .unwrap_or(root_path)
                .to_string();
            (0..root.group_count() as usize)
                .filter_map(|gi| {
                    let gb = chain.read_file(&format!("{stem}_{gi:03}.wmo")).ok()?;
                    wmo_group_liquid_mesh(&gb, rule)?;
                    Some((gi, root.group_infos().get(gi).is_some_and(|g| g.interior)))
                })
                .collect::<Vec<_>>()
        };

        let uc = liquid_groups(&mut chain, "world\\wmo\\lorderon\\undercity\\undercity.wmo");
        let exterior: Vec<usize> = uc.iter().filter(|(_, i)| !i).map(|(g, _)| *g).collect();
        assert_eq!(uc.len(), 38, "Undercity's liquid group count moved: {uc:?}");
        assert_eq!(
            exterior,
            vec![7],
            "exactly one Undercity liquid group is exterior (the flag is per group, not per building)",
        );

        let sw = liquid_groups(
            &mut chain,
            "world\\wmo\\azeroth\\buildings\\stormwind\\stormwind.wmo",
        );
        assert_eq!(sw.len(), 22, "Stormwind's liquid group count moved: {sw:?}");
        assert!(
            sw.iter().all(|(_, interior)| !interior),
            "Stormwind's canals and fountains are open to the sky — none takes the interior fog: {sw:?}",
        );
    }
}
