//! The dungeon finder behind the stock `LFDFrame.lua`, `LFGFrame.lua` and `LFRFrame.lua`: the
//! client's `CGLookingForGroup` (`0x552850`-`0x55d440`). The app pushes the dungeon tables once
//! ([`UiScript::set_lfg_catalog`]) and applies each server message through the `lfg_*` methods,
//! which keep the reference's state and fire its events; the Lua verbs read that state and queue
//! the sends ([`UiScript::take_lfg_requests`]). The state lives in the VM, so a `ReloadUI` drops
//! it; the world-enter cascade that follows asks the server for the queue and the locks again.

mod api;
mod cvar_array;
mod engine;

#[cfg(test)]
mod tests;

pub(super) use api::install;
pub(crate) use engine::LfgModel;

use super::ScriptValue;

/// One `LfgDungeons.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgDungeonView {
    pub id: i32,
    pub name: String,
    pub min_level: i32,
    pub max_level: i32,
    pub target_level: i32,
    pub target_level_min: i32,
    pub target_level_max: i32,
    pub map_id: i32,
    pub difficulty: i32,
    pub flags: i32,
    pub type_id: i32,
    pub texture: String,
    pub expansion_level: i32,
    pub order_index: i32,
    pub group_id: i32,
    pub description: String,
}

/// One `LfgDungeonGroup.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgGroupView {
    pub id: i32,
    pub name: String,
    pub order_index: i32,
    pub parent_group_id: i32,
    pub type_id: i32,
}

/// One `LfgDungeonExpansion.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgExpansionView {
    pub lfg_id: i32,
    pub expansion_level: i32,
    pub hard_level_min: i32,
    pub hard_level_max: i32,
    pub target_level_min: i32,
    pub target_level_max: i32,
}

/// One `MapDifficulty.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapDifficultyView {
    pub map_id: i32,
    pub difficulty: i32,
    pub max_players: i32,
    /// A `GlobalStrings` token.
    pub difficulty_string: String,
}

/// One `DungeonEncounter.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EncounterView {
    pub map_id: i32,
    pub difficulty: i32,
    pub bit: i32,
    pub name: String,
    pub icon: Option<String>,
}

/// The tables, each in file order, the order every 12340 walk over them takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgCatalogView {
    pub dungeons: Vec<LfgDungeonView>,
    pub groups: Vec<LfgGroupView>,
    pub expansions: Vec<LfgExpansionView>,
    pub map_difficulties: Vec<MapDifficultyView>,
    pub encounters: Vec<EncounterView>,
}

/// The group facts the engine reads, `CGPartyInfo`'s and `CGRaidInfo`'s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LfgPartyView {
    /// `CGPartyInfo::InParty`: party slot 1 holds a member.
    pub in_party: bool,
    /// `CGRaidInfo::m_numMembers`.
    pub raid_members: u32,
    /// `GetNumPartyMembers`.
    pub party_members: u32,
    /// `CGPartyInfo::m_realNumMembers`: the non-battleground party's other members.
    pub real_members: u32,
    /// `SMSG_GROUP_LIST`'s LFG slot (`m_lfgSlot`) and status byte (`m_playerLFGFlags`).
    pub lfg_slot: u32,
    pub lfg_flags: u8,
}

impl LfgPartyView {
    /// `m_members[0] || CGRaidInfo::m_numMembers`, which picks the party's server info.
    pub fn grouped(&self) -> bool {
        self.in_party || self.raid_members != 0
    }
}

/// A name the app resolved for a guid: the player's name, class display name and realm.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgName {
    pub name: String,
    pub class_name: String,
    pub realm: String,
}

/// `SMSG_LFG_UPDATE_PLAYER` or `SMSG_LFG_UPDATE_PARTY`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgUpdateView {
    pub update_type: u8,
    pub joined: bool,
    pub lfg_joined: bool,
    pub queued: bool,
    pub no_partial_clear: bool,
    pub achievements: bool,
    pub needs: [u8; 3],
    pub slots: Vec<u32>,
    pub comment: String,
}

/// `SMSG_LFG_QUEUE_STATUS`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgQueueStatusView {
    pub dungeon: u32,
    pub my_wait: i32,
    pub average_wait: i32,
    pub tank_wait: i32,
    pub healer_wait: i32,
    pub damage_wait: i32,
    pub tanks_needed: u8,
    pub healers_needed: u8,
    pub damage_needed: u8,
    pub queued_secs: u32,
}

/// One member row of `SMSG_LFG_PROPOSAL_UPDATE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LfgProposalMemberView {
    pub roles: u32,
    pub is_local: bool,
    pub in_dungeon: bool,
    pub same_group: bool,
    pub answered: bool,
    pub accepted: bool,
}

