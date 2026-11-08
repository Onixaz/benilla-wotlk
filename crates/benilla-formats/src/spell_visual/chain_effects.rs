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
use benilla_dbc::{FieldType, Schema, SchemaField, StringRef, Value};

use crate::dbc::{f32_at, i32_at, parse, str_at, u32_at};
use crate::Chain;

const SPELL_CHAIN_EFFECTS: &str = "DBFilesClient\\SpellChainEffects.dbc";
const SPELL_CHAIN_EFFECTS_FIELDS: usize = 48;

/// The row flags the renderer reads (column 8).
pub mod chain_flag {
    /// Column 9 fixes the joint count instead of the length (`0x9ab2e0`).
    pub const FIXED_JOINTS: u32 = 0x1;
    /// Points built through the minor/major joint hierarchy (`0x9aa210` → `0x9a9dc0`).
    pub const HIERARCHY: u32 = 0x2;
    /// A joint whose jump timer runs out re-rolls its position (`0x9ab3b0`).
    pub const JOINT_JUMPS: u32 = 0x4;
    /// The wave spirals about the beam instead of swinging in one plane (`0x9a9b30`).
    pub const SPIRAL_WAVE: u32 = 0x8;
    /// The beam flickers on and off (`0x9ab3b0`).
    pub const FLICKER: u32 = 0x10;
    /// A pulse of alpha runs along the beam (`0x9a9490`).
    pub const PULSE: u32 = 0x20;
    /// The pulse restarts once it has run off the end (`0x9ab3b0`).
    pub const PULSE_LOOPS: u32 = 0x40;
    /// One cross-section normal for the whole beam, from its midpoint (`0x9aa210`).
    pub const FLAT: u32 = 0x80;
    /// The texture tiles once per column 46 yards (`0x9aa210`).
    pub const TILE_BY_LENGTH: u32 = 0x100;
    /// With [`TILE_BY_LENGTH`], the texture also slides by the tiled length (`0x9aa210`).
    pub const TILE_OFFSET: u32 = 0x200;
    /// The wave starts at column 47's phase instead of a random one (`0x9a8ec0`).
    pub const FIXED_WAVE_PHASE: u32 = 0x400;
}

/// The most rows one play draws, the root and its combo walk (`0x7fc92a`, `0x30` bytes of ids).
pub const MAX_COMBO_ROWS: usize = 12;

