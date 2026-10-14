//! The questgiver family (opcodes 386-402 and 0x418): the NPC accept, progress and reward panels,
//! which carry their own text. 12340 routes every SMSG here through `0x6d7f10`; each reader below
//! names the arm whose read order it follows.

use std::io;

use crate::wire::{
    capacity_hint, read_cstring, read_f32_le, read_i32_le, read_u32_le, read_u64_le, read_u8,
};

/// The emote pairs DETAILS writes, a counted run on the wire.
pub const QUEST_EMOTE_COUNT: u32 = 4;
/// The reward-reputation slots the details, reward and template bodies carry, three arrays each.
pub const QUEST_REPUTATIONS_COUNT: usize = 5;

/// An NPC's `SMSG_QUESTGIVER_STATUS`, which drives the `!`/`?` over its head. 12340 maps the 11
/// values to a marker row through `0xa34f5c` and draws 2-4 only under low-level quest tracking
/// (`0x745dd7`, `0x57bf30`).
pub mod dialog_status {
    pub const NONE: u32 = 0;
    /// Grey `!`: the quest needs a higher level.
    pub const UNAVAILABLE: u32 = 1;
    pub const LOW_LEVEL_AVAILABLE: u32 = 2;
    pub const LOW_LEVEL_REWARD_REP: u32 = 3;
    pub const LOW_LEVEL_AVAILABLE_REP: u32 = 4;
    /// Grey `?`: a quest in the log, not yet completable.
    pub const INCOMPLETE: u32 = 5;
    /// Blue `?`: a repeatable quest ready to turn in.
    pub const REWARD_REP: u32 = 6;
    /// Blue `!`: a repeatable quest on offer.
    pub const AVAILABLE_REP: u32 = 7;
    /// Gold `!`.
    pub const AVAILABLE: u32 = 8;
    /// Gold `?` without the minimap dot.
    pub const REWARD2: u32 = 9;
    /// Gold `?`.
    pub const REWARD: u32 = 10;
}

/// One reward or choice item on a questgiver panel. `display_id` is an `ItemDisplayInfo.dbc` row;
/// an all-zero triple means the server had no item template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuestRewardItem {
    pub item_id: u32,
    pub count: u32,
    pub display_id: u32,
}

/// A required item on the progress panel, the same triple.
pub type QuestRequiredItem = QuestRewardItem;

/// The reward-reputation arrays: faction ids, value-table ids, and raw overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuestReputationRewards {
    pub factions: [u32; QUEST_REPUTATIONS_COUNT],
    pub value_ids: [i32; QUEST_REPUTATIONS_COUNT],
    pub overrides: [i32; QUEST_REPUTATIONS_COUNT],
}

/// The rewards past the item blocks that 3.3.5a's DETAILS and OFFER_REWARD both carry.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct QuestExtraRewards {
    /// Signed, negative for a quest that costs money.
    pub money: i32,
    pub xp: u32,
    /// Ten times the honor `GetRewardHonor` shows (`0x58c110` divides by 10).
    pub honor: u32,
    pub honor_multiplier: f32,
    pub spell: u32,
    /// The spell cast on the player at turn-in, `-1`-style signed in the server's template.
    pub spell_cast: i32,
    pub title: u32,
    pub talents: u32,
    pub arena_points: u32,
    /// Bit `i` keeps reputation reward `i` from spilling to its parent and children
    /// (`0x58d050`'s `0xacf160[i] & mask` test).
    pub rep_no_spillover: u32,
    pub reputation: QuestReputationRewards,
}

/// One greeting-panel quest row (`0x6d0240`). `icon` 3 and 4 list it as active, the rest as
/// available (`0x6d039a`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestListEntry {
    pub quest_id: u32,
    pub icon: u32,
    pub level: u32,
    pub flags: u32,
    pub repeatable: bool,
    pub title: String,
}

