//! `SpellVisualKitModelAttach.dbc`: a kit's extra models, walked after its slots (`0x745a7e`).

use std::collections::HashMap;

use anyhow::Result;
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, u32_at};

pub(crate) const SPELL_VISUAL_KIT_MODEL_ATTACH: &str =
    "DBFilesClient\\SpellVisualKitModelAttach.dbc";

const FIELDS: usize = 10;

/// The attachment id that plants the model at the owner in world space (`0x745b95`).
pub const MODEL_ATTACH_WORLD: i32 = -1;

/// One row: a `SpellVisualEffectName` model at an attachment, moved and turned in that frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModelAttach {
    /// Column 0; the effect add's same-slot replace compares the row itself (`0x6f8a60`).
    pub id: u32,
    /// Column 2, the `SpellVisualEffectName` id.
    pub effect: u32,
    /// Column 3, the M2 attachment id, or [`MODEL_ATTACH_WORLD`].
    pub attachment: i32,
    /// Columns 4-6, in the attachment's frame; y is negated on use (`0x6f84f0`).
    pub offset: [f32; 3],
    /// Columns 7-9 in radians: -yaw about z, then pitch about y, then roll about x (`0x6f84f0`).
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

impl ModelAttach {
    /// The offset as the effect matrix takes it, y negated (`0x6f85e0`).
    pub fn applied_offset(&self) -> [f32; 3] {
        let [x, y, z] = self.offset;
        [x, -y, z]
    }
}

pub(crate) fn model_attach_schema() -> Schema {
    let mut s = Schema::new("SpellVisualKitModelAttach");
    for i in 0..FIELDS {
        let ty = match i {
            0..=2 => FieldType::UInt32,
            3 => FieldType::Int32,
            _ => FieldType::Float32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// The rows by parent kit in id order, dropping parent 0 and parents past the last kit (`0x7fbfa6`).
pub(crate) fn parse_model_attach(
    bytes: &[u8],
    max_kit_id: u32,
) -> Result<HashMap<u32, Box<[ModelAttach]>>> {
    let set = parse(
        bytes,
        model_attach_schema(),
        "SpellVisualKitModelAttach.dbc",
    )?;
    let mut rows: Vec<(u32, ModelAttach)> = set
        .records()
        .iter()
        .filter_map(|r| {
            let f = |i| f32_at(r, i).unwrap_or(0.0);
            let parent = u32_at(r, 1)?;
            (parent != 0 && parent <= max_kit_id).then(|| {
                (
                    parent,
                    ModelAttach {
                        id: u32_at(r, 0).unwrap_or(0),
                        effect: u32_at(r, 2).unwrap_or(0),
                        attachment: i32_at(r, 3).unwrap_or(MODEL_ATTACH_WORLD),
                        offset: [f(4), f(5), f(6)],
                        yaw: f(7),
                        pitch: f(8),
                        roll: f(9),
                    },
                )
            })
        })
        .collect();
    rows.sort_by_key(|(_, a)| a.id);
    let mut by_kit: HashMap<u32, Vec<ModelAttach>> = HashMap::new();
    for (parent, row) in rows {
        by_kit.entry(parent).or_default().push(row);
    }
    Ok(by_kit
        .into_iter()
        .map(|(k, v)| (k, v.into_boxed_slice()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wdbc(rows: &[[u32; FIELDS]]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"WDBC");
        b.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        b.extend_from_slice(&(FIELDS as u32).to_le_bytes());
        b.extend_from_slice(&(FIELDS as u32 * 4).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        for r in rows {
            for v in r {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        b
    }

    /// The width 12340's loader checks (`reference/3.3.5a-dbc.tsv`).
    #[test]
    fn the_schema_holds_the_reference_width() {
        let s = model_attach_schema();
        assert_eq!((s.expanded_len(), s.record_width()), (10, 40));
    }

    #[test]
    fn rows_group_by_parent_in_id_order_and_drop_orphans() {
        let f = |v: f32| v.to_bits();
        let bytes = wdbc(&[
            [
                9,
                7,
                300,
                22,
                f(0.5),
                f(-0.25),
                f(1.0),
                f(0.1),
                f(0.2),
                f(0.3),
            ],
            [3, 7, 301, u32::MAX, 0, 0, 0, 0, 0, 0],
            [4, 0, 302, 19, 0, 0, 0, 0, 0, 0],
            [5, 99, 303, 19, 0, 0, 0, 0, 0, 0],
        ]);
        let by_kit = parse_model_attach(&bytes, 50).expect("parse");
        assert_eq!(
            by_kit.len(),
            1,
            "parent 0 and a parent past the last kit drop"
        );
        let rows = &by_kit[&7];
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [3, 9]);
        assert_eq!(rows[0].attachment, MODEL_ATTACH_WORLD);
        assert_eq!(rows[1].attachment, 22);
        assert_eq!(
            rows[1].applied_offset(),
            [0.5, 0.25, 1.0],
            "y negated on use"
        );
        assert_eq!((rows[1].yaw, rows[1].pitch, rows[1].roll), (0.1, 0.2, 0.3));
    }

    /// Every installed row lands under its parent in id order, and the kit columns read their raw values.
    #[test]
    fn the_installed_kit_tables_load_whole() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = crate::load_spell_visual_catalog(&mut chain).expect("load");
        let mut raw = |name: &str, schema| {
            crate::dbc::parse(&chain.read_file(name).expect(name), schema, name).expect(name)
        };
        let attach = raw(SPELL_VISUAL_KIT_MODEL_ATTACH, model_attach_schema());
        let kits = raw(super::super::SPELL_VISUAL_KIT, super::super::kit_schema());
        let mut kept = 0;
        for r in attach.records() {
            let (id, parent) = (u32_at(r, 0).unwrap(), u32_at(r, 1).unwrap());
            if cat.kit(parent).is_none() {
                continue;
            }
            kept += 1;
            let rows = cat.model_attachments(parent);
            let row = rows
                .iter()
                .find(|a| a.id == id)
                .expect("row under its parent");
            assert_eq!(row.effect, u32_at(r, 2).unwrap());
            assert_eq!(row.attachment, i32_at(r, 3).unwrap());
            assert!(rows.windows(2).all(|w| w[0].id < w[1].id), "id order");
        }
        assert!(kept > 0, "the table has rows");
        for r in kits.records() {
            let kit = cat.kit(u32_at(r, 0).unwrap()).expect("kit");
            assert_eq!(
                kit.start_anim.map(i32::from),
                Some(i32_at(r, 1).unwrap()).filter(|a| *a >= 0)
            );
            for (i, col) in [9, 10].into_iter().enumerate() {
                let v = u32_at(r, col).unwrap();
                assert_eq!(
                    kit.weapon_effects[i],
                    (v != 0 && v != u32::MAX).then_some(v)
                );
            }
        }
        let areas = raw(
            super::super::area_model::SPELL_VISUAL_KIT_AREA_MODEL,
            super::super::area_model::area_model_schema(),
        );
        for (i, r) in areas.records().iter().enumerate() {
            let name = crate::dbc::str_at(&areas, r, 1).unwrap_or_default();
            assert_eq!(cat.area_model(i as u8), name, "row {i} by index");
        }
        assert_eq!(
            cat.area_model(u8::MAX),
            cat.area_model(0),
            "past the table: row 0"
        );
    }
}
