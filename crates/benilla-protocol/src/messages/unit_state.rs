//! Unit and combat state the server pushes outside the descriptor: power, combo points, a cooldown
//! delay, a dropped target, a dismount, a clone's caster look and the forced faction reactions.

use std::io;

use crate::wire::{read_i32_le, read_packed_guid, read_u32_le, read_u64_le, read_u8};

/// `SMSG_POWER_UPDATE` (`0x7236c0`): `[packed guid][u8 power type][u32 value]`.
pub(super) fn read_power_update(r: &mut &[u8]) -> io::Result<(u64, u8, u32)> {
    Ok((read_packed_guid(r)?, read_u8(r)?, read_u32_le(r)?))
}

/// `SMSG_UPDATE_COMBO_POINTS` (`0x526530`'s `0x39d` arm): `[packed target][u8 points]`.
pub(super) fn read_combo_points(r: &mut &[u8]) -> io::Result<(u64, u8)> {
    Ok((read_packed_guid(r)?, read_u8(r)?))
}

/// `SMSG_PET_UPDATE_COMBO_POINTS` (`0x5d36a0`): `[packed pet][packed target][u8 points]`.
pub(super) fn read_pet_combo_points(r: &mut &[u8]) -> io::Result<(u64, u64, u8)> {
    Ok((read_packed_guid(r)?, read_packed_guid(r)?, read_u8(r)?))
}

/// `SMSG_MODIFY_COOLDOWN` (`0x804010`): `[u32 spell][u64 caster][i32 delay ms]`.
pub(super) fn read_modify_cooldown(r: &mut &[u8]) -> io::Result<(u32, u64, i32)> {
    Ok((read_u32_le(r)?, read_u64_le(r)?, read_i32_le(r)?))
}

/// A clone's caster look (`OnReceiveMirrorImageDataFromServer` `0x730290`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorImage {
    /// The display the look is for; the client drops the rest unless it is the clone's own
    /// (`0x7302e9`).
    pub display_id: u32,
    /// `None` when the race byte is 0: the clone keeps its display's own look (`0x73030f`).
    pub look: Option<MirrorLook>,
}

/// The character look of a [`MirrorImage`], `ComponentData`'s preferences in wire order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorLook {
    pub race: u8,
    pub sex: u8,
    pub class: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    /// Whose emblem a guild tabard wears.
    pub guild_id: u32,
    /// ItemDisplayInfo ids, 0 for none: head, shoulder, shirt, chest, waist, legs, feet, wrist,
    /// hands, back, tabard (`AddItemBySlot` order).
    pub items: [u32; 11],
}

/// `SMSG_MIRRORIMAGE_DATA` (`0x7324b0`): `[u64 guid][u32 display][u8 race]`, then, for a nonzero
/// race, seven look bytes, the guild id and eleven item displays.
pub(super) fn read_mirror_image(r: &mut &[u8]) -> io::Result<(u64, MirrorImage)> {
    let guid = read_u64_le(r)?;
    let display_id = read_u32_le(r)?;
    let race = read_u8(r)?;
    let look = if race == 0 {
        None
    } else {
        let mut b = [0u8; 7];
        for v in &mut b {
            *v = read_u8(r)?;
        }
        let guild_id = read_u32_le(r)?;
        let mut items = [0u32; 11];
        for v in &mut items {
            *v = read_u32_le(r)?;
        }
        Some(MirrorLook {
            race,
            sex: b[0],
            class: b[1],
            skin: b[2],
            face: b[3],
            hair_style: b[4],
            hair_color: b[5],
            facial_hair: b[6],
            guild_id,
            items,
        })
    };
    // Whatever follows a zero race is never read.
    *r = &[];
    Ok((guid, MirrorImage { display_id, look }))
}

