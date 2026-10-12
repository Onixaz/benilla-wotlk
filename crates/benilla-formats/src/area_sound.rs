//! The zone audio tables: `AreaTable` (the row an MCNK `areaId` names) joined to `ZoneMusic`,
//! `SoundAmbience` and `ZoneIntroMusicTable`. `AreaTable.dbc` is read through
//! [`crate::area_table`]'s schema; its audio ids are columns 5 to 9.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

/// The audio-relevant slice of one `AreaTable` row.
pub struct AreaEntry {
    pub id: u32,
    /// The parent zone, `0` for a top-level one; a subzone inherits its unset audio ids.
    pub parent: u32,
    /// `SoundAmbience.dbc` id, `0` to inherit.
    pub ambience: u32,
    /// `ZoneMusic.dbc` id, `0` to inherit.
    pub zone_music: u32,
    /// `ZoneIntroMusicTable.dbc` id, `0` to inherit.
    pub intro_sound: u32,
    /// `SoundProviderPreferences.dbc` id of the dry-land reverb preset, `0` to inherit;
    /// interiors carry theirs on `WMOAreaTable`.
    pub sound_provider: u32,
    /// The preset while the listener is submerged (11 is "Underwater"), `0` to inherit.
    pub sound_provider_underwater: u32,
    /// The zone/subzone display name (enUS column).
    pub name: String,
}

/// One `ZoneMusic` row. All `[2]` arrays are `[day, night]`.
pub struct ZoneMusicEntry {
    pub id: u32,
    pub set_name: String,
    /// Silence between tracks in ms, uniform in `[min, max]`.
    pub silence_min: [u32; 2],
    pub silence_max: [u32; 2],
    /// `SoundEntries` kits (type 28), the track pool per phase.
    pub sounds: [u32; 2],
}

/// One `ZoneIntroMusicTable` row, the fanfare on entering an area.
pub struct ZoneIntroEntry {
    pub id: u32,
    pub sound_id: u32,
    /// Higher wins when nested areas compete.
    pub priority: u32,
    /// Replay throttle in minutes.
    pub min_delay_minutes: u32,
}

/// One `SoundAmbience` row: the looping ambience kits (type 50), `[day, night]`.
pub struct SoundAmbienceEntry {
    pub id: u32,
    pub kits: [u32; 2],
}

/// The four tables, resolvable from an MCNK `areaId`.
pub struct AreaSoundCatalog {
    areas: HashMap<u32, AreaEntry>,
    zone_music: HashMap<u32, ZoneMusicEntry>,
    intros: HashMap<u32, ZoneIntroEntry>,
    ambience: HashMap<u32, SoundAmbienceEntry>,
}

/// An area's audio after the parent walk.
pub struct AreaAudio<'a> {
    pub area_name: &'a str,
    pub music: Option<&'a ZoneMusicEntry>,
    pub intro: Option<&'a ZoneIntroEntry>,
    pub ambience: Option<&'a SoundAmbienceEntry>,
    /// Resolved `SoundProviderPreferences` ids, `[dry, underwater]` (0 = none up the chain).
    pub sound_provider: [u32; 2],
}

impl AreaSoundCatalog {
    pub fn area(&self, id: u32) -> Option<&AreaEntry> {
        self.areas.get(&id)
    }

    pub fn zone_music(&self, id: u32) -> Option<&ZoneMusicEntry> {
        self.zone_music.get(&id)
    }

    /// A `ZoneIntroMusicTable` row by id, for a `WMOAreaTable.IntroSound` interior override.
    pub fn intro(&self, id: u32) -> Option<&ZoneIntroEntry> {
        self.intros.get(&id)
    }

    /// A `SoundAmbience` row by id, for a WMO interior override.
    pub fn ambience_row(&self, id: u32) -> Option<&SoundAmbienceEntry> {
        self.ambience.get(&id)
    }

