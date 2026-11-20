//! `LiquidType.dbc` and `LiquidMaterial.dbc`: what a 3.3.5a liquid is, by the `LiquidType` id an
//! MH2O instance, an MCLQ block or a WMO group stores. The id picks the vertex format through its
//! material (`0x7c5690`), the depth swatch, the ambient loop (`0x4d2390`), the frame sheet
//! (`0x8a27c0`) and the class the kind is drawn from.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::liquid::LiquidKind;
use crate::Chain;

const LIQUID_TYPE: &str = "DBFilesClient\\LiquidType.dbc";
const LIQUID_MATERIAL: &str = "DBFilesClient\\LiquidMaterial.dbc";

/// `LiquidType.dbc` columns: 45, `Name` at 1 and `Texture[6]` at 15..=20 the strings.
const TYPE_COLUMNS: usize = 45;
const NAME: usize = 1;
const FLAGS: usize = 2;
/// `SoundBank`: 0 water, 1 ocean, 2 magma, 3 slime.
const SOUND_BANK: usize = 3;
const SOUND_ID: usize = 4;
const MATERIAL_ID: usize = 14;
const TEXTURES: std::ops::RangeInclusive<usize> = 15..=20;
/// `Int[0]`: which depth table the swatch V reads (the reference's depth-coordinate tables).
const DEPTH_TABLE: usize = 41;
/// `Texture[1]`: the depth texture the water material binds on stage 0 (`0x8a5590`).
const DEPTH_TEXTURE: usize = 16;
/// `Float[0]` and `Float[1]`: the sheet's texture-coordinate scale and rotation (`0x8a5590`).
const SHEET_SCALE: usize = 23;
const SHEET_ROTATION: usize = 24;
/// `Float[2]`: the depth texture's V scale (`0x8a5590`).
const DEPTH_V_SCALE: usize = 25;

/// `LiquidType.Flags` bit: the loop is not played while the player is inside (`0x4d2390`).
const FLAG_NO_SOUND_INSIDE: u32 = 0x80;

/// `LiquidType.Flags` bit: a WMO pool of this type takes the exterior liquid environment even in an
/// interior group (`0x793d20`); only 14, WMO Ocean, carries it.
const FLAG_EXTERIOR_POOL: u32 = 0x200;

/// A liquid's vertex format, `LiquidMaterial.LVF` or an MH2O instance's own: what each vertex of
/// the grid stores (`0x8a3130`, `0x8a3160`, `0x8a31b0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    /// 0: a height and a depth byte.
    HeightDepth,
    /// 1: a height and a `u16` texture coordinate pair.
    HeightUv,
    /// 2: a depth byte alone; the height reads 0.
    Depth,
    /// 3 and up: nothing; the height reads 0 and the depth 0.
    None,
}

impl VertexFormat {
    pub fn from_raw(lvf: u32) -> Self {
        match lvf {
            0 => VertexFormat::HeightDepth,
            1 => VertexFormat::HeightUv,
            2 => VertexFormat::Depth,
            _ => VertexFormat::None,
        }
    }

    /// Whether a vertex carries a height; the reference reads 0 where not (`0x8a3130`).
    pub fn has_height(self) -> bool {
        matches!(self, VertexFormat::HeightDepth | VertexFormat::HeightUv)
    }
}

/// The swatch V a depth byte maps to: the river ramp `clamp((d/9)/4.6667)` or the ocean ramp
/// `d/255`, the two tables builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthRamp {
    River,
    Ocean,
}

impl DepthRamp {
    pub fn v(self, depth: u8) -> f32 {
        match self {
            DepthRamp::River => {
                let d = f32::from(depth) * (1.0 / 9.0);
                if d <= 4.666_666_5 {
                    d * 0.214_285_72
                } else {
                    1.0
                }
            }
            DepthRamp::Ocean => (f32::from(depth) / 255.0).min(1.0),
        }
    }
}

