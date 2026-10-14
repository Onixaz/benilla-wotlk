//! The tables 12340's quest reward verbs read beside the quest template:
//!
//! - `QuestXP.dbc` (`ID` = quest level, `Difficulty[10]`): the base XP, indexed by the template's
//!   XP id (`0x5e0420` reads `row + 4 + xpId * 4`).
//! - `QuestFactionReward.dbc` (`ID` 1 gains, 2 losses, `Difficulty[10]`): the reputation a value
//!   id names (`0x58d050` picks row `(valueId < 0) + 1` and column `|valueId|`).
//! - `CharTitles.dbc` (`ID`, `Condition_ID`, `Name`, `Name1`, `Mask_ID`, the names loc-strings):
//!   a title reward's text, `Name1` the female form (`0x5e4c10`).
//! - `TeamContributionPoints.dbc` (`ID`, `Data`): the honor a multiplier scales, one row per level
//!   (`0x7554f0` reads row `level - 1`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, str_at, u32_at};
use crate::Chain;

/// The four quest reward tables.
#[derive(Debug, Default)]
pub struct QuestRewardTables {
    xp: HashMap<u32, [u32; 10]>,
    reputation: HashMap<u32, [i32; 10]>,
    titles: HashMap<u32, (String, String)>,
    honor_per_level: HashMap<u32, f32>,
}

impl QuestRewardTables {
    /// The base XP of a quest of `quest_level` with XP id `xp_id`, `None` when either is out of
    /// the table.
    pub fn base_xp(&self, quest_level: u32, xp_id: u32) -> Option<u32> {
        self.xp
            .get(&quest_level)
            .and_then(|row| row.get(xp_id as usize))
            .copied()
    }

    /// The reputation a value id grants: row 1 for a gain, row 2 for a loss, column `|value_id|`.
    pub fn reputation(&self, value_id: i32) -> Option<i32> {
        let row = if value_id < 0 { 2 } else { 1 };
        let col = value_id.unsigned_abs() as usize;
        self.reputation.get(&row).and_then(|r| r.get(col)).copied()
    }

    /// A title's `(Name, Name1)`, the second the female form; either may be empty.
    pub fn title(&self, id: u32) -> Option<(&str, &str)> {
        self.titles.get(&id).map(|(m, f)| (m.as_str(), f.as_str()))
    }

    /// `TeamContributionPoints.dbc`'s value for `level`.
    pub fn honor_at_level(&self, level: u32) -> Option<f32> {
        self.honor_per_level.get(&level).copied()
    }

    /// A table set from rows, for fixtures.
    pub fn from_rows(
        xp: HashMap<u32, [u32; 10]>,
        reputation: HashMap<u32, [i32; 10]>,
        titles: HashMap<u32, (String, String)>,
        honor_per_level: HashMap<u32, f32>,
    ) -> Self {
        Self {
            xp,
            reputation,
            titles,
            honor_per_level,
        }
    }
}

fn read(chain: &mut Chain, file: &str) -> Result<Vec<u8>> {
    chain
        .read_file(file)
        .with_context(|| format!("reading {file}"))
}

fn id_and_ten(name: &str, signed: bool) -> Schema {
    let mut s = Schema::new(name);
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..10 {
        let t = if signed {
            FieldType::Int32
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("Difficulty{i}"), t));
    }
    s.set_key_field("ID");
    s
}

pub(crate) fn quest_xp_schema() -> Schema {
    id_and_ten("QuestXP", false)
}

pub(crate) fn quest_faction_reward_schema() -> Schema {
    id_and_ten("QuestFactionReward", true)
}

/// `CharTitles.dbc`: 37 columns, `Name` at 2 and `Name1` at 19, each a 17-column loc-string.
pub(crate) fn titles_schema() -> Schema {
    let mut s = Schema::new("CharTitles");
    for i in 0..37 {
        let t = if i == 2 || i == 19 {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("c{i}"), t));
    }
    s
}

pub(crate) fn honor_schema() -> Schema {
    let mut s = Schema::new("TeamContributionPoints");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Data", FieldType::Float32));
    s
}

/// Load the four tables through the patch chain.
pub fn load_quest_reward_tables(chain: &mut Chain) -> Result<QuestRewardTables> {
    let mut out = QuestRewardTables::default();

    let rs = parse(
        &read(chain, "DBFilesClient\\QuestXP.dbc")?,
        quest_xp_schema(),
        "QuestXP",
    )?;
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            out.xp
                .insert(id, std::array::from_fn(|i| u32_at(r, i + 1).unwrap_or(0)));
        }
    }

    let rs = parse(
        &read(chain, "DBFilesClient\\QuestFactionReward.dbc")?,
        quest_faction_reward_schema(),
        "QuestFactionReward",
    )?;
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            out.reputation
                .insert(id, std::array::from_fn(|i| i32_at(r, i + 1).unwrap_or(0)));
        }
    }

    let rs = parse(
        &read(chain, "DBFilesClient\\CharTitles.dbc")?,
        titles_schema(),
        "CharTitles",
    )?;
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            let male = str_at(&rs, r, 2).unwrap_or_default();
            let female = str_at(&rs, r, 19).unwrap_or_default();
            out.titles.insert(id, (male, female));
        }
    }

    let rs = parse(
        &read(chain, "DBFilesClient\\TeamContributionPoints.dbc")?,
        honor_schema(),
        "TeamContributionPoints",
    )?;
    for r in rs.records() {
        if let (Some(id), Some(v)) = (u32_at(r, 0), f32_at(r, 1)) {
            out.honor_per_level.insert(id, v);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped tables load in the 12340 layout: a level-1 quest of XP id 1 grants a positive
    /// base, reputation row 1 gains and row 2 loses, and titles carry a name.
    #[test]
    fn the_shipped_reward_tables_load() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).unwrap();
        let t = load_quest_reward_tables(&mut chain).unwrap();
        assert!(t.base_xp(1, 1).is_some_and(|xp| xp > 0));
        assert!(t.reputation(1).is_some_and(|r| r > 0));
        assert!(t.reputation(-1).is_some_and(|r| r < 0));
        assert!(t.titles.values().any(|(m, _)| !m.is_empty()));
        assert!(t.honor_at_level(1).is_some_and(|h| h > 0.0));
    }

    #[test]
    fn a_value_id_picks_its_row_and_column() {
        let mut rep = HashMap::new();
        rep.insert(1, std::array::from_fn(|i| i as i32 * 10));
        rep.insert(2, std::array::from_fn(|i| -(i as i32) * 10));
        let t = QuestRewardTables::from_rows(HashMap::new(), rep, HashMap::new(), HashMap::new());
        assert_eq!(t.reputation(3), Some(30));
        assert_eq!(t.reputation(-3), Some(-30));
        assert_eq!(t.reputation(12), None);
    }
}
