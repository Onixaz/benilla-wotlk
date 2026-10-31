//! The instance messages in the 12340 client's read order.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages::{
    self, opcode, EncounterUnitUpdate, InstanceLockWarning, InstanceResetFailure,
    RaidInstanceMessage, RaidInstanceWarning,
};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// `CMSG_RESET_INSTANCES` is empty in the reference and in vmangos's handler.
#[test]
fn reset_instances_body_is_empty() {
    assert_eq!(messages::reset_instances(), Vec::<u8>::new());
}

/// `SMSG_RAID_INSTANCE_MESSAGE` (`0x50ca80`): `u32` type, map, difficulty and seconds; the welcome
/// alone reads two more bytes, locked and extended.
#[test]
fn raid_instance_message_wire() {
    // type 4 (WELCOME), map 631 (Icecrown Citadel), difficulty 1, 266400 s, locked, not extended.
    let body = hx(concat!(
        "04000000", "77020000", "01000000", "a0100400", "01", "00"
    ));
    let p = messages::parse_server(opcode::SMSG_RAID_INSTANCE_MESSAGE, &body).unwrap();
    match &p {
        ServerPacket::RaidInstanceMessage { message } => {
            assert_eq!(
                *message,
                RaidInstanceMessage {
                    message_type: 4,
                    map: 631,
                    difficulty: 1,
                    seconds: 266_400,
                    welcome: Some((true, false)),
                }
            );
        }
        other => panic!("expected RaidInstanceMessage, got {}", other.name()),
    }
    match decode(p).as_slice() {
        [SessionEvent::RaidInstanceMessage { message }] => assert_eq!(message.map, 631),
        other => panic!("raid instance message decode: {other:?}"),
    }

    // Any other type stops after the seconds.
    let body = hx(concat!("01000000", "77020000", "00000000", "100e0000"));
    let p = messages::parse_server(opcode::SMSG_RAID_INSTANCE_MESSAGE, &body).unwrap();
    match p {
        ServerPacket::RaidInstanceMessage { message } => {
            assert_eq!((message.seconds, message.welcome), (3_600, None));
        }
        other => panic!("expected RaidInstanceMessage, got {}", other.name()),
    }

    // A welcome without its two bytes is a parse failure, not a zero-filled message.
    let short = hx(concat!("04000000", "77020000", "01000000", "a0100400"));
    assert!(messages::parse_server(opcode::SMSG_RAID_INSTANCE_MESSAGE, &short).is_err());
}

/// The five types of the jump table at `0x50cae4`; the welcome is an event, not a line.
#[test]
fn raid_instance_warning_types() {
    use RaidInstanceWarning as W;
    assert_eq!(W::from_wire(1), Some(W::Hours));
    assert_eq!(W::from_wire(2), Some(W::Minutes));
    assert_eq!(W::from_wire(3), Some(W::MinutesSoon));
    assert_eq!(W::from_wire(4), Some(W::Welcome));
    assert_eq!(W::from_wire(5), Some(W::Expired));
    assert_eq!(W::from_wire(0), None);
    assert_eq!(W::from_wire(6), None);

    assert_eq!(W::Hours.token(), Some("RAID_INSTANCE_WARNING_HOURS"));
    assert_eq!(W::Minutes.token(), Some("RAID_INSTANCE_WARNING_MIN"));
    assert_eq!(
        W::MinutesSoon.token(),
        Some("RAID_INSTANCE_WARNING_MIN_SOON")
    );
    assert_eq!(W::Welcome.token(), None);
    assert_eq!(W::Expired.token(), Some("RAID_INSTANCE_EXPIRED"));
}

/// `SMSG_UPDATE_INSTANCE_ENCOUNTER_UNIT` (`0x5eddd0`): each type's body.
#[test]
fn encounter_unit_wire() {
    use EncounterUnitUpdate as U;
    let read = |hex: &str| match messages::parse_server(
        opcode::SMSG_UPDATE_INSTANCE_ENCOUNTER_UNIT,
        &hx(hex),
    )
    .unwrap()
    {
        ServerPacket::EncounterUnit { update } => update,
        other => panic!("expected EncounterUnit, got {}", other.name()),
    };
    // Packed guid 0xF130_0000_0000_0042: mask 0xc1, then bytes 0x42, 0x30, 0xf1.
    const BOSS: u64 = 0xF130_0000_0000_0042;
    assert_eq!(
        read("00000000c14230f102"),
        U::Engage {
            guid: BOSS,
            priority: 2
        }
    );
    assert_eq!(read("01000000c14230f100"), U::Disengage { guid: BOSS });
    assert_eq!(
        read("02000000c14230f105"),
        U::Priority {
            guid: BOSS,
            priority: 5
        }
    );
    assert_eq!(read("030000000a"), U::Timer { ms: 10 });
    assert_eq!(read("0400000003"), U::EnableObjective { id: 3 });
    assert_eq!(read("050000000301"), U::UpdateObjective { id: 3, delta: 1 });
    assert_eq!(read("0600000003"), U::DisableObjective { id: 3 });
    assert_eq!(read("07000000"), U::Resort);
    assert_eq!(read("08000000"), U::Unknown(8));
    // An engage without its priority byte is a parse failure.
    assert!(messages::parse_server(
        opcode::SMSG_UPDATE_INSTANCE_ENCOUNTER_UNIT,
        &hx("00000000c14230f1")
    )
    .is_err());
}

