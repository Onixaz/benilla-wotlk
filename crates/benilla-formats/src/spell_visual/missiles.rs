//! `SpellMissile.dbc`: a trajectory missile's flight (`0x700880`) and what it collides with
//! (`0x6fc4a0`).

use std::collections::HashMap;

use anyhow::Result;
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};

pub(crate) const SPELL_MISSILE: &str = "DBFilesClient\\SpellMissile.dbc";

/// The gravity a missile with no row falls under (`0x700880`).
pub const DEFAULT_MISSILE_GRAVITY: f32 = 19.291_105;

/// One `SpellMissile.dbc` row.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpellMissileRec {
    /// Column 1: `0x1` casts with a flown aim (`0x80b39d`); `0x4` and `0x8` collide with hostile
    /// and friendly players, `0x10` and `0x20` with hostile and friendly creatures that take
    /// missile hits (`0x6fc4a0`).
    pub flags: u32,
    /// Columns 2-3, the pitch an aim with no vehicle pitch takes the middle of.
    pub pitch_min: f32,
    pub pitch_max: f32,
    /// Columns 4-5, the speeds a flight with no packet speed falls back on, in order, and the
    /// range a vehicle's power picks from.
    pub speed_min: f32,
    pub speed_max: f32,
    /// Columns 6-11, the spread an aim adds to its facing, pitch and speed, each `[min, max]`.
    pub facing_spread: [f32; 2],
    pub pitch_spread: [f32; 2],
    pub speed_spread: [f32; 2],
    /// Column 12, the fall in yards per second squared.
    pub gravity: f32,
    /// Column 13, the longest flight an aim follows, seconds; 0 for the 20 s cap.
    pub max_duration: f32,
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
pub(crate) fn parse_missiles(bytes: &[u8]) -> Result<HashMap<u32, SpellMissileRec>> {
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
                    pitch_min: f(2),
                    pitch_max: f(3),
                    speed_min: f(4),
                    speed_max: f(5),
                    facing_spread: [f(6), f(7)],
                    pitch_spread: [f(8), f(9)],
                    speed_spread: [f(10), f(11)],
                    gravity: f(12),
                    max_duration: f(13),
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
