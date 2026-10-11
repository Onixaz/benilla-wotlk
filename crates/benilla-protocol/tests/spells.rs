//! Wire tests for spells, action buttons, melee and the combat log (`src/messages/spells.rs`).

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;
use common::hx;

/// Every swing refusal has an empty body (vmangos `Server/Packets/Combat.cpp`). DEADTARGET (0x148)
/// and CANT_ATTACK (0x149) share the reference's arm `0x625ab8`, so they decode alike.
#[test]
fn attack_swing_refusal_wire() {
    use benilla_protocol::messages::AttackSwingError;

    for (op, want) in [
        (
            messages::opcode::SMSG_ATTACKSWING_NOTINRANGE,
            AttackSwingError::NotInRange,
        ),
        (
            messages::opcode::SMSG_ATTACKSWING_BADFACING,
            AttackSwingError::BadFacing,
        ),
        (
            messages::opcode::SMSG_ATTACKSWING_DEADTARGET,
            AttackSwingError::DeadOrUnattackable,
        ),
        (
            messages::opcode::SMSG_ATTACKSWING_CANT_ATTACK,
            AttackSwingError::DeadOrUnattackable,
        ),
    ] {
        let packet = messages::parse_server(op, &[]).unwrap_or_else(|e| panic!("{op:#x}: {e}"));
        match &packet {
            ServerPacket::AttackSwingError(got) => assert_eq!(*got, want, "opcode {op:#x}"),
            other => panic!("{op:#x} parsed as {}", other.name()),
        }
        match decode(packet).pop().unwrap() {
            SessionEvent::AttackSwingError(got) => assert_eq!(got, want, "opcode {op:#x} event"),
            other => panic!("{op:#x} event, got {other:?}"),
        }
    }

    // SMSG_CANCEL_COMBAT is the fourth arm: another opcode family, same empty body and effect.
    assert!(
        matches!(
            messages::parse_server(messages::opcode::SMSG_CANCEL_COMBAT, &[]).unwrap(),
            ServerPacket::CancelCombat
        ),
        "SMSG_CANCEL_COMBAT is registered (`0x5e3308` -> `0x5e7dd0`), just not by the combat TU"
    );
    assert!(matches!(
        decode(ServerPacket::CancelCombat).pop().unwrap(),
        SessionEvent::CancelCombat
    ));

    // SMSG_FEIGN_DEATH_RESISTED: in the reference only a `DisplayError(421)` (`0x6e9800`).
    assert!(matches!(
        messages::parse_server(messages::opcode::SMSG_FEIGN_DEATH_RESISTED, &[]).unwrap(),
        ServerPacket::FeignDeathResisted
    ));
    assert!(matches!(
        decode(ServerPacket::FeignDeathResisted).pop().unwrap(),
        SessionEvent::FeignDeathResisted
    ));

    // 0x147 SMSG_ATTACKSWING_NOTSTANDING stays unknown: the reference never registers it (default
    // arm `0x625ade`) and vmangos never sends it.
    assert!(
        matches!(
            messages::parse_server(0x0147, &[]).unwrap(),
            ServerPacket::Other { opcode: 0x0147 }
        ),
        "0x147 NOTSTANDING must stay unparsed — the reference never registers it"
    );
}

