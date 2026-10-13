//! `Talent.dbc` and `TalentTab.dbc`: each class's three talent tabs, and each tab's talents with
//! grid seat, rank spells and prerequisites; `CMSG_LEARN_TALENT` carries a row id from here. The
//! columns are TrinityCore 3.3.5's `TalentEntry` and `TalentTabEntry`: 12340's `Talent` adds a
//! two-word category mask after 5875's 21 columns, and its `TalentTab` name runs over sixteen
//! locales and a mask before the icon, masks, pet-talent mask, order and background.
//!
//! Order matters: `GetTalentInfo(tab, index)` indexes a tab's talents in native row order
//! (`TalentTabInfo[+0xC] + (index−1)*0x54`), never a (tier, column) sort, and a class's tabs come
//! in file order among rows matching both masks (`0x4f2e50`), never by `orderIndex` (column 22).
//! The three hunter-pet tabs carry neither mask, so no race and class reaches them here.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const TALENT: &str = "DBFilesClient\\Talent.dbc";
const TALENT_TAB: &str = "DBFilesClient\\TalentTab.dbc";

const TALENT_FIELDS: usize = 23;
const COL_T_TAB: usize = 1;
const COL_T_ROW: usize = 2;
const COL_T_COL: usize = 3;
const COL_T_RANK0: usize = 4; // ..=8, five rank spells
const COL_T_PREREQ: usize = 13; // ..=15
const COL_T_PREREQ_RANK: usize = 16; // ..=18
const COL_T_FLAGS: usize = 19;
const COL_T_REQUIRED_SPELL: usize = 20;

const TALENT_TAB_FIELDS: usize = 24;
const COL_TT_NAME_ENUS: usize = 1;
const COL_TT_SPELL_ICON: usize = 18;
const COL_TT_RACE_MASK: usize = 19;
const COL_TT_CLASS_MASK: usize = 20;
const COL_TT_BACKGROUND: usize = 23;

/// The rank ceiling: `Talent.dbc` has nine rank columns, and no 12340 talent fills past five.
pub const MAX_TALENT_RANK: usize = 5;

/// One `Talent.dbc` row: a talent's grid seat, rank spells and prerequisites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Talent {
    pub id: u32,
    /// The owning `TalentTab.dbc` id.
    pub tab: u32,
    /// Tier, 0-based (the reference grid draws it 1-based).
    pub row: u32,
    /// Column, 0-based (grid width is 4).
    pub col: u32,
    /// The spell taught at each rank; a talent with `n` ranks fills exactly the first `n`.
    pub ranks: [u32; MAX_TALENT_RANK],
    /// Prerequisite talents and the 0-based rank each needs, the three `Talent.dbc` slots that
    /// name one (`GetTalentPrereqs` walks all three, `0x5c7ed0`). Every shipped prerequisite is
    /// in the same tab, the only arrows the reference frame draws.
    pub prereqs: Vec<(u32, u32)>,
    /// A spell that must be known regardless of talents (0 = none).
    pub required_spell: u32,
    /// Flags bit 0 (`TalentRec+0x4c`), the Lua API's `isExceptional`: an activatable ability.
    pub exceptional: bool,
}

impl Talent {
    /// How many ranks this talent has, the count of populated rank spells.
    pub fn max_rank(&self) -> u32 {
        self.ranks.iter().take_while(|&&s| s != 0).count() as u32
    }
}

/// One `TalentTab.dbc` row, a class page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TalentTabInfo {
    pub id: u32,
    pub name: String,
    /// `raceMask` and `classMask`, bit `1 << (id − 1)`; the client admits a tab only when both
    /// match (2047, every race, on all class tabs but the Death Knight's `0xfffc07ff`).
    pub race_mask: u32,
    pub class_mask: u32,
    /// The `Interface\TalentFrame\<background>-…` art base ("MageFire").
    pub background: String,
    /// `SpellIconID` through `SpellIcon.dbc`, `GetTalentTabInfo`'s icon (`0x5c61c3`); `None`
    /// when it names no row.
    pub icon: Option<String>,
}

