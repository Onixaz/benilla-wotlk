//! `Achievement.dbc`, `Achievement_Category.dbc` and `Achievement_Criteria.dbc`: the achievement
//! window's rows, categories and criteria, each in file order, the order 12340's verbs walk them
//! (`GetRecordByIndex`, and the per-achievement criteria lists `0x5b6df0` builds at game start).
//! Columns are 12340's `AchievementRec` (62), `Achievement_CategoryRec` (20) and
//! `Achievement_CriteriaRec` (31), each string a sixteen-locale run plus its mask, read at enUS.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, str_at, u32_at};
use crate::Chain;

const ACHIEVEMENT: &str = "DBFilesClient\\Achievement.dbc";
const CATEGORY: &str = "DBFilesClient\\Achievement_Category.dbc";
const CRITERIA: &str = "DBFilesClient\\Achievement_Criteria.dbc";

/// A sixteen-locale string and its flags word.
const LOCSTRING: usize = 17;

const A_FACTION: usize = 1;
const A_SUPERCEDES: usize = 3;
const A_TITLE: usize = 4;
const A_DESCRIPTION: usize = A_TITLE + LOCSTRING;
const A_CATEGORY: usize = A_DESCRIPTION + LOCSTRING;
const A_POINTS: usize = A_CATEGORY + 1;
const A_UI_ORDER: usize = A_CATEGORY + 2;
const A_FLAGS: usize = A_CATEGORY + 3;
const A_ICON: usize = A_CATEGORY + 4;
const A_REWARD: usize = A_CATEGORY + 5;
const A_MINIMUM_CRITERIA: usize = A_REWARD + LOCSTRING;
const A_SHARES_CRITERIA: usize = A_MINIMUM_CRITERIA + 1;
const A_FIELDS: usize = A_SHARES_CRITERIA + 1;

const C_PARENT: usize = 1;
const C_NAME: usize = 2;
const C_FIELDS: usize = C_NAME + LOCSTRING + 1;

const K_ACHIEVEMENT: usize = 1;
const K_TYPE: usize = 2;
const K_ASSET: usize = 3;
const K_QUANTITY: usize = 4;
const K_DESCRIPTION: usize = 9;
const K_FLAGS: usize = K_DESCRIPTION + LOCSTRING;
const K_TIMER_TIME: usize = K_FLAGS + 3;
const K_FIELDS: usize = K_FLAGS + 5;

/// One `Achievement.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Achievement {
    pub id: u32,
    /// -1 both sides, 0 Horde, 1 Alliance: the player's PvP faction index.
    pub faction: i32,
    /// The achievement this one follows in a series (0 = none).
    pub supercedes: u32,
    pub title: String,
    pub description: String,
    /// `Achievement_Category.dbc` id; -1 on the hidden rows.
    pub category: i32,
    pub points: u32,
    pub ui_order: u32,
    pub flags: u32,
    /// `SpellIcon.dbc` texture path, "" when the row names none.
    pub icon: String,
    pub reward: String,
    /// Criteria needed of a "N of these" achievement (0 = all).
    pub minimum_criteria: u32,
    /// The achievement whose criteria this one reads (0 = its own).
    pub shares_criteria: u32,
}

/// One `Achievement_Category.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AchievementCategory {
    pub id: i32,
    /// -1 for a root; 1 is the Statistics root.
    pub parent: i32,
    pub name: String,
}

/// One `Achievement_Criteria.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AchievementCriteria {
    pub id: u32,
    pub achievement: u32,
    pub kind: u32,
    pub asset: u32,
    pub quantity: u32,
    pub description: String,
    pub flags: u32,
    /// Seconds a timed criterion runs (0 = untimed).
    pub timer_time: u32,
}

/// The three tables, each in file order.
#[derive(Clone, Debug, Default)]
pub struct AchievementCatalog {
    pub achievements: Vec<Achievement>,
    pub categories: Vec<AchievementCategory>,
    pub criteria: Vec<AchievementCriteria>,
}

fn locstring_fields(s: &mut Schema, name: &str) {
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("{name}{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new(format!("{name}Flags"), FieldType::UInt32));
}

fn ints(s: &mut Schema, from: usize, to: usize) {
    for i in from..to {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::Int32));
    }
}

pub(crate) fn achievement_schema() -> Schema {
    let mut s = Schema::new("Achievement");
    ints(&mut s, 0, A_TITLE);
    locstring_fields(&mut s, "Title");
    locstring_fields(&mut s, "Description");
    ints(&mut s, A_CATEGORY, A_REWARD);
    locstring_fields(&mut s, "Reward");
    ints(&mut s, A_MINIMUM_CRITERIA, A_FIELDS);
    s
}

pub(crate) fn category_schema() -> Schema {
    let mut s = Schema::new("Achievement_Category");
    ints(&mut s, 0, C_NAME);
    locstring_fields(&mut s, "Name");
    ints(&mut s, C_NAME + LOCSTRING, C_FIELDS);
    s
}

