//! The combat log wire, inbound only, each body as its 12340 handler reads it: spell damage,
//! periodic ticks, heals, power gains, damage shields, environmental damage, misses, kills, aura
//! removals, enchants, spell effect logs and building damage.

use std::io::{self, Read};

use crate::wire::{
    capacity_hint, read_f32_le, read_i32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8,
};

/// `SMSG_SPELLNONMELEEDAMAGELOG` hit-info bits the handler `0x6d3840` tests.
pub mod spell_hit_info {
    /// A crit (`0x6d391a`).
    pub const CRIT: u32 = 0x2;
    /// Damage split off another unit, logged as `DAMAGE_SPLIT` (`0x6d3970`).
    pub const SPLIT: u32 = 0x8;
    /// The target is also the source (`0x6d3a10`).
    pub const SOURCE_IS_TARGET: u32 = 0x10;
}

/// `SMSG_SPELLNONMELEEDAMAGELOG`, spell damage dealt (`0x6d3840`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellDamageLog {
    pub target: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub damage: u32,
    pub overkill: u32,
    /// The school mask, one byte on this wire.
    pub school: u8,
    pub absorbed: u32,
    pub resisted: u32,
    pub periodic: bool,
    pub blocked: u32,
    /// [`spell_hit_info`] bits.
    pub hit_info: u32,
}

/// `0x6d3840`: two packed guids, spell, damage, overkill, `u8` school, absorbed, resisted, `u8`
/// periodic, `u8`, blocked, hit info, then a `u8` that adds debug floats by hit-info bit.
pub(super) fn read_spell_damage_log(r: &mut impl Read) -> io::Result<SpellDamageLog> {
    let log = SpellDamageLog {
        target: read_packed_guid(r)?,
        caster: read_packed_guid(r)?,
        spell_id: read_u32_le(r)?,
        damage: read_u32_le(r)?,
        overkill: read_u32_le(r)?,
        school: read_u8(r)?,
        absorbed: read_u32_le(r)?,
        resisted: read_u32_le(r)?,
        periodic: read_u8(r)? != 0,
        blocked: {
            let _unused = read_u8(r)?;
            read_u32_le(r)?
        },
        hit_info: read_u32_le(r)?,
    };
    if read_u8(r)? != 0 {
        // `0x6d3b3e`-`0x6d3bc8`: two floats per bit 0x1 and 0x4, six for 0x20.
        let floats = [(0x1, 2), (0x4, 2), (0x20, 6)]
            .into_iter()
            .filter(|&(bit, _)| log.hit_info & bit != 0)
            .map(|(_, n)| n)
            .sum::<u32>();
        for _ in 0..floats {
            read_f32_le(r)?;
        }
    }
    Ok(log)
}

/// One `SMSG_PERIODICAURALOG` tick, shaped by its aura type (`0x7532c7`'s jump table).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PeriodicTick {
    /// Aura types 3 and 89.
    Damage {
        amount: u32,
        overkill: u32,
        school: u32,
        absorbed: u32,
        resisted: u32,
        crit: bool,
    },
    /// Aura types 8 and 20.
    Heal {
        amount: u32,
        overheal: u32,
        absorbed: u32,
        crit: bool,
    },
    /// Aura types 21 and 24.
    Energize { power: u32, amount: u32 },
    /// Aura type 64, a mana leech.
    Leech {
        power: u32,
        amount: u32,
        multiplier: f32,
    },
}

/// `SMSG_PERIODICAURALOG`, periodic aura ticks (`0x7531f0`).
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicAuraLog {
    pub target: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub ticks: Vec<PeriodicTick>,
}

/// `0x7531f0`; an aura type with no arm ends the read and skips the rest of the body (`0x75367a`).
pub(super) fn read_periodic_aura_log(r: &mut impl Read) -> io::Result<PeriodicAuraLog> {
    let target = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    let mut ticks = Vec::with_capacity(capacity_hint(count, 8));
    for _ in 0..count {
        let tick = match read_u32_le(r)? {
            3 | 89 => PeriodicTick::Damage {
                amount: read_u32_le(r)?,
                overkill: read_u32_le(r)?,
                school: read_u32_le(r)?,
                absorbed: read_u32_le(r)?,
                resisted: read_u32_le(r)?,
                crit: read_u8(r)? != 0,
            },
            8 | 20 => PeriodicTick::Heal {
                amount: read_u32_le(r)?,
                overheal: read_u32_le(r)?,
                absorbed: read_u32_le(r)?,
                crit: read_u8(r)? != 0,
            },
            21 | 24 => PeriodicTick::Energize {
                power: read_u32_le(r)?,
                amount: read_u32_le(r)?,
            },
            64 => PeriodicTick::Leech {
                power: read_u32_le(r)?,
                amount: read_u32_le(r)?,
                multiplier: read_f32_le(r)?,
            },
            _ => {
                io::copy(r, &mut io::sink())?;
                break;
            }
        };
        ticks.push(tick);
    }
    Ok(PeriodicAuraLog {
        target,
        caster,
        spell_id,
        ticks,
    })
}