/// `SMSG_INSTANCE_LOCK_WARNING_QUERY` and the answer `CMSG_INSTANCE_LOCK_RESPONSE`.
#[test]
fn instance_lock_warning_wire() {
    let body = hx(concat!("60ea0000", "05000000", "01"));
    let p = messages::parse_server(opcode::SMSG_INSTANCE_LOCK_WARNING_QUERY, &body).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::InstanceLockWarning { warning }] => assert_eq!(
            *warning,
            InstanceLockWarning {
                ms: 60_000,
                completed_mask: 5,
                extending: true,
            }
        ),
        other => panic!("lock warning decode: {other:?}"),
    }
    assert!(messages::parse_server(
        opcode::SMSG_INSTANCE_LOCK_WARNING_QUERY,
        &hx("60ea000005000000")
    )
    .is_err());
    assert_eq!(messages::instance_lock_response(true), vec![1]);
    assert_eq!(messages::instance_lock_response(false), vec![0]);
}

/// `SMSG_RESET_FAILED_NOTIFY` is empty, and the client's difficulty sets carry one `u32`.
#[test]
fn reset_failed_notify_and_difficulty_sends() {
    let p = messages::parse_server(opcode::SMSG_RESET_FAILED_NOTIFY, &[]).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::ResetFailedNotify] => {}
        other => panic!("reset failed notify decode: {other:?}"),
    }
    assert_eq!(messages::set_difficulty(1), vec![1, 0, 0, 0]);
}

/// `SMSG_INSTANCE_RESET`: one `u32` map id.
#[test]
fn instance_reset_wire() {
    let p = messages::parse_server(opcode::SMSG_INSTANCE_RESET, &hx("24000000")).unwrap();
    match &p {
        ServerPacket::InstanceReset { map } => assert_eq!(*map, 36), // Deadmines
        other => panic!("expected InstanceReset, got {}", other.name()),
    }
    match decode(p).as_slice() {
        [SessionEvent::InstanceReset { map: 36 }] => {}
        other => panic!("instance reset decode: {other:?}"),
    }
    assert!(messages::parse_server(opcode::SMSG_INSTANCE_RESET, &hx("2400")).is_err());
}

/// `SMSG_INSTANCE_RESET_FAILED`: `u32` reason first, then `u32` map id.
#[test]
fn instance_reset_failed_wire() {
    let body = hx(concat!("01000000", "24000000")); // OFFLINE, Deadmines
    let p = messages::parse_server(opcode::SMSG_INSTANCE_RESET_FAILED, &body).unwrap();
    match &p {
        ServerPacket::InstanceResetFailed { failure } => {
            assert_eq!(failure.reason, 1);
            assert_eq!(failure.map, 36);
        }
        other => panic!("expected InstanceResetFailed, got {}", other.name()),
    }
    match decode(p).as_slice() {
        [SessionEvent::InstanceResetFailed { failure }] => {
            assert_eq!(
                InstanceResetFailure::from_wire(failure.reason),
                Some(InstanceResetFailure::Offline)
            );
        }
        other => panic!("instance reset failed decode: {other:?}"),
    }
}

/// The three refusal reasons; vmangos names 3 and up `INSTANCERESET_FAIL_SILENTLY`. Deviation:
/// for those we print nothing, because the reference prints an uninitialized buffer.
#[test]
fn instance_reset_failure_reasons() {
    use InstanceResetFailure as F;
    assert_eq!(F::from_wire(0), Some(F::General));
    assert_eq!(F::from_wire(1), Some(F::Offline));
    assert_eq!(F::from_wire(2), Some(F::Zoning));
    assert_eq!(F::from_wire(3), None, "INSTANCERESET_FAIL_SILENTLY");
    assert_eq!(F::from_wire(99), None);

    assert_eq!(F::General.token(), "INSTANCE_RESET_FAILED");
    assert_eq!(F::Offline.token(), "INSTANCE_RESET_FAILED_OFFLINE");
    assert_eq!(F::Zoning.token(), "INSTANCE_RESET_FAILED_ZONING");
}

/// Three bare `u32` bodies: the save-created flag, the last-instance map and the ownership flag, a
/// bool: the reference tests it against zero.
#[test]
fn save_created_last_instance_and_ownership_wire() {
    let p = messages::parse_server(opcode::SMSG_INSTANCE_SAVE_CREATED, &hx("00000000")).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::InstanceSaveCreated { flag: 0 }] => {}
        other => panic!("save created decode: {other:?}"),
    }

    let p = messages::parse_server(opcode::SMSG_UPDATE_LAST_INSTANCE, &hx("99010000")).unwrap();
    match decode(p).as_slice() {
        [SessionEvent::UpdateLastInstance { map: 409 }] => {}
        other => panic!("last instance decode: {other:?}"),
    }

    for (body, owns) in [("00000000", false), ("01000000", true), ("07000000", true)] {
        let p = messages::parse_server(opcode::SMSG_UPDATE_INSTANCE_OWNERSHIP, &hx(body)).unwrap();
        match decode(p).as_slice() {
            [SessionEvent::UpdateInstanceOwnership { owns: got }] => assert_eq!(*got, owns),
            other => panic!("ownership decode: {other:?}"),
        }
    }
}
