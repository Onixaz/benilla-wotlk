//! `SpellChainEffects.dbc`: the beams of Chain Lightning, Drain Life, Mind Flay, Chain Heal. A kit
//! reaches it only through a CharProc slot of type 0 or 12, which the dispatcher routes on to the
//! chain creator (`0x7fc5f0`). Ids are a lookup, never an index.
//! `benilla-extract <Data> chaincensus` prints the table with every kit that draws a beam.
//!
//! 12340 reads a 177-byte record (`0x8b73b0`): 39 four-byte columns, the five byte columns 39-43
//! (alpha, red, green, blue, blend mode), then the `Combo` string and three more four-byte
//! columns; the strings are columns 7 and 44.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

const SPELL_CHAIN_EFFECTS: &str = "DBFilesClient\\SpellChainEffects.dbc";
const SPELL_CHAIN_EFFECTS_FIELDS: usize = 48;

/// One row: the shape and animation of one beam. Columns 1-4 keep their 1.12.1 positions; the
/// joint, wave, arc, flicker, pulse, colour and combo columns 8-47 are not read.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainEffect {
    /// Column 1: the target segment length; a hop gets `trunc(length / this + 2.0)` segments, so
    /// never fewer than two.
    pub avg_seg_len: f32,
    /// Column 2: the half-width; the ribbon spans twice this in yards.
    pub half_width: f32,
    /// Column 3: the jitter amplitude as a fraction of the beam's length, re-rolled every frame
    /// and blended 0.75 old to 0.25 new; 0.01 on most rows, 0.04 on Chain Lightning's.
    pub noise_scale: f32,
    /// Column 4: the texture's scroll period in seconds, `u = -(phase / this)` with `phase`
    /// advancing by `dt` modulo this. A negative period reverses the scroll, flowing back toward
    /// the caster.
    pub scroll_period_s: f32,
    /// Column 5: how long one hop burns in ms, from its own start (`0x7fa556`); a channel beam
    /// ignores it and lives until swept.
    pub bolt_life_ms: u32,
    /// Column 6: the delay in ms before the first hop lights (`0x7fa4e3`).
    pub first_bolt_delay_ms: u32,
    /// Column 31 (`DelayBetweenEffects`), a float the client truncates to ms: the stagger between
    /// hops, hop `i` lighting at `first + i × this` (`0x7fa569`). 0 on most rows, Chain
    /// Lightning's among them, so its hops light together.
    pub bolt_stagger_ms: u32,
    /// Column 7: the texture path as stored (`Textures\SpellChainEffects\*.blp`), read by the
    /// chain creator at `0x7fc782`.
    pub texture: String,
}

/// Column 31, `DelayBetweenEffects`.
const DELAY_BETWEEN_EFFECTS: usize = 31;
/// Columns 39-43, one byte each.
const BYTE_COLUMNS: std::ops::Range<usize> = 39..44;
/// Column 44, `Combo`, the second string.
const COMBO: usize = 44;

/// A kit's chain CharProc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainProc {
    /// `CharParamZero` truncated, the [`ChainEffect`] id; never 0, as the client no-ops an id with
    /// no row (`0x7fc634`).
    pub effect_id: u32,
    /// `CharParamTwo != 0` (`0x726abf`): the creator publishes the beam to the unit's owner slots,
    /// which the channel teardown sweeps (`0x7fc8a5`); set on channel-stage kits.
    pub flag: bool,
    /// The stored type, 0 or 12; behaviour keys off [`Self::flag`], as the dispatcher never forks.
    pub ty: i32,
}

pub(crate) fn chain_effects_schema() -> Schema {
    let mut s = Schema::new("SpellChainEffects");
    let mut add = |name: &str, ty| s.add_field(SchemaField::new(name.to_string(), ty));
    add("ID", FieldType::UInt32);
    add("AvgSegLen", FieldType::Float32);
    add("HalfWidth", FieldType::Float32);
    add("NoiseScale", FieldType::Float32);
    add("ScrollPeriod", FieldType::Float32);
    add("BoltLife", FieldType::UInt32);
    add("FirstBoltDelay", FieldType::UInt32);
    add("Texture", FieldType::String);
    for i in 8..SPELL_CHAIN_EFFECTS_FIELDS {
        let ty = match i {
            DELAY_BETWEEN_EFFECTS => FieldType::Float32,
            i if BYTE_COLUMNS.contains(&i) => FieldType::UInt8,
            COMBO => FieldType::String,
            _ => FieldType::UInt32,
        };
        add(&format!("F{i}"), ty);
    }
    debug_assert_eq!(s.fields.len(), SPELL_CHAIN_EFFECTS_FIELDS);
    s
}

/// Read `SpellChainEffects.dbc`; a textureless row stays, as the client's constructor never reads
/// the texture.
pub(super) fn load(chain: &mut Chain) -> Result<HashMap<u32, ChainEffect>> {
    let bytes = chain
        .read_file(SPELL_CHAIN_EFFECTS)
        .with_context(|| format!("reading {SPELL_CHAIN_EFFECTS}"))?;
    let set = parse(&bytes, chain_effects_schema(), "SpellChainEffects.dbc")?;
    let mut rows = HashMap::with_capacity(set.records().len());
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id,
            ChainEffect {
                avg_seg_len: f32_at(r, 1).unwrap_or(0.0),
                half_width: f32_at(r, 2).unwrap_or(0.0),
                noise_scale: f32_at(r, 3).unwrap_or(0.0),
                scroll_period_s: f32_at(r, 4).unwrap_or(0.0),
                bolt_life_ms: u32_at(r, 5).unwrap_or(0),
                first_bolt_delay_ms: u32_at(r, 6).unwrap_or(0),
                // `_ftol` under a truncating control word; every shipped value is whole and non-negative.
                bolt_stagger_ms: f32_at(r, DELAY_BETWEEN_EFFECTS).unwrap_or(0.0) as u32,
                texture: str_at(&set, r, 7).unwrap_or_default(),
            },
        );
    }
    Ok(rows)
}
