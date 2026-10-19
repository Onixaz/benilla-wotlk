//! Combat-log bodies in the 12340 layout (`src/messages/combat_log.rs`), each read to its last byte.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{self, opcode, ExecuteLog, PeriodicTick, SpellRemoveKind};
use benilla_protocol::ServerPacket;

/// A creature guid; it packs with mask `0xc9`.
const MOB: u64 = 0xF130_0000_4500_002A;
const ME: u64 = 0x3EE;

/// A little-endian body builder.
#[derive(Default)]
struct Body(Vec<u8>);

impl Body {
    fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(self, v: i32) -> Self {
        self.u32(v as u32)
    }
    fn f32(mut self, v: f32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn pguid(mut self, v: u64) -> Self {
        benilla_protocol::wire::write_packed_guid(v, &mut self.0).unwrap();
        self
    }
}

/// Parse a body that must decode with nothing left over.
fn parse(op: u16, body: Body) -> ServerPacket {
    let (packet, tail) =
        messages::parse_server_with_tail(op, &body.0).unwrap_or_else(|e| panic!("{op:#x}: {e}"));
    assert_eq!(tail, 0, "{op:#x} left {tail} byte(s)");
    packet
}

/// `0x6d3840`: overkill follows damage, and the crit bit is hit-info `0x2`.
#[test]
fn spell_damage_log_reads_overkill_before_the_school_byte() {
    let body = Body::default()
        .pguid(MOB)
        .pguid(ME)
        .u32(133) // spell
        .u32(500) // damage
        .u32(7) // overkill
        .u8(4) // school mask: fire
        .u32(50) // absorbed
        .u32(20) // resisted
        .u8(0) // periodic
        .u8(0)
        .u32(10) // blocked
        .u32(0x2) // hit info: crit
        .u8(0); // no debug floats
    let ServerPacket::SpellDamageLog(s) = parse(opcode::SMSG_SPELLNONMELEEDAMAGELOG, body) else {
        panic!("spell damage log");
    };
    assert_eq!(
        s,
        messages::SpellDamageLog {
            target: MOB,
            caster: ME,
            spell_id: 133,
            damage: 500,
            overkill: 7,
            school: 4,
            absorbed: 50,
            resisted: 20,
            periodic: false,
            blocked: 10,
            hit_info: messages::spell_hit_info::CRIT,
        }
    );
    assert!(matches!(
        decode(ServerPacket::SpellDamageLog(s)).pop(),
        Some(SessionEvent::SpellDamageLog(_))
    ));
}

/// `0x6d3b3e`-`0x6d3bc8`: a set debug byte appends two floats per bit `0x1` and `0x4`, six for
/// `0x20`.
#[test]
fn spell_damage_log_skips_the_debug_floats() {
    let mut body = Body::default()
        .pguid(MOB)
        .pguid(ME)
        .u32(133)
        .u32(5)
        .u32(0)
        .u8(1)
        .u32(0)
        .u32(0)
        .u8(1) // periodic
        .u8(0)
        .u32(0)
        .u32(0x25)
        .u8(1);
    for _ in 0..10 {
        body = body.f32(0.5);
    }
    let ServerPacket::SpellDamageLog(s) = parse(opcode::SMSG_SPELLNONMELEEDAMAGELOG, body) else {
        panic!("spell damage log");
    };
    assert!(s.periodic);
    assert_eq!(s.hit_info, 0x25);
}

/// `0x7531f0`: a damage tick is five `u32`s and a crit byte, a heal tick three and a crit byte.
#[test]
fn periodic_aura_log_reads_every_arm() {
    let body = Body::default()
        .pguid(MOB)
        .pguid(ME)
        .u32(172)
        .u32(4)
        .u32(3) // periodic damage
        .u32(88)
        .u32(2) // overkill
        .u32(0x20) // shadow
        .u32(12)
        .u32(5)
        .u8(1) // crit
        .u32(20) // obs mod health
        .u32(45)
        .u32(3) // overheal
        .u32(4) // absorbed
        .u8(0)
        .u32(24) // periodic energize
        .u32(0)
        .u32(30)
        .u32(64) // leech
        .u32(0)
        .u32(40)
        .f32(0.5);
    let ServerPacket::PeriodicAuraLog(l) = parse(opcode::SMSG_PERIODICAURALOG, body) else {
        panic!("periodic aura log");
    };
    assert_eq!((l.target, l.caster, l.spell_id), (MOB, ME, 172));
    assert_eq!(
        l.ticks,
        vec![
            PeriodicTick::Damage {
                amount: 88,
                overkill: 2,
                school: 0x20,
                absorbed: 12,
                resisted: 5,
                crit: true,
            },
            PeriodicTick::Heal {
                amount: 45,
                overheal: 3,
                absorbed: 4,
                crit: false,
            },
            PeriodicTick::Energize {
                power: 0,
                amount: 30
            },
            PeriodicTick::Leech {
                power: 0,
                amount: 40,
                multiplier: 0.5
            },
        ]
    );
}

/// `0x75367a`: an aura type with no arm stops the read and drops the rest of the body.
#[test]
fn periodic_aura_log_stops_at_an_unknown_aura_type() {
    let body = Body::default()
        .pguid(ME)
        .pguid(ME)
        .u32(1)
        .u32(2)
        .u32(4) // no arm
        .u32(0xdead_beef)
        .u32(3);
    let ServerPacket::PeriodicAuraLog(l) = parse(opcode::SMSG_PERIODICAURALOG, body) else {
        panic!("periodic aura log");
    };
    assert!(l.ticks.is_empty());
}

/// `0x6d3c30`: amount, overheal, absorbed, crit, then a byte nobody reads.
#[test]
fn spell_heal_log_reads_overheal_and_absorb() {
    let body = Body::default()
        .pguid(ME)
        .pguid(ME)
        .u32(2050)
        .u32(120)
        .u32(30)
        .u32(5)
        .u8(1)
        .u8(0);
    let ServerPacket::SpellHealLog(h) = parse(opcode::SMSG_SPELLHEALLOG, body) else {
        panic!("heal log");
    };
    assert_eq!(
        h,
        messages::SpellHealLog {
            target: ME,
            caster: ME,
            spell_id: 2050,
            amount: 120,
            overheal: 30,
            absorbed: 5,
            crit: true,
        }
    );
}

/// `0x6d3df0`: unchanged, two packed guids, spell, power, amount.
#[test]
fn spell_energize_log_reads_power_then_amount() {
    let body = Body::default()
        .pguid(ME)
        .pguid(ME)
        .u32(5229)
        .u32(1)
        .u32(200);
    let ServerPacket::SpellEnergizeLog(e) = parse(opcode::SMSG_SPELLENERGIZELOG, body) else {
        panic!("energize log");
    };
    assert_eq!((e.power, e.amount), (1, 200));
}

/// `0x6d3750`: four `u32`s after the two raw guids.
#[test]
fn damage_shield_reads_spell_damage_overkill_school() {
    let body = Body::default()
        .u64(ME)
        .u64(MOB)
        .u32(467)
        .u32(33)
        .u32(0)
        .u32(8);
    let ServerPacket::DamageShield(d) = parse(opcode::SMSG_SPELLDAMAGESHIELD, body) else {
        panic!("damage shield");
    };
    assert_eq!(
        d,
        messages::DamageShield {
            victim: ME,
            attacker: MOB,
            spell_id: 467,
            damage: 33,
            overkill: 0,
            school: 8,
        }
    );
}

/// `0x6ce260`: the caster comes first.
#[test]
fn insta_kill_log_reads_caster_then_victim() {
    let body = Body::default().u64(MOB).u64(ME).u32(5);
    let ServerPacket::SpellInstaKillLog(k) = parse(opcode::SMSG_SPELLINSTAKILLLOG, body) else {
        panic!("insta kill");
    };
    assert_eq!((k.caster, k.victim, k.spell_id), (MOB, ME, 5));
}

/// `0x6ce2b0` for all three opcodes: the removing spell and a byte precede the count, and each row
/// carries a harmful byte.
#[test]
fn spell_remove_log_reads_the_removing_spell_and_harmful_rows() {
    for (op, kind) in [
        (opcode::SMSG_SPELLDISPELLOG, SpellRemoveKind::Dispel),
        (opcode::SMSG_SPELLSTEALLOG, SpellRemoveKind::Steal),
        (opcode::SMSG_SPELLBREAKLOG, SpellRemoveKind::Break),
    ] {
        let body = Body::default()
            .pguid(MOB)
            .pguid(ME)
            .u32(527)
            .u8(0)
            .u32(2)
            .u32(589)
            .u8(1)
            .u32(1243)
            .u8(0);
        let ServerPacket::SpellRemoveLog(l) = parse(op, body) else {
            panic!("{op:#x}");
        };
        assert_eq!(
            l,
            messages::SpellRemoveLog {
                kind,
                target: MOB,
                caster: ME,
                spell_id: 527,
                auras: vec![(589, true), (1243, false)],
            }
        );
        assert!(matches!(
            decode(ServerPacket::SpellRemoveLog(l)).pop(),
            Some(SessionEvent::SpellRemoveLog(_))
        ));
    }
}

/// `0x750ea0`: the dispelling spell, then the failed auras to the end of the body.
#[test]
fn dispel_failed_reads_the_dispelling_spell_first() {
    let body = Body::default().u64(ME).u64(MOB).u32(527).u32(589).u32(1243);
    let ServerPacket::DispelFailed(d) = parse(opcode::SMSG_DISPEL_FAILED, body) else {
        panic!("dispel failed");
    };
    assert_eq!(
        d,
        messages::DispelFailed {
            caster: ME,
            target: MOB,
            spell_id: 527,
            failed: vec![589, 1243],
        }
    );
}

/// `0x751050`: two packed guids, the item, the enchantment.
#[test]
fn enchantment_log_reads_packed_guids_item_and_enchant() {
    let body = Body::default().pguid(ME).pguid(ME).u32(2504).u32(1897);
    let ServerPacket::EnchantmentLog(e) = parse(opcode::SMSG_ENCHANTMENTLOG, body) else {
        panic!("enchantment log");
    };
    assert_eq!(
        e,
        messages::EnchantmentLog {
            target: ME,
            caster: ME,
            item_entry: 2504,
            enchant_id: 1897,
        }
    );
}

/// `0x6d3220`: packed row guids, one arm per effect group, and an armless effect reads nothing.
#[test]
fn spell_log_execute_reads_each_arm() {
    let body = Body::default()
        .pguid(ME)
        .u32(1)
        .u32(8)
        .u32(8) // power drain
        .u32(1)
        .pguid(MOB)
        .u32(50)
        .u32(0)
        .f32(1.0)
        .u32(19) // extra attacks
        .u32(1)
        .pguid(ME)
        .u32(2)
        .u32(24) // create item
        .u32(1)
        .u32(2589)
        .u32(68) // interrupt
        .u32(1)
        .pguid(MOB)
        .u32(133)
        .u32(111) // durability damage
        .u32(1)
        .pguid(ME)
        .i32(-1)
        .i32(-1)
        .u32(28) // summon
        .u32(1)
        .pguid(MOB)
        .u32(2) // school damage: no arm, reads no row bytes
        .u32(3)
        .u32(113) // resurrect
        .u32(1)
        .pguid(ME);
    let ServerPacket::SpellLogExecute(x) = parse(opcode::SMSG_SPELLLOGEXECUTE, body) else {
        panic!("execute log");
    };
    assert_eq!((x.caster, x.spell_id), (ME, 1));
    let rows: Vec<(u32, Vec<ExecuteLog>)> = x.effects;
    assert_eq!(
        rows,
        vec![
            (
                8,
                vec![ExecuteLog::PowerDrain {
                    target: MOB,
                    amount: 50,
                    power: 0,
                    multiplier: 1.0
                }]
            ),
            (
                19,
                vec![ExecuteLog::ExtraAttacks {
                    target: ME,
                    count: 2
                }]
            ),
            (24, vec![ExecuteLog::CreateItem { item_entry: 2589 }]),
            (
                68,
                vec![ExecuteLog::InterruptCast {
                    target: MOB,
                    spell_id: 133
                }]
            ),
            (
                111,
                vec![ExecuteLog::DurabilityDamage {
                    target: ME,
                    item_entry: -1,
                    slot: -1
                }]
            ),
            (28, vec![ExecuteLog::Summon { target: MOB }]),
            (2, vec![]),
            (113, vec![ExecuteLog::Resurrect { target: ME }]),
        ]
    );
}

/// `0x750d40` and `0x6ce3d0` share one body; the immune log's byte is its periodic flag.
#[test]
fn outcome_logs_share_one_body() {
    let body = || Body::default().u64(ME).u64(MOB).u32(133).u8(1);
    let ServerPacket::SpellOrDamageImmune(o) = parse(opcode::SMSG_SPELLORDAMAGE_IMMUNE, body())
    else {
        panic!("immune");
    };
    assert_eq!(
        o,
        messages::SpellOutcomeLog {
            caster: ME,
            target: MOB,
            spell_id: 133,
            periodic: true,
        }
    );
    assert!(matches!(
        parse(opcode::SMSG_PROCRESIST, body()),
        ServerPacket::ProcResist(_)
    ));
}

/// `0x6d3090`: raw guids and a miss byte per row; a debug log adds two floats per row.
#[test]
fn spell_log_miss_reads_rows_and_debug_floats() {
    let body = Body::default()
        .u32(26364)
        .u64(ME)
        .u8(1)
        .u32(1)
        .u64(MOB)
        .u8(7)
        .f32(1.0)
        .f32(2.0);
    let ServerPacket::SpellLogMiss(m) = parse(opcode::SMSG_SPELLLOGMISS, body) else {
        panic!("log miss");
    };
    assert_eq!(
        m,
        messages::SpellLogMiss {
            spell_id: 26364,
            caster: ME,
            debug: true,
            misses: vec![(MOB, 7)],
        }
    );
}

/// `0x756800`'s environmental arm: a raw guid, the type byte and three `u32`s.
#[test]
fn environmental_damage_log_reads_a_raw_guid() {
    let body = Body::default().u64(ME).u8(2).u32(23).u32(5).u32(1);
    let ServerPacket::EnvironmentalDamageLog(e) = parse(opcode::SMSG_ENVIRONMENTALDAMAGELOG, body)
    else {
        panic!("environmental");
    };
    assert_eq!(
        (e.victim, e.damage_type, e.damage, e.absorb, e.resist),
        (ME, 2, 23, 5, 1)
    );
}

/// `0x750c90`: killer and victim, raw.
#[test]
fn party_kill_log_reads_killer_then_victim() {
    let body = Body::default().u64(ME).u64(MOB);
    let ServerPacket::PartyKillLog(k) = parse(opcode::SMSG_PARTYKILLLOG, body) else {
        panic!("party kill");
    };
    assert_eq!((k.killer, k.victim), (ME, MOB));
}

/// `0x753730`: three packed guids, the signed damage and the spell.
#[test]
fn building_damage_reads_three_guids() {
    let building = 0xF110_0000_0100_0001;
    let body = Body::default()
        .pguid(building)
        .pguid(MOB)
        .pguid(ME)
        .i32(-250)
        .u32(56575);
    let ServerPacket::BuildingDamage(b) = parse(opcode::SMSG_DESTRUCTIBLE_BUILDING_DAMAGE, body)
    else {
        panic!("building damage");
    };
    assert_eq!(
        b,
        messages::BuildingDamage {
            building,
            caster: MOB,
            owner: ME,
            damage: -250,
            spell_id: 56575,
        }
    );
}
