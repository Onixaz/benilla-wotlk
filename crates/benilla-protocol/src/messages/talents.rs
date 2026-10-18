//! 3.3.5a's talent state: `SMSG_TALENTS_INFO` (handler `0x6cd770`) carries every talent group,
//! the active one, unspent points and each group's ranks and glyphs, or the pet's talents; and
//! `CMSG_LEARN_PREVIEW_TALENTS`, the preview mode's one-shot learn.

use std::io::{self, Read};

use crate::wire::{read_i32_le, read_u16_le, read_u32_le, read_u8};

/// One learned talent: a `Talent.dbc` row and its 0-based rank, the index of the rank spell held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LearnedTalent {
    pub talent_id: u32,
    pub rank: u8,
}

/// One talent group (spec) of the player's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TalentGroupInfo {
    pub talents: Vec<LearnedTalent>,
    /// The group's glyph slots as sent; the client keeps the first six (`0x5c9819`).
    pub glyphs: Vec<u16>,
}

/// One decoded `SMSG_TALENTS_INFO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TalentsInfo {
    Player {
        /// Unspent points; a negative wire value reads as 0 (`0x5c9790`).
        unspent: u32,
        /// The active group, 0-based.
        active_group: u8,
        groups: Vec<TalentGroupInfo>,
    },
    Pet {
        unspent: u32,
        talents: Vec<LearnedTalent>,
    },
}

/// Read `SMSG_TALENTS_INFO`: `u8 isPet`. The player body (`0x5c9740`) is `u32 unspent`,
/// `u8 groupCount`, `u8 activeGroup`, then per group `u8 n` × (`u32 talentId`, `u8 rank`) and
/// `u8 glyphCount` × `u16 glyph`. The pet body (`0x5c98a0`) is `u32 unspent`, `u8 n` ×
/// (`u32 talentId`, `u8 rank`).
pub(super) fn read_talents_info(r: &mut impl Read) -> io::Result<TalentsInfo> {
    let is_pet = read_u8(r)? != 0;
    let unspent = read_i32_le(r)?.max(0) as u32;
    if is_pet {
        let talents = read_talent_list(r)?;
        return Ok(TalentsInfo::Pet { unspent, talents });
    }
    let group_count = read_u8(r)?;
    let active_group = read_u8(r)?;
    let mut groups = Vec::with_capacity(group_count.into());
    for _ in 0..group_count {
        let talents = read_talent_list(r)?;
        let glyph_count = read_u8(r)?;
        let glyphs = (0..glyph_count)
            .map(|_| read_u16_le(r))
            .collect::<io::Result<_>>()?;
        groups.push(TalentGroupInfo { talents, glyphs });
    }
    Ok(TalentsInfo::Player {
        unspent,
        active_group,
        groups,
    })
}

fn read_talent_list(r: &mut impl Read) -> io::Result<Vec<LearnedTalent>> {
    let n = read_u8(r)?;
    (0..n)
        .map(|_| {
            Ok(LearnedTalent {
                talent_id: read_u32_le(r)?,
                rank: read_u8(r)?,
            })
        })
        .collect()
}

/// Body of `CMSG_LEARN_PREVIEW_TALENTS` (`0x5c6a10`): `u32 count`, then per talent whose preview
/// rank passes its learned rank `u32 talentId`, `u32 previewRank` (0-based, absolute).
pub fn learn_preview_talents(talents: &[(u32, u32)]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + talents.len() * 8);
    body.extend_from_slice(&(talents.len() as u32).to_le_bytes());
    for (talent_id, rank) in talents {
        body.extend_from_slice(&talent_id.to_le_bytes());
        body.extend_from_slice(&rank.to_le_bytes());
    }
    body
}

/// Body of `CMSG_PET_LEARN_TALENT` (`0x5c8291`): `u64 pet`, `u32 talentId`, `u32 rank`, the
/// pet's next 0-based rank.
pub fn pet_learn_talent(pet_guid: u64, talent_id: u32, rank: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(&talent_id.to_le_bytes());
    body.extend_from_slice(&rank.to_le_bytes());
    body
}

/// Body of `CMSG_LEARN_PREVIEW_TALENTS_PET` (`0x5c6ab4`): the pet guid, then
/// [`learn_preview_talents`]' body.
pub fn learn_preview_talents_pet(pet_guid: u64, talents: &[(u32, u32)]) -> Vec<u8> {
    let mut body = pet_guid.to_le_bytes().to_vec();
    body.extend(learn_preview_talents(talents));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two groups, the second active: AzerothCore's `Player::BuildPlayerTalentsInfoData` layout.
    #[test]
    fn a_dual_spec_player_body() {
        let mut body = vec![0u8];
        body.extend(3u32.to_le_bytes()); // unspent
        body.extend([2u8, 1]); // groups, active
        body.push(2); // group 1: two talents
        body.extend(1867u32.to_le_bytes());
        body.push(4);
        body.extend(1868u32.to_le_bytes());
        body.push(0);
        body.push(6); // six glyph slots
        for g in [0u16, 0, 0, 0, 0, 0] {
            body.extend(g.to_le_bytes());
        }
        body.push(0); // group 2: no talents
        body.push(6);
        for g in [181u16, 0, 0, 0, 0, 0] {
            body.extend(g.to_le_bytes());
        }
        let mut r = body.as_slice();
        let info = read_talents_info(&mut r).unwrap();
        assert!(r.is_empty(), "the whole body is read");
        let TalentsInfo::Player {
            unspent,
            active_group,
            groups,
        } = info
        else {
            panic!("a player body")
        };
        assert_eq!((unspent, active_group, groups.len()), (3, 1, 2));
        assert_eq!(
            groups[0].talents,
            vec![
                LearnedTalent {
                    talent_id: 1867,
                    rank: 4
                },
                LearnedTalent {
                    talent_id: 1868,
                    rank: 0
                }
            ]
        );
        assert!(groups[1].talents.is_empty());
        assert_eq!(groups[1].glyphs[0], 181);
    }

    #[test]
    fn a_pet_body_and_a_negative_unspent() {
        let mut body = vec![1u8];
        body.extend((-2i32).to_le_bytes());
        body.push(1);
        body.extend(2107u32.to_le_bytes());
        body.push(1);
        let mut r = body.as_slice();
        let info = read_talents_info(&mut r).unwrap();
        assert!(r.is_empty());
        let TalentsInfo::Pet { unspent, talents } = info else {
            panic!("a pet body")
        };
        assert_eq!(unspent, 0, "a negative count reads as 0");
        assert_eq!(talents.len(), 1);
    }

    #[test]
    fn the_pet_learn_bodies_lead_with_the_pet_guid() {
        let pet = 0xF140_0000_0000_002Au64;
        let body = pet_learn_talent(pet, 2107, 1);
        assert_eq!(
            body,
            [
                &pet.to_le_bytes()[..],
                &2107u32.to_le_bytes(),
                &1u32.to_le_bytes()
            ]
            .concat()
        );
        let preview = learn_preview_talents_pet(pet, &[(2107, 2)]);
        assert_eq!(&preview[..8], &pet.to_le_bytes());
        assert_eq!(&preview[8..], &learn_preview_talents(&[(2107, 2)])[..]);
    }

    #[test]
    fn the_preview_learn_body() {
        let body = learn_preview_talents(&[(1867, 2), (1868, 0)]);
        assert_eq!(body.len(), 4 + 16);
        assert_eq!(&body[..4], &2u32.to_le_bytes());
        assert_eq!(&body[4..8], &1867u32.to_le_bytes());
        assert_eq!(&body[8..12], &2u32.to_le_bytes());
    }
}
