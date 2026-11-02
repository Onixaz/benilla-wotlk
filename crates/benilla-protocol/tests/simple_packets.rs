//! Server packets with small fixed bodies, parsed and decoded against hand-built bytes.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::{MoveMode, ServerPacket};
use common::hx;

#[test]
fn simple_server_bodies_parse() {
    let mut challenge = hx("01000000efbeadde");
    challenge.extend_from_slice(&[0xAB; 32]);
    match messages::parse_server(messages::opcode::SMSG_AUTH_CHALLENGE, &challenge).unwrap() {
        ServerPacket::AuthChallenge { server_seed } => assert_eq!(server_seed, 0xDEAD_BEEF),
        _ => panic!("auth challenge"),
    }
    // SMSG_NEW_WORLD: u32 map, f32 x/y/z, f32 orientation.
    let nw = hx("010000000000803f0000004000004040000000bf");
    match messages::parse_server(messages::opcode::SMSG_NEW_WORLD, &nw).unwrap() {
        ServerPacket::NewWorld {
            map,
            position,
            orientation,
        } => {
            assert_eq!(map, 1);
            assert_eq!((position.x, position.y, position.z), (1.0, 2.0, 3.0));
            assert_eq!(orientation, -0.5);
        }
        _ => panic!("new world"),
    }
    // SMSG_LOGIN_SETTIMESPEED: packed date (LSB up: minute:6, hour:5, weekday:3, day:6, month:4,
    // year:5), f32 timescale, then a `u32` 0. The day serial counts 31-day months:
    // year·372 + month·31 + day.
    let datetime: u32 = (26 << 24) | (6 << 20) | (17 << 14) | (3 << 11) | (14 << 6) | 30;
    let mut ts = datetime.to_le_bytes().to_vec();
    ts.extend_from_slice(&0.0166_6667f32.to_le_bytes());
    ts.extend_from_slice(&0u32.to_le_bytes());
    match messages::parse_server(messages::opcode::SMSG_LOGIN_SETTIMESPEED, &ts).unwrap() {
        ServerPacket::TimeSpeed {
            hours,
            minutes,
            day_serial,
            timescale,
            packed,
            ..
        } => {
            assert_eq!((hours, minutes), (14, 30));
            assert_eq!(day_serial, 26 * 372 + 6 * 31 + 17);
            // The weekday, day, month and year ride the packed word whole.
            assert_eq!((packed >> 11) & 0x7, 3);
            assert_eq!((packed >> 14) & 0x3f, 17);
            assert_eq!(timescale, 0.0166_6667);
        }
        _ => panic!("settimespeed"),
    }
    // SMSG_CALENDAR_SEND_NUM_PENDING: one u32.
    match messages::parse_server(
        messages::opcode::SMSG_CALENDAR_SEND_NUM_PENDING,
        &2u32.to_le_bytes(),
    )
    .unwrap()
    {
        ServerPacket::Calendar(messages::CalendarPacket::NumPending(count)) => {
            assert_eq!(count, 2)
        }
        _ => panic!("calendar num pending"),
    }
}

/// The response is one u32 of unix-epoch seconds (vmangos `Handlers/QueryHandler.cpp:418-423`).
#[test]
fn query_time_asks_empty_and_decodes_the_server_wall_clock() {
    assert!(
        messages::query_time().is_empty(),
        "CMSG_QUERY_TIME is a NullClientPacket"
    );

    // 2026-08-13T12:00:00Z = 1_786_622_400 = 0x6a7db1c0: four distinct bytes, then the reset
    // 0x00012345 seconds away.
    assert_eq!(u32::from_le_bytes([0xc0, 0xb1, 0x7d, 0x6a]), 1_786_622_400);
    let p = messages::parse_server(
        messages::opcode::SMSG_QUERY_TIME_RESPONSE,
        &hx("c0b17d6a45230100"),
    )
    .unwrap();
    assert!(matches!(
        p,
        ServerPacket::QueryTimeResponse {
            unix_time: 1_786_622_400,
            reset_in: 0x12345,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::ServerUnixTime {
            unix_time: 1_786_622_400,
            reset_in: 0x12345,
        }]
    ));

    // 1.12's four-byte body is short on 3.3.5a.
    assert!(
        messages::parse_server(messages::opcode::SMSG_QUERY_TIME_RESPONSE, &hx("c0b17d6a"))
            .is_err()
    );
}

