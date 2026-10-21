//! `MapDifficulty.dbc` (23 columns in 0x5c-byte records): one row per map and difficulty, with
//! the player cap and the difficulty's `GlobalStrings` token. 12340 finds a row by walking the
//! table for the map's first row and then that map's run (`0x634950`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{i32_at, parse, str_at};

const MAP_DIFFICULTY: &str = "DBFilesClient\\MapDifficulty.dbc";

/// One `MapDifficulty.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapDifficulty {
    pub id: i32,
    pub map_id: i32,
    /// 0-based, as `SMSG_INSTANCE_DIFFICULTY` carries it.
    pub difficulty: i32,
    /// `Message_lang` enUS: the refusal a transfer at this difficulty shows (`0x403a39`).
    pub message: String,
    /// The raid lock length in seconds, 0 for a dungeon.
    pub raid_duration: i32,
    pub max_players: i32,
    /// A `GlobalStrings` token (`"RAID_DIFFICULTY_10PLAYER"`), resolved where it is shown.
    pub difficulty_string: String,
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("MapDifficulty");
    for name in ["ID", "MapID", "Difficulty"] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    s.add_field(SchemaField::new("Message", FieldType::String));
    for i in 1..17 {
        s.add_field(SchemaField::new(format!("Message{i}"), FieldType::UInt32));
    }
    for name in ["RaidDuration", "MaxPlayers"] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    s.add_field(SchemaField::new("Difficultystring", FieldType::String));
    s
}

/// The `(map, difficulty)` row, by 12340's walk (`0x634950`): the run after the map's first row.
pub fn find_map_difficulty(
    rows: &[MapDifficulty],
    map_id: i32,
    difficulty: i32,
) -> Option<&MapDifficulty> {
    let first = rows.iter().position(|r| r.map_id == map_id)?;
    rows[first..]
        .iter()
        .take_while(|r| r.map_id == map_id)
        .find(|r| r.difficulty == difficulty)
}

/// Read `MapDifficulty.dbc` off the patch chain, in file order.
pub fn load_map_difficulties(chain: &mut Chain) -> Result<Vec<MapDifficulty>> {
    let bytes = chain
        .read_file(MAP_DIFFICULTY)
        .context("reading MapDifficulty.dbc")?;
    let rs = parse(&bytes, schema(), "MapDifficulty.dbc")?;
    let int = |r, i| i32_at(r, i).unwrap_or(0);
    Ok(rs
        .records()
        .iter()
        .map(|r| MapDifficulty {
            id: int(r, 0),
            map_id: int(r, 1),
            difficulty: int(r, 2),
            message: str_at(&rs, r, 3).unwrap_or_default(),
            raid_duration: int(r, 20),
            max_players: int(r, 21),
            difficulty_string: str_at(&rs, r, 22).unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(map_id: i32, difficulty: i32, max_players: i32) -> MapDifficulty {
        MapDifficulty {
            map_id,
            difficulty,
            max_players,
            ..Default::default()
        }
    }

    /// The walk stops at the end of the map's first run, so a row split off from it is unseen.
    #[test]
    fn the_lookup_walks_only_the_maps_first_run() {
        let rows = [row(1, 0, 5), row(1, 1, 10), row(2, 0, 25), row(1, 2, 40)];
        assert_eq!(
            find_map_difficulty(&rows, 1, 1).map(|r| r.max_players),
            Some(10)
        );
        assert_eq!(
            find_map_difficulty(&rows, 2, 0).map(|r| r.max_players),
            Some(25)
        );
        assert!(find_map_difficulty(&rows, 1, 2).is_none());
        assert!(find_map_difficulty(&rows, 3, 0).is_none());
    }

    /// Every raid-sized row carries a lock length, and every row's cap is a group size.
    #[test]
    fn the_real_table_reads_its_caps_and_lock_lengths() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_map_difficulties(&mut chain).expect("load MapDifficulty.dbc");
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| (0..=40).contains(&r.max_players)));
        assert!(rows
            .iter()
            .any(|r| r.max_players == 25 && r.raid_duration > 0));
        assert!(rows.iter().any(|r| !r.difficulty_string.is_empty()));
    }
}
