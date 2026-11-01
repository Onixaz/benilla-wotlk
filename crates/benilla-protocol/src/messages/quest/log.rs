//! Quest-log wire: the quest query and response (92/93), log swap, remove and full (403-405), and
//! the `SMSG_QUESTUPDATE_*` progress pushes (406-410), read as 12340 reads them.

use std::io;

use super::giver::{read_reputation, QuestReputationRewards};
use crate::wire::{read_cstring, read_f32_le, read_i32_le, read_u32_le, read_u64_le};

/// Objective rows the response always writes, the unused tail zero-filled.
pub const QUEST_OBJECTIVES_COUNT: u32 = 4;
/// Required-item `{itemId, count}` slots the response always writes.
pub const QUEST_REQUIRED_ITEMS_COUNT: u32 = 6;
/// `{itemId, count}` reward slots the response always writes.
pub const QUEST_REWARDS_COUNT: u32 = 4;
/// `{itemId, count}` reward-choice slots the response always writes.
pub const QUEST_REWARD_CHOICES_COUNT: u32 = 6;

/// One kill, cast or interact objective plus its text, which the packet sends after every row.
/// `creature_or_go` stays raw: a creature entry, or `(-id)|0x80000000` for a gameobject (the
/// leaderboard tests the sign bit, `0x5e19e8`). `item_drop` is the intermediate item the
/// objective drops, which the leaderboard lists while the quest is incomplete.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestObjective {
    pub creature_or_go: u32,
    pub required_count: u32,
    pub item_drop: u32,
    pub item_drop_count: u32,
    pub text: String,
}

/// `QuestFlags`: the template's flags word, which 3.3.5a's giver panels also carry.
pub mod quest_flags {
    pub const STAY_ALIVE: u32 = 0x0000_0001;
    /// Escort: one member accepting sends every other eligible one `SMSG_QUEST_CONFIRM_ACCEPT`.
    pub const PARTY_ACCEPT: u32 = 0x0000_0002;
    pub const EXPLORATION: u32 = 0x0000_0004;
    /// Shareable. The client gates Share Quest on it.
    pub const SHARABLE: u32 = 0x0000_0008;
    pub const EPIC: u32 = 0x0000_0020;
    pub const RAID: u32 = 0x0000_0040;
    /// Rewards ship only in `SMSG_QUESTGIVER_OFFER_REWARD`; DETAILS and this template zero them.
    pub const HIDDEN_REWARDS: u32 = 0x0000_0200;
    /// Granted and completed in one step; never appears in the client's quest log.
    pub const AUTO_REWARDED: u32 = 0x0000_0400;
    pub const DAILY: u32 = 0x0000_1000;
    /// `QuestFlagsPVP` (`0x58c3ec`): the quest flags the player for PvP.
    pub const FLAGS_PVP: u32 = 0x0000_2000;
    pub const WEEKLY: u32 = 0x0000_8000;
    /// `QuestGetAutoAccept` (`0x58c433`).
    pub const AUTO_ACCEPT: u32 = 0x0008_0000;
}

/// `SMSG_QUEST_QUERY_RESPONSE`: the quest template behind the quest log's detail view, read by
/// `0x7f70e0` into the record the leaderboard (`0x5e1460`) walks. Its arrays are fixed-count.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuestTemplate {
    pub quest_id: u32,
    pub method: u32,
    pub level: u32,
    pub min_level: u32,
    pub zone_or_sort: i32,
    pub quest_type: u32,
    pub suggested_players: u32,
    pub rep_objective_faction: u32,
    pub rep_objective_value: u32,
    pub next_quest_in_chain: u32,
    pub xp_id: u32,
    pub money: i32,
    pub money_max_level: u32,
    pub reward_spell: u32,
    pub reward_spell_cast: i32,
    pub honor: u32,
    pub honor_multiplier: f32,
    /// Record `+0x4c`: the item the quest hands out, which the leaderboard skips as a requirement
    /// when one is enough (`0x5e16d9`).
    pub src_item_id: u32,
    pub flags: u32,
    pub reward_title: u32,
    /// Record `+0x2098`: the player kills the last leaderboard line counts.
    pub players_slain: u32,
    pub reward_talents: u32,
    pub reward_arena_points: u32,
    /// Record `+0x28e0`: bit `i` keeps reputation reward `i` from spilling (`0x5df8f3`).
    pub rep_no_spillover: u32,
    pub rewards: [(u32, u32); QUEST_REWARDS_COUNT as usize],
    pub choices: [(u32, u32); QUEST_REWARD_CHOICES_COUNT as usize],
    pub reputation: QuestReputationRewards,
    pub point_map_id: u32,
    pub point_x: f32,
    pub point_y: f32,
    pub point_opt: u32,
    pub title: String,
    pub objectives_text: String,
    pub details: String,
    /// Record `+0x1a24`: the leaderboard's first line, of type `event`, when set (`0x5e1585`).
    pub end_text: String,
    pub completed_text: String,
    pub objectives: [QuestObjective; QUEST_OBJECTIVES_COUNT as usize],
    pub required_items: [(u32, u32); QUEST_REQUIRED_ITEMS_COUNT as usize],
}

