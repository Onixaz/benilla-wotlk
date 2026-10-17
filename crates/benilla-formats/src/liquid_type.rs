//! `LiquidType.dbc` and `LiquidMaterial.dbc`: what a 3.3.5a liquid is, by the `LiquidType` id an
//! MH2O instance, an MCLQ block or a WMO group stores. The id picks the vertex format through its
//! material (`0x7c5690`), the depth swatch (`0x79e3c0`), the ambient loop (`0x4d2390`) and the
//! class the kind is drawn from.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
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
/// `Int[0]`: which depth table the swatch V reads (`0x79e3c0`'s `s_depthCoordTables`).
const DEPTH_TABLE: usize = 41;

/// `LiquidType.Flags` bit: the loop is not played while the player is inside (`0x4d2390`).
const FLAG_NO_SOUND_INSIDE: u32 = 0x80;

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
/// `d/255`, the two tables `0x79e3c0` builds.
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
    /// The depth table index; `None` where the material takes no depth (`0x79e3c0`'s caller
    /// returns no table unless the LVF is 0 or 2).
    pub depth_ramp: Option<DepthRamp>,
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

    /// The kind benilla draws `id` with. The reference binds `Texture[0]`; benilla's four texture
    /// sets stand in by class, the magma and slime classes by their textured material.
    pub fn kind(&self, id: u32) -> Option<LiquidKind> {
        let row = self.get(id)?;
        Some(match row.class {
            1 => LiquidKind::Ocean,
            2 => LiquidKind::Magma,
            3 => LiquidKind::Slime,
            _ => LiquidKind::Still,
        })
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
            },
        );
    }
    Ok(LiquidTypes { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ramps are `0x79e3c0`'s tables: the river saturates near byte 42, the ocean at 255.
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
        assert_eq!(types.get(1).unwrap().depth_ramp, Some(DepthRamp::River));
        assert_eq!(types.get(2).unwrap().depth_ramp, Some(DepthRamp::Ocean));
        assert_eq!(
            types.get(3).unwrap().depth_ramp,
            None,
            "magma reads no depth"
        );
        // Every id a shipped MH2O instance names resolves.
        for id in [1, 2, 3, 5, 7, 81, 141, 181] {
            assert!(types.kind(id).is_some(), "MH2O LiquidType {id}");
            assert!(types.sound(id).is_some(), "LiquidType {id} loops a sound");
        }
    }
}
