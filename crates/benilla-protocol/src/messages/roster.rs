//! Character select: the `SMSG_CHAR_ENUM` [`Character`] entry (vmangos `Player::BuildEnumData`
//! order), the create/delete result codes and the create request.

use std::io::{self, Read};

use crate::wire::{read_cstring, read_u32_le, read_u64_le, read_u8, Vector3d};

/// `SMSG_CHAR_CREATE`'s success and name-in-use codes, the indices of `CHAR_CREATE_SUCCESS` and
/// `CHAR_CREATE_NAME_IN_USE` in 12340's response-token table (`0xad91f0`).
pub const CHAR_CREATE_SUCCESS: u8 = 0x2F;
pub const CHAR_CREATE_NAME_IN_USE: u8 = 0x32;
/// `CHAR_CREATE_SERVER_LIMIT`: the account already holds the realm's character limit.
pub const CHAR_CREATE_SERVER_LIMIT: u8 = 0x35;
/// `SMSG_CHAR_DELETE`'s success code, `CHAR_DELETE_SUCCESS`'s index in the same table.
pub const CHAR_DELETE_SUCCESS: u8 = 0x47;
/// Race/Class/Gender values benilla's char-create sends.
pub const RACE_HUMAN: u8 = 0x1;
pub const CLASS_WARRIOR: u8 = 0x1;
pub const GENDER_MALE: u8 = 0x0;

/// A `CMSG_CHAR_CREATE` request for [`super::char_create`]. The outfit id is always 0 on the wire:
/// the server ignores it and picks the start gear (vmangos `CharacterHandler.cpp:310`). The
/// appearance bytes must be valid `CharSections` indices or `Player::ValidateAppearance` refuses.
#[derive(Debug, Clone)]
pub struct CharCreateReq {
    pub name: String,
    /// `ChrRaces.dbc` id (1 Human … 8 Troll).
    pub race: u8,
    /// `ChrClasses.dbc` id (1 Warrior … 11 Druid).
    pub class: u8,
    /// 0 male, 1 female.
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// The vmangos `CHARACTER_FLAG_*` bits of [`Character::flags`] that the select screen renders.
pub const CHARACTER_FLAG_HIDE_HELM: u32 = 0x0400;
pub const CHARACTER_FLAG_HIDE_CLOAK: u32 = 0x0800;
pub const CHARACTER_FLAG_GHOST: u32 = 0x2000;
pub const CHARACTER_FLAG_RENAME: u32 = 0x4000;

/// One visible-equipment slot: an `ItemDisplayInfo.dbc` id (not an item id, 0 for empty), the
/// item's `InventoryType` and its permanent enchant's `ItemVisuals` id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CharEnumItem {
    pub display_id: u32,
    pub inventory_type: u8,
    /// The enchant's `ItemVisuals` id, 0 for none; the select model glows with it where the
    /// display's own visual names no row (`0x4e3fa6`).
    pub enchant_visual: u32,
}

/// One `SMSG_CHAR_ENUM` roster entry.
#[derive(Debug, Clone)]
pub struct Character {
    pub guid: u64,
    pub name: String,
    /// `ChrRaces.dbc` id (1 Human … 8 Troll).
    pub race: u8,
    /// `ChrClasses.dbc` id (1 Warrior … 11 Druid).
    pub class: u8,
    /// 0 male, 1 female.
    pub gender: u8,
    /// First of the five appearance bytes, all `CharSections` indices.
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub level: u8,
    /// `AreaTable.dbc` id of the character's zone.
    pub zone: u32,
    pub map: u32,
    pub position: Vector3d,
    /// `CHARACTER_FLAG_*` bits.
    pub flags: u32,
    /// The paid-service flags: `0x1` customize, `0x10000` faction change, `0x100000` race change.
    pub customize_flags: u32,
    /// Equipment order: head, neck, shoulders, shirt, chest, waist, legs, feet, wrists, hands,
    /// finger x2, trinket x2, back, main hand, off hand, ranged, tabard.
    pub equipment: [CharEnumItem; 19],
    /// The four bag slots, 19 to 22; the select build wears a modelled one as a quiver.
    pub bags: [CharEnumItem; 4],
    /// The pet's `CreatureDisplayInfo.dbc` id, 0 for none. The server already zeroes it for all but
    /// a living hunter or warlock (`Player::BuildEnumData`), so the client needs no class check.
    pub pet_display_id: u32,
    /// The pet's level; `pet_family` is its `CreatureFamily.dbc` id.
    pub pet_level: u32,
    pub pet_family: u32,
}