pub(crate) fn criteria_schema() -> Schema {
    let mut s = Schema::new("Achievement_Criteria");
    ints(&mut s, 0, K_DESCRIPTION);
    locstring_fields(&mut s, "Description");
    ints(&mut s, K_FLAGS, K_FIELDS);
    s
}

/// Load the three tables off the patch chain, icons resolved through `SpellIcon.dbc`.
pub fn load_achievement_catalog(chain: &mut Chain) -> Result<AchievementCatalog> {
    let icons = crate::dbc::load_spell_icon_map(chain)?;

    let bytes = chain
        .read_file(ACHIEVEMENT)
        .context("reading Achievement.dbc")?;
    let set = parse(&bytes, achievement_schema(), "Achievement.dbc")?;
    let mut achievements = Vec::with_capacity(set.records().len());
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let int = |i| u32_at(r, i).unwrap_or(0);
        achievements.push(Achievement {
            id,
            faction: i32_at(r, A_FACTION).unwrap_or(-1),
            supercedes: int(A_SUPERCEDES),
            title: str_at(&set, r, A_TITLE).unwrap_or_default(),
            description: str_at(&set, r, A_DESCRIPTION).unwrap_or_default(),
            category: i32_at(r, A_CATEGORY).unwrap_or(-1),
            points: int(A_POINTS),
            ui_order: int(A_UI_ORDER),
            flags: int(A_FLAGS),
            icon: icons.get(&int(A_ICON)).cloned().unwrap_or_default(),
            reward: str_at(&set, r, A_REWARD).unwrap_or_default(),
            minimum_criteria: int(A_MINIMUM_CRITERIA),
            shares_criteria: int(A_SHARES_CRITERIA),
        });
    }

    let bytes = chain
        .read_file(CATEGORY)
        .context("reading Achievement_Category.dbc")?;
    let set = parse(&bytes, category_schema(), "Achievement_Category.dbc")?;
    let categories = set
        .records()
        .iter()
        .filter_map(|r| {
            Some(AchievementCategory {
                id: i32_at(r, 0)?,
                parent: i32_at(r, C_PARENT).unwrap_or(-1),
                name: str_at(&set, r, C_NAME).unwrap_or_default(),
            })
        })
        .collect();

    let bytes = chain
        .read_file(CRITERIA)
        .context("reading Achievement_Criteria.dbc")?;
    let set = parse(&bytes, criteria_schema(), "Achievement_Criteria.dbc")?;
    let criteria = set
        .records()
        .iter()
        .filter_map(|r| {
            let int = |i| u32_at(r, i).unwrap_or(0);
            Some(AchievementCriteria {
                id: u32_at(r, 0)?,
                achievement: int(K_ACHIEVEMENT),
                kind: int(K_TYPE),
                asset: int(K_ASSET),
                quantity: int(K_QUANTITY),
                description: str_at(&set, r, K_DESCRIPTION).unwrap_or_default(),
                flags: int(K_FLAGS),
                timer_time: int(K_TIMER_TIME),
            })
        })
        .collect();

    Ok(AchievementCatalog {
        achievements,
        categories,
        criteria,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn the_schemas_are_12340s_widths() {
        assert_eq!(achievement_schema().expanded_len(), 62);
        assert_eq!(category_schema().expanded_len(), 20);
        assert_eq!(criteria_schema().expanded_len(), 31);
    }

    #[test]
    fn the_real_tables_hold_together() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_achievement_catalog(&mut chain).expect("load achievements");
        assert!(cat.achievements.len() > 1000, "{}", cat.achievements.len());
        assert!(cat.criteria.len() > cat.achievements.len());

        let categories: HashSet<i32> = cat.categories.iter().map(|c| c.id).collect();
        // The Statistics root (`0x5b0d50` tests id 1) and its roots' -1 parent.
        assert!(categories.contains(&1));
        assert!(cat
            .categories
            .iter()
            .all(|c| c.parent == -1 || categories.contains(&c.parent)));
        assert!(cat.categories.iter().all(|c| !c.name.is_empty()));

        let achievements: HashSet<u32> = cat.achievements.iter().map(|a| a.id).collect();
        assert!(cat
            .achievements
            .iter()
            .all(|a| a.category == -1 || categories.contains(&a.category)));
        assert!(cat
            .achievements
            .iter()
            .all(|a| (-1..=1).contains(&a.faction)));
        assert!(cat
            .achievements
            .iter()
            .all(|a| a.shares_criteria == 0 || achievements.contains(&a.shares_criteria)));
        // Nearly every visible row names an icon and a title.
        let titled = cat
            .achievements
            .iter()
            .filter(|a| !a.title.is_empty() && a.icon.starts_with("Interface"))
            .count();
        assert!(titled * 10 > cat.achievements.len() * 9, "{titled}");
        // Most criteria belong to a real achievement; 12340 skips the rest (`0x5b6df0`).
        let owned = cat
            .criteria
            .iter()
            .filter(|c| achievements.contains(&c.achievement))
            .count();
        assert!(owned * 10 > cat.criteria.len() * 9, "{owned}");
        assert!(cat.criteria.iter().all(|c| c.kind <= 123));
        assert!(cat.criteria.iter().any(|c| c.timer_time > 0));
    }
}
