//! `ChrRaces.dbc`'s creature type: what a player's race reads as, the last stage of the
//! creature-type resolver. The runtime uses a frozen copy, `creature_type::race_creature_type` in
//! `benilla-app`; this loader backs the test that pins it.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};

use crate::dbc::{parse, u32_at};

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";

/// `CreatureType`, column 8 in 12340: after `BaseLanguage` (column 7, `record + 0x1c`), 7
/// (Humanoid) on every playable race. Column 9 is the resurrection-sickness spell, 15007.
const CHR_RACES_CREATURE_TYPE: usize = 8;

/// Each `ChrRaces.dbc` row's creature type by race id; a race without a row is absent.
pub fn load_race_creature_types(chain: &mut Chain) -> Result<HashMap<u8, u32>> {
    let bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    let schema = crate::characters::chr_races_schema(&crate::dbc_layout::LAYOUTS.chr_races);
    let rs = parse(&bytes, schema, "ChrRaces")?;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            let race = u8::try_from(u32_at(r, 0)?).ok()?;
            Some((race, u32_at(r, CHR_RACES_CREATURE_TYPE)?))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 21 rows, every one Humanoid (7), the rows `race_creature_type` freezes; column 9 beside the
    /// type would read 15007.
    #[test]
    fn every_shipped_race_reads_humanoid() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_race_creature_types(&mut chain).expect("load ChrRaces");
        assert_eq!(rows.len(), 21, "ChrRaces.dbc row count");
        for race in 1..=21u8 {
            assert_eq!(rows.get(&race), Some(&7), "race {race}");
        }
        for race in [0u8, 22, 255] {
            assert!(!rows.contains_key(&race), "race {race} has no row");
        }
    }
}
