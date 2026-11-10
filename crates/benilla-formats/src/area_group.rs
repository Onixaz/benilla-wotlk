//! `AreaGroup.dbc`: the area lists `Spell.dbc`'s `RequiredAreasID` names, each row six areas and
//! the next row of its chain (`0x7199e0`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const AREA_GROUP: &str = "DBFilesClient\\AreaGroup.dbc";

/// One row: up to six `AreaTable.dbc` ids, the first zero ending the row, and the next row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AreaGroup {
    pub id: u32,
    pub areas: [u32; 6],
    pub next: u32,
}

/// The answer of [`AreaGroupCatalog::area_in_group`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AreaGroupMatch {
    pub found: bool,
    /// How many listed areas the walk compared before it stopped.
    pub checked: u32,
}

/// Every row, by id.
#[derive(Debug, Clone, Default)]
pub struct AreaGroupCatalog {
    rows: HashMap<u32, AreaGroup>,
}

impl AreaGroupCatalog {
    /// Build from rows, for tests and fixtures.
    pub fn from_rows(rows: impl IntoIterator<Item = AreaGroup>) -> Self {
        Self {
            rows: rows.into_iter().map(|r| (r.id, r)).collect(),
        }
    }

    pub fn get(&self, id: u32) -> Option<&AreaGroup> {
        self.rows.get(&id)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Walk `group`'s chain for an area (`0x7199e0`): a listed id matches the area itself or its
    /// one parent, never further up. A row or chain link the file lacks ends the walk unfound.
    pub fn area_in_group(&self, area: u32, parent: u32, group: u32) -> AreaGroupMatch {
        let mut at = AreaGroupMatch::default();
        let mut id = group;
        // One pass per row bounds a chain that loops; the shipped chains do not.
        for _ in 0..=self.rows.len() {
            if id == 0 {
                break;
            }
            let Some(row) = self.rows.get(&id) else {
                break;
            };
            for &listed in row.areas.iter().take_while(|&&a| a != 0) {
                at.checked += 1;
                if listed == area || listed == parent {
                    at.found = true;
                    return at;
                }
            }
            id = row.next;
        }
        at
    }
}

pub(crate) fn area_group_schema() -> Schema {
    let mut s = Schema::new("AreaGroup");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new_array("AreaID", FieldType::UInt32, 6));
    s.add_field(SchemaField::new("NextAreaID", FieldType::UInt32));
    s
}

/// Load `AreaGroup.dbc` off the chain.
pub fn load_area_groups(chain: &mut Chain) -> Result<AreaGroupCatalog> {
    let bytes = chain
        .read_file(AREA_GROUP)
        .with_context(|| format!("reading {AREA_GROUP}"))?;
    let rs = parse(&bytes, area_group_schema(), "AreaGroup")?;
    let rows = rs.records().iter().filter_map(|r| {
        let mut areas = [0; 6];
        for (i, a) in areas.iter_mut().enumerate() {
            *a = u32_at(r, 1 + i)?;
        }
        Some(AreaGroup {
            id: u32_at(r, 0)?,
            areas,
            next: u32_at(r, 7)?,
        })
    });
    Ok(AreaGroupCatalog::from_rows(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u32, areas: &[u32], next: u32) -> AreaGroup {
        let mut a = [0; 6];
        a[..areas.len()].copy_from_slice(areas);
        AreaGroup { id, areas: a, next }
    }

    #[test]
    fn the_walk_matches_the_area_or_its_one_parent_and_counts_what_it_compared() {
        let c = AreaGroupCatalog::from_rows([row(1, &[10, 20], 2), row(2, &[30], 0)]);
        assert_eq!(
            c.area_in_group(20, 0, 1),
            AreaGroupMatch {
                found: true,
                checked: 2
            }
        );
        assert!(
            c.area_in_group(99, 30, 1).found,
            "the parent, down the chain"
        );
        assert_eq!(
            c.area_in_group(99, 98, 1),
            AreaGroupMatch {
                found: false,
                checked: 3
            }
        );
    }

    #[test]
    fn a_zero_ends_a_row_but_the_chain_goes_on_and_a_missing_row_ends_it() {
        let mut gap = row(1, &[10], 2);
        gap.areas[2] = 40;
        let c = AreaGroupCatalog::from_rows([gap, row(2, &[30], 7)]);
        assert!(!c.area_in_group(40, 0, 1).found, "past the row's zero");
        assert!(c.area_in_group(30, 0, 1).found);
        assert_eq!(c.area_in_group(1, 0, 9), AreaGroupMatch::default());
    }

    #[test]
    fn a_chain_that_loops_ends() {
        let c = AreaGroupCatalog::from_rows([row(1, &[10], 2), row(2, &[20], 1)]);
        assert!(!c.area_in_group(99, 0, 1).found);
    }

    /// Spellcloth (31373) needs a group whose first area names the tool line.
    #[test]
    fn the_real_spellcloth_group_lists_its_area() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let groups = load_area_groups(&mut chain).expect("AreaGroup.dbc");
        let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let areas = crate::load_area_table_catalog(&mut chain).expect("AreaTable.dbc");
        let group = spells.get(31373).expect("Spellcloth").required_areas_id;
        assert!(group > 0);
        let row = groups.get(group as u32).expect("its group");
        assert!(areas.get(row.areas[0]).is_some(), "the area it names");
    }
}
