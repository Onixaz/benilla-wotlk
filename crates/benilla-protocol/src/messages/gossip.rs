//! Gossip and NPC text, the dialog a friendly NPC opens. The 3.3.5a shapes are the 12340
//! client's: its `SMSG_GOSSIP_MESSAGE` handler (`0x58b1b0`) and its select sender (`0x58af10`).

use std::io;

use crate::wire::{
    capacity_hint, read_cstring, read_f32_le, read_i32_le, read_u32_le, read_u64_le, read_u8,
};

/// The menu's option and quest slots: the reader clamps the option count to 32 (`0x58b200`) and
/// the quest table holds 32 records (`0xbfc968`, stride `0x214`).
pub const GOSSIP_MAX_ITEMS: usize = 32;

/// One `SMSG_GOSSIP_MESSAGE` option, the 24-byte slot at `0xc00bf0`. `index` is echoed on select;
/// `icon` is the `GetGossipOptions` type index; `coded` asks for a code first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GossipOption {
    pub index: u32,
    pub icon: u8,
    pub coded: bool,
    /// Copper the select costs: above the player's money the client refuses it (`0x58afc3`).
    pub box_money: u32,
    pub message: String,
    /// The confirmation the client asks before a select (`GOSSIP_CONFIRM`, `0x58af74`); `None`
    /// when the wire string is empty, as the reader stores no pointer then (`0x58b2ee`).
    pub box_text: Option<String>,
}

/// One quest in `SMSG_GOSSIP_MESSAGE`'s quest list, the `0x214`-byte record at `0xbfc968`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestOption {
    pub quest_id: u32,
    /// The row's dialog status: 3 and 4 list it as active, anything else as available (`0x58a5d0`).
    pub icon: u32,
    /// Signed: -1 marks a quest that scales to the player, never trivial (`0x58aba7`).
    pub level: i32,
    /// The template's quest flags; `GetGossipAvailableQuests` reads `0x1000` as daily.
    pub flags: u32,
    pub repeatable: bool,
    pub title: String,
}

/// One `SMSG_GOSSIP_MESSAGE` menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GossipMenu {
    pub npc: u64,
    /// Echoed first in every `CMSG_GOSSIP_SELECT_OPTION` (`0xc016f8`).
    pub menu_id: u32,
    /// The greeting for a follow-up [`npc_text_query`].
    pub text_id: u32,
    pub options: Vec<GossipOption>,
    pub quests: Vec<QuestOption>,
}

/// `CMSG_GOSSIP_HELLO` body: the NPC's full guid (`0x6d1e20`).
pub fn gossip_hello(npc_guid: u64) -> Vec<u8> {
    npc_guid.to_le_bytes().to_vec()
}

/// `CMSG_GOSSIP_SELECT_OPTION` body (`0x58afe3`..`0x58b030`): guid, the menu id, the option's
/// `index`, then the code only for a coded option.
pub fn gossip_select_option(
    npc_guid: u64,
    menu_id: u32,
    gossip_list_id: u32,
    code: Option<&str>,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&npc_guid.to_le_bytes());
    body.extend_from_slice(&menu_id.to_le_bytes());
    body.extend_from_slice(&gossip_list_id.to_le_bytes());
    if let Some(code) = code {
        body.extend_from_slice(code.as_bytes());
        body.push(0);
    }
    body
}

/// `CMSG_NPC_TEXT_QUERY` body: the text id, then the NPC's guid.
pub fn npc_text_query(text_id: u32, guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&text_id.to_le_bytes());
    body.extend_from_slice(&guid.to_le_bytes());
    body
}

/// Read `SMSG_GOSSIP_MESSAGE` as 12340 does (`0x58b1b0`): guid, menu id, text id, the options,
/// then the quests. Every listed item is read so the parse stays aligned, and only the first 32 of
/// each kind are kept: the client clamps the option count, and its quest table has 32 records.
pub(super) fn read_gossip_message(r: &mut &[u8]) -> io::Result<GossipMenu> {
    let npc = read_u64_le(r)?;
    let menu_id = read_u32_le(r)?;
    let text_id = read_u32_le(r)?;
    let option_count = read_u32_le(r)?;
    let mut options = Vec::with_capacity(capacity_hint(option_count, GOSSIP_MAX_ITEMS));
    for _ in 0..option_count {
        options.push(GossipOption {
            index: read_u32_le(r)?,
            icon: read_u8(r)?,
            coded: read_u8(r)? != 0,
            box_money: read_u32_le(r)?,
            message: read_cstring(r)?,
            box_text: Some(read_cstring(r)?).filter(|t| !t.is_empty()),
        });
    }
    options.truncate(GOSSIP_MAX_ITEMS);
    let quest_count = read_u32_le(r)?;
    let mut quests = Vec::with_capacity(capacity_hint(quest_count, GOSSIP_MAX_ITEMS));
    for _ in 0..quest_count {
        quests.push(QuestOption {
            quest_id: read_u32_le(r)?,
            icon: read_u32_le(r)?,
            level: read_i32_le(r)?,
            flags: read_u32_le(r)?,
            repeatable: read_u8(r)? != 0,
            title: read_cstring(r)?,
        });
    }
    quests.truncate(GOSSIP_MAX_ITEMS);
    Ok(GossipMenu {
        npc,
        menu_id,
        text_id,
        options,
        quests,
    })
}