/// One row: the shape, animation and look of one beam.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChainEffect {
    /// Column 1: the target segment length; `trunc(length / this) + 3` joints (`0x9ab2e0`).
    pub avg_seg_len: f32,
    /// Column 2: the half-width; the ribbon spans twice this in yards.
    pub half_width: f32,
    /// Column 3: the joint offset radius per yard of beam (`0x9ab3b0`).
    pub noise_scale: f32,
    /// Column 4: the texture scroll in tiles per second, wrapped on 1 (`0x9ab3b0`).
    pub tex_scroll_speed: f32,
    /// Column 5: how long one hop burns in ms, from its own start (`0x7fa53f`).
    pub bolt_life_ms: u32,
    /// Column 6: the delay in ms before the first hop lights (`0x7fa4e2`).
    pub first_bolt_delay_ms: u32,
    /// Column 7: the texture path as stored, read by the chain creator at `0x7fc782`.
    pub texture: String,
    /// Column 8, the [`chain_flag`] bits.
    pub flags: u32,
    /// Column 9: the joint count under [`chain_flag::FIXED_JOINTS`].
    pub joint_count: i32,
    /// Column 10: the joint offset radius in yards, added to [`Self::noise_scale`]'s share.
    pub joint_offset_radius: f32,
    /// Column 11: joints per minor joint.
    pub joints_per_minor_joint: i32,
    /// Column 12: minor joints per major joint.
    pub minor_joints_per_major_joint: i32,
    /// Column 13: the joint offset scale at minor joints.
    pub minor_joint_scale: f32,
    /// Column 14: the joint offset scale at major joints.
    pub major_joint_scale: f32,
    /// Column 15: the joint drift speed in yards per second.
    pub joint_move_speed: f32,
    /// Column 16: the weight of a joint's last smoothed offset against its new one.
    pub joint_smoothness: f32,
    /// Columns 17-18: the range of seconds between joint jumps.
    pub joint_jump_s: [f32; 2],
    /// Column 19: the wave's height in yards.
    pub wave_height: f32,
    /// Column 20: the wave's frequency in radians per yard.
    pub wave_freq: f32,
    /// Column 21: the wave's phase speed in radians per second.
    pub wave_speed: f32,
    /// Columns 22-23: the range of the wave's starting angle about the beam.
    pub wave_angle: [f32; 2],
    /// Columns 24-25: the range of the wave's spin about the beam.
    pub wave_spin: [f32; 2],
    /// Column 26: the arc's height in yards.
    pub arc_height: f32,
    /// Columns 27-28: the range of the arc's starting angle about the beam.
    pub arc_angle: [f32; 2],
    /// Columns 29-30: the range of the arc's spin about the beam.
    pub arc_spin: [f32; 2],
    /// Column 31, a float truncated to ms: hop `i` lights at `first + i × this` (`0x7fa569`).
    pub bolt_stagger_ms: u32,
    /// Columns 32-33: the range of seconds a flickering beam stays on.
    pub flicker_on_s: [f32; 2],
    /// Columns 34-35: the range of seconds a flickering beam stays off.
    pub flicker_off_s: [f32; 2],
    /// Column 36: the pulse's speed in yards per second; negative runs it target to caster.
    pub pulse_speed: f32,
    /// Column 37: the pulse's lit length in yards.
    pub pulse_on_length: f32,
    /// Column 38: the pulse's fade length in yards at each end.
    pub pulse_fade_length: f32,
    /// Columns 40, 41, 42 and 39: the beam's red, green, blue and alpha (`0x9a9900`).
    pub rgba: [u8; 4],
    /// Column 43: the render state's blend mode, drawn with depth write only when 2 (`0x9aad37`).
    pub blend_mode: u8,
    /// Column 44 decoded (`0x9a8ce0`): the further rows one play draws, in order.
    pub combo: Vec<u32>,
    /// Column 45: the pass, 0 to 3, the beam draws in (`0x9ab070`).
    pub render_layer: i32,
    /// Column 46: yards per texture tile under [`chain_flag::TILE_BY_LENGTH`].
    pub texture_length: f32,
    /// Column 47: the wave's starting phase under [`chain_flag::FIXED_WAVE_PHASE`].
    pub wave_phase: f32,
}

impl ChainEffect {
    /// Whether column 8 carries `flag`, one of [`chain_flag`].
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

/// Column 31, `DelayBetweenEffects`.
const DELAY_BETWEEN_EFFECTS: usize = 31;
/// Columns 39-43, one byte each.
const BYTE_COLUMNS: std::ops::Range<usize> = 39..44;
/// Column 44, `Combo`, the second string.
const COMBO: usize = 44;
/// The integer columns; every other column but the id, the bytes and the strings is a float.
const INT_COLUMNS: [usize; 7] = [5, 6, 8, 9, 11, 12, 45];

/// A kit's chain CharProc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainProc {
    /// `CharParamZero` truncated, the [`ChainEffect`] id; an id with no row no-ops (`0x7fc634`).
    pub effect_id: u32,
    /// `CharParamTwo != 0` (`0x726abf`): the beam persists until the channel ends (`0x7fc8a5`).
    pub flag: bool,
    /// `CharParamThree != 0` (`0x726ab1`): every hop leaves from the caster (`0x7fa547`).
    pub reverse: bool,
    /// The stored type, 0 or 12; behaviour keys off [`Self::flag`], as the dispatcher never forks.
    pub ty: i32,
}