/// `SMSG_SPELLHEALLOG`, a direct heal landing (`0x6d3c30`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellHealLog {
    pub target: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub amount: u32,
    pub overheal: u32,
    pub absorbed: u32,
    pub crit: bool,
}

/// `0x6d3c30`: two packed guids, spell, amount, overheal, absorbed, `u8` crit, `u8`.
pub(super) fn read_spell_heal_log(r: &mut impl Read) -> io::Result<SpellHealLog> {
    let log = SpellHealLog {
        target: read_packed_guid(r)?,
        caster: read_packed_guid(r)?,
        spell_id: read_u32_le(r)?,
        amount: read_u32_le(r)?,
        overheal: read_u32_le(r)?,
        absorbed: read_u32_le(r)?,
        crit: read_u8(r)? != 0,
    };
    let _unused = read_u8(r)?;
    Ok(log)
}

/// `SMSG_SPELLENERGIZELOG`, an instant power gain (`0x6d3df0`); `power` is the power type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellEnergizeLog {
    pub target: u64,
    pub caster: u64,
    pub spell_id: u32,
    pub power: u32,
    pub amount: u32,
}

pub(super) fn read_spell_energize_log(r: &mut impl Read) -> io::Result<SpellEnergizeLog> {
    Ok(SpellEnergizeLog {
        target: read_packed_guid(r)?,
        caster: read_packed_guid(r)?,
        spell_id: read_u32_le(r)?,
        power: read_u32_le(r)?,
        amount: read_u32_le(r)?,
    })
}

/// `SMSG_SPELLDAMAGESHIELD`, a damage shield striking back (`0x6d3750`): `victim` bears the
/// shield, `attacker` struck it and takes this damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageShield {
    pub victim: u64,
    pub attacker: u64,
    pub spell_id: u32,
    pub damage: u32,
    pub overkill: u32,
    pub school: u32,
}

/// `0x6d3750`: two raw guids, then spell, damage, overkill and school mask.
pub(super) fn read_damage_shield(r: &mut impl Read) -> io::Result<DamageShield> {
    Ok(DamageShield {
        victim: read_u64_le(r)?,
        attacker: read_u64_le(r)?,
        spell_id: read_u32_le(r)?,
        damage: read_u32_le(r)?,
        overkill: read_u32_le(r)?,
        school: read_u32_le(r)?,
    })
}

/// `SMSG_ENVIRONMENTALDAMAGELOG`, environmental damage taken (`0x756800`). `damage_type` is 0
/// fatigue, 1 drowning, 2 falling, 3 lava, 4 slime, 5 fire (`0xad671c`), the row of
/// `EnvironmentalDamage.dbc` whose visual kit plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentalDamageLog {
    pub victim: u64,
    pub damage_type: u8,
    pub damage: u32,
    pub absorb: u32,
    pub resist: u32,
}

/// Read `SMSG_ENVIRONMENTALDAMAGELOG`; the victim guid is raw.
pub(super) fn read_environmental_damage_log(
    r: &mut impl Read,
) -> io::Result<EnvironmentalDamageLog> {
    Ok(EnvironmentalDamageLog {
        victim: read_u64_le(r)?,
        damage_type: read_u8(r)?,
        damage: read_u32_le(r)?,
        absorb: read_u32_le(r)?,
        resist: read_u32_le(r)?,
    })
}

/// `SMSG_SPELLLOGMISS`, a damage shield's per-target misses (`0x6d3090`); each `u8` is a miss
/// type, 1 miss to 11 reflect.
#[derive(Debug, Clone, PartialEq)]
pub struct SpellLogMiss {
    pub spell_id: u32,
    pub caster: u64,
    /// True when each row carries two debug floats, which the handler then does not log.
    pub debug: bool,
    pub misses: Vec<(u64, u8)>,
}

pub(super) fn read_spell_log_miss(r: &mut impl Read) -> io::Result<SpellLogMiss> {
    let spell_id = read_u32_le(r)?;
    let caster = read_u64_le(r)?;
    let debug = read_u8(r)? != 0;
    let count = read_u32_le(r)?;
    let mut misses = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        let target = read_u64_le(r)?;
        let miss_info = read_u8(r)?;
        if debug {
            read_f32_le(r)?;
            read_f32_le(r)?;
        }
        misses.push((target, miss_info));
    }
    Ok(SpellLogMiss {
        spell_id,
        caster,
        debug,
        misses,
    })
}

/// `SMSG_PARTYKILLLOG`, the killing blow, sent to the killer's party (`0x750c90`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartyKillLog {
    pub killer: u64,
    pub victim: u64,
}

