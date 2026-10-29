//! The petition family (opcodes `0x1BB`-`0x1C7`, `0x2C1`): the guild and arena-team charters. A
//! team or guild is founded by buying a charter, collecting its signatures and turning it in. The
//! charter item's enchantment slot 0 holds the petition id (`0x6dd680`); the reference tooltip
//! skips enchant lines on a charter, so nothing may read that slot as an enchantment.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_u16_le, read_u32_le, read_u64_le, read_u8};

/// The guild charter's item entry, vmangos `GUILD_CHARTER` (`PetitionsHandler.cpp:37`).
pub const CHARTER_ITEM_ENTRY: u32 = 5863;

/// The charter's display id, vmangos `CHARTER_DISPLAY_ID` (`PetitionsHandler.cpp:39`).
pub const CHARTER_DISPLAY_ID: u32 = 16161;

/// The template-flag bit of a signable petition (vmangos `ItemPrototype.h:77`); the reference
/// tooltip keys on it for the green `ITEM_SIGNABLE` line and to suppress enchantment lines.
pub const ITEM_FLAG_CHARTER: u32 = 0x0000_2000;

/// The most signatures a charter holds (`PetitionFrame.lua:1`, `PetitionsHandler.cpp:269`); the
/// requirement is [`PetitionQueryResponse::min_signatures`].
pub const MAX_PETITION_SIGNATURES: usize = 9;

/// Charter-name cap in characters (vmangos `MAX_CHARTER_NAME`, `ObjectMgr.h:401`); equal to
/// [`super::GUILD_NAME_MAX_LENGTH`] because the charter name becomes the guild's.
pub const CHARTER_NAME_MAX_LENGTH: usize = 24;

/// The result code shared by `SMSG_PETITION_SIGN_RESULTS` and `SMSG_TURN_IN_PETITION_RESULTS`
/// (vmangos `PetitionSigns`, `Guild/Guild.h:147-154`); each packet uses its own subset.
pub mod petition_result {
    /// Signed, or turned in; both packets.
    pub const OK: u32 = 0;
    /// Sign only: this account already signed. vmangos checks per account before per character
    /// (`GuildMgr.cpp:388-400`), so an alt on the same account gets this.
    pub const ALREADY_SIGNED: u32 = 1;
    /// Turn-in only: the founder is already in a guild (`PetitionsHandler.cpp:427`); a guilded
    /// signer gets `SMSG_GUILD_COMMAND_RESULT` instead (`:258`).
    pub const ALREADY_IN_GUILD: u32 = 2;
    /// Sign only: you cannot sign your own charter (`PetitionsHandler.cpp:237`).
    pub const CANT_SIGN_OWN: u32 = 3;
    /// Turn-in only: not enough signatures (`PetitionsHandler.cpp:438`); vmangos requires exactly
    /// `MinPetitionSigns` (`GuildMgr.h:124`).
    pub const NEED_MORE: u32 = 4;
    /// Not on the same realm (`ERR_PETITION_NOT_SAME_SERVER`); vmangos never sends it.
    pub const NOT_SERVER: u32 = 5;
    /// Turn-in only: the charter is not ours to turn in (`ERR_GUILD_PERMISSIONS`, `0x6cf203`).
    pub const NOT_AUTHORIZED: u32 = 11;
}

/// One row of `SMSG_PETITION_SHOWLIST` (`0x18` bytes in the client): a charter the NPC sells. A
/// guild registrar sends one, an arena registrar the three team sizes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionShowListEntry {
    /// Row index, 1-based; `CMSG_PETITION_BUY` echoes it as the charter's `muid`.
    pub index: u32,
    /// The item entry sold.
    pub charter_entry: u32,
    /// Its display id, the icon `GetPetitionItemInfo` builds.
    pub charter_display_id: u32,
    /// Price in copper.
    pub charter_cost: u32,
    /// The charter type: 0 a guild charter; 12340's buy refusals test 1 for an arena one.
    pub charter_type: u32,
    /// The signatures required; an arena row's is the team size less one.
    pub required_signatures: u32,
}

