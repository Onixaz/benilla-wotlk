//! `SpellMissile.dbc`: a trajectory missile's flight (`0x700880`) and what it collides with
//! (`0x6fc4a0`).

use std::collections::HashMap;

use anyhow::Result;
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};

pub(super) const SPELL_MISSILE: &str = "DBFilesClient\\SpellMissile.dbc";

/// The gravity a missile with no row falls under (`0x700880`).
pub const DEFAULT_MISSILE_GRAVITY: f32 = 19.291_105;

/// One `SpellMissile.dbc` row. Columns 2-3 (the default pitch range), 6-11 (the facing, pitch and
/// speed randomisation) and 13 (the longest flight) shape the aim preview, which is not built.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpellMissileRec {
    /// Column 1: `0x4` and `0x8` collide with hostile and friendly players, `0x10` and `0x20`
    /// with hostile and friendly creatures that take missile hits (`0x6fc4a0`).
    pub flags: u32,
    /// Columns 4-5, the speeds a flight with no packet speed falls back on, in order.
    pub speed_min: f32,
    pub speed_max: f32,
    /// Column 12, the fall in yards per second squared.
    pub gravity: f32,
    /// Column 14, added to each unit's hit sphere.
    pub collision_radius: f32,
}

pub(crate) fn spell_missile_schema() -> Schema {
    let mut s = Schema::new("SpellMissile");
    for i in 0..15 {
        let ty = if i < 2 {
            FieldType::UInt32
        } else {
            FieldType::Float32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// `SpellMissile.dbc`'s rows by id.
pub(super) fn parse_missiles(bytes: &[u8]) -> Result<HashMap<u32, SpellMissileRec>> {
    let set = parse(bytes, spell_missile_schema(), "SpellMissile.dbc")?;
    Ok(set
        .records()
        .iter()
        .filter_map(|r| {
            let f = |i| f32_at(r, i).unwrap_or(0.0);
            Some((
                u32_at(r, 0)?,
                SpellMissileRec {
                    flags: u32_at(r, 1).unwrap_or(0),
                    speed_min: f(4),
                    speed_max: f(5),
                    gravity: f(12),
                    collision_radius: f(14),
                },
            ))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width 12340's loader checks (`reference/3.3.5a-dbc.tsv`).
    #[test]
    fn the_schema_holds_the_reference_width() {
        let s = spell_missile_schema();
        assert_eq!((s.expanded_len(), s.record_width()), (15, 60));
    }
}