/// `SMSG_QUESTGIVER_QUEST_LIST`: the multi-quest greeting panel.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestGiverList {
    pub npc: u64,
    pub greeting: String,
    pub emote_delay: u32,
    pub emote: u32,
    pub quests: Vec<QuestListEntry>,
}

/// `SMSG_QUESTGIVER_QUEST_DETAILS`: the accept panel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuestDetails {
    pub npc: u64,
    /// The player who shared the quest, else 0. Accept answers it when set (`0x58cd65`).
    pub sharer: u64,
    pub quest_id: u32,
    pub title: String,
    pub details: String,
    pub objectives: String,
    /// `CGQuestInfo::m_autoLaunched`: the publisher stores it in the decline latch (`0x58de4f`),
    /// so Decline closes the window instead of going back to the greeting.
    pub auto_launched: bool,
    pub flags: u32,
    pub suggested_players: u32,
    /// Echoed back as `CMSG_QUESTGIVER_ACCEPT_QUEST`'s last field (`0x58c69e` → `0x58cd7d`).
    pub start_cheat: u8,
    pub choices: Vec<QuestRewardItem>,
    pub rewards: Vec<QuestRewardItem>,
    pub extra: QuestExtraRewards,
}

/// `SMSG_QUESTGIVER_OFFER_REWARD`: the reward panel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuestOfferReward {
    pub npc: u64,
    pub quest_id: u32,
    pub title: String,
    pub offer_text: String,
    pub auto_finish: bool,
    pub quest_flags: u32,
    pub suggested_players: u32,
    pub choices: Vec<QuestRewardItem>,
    pub rewards: Vec<QuestRewardItem>,
    pub extra: QuestExtraRewards,
}

/// `SMSG_QUESTGIVER_REQUEST_ITEMS`: the progress panel. Continue is enabled only when all four
/// trailing flag words are non-zero (`0x6d0a1d`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestRequestItems {
    pub npc: u64,
    pub quest_id: u32,
    pub title: String,
    pub request_text: String,
    pub emote: u32,
    pub close_on_cancel: u32,
    pub quest_flags: u32,
    pub suggested_players: u32,
    pub required_money: u32,
    pub required_items: Vec<QuestRequiredItem>,
    pub is_complete: bool,
}

/// `SMSG_QUESTGIVER_QUEST_COMPLETE` (`0x6d1110`): what the turn-in granted. 3.3.5a sends no item
/// list here; the items arrive as item pushes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestComplete {
    pub quest_id: u32,
    pub xp: u32,
    pub money: u32,
    pub honor: u32,
    pub talents: u32,
    pub arena_points: u32,
}

// ── CMSG encoders (12340's builders at 0x6d4c10-0x6d4ee6) ────────────────────────────────────────

/// Body of `CMSG_QUESTGIVER_STATUS_QUERY`: the `u64` NPC guid.
pub fn questgiver_status_query(npc: u64) -> Vec<u8> {
    npc.to_le_bytes().to_vec()
}

/// Body of `CMSG_QUESTGIVER_HELLO`: the NPC guid.
pub fn questgiver_hello(npc: u64) -> Vec<u8> {
    npc.to_le_bytes().to_vec()
}

/// Body of `CMSG_QUESTGIVER_QUERY_QUEST` (`0x6d4c10`): `u64 guid, u32 quest, u8 1`, sent by a
/// quest row's click and answered with the details panel.
pub fn questgiver_query_quest(npc: u64, quest: u32) -> Vec<u8> {
    let mut body = guid_quest(npc, quest);
    body.push(1);
    body
}

/// Body of `CMSG_QUESTGIVER_ACCEPT_QUEST` (`0x6d4ca0`): `u64 guid, u32 quest, u32 start_cheat`,
/// the last the details panel's [`QuestDetails::start_cheat`].
pub fn questgiver_accept_quest(npc: u64, quest: u32, start_cheat: u32) -> Vec<u8> {
    let mut body = guid_quest(npc, quest);
    body.extend_from_slice(&start_cheat.to_le_bytes());
    body
}