/// `SMSG_LFG_PROPOSAL_UPDATE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgProposalView {
    pub slot: u32,
    pub state: u8,
    pub id: u32,
    pub completed_mask: u32,
    pub silent: bool,
    pub members: Vec<LfgProposalMemberView>,
}

/// One member row of `SMSG_LFG_ROLE_CHECK_UPDATE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LfgRoleCheckMemberView {
    pub guid: u64,
    pub confirmed: bool,
    pub roles: u32,
    pub level: u8,
}

/// `SMSG_LFG_ROLE_CHECK_UPDATE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgRoleCheckView {
    pub state: u32,
    pub beginning: bool,
    pub slots: Vec<u32>,
    pub members: Vec<LfgRoleCheckMemberView>,
}

/// `SMSG_LFG_BOOT_PROPOSAL_UPDATE`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgBootView {
    pub in_progress: bool,
    pub did_vote: bool,
    pub my_vote: bool,
    pub target: u64,
    pub total_votes: u32,
    pub boot_votes: u32,
    pub time_left: u32,
    pub votes_needed: u32,
    pub reason: String,
}

/// A reward item: `(item entry, ItemDisplayInfo id, count)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LfgRewardItemView {
    pub item: u32,
    pub display: u32,
    pub count: u32,
}

/// A random or seasonal dungeon's rewards in `SMSG_LFG_PLAYER_INFO`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgDungeonRewardView {
    pub slot: u32,
    pub done: bool,
    pub base_money: i32,
    pub base_xp: i32,
    pub variable_money: i32,
    pub variable_xp: i32,
    pub items: Vec<LfgRewardItemView>,
}

/// One player's locks in `SMSG_LFG_PARTY_INFO` or a join result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgMemberLocksView {
    pub guid: u64,
    pub locks: Vec<(u32, u32)>,
}

/// `SMSG_LFG_PLAYER_REWARD`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LfgCompletionRewardView {
    pub random_slot: u32,
    pub actual_slot: u32,
    pub first: bool,
    pub strangers: i32,
    pub base_money: i32,
    pub base_xp: i32,
    pub variable_money: i32,
    pub variable_xp: i32,
    pub items: Vec<LfgRewardItemView>,
}

/// A send a verb queued, in call order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LfgRequest {
    GetStatus,
    PlayerLockInfo,
    PartyLockInfo,
    Join {
        roles: u32,
        no_partial_clear: bool,
        achievements: bool,
        slots: Vec<u32>,
        needs: [u8; 3],
        comment: String,
    },
    Leave,
    ProposalResult {
        id: u32,
        accept: bool,
    },
    SetRoles(u8),
    BootVote(bool),
    Teleport(bool),
    SetComment(String),
}