/// `SMSG_TRIGGER_CINEMATIC` is one u32 `CinematicSequences.dbc` id; `SMSG_CHAR_DELETE` one byte,
/// which the handshake consumes, so it decodes to no event.
#[test]
fn cinematic_and_char_delete_parse_and_decode() {
    // 41 = the dwarf intro.
    let body = 41u32.to_le_bytes();
    let p = messages::parse_server(messages::opcode::SMSG_TRIGGER_CINEMATIC, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::TriggerCinematic { cinematic_id: 41 }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::CinematicTriggered { cinematic_id: 41 }]
    ));

    // 0x39 = CHAR_DELETE_SUCCESS.
    let p = messages::parse_server(messages::opcode::SMSG_CHAR_DELETE, &[0x39]).unwrap();
    assert!(matches!(p, ServerPacket::CharDelete { result: 0x39 }));
    assert!(decode(p).is_empty());
}

#[test]
fn server_sound_trio_parse_and_decode() {
    let body = 8595u32.to_le_bytes();
    let p = messages::parse_server(messages::opcode::SMSG_PLAY_SOUND, &body).unwrap();
    assert!(matches!(p, ServerPacket::PlaySound { sound_id: 8595 }));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlaySound { sound_id: 8595 }]
    ));

    let body = 2523u32.to_le_bytes();
    let p = messages::parse_server(messages::opcode::SMSG_PLAY_MUSIC, &body).unwrap();
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlayMusic { music_id: 2523 }]
    ));

    // 3355: the fishing-bobber splash (vmangos `GameObject.cpp:373`).
    let mut body = 3355u32.to_le_bytes().to_vec();
    body.extend_from_slice(&0xF110_0000_0000_002Au64.to_le_bytes());
    let p = messages::parse_server(messages::opcode::SMSG_PLAY_OBJECT_SOUND, &body).unwrap();
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PlayObjectSound {
            sound_id: 3355,
            guid: 0xF110_0000_0000_002A,
        }]
    ));
}

/// 12340's body is `u32 weatherId, f32 grade, u8 instant` (`0x526a77`), nine bytes, no sound.
#[test]
fn weather_reads_the_3_3_5a_body_whole() {
    let mut body = 4u32.to_le_bytes().to_vec();
    body.extend_from_slice(&0.5f32.to_le_bytes());
    body.push(1);
    let (p, tail) =
        messages::parse_server_with_tail(messages::opcode::SMSG_WEATHER, &body).unwrap();
    assert_eq!(tail, 0, "the decoder reads the whole body");
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::Weather {
            weather_id: 4,
            grade: 0.5,
            instant: true,
        }]
    ));
}

/// u8 itemClass + u32 subclass bitmask, per vmangos `Skill.cpp` `AppendBodyTo`.
#[test]
fn set_proficiency_parses_and_decodes() {
    assert_eq!(messages::opcode::SMSG_SET_PROFICIENCY, 295);
    // Weapons (class 2), mask 0x2408F: what a fresh warrior gets.
    let body = hx("028f400200");
    let p = messages::parse_server(messages::opcode::SMSG_SET_PROFICIENCY, &body).unwrap();
    match p {
        ServerPacket::SetProficiency {
            item_class,
            subclass_mask,
        } => {
            assert_eq!(item_class, 2);
            assert_eq!(subclass_mask, 0x0002_408f);
        }
        _ => panic!("set proficiency"),
    }
    let p = messages::parse_server(messages::opcode::SMSG_SET_PROFICIENCY, &body).unwrap();
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::Proficiency {
            item_class: 2,
            subclass_mask: 0x0002_408f
        }]
    ));
}