/// Body of `CMSG_QUESTGIVER_COMPLETE_QUEST` (`0x6d4d40`): `u64 guid, u32 quest`, answered with the
/// progress panel.
pub fn questgiver_complete_quest(npc: u64, quest: u32) -> Vec<u8> {
    guid_quest(npc, quest)
}

/// Body of `CMSG_QUESTGIVER_REQUEST_REWARD` (`0x6d4dd0`): `u64 guid, u32 quest`, the progress
/// panel's Continue, answered with the reward panel.
pub fn questgiver_request_reward(npc: u64, quest: u32) -> Vec<u8> {
    guid_quest(npc, quest)
}

/// Body of `CMSG_QUESTGIVER_CHOOSE_REWARD` (`0x6d4e60`): `u64 guid, u32 quest, u32 reward`,
/// `reward` a 0-based choice index (0 with no choice). Answered with
/// `SMSG_QUESTGIVER_QUEST_COMPLETE`.
pub fn questgiver_choose_reward(npc: u64, quest: u32, reward: u32) -> Vec<u8> {
    let mut body = guid_quest(npc, quest);
    body.extend_from_slice(&reward.to_le_bytes());
    body
}

fn guid_quest(npc: u64, quest: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&npc.to_le_bytes());
    body.extend_from_slice(&quest.to_le_bytes());
    body
}

// ── SMSG parsers ─────────────────────────────────────────────────────────────────────────────────

fn read_item_triple(r: &mut &[u8]) -> io::Result<QuestRewardItem> {
    Ok(QuestRewardItem {
        item_id: read_u32_le(r)?,
        count: read_u32_le(r)?,
        display_id: read_u32_le(r)?,
    })
}

fn read_item_block(r: &mut &[u8]) -> io::Result<Vec<QuestRewardItem>> {
    let count = read_u32_le(r)?;
    // Six choice slots, the wider of the two lists.
    let mut items = Vec::with_capacity(capacity_hint(count, 6));
    for _ in 0..count {
        items.push(read_item_triple(r)?);
    }
    Ok(items)
}

/// The three reputation arrays, five `u32` each.
pub(super) fn read_reputation(r: &mut &[u8]) -> io::Result<QuestReputationRewards> {
    let mut rep = QuestReputationRewards::default();
    for f in rep.factions.iter_mut() {
        *f = read_u32_le(r)?;
    }
    for v in rep.value_ids.iter_mut() {
        *v = read_i32_le(r)?;
    }
    for o in rep.overrides.iter_mut() {
        *o = read_i32_le(r)?;
    }
    Ok(rep)
}

/// Read `SMSG_QUESTGIVER_STATUS` (`0x6d11c0`): `u64 npcGuid, u8 status`.
pub(in crate::messages) fn read_questgiver_status(r: &mut &[u8]) -> io::Result<(u64, u32)> {
    Ok((read_u64_le(r)?, u32::from(read_u8(r)?)))
}

/// Read `SMSG_QUESTGIVER_STATUS_MULTIPLE` (`0x6d1230`): `u32 count`, then `{u64 guid, u8 status}`
/// per NPC, the answer to the login and zone-change sweep.
pub(in crate::messages) fn read_questgiver_status_multiple(
    r: &mut &[u8],
) -> io::Result<Vec<(u64, u32)>> {
    let count = read_u32_le(r)?;
    let mut out = Vec::with_capacity(capacity_hint(count, 64));
    for _ in 0..count {
        out.push(read_questgiver_status(r)?);
    }
    Ok(out)
}