/// The procedural texture a water row's `Texture[1]` names, which the water material samples at
/// `(u, depth V)` (`0x8a2e20` creates the three; `0x8a2bf0`, `0x8a2ac0` fill them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthTexture {
    /// `proceduralRiverDepthTex`: the river swatch, shallow to deep by V.
    River,
    /// `proceduralOceanDepthTex`: the ocean swatch, its last row darkened.
    Ocean,
    /// `proceduralWmoWaterTex`: the deep river band at `u < 0.5`, white beyond, alpha by V.
    WmoWater,
}

impl DepthTexture {
    fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "proceduralriverdepthtex" => Some(DepthTexture::River),
            "proceduraloceandepthtex" => Some(DepthTexture::Ocean),
            "proceduralwmowatertex" => Some(DepthTexture::WmoWater),
            _ => None,
        }
    }

    /// Whether this is a depth swatch, colour and alpha both by V, as the terrain water draws.
    pub fn is_swatch(self) -> bool {
        matches!(self, DepthTexture::River | DepthTexture::Ocean)
    }
}

impl LiquidTypeRow {
    /// Whether a WMO pool of this type takes the exterior liquid environment whatever its group's
    /// class (`0x793d20`).
    pub fn exterior_pool(&self) -> bool {
        self.flags & FLAG_EXTERIOR_POOL != 0
    }
}

/// One `LiquidType` row, the columns benilla reads.
#[derive(Debug, Clone)]
pub struct LiquidTypeRow {
    pub flags: u32,
    /// `SoundBank`, the class: 0 water, 1 ocean, 2 magma, 3 slime.
    pub class: u32,
    /// The `SoundEntries` kit looped near this liquid.
    pub sound_id: u32,
    /// The material's vertex format; `None` where the material row is missing.
    pub vertex_format: Option<VertexFormat>,
    /// The depth table index; `None` where the material takes no depth (the reference's caller
    /// returns no table unless the LVF is 0 or 2).
    pub depth_ramp: Option<DepthRamp>,
    /// `Texture[0]`, the animated frame sheet the water and magma materials bind as their body
    /// (`0x8a27c0` copies the names, `0x8a2450` expands `%d` over frames 1–30); `None` where the
    /// slot is empty or not a `%d` sheet.
    pub sheet: Option<String>,
    /// `Texture[1]`, where it names one of the procedural depth textures.
    pub depth_texture: Option<DepthTexture>,
    /// `Float[0]`: the factor on the sheet's texture coordinates; 1 for plain water, 0.25 for WMO
    /// Ocean (`0x8a5590`).
    pub sheet_scale: f32,
    /// `Float[1] · 57.29578`, the angle `0x8a5590` rotates the sheet's coordinates by, in radians.
    /// The reference multiplies the authored degrees by the radians-to-degrees factor and hands the
    /// product to a radian rotation, so WMO Ocean's 180 turns the sheet ≈145.6°, not a half turn.
    /// Kept as the reference draws it: do not "fix" it to `to_radians`.
    pub sheet_rotation: f32,
    /// `Float[2]`, the factor on the depth texture's V (its texture matrix is identity but for
    /// `b1`, `0x8a5590`); 1 for every water type but Orange Slime (181, 8.5).
    pub depth_v_scale: f32,
}

/// Every `LiquidType` row, by id.
#[derive(Debug, Clone, Default)]
pub struct LiquidTypes {
    rows: HashMap<u32, LiquidTypeRow>,
}

impl LiquidTypes {
    pub fn get(&self, id: u32) -> Option<&LiquidTypeRow> {
        self.rows.get(&id)
    }

    /// The kind benilla draws `id` with: the class picks the renderer, the magma and slime classes
    /// by their textured material. The frames are the row's own [`LiquidTypeRow::sheet`].
    pub fn kind(&self, id: u32) -> Option<LiquidKind> {
        let row = self.get(id)?;
        Some(match row.class {
            1 => LiquidKind::Ocean,
            2 => LiquidKind::Magma,
            3 => LiquidKind::Slime,
            _ => LiquidKind::Still,
        })
    }

