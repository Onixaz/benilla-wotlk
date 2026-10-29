//! The dungeon finder's three tables as 12340 lays them out: `LfgDungeons.dbc` (49 columns in
//! 0xc4-byte records), `LfgDungeonGroup.dbc` (21 in 0x54) and `LfgDungeonExpansion.dbc` (8 in
//! 0x20). Each localized string is 17 columns, the enUS slot first; the readers are the client's
//! dungeon-finder code (`0x552850`-`0x55d440`) and its Lua bindings.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{i32_at, parse, str_at};

const LFG_DUNGEONS: &str = "DBFilesClient\\LfgDungeons.dbc";
const LFG_DUNGEON_GROUP: &str = "DBFilesClient\\LfgDungeonGroup.dbc";
const LFG_DUNGEON_EXPANSION: &str = "DBFilesClient\\LfgDungeonExpansion.dbc";

/// One `LfgDungeons.dbc` row, the packed record's fields (`+0x00`-`+0x40`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgDungeon {
    pub id: i32,
    pub name: String,
    pub min_level: i32,
    pub max_level: i32,
    pub target_level: i32,
    pub target_level_min: i32,
    pub target_level_max: i32,
    pub map_id: i32,
    pub difficulty: i32,
    /// `0x2` lists the row in the dungeon window, `0x4` marks a seasonal (holiday) dungeon.
    pub flags: i32,
    /// `1` dungeon, `2` raid, `3` quest, `4` zone, `5` heroic, `6` random (`0xacd644`'s tokens).
    pub type_id: i32,
    pub faction: i32,
    pub texture: String,
    pub expansion_level: i32,
    pub order_index: i32,
    pub group_id: i32,
    pub description: String,
}

/// One `LfgDungeonGroup.dbc` row: a header of the choice lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgDungeonGroup {
    pub id: i32,
    pub name: String,
    pub order_index: i32,
    pub parent_group_id: i32,
    /// 0 keeps the header out of the dungeon window's list (`0x558350`).
    pub type_id: i32,
}

/// One `LfgDungeonExpansion.dbc` row: a dungeon's level brackets for one account expansion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgDungeonExpansion {
    pub id: i32,
    pub lfg_id: i32,
    pub expansion_level: i32,
    pub random_id: i32,
    pub hard_level_min: i32,
    pub hard_level_max: i32,
    pub target_level_min: i32,
    pub target_level_max: i32,
}

/// The three tables in file order, the order every 12340 walk over them takes.
#[derive(Clone, Debug, Default)]
pub struct LfgCatalog {
    pub dungeons: Vec<LfgDungeon>,
    pub groups: Vec<LfgDungeonGroup>,
    pub expansions: Vec<LfgDungeonExpansion>,
}

/// A localized column's enUS slot and the 16 columns after it.
const LOC_COLUMNS: usize = 17;

fn push_loc(s: &mut Schema, name: &str) {
    s.add_field(SchemaField::new(name, FieldType::String));
    for i in 1..LOC_COLUMNS {
        s.add_field(SchemaField::new(format!("{name}{i}"), FieldType::UInt32));
    }
}

fn push_ints(s: &mut Schema, names: &[&str]) {
    for name in names {
        s.add_field(SchemaField::new(*name, FieldType::Int32));
    }
}

pub(crate) fn dungeons_schema() -> Schema {
    let mut s = Schema::new("LfgDungeons");
    push_ints(&mut s, &["ID"]);
    push_loc(&mut s, "Name");
    push_ints(
        &mut s,
        &[
            "MinLevel",
            "MaxLevel",
            "TargetLevel",
            "TargetLevelMin",
            "TargetLevelMax",
            "MapID",
            "Difficulty",
            "Flags",
            "TypeID",
            "Faction",
        ],
    );
    s.add_field(SchemaField::new("TextureFilename", FieldType::String));
    push_ints(&mut s, &["ExpansionLevel", "OrderIndex", "GroupID"]);
    push_loc(&mut s, "Description");
    s
}

pub(crate) fn groups_schema() -> Schema {
    let mut s = Schema::new("LfgDungeonGroup");
    push_ints(&mut s, &["ID"]);
    push_loc(&mut s, "Name");
    push_ints(&mut s, &["OrderIndex", "ParentGroupID", "TypeID"]);
    s
}

