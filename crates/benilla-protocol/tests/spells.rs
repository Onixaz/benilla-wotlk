//! Wire tests for spells, action buttons, melee and the combat log (`src/messages/spells.rs`).

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;
use common::hx;

/// Every swing refusal has an empty body (vmangos `Server/Packets/Combat.cpp`). DEADTARGET (0x148)
/// and CANT_ATTACK (0x149) share the reference's arm, so they decode alike.
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
        "SMSG_CANCEL_COMBAT is registered, just not by the combat TU"
    );
    assert!(matches!(
        decode(ServerPacket::CancelCombat).pop().unwrap(),
        SessionEvent::CancelCombat
    ));

    // SMSG_FEIGN_DEATH_RESISTED: in the reference only a `DisplayError(421)`.
    assert!(matches!(
        messages::parse_server(messages::opcode::SMSG_FEIGN_DEATH_RESISTED, &[]).unwrap(),
        ServerPacket::FeignDeathResisted
    ));
    assert!(matches!(
        decode(ServerPacket::FeignDeathResisted).pop().unwrap(),
        SessionEvent::FeignDeathResisted
    ));

    // 0x147 SMSG_ATTACKSWING_NOTSTANDING stays unknown: the reference never registers it and
    // vmangos never sends it.
    assert!(
        matches!(
            messages::parse_server(0x0147, &[]).unwrap(),
            ServerPacket::Other { opcode: 0x0147 }
        ),
        "0x147 NOTSTANDING must stay unparsed — the reference never registers it"
    );
}

/// The XP, exploration and level-up bodies (vmangos `Server/Packets/Misc.cpp:512-532`).
#[test]
fn progression_wire_golden() {
    use benilla_protocol::messages::{ExplorationXp, LevelUpInfo, XpGain};

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
/// reference the only writer of the beams' chain-target array (`unit+0xd44`).
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
