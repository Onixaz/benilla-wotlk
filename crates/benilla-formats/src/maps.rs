//! `Map.dbc`: a server `mapId` to its MPQ directory (`0` "Azeroth", `1` "Kalimdor", `36`
//! "DeadminesInstance") for `MapTiles::load`, plus the columns the client reads per map
//! (`crate::dbc_layout::Map`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::dbc_layout::Map as MapLayout;
use crate::Chain;

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
    /// `Flags` (column 3, `+0xc`), for maps with a non-zero one.
    flags: HashMap<u32, u32>,
    /// `MapDescription0`/`1` (`+0x1c`/`+0x20`), the Horde and Alliance battleground texts.
    descriptions: HashMap<u32, [String; 2]>,
    /// `MinimapIconScale` (`+0x28`), for maps whose scale is not 1.
    icon_scales: HashMap<u32, f32>,
    /// The entrance (`+0x2c` map, `+0x30`/`+0x34` x and y), for maps with a non-zero one.
    entrances: HashMap<u32, (u32, f32, f32)>,
    /// `MaxPlayers` (`+0x44`), for maps with a non-zero one.
    max_players: HashMap<u32, u32>,
}

/// `Map.dbc` `Flags` bit: the map's difficulty can change inside it, `GetInstanceInfo`'s seventh
/// return (`0x51a9e4`).
const FLAG_DYNAMIC_DIFFICULTY: u32 = 0x100;

impl MapCatalog {
    /// The MPQ directory name (`Directory`) for `map_id`, as `MapTiles::load(chain, dir)` takes it.
    pub fn directory(&self, map_id: u32) -> Option<&str> {
        self.dirs.get(&map_id).map(String::as_str)
    }

    /// The enUS display name (`MapName_Lang`) for `map_id`, "Eastern Kingdoms".
    pub fn name(&self, map_id: u32) -> Option<&str> {
        self.names.get(&map_id).map(String::as_str)
    }