/// `SMSG_SET_FORCED_REACTIONS` (`0x5d15d0`): `[u32 count]` then `(u32 faction, u32 reaction)`
/// pairs, replacing the whole list.
pub(super) fn read_forced_reactions(r: &mut &[u8]) -> io::Result<Vec<(u32, u32)>> {
    let count = read_u32_le(r)?;
    let mut out = Vec::with_capacity(crate::wire::capacity_hint(count, 64));
    for _ in 0..count {
        out.push((read_u32_le(r)?, read_u32_le(r)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(guid: u64) -> Vec<u8> {
        let mut mask = 0u8;
        let mut bytes = Vec::new();
        for i in 0..8 {
            let b = (guid >> (i * 8)) as u8;
            if b != 0 {
                mask |= 1 << i;
                bytes.push(b);
            }
        }
        let mut out = vec![mask];
        out.extend(bytes);
        out
    }

    #[test]
    fn a_power_update_reads_guid_type_and_value() {
        let mut body = packed(0xF130_0000_0000_0042);
        body.push(3);
        body.extend(95u32.to_le_bytes());
        assert_eq!(
            read_power_update(&mut body.as_slice()).unwrap(),
            (0xF130_0000_0000_0042, 3, 95)
        );
    }

    #[test]
    fn combo_bodies_read_target_and_points() {
        let mut body = packed(0xF130_0000_0000_0007);
        body.push(4);
        assert_eq!(
            read_combo_points(&mut body.as_slice()).unwrap(),
            (0xF130_0000_0000_0007, 4)
        );
        let mut body = packed(0xF140_0000_0000_0001);
        body.extend(packed(0xF130_0000_0000_0007));
        body.push(2);
        assert_eq!(
            read_pet_combo_points(&mut body.as_slice()).unwrap(),
            (0xF140_0000_0000_0001, 0xF130_0000_0000_0007, 2)
        );
    }

    #[test]
    fn a_modify_cooldown_delay_is_signed() {
        let mut body = 31687u32.to_le_bytes().to_vec();
        body.extend(7u64.to_le_bytes());
        body.extend((-30_000i32).to_le_bytes());
        assert_eq!(
            read_modify_cooldown(&mut body.as_slice()).unwrap(),
            (31687, 7, -30_000)
        );
    }

    #[test]
    fn a_mirror_image_with_a_race_carries_the_look_and_eleven_items() {
        let mut body = 9u64.to_le_bytes().to_vec();
        body.extend(49u32.to_le_bytes());
        body.extend([1, 0, 8, 2, 3, 4, 5, 6]);
        body.extend(77u32.to_le_bytes());
        for i in 0..11u32 {
            body.extend((100 + i).to_le_bytes());
        }
        let (guid, image) = read_mirror_image(&mut body.as_slice()).unwrap();
        assert_eq!((guid, image.display_id), (9, 49));
        let look = image.look.expect("a race byte brings the look");
        assert_eq!(
            (look.race, look.sex, look.class, look.skin, look.facial_hair),
            (1, 0, 8, 2, 6)
        );
        assert_eq!(
            (look.guild_id, look.items[0], look.items[10]),
            (77, 100, 110)
        );
    }

    #[test]
    fn a_mirror_image_with_race_zero_has_no_look_and_drops_its_tail() {
        let mut body = 9u64.to_le_bytes().to_vec();
        body.extend(49u32.to_le_bytes());
        body.extend([0, 1, 1, 0, 0]);
        let mut r = body.as_slice();
        let (_, image) = read_mirror_image(&mut r).unwrap();
        assert_eq!(image.look, None);
        assert!(r.is_empty());
    }

    #[test]
    fn forced_reactions_read_count_then_pairs() {
        let mut body = 2u32.to_le_bytes().to_vec();
        for v in [529u32, 4, 1135, 1] {
            body.extend(v.to_le_bytes());
        }
        assert_eq!(
            read_forced_reactions(&mut body.as_slice()).unwrap(),
            vec![(529, 4), (1135, 1)]
        );
    }
}
