//! `Weather.dbc`: what an `SMSG_WEATHER` id names. 12340's handler (`0x526a77`) looks the
//! packet's id up in this table (`[0xad4eb0]`) and hands `SetWeather` (`0x7846a0`) the row's
//! effect type (`+0x08`, honoured below 4); the row's ambience kit (`+0x04`) becomes the weather
//! sound (`0x4c8610`). An id the table lacks is fine weather with no sound.

use std::collections::HashMap;

use anyhow::Result;
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

/// One `Weather.dbc` row, as far as the reference's packet handler reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeatherRow {
    /// The effect type `SetWeather` takes: 0 fine, 1 rain, 2 snow, 3 sand.
    pub effect_type: u32,
    /// The ambience `SoundEntries` kit, 0 for none.
    pub ambience_kit: u32,
}

/// The `id → row` map.
pub struct WeatherCatalog {
    rows: HashMap<u32, WeatherRow>,
}

impl WeatherCatalog {
    /// The row an `SMSG_WEATHER` id names.
    pub fn get(&self, id: u32) -> Option<WeatherRow> {
        self.rows.get(&id).copied()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("Weather");
    for name in ["ID", "AmbienceID", "EffectType"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    // `+0x0c` goes to `SetWeather` as its last argument; `+0x10..0x18` the effect colour.
    for name in ["Scale", "EffectColorR", "EffectColorG", "EffectColorB"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s.add_field(SchemaField::new("EffectTexture", FieldType::String));
    s
}

/// Read `Weather.dbc` off the patch chain.
pub fn load_weather_catalog(chain: &mut Chain) -> Result<WeatherCatalog> {
    let bytes = chain.read_file("DBFilesClient\\Weather.dbc")?;
    let rs = parse(&bytes, schema(), "Weather")?;
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| {
            Some((
                u32_at(r, 0)?,
                WeatherRow {
                    ambience_kit: u32_at(r, 1)?,
                    effect_type: u32_at(r, 2)?,
                },
            ))
        })
        .collect();
    Ok(WeatherCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_table_loads_rows_of_every_effect_type() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_weather_catalog(&mut chain).expect("load Weather.dbc");
        assert!(!cat.is_empty());
        let types: std::collections::HashSet<u32> =
            cat.rows.values().map(|r| r.effect_type).collect();
        for t in 0..=3 {
            assert!(types.contains(&t), "no row of effect type {t}: {types:?}");
        }
        assert!(
            cat.rows
                .values()
                .any(|r| r.effect_type == 1 && r.ambience_kit != 0),
            "a rain row carries its ambience kit"
        );
    }
}