pub(super) fn read_party_kill_log(r: &mut impl Read) -> io::Result<PartyKillLog> {
    Ok(PartyKillLog {
        killer: read_u64_le(r)?,
        victim: read_u64_le(r)?,
    })
}

/// `SMSG_SPELLINSTAKILLLOG`, an instant kill (`0x6ce260`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellInstaKillLog {
    pub caster: u64,
    pub victim: u64,
    pub spell_id: u32,
}

/// `0x6ce260`: caster and victim as raw guids, then the spell.
pub(super) fn read_spell_insta_kill_log(r: &mut impl Read) -> io::Result<SpellInstaKillLog> {
    Ok(SpellInstaKillLog {
        caster: read_u64_le(r)?,
        victim: read_u64_le(r)?,
        spell_id: read_u32_le(r)?,
    })
}

/// `SMSG_PROCRESIST` (`0x750d40`) or `SMSG_SPELLORDAMAGE_IMMUNE` (`0x6ce3d0`), which share one
/// body: two raw guids, the spell and a byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellOutcomeLog {
    pub caster: u64,
    pub target: u64,
    pub spell_id: u32,
    /// The immune log's periodic flag (`0x6ce439`); the resist log reads and drops it.
    pub periodic: bool,
}

pub(super) fn read_spell_outcome_log(r: &mut impl Read) -> io::Result<SpellOutcomeLog> {
    Ok(SpellOutcomeLog {
        caster: read_u64_le(r)?,
        target: read_u64_le(r)?,
        spell_id: read_u32_le(r)?,
        periodic: read_u8(r)? != 0,
    })
}

/// Which of the three aura-removal opcodes a [`SpellRemoveLog`] came on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellRemoveKind {
    /// `SMSG_SPELLBREAKLOG`: auras broken by damage or a spell.
    Break,
    /// `SMSG_SPELLDISPELLOG`.
    Dispel,
    /// `SMSG_SPELLSTEALLOG`.
    Steal,
}

/// The auras a dispel, a steal or a break removed; the three opcodes share `0x6ce2b0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellRemoveLog {
    pub kind: SpellRemoveKind,
    /// The unit the auras were on.
    pub target: u64,
    /// The dispeller, the thief or the breaker.
    pub caster: u64,
    /// The removing spell; 0 for a break by damage.
    pub spell_id: u32,
    /// `(aura spell, harmful)` per removed aura.
    pub auras: Vec<(u32, bool)>,
}

/// `0x6ce2b0`: two packed guids, the removing spell, a `u8`, a count, then `u32` spell and `u8`
/// harmful per row.
pub(super) fn read_spell_remove_log(
    r: &mut impl Read,
    kind: SpellRemoveKind,
) -> io::Result<SpellRemoveLog> {
    let target = read_packed_guid(r)?;
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let _unused = read_u8(r)?;
    let count = read_u32_le(r)?;
    let mut auras = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        auras.push((read_u32_le(r)?, read_u8(r)? != 0));
    }
    Ok(SpellRemoveLog {
        kind,
        target,
        caster,
        spell_id,
        auras,
    })
}

/// `SMSG_DISPEL_FAILED`, the auras a dispel failed to remove (`0x750ea0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispelFailed {
    pub caster: u64,
    pub target: u64,
    /// The dispelling spell.
    pub spell_id: u32,
    /// The auras that stayed, read to the end of the body.
    pub failed: Vec<u32>,
}

pub(super) fn read_dispel_failed(r: &mut impl Read) -> io::Result<DispelFailed> {
    let caster = read_u64_le(r)?;
    let target = read_u64_le(r)?;
    let spell_id = read_u32_le(r)?;
    let mut failed = Vec::new();
    loop {
        let mut b = [0u8; 4];
        match r.read_exact(&mut b) {
            Ok(()) => failed.push(u32::from_le_bytes(b)),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
    }
    Ok(DispelFailed {
        caster,
        target,
        spell_id,
        failed,
    })
}

/// `SMSG_ENCHANTMENTLOG`, an enchant landing on or fading from an item (`0x751050`); a zero
/// `caster` means it faded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentLog {
    /// The item's owner.
    pub target: u64,
    pub caster: u64,
    pub item_entry: u32,
    /// The `SpellItemEnchantment.dbc` row.
    pub enchant_id: u32,
}

/// `0x751050`: two packed guids, the item entry, the enchantment id.
pub(super) fn read_enchantment_log(r: &mut impl Read) -> io::Result<EnchantmentLog> {
    Ok(EnchantmentLog {
        target: read_packed_guid(r)?,
        caster: read_packed_guid(r)?,
        item_entry: read_u32_le(r)?,
        enchant_id: read_u32_le(r)?,
    })
}

