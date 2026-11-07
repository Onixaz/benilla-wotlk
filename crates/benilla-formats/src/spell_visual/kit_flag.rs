//! `SpellVisualKit` column 37, the kit flags benilla reads.

/// The kit's anim plays as authored: play flag `0x2`, no Special remap, no model fallback (`0x73b198`).
pub const RAW_ANIM: u32 = 0x4;
/// The kit's anim skips the Special remap from the held hands: play flag `0x8` (`0x73b1ae`).
pub const NO_SPECIAL_REMAP: u32 = 0x10;
/// A stage-2 model re-arms its birth sequence at each end instead of handing over to `Hold` (`0x7449e2`).
pub const AURA_LOOPS_BIRTH: u32 = 0x20;