// ── CMSG encoders ────────────────────────────────────────────────────────────────────────────────

/// Body of `CMSG_QUEST_QUERY`: the quest id. Unlike the giver query, no NPC guid.
pub fn quest_query(quest_id: u32) -> Vec<u8> {
    quest_id.to_le_bytes().to_vec()
}

/// Body of `CMSG_QUESTLOG_REMOVE_QUEST`: abandons a log slot. There is no reply; the cleared
/// `PLAYER_QUEST_LOG` fields are the confirmation.
pub fn questlog_remove_quest(slot: u8) -> Vec<u8> {
    vec![slot]
}

/// Body of `CMSG_QUESTLOG_SWAP_QUEST`.
pub fn questlog_swap_quest(slot1: u8, slot2: u8) -> Vec<u8> {
    vec![slot1, slot2]
}

// ── SMSG parsers ─────────────────────────────────────────────────────────────────────────────────

/// Read `SMSG_QUEST_QUERY_RESPONSE` in `0x7f70e0`'s order.
pub(in crate::messages) fn read_quest_query_response(r: &mut &[u8]) -> io::Result<QuestTemplate> {
    let mut q = QuestTemplate {
        quest_id: read_u32_le(r)?,
        method: read_u32_le(r)?,
        level: read_u32_le(r)?,
        min_level: read_u32_le(r)?,
        zone_or_sort: read_i32_le(r)?,
        quest_type: read_u32_le(r)?,
        suggested_players: read_u32_le(r)?,
        rep_objective_faction: read_u32_le(r)?,
        rep_objective_value: read_u32_le(r)?,
        ..Default::default()
    };
    let _opposite_rep_faction = read_u32_le(r)?;
    let _opposite_rep_value = read_u32_le(r)?;
    q.next_quest_in_chain = read_u32_le(r)?;
    q.xp_id = read_u32_le(r)?;
    q.money = read_i32_le(r)?;
    q.money_max_level = read_u32_le(r)?;
    q.reward_spell = read_u32_le(r)?;
    q.reward_spell_cast = read_i32_le(r)?;
    q.honor = read_u32_le(r)?;
    q.honor_multiplier = read_f32_le(r)?;
    q.src_item_id = read_u32_le(r)?;
    q.flags = read_u32_le(r)?;
    q.reward_title = read_u32_le(r)?;
    q.players_slain = read_u32_le(r)?;
    q.reward_talents = read_u32_le(r)?;
    q.reward_arena_points = read_u32_le(r)?;
    q.rep_no_spillover = read_u32_le(r)?;

    // Fixed-count, unlike the giver panels' count-prefixed blocks; HIDDEN_REWARDS only zeroes them.
    for slot in q.rewards.iter_mut() {
        *slot = (read_u32_le(r)?, read_u32_le(r)?);
    }
    for slot in q.choices.iter_mut() {
        *slot = (read_u32_le(r)?, read_u32_le(r)?);
    }
    q.reputation = read_reputation(r)?;

    q.point_map_id = read_u32_le(r)?;
    q.point_x = read_f32_le(r)?;
    q.point_y = read_f32_le(r)?;
    q.point_opt = read_u32_le(r)?;

    q.title = read_cstring(r)?;
    // Objectives before details: the reverse of the giver panel's `QuestDetails` order.
    q.objectives_text = read_cstring(r)?;
    q.details = read_cstring(r)?;
    q.end_text = read_cstring(r)?;
    q.completed_text = read_cstring(r)?;

    for o in q.objectives.iter_mut() {
        o.creature_or_go = read_u32_le(r)?;
        o.required_count = read_u32_le(r)?;
        o.item_drop = read_u32_le(r)?;
        o.item_drop_count = read_u32_le(r)?;
    }
    for slot in q.required_items.iter_mut() {
        *slot = (read_u32_le(r)?, read_u32_le(r)?);
    }
    // The objective texts follow every row, not interleaved with them.
    for o in q.objectives.iter_mut() {
        o.text = read_cstring(r)?;
    }
    Ok(q)
}

