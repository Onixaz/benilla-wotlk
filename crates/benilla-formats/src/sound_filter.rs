//! `SoundFilter.dbc` and `SoundFilterElem.dbc`: the named DSP chains the client builds at sound
//! init when `Sound_EnableDSPEffects` is on (`0x4ce7c0`). A chain is its elements in record order,
//! each a filter type and nine parameters: `params[0]` is the enable switch (`0.0` bypasses the
//! element) and `params[1..]` the type's own, handed to FMOD in order (`0x4cdf70`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

/// `SoundFilterElem.FilterType` (`0x4cd9d0` maps it to FMOD's).
pub mod filter_type {
    pub const PITCH_SHIFT: u32 = 1;
    pub const HIGH_PASS: u32 = 2;
    pub const LOW_PASS: u32 = 3;
    pub const CHORUS: u32 = 4;
    pub const ECHO: u32 = 5;
    /// The chain's channel-group volume, `params[1]` when `params[0] > 0`; no DSP.
    pub const VOLUME: u32 = 6;
}

/// One element of a chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundFilterElem {
    pub filter_type: u32,
    pub params: [f32; 9],
}

impl SoundFilterElem {
    /// `params[0]`, the element's enable switch (`BypassDsp(dsp, params[0] == 0.0)`).
    pub fn enabled(&self) -> bool {
        self.params[0] != 0.0
    }
}

/// The chains by `SoundFilter.Name`.
#[derive(Default)]
pub struct SoundFilterCatalog {
    chains: HashMap<String, Vec<SoundFilterElem>>,
}

impl SoundFilterCatalog {
    /// The chain named `name`, compared case-blind as the reference's `HASHKEY_STRI` table does.
    pub fn chain(&self, name: &str) -> Option<&[SoundFilterElem]> {
        self.chains
            .get(&name.to_ascii_lowercase())
            .map(Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.chains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chains.is_empty()
    }
}

pub(crate) fn filter_schema() -> Schema {
    let mut s = Schema::new("SoundFilter");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    s
}

pub(crate) fn elem_schema() -> Schema {
    let mut s = Schema::new("SoundFilterElem");
    for name in ["ID", "SoundFilterID", "OrderIndex", "FilterType"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for i in 0..9 {
        s.add_field(SchemaField::new(format!("Params{i}"), FieldType::Float32));
    }
    s
}

/// Read both tables off the patch chain. Every filter gets a chain, an empty one if no element
/// names it; an element naming no filter is skipped.
pub fn load_sound_filter_catalog(chain: &mut Chain) -> Result<SoundFilterCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\SoundFilter.dbc")
        .context("reading SoundFilter.dbc")?;
    let filters = parse(&bytes, filter_schema(), "SoundFilter")?;
    let mut names = HashMap::with_capacity(filters.records().len());
    let mut chains: HashMap<String, Vec<SoundFilterElem>> = HashMap::new();
    for r in filters.records() {
        let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&filters, r, 1)) else {
            continue;
        };
        let key = name.to_ascii_lowercase();
        chains.entry(key.clone()).or_default();
        names.insert(id, key);
    }
    let bytes = chain
        .read_file("DBFilesClient\\SoundFilterElem.dbc")
        .context("reading SoundFilterElem.dbc")?;
    let elems = parse(&bytes, elem_schema(), "SoundFilterElem")?;
    for r in elems.records() {
        let Some(key) = u32_at(r, 1).and_then(|f| names.get(&f)) else {
            continue;
        };
        let mut params = [0.0; 9];
        for (i, p) in params.iter_mut().enumerate() {
            *p = f32_at(r, 4 + i).unwrap_or(0.0);
        }
        if let Some(chain) = chains.get_mut(key) {
            chain.push(SoundFilterElem {
                filter_type: u32_at(r, 3).unwrap_or(0),
                params,
            });
        }
    }
    Ok(SoundFilterCatalog { chains })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chain on the install is a Death Knight voice, `Death Knight <race> <sex>`, and every
    /// element one of the types the reference builds.
    #[test]
    fn real_chains_are_the_death_knight_voices() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sound_filter_catalog(&mut chain).expect("load sound filters");
        assert!(!cat.is_empty());
        let dk = cat
            .chain("death knight human male")
            .expect("the human male chain");
        assert!(!dk.is_empty());
        for (name, elems) in &cat.chains {
            assert!(name.starts_with("death knight "), "{name}");
            for e in elems {
                assert!(
                    (filter_type::PITCH_SHIFT..=filter_type::VOLUME).contains(&e.filter_type),
                    "{name}: {e:?}"
                );
            }
        }
    }
}
