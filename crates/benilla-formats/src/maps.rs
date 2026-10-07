//! `Map.dbc`: a server `mapId` to its MPQ directory (`0` "Azeroth", `1` "Kalimdor", `36`
//! "DeadminesInstance") for `MapTiles::load`, plus the columns the client reads per map, in the
//! layout of the client the chain was opened for (`benilla_expansions::dbc::Map`).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};
use benilla_expansions::dbc::Map as MapLayout;

use crate::dbc::{f32_at, parse, str_at, u32_at};

const MAP: &str = "DBFilesClient\\Map.dbc";

/// `Map.dbc`'s per-map data, keyed by map id.
pub struct MapCatalog {
    dirs: HashMap<u32, String>,
    /// `MapName_Lang[enUS]` (field 4, `+0x10`, read by `0x4a65a0`). The continent dropdown shows
    /// this ("Eastern Kingdoms"), not the art folder ("Azeroth") (`GetMapContinents` `0x4a7ce0`).
    names: HashMap<u32, String>,
    /// `LoadingScreenID`, for maps with a non-zero one.
    loading_screens: HashMap<u32, u32>,
    /// `InstanceType` (field 2, `+0x8`, read at `0x48a772`, `0x495cb9`, `0x495d33`): 0 none,
    /// 1 party, 2 raid, 3 battleground, as `IsInInstance` spells them (`0x83de58`, guarded `< 4`).
    instance_types: HashMap<u32, u32>,
    /// The battleground columns of every row, since the client resolves map 0's too.
    battleground: HashMap<u32, MapBattlegroundColumns>,
}

/// The `Map.dbc` columns the battleground verbs read by row offset (`GetBattlefieldInfo`
/// `0x4ab0b0`, the list handler `0x4aa6c0`); `+0x4·k` is field `k`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapBattlegroundColumns {
    /// Field 13 (`+0x34`), the bracket base and `GetBattlefieldInfo`'s third value.
    pub min_level: u32,
    /// Field 14 (`+0x38`), `GetBattlefieldInfo`'s fourth value.
    pub max_level: u32,
    /// Field 15 (`+0x3c`): a group join needs both the party and the raid count `<=` this, else
    /// message 442 and nothing sent (`0x4a9f60`).
    pub max_players: u32,
    /// Field 16 (`+0x40`, signed), `GetBattlefieldInfo`'s fifth value; `-1` on every battleground.
    pub field_16: i32,
    /// Fields 17 and 18 (`+0x44`/`+0x48`, f32), `GetBattlefieldInfo`'s sixth and seventh values.
    pub field_17: f32,
    pub field_18: f32,
    /// Fields 20 and 29 (`+0x50`, `+0x74`), the localized descriptions by faction-group index:
    /// `0` for a `FactionTemplate` mask with bit `0x4`, `1` for bit `0x2` (`0x5efe00`).
    pub descriptions: [String; 2],
    /// Field 39 (`+0x9c`), the level-bracket span; `0` means one bracket and zeroed bounds.
    pub bracket_span: u32,
    /// Field 40 (`+0xa0`), non-zero for a group queue (`CanJoinBattlefieldAsGroup` `0x4ac380`).
    pub group_queue: u32,
    /// Field 41 (`+0xa4`, f32), `MinimapIconScale`: what `GetBattlefieldMapIconScale()` answers
    /// for the active queue slot's map (`0x4ac3d0`).
    pub minimap_icon_scale: f32,
}

impl MapBattlegroundColumns {
    /// A wire bracket's `(min, max)` levels (`0x4aa6c0`/`0x4aa850`): with a span,
    /// `min = bracket · span + min_level` and `max = min(span + min − 1, 60)`, else both zero.
    pub fn bracket_levels(&self, bracket: u8) -> (u32, u32) {
        if self.bracket_span == 0 {
            return (0, 0);
        }
        let min = u32::from(bracket) * self.bracket_span + self.min_level;
        (min, (self.bracket_span + min).saturating_sub(1).min(60))
    }
}