/// Read `SMSG_QUESTUPDATE_ADD_KILL` (`0x6d81b1`): `(questId, entry, count, required, guid)`.
/// A gameobject `entry` is `(-id)|0x80000000`. This is the toast line only: the
/// `PLAYER_QUEST_LOG` counter is the durable state.
pub(in crate::messages) fn read_quest_update_add_kill(
    r: &mut &[u8],
) -> io::Result<(u32, u32, u32, u32, u64)> {
    Ok((
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u32_le(r)?,
        read_u64_le(r)?,
    ))
}

/// Read `SMSG_QUESTUPDATE_ADD_ITEM`: `u32 itemId, u32 count`. 12340 registers no handler for it.
pub(in crate::messages) fn read_quest_update_add_item(r: &mut &[u8]) -> io::Result<(u32, u32)> {
    Ok((read_u32_le(r)?, read_u32_le(r)?))
}

/// Read `SMSG_QUESTUPDATE_ADD_PVP_KILL` (`0x6d8030`): `u32 questId, u32 count`. The required
/// count comes from the template (`+0x2098`); AzerothCore's trailing copy of it is left unread.
pub(in crate::messages) fn read_quest_update_add_pvp_kill(r: &mut &[u8]) -> io::Result<(u32, u32)> {
    let quest_id = read_u32_le(r)?;
    let count = read_u32_le(r)?;
    *r = &[];
    Ok((quest_id, count))
}