pub(crate) fn expansions_schema() -> Schema {
    let mut s = Schema::new("LfgDungeonExpansion");
    push_ints(
        &mut s,
        &[
            "ID",
            "LfgID",
            "ExpansionLevel",
            "RandomID",
            "HardLevelMin",
            "HardLevelMax",
            "TargetLevelMin",
            "TargetLevelMax",
        ],
    );
    s
}

/// Read the three dungeon finder tables off the patch chain.
pub fn load_lfg_catalog(chain: &mut Chain) -> Result<LfgCatalog> {
    let bytes = chain
        .read_file(LFG_DUNGEONS)
        .context("reading LfgDungeons.dbc")?;
    let rs = parse(&bytes, dungeons_schema(), "LfgDungeons.dbc")?;
    let int = |r, i| i32_at(r, i).unwrap_or(0);
    let dungeons = rs
        .records()
        .iter()
        .map(|r| LfgDungeon {
            id: int(r, 0),
            name: str_at(&rs, r, 1).unwrap_or_default(),
            min_level: int(r, 18),
            max_level: int(r, 19),
            target_level: int(r, 20),
            target_level_min: int(r, 21),
            target_level_max: int(r, 22),
            map_id: int(r, 23),
            difficulty: int(r, 24),
            flags: int(r, 25),
            type_id: int(r, 26),
            faction: int(r, 27),
            texture: str_at(&rs, r, 28).unwrap_or_default(),
            expansion_level: int(r, 29),
            order_index: int(r, 30),
            group_id: int(r, 31),
            description: str_at(&rs, r, 32).unwrap_or_default(),
        })
        .collect();

    let bytes = chain
        .read_file(LFG_DUNGEON_GROUP)
        .context("reading LfgDungeonGroup.dbc")?;
    let rs = parse(&bytes, groups_schema(), "LfgDungeonGroup.dbc")?;
    let groups = rs
        .records()
        .iter()
        .map(|r| LfgDungeonGroup {
            id: int(r, 0),
            name: str_at(&rs, r, 1).unwrap_or_default(),
            order_index: int(r, 18),
            parent_group_id: int(r, 19),
            type_id: int(r, 20),
        })
        .collect();

    let bytes = chain
        .read_file(LFG_DUNGEON_EXPANSION)
        .context("reading LfgDungeonExpansion.dbc")?;
    let rs = parse(&bytes, expansions_schema(), "LfgDungeonExpansion.dbc")?;
    let expansions = rs
        .records()
        .iter()
        .map(|r| LfgDungeonExpansion {
            id: int(r, 0),
            lfg_id: int(r, 1),
            expansion_level: int(r, 2),
            random_id: int(r, 3),
            hard_level_min: int(r, 4),
            hard_level_max: int(r, 5),
            target_level_min: int(r, 6),
            target_level_max: int(r, 7),
        })
        .collect();

    Ok(LfgCatalog {
        dungeons,
        groups,
        expansions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every listed dungeon is named, typed and filed under a group the group table holds, and
    /// every bracket row names a dungeon: a one-column slip breaks both.
    #[test]
    fn the_lfg_tables_cross_reference() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lfg_catalog(&mut chain).expect("load the LFG tables");
        assert!(!cat.dungeons.is_empty() && !cat.groups.is_empty() && !cat.expansions.is_empty());
        let group = |id| cat.groups.iter().any(|g| g.id == id);
        for d in cat.dungeons.iter().filter(|d| d.flags & 0x2 != 0) {
            assert!(!d.name.is_empty(), "row {} has a name", d.id);
            assert!(group(d.group_id), "row {} files under a known group", d.id);
            assert!((1..=6).contains(&d.type_id), "row {} type", d.id);
        }
        assert!(cat.dungeons.iter().any(|d| d.type_id == 6));
        for e in &cat.expansions {
            assert!(
                cat.dungeons.iter().any(|d| d.id == e.lfg_id),
                "bracket {}",
                e.id
            );
            assert!(e.hard_level_min <= e.hard_level_max, "bracket {}", e.id);
        }
    }
}