impl Character {
    /// One `SMSG_CHAR_ENUM` entry (AzerothCore `Player::BuildEnumData`).
    pub(super) fn read(r: &mut impl Read) -> io::Result<Self> {
        let guid = read_u64_le(r)?;
        let name = read_cstring(r)?;
        let race = read_u8(r)?;
        let class = read_u8(r)?;
        let gender = read_u8(r)?;
        let skin = read_u8(r)?;
        let face = read_u8(r)?;
        let hair_style = read_u8(r)?;
        let hair_color = read_u8(r)?;
        let facial_hair = read_u8(r)?;
        let level = read_u8(r)?;
        let zone = read_u32_le(r)?;
        let map = read_u32_le(r)?;
        let position = Vector3d::read(r)?;
        let _guild_id = read_u32_le(r)?;
        let flags = read_u32_le(r)?;
        let customize_flags = read_u32_le(r)?;
        let _first_login = read_u8(r)?;
        let pet_display_id = read_u32_le(r)?;
        let pet_level = read_u32_le(r)?;
        let pet_family = read_u32_le(r)?;
        let mut equipment = [CharEnumItem::default(); 19];
        let mut bags = [CharEnumItem::default(); 4];
        // 19 equipment slots then the four bags, each `(u32 display, u8 inventoryType,
        // u32 enchantVisual)` (`0x464def`, 23 slots).
        for slot in equipment.iter_mut().chain(&mut bags) {
            slot.display_id = read_u32_le(r)?;
            slot.inventory_type = read_u8(r)?;
            slot.enchant_visual = read_u32_le(r)?;
        }
        Ok(Self {
            guid,
            name,
            race,
            class,
            gender,
            skin,
            face,
            hair_style,
            hair_color,
            facial_hair,
            level,
            zone,
            map,
            position,
            flags,
            customize_flags,
            equipment,
            bags,
            pet_display_id,
            pet_level,
            pet_family,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A roster entry's 23 slots each read `(u32 display, u8 inventoryType, u32 enchantVisual)`
    /// (`0x464def`), the bags after the 19 equipment slots.
    #[test]
    fn each_enum_slot_carries_its_enchant_visual() {
        let mut b = Vec::new();
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(b"Name\0");
        b.extend_from_slice(&[1, 1, 0, 2, 3, 4, 5, 6, 80]);
        b.extend_from_slice(&12u32.to_le_bytes()); // zone
        b.extend_from_slice(&0u32.to_le_bytes()); // map
        b.extend_from_slice(&[0; 12]); // position
        b.extend_from_slice(&[0; 12]); // guild, flags, customize flags
        b.push(0); // first login
        b.extend_from_slice(&[0; 12]); // pet display, level, family
        for slot in 0..23u32 {
            b.extend_from_slice(&(100 + slot).to_le_bytes());
            b.push(slot as u8);
            b.extend_from_slice(&(slot * 10).to_le_bytes());
        }
        let c = Character::read(&mut b.as_slice()).expect("one entry");
        assert_eq!(c.equipment[15].display_id, 115, "main hand");
        assert_eq!(c.equipment[15].enchant_visual, 150);
        assert_eq!(c.equipment[18].inventory_type, 18);
        assert_eq!(c.bags[0].display_id, 119);
        assert_eq!(c.bags[3].enchant_visual, 220);
    }
}