/// Read `SMSG_QUESTGIVER_QUEST_LIST` (`0x6d0240`): `u64 npc`, greeting, `u32 emoteDelay, u32 emote,
/// u8 count`, then per quest `u32 id, u32 icon, u32 level, u32 flags, u8 repeatable`, title.
pub(in crate::messages) fn read_questgiver_quest_list(r: &mut &[u8]) -> io::Result<QuestGiverList> {
    let npc = read_u64_le(r)?;
    let greeting = read_cstring(r)?;
    let emote_delay = read_u32_le(r)?;
    let emote = read_u32_le(r)?;
    let count = read_u8(r)?;
    // The client's row arrays hold 32 (`0xc9d550`, `0x80` bytes each).
    let mut quests = Vec::with_capacity(capacity_hint(count, 32));
    for _ in 0..count {
        quests.push(QuestListEntry {
            quest_id: read_u32_le(r)?,
            icon: read_u32_le(r)?,
            level: read_u32_le(r)?,
            flags: read_u32_le(r)?,
            repeatable: read_u8(r)? != 0,
            title: read_cstring(r)?,
        });
    }
    Ok(QuestGiverList {
        npc,
        greeting,
        emote_delay,
        emote,
        quests,
    })
}

/// The money-to-reputation run DETAILS and OFFER_REWARD share, minus OFFER_REWARD's extra word
/// after the honor multiplier (`skip_after_honor`).
fn read_extra_rewards(r: &mut &[u8], skip_after_honor: bool) -> io::Result<QuestExtraRewards> {
    let money = read_i32_le(r)?;
    let xp = read_u32_le(r)?;
    let honor = read_u32_le(r)?;
    let honor_multiplier = read_f32_le(r)?;
    if skip_after_honor {
        let _unknown = read_u32_le(r)?;
    }
    let spell = read_u32_le(r)?;
    let spell_cast = read_i32_le(r)?;
    let title = read_u32_le(r)?;
    let talents = read_u32_le(r)?;
    let arena_points = read_u32_le(r)?;
    let rep_no_spillover = read_u32_le(r)?;
    Ok(QuestExtraRewards {
        money,
        xp,
        honor,
        honor_multiplier,
        spell,
        spell_cast,
        title,
        talents,
        arena_points,
        rep_no_spillover,
        reputation: read_reputation(r)?,
    })
}

/// Read `SMSG_QUESTGIVER_QUEST_DETAILS` (`0x6d0460`): `u64 npc, u64 sharer, u32 quest`, title,
/// details, objectives, `u8 autoLaunched, u32 flags, u32 suggestedPlayers, u8 startCheat`, the
/// choice and reward blocks, the extra rewards, then a counted run of `{u32 emote, u32 delay}`.
pub(in crate::messages) fn read_questgiver_quest_details(
    r: &mut &[u8],
) -> io::Result<QuestDetails> {
    let npc = read_u64_le(r)?;
    let sharer = read_u64_le(r)?;
    let quest_id = read_u32_le(r)?;
    let title = read_cstring(r)?;
    let details = read_cstring(r)?;
    let objectives = read_cstring(r)?;
    let auto_launched = read_u8(r)? != 0;
    let flags = read_u32_le(r)?;
    let suggested_players = read_u32_le(r)?;
    let start_cheat = read_u8(r)?;
    let choices = read_item_block(r)?;
    let rewards = read_item_block(r)?;
    let extra = read_extra_rewards(r, false)?;
    let emote_count = read_u32_le(r)?;
    for _ in 0..emote_count {
        let _emote = read_u32_le(r)?;
        let _delay = read_u32_le(r)?;
    }
    Ok(QuestDetails {
        npc,
        sharer,
        quest_id,
        title,
        details,
        objectives,
        auto_launched,
        flags,
        suggested_players,
        start_cheat,
        choices,
        rewards,
        extra,
    })
}