/// `SMSG_PETITION_SHOWLIST`: the registrar NPC's charter list, which opens its window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionShowList {
    /// The NPC; every later verb in the flow that names an NPC takes this guid.
    pub npc: u64,
    pub entries: Vec<PetitionShowListEntry>,
}

/// Read `SMSG_PETITION_SHOWLIST` (`0x6d69f0`): `u64 npc, u8 count`, then six `u32` per row.
/// The client keeps ten rows; the rest are read and dropped there.
pub(super) fn read_petition_show_list(r: &mut impl Read) -> io::Result<PetitionShowList> {
    let npc = read_u64_le(r)?;
    let count = read_u8(r)?;
    let mut entries = Vec::with_capacity(capacity_hint(count, 10));
    for _ in 0..count {
        entries.push(PetitionShowListEntry {
            index: read_u32_le(r)?,
            charter_entry: read_u32_le(r)?,
            charter_display_id: read_u32_le(r)?,
            charter_cost: read_u32_le(r)?,
            charter_type: read_u32_le(r)?,
            required_signatures: read_u32_le(r)?,
        });
    }
    Ok(PetitionShowList { npc, entries })
}

/// One signature on a charter: the signer's guid and a trailing dword.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionSignature {
    /// The signer's guid; the packet carries no names.
    pub signer: u64,
    /// Always 0 from vmangos (`GuildMgr.cpp:358-366`); decoded, not skipped.
    pub unknown: u32,
}

/// `SMSG_PETITION_SHOW_SIGNATURES`: a charter's signers, in answer to our request or when
/// someone offers us theirs. Its name and requirement come from `SMSG_PETITION_QUERY_RESPONSE`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionShowSignatures {
    /// The charter item's guid, which every later verb takes; not the petition id.
    pub item: u64,
    /// The charter's owner; whether it is us decides the leader's view or a signer's.
    pub owner: u64,
    /// The key `CMSG_PETITION_QUERY` takes, also carried in the charter's enchantment slot 0.
    pub petition_id: u32,
    pub signatures: Vec<PetitionSignature>,
}

/// Read `SMSG_PETITION_SHOW_SIGNATURES` (hand-built at vmangos `PetitionsHandler.cpp:160-168`,
/// `:390-397`): `u64 item, u64 owner, u32 petitionId, u8 count`, then `u64 signer, u32 0` each.
pub(super) fn read_petition_show_signatures(
    r: &mut impl Read,
) -> io::Result<PetitionShowSignatures> {
    let item = read_u64_le(r)?;
    let owner = read_u64_le(r)?;
    let petition_id = read_u32_le(r)?;
    let count = read_u8(r)?;
    // "Client hard limit at 9 signatures" (vmangos `PetitionsHandler.cpp:269-270`).
    let mut signatures = Vec::with_capacity(capacity_hint(count, 9));
    for _ in 0..count {
        signatures.push(PetitionSignature {
            signer: read_u64_le(r)?,
            unknown: read_u32_le(r)?,
        });
    }
    Ok(PetitionShowSignatures {
        item,
        owner,
        petition_id,
        signatures,
    })
}

/// `SMSG_PETITION_SIGN_RESULTS`: the verdict on one signature. A success goes, identical, to the
/// signer and the online owner (`PetitionsHandler.cpp:299`, `:312-316`); both name the signer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionSignResults {
    /// The charter item's guid.
    pub item: u64,
    /// The signer's guid, in both copies.
    pub player: u64,
    /// A [`petition_result`] code.
    pub result: u32,
}

/// Read `SMSG_PETITION_SIGN_RESULTS` (vmangos `Packets/Petition.cpp:69-74`).
pub(super) fn read_petition_sign_results(r: &mut impl Read) -> io::Result<PetitionSignResults> {
    Ok(PetitionSignResults {
        item: read_u64_le(r)?,
        player: read_u64_le(r)?,
        result: read_u32_le(r)?,
    })
}