/// Talent tabs by class and talents by tab.
pub struct TalentCatalog {
    tabs: Vec<TalentTabInfo>,
    /// Tab id to its talents in native row order, the order `GetTalentInfo` indexes.
    by_tab: HashMap<u32, Vec<Talent>>,
}

impl TalentCatalog {
    /// A race and class's tabs (unit bytes, 1-based) in file order, as `0x4f2e50` filters them.
    pub fn tabs_for_class(&self, race: u8, class: u8) -> Vec<&TalentTabInfo> {
        if !(1..=32).contains(&class) || !(1..=32).contains(&race) {
            return Vec::new();
        }
        let race_bit = 1u32 << (race - 1);
        let class_bit = 1u32 << (class - 1);
        self.tabs
            .iter()
            .filter(|t| t.class_mask & class_bit != 0 && t.race_mask & race_bit != 0)
            .collect()
    }

    /// A tab's talents in native row order, the display and index order.
    pub fn talents_in_tab(&self, tab_id: u32) -> &[Talent] {
        self.by_tab.get(&tab_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// One talent by id, for resolving a prerequisite arrow.
    pub fn talent(&self, tab_id: u32, talent_id: u32) -> Option<&Talent> {
        self.talents_in_tab(tab_id)
            .iter()
            .find(|t| t.id == talent_id)
    }

    pub fn len(&self) -> usize {
        self.by_tab.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tab.is_empty()
    }
}

fn talent_schema() -> Schema {
    let mut s = Schema::new("Talent");
    for i in 0..TALENT_FIELDS {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

fn talent_tab_schema() -> Schema {
    let mut s = Schema::new("TalentTab");
    for i in 0..TALENT_TAB_FIELDS {
        if i == COL_TT_NAME_ENUS {
            s.add_field(SchemaField::new("NameEnUs", FieldType::String));
        } else if i == COL_TT_BACKGROUND {
            s.add_field(SchemaField::new("BackgroundFile", FieldType::String));
        } else {
            s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
        }
    }
    s
}

/// Load both tables off the patch chain.
pub fn load_talent_catalog(chain: &mut Chain) -> Result<TalentCatalog> {
    let tab_bytes = chain
        .read_file(TALENT_TAB)
        .context("reading TalentTab.dbc")?;
    let tab_set = parse(&tab_bytes, talent_tab_schema(), "TalentTab.dbc")?;
    let icons = crate::dbc::load_spell_icon_map(chain)?;
    let mut tabs = Vec::new();
    for r in tab_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        tabs.push(TalentTabInfo {
            id,
            name: str_at(&tab_set, r, COL_TT_NAME_ENUS).unwrap_or_default(),
            race_mask: u32_at(r, COL_TT_RACE_MASK).unwrap_or(0),
            class_mask: u32_at(r, COL_TT_CLASS_MASK).unwrap_or(0),
            background: str_at(&tab_set, r, COL_TT_BACKGROUND).unwrap_or_default(),
            icon: u32_at(r, COL_TT_SPELL_ICON).and_then(|i| icons.get(&i).cloned()),
        });
    }

    let t_bytes = chain.read_file(TALENT).context("reading Talent.dbc")?;
    let t_set = parse(&t_bytes, talent_schema(), "Talent.dbc")?;
    let mut by_tab: HashMap<u32, Vec<Talent>> = HashMap::new();
    for r in t_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut ranks = [0u32; MAX_TALENT_RANK];
        for (i, rank) in ranks.iter_mut().enumerate() {
            *rank = u32_at(r, COL_T_RANK0 + i).unwrap_or(0);
        }
        let t = Talent {
            id,
            tab: u32_at(r, COL_T_TAB).unwrap_or(0),
            row: u32_at(r, COL_T_ROW).unwrap_or(0),
            col: u32_at(r, COL_T_COL).unwrap_or(0),
            ranks,
            prereqs: (0..3)
                .filter_map(|i| {
                    let talent = u32_at(r, COL_T_PREREQ + i).unwrap_or(0);
                    (talent != 0).then(|| (talent, u32_at(r, COL_T_PREREQ_RANK + i).unwrap_or(0)))
                })
                .collect(),
            required_spell: u32_at(r, COL_T_REQUIRED_SPELL).unwrap_or(0),
            exceptional: u32_at(r, COL_T_FLAGS).unwrap_or(0) & 1 != 0,
        };
        // Native row order, never sorted.
        by_tab.entry(t.tab).or_default().push(t);
    }

    Ok(TalentCatalog { tabs, by_tab })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_talent_tables_hold_the_grid_and_prereq_invariants() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_talent_catalog(&mut chain).expect("load Talent/TalentTab");
        assert!(
            cat.len() > 300,
            "the 12340 talent table has hundreds of rows, got {}",
            cat.len()
        );

        const HUMAN: u8 = 1;
        const MAGE: u8 = 8;
        let tabs = cat.tabs_for_class(HUMAN, MAGE);
        assert_eq!(
            tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["Arcane", "Fire", "Frost"]
        );
        assert_eq!(
            tabs.iter()
                .map(|t| t.background.as_str())
                .collect::<Vec<_>>(),
            vec!["MageArcane", "MageFire", "MageFrost"]
        );
        assert!(
            tabs.iter().all(|t| t
                .icon
                .as_deref()
                .is_some_and(|i| i.starts_with("Interface"))),
            "every mage tab names an icon"
        );
        // Every class tab's raceMask covers races 1-11 alone, so race 12 admits nothing.
        assert!(
            cat.tabs_for_class(12, MAGE).is_empty(),
            "race bit 12 is outside every mask"
        );
        // Every class has pages: 1-9 (6 the Death Knight) and 11 (druid).
        for class in [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 11] {
            let n = cat.tabs_for_class(HUMAN, class).len();
            assert_eq!(n, 3, "class {class} has three talent pages, got {n}");
        }

        for tabs in cat.tabs_for_class(HUMAN, MAGE) {
            let talents = cat.talents_in_tab(tabs.id);
            assert!(!talents.is_empty());
            let seats: Vec<(u32, u32)> = talents.iter().map(|t| (t.row, t.col)).collect();
            let mut sorted = seats.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                seats, sorted,
                "the shipped data's (row, col)-ordered authoring"
            );
        }

        let all_ids: std::collections::HashSet<u32> =
            cat.by_tab.values().flatten().map(|t| t.id).collect();
        for t in cat.by_tab.values().flatten() {
            assert!(
                t.col < 4,
                "talent {} col {} inside the 4-wide grid",
                t.id,
                t.col
            );
            assert!(t.row < 11, "talent {} row {} inside 11 tiers", t.id, t.row);
            let n = t.max_rank() as usize;
            assert!(n >= 1, "talent {} has at least one rank", t.id);
            assert!(
                t.ranks[n..].iter().all(|&s| s == 0),
                "talent {} rank array is prefix-shaped",
                t.id
            );
            for &(prereq, rank) in &t.prereqs {
                // 12340 ships two prerequisites that name no row: 1756 → 1409 and 1993 → 1994.
                if !all_ids.contains(&prereq) {
                    assert!(
                        matches!((t.id, prereq), (1756, 1409) | (1993, 1994)),
                        "talent {} prereq {prereq} resolves",
                        t.id
                    );
                    continue;
                }
                assert!(
                    cat.talent(t.tab, prereq).is_some(),
                    "talent {} prereq {prereq} lives in the same tab",
                    t.id
                );
                assert!(
                    (rank as usize) < MAX_TALENT_RANK,
                    "talent {} prereq rank {rank} is 0-based sane",
                    t.id
                );
            }
        }
    }
}
