//! Quest log header names. As in the client, a quest's `ZoneOrSort` is an `AreaTable.dbc` id
//! when positive and a negated `QuestSort.dbc` id (class, profession, seasonal) when negative.

use std::collections::HashMap;

use anyhow::Result;
use benilla_dbc::Schema;

use crate::chain::Chain;
use crate::dbc::{id_name_schema, load_id_name_map};

/// `ZoneOrSort` → header name.
#[derive(Debug, Default)]
pub struct QuestHeaderNames {
    zones: HashMap<u32, String>,
    sorts: HashMap<u32, String>,
}

impl QuestHeaderNames {
    /// The header title; `None` for 0 or an unknown id, where the caller picks its own bucket.
    pub fn resolve(&self, zone_or_sort: i32) -> Option<&str> {
        if zone_or_sort > 0 {
            self.zones.get(&(zone_or_sort as u32)).map(String::as_str)
        } else if zone_or_sort < 0 {
            self.sorts
                .get(&(zone_or_sort.unsigned_abs()))
                .map(String::as_str)
        } else {
            None
        }
    }
}

/// `QuestSort.dbc`: `ID` and the `SortName` loc-string at column 1, 18 columns.
pub(crate) fn quest_sort_schema() -> Schema {
    id_name_schema("QuestSort", 1, 18)
}

/// Load both name tables through the patch chain.
pub fn load_quest_header_names(chain: &mut Chain) -> Result<QuestHeaderNames> {
    Ok(QuestHeaderNames {
        zones: load_id_name_map(
            chain,
            "DBFilesClient\\AreaTable.dbc",
            crate::area_table::area_table_schema(&crate::dbc_layout::LAYOUTS.area_table),
            11,
        )?,
        sorts: load_id_name_map(
            chain,
            "DBFilesClient\\QuestSort.dbc",
            quest_sort_schema(),
            1,
        )?,
    })
}