/// f32 bonus, u8 show effect, u32 count, then (u32 repListId, i32 standing) rows (`0x5d20a0`).
#[test]
fn set_faction_standing_parses_and_decodes() {
    assert_eq!(messages::opcode::SMSG_SET_FACTION_STANDING, 292);
    // Two slots: list 46 -> 3000, list 89 -> -6000 (standing is signed).
    let body = hx("0000000001020000002e000000b80b00005900000090e8ffff");
    let p = messages::parse_server(messages::opcode::SMSG_SET_FACTION_STANDING, &body).unwrap();
    match p {
        ServerPacket::SetFactionStanding { ref standings } => {
            assert_eq!(standings[..], [(46, 3000), (89, -6000)]);
        }
        _ => panic!("set faction standing"),
    }
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::ReputationDelta { ref standings }]
            if standings[..] == [(46, 3000), (89, -6000)]
    ));
}

/// An opcode with no parse arm decodes to `PacketDropped`, not silence, so the app can tally it.
#[test]
fn unknown_opcode_decodes_to_packet_dropped() {
    // Sentinel: CMSG_BOOTME (0x0001) has no parse arm; swap in another if it gains one.
    let packet = messages::parse_server(0x0001, &hx("0102030405")).unwrap();
    assert!(matches!(packet, ServerPacket::Other { opcode: 0x0001 }));
    match decode(packet).as_slice() {
        [SessionEvent::PacketDropped {
            opcode: 0x0001,
            unparseable: false,
        }] => {}
        other => panic!("expected one PacketDropped event, got {other:?}"),
    }
    assert_eq!(
        messages::opcode_name(0x021E),
        Some("SMSG_QUEST_FORCE_REMOVE")
    );
    // Past 1.12.1's last opcode 0x033B: 3.3.5a-only.
    assert_eq!(
        messages::opcode_name(0x0427),
        Some("SMSG_ENABLE_BARBER_SHOP")
    );
    assert_eq!(messages::opcode_name(0x051E), Some("SMSG_MULTIPLE_MOVES"));
    assert_eq!(
        messages::opcode_name(messages::opcode::SMSG_UPDATE_OBJECT),
        Some("SMSG_UPDATE_OBJECT")
    );
    // 12340 dispatches ids below 0x51F only.
    assert_eq!(messages::opcode_name(0x051F), None);
    assert_eq!(messages::opcode_name(0xFFFF), None);
}

