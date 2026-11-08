//! `SpellVisualKitAreaModel.dbc`: the shard models a DynamicObject's emitter scatters, picked by
//! row index, not id (`0x7fbd70`).

use anyhow::Result;
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at};

pub(crate) const SPELL_VISUAL_KIT_AREA_MODEL: &str = "DBFilesClient\\SpellVisualKitAreaModel.dbc";

/// The model an emitter falls back on with no row at all (`0x7fbda5`, string `0xa4207c`).
pub const AREA_MODEL_FALLBACK: &str = "Spells\\Blizzard_Impact_Base.mdx";

pub(crate) fn area_model_schema() -> Schema {
    let mut s = Schema::new("SpellVisualKitAreaModel");
    s.add_field(SchemaField::new("F0", FieldType::UInt32));
    s.add_field(SchemaField::new("F1", FieldType::String));
    s.add_field(SchemaField::new("F2", FieldType::UInt32));
    s
}

/// Column 1 of each row, in file order: the index space the emitter picks from.
pub(crate) fn parse_area_models(bytes: &[u8]) -> Result<Vec<String>> {
    let set = parse(bytes, area_model_schema(), "SpellVisualKitAreaModel.dbc")?;
    Ok(set
        .records()
        .iter()
        .map(|r| str_at(&set, r, 1).unwrap_or_default())
        .collect())
}

/// The shard model for `density`: row `density`, else row 0, else [`AREA_MODEL_FALLBACK`] (`0x7fbd86`).
pub(crate) fn area_model_at(rows: &[String], density: u8) -> &str {
    rows.get(usize::from(density))
        .or_else(|| rows.first())
        .map_or(AREA_MODEL_FALLBACK, String::as_str)
}

/// The emitter's model index from `CharParamZero`: rounded to an integer, low byte kept (`0x70501c`).
pub fn shard_density(param0: f32) -> u8 {
    param0.round_ties_even() as i32 as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width 12340's loader checks (`reference/3.3.5a-dbc.tsv`).
    #[test]
    fn the_schema_holds_the_reference_width() {
        let s = area_model_schema();
        assert_eq!((s.expanded_len(), s.record_width()), (3, 12));
    }

    #[test]
    fn the_pick_is_by_index_then_row_zero_then_the_hard_coded_model() {
        let rows = vec!["Spells\\A.mdx".to_string(), "Spells\\B.mdx".to_string()];
        assert_eq!(area_model_at(&rows, 1), "Spells\\B.mdx");
        assert_eq!(
            area_model_at(&rows, 2),
            "Spells\\A.mdx",
            "past the table: row 0"
        );
        assert_eq!(area_model_at(&[], 0), AREA_MODEL_FALLBACK);
    }

    #[test]
    fn the_density_rounds_and_keeps_the_low_byte() {
        assert_eq!(shard_density(0.0), 0);
        assert_eq!(shard_density(1.0), 1);
        assert_eq!(
            shard_density(13.6),
            14,
            "fistp rounds, the 1.12 decode truncated"
        );
        assert_eq!(shard_density(258.0), 2, "movzx keeps the low byte");
        assert_eq!(shard_density(-1.0), 255);
    }
}
