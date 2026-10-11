//! `SMSG_CHAR_ENUM` in vmangos `Player::BuildEnumData` order (`Player.cpp:1629`), two entries per
//! body so any misalignment shows, plus the logout and refused-login replies.

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use benilla_protocol::ServerPacket;

/// One 3.3.5a entry in AzerothCore `Player::BuildEnumData` order: the 1.12.1 fields through the
/// character flags, `u32 customizeFlags`, then 23 slots each carrying an enchant visual.
fn enum_entry_wotlk(guid: u64, name: &str, race: u8, class: u8, pet_display: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&guid.to_le_bytes());
    b.extend_from_slice(name.as_bytes());
    b.push(0);
    b.extend_from_slice(&[race, class, 1]); // female
    b.extend_from_slice(&[3, 4, 5, 6, 7]); // skin/face/hairStyle/hairColor/facialHair
    b.push(80); // level
    b.extend_from_slice(&4395u32.to_le_bytes()); // zone (Dalaran)
    b.extend_from_slice(&571u32.to_le_bytes()); // map (Northrend)
    for c in [5804.1f32, 624.7, 647.8] {
        b.extend_from_slice(&c.to_le_bytes());
    }
    b.extend_from_slice(&0u32.to_le_bytes()); // guild id
    b.extend_from_slice(&0x0000_0400u32.to_le_bytes()); // character flags (HIDE_HELM)
    b.extend_from_slice(&0x0001_0000u32.to_le_bytes()); // customize flags (FACTION)
    b.push(0); // first login
    b.extend_from_slice(&pet_display.to_le_bytes());
    b.extend_from_slice(&80u32.to_le_bytes()); // pet level
    b.extend_from_slice(&2u32.to_le_bytes()); // pet family
    for slot in 0..23u32 {
        b.extend_from_slice(&(3000 + slot).to_le_bytes()); // display id
        b.push(slot as u8); // inventory type
        b.extend_from_slice(&(90 + slot).to_le_bytes()); // enchant visual
    }
    b
}

#[test]
fn a_wotlk_char_enum_roster_parses_in_its_own_layout() {
    let mut body = vec![2u8];
    body.extend(enum_entry_wotlk(0x11, "Deathknight", 10, 6, 0));
    body.extend(enum_entry_wotlk(0x12, "Hunter", 11, 3, 4449));

    let ServerPacket::CharEnum { characters } =
        messages::parse_server(messages::opcode::SMSG_CHAR_ENUM, &body).unwrap()
    else {
        panic!("char enum")
    };
    assert_eq!(characters.len(), 2);
    for (c, (guid, name, race, class, pet)) in characters.iter().zip([
        (0x11, "Deathknight", 10, 6, 0),
        (0x12, "Hunter", 11, 3, 4449),
    ]) {
        assert_eq!(
            (c.guid, c.name.as_str(), c.race, c.class, c.gender),
            (guid, name, race, class, 1)
        );
        assert_eq!(
            (c.skin, c.face, c.hair_style, c.hair_color, c.facial_hair),
            (3, 4, 5, 6, 7)
        );
        assert_eq!((c.level, c.zone, c.map, c.flags), (80, 4395, 571, 0x400));
        assert_eq!((c.position.x, c.position.z), (5804.1, 647.8));
        // The customize flags sit before the pet triple: a parse that skips them shifts it.
        assert_eq!((c.pet_display_id, c.pet_level, c.pet_family), (pet, 80, 2));
        for (slot, item) in c.equipment.iter().enumerate() {
            assert_eq!(
                (item.display_id, item.inventory_type),
                (3000 + slot as u32, slot as u8)
            );
        }
    }
}

#[test]
fn logout_complete_decodes_to_logged_out() {
    let p = messages::parse_server(messages::opcode::SMSG_LOGOUT_COMPLETE, &[]).unwrap();
    assert!(matches!(decode(p)[..], [SessionEvent::LoggedOut]));
}

/// `SMSG_CHARACTER_LOGIN_FAILED` (opcode 65): one raw 1-based reason byte the glue layer maps;
/// vmangos sends only `0x01`, and `0x08` is past the reference's six-entry switch.
#[test]
fn a_refused_character_login_parses_and_decodes() {
    for result in [0x01u8, 0x08] {
        let p = messages::parse_server(
            messages::opcode::SMSG_CHARACTER_LOGIN_FAILED,
            std::slice::from_ref(&result),
        )
        .unwrap();
        assert!(matches!(
            p,
            ServerPacket::CharacterLoginFailed { result: got } if got == result
        ));
        assert!(matches!(
            decode(p)[..],
            [SessionEvent::CharacterLoginFailed { result: got }] if got == result
        ));
    }
}