/// One of the eight greeting variants of an `SMSG_NPC_TEXT_UPDATE` record, both gender columns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NpcTextBlock {
    /// The block's draw weight, not a rank; vmangos's fallback record sets all eight to 0.
    pub probability: f32,
    /// `text0`, shown when the NPC is male or genderless.
    pub male: String,
    /// `text1`, shown when the NPC is female.
    pub female: String,
}

impl NpcTextBlock {
    fn column(&self, female: bool) -> &str {
        if female {
            &self.female
        } else {
            &self.male
        }
    }
}

/// The greeting blocks in every `SMSG_NPC_TEXT_UPDATE` record.
pub const NPC_TEXT_BLOCKS: usize = 8;

/// Read `SMSG_NPC_TEXT_UPDATE` (vmangos `GossipDef.cpp:298-369`), all eight blocks undecided; the
/// greeting is picked when the frame opens, by [`select_greeting`].
///
/// The language is parsed and dropped: the 1.12 client garbles a block in a non-zero language
/// (`0x49b560`), which is not built, and no 1.12 record is known to set one.
pub(super) fn read_npc_text_update(r: &mut &[u8]) -> io::Result<(u32, Vec<NpcTextBlock>)> {
    let text_id = read_u32_le(r)?;
    let mut blocks = Vec::with_capacity(NPC_TEXT_BLOCKS);
    for _ in 0..NPC_TEXT_BLOCKS {
        let probability = read_f32_le(r)?;
        let male = read_cstring(r)?;
        let female = read_cstring(r)?;
        let _language_id = read_u32_le(r)?;
        for _ in 0..3 {
            let _emote_delay = read_u32_le(r)?;
            let _emote_id = read_u32_le(r)?;
        }
        blocks.push(NpcTextBlock {
            probability,
            male,
            female,
        });
    }
    Ok((text_id, blocks))
}

/// `SMSG_GOSSIP_POI`, the map flag a guard drops when asked for directions; the server sends it
/// unasked from a gossip option (`Player::OnGossipSelect`). The 1.12 client treats it as an
/// `AreaPOI.dbc` record in minimap blip slot 1 (`0x4e2840`), hence the DBC field names.
#[derive(Debug, Clone, PartialEq)]
pub struct GossipPoi {
    /// As `AreaPOI.dbc` `Flags`: bit 0 makes it a candidate, bit 1 draws the in-range icon; every
    /// vmangos row sends 99.
    pub flags: u32,
    /// World x and y; the wire carries no z.
    pub pos: [f32; 2],
    /// The `POIIcons.blp` cell (8×8 grid, drawn only below 64); every vmangos row uses 6, the red
    /// flag (`GossipDef.h:113`).
    pub icon: u32,
    /// vmangos `points_of_interest.data`, 0 in every row.
    pub data: u32,
    /// The destination's name, the marker's tooltip.
    pub name: String,
}

/// Read `SMSG_GOSSIP_POI`; 12340's handler (`0x58a870`) reads the same shape, the name up to 64
/// bytes.
pub(super) fn read_gossip_poi(r: &mut &[u8]) -> io::Result<GossipPoi> {
    let flags = read_u32_le(r)?;
    let x = read_f32_le(r)?;
    let y = read_f32_le(r)?;
    Ok(GossipPoi {
        flags,
        pos: [x, y],
        icon: read_u32_le(r)?,
        data: read_u32_le(r)?,
        name: read_cstring(r)?,
    })
}

/// Pick the greeting as the client does (`0x58ac30`): a weighted draw over the blocks with
/// text in the NPC's gender column (female only at 1), never falling back to the other column.
/// `roll` is uniform in `[1.0, 2.0)`, `<=` makes an all-zero record pick block 0, and the sums
/// are `f64` as on the client's x87 stack. `None` is its "Missing gossip text!".
pub fn select_greeting(blocks: &[NpcTextBlock], npc_gender: u8, roll: f32) -> Option<&str> {
    let female = npc_gender == 1;
    let drawn = || blocks.iter().filter(|b| !b.column(female).is_empty());
    let sum: f64 = drawn().map(|b| b.probability as f64).sum();
    let threshold = (2.0 - roll as f64) * sum;
    let mut acc = 0.0f64;
    for block in drawn() {
        acc += block.probability as f64;
        if threshold <= acc {
            return Some(block.column(female));
        }
    }
    None
}