pub(crate) fn chain_effects_schema() -> Schema {
    let mut s = Schema::new("SpellChainEffects");
    let mut add = |name: &str, ty| s.add_field(SchemaField::new(name.to_string(), ty));
    add("ID", FieldType::UInt32);
    for i in 1..SPELL_CHAIN_EFFECTS_FIELDS {
        let ty = match i {
            7 | COMBO => FieldType::String,
            i if BYTE_COLUMNS.contains(&i) => FieldType::UInt8,
            i if INT_COLUMNS.contains(&i) => FieldType::Int32,
            _ => FieldType::Float32,
        };
        add(&format!("F{i}"), ty);
    }
    debug_assert_eq!(s.fields.len(), SPELL_CHAIN_EFFECTS_FIELDS);
    s
}

/// The combo id decode (`0x9a8ce0`): the high byte carries the three low bytes' low bits.
pub fn decode_combo_id(id: u32) -> u32 {
    let bits = id & 0xFEFE_FEFE;
    let mut high = (bits >> 7) & 0x20_0000;
    high = ((high | (bits & 0x2000_0000)) >> 3) | (bits & 0x200_0000);
    high = ((high >> 4) | (bits & 0x4000_0000)) >> 3;
    high |= bits & 0x400_0000;
    (high >> 11) | ((((bits >> 4) & 0xFF80_0000) | bits) & 0xFF_FFFF)
}

