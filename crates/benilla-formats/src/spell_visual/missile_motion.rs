//! `SpellMissileMotion.dbc`: the Lua motion scripts a missile runs each frame (`0x6ff0a0`), which
//! move, turn and scale its model off its straight path and may change its speed (`0x700350`).

use std::collections::HashMap;

use anyhow::Result;
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

pub(crate) const SPELL_MISSILE_MOTION: &str = "DBFilesClient\\SpellMissileMotion.dbc";

/// The script's twelve number arguments, in call order (`0xada7c0`).
pub const MOTION_ARGS: [&str; 12] = [
    "progress",
    "time",
    "missileIndex",
    "missileCount",
    "distanceToFirePos",
    "distanceToImpactPos",
    "startDistance",
    "totalDistance",
    "rand1",
    "rand2",
    "rand3",
    "spellID",
];

/// The script's twelve number results, each a local preset to 0.0 (`0xada7f4`).
pub const MOTION_RETURNS: [&str; 12] = [
    "transAngle",
    "transMag",
    "transRight",
    "transFront",
    "transUp",
    "modelYaw",
    "modelPitch",
    "modelRoll",
    "speedAbs",
    "speedScalar",
    "speedOffset",
    "scale",
];

/// One `SpellMissileMotion.dbc` row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissileMotionRec {
    /// Column 1, the chunk name the script compiles under.
    pub name: String,
    /// Column 2, the Lua body run between the argument list and the results.
    pub script: String,
    /// Column 3; no 12340 reader.
    pub flags: u32,
    /// Column 4: the copies the missile flies, each with its own index; below 1 flies one
    /// (`0x7336b3`).
    pub missile_count: u32,
}

impl MissileMotionRec {
    /// The copies a missile with this motion flies, at least one (`0x7336b3`).
    pub fn copies(&self) -> u32 {
        if self.missile_count as i32 >= 1 {
            self.missile_count
        } else {
            1
        }
    }

    /// The chunk 12340 compiles for the row (`0x7587d0`): a function of [`MOTION_ARGS`] that presets
    /// [`MOTION_RETURNS`] to 0.0, runs the body and returns them.
    pub fn chunk(&self) -> String {
        let mut s = String::with_capacity(self.script.len() + 512);
        s.push_str("return function(");
        s.push_str(&MOTION_ARGS.join(", "));
        s.push_str(")\n");
        for r in MOTION_RETURNS {
            s.push_str("local ");
            s.push_str(r);
            s.push_str(" = 0.0\n");
        }
        s.push_str(&self.script);
        s.push_str("\nreturn ");
        s.push_str(&MOTION_RETURNS.join(", "));
        s.push_str("\nend\n");
        s
    }
}

pub(crate) fn spell_missile_motion_schema() -> Schema {
    let mut s = Schema::new("SpellMissileMotion");
    for i in 0..5 {
        let ty = if matches!(i, 1 | 2) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// `SpellMissileMotion.dbc`'s rows by id.
pub(crate) fn parse_missile_motions(bytes: &[u8]) -> Result<HashMap<u32, MissileMotionRec>> {
    let set = parse(
        bytes,
        spell_missile_motion_schema(),
        "SpellMissileMotion.dbc",
    )?;
    Ok(set
        .records()
        .iter()
        .filter_map(|r| {
            Some((
                u32_at(r, 0)?,
                MissileMotionRec {
                    name: str_at(&set, r, 1).unwrap_or_default(),
                    script: str_at(&set, r, 2).unwrap_or_default(),
                    flags: u32_at(r, 3).unwrap_or(0),
                    missile_count: u32_at(r, 4).unwrap_or(0),
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
        let s = spell_missile_motion_schema();
        assert_eq!((s.expanded_len(), s.record_width()), (5, 20));
    }

    /// The chunk wraps the body the way `0x7587d0` builds it: arguments, preset results, body,
    /// results returned.
    #[test]
    fn the_chunk_wraps_the_body_in_the_reference_function() {
        let rec = MissileMotionRec {
            name: "Forward Spin".into(),
            script: "modelPitch = time * 720".into(),
            flags: 0,
            missile_count: 0,
        };
        let chunk = rec.chunk();
        assert!(chunk.starts_with(
            "return function(progress, time, missileIndex, missileCount, distanceToFirePos, \
             distanceToImpactPos, startDistance, totalDistance, rand1, rand2, rand3, spellID)\n\
             local transAngle = 0.0\n"
        ));
        assert!(chunk.contains("local scale = 0.0\nmodelPitch = time * 720\nreturn transAngle, "));
        assert!(chunk.ends_with("speedOffset, scale\nend\n"));
        assert_eq!(rec.copies(), 1, "a zero count flies one");
    }

    /// The motions the `SpellVisual` rows name load; an id with no row flies unscripted
    /// (`0x6ff0e6`), and the data has such ids.
    #[test]
    fn real_motion_ids_resolve_to_scripts() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let visuals = crate::load_spell_visual_catalog(&mut chain).expect("visuals");
        let named: Vec<u32> = visuals
            .visuals()
            .map(|(_, s)| s.missile_motion)
            .filter(|&m| m != 0)
            .collect();
        let resolved = named
            .iter()
            .filter(|&&m| visuals.missile_motion(m).is_some())
            .count();
        assert!(
            resolved * 2 > named.len(),
            "{resolved} of {} resolve",
            named.len()
        );
    }
}
