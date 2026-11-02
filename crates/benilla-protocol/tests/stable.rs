//! Stable-master wire: send bodies per vmangos `Server/Packets/Npc.cpp:51-76`, the pet list per
//! `Handlers/NPCHandler.cpp:522-575` and the result byte per `Npc.cpp:99-102`.

use benilla_protocol::messages::{self, stable_result, StabledPet};
use benilla_protocol::ServerPacket;

fn hx(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

const NPC: u64 = 0x1234_5678_9abc_def0;
const NPC_HEX: &str = "f0debc9a78563412";

#[test]
fn stable_send_bodies_golden() {
    // Three guid-only verbs with identical bodies: only the opcode tells them apart.
    assert_eq!(
        messages::list_stabled_pets(NPC),
        hx(NPC_HEX),
        "MSG_LIST_STABLED_PETS body"
    );
    assert_eq!(
        messages::stable_pet(NPC),
        hx(NPC_HEX),
        "CMSG_STABLE_PET body"
    );
    assert_eq!(
        messages::buy_stable_slot(NPC),
        hx(NPC_HEX),
        "CMSG_BUY_STABLE_SLOT body"
    );

    // u64 npcGuid + u32 petNumber, the pet's own id (`character_pet.id`), never its slot.
    assert_eq!(
        messages::unstable_pet(NPC, 42),
        hx(&format!("{NPC_HEX}2a000000")),
        "CMSG_UNSTABLE_PET body"
    );
    assert_eq!(
        messages::stable_swap_pet(NPC, 42),
        hx(&format!("{NPC_HEX}2a000000")),
        "CMSG_STABLE_SWAP_PET body"
    );
}

/// Appends one pet record in wire order (`0x5a1264`..`0x5a1290`).
fn push_pet(body: &mut Vec<u8>, pet_number: u32, entry: u32, level: u32, name: &str, flags: u8) {
    body.extend_from_slice(&pet_number.to_le_bytes());
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&level.to_le_bytes());
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.push(flags);
}

fn list_body(num_stable_slots: u8, pets: &[(u32, u32, u32, &str, u8)]) -> Vec<u8> {
    let mut body = NPC.to_le_bytes().to_vec();
    body.push(pets.len() as u8);
    body.push(num_stable_slots);
    for &(n, e, l, name, flags) in pets {
        push_pet(&mut body, n, e, l, name, flags);
    }
    body
}

fn parse_list(body: &[u8]) -> (u64, u8, Vec<StabledPet>) {
    match messages::parse_server(messages::opcode::MSG_LIST_STABLED_PETS, body).unwrap() {
        ServerPacket::ListStabledPets {
            npc,
            num_stable_slots,
            pets,
        } => (npc, num_stable_slots, pets),
        _ => panic!("expected ListStabledPets"),
    }
}

/// A 3.3.5a list as AzerothCore sends it: the current pet's row (flags 1) and stabled rows (flags
/// 3), with no loyalty field. Every row reads whole, the last one too.
#[test]
fn a_full_stable_decodes_every_row_in_wire_order() {
    let body = list_body(
        4,
        &[
            (7, 299, 41, "Rex", 1),
            (8, 1126, 38, "Bruiser", 3),
            (9, 883, 12, "Nibbles", 3),
            (10, 3825, 70, "Ghost", 3),
        ],
    );
    let (npc, slots, pets) = parse_list(&body);
    assert_eq!(npc, NPC);
    assert_eq!(slots, 4, "purchased slots, not occupied ones");
    assert_eq!(pets.len(), 4);
    assert_eq!(
        pets[0],
        StabledPet {
            pet_number: 7,
            creature_entry: 299,
            level: 41,
            name: "Rex".into(),
            flags: 1,
        }
    );
    assert!(pets[0].is_current());
    assert!(pets[1..].iter().all(|p| !p.is_current()));
    // The name is a cstring mid-record; every later field depends on consuming it exactly.
    assert_eq!(
        (
            pets[3].pet_number,
            pets[3].creature_entry,
            pets[3].name.as_str()
        ),
        (10, 3825, "Ghost")
    );
}

/// The current pet is told by its flags byte lacking `0x2`, wherever its row sits.
#[test]
fn the_current_pet_is_the_row_without_bit_two() {
    let body = list_body(2, &[(8, 1126, 38, "Bruiser", 3), (7, 299, 41, "Rex", 1)]);
    let (_, _, pets) = parse_list(&body);
    assert_eq!(
        pets.iter().map(StabledPet::is_current).collect::<Vec<_>>(),
        [false, true]
    );

    let (_, slots, pets) = parse_list(&list_body(1, &[(8, 1126, 38, "Bruiser", 3)]));
    assert_eq!(slots, 1);
    assert!(
        !pets[0].is_current(),
        "a petless hunter's lone row is stabled"
    );
}

/// The purchased slot count and the row count are independent numbers.
#[test]
fn an_empty_list_still_carries_the_purchased_slot_count() {
    let (_, slots, pets) = parse_list(&list_body(1, &[]));
    assert_eq!(slots, 1);
    assert!(pets.is_empty());

    let (_, slots, pets) = parse_list(&list_body(0, &[]));
    assert_eq!(slots, 0);
    assert!(pets.is_empty());
}

/// Each result byte parses as itself, and the codes are the handler's cases (`0x5a17f0`).
#[test]
fn stable_result_codes_parse_as_their_vmangos_bytes() {
    for code in [
        stable_result::ERR_MONEY,
        stable_result::ERR_STABLE,
        stable_result::SUCCESS_STABLE,
        stable_result::SUCCESS_UNSTABLE,
        stable_result::SUCCESS_BUY_SLOT,
    ] {
        match messages::parse_server(messages::opcode::SMSG_STABLE_RESULT, &[code]).unwrap() {
            ServerPacket::StableResult { result } => assert_eq!(result, code),
            _ => panic!("expected StableResult"),
        }
    }

    assert_eq!(
        [
            stable_result::ERR_MONEY,
            stable_result::ERR_STABLE,
            stable_result::SUCCESS_STABLE,
            stable_result::SUCCESS_UNSTABLE,
            stable_result::SUCCESS_BUY_SLOT,
        ],
        [0x01, 0x06, 0x08, 0x09, 0x0A]
    );
}
