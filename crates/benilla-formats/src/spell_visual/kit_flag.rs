//! `SpellVisualKit` column 37, the kit flags benilla reads.

/// The kit's anim plays as authored: play flag `0x2`, no Special remap, no model fallback (`0x73b198`).
pub const RAW_ANIM: u32 = 0x4;
/// The kit's anim skips the unit's tier resolve but keeps the Special remap: play flag `0x4` (`0x73b1a6`).
pub const NO_TIER_RESOLVE: u32 = 0x8;
/// The kit's anim skips the Special remap from the held hands: play flag `0x8` (`0x73b1ae`).
pub const NO_SPECIAL_REMAP: u32 = 0x10;
/// A stage-2 model re-arms its birth sequence at each end instead of handing over to `Hold` (`0x7449e2`).
pub const AURA_LOOPS_BIRTH: u32 = 0x20;
/// A held precast or channel anim is played again at its base anim's end, anim flag `0x10` (`0x7243d4`).
pub const REPICK_HELD_ANIM: u32 = 0x40;
/// A missile replays the state kit only on a target whose aura has an effect applied (`0x702f2e`).
pub const ACTIVE_AURA_ONLY: u32 = 0x80;
/// The kit's anim keeps the weapons out: play flag `0x20` skips the clip's stow (`0x73b1b6`, `0x738180`).
pub const KEEP_WEAPONS: u32 = 0x100;
/// The kit's models' particles and ribbons keep their own alpha (`0x74483a`, `0x82e2a9`).
pub const OWN_MODEL_ALPHA: u32 = 0x200;
/// A CharProc-14 alpha fades the rider's body, not the mount it sits on (`0x72682e`, `0x71ac41`).
pub const FADE_BODY_ONLY: u32 = 0x400;