    /// An `areaId`'s audio rows, each id left `0` taken from the nearest ancestor up
    /// `ParentAreaID`: a subzone without its own music plays its zone's.
    pub fn resolve(&self, area_id: u32) -> Option<AreaAudio<'_>> {
        let first = self.areas.get(&area_id)?;
        let (mut music, mut intro, mut ambience) = (None, None, None);
        let mut provider = [0u32; 2];
        let mut cur = Some(first);
        for _ in 0..8 {
            let Some(a) = cur else { break };
            if music.is_none() && a.zone_music != 0 {
                music = self.zone_music.get(&a.zone_music);
            }
            if intro.is_none() && a.intro_sound != 0 {
                intro = self.intros.get(&a.intro_sound);
            }
            if ambience.is_none() && a.ambience != 0 {
                ambience = self.ambience.get(&a.ambience);
            }
            if provider[0] == 0 {
                provider[0] = a.sound_provider;
            }
            if provider[1] == 0 {
                provider[1] = a.sound_provider_underwater;
            }
            if a.parent == 0 {
                break;
            }
            cur = self.areas.get(&a.parent);
        }
        Some(AreaAudio {
            area_name: &first.name,
            music,
            intro,
            ambience,
            sound_provider: provider,
        })
    }

    pub fn len(&self) -> usize {
        self.areas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.areas.is_empty()
    }
}

fn zone_music_schema() -> Schema {
    let mut s = Schema::new("ZoneMusic");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("SetName", FieldType::String));
    for name in [
        "SilenceMinDay",
        "SilenceMinNight",
        "SilenceMaxDay",
        "SilenceMaxNight",
        "SoundDay",
        "SoundNight",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

fn intro_schema() -> Schema {
    let mut s = Schema::new("ZoneIntroMusicTable");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    s.add_field(SchemaField::new("SoundID", FieldType::UInt32));
    s.add_field(SchemaField::new("Priority", FieldType::UInt32));
    s.add_field(SchemaField::new("MinDelayMinutes", FieldType::UInt32));
    s
}

fn ambience_schema() -> Schema {
    let mut s = Schema::new("SoundAmbience");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("AmbienceDay", FieldType::UInt32));
    s.add_field(SchemaField::new("AmbienceNight", FieldType::UInt32));
    s
}