/// `SMSG_PETITION_QUERY_RESPONSE`: a petition's record (`0x13c4` bytes in the client), keyed by
/// petition id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionQueryResponse {
    /// The petition id we asked about.
    pub petition_id: u32,
    /// The charter owner's character guid.
    pub owner: u64,
    /// The proposed guild or team's name.
    pub name: String,
    /// Free text on the charter.
    pub body_text: String,
    /// Signatures required (`+0x1110`): `GetPetitionInfo`'s seventh return; an arena charter's is
    /// the team size less one.
    pub min_signatures: u32,
    /// Signatures that fit (`+0x1114`), `GetPetitionInfo`'s fourth.
    pub max_signatures: u32,
    pub deadline: u32,
    pub issue_date: u32,
    pub allowed_guild_id: u32,
    pub allowed_classes: u32,
    pub allowed_races: u32,
    /// The record's one `u16`.
    pub allowed_genders: u16,
    /// The level window `OfferPetition` checks a target against (`0x5cf220`); 0 for none.
    pub allowed_min_level: u32,
    pub allowed_max_level: u32,
    /// The choice count; ten strings follow whatever it says.
    pub num_choices: u32,
    pub choices: Vec<String>,
    /// The vendor row the charter was bought from.
    pub muid: u32,
    /// `+0x13bc`: 0 a guild charter, 1 an arena charter, else another petition.
    pub static_type: u32,
}

/// The arena charter's `static_type` (`GetPetitionInfo` answers `"arena"`).
pub const PETITION_TYPE_ARENA: u32 = 1;

/// Read `SMSG_PETITION_QUERY_RESPONSE` (the petition record's unpack, `0x98d090`): the fields in
/// record order, then always ten choice strings (`0x98d171`), then `muid` before `static_type`.
pub(super) fn read_petition_query_response(r: &mut impl Read) -> io::Result<PetitionQueryResponse> {
    let mut p = PetitionQueryResponse {
        petition_id: read_u32_le(r)?,
        owner: read_u64_le(r)?,
        name: read_cstring(r)?,
        body_text: read_cstring(r)?,
        min_signatures: read_u32_le(r)?,
        max_signatures: read_u32_le(r)?,
        deadline: read_u32_le(r)?,
        issue_date: read_u32_le(r)?,
        allowed_guild_id: read_u32_le(r)?,
        allowed_classes: read_u32_le(r)?,
        allowed_races: read_u32_le(r)?,
        allowed_genders: read_u16_le(r)?,
        allowed_min_level: read_u32_le(r)?,
        allowed_max_level: read_u32_le(r)?,
        num_choices: read_u32_le(r)?,
        ..Default::default()
    };
    for _ in 0..PETITION_CHOICES {
        p.choices.push(read_cstring(r)?);
    }
    p.muid = read_u32_le(r)?;
    p.static_type = read_u32_le(r)?;
    Ok(p)
}

/// The petition record's fixed ten choice rows, packed and unpacked whatever the count says.
const PETITION_CHOICES: usize = 10;

/// `MSG_PETITION_RENAME`: the server's echo of a successful rename (`PetitionsHandler.cpp:209`);
/// a rejected name gets `SMSG_GUILD_COMMAND_RESULT`. Any holder of the item may rename it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionRename {
    /// The charter item's guid.
    pub item: u64,
    pub name: String,
}

/// Read `MSG_PETITION_RENAME` (vmangos `Packets/Petition.cpp:104-108`): `u64` item, cstring name.
pub(super) fn read_petition_rename(r: &mut impl Read) -> io::Result<PetitionRename> {
    Ok(PetitionRename {
        item: read_u64_le(r)?,
        name: read_cstring(r)?,
    })
}

