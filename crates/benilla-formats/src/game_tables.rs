//! The gt tables behind the barber shop's prices and the character sheet's ratings, crit chances
//! and spirit regeneration, read as 12340 reads them: `GameTable_Lookup(slot, row, col)`
//! (`0x7f6990`) fetches the record whose id is `col × rows + row` (`0x7f69b0`), `rows` being the
//! table's `GameTables.dbc` row count (`GameTable_Bind` `0x7f6a10`). A one-column table's record
//! id is its index (`0x65c330`); `gtOCTClassCombatRatingScalar` keeps its id in column 0 and its
//! value in column 1 (`0x660f80`, `0x7f69e0`). An id outside the table reads 0, where `GetRecord`
//! (`0x65c290`) finds none.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, str_at, u32_at};

/// The gt tables the barber shop and the character sheet read, by their lookup slot in 12340's
/// adapter table (`g_gameTableAdapters` `0xaf4f90`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GameTableId {
    /// Slot 0: the barber shop's base price in copper by level (`0x52e850`, `0x52eba0`).
    BarberShopCostBase,
    /// Slot 1: rating points per 1% by level (row) and rating (column).
    CombatRatings,
    /// Slot 2: melee crit per agility by level and class.
    ChanceToMeleeCrit,
    /// Slot 3: base melee crit by class.
    ChanceToMeleeCritBase,
    /// Slot 4: spell crit per intellect by level and class.
    ChanceToSpellCrit,
    /// Slot 5: base spell crit by class.
    ChanceToSpellCritBase,
    /// Slot 7: health regen per spirit up to the cap, by level and class.
    OctRegenHp,
    /// Slot 9: health regen per spirit past the cap.
    RegenHpPerSpt,
    /// Slot 10: mana regen per spirit.
    RegenMpPerSpt,
    /// Slot 11: each class's rating scalar, by rating + 1 (row) and class (column).
    OctClassCombatRatingScalar,
}

impl GameTableId {
    const ALL: [GameTableId; 10] = [
        GameTableId::BarberShopCostBase,
        GameTableId::CombatRatings,
        GameTableId::ChanceToMeleeCrit,
        GameTableId::ChanceToMeleeCritBase,
        GameTableId::ChanceToSpellCrit,
        GameTableId::ChanceToSpellCritBase,
        GameTableId::OctRegenHp,
        GameTableId::RegenHpPerSpt,
        GameTableId::RegenMpPerSpt,
        GameTableId::OctClassCombatRatingScalar,
    ];

    /// The adapter's name, matched against `GameTables.dbc` column 0, and the file stem.
    fn name(self) -> &'static str {
        match self {
            GameTableId::BarberShopCostBase => "BarberShopCostBase",
            GameTableId::CombatRatings => "CombatRatings",
            GameTableId::ChanceToMeleeCrit => "ChanceToMeleeCrit",
            GameTableId::ChanceToMeleeCritBase => "ChanceToMeleeCritBase",
            GameTableId::ChanceToSpellCrit => "ChanceToSpellCrit",
            GameTableId::ChanceToSpellCritBase => "ChanceToSpellCritBase",
            GameTableId::OctRegenHp => "OCTRegenHP",
            GameTableId::RegenHpPerSpt => "RegenHPPerSpt",
            GameTableId::RegenMpPerSpt => "RegenMPPerSpt",
            GameTableId::OctClassCombatRatingScalar => "OCTClassCombatRatingScalar",
        }
    }

    /// Whether the record is `(id, value)` rather than a bare value.
    fn keyed(self) -> bool {
        self == GameTableId::OctClassCombatRatingScalar
    }
}

/// One gt table: its `GameTables.dbc` row count and its values by record id.
#[derive(Clone, Debug, Default)]
struct GameTable {
    rows: u32,
    values: HashMap<u32, f32>,
}

/// The loaded gt tables.
#[derive(Clone, Debug, Default)]
pub struct GameTables {
    tables: HashMap<GameTableId, GameTable>,
}

