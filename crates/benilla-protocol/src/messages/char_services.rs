//! The paid character services at character select: rename, customize, race and faction change,
//! and the declined (Russian case) names. Each request carries the character's guid; each answer a
//! result, 0 for success, then the character as it now stands.

use std::io;

use crate::wire::{read_cstring, read_u32_le, read_u64_le, read_u8};

/// A character's new look, as a customize or a race or faction change applied it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharAppearance {
    pub guid: u64,
    pub name: String,
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    /// The new race of a race or faction change; 0 for a customize, which keeps the race.
    pub race: u8,
}

/// `SMSG_CHAR_RENAME` (`0x4da090`): the result, then the guid and the new name while the body has
/// more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharRenameResult {
    pub result: u8,
    pub renamed: Option<(u64, String)>,
}

/// `SMSG_CHAR_CUSTOMIZE` (`0x4d9190`) and `SMSG_CHAR_FACTION_CHANGE` (`0x4d92d0`): the result,
/// then the character's look while the body has more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharChangeResult {
    pub result: u8,
    pub applied: Option<CharAppearance>,
}

/// `SMSG_SET_PLAYER_DECLINED_NAMES_RESULT` (`0x4da190`): a `u32` result, then the guid on success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclinedNamesResult {
    pub result: u32,
    pub guid: u64,
}

/// What a paid character change sends: the guid, the name and the look the create screen holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharChangeReq {
    pub guid: u64,
    pub name: String,
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub race: u8,
}

pub(super) fn read_char_rename(r: &mut &[u8]) -> io::Result<CharRenameResult> {
    let result = read_u8(r)?;
    let renamed = if r.is_empty() {
        None
    } else {
        Some((read_u64_le(r)?, read_cstring(r)?))
    };
    Ok(CharRenameResult { result, renamed })
}

/// The customize answer carries six look bytes, the faction change a seventh, the race.
pub(super) fn read_char_change(r: &mut &[u8], with_race: bool) -> io::Result<CharChangeResult> {
    let result = read_u8(r)?;
    if r.is_empty() {
        return Ok(CharChangeResult {
            result,
            applied: None,
        });
    }
    let mut a = CharAppearance {
        guid: read_u64_le(r)?,
        name: read_cstring(r)?,
        gender: read_u8(r)?,
        skin: read_u8(r)?,
        face: read_u8(r)?,
        hair_style: read_u8(r)?,
        hair_color: read_u8(r)?,
        facial_hair: read_u8(r)?,
        race: 0,
    };
    if with_race {
        a.race = read_u8(r)?;
    }
    Ok(CharChangeResult {
        result,
        applied: Some(a),
    })
}

pub(super) fn read_declined_names_result(r: &mut &[u8]) -> io::Result<DeclinedNamesResult> {
    let result = read_u32_le(r)?;
    let guid = if result == 0 { read_u64_le(r)? } else { 0 };
    Ok(DeclinedNamesResult { result, guid })
}

fn guid_and_name(guid: u64, name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + name.len() + 7);
    body.extend_from_slice(&guid.to_le_bytes());
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body
}

/// `CMSG_CHAR_RENAME` body: the guid and the new name (`0x4d8d20`).
pub fn char_rename(guid: u64, name: &str) -> Vec<u8> {
    guid_and_name(guid, name)
}

/// `CMSG_CHAR_CUSTOMIZE` body (`0x4d8e10`): the guid, the name, then gender, skin, hair colour,
/// hair style, facial hair and face.
pub fn char_customize(req: &CharChangeReq) -> Vec<u8> {
    let mut body = guid_and_name(req.guid, &req.name);
    body.extend_from_slice(&[
        req.gender,
        req.skin,
        req.hair_color,
        req.hair_style,
        req.facial_hair,
        req.face,
    ]);
    body
}

/// `CMSG_CHAR_FACTION_CHANGE` and `CMSG_CHAR_RACE_CHANGE` body (`0x4d8f20`, `0x4d9040`): the
/// customize body, then the race.
pub fn char_faction_change(req: &CharChangeReq) -> Vec<u8> {
    let mut body = char_customize(req);
    body.push(req.race);
    body
}