    /// Every named map, `(id, name)`.
    pub fn names(&self) -> impl Iterator<Item = (u32, &str)> + '_ {
        self.names.iter().map(|(&id, n)| (id, n.as_str()))
    }

    /// `LoadingScreenID` for `map_id`, to resolve against [`crate::LoadingScreenCatalog`].
    pub fn loading_screen_id(&self, map_id: u32) -> Option<u32> {
        self.loading_screens.get(&map_id).copied()
    }

    /// `InstanceType` for `map_id`. The client treats a missing row as type 0 wherever it asks.
    pub fn instance_type(&self, map_id: u32) -> Option<u32> {
        self.instance_types.get(&map_id).copied()
    }

    /// `map_id`'s description for a side, 0 Horde and 1 Alliance (`GetBattlefieldInfo`,
    /// `0x54d7c0`); empty where the row has none.
    pub fn description(&self, map_id: u32, side: usize) -> Option<&str> {
        self.descriptions
            .get(&map_id)
            .and_then(|d| d.get(side))
            .map(String::as_str)
    }

    /// `MinimapIconScale` for `map_id`, 1.0 for a map with no row (`0x54c740`).
    pub fn icon_scale(&self, map_id: u32) -> f32 {
        self.icon_scales.get(&map_id).copied().unwrap_or(1.0)
    }

    /// Whether `map_id`'s difficulty can change inside it ([`FLAG_DYNAMIC_DIFFICULTY`]).
    pub fn has_dynamic_difficulty(&self, map_id: u32) -> bool {
        self.flags
            .get(&map_id)
            .is_some_and(|f| f & FLAG_DYNAMIC_DIFFICULTY != 0)
    }

    /// Whether `map_id` is a party dungeon, the reference's lockout test rather than "is an
    /// instance": `cmp [rec+8],1` gates `SMSG_UPDATE_LAST_INSTANCE` and `CanShowResetInstances`.
    pub fn is_party_dungeon(&self, map_id: u32) -> bool {
        self.instance_type(map_id) == Some(1)
    }

    /// The map's entrance `(map, x, y)` on another map, which a corpse and the world map use
    /// for an instance (`0x547170`); `None` where the position is zero.
    pub fn entrance(&self, map_id: u32) -> Option<(u32, f32, f32)> {
        self.entrances.get(&map_id).copied()
    }

    /// Every map with an entrance, as `(map, (entrance map, x, y))`.
    pub fn entrances(&self) -> impl Iterator<Item = (u32, (u32, f32, f32))> + '_ {
        self.entrances.iter().map(|(&k, &v)| (k, v))
    }

    /// `MaxPlayers` (`+0x44`), the fallback `GetMaxPlayersForMap` (`0x634ab0`) takes when the
    /// difficulty's `MapDifficulty` row sets none; 0 when unset.
    pub fn max_players(&self, map_id: u32) -> u32 {
        self.max_players.get(&map_id).copied().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

const INSTANCE_TYPE_FIELD: usize = 2;
const FLAGS_FIELD: usize = 3;

/// The layout's columns typed; the unread are `u32` placeholders, so only the record has to add up.
pub(crate) fn map_schema(l: &MapLayout) -> Schema {
    let mut s = Schema::new("Map");
    for i in 0..l.fields {
        let ty = if i == 1 || i == l.map_name || l.description.contains(&i) {
            FieldType::String
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
    let l = &crate::dbc_layout::LAYOUTS.map;
    let rs = parse(&bytes, map_schema(l), "Map")?;
    let mut dirs = HashMap::with_capacity(rs.records().len());
    let mut names = HashMap::with_capacity(rs.records().len());
    let mut loading_screens = HashMap::new();
    let mut instance_types = HashMap::with_capacity(rs.records().len());
    let mut flags = HashMap::new();
    let mut entrances = HashMap::new();
    let mut descriptions = HashMap::new();
    let mut icon_scales = HashMap::new();
    let mut max_players = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
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
        if let Some(f) = u32_at(r, FLAGS_FIELD).filter(|&v| v != 0) {
            flags.insert(id, f);
        }
        let [h, a] = l.description.map(|c| str_at(&rs, r, c).unwrap_or_default());
        if !h.is_empty() || !a.is_empty() {
            descriptions.insert(id, [h, a]);
        }
        // `MinimapIconScale`, the column after the loading screen, typed `u32` like the rest.
        if let Some(scale) = u32_at(r, l.loading_screen + 1).map(f32::from_bits) {
            if scale != 1.0 {
                icon_scales.insert(id, scale);
            }
        }
        // The three columns after `MinimapIconScale`, two past the loading screen; the schema
        // types them as `u32`, so the floats are read by their bits.
        let at = |i: usize| u32_at(r, l.loading_screen + i);
        if let (Some(m), Some(x), Some(y)) = (at(2), at(3), at(4)) {
            let (x, y) = (f32::from_bits(x), f32::from_bits(y));
            if x != 0.0 || y != 0.0 {
                entrances.insert(id, (m, x, y));
            }
        }
        // `MaxPlayers`, eight columns past the loading screen (`+0x24` to `+0x44`).
        if let Some(n) = at(8).filter(|&n| n != 0) {
            max_players.insert(id, n);
        }
    }
    Ok(MapCatalog {
        dirs,
        names,
        loading_screens,
        instance_types,
        flags,
        entrances,
        descriptions,
        icon_scales,
        max_players,
    })
}

/// Every map's `(id, Directory)`, read through a shared chain.
pub(crate) fn map_directories(chain: &Chain) -> Result<Vec<(u32, String)>> {
    let bytes = chain.read(MAP).with_context(|| format!("reading {MAP}"))?;
    let rs = parse(&bytes, map_schema(&crate::dbc_layout::LAYOUTS.map), "Map")?;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 0)?, str_at(&rs, r, 1)?)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_3_3_5a_map_catalog_names_each_map_s_directory_and_screen() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let catalog = load_map_catalog(&mut chain).expect("Map.dbc");
        assert_eq!(catalog.directory(0), Some("Azeroth"));
        assert_eq!(catalog.name(0), Some("Eastern Kingdoms"));
        assert_eq!(catalog.loading_screen_id(0), Some(4));
        assert_eq!(catalog.directory(571), Some("Northrend"));
        assert_eq!(catalog.directory(489), Some("PVPZone03"));
        assert_eq!(catalog.name(489), Some("Warsong Gulch"));
        assert_eq!(catalog.instance_type(489), Some(3));
        assert_eq!(catalog.loading_screen_id(489), Some(122));
        // Flags `0x100`: the two raids whose difficulty changes inside, and no other map.
        assert!(catalog.has_dynamic_difficulty(631), "Icecrown Citadel");
        assert!(catalog.has_dynamic_difficulty(724), "The Ruby Sanctum");
        assert!(!catalog.has_dynamic_difficulty(0) && !catalog.has_dynamic_difficulty(533));
    }
}