/// What a server message shows besides its events: a `DisplayError` line or a UI sound kit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LfgLine {
    Key(&'static str),
    /// A `%s` line with a name.
    Str(&'static str, String),
    /// A `%d` line.
    Int(&'static str, i64),
    /// A `%s` line with `GetCurrencyText`'s coin string.
    Money(&'static str, i64),
    /// `SI2::PlayUISound`'s kit id.
    Sound(u32),
}

impl super::UiScript {
    /// Push the dungeon tables; the choice lists are built at [`Self::lfg_initialize_game`].
    pub fn set_lfg_catalog(&mut self, catalog: LfgCatalogView) {
        self.model_mut().lfg.set_catalog(catalog);
    }

    /// `InitializeGame` (`0x55d440`): every list and state reset, the choice lists rebuilt for
    /// the account's expansion.
    pub fn lfg_initialize_game(&mut self) {
        let mut model = self.model_mut();
        let expansion = model.account_expansion;
        model.lfg.initialize_game(expansion);
    }

    /// `EnterWorld` (`0x557240`): the proposal latch reset. The app sends the status and lock
    /// requests that follow it.
    pub fn lfg_enter_world(&mut self) {
        self.model_mut().lfg.last_proposal_id = 0;
    }

    /// Push the logged-in character's class id, `ClientServices::GetCharacterClass`.
    pub fn set_lfg_class(&mut self, class_id: u32) {
        set_field!(self, lfg.class_id, class_id);
    }

    /// Push the group facts; a moved LFG slot fires `LFG_UPDATE` (`SetLfgSlot 0x52c0a0`).
    pub fn set_lfg_party(&mut self, party: LfgPartyView) {
        if self.model_ref().lfg.party == party {
            return;
        }
        let moved = {
            let mut model = self.model_mut();
            let moved = model.lfg.party.lfg_slot != party.lfg_slot;
            model.lfg.party = party;
            moved
        };
        if moved {
            self.fire_event("LFG_UPDATE", Vec::new());
        }
    }

    /// Push the current map and `SMSG_INSTANCE_DIFFICULTY`'s pair (`[0xbd0894]`).
    pub fn set_lfg_world(&mut self, map_id: i32, difficulty: u32, dynamic: u32) {
        set_field!(self, lfg.world, (map_id, difficulty, dynamic));
    }

    /// Push the names of the guids [`Self::lfg_wanted_names`] listed.
    pub fn set_lfg_names(&mut self, names: std::collections::HashMap<u64, LfgName>) {
        set_field!(self, lfg.names, names);
    }

    /// The guids the verbs name: role check members, the boot target, the party's lock owners.
    pub fn lfg_wanted_names(&self) -> Vec<u64> {
        self.model_ref().lfg.wanted_names()
    }

    /// Push the `Interface\Icons` paths of the reward displays [`Self::lfg_wanted_displays`]
    /// listed (`GetInventoryArt`).
    pub fn set_lfg_display_icons(&mut self, icons: std::collections::HashMap<u32, String>) {
        set_field!(self, lfg.display_icons, icons);
    }

    /// The `ItemDisplayInfo` ids of every reward item held.
    pub fn lfg_wanted_displays(&self) -> Vec<u32> {
        self.model_ref().lfg.wanted_displays()
    }

    /// Drain the sends the verbs queued.
    pub fn take_lfg_requests(&mut self) -> Vec<LfgRequest> {
        drain_field!(self, lfg.requests)
    }

    /// `JoiningParty` (`0x559310`), from `SMSG_GROUP_LIST`'s join edge: the server infos reset,
    /// the status asked again, `LFG_UPDATE(true)`.
    pub fn lfg_joining_party(&mut self) {
        {
            let mut model = self.model_mut();
            model.lfg.player_info = Default::default();
            model.lfg.party_info = Default::default();
            model.lfg.requests.push(LfgRequest::GetStatus);
        }
        self.fire_event("LFG_UPDATE", vec![ScriptValue::Bool(true)]);
    }

    /// `ClearPartyInfo` (`0x55b120`), on leaving the group: the party locks dropped, a running
    /// role check hidden and a pending proposal failed.
    pub fn lfg_clear_party_info(&mut self) {
        let events = self.model_mut().lfg.clear_party_info();
        self.fire_all(events);
    }

    /// `SMSG_LFG_UPDATE_PLAYER`; `grouped` is the group fact at the packet.
    pub fn lfg_update_player(&mut self, update: LfgUpdateView, grouped: bool) -> Vec<LfgLine> {
        let (events, lines) = self.model_mut().lfg.update_player(update, grouped);
        self.fire_all(events);
        lines
    }

    /// `SMSG_LFG_UPDATE_PARTY`.
    pub fn lfg_update_party(&mut self, update: LfgUpdateView, grouped: bool) -> Vec<LfgLine> {
        let (events, lines) = self.model_mut().lfg.update_party(update, grouped);
        self.fire_all(events);
        lines
    }

    /// `SMSG_LFG_QUEUE_STATUS`.
    pub fn lfg_queue_status(&mut self, status: LfgQueueStatusView) {
        let now = self.now();
        self.model_mut().lfg.queue_status(status, now);
        self.fire_event("LFG_QUEUE_STATUS_UPDATE", Vec::new());
    }

    /// `SMSG_LFG_DISABLED`.
    pub fn lfg_disabled(&mut self) -> Vec<LfgLine> {
        self.model_mut().lfg.disabled();
        self.fire_event("LFG_UPDATE", vec![ScriptValue::Bool(false)]);
        vec![LfgLine::Key("ERR_SYSTEM_DISABLED")]
    }

    /// `SMSG_LFG_PROPOSAL_UPDATE`.
    pub fn lfg_proposal(&mut self, proposal: LfgProposalView) -> Vec<LfgLine> {
        let (events, lines) = self.model_mut().lfg.proposal_update(proposal);
        self.fire_all(events);
        lines
    }

    /// `SMSG_LFG_ROLE_CHECK_UPDATE`; `names` resolves a member's name, `None` while uncached.
    pub fn lfg_role_check(
        &mut self,
        check: LfgRoleCheckView,
        player: u64,
        names: &dyn Fn(u64) -> Option<String>,
    ) -> Vec<LfgLine> {
        let (events, lines) = self.model_mut().lfg.role_check_update(check, player, names);
        self.fire_all(events);
        lines
    }

    /// `SMSG_LFG_ROLE_CHOSEN`; the reference drops it for a name not in its cache.
    pub fn lfg_role_chosen(
        &mut self,
        name: Option<String>,
        accepted: bool,
        roles: u32,
    ) -> Vec<LfgLine> {
        let Some(name) = name else {
            return Vec::new();
        };
        if accepted {
            self.fire_event(
                "LFG_ROLE_CHECK_ROLE_CHOSEN",
                engine::role_chosen_args(name, roles),
            );
            Vec::new()
        } else {
            vec![LfgLine::Str("ERR_LFG_PLAYER_DECLINED_ROLE_CHECK", name)]
        }
    }

    /// `SMSG_LFG_JOIN_RESULT`.
    pub fn lfg_join_result(
        &mut self,
        result: u32,
        reason: u32,
        party_locks: Option<Vec<LfgMemberLocksView>>,
    ) -> Vec<LfgLine> {
        self.model_mut()
            .lfg
            .join_result(result, reason, party_locks)
    }

    /// `SMSG_LFG_BOOT_PROPOSAL_UPDATE`; `target_name` is the cached name of its target.
    pub fn lfg_boot(&mut self, boot: LfgBootView, target_name: Option<String>) -> Vec<LfgLine> {
        let mut lines = Vec::new();
        if !boot.in_progress {
            if let Some(name) = target_name {
                let key = if boot.boot_votes < boot.votes_needed {
                    "ERR_PARTY_LFG_BOOT_VOTE_FAILED"
                } else {
                    "ERR_PARTY_LFG_BOOT_VOTE_SUCCEEDED"
                };
                lines.push(LfgLine::Str(key, name));
            }
        }
        self.model_mut().lfg.boot = boot;
        self.fire_event("LFG_BOOT_PROPOSAL_UPDATE", Vec::new());
        lines
    }

    /// `SMSG_LFG_PLAYER_INFO`.
    pub fn lfg_player_info(&mut self, rewards: Vec<LfgDungeonRewardView>, locks: Vec<(u32, u32)>) {
        self.model_mut().lfg.player_info_packet(rewards, locks);
        self.fire_event("LFG_LOCK_INFO_RECEIVED", Vec::new());
        self.fire_event("LFG_UPDATE_RANDOM_INFO", Vec::new());
    }

    /// `SMSG_LFG_PARTY_INFO`.
    pub fn lfg_party_info(&mut self, members: Vec<LfgMemberLocksView>) {
        self.model_mut().lfg.party_info_packet(members);
        self.fire_event("LFG_LOCK_INFO_RECEIVED", Vec::new());
        self.fire_event("LFG_UPDATE_RANDOM_INFO", Vec::new());
    }

    /// `SMSG_LFG_PLAYER_REWARD`.
    pub fn lfg_completion_reward(&mut self, reward: LfgCompletionRewardView) -> Vec<LfgLine> {
        let lines = self.model_mut().lfg.completion_reward(reward);
        self.fire_event("LFG_COMPLETION_REWARD", Vec::new());
        lines
    }

    /// `SMSG_LFG_TELEPORT_DENIED`.
    pub fn lfg_teleport_denied(&mut self, reason: u32) -> Vec<LfgLine> {
        engine::teleport_denied(reason)
    }

    /// `SMSG_LFG_OFFER_CONTINUE`: the dungeon's name, id and type, for a row the table holds.
    pub fn lfg_offer_continue(&mut self, slot: u32) {
        let args = self.model_ref().lfg.offer_continue_args(slot);
        if let Some(args) = args {
            self.fire_event("LFG_OFFER_CONTINUE", args);
        }
    }

    /// `SMSG_OPEN_LFG_DUNGEON_FINDER` (`0x5528d0`).
    pub fn lfg_open_from_gossip(&mut self, slot: u32) {
        self.fire_event(
            "LFG_OPEN_FROM_GOSSIP",
            vec![ScriptValue::Int(i64::from(slot as i32))],
        );
    }

    /// A reward item's template landed: the random-dungeon panel repaints
    /// (`RandomDungeonRewardItemStatsCallback 0x552950`).
    pub(crate) fn lfg_item_arrived(&mut self, item: u32) {
        let waited = self.model_mut().lfg.reward_item_waits.remove(&item);
        if waited {
            self.fire_event("LFG_UPDATE_RANDOM_INFO", Vec::new());
        }
    }

    fn fire_all(&mut self, events: Vec<(&'static str, Vec<ScriptValue>)>) {
        for (event, args) in events {
            self.fire_event(event, args);
        }
    }
}