#[test]
fn death_arc_family_parses_and_decodes() {
    use benilla_protocol::messages::opcode;

    // Found: a dungeon corpse reports its entrance's map and coords, so the two maps differ.
    let mut body = vec![1u8];
    body.extend_from_slice(&0i32.to_le_bytes());
    body.extend_from_slice(&(-11209.6f32).to_le_bytes());
    body.extend_from_slice(&1666.54f32.to_le_bytes());
    body.extend_from_slice(&25.0f32.to_le_bytes());
    body.extend_from_slice(&36u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // no transport
    let pkt = messages::parse_server(opcode::MSG_CORPSE_QUERY, &body).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseQuery {
            found,
            display_map,
            position,
            corpse_map,
            transport: 0,
        }] => {
            assert!(found);
            assert_eq!(*display_map, 0);
            assert_eq!(*corpse_map, 36);
            assert_eq!(position[2], 25.0);
        }
        other => panic!("corpse query decode: {other:?}"),
    }
    // Not found, also sent unprompted when the corpse turns to bones: a lone u8 0.
    let pkt = messages::parse_server(opcode::MSG_CORPSE_QUERY, &[0u8]).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseQuery { found: false, .. }] => {}
        other => panic!("corpse query not-found decode: {other:?}"),
    }

    let mut body = 0i32.to_le_bytes().to_vec();
    for f in [-9339.1f32, 171.4, 61.6] {
        body.extend_from_slice(&f.to_le_bytes());
    }
    let pkt = messages::parse_server(opcode::SMSG_DEATH_RELEASE_LOC, &body).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::DeathReleaseLoc { map: 0, position }] => assert_eq!(position[1], 171.4),
        other => panic!("death release loc decode: {other:?}"),
    }
    let pkt = messages::parse_server(opcode::SMSG_PRE_RESURRECT, &hx("012a")).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::PreResurrect { guid: 0x2A }] => {}
        other => panic!("pre-resurrect decode: {other:?}"),
    }
    let mut body = Vec::new();
    for f in [1.0f32, 2.0, 3.0, 0.5] {
        body.extend_from_slice(&f.to_le_bytes());
    }
    let pkt =
        messages::parse_server(opcode::SMSG_CORPSE_MAP_POSITION_QUERY_RESPONSE, &body).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseMapPosition { position, facing }] => {
            assert_eq!((*position, *facing), ([1.0, 2.0, 3.0], 0.5));
        }
        other => panic!("corpse map position decode: {other:?}"),
    }
    let pkt = messages::parse_server(opcode::SMSG_CORPSE_NOT_IN_INSTANCE, &[]).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseNotInInstance] => {}
        other => panic!("corpse not in instance decode: {other:?}"),
    }

    let pkt = messages::parse_server(opcode::SMSG_CORPSE_RECLAIM_DELAY, &30_000u32.to_le_bytes())
        .unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::CorpseReclaimDelay { delay_ms: 30_000 }] => {}
        other => panic!("reclaim delay decode: {other:?}"),
    }

    // SMSG_RESURRECT_REQUEST, player caster: guid, u32 len 1, empty cstring, sickness 0, timer 1.
    let mut body = 0x2Au64.to_le_bytes().to_vec();
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&[0, 0, 1]);
    let pkt = messages::parse_server(opcode::SMSG_RESURRECT_REQUEST, &body).unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::ResurrectRequest {
            caster: 0x2A,
            name,
            sickness: false,
            has_timer: true,
        }] => assert!(name.is_empty(), "player caster ⇒ empty wire name"),
        other => panic!("resurrect request decode: {other:?}"),
    }

    let pkt = messages::parse_server(
        opcode::SMSG_SPIRIT_HEALER_CONFIRM,
        &0xF130_0000_0000_2AB3u64.to_le_bytes(),
    )
    .unwrap();
    match decode(pkt).as_slice() {
        [SessionEvent::SpiritHealerConfirm {
            npc: 0xF130_0000_0000_2AB3,
        }] => {}
        other => panic!("spirit healer confirm decode: {other:?}"),
    }

    let body = hx("012a07000000"); // packed(0x2A) + counter 7
    for (op, mode, apply) in [
        (opcode::SMSG_FORCE_MOVE_ROOT, MoveMode::Root, true),
        (opcode::SMSG_FORCE_MOVE_UNROOT, MoveMode::Root, false),
        (opcode::SMSG_MOVE_WATER_WALK, MoveMode::WaterWalk, true),
        (opcode::SMSG_MOVE_LAND_WALK, MoveMode::WaterWalk, false),
        (opcode::SMSG_MOVE_FEATHER_FALL, MoveMode::FeatherFall, true),
        (opcode::SMSG_MOVE_NORMAL_FALL, MoveMode::FeatherFall, false),
        (opcode::SMSG_MOVE_SET_HOVER, MoveMode::Hover, true),
        (opcode::SMSG_MOVE_UNSET_HOVER, MoveMode::Hover, false),
        (opcode::SMSG_MOVE_SET_CAN_FLY, MoveMode::CanFly, true),
        (opcode::SMSG_MOVE_UNSET_CAN_FLY, MoveMode::CanFly, false),
    ] {
        let pkt = messages::parse_server(op, &body).unwrap();
        match decode(pkt).as_slice() {
            [SessionEvent::MoveMode {
                guid: 0x2A,
                counter: 7,
                mode: got_mode,
                apply: got_apply,
            }] if *got_mode == mode && *got_apply == apply => {}
            other => panic!("move mode decode for opcode {op:#06x}: {other:?}"),
        }
    }

    // The movement-flag bits (vmangos `Objects/MovementInfo.h:28-62`).
    assert_eq!(MoveMode::Root.flag(), 0x0000_1000);
    assert_eq!(MoveMode::WaterWalk.flag(), 0x1000_0000);
    assert_eq!(MoveMode::FeatherFall.flag(), 0x2000_0000);
    assert_eq!(MoveMode::Hover.flag(), 0x4000_0000);

    // Root alone acks per direction, with no apply dword (vmangos `HandleMoveRootAck`).
    assert_eq!(
        MoveMode::Root.ack_opcode(true),
        opcode::CMSG_FORCE_MOVE_ROOT_ACK
    );
    assert_eq!(
        MoveMode::Root.ack_opcode(false),
        opcode::CMSG_FORCE_MOVE_UNROOT_ACK
    );
    assert!(!MoveMode::Root.ack_carries_apply());
    for mode in [
        MoveMode::WaterWalk,
        MoveMode::FeatherFall,
        MoveMode::Hover,
        MoveMode::CanFly,
    ] {
        assert_eq!(
            mode.ack_opcode(true),
            mode.ack_opcode(false),
            "{mode:?}: one ack opcode both ways"
        );
        assert!(mode.ack_carries_apply(), "{mode:?}: ack carries apply");
    }
}