    /// The frame sheet `id` binds, a `%d` path template ([`LiquidTypeRow::sheet`]).
    pub fn sheet(&self, id: u32) -> Option<&str> {
        self.get(id)?.sheet.as_deref()
    }

    /// Every row, by id, in no order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &LiquidTypeRow)> {
        self.rows.iter().map(|(&id, row)| (id, row))
    }

    /// The ambient loop kit for `id`, and whether it is muted while the player is inside.
    pub fn sound(&self, id: u32) -> Option<(u32, bool)> {
        let row = self.get(id)?;
        (row.sound_id != 0).then_some((row.sound_id, row.flags & FLAG_NO_SOUND_INSIDE != 0))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// A table from explicit rows, for tests without an install.
    pub fn from_rows(rows: impl IntoIterator<Item = (u32, LiquidTypeRow)>) -> Self {
        LiquidTypes {
            rows: rows.into_iter().collect(),
        }
    }
}

/// A float column the schema reads as `u32`, bit for bit.
fn f32_bits(r: &benilla_dbc::Record, i: usize) -> Option<f32> {
    u32_at(r, i).map(f32::from_bits)
}

pub(crate) fn type_schema() -> Schema {
    let mut s = Schema::new("LiquidType");
    for i in 0..TYPE_COLUMNS {
        let ty = if i == NAME || TEXTURES.contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    s
}

pub(crate) fn material_schema() -> Schema {
    let mut s = Schema::new("LiquidMaterial");
    for name in ["ID", "LVF", "Flags"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read `LiquidType.dbc` and `LiquidMaterial.dbc` off the patch chain.
pub fn load_liquid_types(chain: &Chain) -> Result<LiquidTypes> {
    let bytes = chain
        .read(LIQUID_MATERIAL)
        .with_context(|| format!("reading {LIQUID_MATERIAL}"))?;
    let rs = parse(&bytes, material_schema(), "LiquidMaterial")?;
    let lvf: HashMap<u32, u32> = rs
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 1)?)))
        .collect();

    let bytes = chain
        .read(LIQUID_TYPE)
        .with_context(|| format!("reading {LIQUID_TYPE}"))?;
    let rs = parse(&bytes, type_schema(), "LiquidType")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let material = u32_at(r, MATERIAL_ID).and_then(|m| lvf.get(&m).copied());
        let vertex_format = material.map(VertexFormat::from_raw);
        let depth_ramp =
            material
                .filter(|&l| l == 0 || l == 2)
                .map(|_| match u32_at(r, DEPTH_TABLE) {
                    Some(1) => DepthRamp::Ocean,
                    _ => DepthRamp::River,
                });
        rows.insert(
            id,
            LiquidTypeRow {
                flags: u32_at(r, FLAGS).unwrap_or(0),
                class: u32_at(r, SOUND_BANK).unwrap_or(0),
                sound_id: u32_at(r, SOUND_ID).unwrap_or(0),
                vertex_format,
                depth_ramp,
                sheet: str_at(&rs, r, *TEXTURES.start()).filter(|t| t.contains("%d")),
                depth_texture: str_at(&rs, r, DEPTH_TEXTURE)
                    .and_then(|name| DepthTexture::from_name(&name)),
                sheet_scale: f32_bits(r, SHEET_SCALE).unwrap_or(1.0),
                sheet_rotation: f32_bits(r, SHEET_ROTATION).unwrap_or(0.0) * 57.295_78,
                depth_v_scale: f32_bits(r, DEPTH_V_SCALE).unwrap_or(1.0),
            },
        );
    }
    Ok(LiquidTypes { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ramps are the reference's tables: the river saturates near byte 42, the ocean at 255.
    #[test]
    fn depth_ramps_are_the_two_reference_tables() {
        assert_eq!(DepthRamp::River.v(0), 0.0);
        assert!((DepthRamp::River.v(21) - 0.5).abs() < 1e-3);
        assert!(DepthRamp::River.v(41) < 1.0);
        assert_eq!(DepthRamp::River.v(43), 1.0);
        assert!((DepthRamp::Ocean.v(51) - 0.2).abs() < 1e-6);
        assert_eq!(DepthRamp::Ocean.v(255), 1.0);
    }

    #[test]
    fn real_tables_resolve_the_shipped_terrain_liquids() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let types = load_liquid_types(&chain).expect("load LiquidType");
        assert!(!types.is_empty());
        // The four MCLQ ids (header bit + 1) and Outland's magma (15), by class and format.
        for (id, kind, lvf) in [
            (1, LiquidKind::Still, VertexFormat::HeightDepth),
            (2, LiquidKind::Ocean, VertexFormat::HeightDepth),
            (3, LiquidKind::Magma, VertexFormat::HeightUv),
            (4, LiquidKind::Slime, VertexFormat::HeightUv),
            (15, LiquidKind::Magma, VertexFormat::HeightUv),
        ] {
            assert_eq!(types.kind(id), Some(kind), "LiquidType {id}");
            assert_eq!(types.get(id).unwrap().vertex_format, Some(lvf), "{id}");
        }
        // Each row names its own sheet: Outland's magma is the green one, not the base lava.
        for (id, sheet) in [
            (1, "XTextures\\river\\lake_a.%d.blp"),
            (3, "XTextures\\lava\\lava.%d.blp"),
            (9, "XTextures\\river\\fast_a.%d.blp"),
            (15, "XTextures\\LavaGreen\\lavagreen.%d.blp"),
        ] {
            assert_eq!(types.sheet(id), Some(sheet), "LiquidType {id}");
        }
        assert_eq!(types.get(1).unwrap().depth_ramp, Some(DepthRamp::River));
        assert_eq!(types.get(2).unwrap().depth_ramp, Some(DepthRamp::Ocean));
        assert_eq!(
            types.get(3).unwrap().depth_ramp,
            None,
            "magma reads no depth"
        );
        // WMO Ocean draws the ocean swatch with its sheet four times as wide; WMO water draws the
        // band texture at the plain sheet scale.
        let ocean = types.get(14).unwrap();
        assert_eq!(ocean.depth_texture, Some(DepthTexture::Ocean));
        assert_eq!(ocean.sheet_scale, 0.25);
        assert_eq!(ocean.sheet_rotation, 180.0 * 57.295_78);
        let water = types.get(13).unwrap();
        assert_eq!(water.depth_texture, Some(DepthTexture::WmoWater));
        assert_eq!((water.sheet_scale, water.sheet_rotation), (1.0, 0.0));
        // The depth V stays as authored on plain water; Orange Slime stretches it 8.5×.
        assert_eq!(water.depth_v_scale, 1.0);
        assert_eq!(types.get(1).unwrap().depth_v_scale, 1.0);
        assert_eq!(types.get(181).unwrap().depth_v_scale, 8.5);
        assert_eq!(
            types.get(2).unwrap().depth_texture,
            Some(DepthTexture::Ocean)
        );
        assert_eq!(
            types.get(1).unwrap().depth_texture,
            Some(DepthTexture::River)
        );
        // Only WMO Ocean is an exterior pool; interior WMO water (13, 17) is not.
        let exterior: Vec<u32> = (0..=200)
            .filter(|&id| types.get(id).is_some_and(|r| r.exterior_pool()))
            .collect();
        assert_eq!(exterior, [14], "the 0x200 rows");
        // Every id a shipped MH2O instance names resolves.
        for id in [1, 2, 3, 5, 7, 81, 141, 181] {
            assert!(types.kind(id).is_some(), "MH2O LiquidType {id}");
            assert!(types.sound(id).is_some(), "LiquidType {id} loops a sound");
        }
    }
}