impl GameTables {
    /// Tables from explicit `(rows, values by id)`, for tests.
    pub fn from_tables(
        tables: impl IntoIterator<Item = (GameTableId, u32, Vec<(u32, f32)>)>,
    ) -> Self {
        Self {
            tables: tables
                .into_iter()
                .map(|(id, rows, values)| {
                    (
                        id,
                        GameTable {
                            rows,
                            values: values.into_iter().collect(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// `GameTable_Lookup(slot, row, col)`: the value of record `col × rows + row`, 0 when absent.
    pub fn lookup(&self, table: GameTableId, row: i32, col: i32) -> f32 {
        let Some(t) = self.tables.get(&table) else {
            return 0.0;
        };
        let id = i64::from(col) * i64::from(t.rows) + i64::from(row);
        u32::try_from(id)
            .ok()
            .and_then(|id| t.values.get(&id))
            .copied()
            .unwrap_or(0.0)
    }
}

pub(crate) fn game_tables_schema() -> Schema {
    let mut s = Schema::new("GameTables");
    s.add_field(SchemaField::new("Name", FieldType::String));
    s.add_field(SchemaField::new("NumRows", FieldType::UInt32));
    s.add_field(SchemaField::new("NumColumns", FieldType::UInt32));
    s
}

/// A one-column gt table, `table` its file stem: a bare float per record.
pub(crate) fn gt_value_schema(table: &str) -> Schema {
    let mut s = Schema::new(table);
    s.add_field(SchemaField::new("Value", FieldType::Float32));
    s
}

/// `gtOCTClassCombatRatingScalar`: an id and a float.
pub(crate) fn gt_keyed_value_schema() -> Schema {
    let mut s = Schema::new("gtOCTClassCombatRatingScalar");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Value", FieldType::Float32));
    s.set_key_field("ID");
    s
}

/// Load `GameTables.dbc` and the gt tables [`GameTableId`] names off the chain.
pub fn load_game_tables(chain: &mut Chain) -> Result<GameTables> {
    let path = "DBFilesClient\\GameTables.dbc";
    let bytes = chain
        .read_file(path)
        .with_context(|| format!("reading {path}"))?;
    let rs = parse(&bytes, game_tables_schema(), "GameTables")?;
    let mut rows_by_name = HashMap::new();
    for r in rs.records() {
        if let (Some(name), Some(rows)) = (str_at(&rs, r, 0), u32_at(r, 1)) {
            rows_by_name.insert(name, rows);
        }
    }
    let mut tables = HashMap::new();
    for id in GameTableId::ALL {
        let name = id.name();
        let rows = *rows_by_name
            .get(name)
            .with_context(|| format!("GameTables.dbc has no {name} row"))?;
        let stem = format!("gt{name}");
        let path = format!("DBFilesClient\\{stem}.dbc");
        let bytes = chain
            .read_file(&path)
            .with_context(|| format!("reading {path}"))?;
        let mut values = HashMap::new();
        if id.keyed() {
            let rs = parse(&bytes, gt_keyed_value_schema(), &stem)?;
            for r in rs.records() {
                if let (Some(k), Some(v)) = (u32_at(r, 0), f32_at(r, 1)) {
                    values.insert(k, v);
                }
            }
        } else {
            let rs = parse(&bytes, gt_value_schema(&stem), &stem)?;
            for (i, r) in rs.records().iter().enumerate() {
                if let Some(v) = f32_at(r, 0) {
                    values.insert(i as u32, v);
                }
            }
        }
        tables.insert(id, GameTable { rows, values });
    }
    Ok(GameTables { tables })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_reads_column_major_by_the_bound_row_count() {
        let t = GameTables::from_tables([(
            GameTableId::CombatRatings,
            100,
            vec![(0, 1.5), (99, 2.5), (100, 3.5), (2405, 9.0)],
        )]);
        assert_eq!(t.lookup(GameTableId::CombatRatings, 0, 0), 1.5);
        assert_eq!(t.lookup(GameTableId::CombatRatings, 99, 0), 2.5);
        assert_eq!(t.lookup(GameTableId::CombatRatings, 0, 1), 3.5);
        assert_eq!(t.lookup(GameTableId::CombatRatings, 5, 24), 9.0);
        // No record, or a table not loaded: 0.
        assert_eq!(t.lookup(GameTableId::CombatRatings, 1, 0), 0.0);
        assert_eq!(t.lookup(GameTableId::CombatRatings, -1, 0), 0.0);
        assert_eq!(t.lookup(GameTableId::RegenMpPerSpt, 0, 0), 0.0);
    }

    #[test]
    fn the_install_s_tables_bind_and_answer() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let t = load_game_tables(&mut chain).expect("gt tables");
        // Hit rating (CR_HIT_MELEE, index 5) costs more points per 1% at 80 than at 10.
        let hit = |level: i32| t.lookup(GameTableId::CombatRatings, level - 1, 5);
        assert!(hit(10) > 0.0 && hit(80) > hit(10));
        // Every class has a scalar for that rating, at row rating + 1; column 9 is class 10, which
        // 3.3.5a has not.
        for class in (0..9).chain([10]) {
            assert!(t.lookup(GameTableId::OctClassCombatRatingScalar, 6, class) > 0.0);
        }
        // A mage (class 8) has a base spell crit and gains crit from intellect at 80.
        assert!(t.lookup(GameTableId::ChanceToSpellCritBase, 0, 7) > 0.0);
        assert!(t.lookup(GameTableId::ChanceToSpellCrit, 79, 7) > 0.0);
        // The barber's base price climbs with level.
        let base = |level: i32| t.lookup(GameTableId::BarberShopCostBase, level - 1, 0);
        assert!(base(10) > 0.0 && base(80) > base(10));
    }
}