/// Combat-log bodies per vmangos `Server/Packets/Spell.cpp:68-86,124-140`, `Unit.cpp:4395-4443`,
/// `Server/Packets/Combat.cpp:73-79` and `Server/Packets/Misc.cpp:512-532`.
#[test]
fn combat_log_wire_golden() {
    use benilla_protocol::messages::{
        DamageShield, EnvironmentalDamageLog, ExplorationXp, LevelUpInfo, PeriodicAuraLog,
        PeriodicTick, SpellDamageLog, SpellLogMiss, XpGain,
    };

    let creature = 0xF130_0000_4500_002Au64; // packs with mask 0xC9

    let body = hx(concat!(
        "c92a4530f1", // target pguid: the creature
        "0101",       // attacker pguid: guid 1
        "85000000",   // spellId 133
        "f4010000",   // damage 500
        "03",         // school 3 (frost)
        "32000000",   // absorbed 50
        "ecffffff",   // resist -20
        "00",         // periodicLog: false (direct, not a DoT tick)
        "00",         // unused
        "0a000000",   // blocked 10
        "02000000",   // hitInfo 0x2 (SPELL_HIT_TYPE_CRIT)
        "00",         // extendedData (always 0, dropped)
    ));
    let packet =
        messages::parse_server(messages::opcode::SMSG_SPELLNONMELEEDAMAGELOG, &body).unwrap();
    match &packet {
        ServerPacket::SpellDamageLog(s) => {
            assert_eq!(
                s,
                &SpellDamageLog {
                    target: creature,
                    attacker: 1,
                    spell_id: 133,
                    damage: 500,
                    school: 3,
                    absorb: 50,
                    resist: -20,
                    periodic: false,
                    blocked: 10,
                    hit_info: 0x2,
                }
            );
        }
        other => panic!("spell damage log, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellDamageLog(s) => assert_eq!(s.hit_info & 0x2, 0x2, "crit bit"),
        other => panic!("spell damage log event, got {other:?}"),
    }

    // SMSG_PERIODICAURALOG: target, caster, spellId, then count x {u32 auraType, payload}.
    let body = hx(concat!(
        "c92a4530f1", // target pguid: the creature
        "0101",       // caster pguid: guid 1
        "ac000000",   // spellId 172 (Corruption)
        "01000000",   // count 1
        "03000000",   // auraType 3 (PERIODIC_DAMAGE)
        "58000000",   // damage 88
        "06000000",   // school 6 (shadow)
        "0c000000",   // absorb 12
        "fbffffff",   // resist -5
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_PERIODICAURALOG, &body).unwrap();
    match &packet {
        ServerPacket::PeriodicAuraLog(l) => {
            assert_eq!(
                l,
                &PeriodicAuraLog {
                    target: creature,
                    caster: 1,
                    spell_id: 172,
                    ticks: vec![PeriodicTick::Damage {
                        amount: 88,
                        school: 6,
                        absorb: 12,
                        resist: -5,
                    }],
                }
            );
        }
        other => panic!("periodic aura log (damage), got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::PeriodicAuraLog(l) => assert_eq!(l.ticks.len(), 1),
        other => panic!("periodic aura log event, got {other:?}"),
    }

    let body = hx(concat!(
        "0101",     // target pguid: guid 1
        "0101",     // caster pguid: guid 1 (self)
        "59000000", // spellId 89 (Renew)
        "01000000", // count 1
        "08000000", // auraType 8 (PERIODIC_HEAL)
        "2d000000", // amount 45
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_PERIODICAURALOG, &body).unwrap();
    match &packet {
        ServerPacket::PeriodicAuraLog(l) => {
            assert_eq!(
                l,
                &PeriodicAuraLog {
                    target: 1,
                    caster: 1,
                    spell_id: 89,
                    ticks: vec![PeriodicTick::Heal { amount: 45 }],
                }
            );
        }
        other => panic!("periodic aura log (heal), got {}", other.name()),
    }

    // An unhandled auraType has no known payload width, so the decode fails rather than desync.
    let body = hx(concat!(
        "0101",     // target
        "0101",     // caster
        "01000000", // spellId 1
        "01000000", // count 1
        "04000000", // auraType 4 (unhandled)
    ));
    assert!(messages::parse_server(messages::opcode::SMSG_PERIODICAURALOG, &body).is_err());

    // SMSG_SPELLDAMAGESHIELD: `victim` bears the shield; `attacker` takes the damage back.
    let body = hx(concat!(
        "1111000000000000", // victim raw guid 0x1111
        "2222000000000000", // attacker raw guid 0x2222
        "21000000",         // damage 33
        "04000000",         // school 4 (nature)
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELLDAMAGESHIELD, &body).unwrap();
    match &packet {
        ServerPacket::DamageShield(s) => {
            assert_eq!(
                s,
                &DamageShield {
                    victim: 0x1111,
                    attacker: 0x2222,
                    damage: 33,
                    school: 4,
                }
            );
        }
        other => panic!("damage shield, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::DamageShield(s) => assert_eq!((s.victim, s.attacker), (0x1111, 0x2222)),
        other => panic!("damage shield event, got {other:?}"),
    }

    // SMSG_ENVIRONMENTALDAMAGELOG: a raw victim guid (vmangos `ObjectGuid.cpp:174`); absorbed and
    // resist are the post-1.6.1 tail (`Server/Packets/Combat.cpp:58-67`).
    let body = hx(concat!(
        "0300000000000000", // victim raw guid 3
        "02",               // damageType 2 (DAMAGE_FALL)
        "17000000",         // damage 23
        "05000000",         // absorbed 5
        "ffffffff",         // resist -1
    ));
    let packet =
        messages::parse_server(messages::opcode::SMSG_ENVIRONMENTALDAMAGELOG, &body).unwrap();
    match &packet {
        ServerPacket::EnvironmentalDamageLog(e) => {
            assert_eq!(
                e,
                &EnvironmentalDamageLog {
                    victim: 3,
                    damage_type: 2,
                    damage: 23,
                    absorb: 5,
                    resist: -1,
                }
            );
        }
        other => panic!("environmental damage log, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::EnvironmentalDamageLog(e) => {
            assert_eq!((e.victim, e.damage_type, e.damage), (3, 2, 23));
        }
        other => panic!("environmental damage log event, got {other:?}"),
    }

    // SMSG_SPELLLOGMISS: count x {raw target guid, u8 missInfo}; useExtended 1 adds 2 x f32 per
    // entry. REFLECT's extra byte rides only SMSG_SPELL_GO's miss list, not this one.
    let body = hx(concat!(
        "85000000",         // spellId 133
        "4200000000000000", // caster raw guid 0x42
        "00",               // useExtended: 0
        "02000000",         // count 2
        "aa00000000000000", // target 0xAA
        "03",               // missInfo 3 (DODGE)
        "bb00000000000000", // target 0xBB
        "0b",               // missInfo 11 (REFLECT)
    ));
    let packet = messages::parse_server(messages::opcode::SMSG_SPELLLOGMISS, &body).unwrap();
    match &packet {
        ServerPacket::SpellLogMiss(s) => {
            assert_eq!(
                s,
                &SpellLogMiss {
                    spell_id: 133,
                    caster: 0x42,
                    misses: vec![(0xAA, 3), (0xBB, 11)],
                }
            );
        }
        other => panic!("spell log miss, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellLogMiss(s) => assert_eq!(s.misses, vec![(0xAA, 3), (0xBB, 11)]),
        other => panic!("spell log miss event, got {other:?}"),
    }

    let mut body = 0xCCu64.to_le_bytes().to_vec(); // victim
    body.extend_from_slice(&250u32.to_le_bytes()); // total
    body.push(0); // xpType: kill
    body.extend_from_slice(&200u32.to_le_bytes()); // base xp (kept: rested bonus = total − base)
    body.extend_from_slice(&1.25f32.to_le_bytes()); // group bonus (dropped)
    let packet = messages::parse_server(messages::opcode::SMSG_LOG_XPGAIN, &body).unwrap();
    match &packet {
        ServerPacket::XpGain(x) => {
            assert_eq!(
                x,
                &XpGain {
                    victim: 0xCC,
                    total: 250,
                    base: 200,
                    kill: true,
                }
            );
        }
        other => panic!("xp gain (kill), got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::XpGain(x) => assert_eq!((x.victim, x.total, x.kill), (0xCC, 250, true)),
        other => panic!("xp gain event, got {other:?}"),
    }

    let mut body = 0u64.to_le_bytes().to_vec(); // victim: 0 (non-kill xp has no victim)
    body.extend_from_slice(&50u32.to_le_bytes()); // total
    body.push(1); // xpType: non-kill, nothing trails
    let packet = messages::parse_server(messages::opcode::SMSG_LOG_XPGAIN, &body).unwrap();
    match &packet {
        ServerPacket::XpGain(x) => {
            assert_eq!(
                x,
                &XpGain {
                    victim: 0,
                    total: 50,
                    base: 50, // non-kill carries no base on the wire; base = total
                    kill: false,
                }
            );
        }
        other => panic!("xp gain (non-kill), got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::XpGain(x) => assert_eq!((x.victim, x.total, x.kill), (0, 50, false)),
        other => panic!("xp gain event, got {other:?}"),
    }

    // SMSG_EXPLORATION_EXPERIENCE: u32 AreaTable.dbc id (40 = Westfall) + u32 xp, sent even when
    // xp is 0 (vmangos `Misc.cpp:552-556`).
    let mut body = 40u32.to_le_bytes().to_vec();
    body.extend_from_slice(&85u32.to_le_bytes());
    assert_eq!(body.len(), 8);
    let packet =
        messages::parse_server(messages::opcode::SMSG_EXPLORATION_EXPERIENCE, &body).unwrap();
    match &packet {
        ServerPacket::ExplorationXp(x) => {
            assert_eq!(
                x,
                &ExplorationXp {
                    area_id: 40,
                    xp: 85,
                }
            );
        }
        other => panic!("exploration xp, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::ExplorationXp(x) => assert_eq!((x.area_id, x.xp), (40, 85)),
        other => panic!("exploration xp event, got {other:?}"),
    }

    // SMSG_LEVELUP_INFO: u32 level, then the i32 gains, healthGain, powerGains[5] (mana..happiness)
    // and statGains[5] (str..spirit), no guid (vmangos `Misc.cpp:524-532`).
    let mut body = Vec::new();
    for v in [7u32, 22, 15, 0, 0, 0, 0, 1, 0, 1, 2, 1] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(body.len(), 48);
    let packet = messages::parse_server(messages::opcode::SMSG_LEVELUP_INFO, &body).unwrap();
    match &packet {
        ServerPacket::LevelUp(l) => {
            assert_eq!(
                l,
                &LevelUpInfo {
                    level: 7,
                    health: 22,
                    powers: [15, 0, 0, 0, 0],
                    stats: [1, 0, 1, 2, 1],
                }
            );
        }
        other => panic!("level up, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::LevelUp(l) => assert_eq!((l.level, l.health, l.powers[0]), (7, 22, 15)),
        other => panic!("level up event, got {other:?}"),
    }

    // A loss the server wraps into its u32 reads negative, as the reference's `%d` fire prints it.
    let mut body = Vec::new();
    for v in [
        7u32, 0xFFFFFFFE, 0xFFFFFFFD, 0, 0, 0, 0, 0xFFFFFFFE, 0, 0xFFFFFFFB, 0xFFFFFFFA, 0xFFFFFFF9,
    ] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(body.len(), 48);
    let packet = messages::parse_server(messages::opcode::SMSG_LEVELUP_INFO, &body).unwrap();
    match &packet {
        ServerPacket::LevelUp(l) => {
            assert_eq!(
                l,
                &LevelUpInfo {
                    level: 7,
                    health: -2,
                    powers: [-3, 0, 0, 0, 0],
                    stats: [-2, 0, -5, -6, -7],
                }
            );
        }
        other => panic!("level up, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::LevelUp(l) => assert_eq!((l.level, l.health, l.powers[0]), (7, -2, -3)),
        other => panic!("level up event, got {other:?}"),
    }
}

/// Raw caster guid, u32 spellId, u32 count, raw guids (vmangos `Spell.cpp:4974-4997`); in the
/// reference (`0x6e9820`) the only writer of the beams' chain-target array (`unit+0xd44`).
#[test]
fn spell_chain_targets_wire() {
    use benilla_protocol::messages::SpellChainTargets;

    const CASTER: u64 = 0x0000_01f1_3045_2ac9;

    let body = hx(concat!(
        "c92a4530f1010000", // caster guid, raw u64 (not packed)
        "b1020000",         // spellId 689 (Drain Life)
        "01000000",         // count 1: a u32, not the u8 the GO lists use
        "aa00000000000000", // target guid 0xAA, raw
    ));
    let packet =
        messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).unwrap();
    match &packet {
        ServerPacket::SpellChainTargets(c) => assert_eq!(
            c,
            &SpellChainTargets {
                caster: CASTER,
                spell_id: 689,
                targets: vec![0xAA],
            }
        ),
        other => panic!("chain targets, got {}", other.name()),
    }
    match decode(packet).pop().unwrap() {
        SessionEvent::SpellChainTargets {
            caster,
            spell_id,
            targets,
        } => assert_eq!((caster, spell_id, targets), (CASTER, 689, vec![0xAA])),
        other => panic!("chain targets event, got {other:?}"),
    }

    let body = hx(concat!(
        "0100000000000000",
        "a5010000", // spellId 421 (Chain Lightning)
        "03000000",
        "aa00000000000000",
        "bb00000000000000",
        "cc00000000000000",
    ));
    match messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).unwrap()
    {
        ServerPacket::SpellChainTargets(c) => {
            assert_eq!(c.targets, vec![0xAA, 0xBB, 0xCC], "wire order is hop order");
        }
        other => panic!("chain targets, got {}", other.name()),
    }

    // An empty list is legal on the wire, though the server only sends the packet with hits.
    let body = hx(concat!("0100000000000000", "a5010000", "00000000"));
    match messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).unwrap()
    {
        ServerPacket::SpellChainTargets(c) => assert!(c.targets.is_empty()),
        other => panic!("chain targets, got {}", other.name()),
    }

    // Count 2 with one guid following is an error, not a short list.
    let body = hx(concat!(
        "0100000000000000",
        "a5010000",
        "02000000",
        "aa00000000000000",
    ));
    assert!(
        messages::parse_server(messages::opcode::SMSG_SPELL_UPDATE_CHAIN_TARGETS, &body).is_err()
    );
}

/// Eight combat-log opcodes, each parsed against the vmangos writer cited at it.
#[test]
fn kill_outcome_dispel_enchantment_and_execute_logs_parse() {
    use benilla_protocol::messages::{
        DispelFailed, EnchantmentLog, ExecuteLog, PartyKillLog, SpellDispelLog, SpellInstaKillLog,
        SpellLogExecute, SpellOutcomeLog,
    };

    // SMSG_PARTYKILLLOG: raw killer + raw victim guid (vmangos `Server/Packets/Combat.cpp:52-56`).
    let body = hx(concat!(
        "0700000000000000", // killer 7
        "2a00000000000000", // victim 42
    ));
    match messages::parse_server(messages::opcode::SMSG_PARTYKILLLOG, &body).unwrap() {
        ServerPacket::PartyKillLog(p) => assert_eq!(
            p,
            PartyKillLog {
                killer: 7,
                victim: 42
            }
        ),
        other => panic!("party kill log, got {}", other.name()),
    }

    // SMSG_SPELLINSTAKILLLOG: raw victim guid, u32 spellId (vmangos `SpellEffects.cpp:274-279`).
    let body = hx(concat!("0900000000000000", "0d270000"));
    match messages::parse_server(messages::opcode::SMSG_SPELLINSTAKILLLOG, &body).unwrap() {
        ServerPacket::SpellInstaKillLog(p) => assert_eq!(
            p,
            SpellInstaKillLog {
                victim: 9,
                spell_id: 9997
            }
        ),
        other => panic!("instakill log, got {}", other.name()),
    }

    // One body, two opcodes (vmangos `Server/Packets/Spell.cpp:88-102`).
    let body = hx(concat!(
        "0100000000000000", // caster 1
        "0200000000000000", // target 2
        "39300000",         // spellId 12345
        "01",               // logFormat 1 (the reference's "is periodic" flag)
    ));
    let expect = SpellOutcomeLog {
        caster: 1,
        target: 2,
        spell_id: 12345,
        log_format: 1,
    };
    match messages::parse_server(messages::opcode::SMSG_PROCRESIST, &body).unwrap() {
        ServerPacket::ProcResist(p) => assert_eq!(p, expect),
        other => panic!("proc resist, got {}", other.name()),
    }
    match messages::parse_server(messages::opcode::SMSG_SPELLORDAMAGE_IMMUNE, &body).unwrap() {
        ServerPacket::SpellOrDamageImmune(p) => assert_eq!(p, expect),
        other => panic!("spell-or-damage immune, got {}", other.name()),
    }

    // SMSG_SPELLDISPELLOG, per vmangos `SpellEffects.cpp:2524-2539` (its 1.12.1 branch).
    let body = hx(concat!(
        "0105",     // packed victim: mask 0x01, byte 0x05 -> 5
        "0206",     // packed caster: mask 0x02, byte 0x06 -> 0x600
        "02000000", // count 2
        "0a000000", // 10
        "14000000", // 20
    ));
    match messages::parse_server(messages::opcode::SMSG_SPELLDISPELLOG, &body).unwrap() {
        ServerPacket::SpellDispelLog(p) => assert_eq!(
            p,
            SpellDispelLog {
                victim: 5,
                caster: 0x600,
                spell_ids: vec![10, 20],
            }
        ),
        other => panic!("spell dispel log, got {}", other.name()),
    }

    // SMSG_DISPEL_FAILED: raw caster, raw victim, then spell ids to the end of the body, with no
    // count (vmangos `SpellEffects.cpp:2549-2555`).
    let body = hx(concat!(
        "0300000000000000",
        "0400000000000000",
        "63000000",
        "64000000",
    ));
    match messages::parse_server(messages::opcode::SMSG_DISPEL_FAILED, &body).unwrap() {
        ServerPacket::DispelFailed(p) => assert_eq!(
            p,
            DispelFailed {
                caster: 3,
                victim: 4,
                spell_ids: vec![99, 100],
            }
        ),
        other => panic!("dispel failed, got {}", other.name()),
    }
    let body = hx(concat!("0300000000000000", "0400000000000000"));
    match messages::parse_server(messages::opcode::SMSG_DISPEL_FAILED, &body).unwrap() {
        ServerPacket::DispelFailed(p) => assert!(p.spell_ids.is_empty()),
        other => panic!("dispel failed, got {}", other.name()),
    }

    // SMSG_ENCHANTMENTLOG, per vmangos `Server/Packets/Item.cpp:235-242`.
    let body = hx(concat!(
        "0000000000000000", // caster 0: the enchant faded
        "0b00000000000000", // owner 11
        "d2040000",         // itemEntry 1234
        "39050000",         // spellId 1337
        "00",               // showAffiliation false
    ));
    match messages::parse_server(messages::opcode::SMSG_ENCHANTMENTLOG, &body).unwrap() {
        ServerPacket::EnchantmentLog(p) => assert_eq!(
            p,
            EnchantmentLog {
                caster: 0,
                owner: 11,
                item_entry: 1234,
                spell_id: 1337,
                show_affiliation: false,
            }
        ),
        other => panic!("enchantment log, got {}", other.name()),
    }

    // SMSG_SPELLLOGEXECUTE (vmangos `Spell.cpp:4662-4778`): row shape varies by effect.
    let body = hx(concat!(
        "0107",     // packed caster 7
        "e8030000", // spellId 1000
        "03000000", // 3 groups
        // group 1: effect 8 POWER_DRAIN, one row
        "08000000",
        "01000000",
        "1400000000000000", // target 20
        "2c010000",         // amount 300
        "00000000",         // power 0 (mana)
        "0000803f",         // multiplier 1.0
        // group 2: effect 24 CREATE_ITEM, one row of just an item entry
        "18000000",
        "01000000",
        "b80b0000", // itemEntry 3000
        // group 3: effect 102 DISMISS_PET, one row of just a guid
        "66000000",
        "01000000",
        "1e00000000000000", // target 30
    ));
    match messages::parse_server(messages::opcode::SMSG_SPELLLOGEXECUTE, &body).unwrap() {
        ServerPacket::SpellLogExecute(p) => assert_eq!(
            p,
            SpellLogExecute {
                caster: 7,
                spell_id: 1000,
                effects: vec![
                    (
                        8,
                        vec![ExecuteLog::PowerDrain {
                            target: 20,
                            amount: 300,
                            power: 0,
                            multiplier: 1.0,
                        }]
                    ),
                    (24, vec![ExecuteLog::CreateItem { item_entry: 3000 }]),
                    (102, vec![ExecuteLog::Target { target: 30 }]),
                ],
            }
        ),
        other => panic!("spell log execute, got {}", other.name()),
    }

    // An effect vmangos has no case for has no known row width, so the body errors; vmangos never
    // sends one, since its switch returns before `SendMessageToSet`.
    let body = hx(concat!(
        "0107", "e8030000", "01000000", "77000000", "01000000"
    ));
    assert!(messages::parse_server(messages::opcode::SMSG_SPELLLOGEXECUTE, &body).is_err());
}

/// The flat and pct spell-modifier opcodes share one body; only the opcode sets `flat`.
#[test]
fn spell_modifier_wire() {
    // u8 mask_bit 35, u8 op 14 (SPELLMOD_COST), i32 value -30. Bit 35, Cure Poison 526's only
    // bit, sits past the low dword that a 32-bit mask would keep.
    let body = hx("230ee2ffffff");
    for (wire_op, is_flat) in [
        (messages::opcode::SMSG_SET_FLAT_SPELL_MODIFIER, true),
        (messages::opcode::SMSG_SET_PCT_SPELL_MODIFIER, false),
    ] {
        let packet = messages::parse_server(wire_op, &body).unwrap();
        assert_eq!(
            packet.name(),
            if is_flat {
                "SMSG_SET_FLAT_SPELL_MODIFIER"
            } else {
                "SMSG_SET_PCT_SPELL_MODIFIER"
            }
        );
        match &packet {
            ServerPacket::SpellModifier {
                flat,
                mask_bit,
                op,
                value,
            } => assert_eq!((*flat, *mask_bit, *op, *value), (is_flat, 35, 14, -30)),
            other => panic!("spell modifier, got {}", other.name()),
        }
        match decode(packet).pop().unwrap() {
            SessionEvent::SpellModifier {
                flat,
                mask_bit,
                op,
                value,
            } => assert_eq!((flat, mask_bit, op, value), (is_flat, 35, 14, -30)),
            _ => panic!("spell modifier event"),
        }
    }

    assert!(messages::parse_server(
        messages::opcode::SMSG_SET_FLAT_SPELL_MODIFIER,
        &hx("230ee2ffff")
    )
    .is_err());
}