/// Root's ack lacks the trailing `f32` apply the other modes carry (`0x7151f0`).
#[test]
fn move_flag_ack_bodies_differ_by_the_apply_tail() {
    use benilla_protocol::messages::{move_flag_ack, MovementInfo};
    use benilla_protocol::wire::Vector3d;

    let info = MovementInfo {
        flags: 0,
        flags2: 0,
        timestamp: 0x1122_3344,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        transport: None,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
    };
    let root = move_flag_ack(0x2A, 7, &info, None);
    let walk = move_flag_ack(0x2A, 7, &info, Some(true));
    assert_eq!(root.len() + 4, walk.len());
    assert_eq!(&walk[..root.len()], root.as_slice());
    assert_eq!(&walk[root.len()..], 1.0f32.to_le_bytes());
    let revoke = move_flag_ack(0x2A, 7, &info, Some(false));
    assert_eq!(&revoke[root.len()..], 0.0f32.to_le_bytes());
    // The ack's guid is packed: mask 0x01, one byte.
    assert_eq!(&root[..2], &[0x01, 0x2A]);
    assert_eq!(&root[2..6], &7u32.to_le_bytes());
}

/// `SMSG_DURABILITY_DAMAGE_DEATH` (0x2BD): empty body; the cue for the 10% durability-loss line.
#[test]
fn durability_damage_death_is_an_empty_body_cue() {
    let pkt = messages::parse_server(messages::opcode::SMSG_DURABILITY_DAMAGE_DEATH, &[]).unwrap();
    assert!(matches!(pkt, ServerPacket::DurabilityDamageDeath));
    assert!(matches!(
        decode(pkt).as_slice(),
        [SessionEvent::DurabilityDamageDeath]
    ));
}

