//! `SMSG_EQUIPMENT_SET_LIST` (`0x5af490`): the player's saved equipment sets, which an action
//! button names by id (`0x20000000 | id`, `CGActionBar::GetEquipmentSetID` `0x5a7950`).

use std::io;

use crate::wire::{read_cstring, read_packed_guid, read_u32_le};

/// The slots a set records, every equipment slot (`0x5af608` reads 19 guids).
pub const EQUIPMENT_SET_SLOTS: usize = 19;

/// The most sets the client keeps; it stops reading at the eleventh (`0x5af545`).
pub const MAX_EQUIPMENT_SETS: usize = 10;

/// One saved set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentSet {
    /// The set's own guid, which the server keys saves on.
    pub guid: u64,
    /// The id an action button names, `GetSetByID`'s key (`[set+0x10]`, `0x5ae5c0`).
    pub id: u32,
    /// The name `GetActionInfo` answers (`[set+0x114]`, `0x5a9178`).
    pub name: String,
    /// The icon file name under `Interface\Icons\` (`[set+0x14]`, `0x5ae6b0`).
    pub icon: String,
    /// Each slot's item guid; 0 is empty, 1 an ignored slot.
    pub items: [u64; EQUIPMENT_SET_SLOTS],
}

/// Read `SMSG_EQUIPMENT_SET_LIST`: `u32 count`, then per set a packed guid, `u32 id`, the name,
/// the icon and 19 packed item guids. Past ten sets the client reads no more (`0x5af545`).
pub(super) fn read_equipment_set_list(r: &mut &[u8]) -> io::Result<Vec<EquipmentSet>> {
    let count = read_u32_le(r)? as usize;
    let mut sets = Vec::with_capacity(count.min(MAX_EQUIPMENT_SETS));
    for _ in 0..count.min(MAX_EQUIPMENT_SETS) {
        let guid = read_packed_guid(r)?;
        let id = read_u32_le(r)?;
        let name = read_cstring(r)?;
        let icon = read_cstring(r)?;
        let mut items = [0u64; EQUIPMENT_SET_SLOTS];
        for item in &mut items {
            *item = read_packed_guid(r)?;
        }
        sets.push(EquipmentSet {
            guid,
            id,
            name,
            icon,
            items,
        });
    }
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packed guid: the mask byte, then the nonzero bytes low first.
    fn packed(guid: u64) -> Vec<u8> {
        let mut mask = 0u8;
        let mut bytes = Vec::new();
        for (i, b) in guid.to_le_bytes().iter().enumerate() {
            if *b != 0 {
                mask |= 1 << i;
                bytes.push(*b);
            }
        }
        [vec![mask], bytes].concat()
    }

    fn set_body(guid: u64, id: u32, name: &str, icon: &str) -> Vec<u8> {
        let mut b = packed(guid);
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        b.extend_from_slice(icon.as_bytes());
        b.push(0);
        for slot in 0..EQUIPMENT_SET_SLOTS as u64 {
            b.extend(packed(if slot == 0 { 0x4000_0000_0000_0123 } else { 0 }));
        }
        b
    }

    #[test]
    fn two_sets_read_whole() {
        let mut body = 2u32.to_le_bytes().to_vec();
        body.extend(set_body(0x10, 0, "Tank", "INV_Shield_06"));
        body.extend(set_body(0x11, 3, "Heal", "Spell_Holy_Heal"));
        let mut r = &body[..];
        let sets = read_equipment_set_list(&mut r).unwrap();
        assert!(r.is_empty(), "the body is fully consumed");
        assert_eq!(sets.len(), 2);
        assert_eq!(
            (sets[1].guid, sets[1].id, sets[1].name.as_str()),
            (0x11, 3, "Heal")
        );
        assert_eq!(sets[0].icon, "INV_Shield_06");
        assert_eq!(sets[0].items[0], 0x4000_0000_0000_0123);
        assert_eq!(sets[0].items[18], 0);
    }

    #[test]
    fn an_empty_list_is_the_count_alone() {
        let body = 0u32.to_le_bytes();
        let mut r = &body[..];
        assert!(read_equipment_set_list(&mut r).unwrap().is_empty());
        assert!(r.is_empty());
    }

    #[test]
    fn a_short_set_errors() {
        let mut body = 1u32.to_le_bytes().to_vec();
        body.extend(&set_body(0x10, 0, "Tank", "INV_Shield_06")[..12]);
        assert!(read_equipment_set_list(&mut &body[..]).is_err());
    }
}