/// Read `SMSG_QUESTGIVER_OFFER_REWARD` (`0x6d0ab0`): npc, id, title, text, `u8 autoFinish,
/// u32 flags, u32 suggestedPlayers`, a counted run of `{u32 delay, u32 emote}`, the choice and
/// reward blocks, then the extra rewards with one more word after the honor multiplier.
pub(in crate::messages) fn read_questgiver_offer_reward(
    r: &mut &[u8],
) -> io::Result<QuestOfferReward> {
    let npc = read_u64_le(r)?;
    let quest_id = read_u32_le(r)?;
    let title = read_cstring(r)?;
    let offer_text = read_cstring(r)?;
    let auto_finish = read_u8(r)? != 0;
    let quest_flags = read_u32_le(r)?;
    let suggested_players = read_u32_le(r)?;
    let emote_count = read_u32_le(r)?;
    for _ in 0..emote_count {
        let _delay = read_u32_le(r)?;
        let _emote = read_u32_le(r)?;
    }
    let choices = read_item_block(r)?;
    let rewards = read_item_block(r)?;
    let extra = read_extra_rewards(r, true)?;
    Ok(QuestOfferReward {
        npc,
        quest_id,
        title,
        offer_text,
        auto_finish,
        quest_flags,
        suggested_players,
        choices,
        rewards,
        extra,
    })
}

/// Read `SMSG_QUESTGIVER_REQUEST_ITEMS` (`0x6d08f0`): npc, id, title, text, `u32` emote delay,
/// emote, close-on-cancel, flags, suggested players and money, the required items, then four flag
/// words that must all be non-zero for Continue.
pub(in crate::messages) fn read_questgiver_request_items(
    r: &mut &[u8],
) -> io::Result<QuestRequestItems> {
    let npc = read_u64_le(r)?;
    let quest_id = read_u32_le(r)?;
    let title = read_cstring(r)?;
    let request_text = read_cstring(r)?;
    let _emote_delay = read_u32_le(r)?;
    let emote = read_u32_le(r)?;
    let close_on_cancel = read_u32_le(r)?;
    let quest_flags = read_u32_le(r)?;
    let suggested_players = read_u32_le(r)?;
    let required_money = read_u32_le(r)?;
    let required_items = read_item_block(r)?;
    let mut is_complete = true;
    for _ in 0..4 {
        is_complete &= read_u32_le(r)? != 0;
    }
    Ok(QuestRequestItems {
        npc,
        quest_id,
        title,
        request_text,
        emote,
        close_on_cancel,
        quest_flags,
        suggested_players,
        required_money,
        required_items,
        is_complete,
    })
}

/// Read `SMSG_QUESTGIVER_QUEST_COMPLETE` (`0x6d1110`): six `u32`s, quest, xp, money, honor,
/// talents, arena points.
pub(in crate::messages) fn read_questgiver_quest_complete(
    r: &mut &[u8],
) -> io::Result<QuestComplete> {
    Ok(QuestComplete {
        quest_id: read_u32_le(r)?,
        xp: read_u32_le(r)?,
        money: read_u32_le(r)?,
        honor: read_u32_le(r)?,
        talents: read_u32_le(r)?,
        arena_points: read_u32_le(r)?,
    })
}