/// u32 mapId, then u32 transportEntry + u32 oldMapId when the player rides a transport through
/// (vmangos `Misc.cpp:493-501`).
#[test]
fn transfer_pending_both_shapes_and_abort_parse_and_decode() {
    let plain =
        messages::parse_server(messages::opcode::SMSG_TRANSFER_PENDING, &hx("01000000")).unwrap();
    assert!(matches!(
        plain,
        ServerPacket::TransferPending {
            map: 1,
            transport: None
        }
    ));
    assert!(matches!(
        decode(plain).as_slice(),
        [SessionEvent::TransferPending {
            map_id: 1,
            transport_entry: None
        }]
    ));
    // Riding: 176310 is the Menethil boat, arriving in EK (0) from Kalimdor (1).
    let mut riding = 0u32.to_le_bytes().to_vec();
    riding.extend_from_slice(&176310u32.to_le_bytes());
    riding.extend_from_slice(&1u32.to_le_bytes());
    let riding = messages::parse_server(messages::opcode::SMSG_TRANSFER_PENDING, &riding).unwrap();
    assert!(matches!(
        riding,
        ServerPacket::TransferPending {
            map: 0,
            transport: Some((176310, 1))
        }
    ));
    assert!(matches!(
        decode(riding).as_slice(),
        [SessionEvent::TransferPending {
            map_id: 0,
            transport_entry: Some(176310)
        }]
    ));
    // Map 33, reason 2 (instance full): no third byte.
    let abort =
        messages::parse_server(messages::opcode::SMSG_TRANSFER_ABORTED, &[33, 0, 0, 0, 2]).unwrap();
    assert!(matches!(
        abort,
        ServerPacket::TransferAborted {
            map: 33,
            reason: 2,
            arg: None
        }
    ));
    assert!(matches!(
        decode(abort).as_slice(),
        [SessionEvent::TransferAborted {
            map: 33,
            reason: 2,
            arg: None
        }]
    ));
    // Reason 8 (difficulty) carries the difficulty.
    let heroic = messages::parse_server(
        messages::opcode::SMSG_TRANSFER_ABORTED,
        &[0x44, 2, 0, 0, 8, 1],
    )
    .unwrap();
    assert!(matches!(
        heroic,
        ServerPacket::TransferAborted {
            map: 580,
            reason: 8,
            arg: Some(1)
        }
    ));
}

/// Packed mover guid then u8 allowMove (vmangos `Server/Packets/Misc.cpp:677-682`).
#[test]
fn client_control_update_reads_a_packed_mover_and_the_allow_byte() {
    // Grant: creature 0xF130000C1A00A2B4, allowMove 1. Mask 0xdb marks its non-zero LE bytes
    // (b4 a2 00 1a 0c 00 30 f1 → indices 0,1,3,4,6,7), which follow in order.
    let grant = hx("dbb4a21a0c30f101");
    match messages::parse_server(messages::opcode::SMSG_CLIENT_CONTROL_UPDATE, &grant).unwrap() {
        ServerPacket::ClientControlUpdate { mover, allow_move } => {
            assert_eq!(mover, 0xF130_000C_1A00_A2B4);
            assert!(allow_move);
        }
        other => panic!("client control update, got {}", other.name()),
    }
    // Revoke: our own guid 0x45, allowMove 0; vmangos never roots a mind-controlled player.
    let revoke = hx("014500");
    let revoke = messages::parse_server(messages::opcode::SMSG_CLIENT_CONTROL_UPDATE, &revoke)
        .expect("revoke parses");
    assert!(matches!(
        revoke,
        ServerPacket::ClientControlUpdate {
            mover: 0x45,
            allow_move: false
        }
    ));
    assert!(matches!(
        decode(revoke).as_slice(),
        [SessionEvent::ClientControl {
            mover: 0x45,
            allow_move: false
        }]
    ));
    // A zero guid packs to a lone zero mask byte, so the very next byte is allowMove.
    let empty = messages::parse_server(messages::opcode::SMSG_CLIENT_CONTROL_UPDATE, &hx("0001"))
        .expect("empty guid parses");
    assert!(matches!(
        empty,
        ServerPacket::ClientControlUpdate {
            mover: 0,
            allow_move: true
        }
    ));
}

