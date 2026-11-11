//! `PaperDollItemFrame.dbc`: the equipment and bag slot names used by the FrameScript inventory API.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::dbc_layout::LAYOUTS;
use crate::Chain;

const PAPERDOLL_ITEM_FRAME: &str = "DBFilesClient\\PaperDollItemFrame.dbc";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaperDollItemFrameRow {
    pub name: String,
    pub texture: String,
    pub slot: u32,
}

#[derive(Clone, Debug, Default)]
pub struct PaperDollItemFrameCatalog {
    rows: Vec<PaperDollItemFrameRow>,
}

impl PaperDollItemFrameCatalog {
    pub fn rows(&self) -> &[PaperDollItemFrameRow] {
        &self.rows
    }
    pub fn find(&self, name: &str) -> Option<&PaperDollItemFrameRow> {
        self.rows.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("PaperDollItemFrame");
    s.add_field(SchemaField::new("Name", FieldType::String));
    s.add_field(SchemaField::new("Texture", FieldType::String));
    s.add_field(SchemaField::new("Slot", FieldType::UInt32));
    s
}

pub fn load_paperdoll_item_frame(chain: &mut Chain) -> Result<PaperDollItemFrameCatalog> {
    let bytes = chain
        .read_file(PAPERDOLL_ITEM_FRAME)
        .with_context(|| format!("reading {PAPERDOLL_ITEM_FRAME}"))?;
    let rs = parse(&bytes, schema(), "PaperDollItemFrame")?;
    debug_assert_eq!(LAYOUTS.paperdoll_item_frame.fields, 3);
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(PaperDollItemFrameRow {
                name: str_at(&rs, r, 0)?,
                texture: str_at(&rs, r, 1)?,
                slot: u32_at(r, 2)?,
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
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_paperdoll_item_frame(&mut chain).expect("PaperDollItemFrame.dbc");
        assert_eq!(cat.find("HeadSlot").map(|r| r.slot), Some(1));
        assert_eq!(cat.find("Bag12").map(|r| r.slot), Some(75));
    }
}
