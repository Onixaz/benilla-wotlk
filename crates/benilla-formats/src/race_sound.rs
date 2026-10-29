//! `ChrRaces.dbc` column 3: the sound the client plays on `SMSG_EXPLORATION_EXPERIENCE`, by the
//! player's race (row `[race]`, offset `0xc`).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};

use crate::dbc::{parse, u32_at};

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";

/// Race (the `UNIT_FIELD_BYTES_0` race byte) → exploration `SoundEntries` id; a 0 is absent.
pub struct ExplorationSoundCatalog {
    by_race: HashMap<u32, u32>,
}

impl ExplorationSoundCatalog {
    pub fn kit(&self, race: u32) -> Option<u32> {
        self.by_race.get(&race).copied()
    }

    pub fn len(&self) -> usize {
        self.by_race.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_race.is_empty()
    }
}

/// Load the race → exploration-sound map off the patch chain.
pub fn load_exploration_sound_catalog(chain: &mut Chain) -> Result<ExplorationSoundCatalog> {
    let bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    let schema = crate::characters::chr_races_schema(&crate::dbc_layout::LAYOUTS.chr_races);
    let rs = parse(&bytes, schema, "ChrRaces")?;
    let mut by_race = HashMap::new();
    for r in rs.records() {
        let (Some(id), Some(kit)) = (u32_at(r, 0), u32_at(r, 3)) else {
            continue;
        };
        if kit != 0 {
            by_race.insert(id, kit);
        }
    }
    Ok(ExplorationSoundCatalog { by_race })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_chr_races_exploration_kits() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_exploration_sound_catalog(&mut chain).expect("load ChrRaces");
        assert_eq!(cat.len(), 10, "the ten playable races, Goblin (9) none");
        for race in (1..=11u32).filter(|&r| r != 9) {
            assert!(
                cat.kit(race).is_some(),
                "race {race} has no exploration sound"
            );
        }
        // The shipped kits run 4140-4147; Human and Orc guard the column position, and Blood Elf
        // and Draenei reuse the Undead's and the Human's.
        assert_eq!(cat.kit(1), Some(4140));
        assert_eq!(cat.kit(2), Some(4141));
        assert_eq!(cat.kit(10), Some(4142));
        assert_eq!(cat.kit(11), Some(4140));
    }
}