/// The DBC's string block, which the combo walk reads in place.
fn string_block(bytes: &[u8]) -> &[u8] {
    let rd = |o: usize| {
        bytes
            .get(o..o + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let start = 20 + rd(4) * rd(12);
    let end = start.saturating_add(rd(16)).min(bytes.len());
    bytes.get(start..end).unwrap_or_default()
}

/// The combo walk's ids (`0x7fc8ea`): words from the string's offset, read on past its terminator.
fn combo_ids(strings: &[u8], offset: u32) -> Vec<u32> {
    let word = |k: usize| {
        let at = offset as usize + 4 * k;
        let b: [u8; 4] = std::array::from_fn(|i| strings.get(at + i).copied().unwrap_or(0));
        decode_combo_id(u32::from_le_bytes(b))
    };
    (0..MAX_COMBO_ROWS - 1)
        .map(word)
        .take_while(|&id| id != 0)
        .collect()
}

/// Read `SpellChainEffects.dbc`; a textureless row stays, as the texture is only read at use.
pub(super) fn load(chain: &mut Chain) -> Result<HashMap<u32, ChainEffect>> {
    let bytes = chain
        .read_file(SPELL_CHAIN_EFFECTS)
        .with_context(|| format!("reading {SPELL_CHAIN_EFFECTS}"))?;
    let set = parse(&bytes, chain_effects_schema(), "SpellChainEffects.dbc")?;
    let strings = string_block(&bytes);
    let mut rows = HashMap::with_capacity(set.records().len());
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let f = |i: usize| f32_at(r, i).unwrap_or(0.0);
        let int = |i: usize| i32_at(r, i).unwrap_or(0);
        let byte = |i: usize| u32_at(r, i).unwrap_or(0) as u8;
        let pair = |a: usize| [f(a), f(a + 1)];
        let combo = match r.get_value(COMBO) {
            Some(Value::StringRef(StringRef(off))) => combo_ids(strings, *off),
            _ => Vec::new(),
        };
        rows.insert(
            id,
            ChainEffect {
                avg_seg_len: f(1),
                half_width: f(2),
                noise_scale: f(3),
                tex_scroll_speed: f(4),
                bolt_life_ms: int(5) as u32,
                first_bolt_delay_ms: int(6) as u32,
                texture: str_at(&set, r, 7).unwrap_or_default(),
                flags: int(8) as u32,
                joint_count: int(9),
                joint_offset_radius: f(10),
                joints_per_minor_joint: int(11),
                minor_joints_per_major_joint: int(12),
                minor_joint_scale: f(13),
                major_joint_scale: f(14),
                joint_move_speed: f(15),
                joint_smoothness: f(16),
                joint_jump_s: pair(17),
                wave_height: f(19),
                wave_freq: f(20),
                wave_speed: f(21),
                wave_angle: pair(22),
                wave_spin: pair(24),
                arc_height: f(26),
                arc_angle: pair(27),
                arc_spin: pair(29),
                // `_ftol`; every shipped value is whole and non-negative.
                bolt_stagger_ms: f(DELAY_BETWEEN_EFFECTS) as u32,
                flicker_on_s: pair(32),
                flicker_off_s: pair(34),
                pulse_speed: f(36),
                pulse_on_length: f(37),
                pulse_fade_length: f(38),
                rgba: [byte(40), byte(41), byte(42), byte(39)],
                blend_mode: byte(43),
                combo,
                render_layer: int(45),
                texture_length: f(46),
                wave_phase: f(47),
            },
        );
    }
    // The walk also stops at the first id naming no row (`0x7fc906`-`0x7fc925`).
    let ids: std::collections::HashSet<u32> = rows.keys().copied().collect();
    for row in rows.values_mut() {
        if let Some(end) = row.combo.iter().position(|id| !ids.contains(id)) {
            row.combo.truncate(end);
        }
        row.combo.shrink_to_fit();
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decode inverts the encoding: each byte's low bit moves to the high byte.
    #[test]
    fn the_combo_decode_restores_the_low_bits() {
        // Every byte's low bit set, so no byte is 0; bits 0, 8 and 16 ride in bits 28-30.
        let encode = |id: u32| {
            (id & 0x00FE_FEFE)
                | 0x0101_0101
                | (id & 1) << 28
                | ((id >> 8) & 1) << 29
                | ((id >> 16) & 1) << 30
        };
        for id in [1u32, 2, 743, 0x00FF_FFFF, 1178, 0x0001_0101] {
            let word = encode(id);
            assert!(word.to_le_bytes().iter().all(|&b| b != 0), "{id:#x}");
            assert_eq!(decode_combo_id(word), id, "{id:#x}");
        }
        assert_eq!(decode_combo_id(0), 0);
    }

    #[test]
    fn the_walk_stops_at_a_zero_word_and_reads_past_the_terminator() {
        // An empty string at 0, then a word of zeros: nothing to walk.
        assert!(combo_ids(&[0, 0, 0, 0, 0], 0).is_empty());
        // Past the block's end reads as zeros.
        assert!(combo_ids(&[], 7).is_empty());
    }

    /// The shipped table: every combo id names a row and every blend mode is one of the twelve.
    #[test]
    fn real_rows_hold_their_shape_columns() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load(&mut chain).expect("load SpellChainEffects");
        let cat = crate::load_spell_visual_catalog(&mut chain).expect("load spell visuals");
        assert!(
            rows.values().any(|r| !r.combo.is_empty()),
            "some row walks a combo"
        );
        for (id, r) in &rows {
            assert!(r.combo.len() < MAX_COMBO_ROWS, "row {id}");
            assert!(r.combo.iter().all(|c| rows.contains_key(c)), "row {id}");
            assert!(r.blend_mode < 12, "row {id}: blend {}", r.blend_mode);
            assert!(r.flags < 0x800, "row {id}: flags {:#x}", r.flags);
        }
        // The byte columns sit where the renderer reads them: most rows are opaque-alpha.
        let lit = rows.values().filter(|r| r.rgba[3] == 0xff).count();
        assert!(
            lit * 2 > rows.len(),
            "{lit} of {} rows at alpha 255",
            rows.len()
        );
        // The catalog serves the same rows the loader reads.
        assert_eq!(cat.chain_effect_len(), rows.len());
    }
}