/// Read the four tables off the patch chain.
pub fn load_area_sound_catalog(chain: &mut Chain) -> Result<AreaSoundCatalog> {
    let read = |chain: &mut Chain, file: &str, schema: Schema, what: &str| {
        let bytes = chain
            .read_file(file)
            .with_context(|| format!("reading {file}"))?;
        parse(&bytes, schema, what)
    };

    let rs = read(
        chain,
        "DBFilesClient\\AreaTable.dbc",
        crate::area_table::area_table_schema(&crate::dbc_layout::LAYOUTS.area_table),
        "AreaTable",
    )?;
    let mut areas = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        areas.insert(
            id,
            AreaEntry {
                id,
                parent: u32_at(r, 2).unwrap_or(0),
                ambience: u32_at(r, 7).unwrap_or(0),
                zone_music: u32_at(r, 8).unwrap_or(0),
                intro_sound: u32_at(r, 9).unwrap_or(0),
                sound_provider: u32_at(r, 5).unwrap_or(0),
                sound_provider_underwater: u32_at(r, 6).unwrap_or(0),
                name: str_at(&rs, r, 11).unwrap_or_default(),
            },
        );
    }

    let rs = read(
        chain,
        "DBFilesClient\\ZoneMusic.dbc",
        zone_music_schema(),
        "ZoneMusic",
    )?;
    let mut zone_music = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        zone_music.insert(
            id,
            ZoneMusicEntry {
                id,
                set_name: str_at(&rs, r, 1).unwrap_or_default(),
                silence_min: [u32_at(r, 2).unwrap_or(0), u32_at(r, 3).unwrap_or(0)],
                silence_max: [u32_at(r, 4).unwrap_or(0), u32_at(r, 5).unwrap_or(0)],
                sounds: [u32_at(r, 6).unwrap_or(0), u32_at(r, 7).unwrap_or(0)],
            },
        );
    }

    let rs = read(
        chain,
        "DBFilesClient\\ZoneIntroMusicTable.dbc",
        intro_schema(),
        "ZoneIntroMusicTable",
    )?;
    let mut intros = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        intros.insert(
            id,
            ZoneIntroEntry {
                id,
                sound_id: u32_at(r, 2).unwrap_or(0),
                priority: u32_at(r, 3).unwrap_or(0),
                min_delay_minutes: u32_at(r, 4).unwrap_or(0),
            },
        );
    }

    let rs = read(
        chain,
        "DBFilesClient\\SoundAmbience.dbc",
        ambience_schema(),
        "SoundAmbience",
    )?;
    let mut ambience = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        ambience.insert(
            id,
            SoundAmbienceEntry {
                id,
                kits: [u32_at(r, 1).unwrap_or(0), u32_at(r, 2).unwrap_or(0)],
            },
        );
    }

    Ok(AreaSoundCatalog {
        areas,
        zone_music,
        intros,
        ambience,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every audio id in `AreaTable` columns 5 to 9 names a row of its own table, so the columns
    /// sit where the loader reads them. Four intro ids name rows the shipped table lacks.
    #[test]
    fn real_area_audio_columns_name_their_tables() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_area_sound_catalog(&mut chain).expect("load area-sound catalog");
        let providers =
            crate::load_sound_provider_catalog(&mut chain).expect("load sound providers");
        let mut seen = [0usize; 5];
        let mut missing = Vec::new();
        for a in cat.areas.values() {
            let cols = [
                (a.sound_provider, providers.get(a.sound_provider).is_some()),
                (
                    a.sound_provider_underwater,
                    providers.get(a.sound_provider_underwater).is_some(),
                ),
                (a.ambience, cat.ambience_row(a.ambience).is_some()),
                (a.zone_music, cat.zone_music(a.zone_music).is_some()),
                (a.intro_sound, cat.intro(a.intro_sound).is_some()),
            ];
            for (k, (id, found)) in cols.into_iter().enumerate() {
                if id != 0 {
                    seen[k] += 1;
                    if !found {
                        missing.push((5 + k, id));
                    }
                }
            }
        }
        assert!(seen.iter().all(|&n| n > 0), "each column is set: {seen:?}");
        missing.sort_unstable();
        assert_eq!(missing, [(9, 65), (9, 67), (9, 68), (9, 71)]);

        let dalaran = cat.resolve(4395).expect("Dalaran resolves");
        assert_eq!(dalaran.area_name, "Dalaran");
        assert_eq!(
            dalaran.music.expect("city music").set_name,
            "Zone-DalaranCity"
        );
    }

    /// The 12340 tables: Elwynn Forest (area 12) and a subzone inheriting its music.
    #[test]
    fn real_area_chain_resolves_elwynn() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_area_sound_catalog(&mut chain).expect("load area-sound catalog");
        assert_eq!(cat.len(), 2307, "all AreaTable rows load");

        let elwynn = cat.resolve(12).expect("Elwynn resolves");
        assert_eq!(elwynn.area_name, "Elwynn Forest");
        assert_eq!(
            elwynn.sound_provider,
            [0, 11],
            "Elwynn: no dry reverb, Underwater preset submerged"
        );
        assert_eq!(
            cat.resolve(24).expect("Northshire Abbey").sound_provider,
            [73, 11],
            "the abbey carries PRESET_AUDITORIUM dry"
        );
        let music = elwynn.music.expect("Elwynn has zone music");
        assert_eq!(music.set_name, "Zone-Forest");
        assert_eq!(music.silence_min, [180_000, 180_000]);
        assert_eq!(music.silence_max, [300_000, 300_000]);
        assert_eq!(music.sounds, [2523, 2523]);
        assert!(elwynn.ambience.is_some(), "Elwynn has ambience");

        let sub = cat
            .areas
            .values()
            .find(|a| a.parent == 12 && a.zone_music == 0)
            .expect("some Elwynn subzone without its own music");
        let resolved = cat.resolve(sub.id).expect("subzone resolves");
        assert_eq!(
            resolved.music.expect("inherited music").id,
            music.id,
            "subzone {} ({}) inherits Elwynn's music",
            sub.id,
            sub.name
        );
    }
}