/// One `SMSG_SPELLLOGEXECUTE` row, shaped by its effect (`0x6d32f6`'s jump table).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExecuteLog {
    /// Effects 8 and 62, a power drain: amount, then power type.
    PowerDrain {
        target: u64,
        amount: u32,
        power: u32,
        multiplier: f32,
    },
    /// Effects 18 and 113, a resurrection.
    Resurrect { target: u64 },
    /// Effect 19, extra attacks.
    ExtraAttacks { target: u64, count: u32 },
    /// Effects 24, 59 and 157, an item created; no target on the wire.
    CreateItem { item_entry: u32 },
    /// Effects 28, 50, 56, 76, 81, 83 and 104-107, a summon.
    Summon { target: u64 },
    /// Effect 33, a lock opened.
    OpenLock { target: u64 },
    /// Effect 68, a cast interrupted: the interrupted spell.
    InterruptCast { target: u64, spell_id: u32 },
    /// Effect 101, a pet fed; no target on the wire.
    FeedPet { item_entry: u32 },
    /// Effect 102, a pet dismissed.
    DismissPet { target: u64 },
    /// Effect 111, durability damage; both fields `-1` is the all-items form.
    DurabilityDamage {
        target: u64,
        item_entry: i32,
        slot: i32,
    },
}

/// `SMSG_SPELLLOGEXECUTE`, what a cast's effects did (`0x6d3220`): one group of rows per effect.
#[derive(Debug, Clone, PartialEq)]
pub struct SpellLogExecute {
    pub caster: u64,
    pub spell_id: u32,
    /// `(effect, the rows it logged)`, in wire order.
    pub effects: Vec<(u32, Vec<ExecuteLog>)>,
}

/// `0x6d3220`; an effect with no arm reads no row bytes (the default `0x6d3636`).
pub(super) fn read_spell_log_execute(r: &mut impl Read) -> io::Result<SpellLogExecute> {
    let caster = read_packed_guid(r)?;
    let spell_id = read_u32_le(r)?;
    let group_count = read_u32_le(r)?;
    let mut effects = Vec::with_capacity(capacity_hint(group_count, 8));
    for _ in 0..group_count {
        let effect = read_u32_le(r)?;
        let rows = read_u32_le(r)?;
        let mut out = Vec::with_capacity(capacity_hint(rows, 64));
        for _ in 0..rows {
            let row = match effect {
                8 | 62 => ExecuteLog::PowerDrain {
                    target: read_packed_guid(r)?,
                    amount: read_u32_le(r)?,
                    power: read_u32_le(r)?,
                    multiplier: read_f32_le(r)?,
                },
                18 | 113 => ExecuteLog::Resurrect {
                    target: read_packed_guid(r)?,
                },
                19 => ExecuteLog::ExtraAttacks {
                    target: read_packed_guid(r)?,
                    count: read_u32_le(r)?,
                },
                24 | 59 | 157 => ExecuteLog::CreateItem {
                    item_entry: read_u32_le(r)?,
                },
                28 | 50 | 56 | 76 | 81 | 83 | 104..=107 => ExecuteLog::Summon {
                    target: read_packed_guid(r)?,
                },
                33 => ExecuteLog::OpenLock {
                    target: read_packed_guid(r)?,
                },
                68 => ExecuteLog::InterruptCast {
                    target: read_packed_guid(r)?,
                    spell_id: read_u32_le(r)?,
                },
                101 => ExecuteLog::FeedPet {
                    item_entry: read_u32_le(r)?,
                },
                102 => ExecuteLog::DismissPet {
                    target: read_packed_guid(r)?,
                },
                111 => ExecuteLog::DurabilityDamage {
                    target: read_packed_guid(r)?,
                    item_entry: read_i32_le(r)?,
                    slot: read_i32_le(r)?,
                },
                _ => continue,
            };
            out.push(row);
        }
        effects.push((effect, out));
    }
    Ok(SpellLogExecute {
        caster,
        spell_id,
        effects,
    })
}

/// `SMSG_DESTRUCTIBLE_BUILDING_DAMAGE`, a siege hit or repair on a building (`0x753730`); a
/// damage of zero or less is a repair of its negation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildingDamage {
    pub building: u64,
    pub caster: u64,
    /// The vehicle's owner, whose client floats the number.
    pub owner: u64,
    pub damage: i32,
    pub spell_id: u32,
}

/// `0x753730`: three packed guids, the signed damage and the spell.
pub(super) fn read_building_damage(r: &mut impl Read) -> io::Result<BuildingDamage> {
    Ok(BuildingDamage {
        building: read_packed_guid(r)?,
        caster: read_packed_guid(r)?,
        owner: read_packed_guid(r)?,
        damage: read_i32_le(r)?,
        spell_id: read_u32_le(r)?,
    })
}