/// Read `SMSG_TURN_IN_PETITION_RESULTS` (vmangos `Packets/Petition.cpp:76-79`): a lone
/// [`petition_result`] code. A name collision sends `SMSG_GUILD_COMMAND_RESULT` and no result
/// packet at all (`PetitionsHandler.cpp:445`).
pub(super) fn read_turn_in_petition_results(r: &mut impl Read) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `MSG_PETITION_DECLINE` inbound (vmangos `Packets/Petition.cpp:110-113`): the decliner's
/// guid, sent to the owner only. The outbound form carries the item guid instead.
pub(super) fn read_petition_decline(r: &mut impl Read) -> io::Result<u64> {
    read_u64_le(r)
}

/// Body of `CMSG_PETITION_SHOWLIST` (vmangos `Packets/Petition.cpp:3-6`): the NPC's guid. The
/// server also sends the list unasked for a `GOSSIP_OPTION_PETITIONER` row (`Player.cpp:12428`).
pub fn petition_show_list(npc: u64) -> Vec<u8> {
    npc.to_le_bytes().to_vec()
}

/// Body of `CMSG_PETITION_BUY` (`0x6d6b10`): the NPC, then a zeroed petition record packed by
/// `0x98cf70` with the name as its title and the vendor row's index as its `muid`.
pub fn petition_buy(npc: u64, name: &str, index: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(81 + name.len());
    body.extend_from_slice(&npc.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // id
    body.extend_from_slice(&0u64.to_le_bytes()); // owner
    push_cstring(&mut body, name);
    body.push(0); // body text
    body.extend_from_slice(&[0u8; 28]); // min, max, deadline, issue, guild, classes, races
    body.extend_from_slice(&0u16.to_le_bytes()); // genders
    body.extend_from_slice(&[0u8; 12]); // min level, max level, choice count
    body.extend_from_slice(&[0u8; PETITION_CHOICES]); // ten empty choices
    body.extend_from_slice(&index.to_le_bytes()); // muid
    body.extend_from_slice(&0u32.to_le_bytes()); // static type
    body
}

/// Body of `CMSG_PETITION_SHOW_SIGNATURES` (vmangos `Packets/Petition.cpp:8-11`): the charter
/// item's guid, unanswered if we are guilded or lack the item (`PetitionsHandler.cpp:140-143`).
pub fn petition_show_signatures(item: u64) -> Vec<u8> {
    item.to_le_bytes().to_vec()
}

/// Body of `CMSG_PETITION_SIGN` (vmangos `Packets/Petition.cpp:35-39`): the item guid, then an `i8`
/// the server skips. `SignPetition` defaults it to 1.
pub fn petition_sign(item: u64, arg: i8) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.extend_from_slice(&item.to_le_bytes());
    body.push(arg as u8);
    body
}

/// Body of `CMSG_OFFER_PETITION` (`0x5cf220`): a leading word, an arena charter's team size and
/// 0 for any other, then the item guid and the target's.
pub fn offer_petition(team_size: u32, item: u64, player: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(20);
    body.extend_from_slice(&team_size.to_le_bytes());
    body.extend_from_slice(&item.to_le_bytes());
    body.extend_from_slice(&player.to_le_bytes());
    body
}

/// Body of `CMSG_TURN_IN_PETITION` for a guild charter (`0x6dd3a0`): the charter item's guid.
pub fn turn_in_petition(item: u64) -> Vec<u8> {
    item.to_le_bytes().to_vec()
}

/// The banner an arena charter is turned in with (`TurnInArenaPetition`, `0x5a26d0`): three
/// `0xAARRGGBB` colours and two style indices; all `0xFFFFFFFF` when the call carried no colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaBanner {
    pub background_color: u32,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
}

/// Body of `CMSG_TURN_IN_PETITION` for an arena charter (`0x6dd680`): the item guid, then the
/// banner's five words.
pub fn turn_in_arena_petition(item: u64, banner: ArenaBanner) -> Vec<u8> {
    let mut body = item.to_le_bytes().to_vec();
    for v in [
        banner.background_color,
        banner.emblem_style,
        banner.emblem_color,
        banner.border_style,
        banner.border_color,
    ] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    body
}