impl MapCatalog {
    /// The MPQ directory name (`Directory`) for `map_id`, as `MapTiles::load(chain, dir)` takes it.
    pub fn directory(&self, map_id: u32) -> Option<&str> {
        self.dirs.get(&map_id).map(String::as_str)
    }

    /// The enUS display name (`MapName_Lang`) for `map_id`, "Eastern Kingdoms".
    pub fn name(&self, map_id: u32) -> Option<&str> {
        self.names.get(&map_id).map(String::as_str)
    }

    /// `LoadingScreenID` for `map_id`, to resolve against [`crate::LoadingScreenCatalog`].
    pub fn loading_screen_id(&self, map_id: u32) -> Option<u32> {
        self.loading_screens.get(&map_id).copied()
    }

    /// `InstanceType` for `map_id`. The client treats a missing row as type 0 wherever it asks.
    pub fn instance_type(&self, map_id: u32) -> Option<u32> {
        self.instance_types.get(&map_id).copied()
    }

    /// Whether `map_id` is a party dungeon, the reference's lockout test rather than "is an
    /// instance": `cmp [rec+8],1` gates `SMSG_UPDATE_LAST_INSTANCE` and `CanShowResetInstances`.
    pub fn is_party_dungeon(&self, map_id: u32) -> bool {
        self.instance_type(map_id) == Some(1)
    }