/// Read `SMSG_QUEST_FORCE_REMOVE` (`0x6d8370`): the quest id.
pub(in crate::messages) fn read_quest_force_remove(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_QUERY_QUESTS_COMPLETED_RESPONSE` (`0x5b5190`): a `u32` count, then the ids.
pub(in crate::messages) fn read_quests_completed(r: &mut &[u8]) -> io::Result<Vec<u32>> {
    let count = read_u32_le(r)?;
    let mut ids = Vec::with_capacity((count as usize).min(r.len() / 4));
    for _ in 0..count {
        ids.push(read_u32_le(r)?);
    }
    Ok(ids)
}

/// Body of `CMSG_QUERY_QUESTS_COMPLETED`: empty.
pub fn query_quests_completed() -> Vec<u8> {
    Vec::new()
}

/// Read `SMSG_QUESTUPDATE_COMPLETE` (`0x6d8030`): the quest id, once every objective is done.
pub(in crate::messages) fn read_quest_update_complete(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_QUESTUPDATE_FAILED` (`0x6d8030`): one `u32` quest id.
pub(in crate::messages) fn read_quest_update_failed(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_QUESTUPDATE_FAILEDTIMER` (`0x6d8030`): the id of a timed quest that expired.
pub(in crate::messages) fn read_quest_update_failedtimer(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fixture builders ─────────────────────────────────────────────────────────────────────────
    fn push_u32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_i32(b: &mut Vec<u8>, v: i32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_u64(b: &mut Vec<u8>, v: u64) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_f32(b: &mut Vec<u8>, v: f32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn push_cstr(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(s.as_bytes());
        b.push(0);
    }

    // ── CMSG encode goldens ──────────────────────────────────────────────────────────────────────

    #[test]
    fn cmsg_quest_query_body() {
        assert_eq!(quest_query(1234), vec![0xD2, 0x04, 0x00, 0x00]);
    }

    #[test]
    fn cmsg_questlog_remove_quest_body() {
        assert_eq!(questlog_remove_quest(5), vec![5]);
    }

    #[test]
    fn cmsg_questlog_swap_quest_body() {
        assert_eq!(questlog_swap_quest(3, 7), vec![3, 7]);
    }

    // ── SMSG parse fixtures ──────────────────────────────────────────────────────────────────────

    const GO_57: u32 = (-57i32) as u32 | 0x8000_0000;

    /// A template in `0x7f70e0`'s order, every field distinct so a misalignment shows.
    fn template_bytes() -> Vec<u8> {
        let mut b = Vec::new();
        // id, method, level, min level
        for v in [100u32, 1, 12, 8] {
            push_u32(&mut b, v);
        }
        push_i32(&mut b, -3); // zone or sort
                              // type, suggested, rep faction and value, opposite faction and value, next, xp id
        for v in [2u32, 5, 169, 500, 0, 0, 777, 6] {
            push_u32(&mut b, v);
        }
        push_i32(&mut b, 5000); // money
        push_u32(&mut b, 10); // money at max level
        push_u32(&mut b, 4321); // spell
        push_i32(&mut b, -1); // spell cast
        push_u32(&mut b, 70); // honor
        push_f32(&mut b, 0.25); // honor multiplier
                                // source item, flags, title, players slain, talents, arena, no-spillover mask
        for v in [900u32, 0x1008, 3, 2, 1, 25, 0b100] {
            push_u32(&mut b, v);
        }
        for (id, n) in [(3000, 1), (0, 0), (3005, 3), (0, 0)] {
            push_u32(&mut b, id);
            push_u32(&mut b, n);
        }
        for (id, n) in [(0, 0), (4000, 2), (0, 0), (0, 0), (0, 0), (0, 0)] {
            push_u32(&mut b, id);
            push_u32(&mut b, n);
        }
        // The three reputation arrays.
        for i in 0..15 {
            push_u32(&mut b, 60 + i);
        }
        push_u32(&mut b, 1); // point map
        push_f32(&mut b, 123.5);
        push_f32(&mut b, -45.25);
        push_u32(&mut b, 2);
        push_cstr(&mut b, "A Threat Within");
        push_cstr(&mut b, "Slay 10 kobolds.");
        push_cstr(&mut b, "Kill the kobolds threatening Goldshire.");
        push_cstr(&mut b, "Explore the mine");
        push_cstr(&mut b, "Return to Marshal McBride.");
        for (who, n, drop, dn) in [
            (100, 10, 0, 0),
            (GO_57, 1, 0, 0),
            (0, 0, 5, 1),
            (0, 0, 0, 0),
        ] {
            push_u32(&mut b, who);
            push_u32(&mut b, n);
            push_u32(&mut b, drop);
            push_u32(&mut b, dn);
        }
        for (id, n) in [(2000, 5), (0, 0), (0, 0), (0, 0), (0, 0), (2001, 1)] {
            push_u32(&mut b, id);
            push_u32(&mut b, n);
        }
        push_cstr(&mut b, "Kill 10 kobolds");
        push_cstr(&mut b, "Destroy the barricade");
        push_cstr(&mut b, "");
        push_cstr(&mut b, "");
        b
    }

    #[test]
    fn quest_query_response_reads_12340_s_order() {
        let b = template_bytes();
        let mut r = b.as_slice();
        let q = read_quest_query_response(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(q.quest_id, 100);
        assert_eq!(q.level, 12);
        assert_eq!(q.min_level, 8);
        assert_eq!(q.zone_or_sort, -3);
        assert_eq!(q.quest_type, 2);
        assert_eq!(q.suggested_players, 5);
        assert_eq!(q.rep_objective_faction, 169);
        assert_eq!(q.next_quest_in_chain, 777);
        assert_eq!(q.xp_id, 6);
        assert_eq!(q.money, 5000);
        assert_eq!(q.reward_spell, 4321);
        assert_eq!(q.reward_spell_cast, -1);
        assert_eq!(q.honor, 70);
        assert_eq!(q.honor_multiplier, 0.25);
        assert_eq!(q.src_item_id, 900);
        assert_eq!(q.flags, 0x1008);
        assert_eq!(q.reward_title, 3);
        assert_eq!(q.players_slain, 2);
        assert_eq!(q.reward_talents, 1);
        assert_eq!(q.reward_arena_points, 25);
        assert_eq!(q.rep_no_spillover, 0b100);
        assert_eq!(q.rewards, [(3000, 1), (0, 0), (3005, 3), (0, 0)]);
        assert_eq!(q.choices[1], (4000, 2));
        assert_eq!(q.reputation.factions, [60, 61, 62, 63, 64]);
        assert_eq!(q.reputation.overrides, [70, 71, 72, 73, 74]);
        assert_eq!(q.point_x, 123.5);
        assert_eq!(q.point_opt, 2);
        assert_eq!(q.title, "A Threat Within");
        assert_eq!(q.objectives_text, "Slay 10 kobolds.");
        assert_eq!(q.details, "Kill the kobolds threatening Goldshire.");
        assert_eq!(q.end_text, "Explore the mine");
        assert_eq!(q.completed_text, "Return to Marshal McBride.");
        assert_eq!(q.objectives[0].creature_or_go, 100);
        assert_eq!(q.objectives[0].required_count, 10);
        assert_eq!(q.objectives[0].text, "Kill 10 kobolds");
        assert_eq!(q.objectives[1].creature_or_go, GO_57);
        assert_eq!(q.objectives[2].item_drop, 5);
        assert_eq!(q.objectives[2].item_drop_count, 1);
        assert_eq!(q.required_items[0], (2000, 5));
        assert_eq!(q.required_items[5], (2001, 1));
    }

    #[test]
    fn a_short_quest_query_response_is_an_error() {
        let b = template_bytes();
        assert!(read_quest_query_response(&mut &b[..b.len() - 1]).is_err());
    }

    #[test]
    fn quest_update_add_kill_reads_guid() {
        let mut b = Vec::new();
        push_u32(&mut b, 100); // questId
        push_u32(&mut b, 200); // entry
        push_u32(&mut b, 3); // count
        push_u32(&mut b, 10); // required
        push_u64(&mut b, 0xF130_0000_0000_0777); // guid
        let (quest_id, entry, count, required, guid) =
            read_quest_update_add_kill(&mut b.as_slice()).unwrap();
        assert_eq!(quest_id, 100);
        assert_eq!(entry, 200);
        assert_eq!(count, 3);
        assert_eq!(required, 10);
        assert_eq!(guid, 0xF130_0000_0000_0777);
    }

    #[test]
    fn quest_update_simple_smsgs_parse() {
        let mut b = Vec::new();
        push_u32(&mut b, 42);
        assert_eq!(read_quest_update_complete(&mut b.as_slice()).unwrap(), 42);
        assert_eq!(read_quest_update_failed(&mut b.as_slice()).unwrap(), 42);
        assert_eq!(
            read_quest_update_failedtimer(&mut b.as_slice()).unwrap(),
            42
        );

        let mut b2 = Vec::new();
        push_u32(&mut b2, 3000);
        push_u32(&mut b2, 5);
        assert_eq!(
            read_quest_update_add_item(&mut b2.as_slice()).unwrap(),
            (3000, 5)
        );
    }

    /// The three 12340 handles that had no arm: each now decodes, AzerothCore's trailing required
    /// count on the PvP update included.
    #[test]
    fn pvp_kill_force_remove_and_completed_quests_decode() {
        use crate::messages::{opcode, parse_server_with_tail, ServerPacket};

        let mut b = Vec::new();
        for v in [11_052, 3, 10] {
            push_u32(&mut b, v);
        }
        let (p, tail) = parse_server_with_tail(opcode::SMSG_QUESTUPDATE_ADD_PVP_KILL, &b).unwrap();
        assert!(matches!(
            p,
            ServerPacket::QuestUpdateAddPvpKill {
                quest_id: 11_052,
                count: 3
            }
        ));
        assert_eq!(
            tail, 0,
            "the server's required count is consumed, not left as a tail"
        );

        let mut b = Vec::new();
        push_u32(&mut b, 783);
        let (p, tail) = parse_server_with_tail(opcode::SMSG_QUEST_FORCE_REMOVE, &b).unwrap();
        assert!(matches!(
            p,
            ServerPacket::QuestForceRemove { quest_id: 783 }
        ));
        assert_eq!(tail, 0);

        let mut b = Vec::new();
        for v in [3, 7, 33, 783] {
            push_u32(&mut b, v);
        }
        let (p, tail) =
            parse_server_with_tail(opcode::SMSG_QUERY_QUESTS_COMPLETED_RESPONSE, &b).unwrap();
        match p {
            ServerPacket::QuestsCompleted(ids) => assert_eq!(ids, [7, 33, 783]),
            other => panic!("expected QuestsCompleted, got {}", other.name()),
        }
        assert_eq!(tail, 0);
        // A count past the body fails the read instead of inventing ids.
        let mut b = Vec::new();
        push_u32(&mut b, 2);
        push_u32(&mut b, 7);
        assert!(parse_server_with_tail(opcode::SMSG_QUERY_QUESTS_COMPLETED_RESPONSE, &b).is_err());
        assert!(query_quests_completed().is_empty());
    }
}