/// Body of `CMSG_PETITION_QUERY` (vmangos `Packets/Petition.cpp:13-17`): `u32 petitionId`,
/// `u64 item`; the server looks up by id alone (`PetitionsHandler.cpp:171-185`).
pub fn petition_query(petition_id: u32, item: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&petition_id.to_le_bytes());
    body.extend_from_slice(&item.to_le_bytes());
    body
}

/// Body of `MSG_PETITION_RENAME` outbound (vmangos `Packets/Petition.cpp:29-33`): `u64` item,
/// cstring name. Not truncated to [`CHARTER_NAME_MAX_LENGTH`]; the server validates it.
pub fn petition_rename(item: u64, name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(9 + name.len());
    body.extend_from_slice(&item.to_le_bytes());
    push_cstring(&mut body, name);
    body
}

/// Body of `MSG_PETITION_DECLINE` outbound (vmangos `Packets/Petition.cpp:19-22`): the charter
/// item's guid; the server forwards our guid to the owner.
pub fn petition_decline(item: u64) -> Vec<u8> {
    item.to_le_bytes().to_vec()
}

fn push_cstring(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packed record: a body short of it leaves the server's reader mid-field.
    #[test]
    fn petition_buy_packs_a_whole_record() {
        let body = petition_buy(0x1234_5678_9abc_def0, "Test", 2);
        // 8 npc + 4 id + 8 owner + 5 name + 1 body + 28 + 2 + 12 + 10 choices + 4 muid + 4 type
        assert_eq!(body.len(), 86);
        assert_eq!(
            &body[body.len() - 8..],
            &[2, 0, 0, 0, 0, 0, 0, 0],
            "muid, then type 0"
        );
    }

    /// Ten choice strings follow whatever count the record carries, and `muid` precedes the type.
    #[test]
    fn the_query_response_reads_ten_choices_then_muid_then_type() {
        let mut b = 7u32.to_le_bytes().to_vec();
        b.extend_from_slice(&0x55u64.to_le_bytes());
        b.extend_from_slice(b"Probes\0\0");
        for v in [1u32, 1, 2, 0, 0, 0, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&0u16.to_le_bytes());
        for v in [0u32, 0, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0u8; 10]);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        let p = read_petition_query_response(&mut b.as_slice()).unwrap();
        assert_eq!(
            (p.petition_id, p.owner, p.name.as_str()),
            (7, 0x55, "Probes")
        );
        assert_eq!(
            (p.min_signatures, p.max_signatures, p.num_choices),
            (1, 1, 0)
        );
        assert_eq!(
            (p.choices.len(), p.muid, p.static_type),
            (10, 0, PETITION_TYPE_ARENA)
        );
    }

    #[test]
    fn the_offer_and_the_arena_turn_in() {
        let mut want = 2u32.to_le_bytes().to_vec();
        want.extend_from_slice(&0x99u64.to_le_bytes());
        want.extend_from_slice(&0x42u64.to_le_bytes());
        assert_eq!(offer_petition(2, 0x99, 0x42), want);
        let banner = ArenaBanner {
            background_color: 0xFF10_2030,
            emblem_style: 7,
            emblem_color: 1,
            border_style: 3,
            border_color: 2,
        };
        assert_eq!(turn_in_arena_petition(0x99, banner).len(), 28);
    }

    #[test]
    fn charter_constants_match_the_world_data() {
        assert_eq!(CHARTER_ITEM_ENTRY, 5863);
        assert_eq!(CHARTER_DISPLAY_ID, 16161);
        assert_eq!(ITEM_FLAG_CHARTER, 0x2000);
        assert_eq!(CHARTER_NAME_MAX_LENGTH, super::super::GUILD_NAME_MAX_LENGTH);
    }
}