/// Read `SMSG_QUESTGIVER_QUEST_INVALID` (`0x6cd7c0`): one `u32` message code.
pub(in crate::messages) fn read_questgiver_quest_invalid(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_QUESTGIVER_QUEST_FAILED` (`0x6cda70`): `u32 questId, u32 reason`.
pub(in crate::messages) fn read_questgiver_quest_failed(r: &mut &[u8]) -> io::Result<(u32, u32)> {
    Ok((read_u32_le(r)?, read_u32_le(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Byte-buffer builders ─────────────────────────────────────────────────────────────────────
    fn push_u32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_i32(b: &mut Vec<u8>, v: i32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_u64(b: &mut Vec<u8>, v: u64) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_cstr(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(s.as_bytes());
        b.push(0);
    }
    fn push_triple(b: &mut Vec<u8>, id: u32, count: u32, disp: u32) {
        push_u32(b, id);
        push_u32(b, count);
        push_u32(b, disp);
    }
    /// Money through the reputation arrays, DETAILS' order, plus OFFER_REWARD's extra word.
    fn push_extra(b: &mut Vec<u8>, offer: bool) {
        push_i32(b, 1234); // money
        push_u32(b, 450); // xp
        push_u32(b, 70); // honor x10
        b.extend_from_slice(&0.5f32.to_le_bytes());
        if offer {
            push_u32(b, 8);
        }
        push_u32(b, 133); // spell
        push_i32(b, -1); // spell cast
        push_u32(b, 3); // title
        push_u32(b, 1); // talents
        push_u32(b, 25); // arena
        push_u32(b, 0b10); // no spillover for reward 1
        for i in 0..5 {
            push_u32(b, 72 + i);
        }
        for i in 0..5 {
            push_i32(b, i);
        }
        for i in 0..5 {
            push_i32(b, -i);
        }
    }
    fn assert_extra(e: &QuestExtraRewards) {
        assert_eq!(e.money, 1234);
        assert_eq!(e.xp, 450);
        assert_eq!(e.honor, 70);
        assert_eq!(e.honor_multiplier, 0.5);
        assert_eq!(e.spell, 133);
        assert_eq!(e.spell_cast, -1);
        assert_eq!(e.title, 3);
        assert_eq!(e.talents, 1);
        assert_eq!(e.arena_points, 25);
        assert_eq!(e.rep_no_spillover, 0b10);
        assert_eq!(e.reputation.factions, [72, 73, 74, 75, 76]);
        assert_eq!(e.reputation.value_ids, [0, 1, 2, 3, 4]);
        assert_eq!(e.reputation.overrides, [0, -1, -2, -3, -4]);
    }

    // ── CMSG encode goldens ──────────────────────────────────────────────────────────────────────

    #[test]
    fn cmsg_guid_only_bodies() {
        assert_eq!(
            questgiver_status_query(0x0102_0304_0506_0708),
            vec![0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
        );
        assert_eq!(questgiver_hello(0xAABB), vec![0xBB, 0xAA, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn cmsg_guid_quest_bodies() {
        let mut want = Vec::new();
        push_u64(&mut want, 0x42);
        push_u32(&mut want, 3);
        assert_eq!(questgiver_complete_quest(0x42, 3), want);
        assert_eq!(questgiver_request_reward(0x42, 3), want);
    }

    #[test]
    fn cmsg_query_quest_ends_in_a_one_byte() {
        let mut want = Vec::new();
        push_u64(&mut want, 0x42);
        push_u32(&mut want, 3);
        want.push(1);
        assert_eq!(questgiver_query_quest(0x42, 3), want);
    }

    #[test]
    fn cmsg_accept_quest_echoes_the_start_cheat() {
        let mut want = Vec::new();
        push_u64(&mut want, 0x42);
        push_u32(&mut want, 3);
        push_u32(&mut want, 1);
        assert_eq!(questgiver_accept_quest(0x42, 3, 1), want);
    }

    #[test]
    fn cmsg_choose_reward_body() {
        let mut want = Vec::new();
        push_u64(&mut want, 0x99);
        push_u32(&mut want, 40);
        push_u32(&mut want, 2);
        assert_eq!(questgiver_choose_reward(0x99, 40, 2), want);
    }

    // ── SMSG parse fixtures ──────────────────────────────────────────────────────────────────────

    #[test]
    fn status_is_a_guid_and_one_byte() {
        // The nine bytes AzerothCore sends (#147): a creature guid and status 0.
        let b = [0xdb, 0x03, 0x00, 0xc4, 0x00, 0x00, 0x30, 0xf1, 0x00];
        let mut r = &b[..];
        let (npc, status) = read_questgiver_status(&mut r).unwrap();
        assert_eq!(npc, 0xF130_0000_C400_03DB);
        assert_eq!(status, dialog_status::NONE);
        assert!(r.is_empty());
    }

    #[test]
    fn status_multiple_reads_a_counted_run() {
        let mut b = Vec::new();
        push_u32(&mut b, 2);
        push_u64(&mut b, 0xF130_0000_0000_0042);
        b.push(dialog_status::AVAILABLE as u8);
        push_u64(&mut b, 0xF130_0000_0000_0043);
        b.push(dialog_status::REWARD as u8);
        let mut r = b.as_slice();
        let all = read_questgiver_status_multiple(&mut r).unwrap();
        assert_eq!(
            all,
            vec![
                (0xF130_0000_0000_0042, dialog_status::AVAILABLE),
                (0xF130_0000_0000_0043, dialog_status::REWARD)
            ]
        );
        assert!(r.is_empty());
    }

    #[test]
    fn quest_list_rows_carry_flags_and_repeatable() {
        let mut b = Vec::new();
        push_u64(&mut b, 0xABCD);
        push_cstr(&mut b, "Greetings, hero.");
        push_u32(&mut b, 0); // emoteDelay
        push_u32(&mut b, 1); // emote
        b.push(2); // count (u8)
        push_u32(&mut b, 100);
        push_u32(&mut b, 2); // icon
        push_u32(&mut b, 3); // level
        push_u32(&mut b, 0x8); // flags
        b.push(0); // repeatable
        push_cstr(&mut b, "A Threat Within");
        push_u32(&mut b, 200);
        push_u32(&mut b, 4);
        push_u32(&mut b, 5);
        push_u32(&mut b, 0);
        b.push(1);
        push_cstr(&mut b, "Report to Goldshire");
        let mut r = b.as_slice();
        let list = read_questgiver_quest_list(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(list.greeting, "Greetings, hero.");
        assert_eq!(list.quests.len(), 2);
        assert_eq!(list.quests[0].flags, 0x8);
        assert!(!list.quests[0].repeatable);
        assert_eq!(list.quests[0].title, "A Threat Within");
        assert_eq!(list.quests[1].icon, 4);
        assert!(list.quests[1].repeatable);
        assert_eq!(list.quests[1].title, "Report to Goldshire");
    }

    #[test]
    fn details_reads_12340_s_order() {
        let mut b = Vec::new();
        push_u64(&mut b, 0x42); // npc
        push_u64(&mut b, 0x77); // sharer
        push_u32(&mut b, 100);
        push_cstr(&mut b, "A Threat Within");
        push_cstr(&mut b, "Kill the kobolds.");
        push_cstr(&mut b, "Slay 10 kobolds.");
        b.push(1); // autoLaunched
        push_u32(&mut b, 0x8); // flags
        push_u32(&mut b, 3); // suggested players
        b.push(1); // start cheat
        push_u32(&mut b, 1);
        push_triple(&mut b, 2000, 1, 555);
        push_u32(&mut b, 1);
        push_triple(&mut b, 3000, 2, 777);
        push_extra(&mut b, false);
        push_u32(&mut b, QUEST_EMOTE_COUNT);
        for i in 0..QUEST_EMOTE_COUNT {
            push_u32(&mut b, 10 + i);
            push_u32(&mut b, 100 + i);
        }
        let mut r = b.as_slice();
        let d = read_questgiver_quest_details(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(d.npc, 0x42);
        assert_eq!(d.sharer, 0x77);
        assert_eq!(d.quest_id, 100);
        assert_eq!(d.objectives, "Slay 10 kobolds.");
        assert!(d.auto_launched);
        assert_eq!(d.flags, 0x8);
        assert_eq!(d.suggested_players, 3);
        assert_eq!(d.start_cheat, 1);
        assert_eq!(
            d.choices,
            vec![QuestRewardItem {
                item_id: 2000,
                count: 1,
                display_id: 555
            }]
        );
        assert_eq!(d.rewards[0].item_id, 3000);
        assert_extra(&d.extra);
    }

    #[test]
    fn details_with_hidden_rewards_reads_empty_blocks() {
        let mut b = Vec::new();
        push_u64(&mut b, 0x42);
        push_u64(&mut b, 0);
        push_u32(&mut b, 100);
        push_cstr(&mut b, "T");
        push_cstr(&mut b, "D");
        push_cstr(&mut b, "O");
        b.push(0);
        push_u32(&mut b, 0x200);
        push_u32(&mut b, 0);
        b.push(0);
        push_u32(&mut b, 0); // choice count
        push_u32(&mut b, 0); // reward count
        push_extra(&mut b, false);
        push_u32(&mut b, 0); // no emotes
        let mut r = b.as_slice();
        let d = read_questgiver_quest_details(&mut r).unwrap();
        assert!(r.is_empty());
        assert!(d.choices.is_empty());
        assert!(d.rewards.is_empty());
    }

    #[test]
    fn offer_reward_reads_12340_s_order() {
        let mut b = Vec::new();
        push_u64(&mut b, 0x42);
        push_u32(&mut b, 100);
        push_cstr(&mut b, "Well done.");
        push_cstr(&mut b, "Here is your reward.");
        b.push(1); // autoFinish
        push_u32(&mut b, 0x200); // flags
        push_u32(&mut b, 0); // suggested players
        push_u32(&mut b, 2); // emoteCount
        push_u32(&mut b, 500);
        push_u32(&mut b, 1);
        push_u32(&mut b, 0);
        push_u32(&mut b, 2);
        push_u32(&mut b, 0); // choiceCount
        push_u32(&mut b, 1); // rewCount
        push_triple(&mut b, 3000, 1, 888);
        push_extra(&mut b, true);
        let mut r = b.as_slice();
        let o = read_questgiver_offer_reward(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(o.offer_text, "Here is your reward.");
        assert!(o.auto_finish);
        assert_eq!(o.quest_flags, 0x200);
        assert!(o.choices.is_empty());
        assert_eq!(o.rewards[0].display_id, 888);
        assert_extra(&o.extra);
    }

    #[test]
    fn request_items_needs_all_four_flag_words() {
        let mut b = Vec::new();
        push_u64(&mut b, 0x42);
        push_u32(&mut b, 100);
        push_cstr(&mut b, "A Threat Within");
        push_cstr(&mut b, "Bring me the tusks.");
        push_u32(&mut b, 0); // emoteDelay
        push_u32(&mut b, 1); // emote
        push_u32(&mut b, 1); // closeOnCancel
        push_u32(&mut b, 0x8); // flags
        push_u32(&mut b, 0); // suggested players
        push_u32(&mut b, 0); // requiredMoney
        push_u32(&mut b, 1);
        push_triple(&mut b, 2001, 8, 111);
        for w in [0x03, 0x04, 0x08, 0x10] {
            push_u32(&mut b, w);
        }
        let mut r = b.as_slice();
        let ri = read_questgiver_request_items(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(ri.request_text, "Bring me the tusks.");
        assert_eq!(ri.close_on_cancel, 1);
        assert_eq!(ri.quest_flags, 0x8);
        assert_eq!(ri.required_items[0].count, 8);
        assert!(ri.is_complete);

        // AzerothCore writes 0 in the first word for a quest that cannot complete yet.
        let mut b2 = b.clone();
        let off = b2.len() - 16;
        b2[off..off + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(
            !read_questgiver_request_items(&mut b2.as_slice())
                .unwrap()
                .is_complete
        );
    }

    #[test]
    fn quest_complete_is_six_words() {
        let mut b = Vec::new();
        for v in [100, 450, 1234, 70, 1, 25] {
            push_u32(&mut b, v);
        }
        let mut r = b.as_slice();
        let c = read_questgiver_quest_complete(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(
            c,
            QuestComplete {
                quest_id: 100,
                xp: 450,
                money: 1234,
                honor: 70,
                talents: 1,
                arena_points: 25,
            }
        );
    }

    #[test]
    fn quest_invalid_and_failed_parse() {
        let mut b = Vec::new();
        push_u32(&mut b, 7);
        assert_eq!(read_questgiver_quest_invalid(&mut b.as_slice()).unwrap(), 7);

        let mut b2 = Vec::new();
        push_u32(&mut b2, 100);
        push_u32(&mut b2, 4);
        assert_eq!(
            read_questgiver_quest_failed(&mut b2.as_slice()).unwrap(),
            (100, 4)
        );
    }
}