/// `CMSG_SET_PLAYER_DECLINED_NAMES` body (`0x4d9a40`): the guid, the character's name, then its
/// five declined forms.
pub fn set_player_declined_names(guid: u64, name: &str, cases: &[String; 5]) -> Vec<u8> {
    let mut body = guid_and_name(guid, name);
    for case in cases {
        body.extend_from_slice(case.as_bytes());
        body.push(0);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    fn look(b: &mut Vec<u8>) {
        b.extend_from_slice(&0x2Au64.to_le_bytes());
        b.extend_from_slice(b"Newname\0");
        b.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn each_result_reads_its_12340_body() {
        let mut b = vec![0];
        b.extend_from_slice(&0x2Au64.to_le_bytes());
        b.extend_from_slice(b"Newname\0");
        match parse_server(opcode::SMSG_CHAR_RENAME, &b).unwrap() {
            ServerPacket::CharRename(r) => {
                assert_eq!(r.result, 0);
                assert_eq!(r.renamed, Some((0x2A, "Newname".into())));
            }
            other => panic!("{}", other.name()),
        }
        // A refusal is the result alone.
        match parse_server(opcode::SMSG_CHAR_RENAME, &[0x32]).unwrap() {
            ServerPacket::CharRename(r) => assert_eq!((r.result, r.renamed), (0x32, None)),
            other => panic!("{}", other.name()),
        }

        let mut b = vec![0];
        look(&mut b);
        let want = CharAppearance {
            guid: 0x2A,
            name: "Newname".into(),
            gender: 1,
            skin: 2,
            face: 3,
            hair_style: 4,
            hair_color: 5,
            facial_hair: 6,
            race: 0,
        };
        match parse_server(opcode::SMSG_CHAR_CUSTOMIZE, &b).unwrap() {
            ServerPacket::CharCustomize(r) => assert_eq!(r.applied, Some(want.clone())),
            other => panic!("{}", other.name()),
        }
        b.push(10);
        match parse_server(opcode::SMSG_CHAR_FACTION_CHANGE, &b).unwrap() {
            ServerPacket::CharFactionChange(r) => {
                assert_eq!(r.applied, Some(CharAppearance { race: 10, ..want }))
            }
            other => panic!("{}", other.name()),
        }
        match parse_server(opcode::SMSG_CHAR_FACTION_CHANGE, &[0x3d]).unwrap() {
            ServerPacket::CharFactionChange(r) => assert_eq!((r.result, r.applied), (0x3d, None)),
            other => panic!("{}", other.name()),
        }

        let mut b = 0u32.to_le_bytes().to_vec();
        b.extend_from_slice(&0x2Au64.to_le_bytes());
        assert!(matches!(
            parse_server(opcode::SMSG_SET_PLAYER_DECLINED_NAMES_RESULT, &b).unwrap(),
            ServerPacket::DeclinedNamesResult(DeclinedNamesResult {
                result: 0,
                guid: 0x2A
            })
        ));
        assert!(matches!(
            parse_server(
                opcode::SMSG_SET_PLAYER_DECLINED_NAMES_RESULT,
                &1u32.to_le_bytes()
            )
            .unwrap(),
            ServerPacket::DeclinedNamesResult(DeclinedNamesResult { result: 1, guid: 0 })
        ));
    }

    #[test]
    fn the_requests_put_the_look_in_the_references_order() {
        let req = CharChangeReq {
            guid: 0x2A,
            name: "Ab".into(),
            gender: 1,
            skin: 2,
            face: 3,
            hair_style: 4,
            hair_color: 5,
            facial_hair: 6,
            race: 7,
        };
        let mut head = 0x2Au64.to_le_bytes().to_vec();
        head.extend_from_slice(b"Ab\0");
        let mut want = head.clone();
        want.extend_from_slice(&[1, 2, 5, 4, 6, 3]);
        assert_eq!(char_customize(&req), want);
        want.push(7);
        assert_eq!(char_faction_change(&req), want);
        assert_eq!(char_rename(0x2A, "Ab"), head);
        let cases = ["a", "b", "c", "d", "e"].map(String::from);
        let mut want = head;
        want.extend_from_slice(b"a\0b\0c\0d\0e\0");
        assert_eq!(set_player_declined_names(0x2A, "Ab", &cases), want);
    }
}
