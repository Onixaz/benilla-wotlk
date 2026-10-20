//! The quest sends: the questgiver dialog, the quest log and sharing. The dialog is client-driven:
//! each panel is its own send, and the server answers only the panel asked for.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_QUESTGIVER_HELLO`: answered by the quest list or the gossip menu. The reference also
    /// sends it when a quest session on a non-gossip NPC ends, to return to the giver's list.
    pub fn questgiver_hello(&mut self, npc: u64) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_HELLO,
            &messages::questgiver_hello(npc),
        )
    }

    /// `CMSG_QUESTGIVER_QUERY_QUEST`: a quest row's click, asking for the details panel.
    pub fn questgiver_query_quest(&mut self, npc: u64, quest: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_QUERY_QUEST,
            &messages::questgiver_query_quest(npc, quest),
        )
    }

    /// `CMSG_QUESTGIVER_ACCEPT_QUEST`: answered by `SMSG_GOSSIP_COMPLETE` closing the window.
    pub fn questgiver_accept_quest(
        &mut self,
        npc: u64,
        quest: u32,
        start_cheat: u32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_ACCEPT_QUEST,
            &messages::questgiver_accept_quest(npc, quest, start_cheat),
        )
    }

    /// `CMSG_QUESTGIVER_COMPLETE_QUEST`: answered by the progress panel, or by the reward panel
    /// when no items are required.
    pub fn questgiver_complete_quest(&mut self, npc: u64, quest: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_COMPLETE_QUEST,
            &messages::questgiver_complete_quest(npc, quest),
        )
    }

    /// `CMSG_QUESTGIVER_REQUEST_REWARD`: the progress panel's Continue, answered by the reward
    /// panel.
    pub fn questgiver_request_reward(&mut self, npc: u64, quest: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_REQUEST_REWARD,
            &messages::questgiver_request_reward(npc, quest),
        )
    }

    /// `CMSG_QUESTGIVER_CHOOSE_REWARD`: `reward` is the choice index; answered by
    /// `SMSG_QUESTGIVER_QUEST_COMPLETE`.
    pub fn questgiver_choose_reward(&mut self, npc: u64, quest: u32, reward: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_CHOOSE_REWARD,
            &messages::questgiver_choose_reward(npc, quest, reward),
        )
    }

    /// `CMSG_QUEST_QUERY`: a quest template by id alone, the quest log's ask-once source.
    pub fn quest_query(&mut self, quest_id: u32) -> Result<()> {
        self.send(opcode::CMSG_QUEST_QUERY, &messages::quest_query(quest_id))
    }

    /// `CMSG_QUEST_POI_QUERY`: the POI regions of quests the log holds none for.
    pub fn quest_poi_query(&mut self, quests: &[u32]) -> Result<()> {
        self.send(
            opcode::CMSG_QUEST_POI_QUERY,
            &messages::quest_poi_query(quests),
        )
    }

    /// `CMSG_QUESTGIVER_STATUS_QUERY`: the NPC's overhead `!` or `?` marker.
    pub fn questgiver_status_query(&mut self, npc: u64) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTGIVER_STATUS_QUERY,
            &messages::questgiver_status_query(npc),
        )
    }

    /// `CMSG_QUESTLOG_REMOVE_QUEST`: no ack; the server clears the `PLAYER_QUEST_LOG` slot fields.
    pub fn questlog_remove_quest(&mut self, slot: u8) -> Result<()> {
        self.send(
            opcode::CMSG_QUESTLOG_REMOVE_QUEST,
            &messages::questlog_remove_quest(slot),
        )
    }

    /// `CMSG_PUSHQUESTTOPARTY`: the server walks the group and answers with `MSG_QUEST_PUSH_RESULT`
    /// per member, often two (`SHARING_QUEST`, then the outcome).
    pub fn push_quest_to_party(&mut self, quest_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_PUSHQUESTTOPARTY,
            &messages::push_quest_to_party(quest_id),
        )
    }

    /// `CMSG_QUEST_CONFIRM_ACCEPT`: Yes to a party member's `QUEST_FLAGS_PARTY_ACCEPT` quest; No
    /// sends nothing.
    pub fn quest_confirm_accept(&mut self, quest_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_QUEST_CONFIRM_ACCEPT,
            &messages::quest_confirm_accept(quest_id),
        )
    }

    /// `MSG_QUEST_PUSH_RESULT`: our answer to a shared quest, relayed to `sharer`, the player guid
    /// the shared details panel arrived under.
    pub fn quest_push_result(&mut self, sharer: u64, msg: messages::QuestShareMsg) -> Result<()> {
        self.send(
            opcode::MSG_QUEST_PUSH_RESULT,
            &messages::quest_push_result(sharer, msg),
        )
    }
}