#[test]
fn pet_feedback_family_parses_and_decodes() {
    use benilla_protocol::messages::opcode;

    // 9 = PETTAME_TOOHIGHLEVEL.
    let p = messages::parse_server(opcode::SMSG_PET_TAME_FAILURE, &hx("09")).unwrap();
    assert!(matches!(p, ServerPacket::PetTameFailure { reason: 9 }));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PetTameFailure { reason: 9 }]
    ));
    // Reason to GlobalStrings key: the reference switches over 1..=12 (`0x800390`).
    assert_eq!(messages::pet_tame_failure_key(1), "PETTAME_INVALIDCREATURE");
    assert_eq!(
        messages::pet_tame_failure_key(5),
        "PETTAME_ANOTHERSUMMONACTIVE"
    );
    assert_eq!(messages::pet_tame_failure_key(11), "PETTAME_NOTDEAD");
    // Out of range on either side falls to the default arm.
    assert_eq!(messages::pet_tame_failure_key(0), "PETTAME_UNKNOWNERROR");
    assert_eq!(
        messages::pet_tame_failure_key(12),
        "PETTAME_CANTCONTROLEXOTIC"
    );
    assert_eq!(messages::pet_tame_failure_key(13), "PETTAME_UNKNOWNERROR");
    assert_eq!(messages::pet_tame_failure_key(200), "PETTAME_UNKNOWNERROR");

    // SMSG_PET_NAME_INVALID: the name code, the name ("Bruce") and the declension flag, then with
    // the flag set its five declined forms (here empty).
    for (body, want) in [
        ("0c000000".to_owned() + "427275636500" + "00", 12),
        ("10000000".to_owned() + "00" + "01" + "0000000000", 16),
    ] {
        let p = messages::parse_server(opcode::SMSG_PET_NAME_INVALID, &hx(&body)).unwrap();
        assert!(
            matches!(p, ServerPacket::PetNameInvalid { code } if code == want),
            "body {body:?}"
        );
        assert!(matches!(
            decode(p)[..],
            [SessionEvent::PetNameInvalid { code }] if code == want
        ));
    }

    // The reference reads no body here (vmangos writes none), so stray bytes change nothing.
    for body in ["", "deadbeef"] {
        let p = messages::parse_server(opcode::SMSG_PET_BROKEN, &hx(body)).unwrap();
        assert!(matches!(p, ServerPacket::PetBroken), "body {body:?}");
        assert!(matches!(decode(p)[..], [SessionEvent::PetBroken]));
    }

    // SMSG_PET_ACTION_SOUND: u64 guid then the u32 talk selector.
    let body = hx("2a0000000000401001000000");
    let p = messages::parse_server(opcode::SMSG_PET_ACTION_SOUND, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::PetActionSound {
            pet_guid: 0x1040_0000_0000_002A,
            talk: messages::PET_TALK_ATTACK,
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PetActionSound {
            pet_guid: 0x1040_0000_0000_002A,
            talk: 1
        }]
    ));

    // SMSG_PET_DISMISS_SOUND: u32 CreatureModelData id then raw WoW x/y/z (the human start).
    // The reference handler's +1.0 on z belongs to playback, not to the decode.
    let mut body = hx("d6020000"); // model 726
    for f in [-8949.95_f32, -132.493, 83.5312] {
        body.extend_from_slice(&f.to_le_bytes());
    }
    let p = messages::parse_server(opcode::SMSG_PET_DISMISS_SOUND, &body).unwrap();
    assert!(matches!(
        p,
        ServerPacket::PetDismissSound {
            model_id: 726,
            position: benilla_protocol::wire::Vector3d {
                x: -8949.95,
                y: -132.493,
                z: 83.5312,
            },
        }
    ));
    assert!(matches!(
        decode(p)[..],
        [SessionEvent::PetDismissSound {
            model_id: 726,
            position: [-8949.95, -132.493, 83.5312],
        }]
    ));
}

#[test]
fn time_sync_request_decodes_and_its_answer_is_counter_then_clock() {
    let req = messages::parse_server(messages::opcode::SMSG_TIME_SYNC_REQ, &hx("07000000"))
        .expect("time sync request parses");
    assert!(matches!(req, ServerPacket::TimeSyncReq { counter: 7 }));
    assert!(matches!(
        decode(req).as_slice(),
        [SessionEvent::TimeSyncReq { counter: 7 }]
    ));
    assert_eq!(
        messages::time_sync_resp(7, 0x0102_0304),
        hx("0700000004030201")
    );
}