    /// The battleground columns for `map_id`; the client reads map 0's when nothing was listed.
    /// `None` on 3.3.5a, whose `Map.dbc` does not carry them.
    pub fn battleground(&self, map_id: u32) -> Option<&MapBattlegroundColumns> {
        self.battleground.get(&map_id)
    }

    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

const INSTANCE_TYPE_FIELD: usize = 2;

/// The layout's columns typed; the unread are `u32` placeholders, so only the record has to add up.
fn map_schema(l: &MapLayout) -> Schema {
    let mut s = Schema::new("Map");
    let strings = [Some(1), Some(l.map_name)]
        .into_iter()
        .chain(l.battleground.iter().flat_map(|b| b.descriptions.map(Some)))
        .flatten()
        .collect::<Vec<_>>();
    let floats = l
        .battleground
        .iter()
        .flat_map(|b| [b.unnamed + 1, b.unnamed + 2, b.minimap_icon_scale])
        .collect::<Vec<_>>();
    for i in 0..l.fields {
        let ty = if strings.contains(&i) {
            FieldType::String
        } else if floats.contains(&i) {
            FieldType::Float32
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    s
}

/// Read Map.dbc off the patch chain into a [`MapCatalog`].
pub fn load_map_catalog(chain: &mut Chain) -> Result<MapCatalog> {
    let bytes = chain
        .read_file(MAP)
        .with_context(|| format!("reading {MAP}"))?;
    let l = &chain.expansion().profile().dbc.map;
    let rs = parse(&bytes, map_schema(l), "Map")?;
    let mut dirs = HashMap::with_capacity(rs.records().len());
    let mut names = HashMap::with_capacity(rs.records().len());
    let mut loading_screens = HashMap::new();
    let mut instance_types = HashMap::with_capacity(rs.records().len());
    let mut battleground = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(b) = &l.battleground {
            battleground.insert(
                id,
                MapBattlegroundColumns {
                    min_level: u32_at(r, b.min_level).unwrap_or(0),
                    max_level: u32_at(r, b.max_level).unwrap_or(0),
                    max_players: u32_at(r, b.max_players).unwrap_or(0),
                    field_16: u32_at(r, b.unnamed).unwrap_or(0) as i32,
                    field_17: f32_at(r, b.unnamed + 1).unwrap_or(0.0),
                    field_18: f32_at(r, b.unnamed + 2).unwrap_or(0.0),
                    descriptions: b
                        .descriptions
                        .map(|f| str_at(&rs, r, f).unwrap_or_default()),
                    bracket_span: u32_at(r, b.bracket_span).unwrap_or(0),
                    group_queue: u32_at(r, b.group_queue).unwrap_or(0),
                    minimap_icon_scale: f32_at(r, b.minimap_icon_scale).unwrap_or(1.0),
                },
            );
        }
        if let Some(dir) = str_at(&rs, r, 1) {
            dirs.insert(id, dir);
        }
        if let Some(name) = str_at(&rs, r, l.map_name).filter(|n| !n.is_empty()) {
            names.insert(id, name);
        }
        // Type 0 is recorded too, so `None` means only that no such map exists.
        if let Some(ty) = u32_at(r, INSTANCE_TYPE_FIELD) {
            instance_types.insert(id, ty);
        }
        if let Some(ls) = u32_at(r, l.loading_screen).filter(|&v| v != 0) {
            loading_screens.insert(id, ls);
        }
    }
    Ok(MapCatalog {
        dirs,
        names,
        loading_screens,
        instance_types,
        battleground,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_battleground_columns_read_the_shipped_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        if chain.expansion() != benilla_expansions::Expansion::Classic {
            return; // 3.3.5a's rows: the next test
        }
        let catalog = load_map_catalog(&mut chain).expect("Map.dbc");
        let wsg = catalog.battleground(489).expect("Warsong Gulch");
        assert_eq!(
            (wsg.min_level, wsg.max_level, wsg.max_players, wsg.field_16),
            (10, 60, 10, -1)
        );
        assert_eq!((wsg.bracket_span, wsg.group_queue), (10, 1));
        assert!(wsg.descriptions[0].starts_with("A valley bordering Ashenvale"));
        assert_eq!(
            wsg.descriptions[0], wsg.descriptions[1],
            "both faction descriptions carry the same text on the shipped rows"
        );
        assert_eq!(wsg.bracket_levels(0), (10, 19));
        assert_eq!(wsg.bracket_levels(5), (60, 60), "the max clamps at 60");
        let ab = catalog.battleground(529).expect("Arathi Basin");
        assert_eq!(
            (ab.min_level, ab.max_players, ab.bracket_span),
            (20, 15, 10)
        );
        assert!(
            (ab.minimap_icon_scale - 1.25).abs() < 1e-6,
            "Arathi Basin\'s MinimapIconScale"
        );
        assert!((wsg.minimap_icon_scale - 1.0).abs() < 1e-6);
        assert_eq!(ab.bracket_levels(1), (30, 39));
        let av = catalog.battleground(30).expect("Alterac Valley");
        assert_eq!((av.min_level, av.max_players, av.group_queue), (51, 40, 0));
        assert_eq!(
            av.bracket_levels(0),
            (0, 0),
            "a zero span zeroes both bounds"
        );
        assert!((av.field_17 - 0.74).abs() < 1e-3 && (av.field_18 - 0.34).abs() < 1e-3);
        assert!(
            catalog.battleground(0).is_some(),
            "every row carries the columns — the client resolves map 0's when nothing was listed"
        );
    }

    #[test]
    fn a_3_3_5a_map_catalog_names_each_map_s_directory_and_screen() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        if chain.expansion() != benilla_expansions::Expansion::Wotlk {
            return; // 1.12.1's rows: the test above
        }
        let catalog = load_map_catalog(&mut chain).expect("Map.dbc");
        assert_eq!(catalog.directory(0), Some("Azeroth"));
        assert_eq!(catalog.name(0), Some("Eastern Kingdoms"));
        assert_eq!(catalog.loading_screen_id(0), Some(4));
        assert_eq!(catalog.directory(571), Some("Northrend"));
        assert_eq!(catalog.directory(489), Some("PVPZone03"));
        assert_eq!(catalog.name(489), Some("Warsong Gulch"));
        assert_eq!(catalog.instance_type(489), Some(3));
        assert_eq!(catalog.loading_screen_id(489), Some(122));
        assert!(
            catalog.battleground(489).is_none(),
            "not in 3.3.5a's Map.dbc"
        );
    }
}
