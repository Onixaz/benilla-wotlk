//! `Cfg_Configs.dbc`: build 12340 reads four 32-bit columns (`0x8b72b0`). At world entry,
//! `0x405540` matches the selected realm's type and latches column 2 for the PvP display gate.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CFG_CONFIGS: &str = "DBFilesClient\\Cfg_Configs.dbc";

/// One realm-type configuration row, in file order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealmConfig {
    /// Column 1, matched against the selected realm's wire type.
    pub realm_type: u32,
    /// Column 2, latched at world entry.
    pub pvp: bool,
    /// Column 3, used by the stock realm list.
    pub rp: bool,
}

/// `ID`, `RealmType`, `PlayerKillingAllowed`, `Roleplaying`: four 32-bit columns.
pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("Cfg_Configs");
    for name in ["ID", "RealmType", "PlayerKillingAllowed", "Roleplaying"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Every configuration row from the player's patch chain, in file order.
pub fn load_realm_configs(chain: &mut Chain) -> Result<Vec<RealmConfig>> {
    let bytes = chain
        .read_file(CFG_CONFIGS)
        .with_context(|| format!("reading {CFG_CONFIGS}"))?;
    let rows = parse(&bytes, schema(), "Cfg_Configs")?;
    Ok(rows
        .records()
        .iter()
        .filter_map(|r| {
            Some(RealmConfig {
                realm_type: u32_at(r, 1)?,
                pvp: u32_at(r, 2)? != 0,
                rp: u32_at(r, 3)? != 0,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_shipped_realm_types_come_from_the_install() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_realm_configs(&mut chain).expect("Cfg_Configs.dbc");
        let actual: Vec<_> = rows.iter().map(|r| (r.realm_type, r.pvp, r.rp)).collect();
        assert_eq!(
            actual,
            [
                (0, false, false),
                (1, true, false),
                (2, false, false),
                (3, true, false),
                (4, false, false),
                (5, true, false),
                (6, false, true),
                (7, false, true),
                (8, true, true),
                (9, false, false),
                (10, true, false),
                (11, false, false),
                (12, true, false),
            ]
        );
    }
}
