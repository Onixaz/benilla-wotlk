//! Game object messages: `CMSG_GAMEOBJ_USE`, the one verb for every usable object, which the
//! server answers through loot, gossip or state; the template query; and one-shot animations.

use std::io;

use crate::wire::{read_cstring, read_f32_le, read_i32_le, read_u32_le, read_u64_le};

/// `CMSG_GAMEOBJ_USE` body: the object's full guid (`Handlers/SpellHandler.cpp`). A chest's loot
/// opens through this, never `CMSG_LOOT`, which rejects a game object guid.
pub fn gameobj_use(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// `CMSG_GAMEOBJECT_QUERY` body: the template entry, then a full guid (vmangos `QueryGameObject`).
pub fn gameobject_query(entry: u32, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&entry.to_le_bytes());
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// A game object template from `SMSG_GAMEOBJECT_QUERY_RESPONSE`; `data` is the raw
/// `GameObjectData` union, whose slots mean different things per `type_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct GameObjectQueryInfo {
    pub type_id: u32,
    pub display_id: u32,
    pub name: String,
    /// The icon name (AzerothCore `IconName`).
    pub icon: String,
    /// The cast bar text while the object is opened (AzerothCore `castBarCaption`).
    pub cast_bar_caption: String,
    pub data: [i32; 24],
    pub scale: f32,
    /// The quest items the object can drop.
    pub quest_items: [u32; 6],
}

/// Read `SMSG_GAMEOBJECT_QUERY_RESPONSE` as 12340's GameObject record unpack (`0x98d750`) does:
/// four names, icon, cast-bar caption and an unused string, `data[24]`, an `f32` scale and six
/// quest items. A miss is the lone `u32` `entry | 0x8000_0000`.
pub(super) fn read_gameobject_query_response(
    r: &mut &[u8],
) -> io::Result<(u32, Option<GameObjectQueryInfo>)> {
    let entry = read_u32_le(r)?;
    if entry & 0x8000_0000 != 0 {
        return Ok((entry & 0x7FFF_FFFF, None));
    }
    let type_id = read_u32_le(r)?;
    let display_id = read_u32_le(r)?;
    let name = read_cstring(r)?;
    for _ in 0..3 {
        let _ = read_cstring(r)?; // name2..name4
    }
    let icon = read_cstring(r)?;
    let cast_bar_caption = read_cstring(r)?;
    let _ = read_cstring(r)?; // AzerothCore `unk1`
    let mut data = [0i32; 24];
    for slot in &mut data {
        *slot = read_i32_le(r)?;
    }
    let scale = read_f32_le(r)?;
    let mut quest_items = [0u32; 6];
    for item in &mut quest_items {
        *item = read_u32_le(r)?;
    }
    Ok((
        entry,
        Some(GameObjectQueryInfo {
            type_id,
            display_id,
            name,
            icon,
            cast_bar_caption,
            data,
            scale,
            quest_items,
        }),
    ))
}

/// Read `SMSG_GAMEOBJECT_CUSTOM_ANIM`: the guid, then `animId`, which the 1.12 client plays as
/// custom substate `8 + animId` (AnimationData 153 to 156), ignoring `animId >= 4`.
pub(super) fn read_gameobject_custom_anim(r: &mut &[u8]) -> io::Result<(u64, u32)> {
    let guid = read_u64_le(r)?;
    let anim_id = read_u32_le(r)?;
    Ok((guid, anim_id))
}