/// 3.3.5a's gravity and collision-height grants on our mover, their acks, and the observer
/// flight and gravity modes (`0x732450`, `0x741c30`).
#[test]
fn gravity_collision_height_and_spline_flight_parse_and_ack() {
    use benilla_protocol::messages::{collision_height_ack, opcode, MovementInfo, SplineMode};
    use benilla_protocol::wire::Vector3d;

    for (op, apply, ack) in [
        (
            opcode::SMSG_MOVE_GRAVITY_DISABLE,
            true,
            opcode::CMSG_MOVE_GRAVITY_DISABLE_ACK,
        ),
        (
            opcode::SMSG_MOVE_GRAVITY_ENABLE,
            false,
            opcode::CMSG_MOVE_GRAVITY_ENABLE_ACK,
        ),
    ] {
        let p = messages::parse_server(op, &hx("012a07000000")).expect("gravity parses");
        assert!(matches!(
            p,
            ServerPacket::MoveMode {
                guid: 0x2A,
                counter: 7,
                mode: MoveMode::DisableGravity,
                apply: a,
            } if a == apply
        ));
        assert_eq!(MoveMode::DisableGravity.ack_opcode(apply), ack);
    }
    assert!(!MoveMode::DisableGravity.ack_carries_apply(), "no value");

    let mut body = hx("012a07000000");
    body.extend_from_slice(&1.5f32.to_le_bytes());
    let p = messages::parse_server(opcode::SMSG_MOVE_SET_COLLISION_HGT, &body).unwrap();
    assert!(matches!(
        decode(p).as_slice(),
        [SessionEvent::MoveSetCollisionHeight {
            guid: 0x2A,
            counter: 7,
            height
        }] if *height == 1.5
    ));
    let info = MovementInfo {
        flags: 0,
        flags2: 0,
        timestamp: 1,
        position: Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        orientation: 0.0,
        transport: None,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
    };
    let bare = messages::move_flag_ack(0x2A, 7, &info, None);
    let ack = collision_height_ack(0x2A, 7, &info, 1.5);
    assert_eq!(&ack[..bare.len()], bare.as_slice());
    assert_eq!(
        &ack[bare.len()..],
        1.5f32.to_le_bytes(),
        "the height is the value"
    );

    for (op, mode, apply) in [
        (
            opcode::SMSG_SPLINE_MOVE_SET_FLYING,
            SplineMode::Flying,
            true,
        ),
        (
            opcode::SMSG_SPLINE_MOVE_UNSET_FLYING,
            SplineMode::Flying,
            false,
        ),
        (
            opcode::SMSG_SPLINE_MOVE_GRAVITY_DISABLE,
            SplineMode::DisableGravity,
            true,
        ),
        (
            opcode::SMSG_SPLINE_MOVE_GRAVITY_ENABLE,
            SplineMode::DisableGravity,
            false,
        ),
    ] {
        let p = messages::parse_server(op, &hx("012a")).expect("spline mode parses");
        assert!(
            matches!(p, ServerPacket::SplineMoveMode { guid: 0x2A, mode: m, apply: a } if m == mode && a == apply),
            "{op:#x}"
        );
    }
    // The relay: a packed guid and a `MovementInfo` with `DISABLE_GRAVITY` (0x400), which maps
    // onto the same 1.12.1 bit. Flags, flags2, time, x/y/z, facing, fall time.
    let mut relay = hx("012a");
    relay.extend_from_slice(&0x400u32.to_le_bytes());
    relay.extend_from_slice(&0u16.to_le_bytes());
    relay.extend_from_slice(&[0u8; 4 + 12 + 4 + 4]);
    let p = messages::parse_server(opcode::MSG_MOVE_GRAVITY_CHNG, &relay).expect("relay");
    assert!(matches!(
        p,
        ServerPacket::PlayerMove { guid: 0x2A, flags, .. }
            if flags & messages::CLASSIC_DISABLE_GRAVITY != 0
    ));
}
