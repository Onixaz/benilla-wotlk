//! `PaperDollItemFrame.dbc`: a slot button's name, its empty-slot texture and its slot number, in
//! file order, which the four readers walk (`0x5e7d60`, `0x5e7c00`, `0x51cdb0`, `0x5e95c0`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, str_at};
use crate::Chain;

const PAPERDOLL_ITEM_FRAME: &str = "DBFilesClient\\PaperDollItemFrame.dbc";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaperDollItemFrameRow {
    pub name: String,
    /// The whole path, as `GetInventorySlotInfo` hands it to Lua.
    pub texture: String,
    /// Signed in the record (`0x5e7df5`).
    pub slot: i32,
}

#[derive(Clone, Debug, Default)]
pub struct PaperDollItemFrameCatalog {
    rows: Vec<PaperDollItemFrameRow>,
}

impl PaperDollItemFrameCatalog {
    pub fn rows(&self) -> &[PaperDollItemFrameRow] {
        &self.rows
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("PaperDollItemFrame");
    s.add_field(SchemaField::new("Name", FieldType::String));
    s.add_field(SchemaField::new("Texture", FieldType::String));
    s.add_field(SchemaField::new("Slot", FieldType::Int32));
    s
}

pub fn load_paperdoll_item_frame(chain: &Chain) -> Result<PaperDollItemFrameCatalog> {
    let bytes = chain
        .read(PAPERDOLL_ITEM_FRAME)
        .with_context(|| format!("reading {PAPERDOLL_ITEM_FRAME}"))?;
    let rs = parse(&bytes, schema(), "PaperDollItemFrame")?;
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(PaperDollItemFrameRow {
                name: str_at(&rs, r, 0)?,
                texture: str_at(&rs, r, 1)?,
                slot: i32_at(r, 2)?,
            })
        })
        .collect();
    Ok(PaperDollItemFrameCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_reference_rows() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let cat = load_paperdoll_item_frame(&chain).expect("PaperDollItemFrame.dbc");
        let row = |name: &str| cat.rows().iter().find(|r| r.name == name);
        assert_eq!(row("HeadSlot").map(|r| r.slot), Some(1));
        assert_eq!(row("Bag12").map(|r| r.slot), Some(75));
        // The texture column is a whole path with its extension, not a basename.
        for r in cat.rows() {
            let t = r.texture.to_ascii_lowercase();
            assert!(t.contains('\\') && t.ends_with(".blp"), "{}", r.texture);
        }
    }
}