/// Read `SMSG_GAMEOBJECT_DESPAWN_ANIM`, only the guid (`Objects/Object.cpp:2308`); the 1.12
/// client plays substate 12, AnimationData 157 Despawn.
pub(super) fn read_gameobject_despawn_anim(r: &mut &[u8]) -> io::Result<u64> {
    read_u64_le(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cmsg_gameobj_use_body_golden() {
        // SpellHandler.cpp reads a single ObjectGuid.
        assert_eq!(
            gameobj_use(0x1234_5678_9abc_def0),
            hx("f0debc9a78563412"),
            "CMSG_GAMEOBJ_USE body"
        );
    }

    #[test]
    fn cmsg_gameobject_query_body_golden() {
        // As vmangos `QueryGameObject::ReadFromWorldPacket` reads it.
        assert_eq!(
            gameobject_query(1731, 0x1234_5678_9abc_def0),
            hx("c3060000f0debc9a78563412"),
            "CMSG_GAMEOBJECT_QUERY body"
        );
    }

    /// The 12340 layout (`0x98d750`), with the two strings 1.12 lacks empty as servers send
    /// them: a decoder that skips them reads `data[0]` two bytes early, a lock 57 as `0x390000`.
    #[test]
    fn gameobject_query_response_hit_decodes_the_12340_layout() {
        let mut body = 106318u32.to_le_bytes().to_vec(); // entry
        body.extend_from_slice(&3u32.to_le_bytes()); // type = GAMEOBJECT_TYPE_CHEST
        body.extend_from_slice(&123u32.to_le_bytes()); // displayId
        body.extend_from_slice(b"Chest\0"); // name
        body.extend_from_slice(&[0, 0, 0]); // name2..name4, empty C-strings
        body.extend_from_slice(b"Icon\0"); // icon
        body.extend_from_slice(&[0, 0]); // cast-bar caption, unused string
        let mut data = [0i32; 24];
        data[0] = 57; // lockId
        data[1] = 2; // lootId
        data[23] = -1;
        for v in data {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(&1.5f32.to_le_bytes()); // scale
        for item in [0u32, 0, 0, 0, 0, 2589] {
            body.extend_from_slice(&item.to_le_bytes());
        }

        let mut r = &body[..];
        let (entry, info) = read_gameobject_query_response(&mut r).unwrap();
        assert!(r.is_empty(), "{} trailing byte(s)", r.len());
        assert_eq!(entry, 106318);
        let info = info.expect("hit");
        assert_eq!(info.type_id, 3);
        assert_eq!(info.display_id, 123);
        assert_eq!(info.name, "Chest");
        assert_eq!(info.icon, "Icon");
        assert_eq!(info.cast_bar_caption, "");
        assert_eq!(info.data[0], 57);
        assert_eq!(info.data[1], 2);
        assert_eq!(info.data[23], -1);
        assert_eq!(info.scale, 1.5);
        assert_eq!(info.quest_items, [0, 0, 0, 0, 0, 2589]);

        match parse_server(opcode::SMSG_GAMEOBJECT_QUERY_RESPONSE, &body).unwrap() {
            ServerPacket::GameObjectQueryResponse { entry, info } => {
                assert_eq!(entry, 106318);
                assert_eq!(info.expect("hit").data[0], 57);
            }
            other => panic!("expected GameObjectQueryResponse, got {}", other.name()),
        }
    }

    #[test]
    fn gameobject_query_response_keeps_a_cast_bar_caption() {
        let mut body = 1731u32.to_le_bytes().to_vec();
        body.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice(&310u32.to_le_bytes());
        body.extend_from_slice(b"Copper Vein\0\0\0\0");
        body.extend_from_slice(b"\0Mining\0\0");
        let mut data = [0i32; 24];
        data[0] = 38;
        for v in data {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(&1.0f32.to_le_bytes());
        body.extend_from_slice(&[0; 24]);

        let mut r = &body[..];
        let (_, info) = read_gameobject_query_response(&mut r).unwrap();
        assert!(r.is_empty());
        let info = info.expect("hit");
        assert_eq!(info.icon, "");
        assert_eq!(info.cast_bar_caption, "Mining");
        assert_eq!(info.data[0], 38);
    }

    #[test]
    fn gameobject_despawn_anim_decodes() {
        let body = hx("f0debc9a78563412");
        match parse_server(opcode::SMSG_GAMEOBJECT_DESPAWN_ANIM, &body).unwrap() {
            ServerPacket::GameObjectDespawnAnim { guid } => {
                assert_eq!(guid, 0x1234_5678_9abc_def0);
            }
            other => panic!("expected GameObjectDespawnAnim, got {}", other.name()),
        }
    }

    #[test]
    fn gameobject_custom_anim_decodes() {
        // A fishing bobber's bite is animId 0.
        let body = hx("f0debc9a7856341200000000");
        match parse_server(opcode::SMSG_GAMEOBJECT_CUSTOM_ANIM, &body).unwrap() {
            ServerPacket::GameObjectCustomAnim { guid, anim_id } => {
                assert_eq!(guid, 0x1234_5678_9abc_def0);
                assert_eq!(anim_id, 0);
            }
            other => panic!("expected GameObjectCustomAnim, got {}", other.name()),
        }
    }

    #[test]
    fn fish_verdicts_decode_from_empty_bodies() {
        // Both fishing verdicts are size-0 sends (vmangos `GameObject::Update`/`Use`).
        assert!(matches!(
            parse_server(opcode::SMSG_FISH_NOT_HOOKED, &[]).unwrap(),
            ServerPacket::FishNotHooked
        ));
        assert!(matches!(
            parse_server(opcode::SMSG_FISH_ESCAPED, &[]).unwrap(),
            ServerPacket::FishEscaped
        ));
    }

    #[test]
    fn gameobject_query_response_miss_decodes() {
        let body = (1731u32 | 0x8000_0000).to_le_bytes();
        match parse_server(opcode::SMSG_GAMEOBJECT_QUERY_RESPONSE, &body).unwrap() {
            ServerPacket::GameObjectQueryResponse { entry, info } => {
                assert_eq!(entry, 1731);
                assert!(info.is_none());
            }
            other => panic!("expected GameObjectQueryResponse, got {}", other.name()),
        }
    }
}
