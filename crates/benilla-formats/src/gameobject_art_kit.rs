//! `GameObjectArtKit.dbc`: the art a GameObject's `GAMEOBJECT_BYTES_1` byte 2 names. Of a row's
//! three textures, texture `i` replaces the model's texture type `11 + i`, and attach model `i`
//! hangs on the model's attachment id `i` (`0x713da0`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const GAMEOBJECT_ART_KIT: &str = "DBFilesClient\\GameObjectArtKit.dbc";

/// A row's texture slots, the M2 texture types 11 to 13.
pub const ART_KIT_TEXTURES: usize = 3;
/// A row's attach-model slots, the attachment ids 0 to 3.
pub const ART_KIT_MODELS: usize = 4;

/// One `GameObjectArtKit` row; an empty string is `None`, which the reference skips.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtKit {
    /// `TextureVariation[3]`, each replacing texture type `11 + i`.
    pub textures: [Option<String>; ART_KIT_TEXTURES],
    /// `AttachModel[4]`, each hung on attachment id `i`.
    pub models: [Option<String>; ART_KIT_MODELS],
}

impl ArtKit {
    /// The attach models with their attachment ids, in slot order.
    pub fn attached(&self) -> impl Iterator<Item = (u16, &str)> {
        self.models
            .iter()
            .enumerate()
            .filter_map(|(i, m)| m.as_deref().map(|m| (i as u16, m)))
    }
}

/// Art kit id → row.
pub struct GameObjectArtKits(HashMap<u32, ArtKit>);

impl GameObjectArtKits {
    /// Build from rows, for tests.
    pub fn from_rows(rows: HashMap<u32, ArtKit>) -> Self {
        GameObjectArtKits(rows)
    }

    /// The row an art kit names; the field is a signed byte, so a negative kit names none.
    pub fn get(&self, art_kit: i8) -> Option<&ArtKit> {
        u32::try_from(art_kit).ok().and_then(|id| self.0.get(&id))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 8 columns, 32-byte rows, as the 12340 reader (`0x8afa10`) takes them.
pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("GameObjectArtKit");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..ART_KIT_TEXTURES {
        s.add_field(SchemaField::new(
            format!("TextureVariation{i}"),
            FieldType::String,
        ));
    }
    for i in 0..ART_KIT_MODELS {
        s.add_field(SchemaField::new(
            format!("AttachModel{i}"),
            FieldType::String,
        ));
    }
    s.set_key_field("ID");
    s
}

/// Load `GameObjectArtKit.dbc` from the patch chain.
pub fn load_gameobject_art_kits(chain: &mut Chain) -> Result<GameObjectArtKits> {
    let bytes = chain
        .read_file(GAMEOBJECT_ART_KIT)
        .with_context(|| format!("reading {GAMEOBJECT_ART_KIT}"))?;
    table_from(&bytes)
}

fn table_from(bytes: &[u8]) -> Result<GameObjectArtKits> {
    let rs = parse(bytes, schema(), "GameObjectArtKit")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id,
            ArtKit {
                textures: std::array::from_fn(|i| str_at(&rs, r, 1 + i)),
                models: std::array::from_fn(|i| str_at(&rs, r, 1 + ART_KIT_TEXTURES + i)),
            },
        );
    }
    Ok(GameObjectArtKits(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WDBC of `(id, [7 strings])` rows, `""` an empty column.
    fn synthesize(rows: &[(u32, [&str; 7])]) -> Vec<u8> {
        let mut strings = vec![0u8];
        let mut offset = |s: &str| -> u32 {
            if s.is_empty() {
                return 0;
            }
            let at = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
            at
        };
        let mut records = Vec::new();
        for (id, cols) in rows {
            records.extend_from_slice(&id.to_le_bytes());
            for c in cols {
                records.extend_from_slice(&offset(c).to_le_bytes());
            }
        }
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&8u32.to_le_bytes()); // fields
        dbc.extend_from_slice(&32u32.to_le_bytes()); // record size
        dbc.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&records);
        dbc.extend_from_slice(&strings);
        dbc
    }

    #[test]
    fn a_row_splits_into_textures_and_attach_models_by_slot() {
        let table = table_from(&synthesize(&[
            (1, ["", "", "", "Flag.mdx", "", "", ""]),
            (
                7,
                ["Tex0.blp", "", "Tex2.blp", "", "", "Ribbon.mdx", "Glow.mdx"],
            ),
        ]))
        .unwrap();
        assert_eq!(table.len(), 2);
        let one = table.get(1).unwrap();
        assert_eq!(one.textures, [None, None, None]);
        assert_eq!(one.attached().collect::<Vec<_>>(), [(0, "Flag.mdx")]);
        let seven = table.get(7).unwrap();
        assert_eq!(
            seven.textures,
            [Some("Tex0.blp".into()), None, Some("Tex2.blp".into())]
        );
        // An empty slot keeps its neighbours on their own attachment ids.
        assert_eq!(
            seven.attached().collect::<Vec<_>>(),
            [(2, "Ribbon.mdx"), (3, "Glow.mdx")]
        );
    }

    #[test]
    fn kit_zero_and_a_negative_kit_name_no_row() {
        let table = table_from(&synthesize(&[(1, ["", "", "", "Flag.mdx", "", "", ""])])).unwrap();
        assert!(table.get(0).is_none());
        assert!(table.get(-1).is_none());
        assert!(table.get(1).is_some());
    }

    /// The 12340 table: 17 rows, all attach models. Row 1 is the Eastern Plaguelands tower's
    /// Horde banner; row 81 hangs the lava splash on all four attachment ids.
    #[test]
    fn real_gameobject_art_kit_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let table = load_gameobject_art_kits(&mut chain).expect("GameObjectArtKit.dbc");
        assert_eq!(table.len(), 17);
        let one = table.get(1).expect("row 1");
        assert_eq!(one.textures, [None, None, None]);
        assert_eq!(
            one.attached().collect::<Vec<_>>(),
            [(
                0,
                "World\\Generic\\PVP\\BattlefieldBanners\\\
                 BattlefieldBanner_State_FlagH_Plaguelands.mdx"
            )]
        );
        assert_eq!(table.get(81).expect("row 81").attached().count(), 4);
    }
}
