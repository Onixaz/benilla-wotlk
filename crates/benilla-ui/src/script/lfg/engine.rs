//! `CGLookingForGroup`'s state and its packet handler's transitions (`LFGPacketHandler
//! 0x55bdc0`). Each transition returns the events it fires, in order, and the lines it shows.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::super::ScriptValue;
use super::{
    LfgBootView, LfgCatalogView, LfgCompletionRewardView, LfgDungeonRewardView, LfgDungeonView,
    LfgExpansionView, LfgLine, LfgMemberLocksView, LfgName, LfgPartyView, LfgProposalMemberView,
    LfgProposalView, LfgQueueStatusView, LfgRequest, LfgRewardItemView, LfgRoleCheckMemberView,
    LfgRoleCheckView, LfgUpdateView, MapDifficultyView,
};

pub(super) type Events = Vec<(&'static str, Vec<ScriptValue>)>;

/// `LfgDungeons` flag `0x2`: the row is listed in the dungeon window.
pub(super) const FLAG_LISTED: i32 = 0x2;
/// `LfgDungeons` flag `0x4`: a seasonal dungeon, offered like a random one.
pub(super) const FLAG_SEASONAL: i32 = 0x4;
/// `LfgDungeons` type 2: a raid, listed in the raid browser.
pub(super) const TYPE_RAID: i32 = 2;
/// `LfgDungeons` type 6: a random dungeon.
pub(super) const TYPE_RANDOM: i32 = 6;

/// `SI2::PlayUISound` kits: the queue join, the role check and the failure chimes.
const SOUND_JOINED: u32 = 0x210e;
const SOUND_ROLE_CHECK: u32 = 0x43a6;
const SOUND_FAILED: u32 = 0x43bd;

/// `s_classRoles` (`0xa394b0`), by class id - 1: bit 1 leader, 2 tank, 4 healer, 8 damage.
const CLASS_ROLES: [u8; 11] = [
    0x0b, 0x0f, 0x09, 0x09, 0x0d, 0x0b, 0x0d, 0x09, 0x09, 0x00, 0x0f,
];

/// `s_joinResultErrors` (`0xacdda8`): a join result's line.
const JOIN_RESULT_ERRORS: [(u32, &str); 17] = [
    (1, "ERR_LFG_ROLE_CHECK_FAILED"),
    (2, "ERR_LFG_GROUP_FULL"),
    (4, "ERR_LFG_NO_LFG_OBJECT"),
    (5, "ERR_LFG_NO_SLOTS_PLAYER"),
    (6, "ERR_LFG_NO_SLOTS_PARTY"),
    (7, "ERR_LFG_MISMATCHED_SLOTS"),
    (8, "ERR_LFG_PARTY_PLAYERS_FROM_DIFFERENT_REALMS"),
    (9, "ERR_LFG_MEMBERS_NOT_PRESENT"),
    (10, "ERR_LFG_GET_INFO_TIMEOUT"),
    (11, "ERR_LFG_INVALID_SLOT"),
    (12, "ERR_LFG_DESERTER_PLAYER"),
    (13, "ERR_LFG_DESERTER_PARTY"),
    (14, "ERR_LFG_RANDOM_COOLDOWN_PLAYER"),
    (15, "ERR_LFG_RANDOM_COOLDOWN_PARTY"),
    (16, "ERR_LFG_TOO_MANY_MEMBERS"),
    (17, "ERR_LFG_CANT_USE_DUNGEONS"),
    (18, "ERR_LFG_ROLE_CHECK_FAILED"),
];

/// A dungeon's slot: its id in the low 24 bits, its type above (`0x559d40`).
pub(super) fn slot_of(d: &LfgDungeonView) -> u32 {
    (d.id as u32 & 0xff_ffff) | ((d.type_id as u32) << 24)
}

/// `GetClassRoles` (`0x552940`): the class's role mask, 0 for an unknown class.
pub(super) fn class_roles(class_id: u32) -> u8 {
    class_id
        .checked_sub(1)
        .and_then(|i| CLASS_ROLES.get(i as usize).copied())
        .unwrap_or(0)
}

/// `LFG_ROLE_CHECK_ROLE_CHOSEN`'s arguments: the name and the tank, healer and damage bits.
pub(crate) fn role_chosen_args(name: String, roles: u32) -> Vec<ScriptValue> {
    vec![
        ScriptValue::Str(name),
        ScriptValue::Bool(roles >> 1 & 1 != 0),
        ScriptValue::Bool(roles >> 2 & 1 != 0),
        ScriptValue::Bool(roles >> 3 & 1 != 0),
    ]
}

/// `SMSG_LFG_TELEPORT_DENIED`'s lines: 0, 5 and 7 are silent.
pub(super) fn teleport_denied(reason: u32) -> Vec<LfgLine> {
    match reason {
        0 | 5 | 7 => Vec::new(),
        1 => vec![LfgLine::Key("ERR_PLAYER_DEAD")],
        2 => vec![LfgLine::Key("ERR_NOT_WHILE_FALLING")],
        4 => vec![LfgLine::Key("ERR_NOT_WHILE_FATIGUED")],
        6 => vec![
            LfgLine::Key("ERR_INVALID_TELEPORT_LOCATION"),
            LfgLine::Key("ERR_CLIENT_LOCKED_OUT"),
        ],
        _ => vec![LfgLine::Key("ERR_CLIENT_LOCKED_OUT")],
    }
}

/// `LFGInfo`: the local selection, or the server's view of the player's or the party's queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LfgInfo {
    pub(crate) joined: bool,
    /// `+0x01`: the server's "LFG joined".
    pub(crate) lfg_joined: bool,
    pub(crate) queued: bool,
    /// `+0x03`: the local selection changed since the last join.
    pub(crate) dirty: bool,
    pub(crate) no_partial_clear: bool,
    pub(crate) achievements: bool,
    pub(crate) needs: [u8; 3],
    /// In list order.
    pub(crate) dungeons: Vec<u32>,
    pub(crate) comment: String,
}

impl LfgInfo {
    fn from_update(u: &LfgUpdateView) -> Self {
        let mut info = Self {
            joined: u.joined,
            lfg_joined: u.lfg_joined,
            queued: u.queued,
            no_partial_clear: u.no_partial_clear,
            achievements: u.achievements,
            needs: u.needs,
            comment: u.comment.clone(),
            ..Default::default()
        };
        for &slot in &u.slots {
            info.add_dungeon(slot);
        }
        info
    }

    /// `AddDungeon` (`0x559950`): no duplicate.
    fn add_dungeon(&mut self, slot: u32) {
        if !self.dungeons.contains(&slot) {
            self.dungeons.push(slot);
        }
    }

    /// `Equals` (`0x5570f0`): the flags, the needs, the comment and the dungeon set.
    fn equals(&self, other: &Self) -> bool {
        self.joined == other.joined
            && self.lfg_joined == other.lfg_joined
            && self.queued == other.queued
            && self.no_partial_clear == other.no_partial_clear
            && self.achievements == other.achievements
            && self.dungeons.len() == other.dungeons.len()
            && self.needs == other.needs
            && self.comment == other.comment
            && self.dungeons.iter().all(|d| other.dungeons.contains(d))
    }

    /// `CopyFrom` (`0x5599f0`): the flags and comment copied; dungeons gone from `other` dropped,
    /// new ones appended, so kept ones hold their place.
    fn copy_from(&mut self, other: &Self) {
        self.joined = other.joined;
        self.lfg_joined = other.lfg_joined;
        self.queued = other.queued;
        self.no_partial_clear = other.no_partial_clear;
        self.achievements = other.achievements;
        self.needs = other.needs;
        self.comment = other.comment.clone();
        self.dungeons.retain(|d| other.dungeons.contains(d));
        for &d in &other.dungeons {
            self.add_dungeon(d);
        }
    }

    /// `HasRaidDungeon` (`0x557260`): joined, with a raid among the dungeons.
    fn has_raid(&self, catalog: &Catalog) -> bool {
        self.joined
            && self.dungeons.iter().any(|&s| {
                catalog
                    .dungeon(s as i32 & 0xff_ffff)
                    .is_some_and(|d| d.type_id == TYPE_RAID)
            })
    }
}

/// `LFGDungeonEntry`: one dungeon of a lock info, its rewards and its lock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DungeonEntry {
    pub(crate) slot: u32,
    pub(crate) reward_done: bool,
    pub(crate) locked: bool,
    pub(crate) lock_reason: u32,
    pub(crate) base_money: i32,
    pub(crate) base_xp: i32,
    pub(crate) variable_money: i32,
    pub(crate) variable_xp: i32,
    pub(crate) rewards: Vec<LfgRewardItemView>,
}

/// `LFGLockInfo`: one player's dungeons.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LockInfo {
    pub(crate) valid: bool,
    pub(crate) guid: u64,
    pub(crate) dungeons: Vec<DungeonEntry>,
}

impl LockInfo {
    /// `AddDungeon` (`0x559840`): the slot's entry, appended when new.
    fn entry(&mut self, slot: u32) -> &mut DungeonEntry {
        let i = match self.dungeons.iter().position(|d| d.slot == slot) {
            Some(i) => i,
            None => {
                self.dungeons.push(DungeonEntry {
                    slot,
                    ..Default::default()
                });
                self.dungeons.len() - 1
            }
        };
        &mut self.dungeons[i]
    }

    /// `ParseBlackList` (`0x559b10`).
    fn add_locks(&mut self, locks: &[(u32, u32)]) {
        for &(slot, reason) in locks {
            let e = self.entry(slot);
            e.locked = true;
            e.lock_reason = reason;
        }
    }

    pub(super) fn find(&self, slot: u32) -> Option<&DungeonEntry> {
        self.dungeons.iter().find(|d| d.slot == slot)
    }
}

/// `LFGProposal`: the group offer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Proposal {
    /// `+0x00`, the dungeon slot; 0 is no proposal.
    pub(crate) slot: u32,
    pub(crate) state: u8,
    /// `+0x08`, the id the answer names.
    pub(crate) id: u32,
    pub(crate) completed_mask: u32,
    pub(crate) silent: bool,
    pub(crate) members: [LfgProposalMemberView; 5],
}

/// `LFGQueueStatus`, with the queued time on the `GetTime()` clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct QueueStatus {
    pub(crate) lfg_id: u32,
    pub(crate) my_wait: i32,
    pub(crate) average_wait: i32,
    pub(crate) tank_wait: i32,
    pub(crate) healer_wait: i32,
    pub(crate) damage_wait: i32,
    pub(crate) tanks_needed: u8,
    pub(crate) healers_needed: u8,
    pub(crate) damage_needed: u8,
    pub(crate) queued_at: f64,
}

/// A verb's send throttle: three calls per window (`s_joinAttempts` and kin start at 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Throttle {
    count: i32,
    since: f64,
}

impl Default for Throttle {
    fn default() -> Self {
        Self {
            count: 2,
            since: f64::NEG_INFINITY,
        }
    }
}

impl Throttle {
    /// Whether a call at `now` goes through, counting it.
    pub(crate) fn allow(&mut self, now: f64, window: f64) -> bool {
        self.count += 1;
        if self.count > 2 {
            if now - self.since < window {
                return false;
            }
            self.since = now;
            self.count = 0;
        }
        true
    }
}

/// The verbs' send throttles.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Throttles {
    pub(crate) join: Throttle,
    pub(crate) comment: Throttle,
    pub(crate) proposal: Throttle,
    pub(crate) boot_vote: Throttle,
    pub(crate) role_check: Throttle,
    pub(crate) teleport: Throttle,
    pub(crate) player_lock: Throttle,
    pub(crate) party_lock: Throttle,
}

/// The tables, by id where 12340 looks rows up by id (`GetRecord`), in file order otherwise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(crate) view: LfgCatalogView,
    dungeon_index: HashMap<i32, usize>,
    group_index: HashMap<i32, usize>,
}

impl Catalog {
    fn new(view: LfgCatalogView) -> Self {
        let dungeon_index = view
            .dungeons
            .iter()
            .enumerate()
            .map(|(i, d)| (d.id, i))
            .collect();
        let group_index = view
            .groups
            .iter()
            .enumerate()
            .map(|(i, g)| (g.id, i))
            .collect();
        Self {
            view,
            dungeon_index,
            group_index,
        }
    }

    pub(crate) fn dungeon(&self, id: i32) -> Option<&LfgDungeonView> {
        self.dungeon_index.get(&id).map(|&i| &self.view.dungeons[i])
    }

    pub(crate) fn group(&self, id: i32) -> Option<&super::LfgGroupView> {
        self.group_index.get(&id).map(|&i| &self.view.groups[i])
    }

    /// `GetLfgDungeonExpansionRec` (`0x6349f0`): the run after the dungeon's first row.
    pub(crate) fn expansion(&self, id: i32, expansion: i32) -> Option<&LfgExpansionView> {
        let rows = &self.view.expansions;
        let first = rows.iter().position(|r| r.lfg_id == id)?;
        rows[first..]
            .iter()
            .take_while(|r| r.lfg_id == id)
            .find(|r| r.expansion_level == expansion)
    }

    /// `GetMapDifficultyEntryByMapIdAndDifficulty` (`0x634950`).
    pub(crate) fn map_difficulty(&self, map: i32, difficulty: i32) -> Option<&MapDifficultyView> {
        let rows = &self.view.map_difficulties;
        let first = rows.iter().position(|r| r.map_id == map)?;
        rows[first..]
            .iter()
            .take_while(|r| r.map_id == map)
            .find(|r| r.difficulty == difficulty)
    }

    /// `DungeonEncounterCountMatchingRows` (`0x553830`): `(total, completed)`.
    pub(crate) fn encounter_counts(&self, map: i32, difficulty: i32, mask: u32) -> (u32, u32) {
        let mut total = 0;
        let mut done = 0;
        for e in self
            .view
            .encounters
            .iter()
            .filter(|e| e.map_id == map && e.difficulty == difficulty)
        {
            total += 1;
            if mask & 1u32.wrapping_shl(e.bit as u32) != 0 {
                done += 1;
            }
        }
        (total, done)
    }
}

/// One entry of the dungeon window's or the raid browser's list: a dungeon id and its group, or
/// a negated group id and its parent group (`LFGEntryInfo`).
pub(crate) type Entry = (i32, i32);

/// `QSortLFGDungeons` (`0x5536a0`): by group order, the header first, then by order index, then
/// by mean level, then by name.
fn compare_entries(a: Entry, b: Entry, catalog: &Catalog, expansion: i32) -> Ordering {
    let group_of = |e: Entry| if e.0 < 0 { -e.0 } else { e.1 };
    let order_of = |g: i32| {
        if g == 0 {
            -1
        } else {
            catalog.group(g).map_or(0, |r| r.order_index)
        }
    };
    let (ga, gb) = (group_of(a), group_of(b));
    if ga != gb {
        let o = order_of(ga).cmp(&order_of(gb));
        if o != Ordering::Equal {
            return o;
        }
    }
    let (ha, hb) = (a.0 < 0, b.0 < 0);
    if ha != hb {
        return if ha {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    // Two headers of one order: the reference reads a dungeon row for a group id here; the
    // shipped group orders are distinct, so none reaches it.
    let (Some(ra), Some(rb)) = (catalog.dungeon(a.0), catalog.dungeon(b.0)) else {
        return a.0.cmp(&b.0);
    };
    let o = ra.order_index.cmp(&rb.order_index);
    if o != Ordering::Equal {
        return o;
    }
    let level = |r: &LfgDungeonView| {
        catalog
            .expansion(r.id, expansion)
            .map_or(r.min_level + r.max_level, |e| {
                e.hard_level_min + e.hard_level_max
            })
            / 2
    };
    let o = level(ra).cmp(&level(rb));
    if o != Ordering::Equal {
        return o;
    }
    ra.name.to_lowercase().cmp(&rb.name.to_lowercase())
}

/// The engine's whole state.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LfgModel {
    pub(crate) catalog: Catalog,
    /// `[0xacd880]`, the window's own selection.
    pub(crate) local: LfgInfo,
    /// `[0xacd9a0]` and `[0xacdac0]`: the server's view, solo and grouped.
    pub(crate) player_info: LfgInfo,
    pub(crate) party_info: LfgInfo,
    pub(crate) player_lock: LockInfo,
    pub(crate) party_lock_valid: bool,
    pub(crate) party_lock: Vec<LockInfo>,
    pub(crate) reward: LfgCompletionRewardView,
    pub(crate) queue: QueueStatus,
    pub(crate) proposal: Proposal,
    pub(crate) last_proposal_id: u32,
    pub(crate) boot: LfgBootView,
    pub(crate) role_check_state: u32,
    pub(crate) role_check_slots: Vec<u32>,
    pub(crate) role_check_members: Vec<LfgRoleCheckMemberView>,
    pub(crate) random_dungeons: Vec<u32>,
    pub(crate) best_choice: u32,
    pub(crate) lfd_entries: Vec<Entry>,
    pub(crate) lfr_entries: Vec<Entry>,
    pub(crate) throttles: Throttles,
    /// `GetAccountExpansionLevel`, which picks a dungeon's `LfgDungeonExpansion` brackets.
    pub(crate) account_expansion: i32,
    pub(crate) class_id: u32,
    pub(crate) party: LfgPartyView,
    /// `(map, SMSG_INSTANCE_DIFFICULTY difficulty, dynamic difficulty)`.
    pub(crate) world: (i32, u32, u32),
    pub(crate) names: HashMap<u64, LfgName>,
    pub(crate) display_icons: HashMap<u32, String>,
    /// Reward items whose templates the reward getters asked for.
    pub(crate) reward_item_waits: HashSet<u32>,
    pub(crate) requests: Vec<LfgRequest>,
}

impl LfgModel {
    pub(super) fn set_catalog(&mut self, view: LfgCatalogView) {
        self.catalog = Catalog::new(view);
    }

    /// `InitializeGame` (`0x55d440`).
    pub(super) fn initialize_game(&mut self, expansion: u8) {
        let keep = (
            std::mem::take(&mut self.catalog),
            self.class_id,
            self.party,
            self.world,
        );
        *self = Self {
            catalog: keep.0,
            class_id: keep.1,
            party: keep.2,
            world: keep.3,
            account_expansion: i32::from(expansion),
            ..Default::default()
        };
        self.update_entries(i32::from(expansion));
    }

    /// `UpdateLFDEntries` (`0x558350`) and `UpdateLFREntries` (`0x558520`).
    fn update_entries(&mut self, expansion: i32) {
        let cat = &self.catalog;
        let mut lfd: Vec<Entry> = cat
            .view
            .dungeons
            .iter()
            .filter(|d| {
                d.flags & FLAG_LISTED != 0
                    && d.type_id != TYPE_RANDOM
                    && d.flags & FLAG_SEASONAL == 0
            })
            .map(|d| (d.id, d.group_id))
            .collect();
        lfd.extend(
            cat.view
                .groups
                .iter()
                .filter(|g| g.type_id != 0)
                .map(|g| (-g.id, g.parent_group_id)),
        );
        lfd.sort_by(|&a, &b| compare_entries(a, b, cat, expansion));
        let mut lfr: Vec<Entry> = cat
            .view
            .dungeons
            .iter()
            .filter(|d| d.type_id == TYPE_RAID)
            .map(|d| (d.id, d.group_id))
            .collect();
        lfr.extend(cat.view.groups.iter().map(|g| (-g.id, g.parent_group_id)));
        lfr.sort_by(|&a, &b| compare_entries(a, b, cat, expansion));
        self.lfd_entries = lfd;
        self.lfr_entries = lfr;
    }

    /// The server info the verbs read: the party's in a group, else the player's.
    pub(crate) fn server_info(&self) -> &LfgInfo {
        if self.party.grouped() {
            &self.party_info
        } else {
            &self.player_info
        }
    }

    /// `ClearDungeonList` (`0x559340`).
    pub(crate) fn clear_dungeon_list(&mut self) {
        self.local.dungeons.clear();
        self.local.dirty = true;
    }

    /// `SetLFGDungeon` (`0x559b70`): a dungeon or heroic joins a dungeon list, a raid a raid
    /// list; any other type, or a raid in a party, starts the list over.
    pub(crate) fn set_dungeon(&mut self, slot: u32) {
        let ty = slot >> 24;
        let clear = if ty == 1 || ty == 5 {
            self.local
                .dungeons
                .iter()
                .any(|&d| !matches!(d >> 24 & 0xff, 1 | 5))
        } else if ty != 2 || self.party.party_members != 0 {
            true
        } else {
            self.local.dungeons.iter().any(|&d| d >> 24 & 0xff != 2)
        };
        if clear {
            self.clear_dungeon_list();
        }
        if self.local.dungeons.contains(&slot) {
            return;
        }
        self.local.dungeons.push(slot);
        self.local.dirty = true;
    }

    /// `ClearLFGDungeon` (`0x5593a0`).
    pub(crate) fn clear_dungeon(&mut self, slot: u32) {
        if let Some(i) = self.local.dungeons.iter().position(|&d| d == slot) {
            self.local.dungeons.remove(i);
            self.local.dirty = true;
        }
    }

    /// `IsSlotValidForAll` (`0x559c90`): no lock on us, nor, outside a raid, on a party member.
    pub(crate) fn slot_valid_for_all(&self, slot: u32) -> bool {
        if self.player_lock.valid && self.player_lock.find(slot).is_some_and(|e| e.locked) {
            return false;
        }
        if slot & 0xff00_0000 != 0x0200_0000 && self.party_lock_valid {
            for m in &self.party_lock {
                if m.find(slot).is_some_and(|e| e.locked) {
                    return false;
                }
            }
        }
        true
    }

    /// `UpdateRandomDungeons` (`0x55aaf0`): every random dungeon, and every seasonal one the
    /// server lists unlocked, in file order; then the best choice.
    fn update_random_dungeons(&mut self) {
        let lock = &self.player_lock;
        self.random_dungeons = self
            .catalog
            .view
            .dungeons
            .iter()
            .filter(|d| {
                d.type_id == TYPE_RANDOM
                    || (d.flags & FLAG_SEASONAL != 0
                        && lock.find(slot_of(d)).is_some_and(|e| !e.locked))
            })
            .map(slot_of)
            .collect();
        self.update_best_choice();
    }

    /// `UpdateRandomDungeonBestChoice` (`0x559ea0`): the newest expansion, then the highest
    /// target level, then the highest minimum level, among the random dungeons all can join.
    fn update_best_choice(&mut self) {
        let mut best: Option<&LfgDungeonView> = None;
        for &slot in &self.random_dungeons {
            let Some(d) = self.catalog.dungeon(slot as i32 & 0xff_ffff) else {
                continue;
            };
            if self.better_random(d, best) {
                best = Some(d);
            }
        }
        let choice = best.map_or(0, slot_of);
        self.best_choice = choice;
    }

    /// `IsBetterRandomDungeon` (`0x559da0`).
    fn better_random(&self, d: &LfgDungeonView, best: Option<&LfgDungeonView>) -> bool {
        if d.type_id != TYPE_RANDOM
            || !self.slot_valid_for_all(d.id as u32 & 0xff_ffff | 0x0600_0000)
        {
            return false;
        }
        let Some(best) = best else {
            return true;
        };
        if d.expansion_level != best.expansion_level {
            return d.expansion_level > best.expansion_level;
        }
        let exp = self.account_expansion;
        let target = |r: &LfgDungeonView| {
            self.catalog
                .expansion(r.id, exp)
                .map_or(r.target_level_min + r.target_level_max, |e| {
                    e.target_level_min + e.target_level_max
                })
                / 2
        };
        let (t, tb) = (target(d), target(best));
        if t != tb {
            return t > tb;
        }
        let min = |r: &LfgDungeonView| {
            self.catalog
                .expansion(r.id, exp)
                .map_or(r.min_level, |e| e.hard_level_min)
        };
        min(d) > min(best)
    }

    /// `GetDungeonRewardItem` (`0x5574c0`).
    pub(crate) fn reward_item(&self, slot: u32, index: i64) -> Option<LfgRewardItemView> {
        if !self.player_lock.valid {
            return None;
        }
        let e = self.player_lock.find(slot)?;
        usize::try_from(index)
            .ok()
            .and_then(|i| e.rewards.get(i).copied())
    }

    /// `ClearPartyInfo` (`0x55b120`).
    pub(super) fn clear_party_info(&mut self) -> Events {
        let mut events = Vec::new();
        self.party_lock.clear();
        self.party_lock_valid = false;
        if self.role_check_state == 2 {
            self.role_check_state = 0;
            self.role_check_slots.clear();
            self.role_check_members.clear();
            events.push(("LFG_ROLE_CHECK_HIDE", Vec::new()));
        }
        if self.proposal.slot != 0 {
            self.proposal = Proposal::default();
            events.push(("LFG_PROPOSAL_FAILED", Vec::new()));
        }
        events
    }

    /// `ShowProposal` (`0x552900`): an update, and a show for a new proposal id.
    fn show_proposal(&mut self, events: &mut Events) {
        events.push(("LFG_PROPOSAL_UPDATE", Vec::new()));
        if self.last_proposal_id != self.proposal.id {
            self.last_proposal_id = self.proposal.id;
            if !self.proposal.silent {
                events.push(("LFG_PROPOSAL_SHOW", Vec::new()));
            }
        }
    }

    pub(super) fn proposal_update(&mut self, p: LfgProposalView) -> (Events, Vec<LfgLine>) {
        let mut members = [LfgProposalMemberView::default(); 5];
        let mut declined_self = false;
        let mut declined_party = false;
        for (slot, m) in members.iter_mut().zip(p.members.iter().take(5)) {
            *slot = *m;
            if m.answered && !m.accepted {
                if m.is_local {
                    declined_self = true;
                } else if m.same_group {
                    declined_party = true;
                }
            }
        }
        self.proposal = Proposal {
            slot: p.slot,
            state: p.state,
            id: p.id,
            completed_mask: p.completed_mask,
            silent: p.silent,
            members,
        };
        let mut events = Vec::new();
        let mut lines = Vec::new();
        self.show_proposal(&mut events);
        if declined_party || declined_self {
            lines.push(LfgLine::Sound(SOUND_FAILED));
            lines.push(LfgLine::Key(if declined_party {
                "ERR_LFG_PROPOSAL_DECLINED_PARTY"
            } else {
                "ERR_LFG_PROPOSAL_DECLINED_SELF"
            }));
            self.proposal = Proposal::default();
            events.push(("LFG_PROPOSAL_FAILED", Vec::new()));
        }
        (events, lines)
    }

    pub(super) fn role_check_update(
        &mut self,
        rc: LfgRoleCheckView,
        player: u64,
        names: &dyn Fn(u64) -> Option<String>,
    ) -> (Events, Vec<LfgLine>) {
        let mut events = Vec::new();
        let mut lines = Vec::new();
        self.role_check_state = rc.state;
        self.role_check_slots = rc.slots;
        self.role_check_members = rc.members;
        let local = self
            .role_check_members
            .iter()
            .position(|m| m.guid == player)
            .unwrap_or(0);
        if rc.beginning {
            lines.push(LfgLine::Key("ERR_LFG_ROLE_CHECK_INITIATED"));
            lines.push(LfgLine::Sound(SOUND_ROLE_CHECK));
            for m in self.role_check_members.iter().filter(|m| m.confirmed) {
                if let Some(name) = names(m.guid) {
                    events.push((
                        "LFG_ROLE_CHECK_ROLE_CHOSEN",
                        role_chosen_args(name, m.roles),
                    ));
                }
            }
        }
        events.push(("LFG_ROLE_CHECK_UPDATE", Vec::new()));
        let local_confirmed = self
            .role_check_members
            .get(local)
            .is_some_and(|m| m.confirmed);
        if !local_confirmed && self.role_check_state == 2 {
            events.push(("LFG_ROLE_CHECK_SHOW", Vec::new()));
        } else if self.role_check_state == 5 {
            lines.push(LfgLine::Sound(SOUND_FAILED));
            lines.push(LfgLine::Key("ERR_LFG_ROLE_CHECK_ABORTED"));
            events.push(("LFG_ROLE_CHECK_HIDE", Vec::new()));
        } else {
            if self.role_check_state == 6 {
                lines.push(LfgLine::Sound(SOUND_FAILED));
            }
            events.push(("LFG_ROLE_CHECK_HIDE", Vec::new()));
        }
        (events, lines)
    }

    pub(super) fn join_result(
        &mut self,
        result: u32,
        reason: u32,
        party_locks: Option<Vec<LfgMemberLocksView>>,
    ) -> Vec<LfgLine> {
        if result == 6 {
            self.party_lock.clear();
            self.add_party_locks(party_locks.unwrap_or_default());
            self.party_lock_valid = true;
        }
        if result == 1 {
            match reason {
                3 => return vec![LfgLine::Key("ERR_LFG_ROLE_CHECK_FAILED_TIMEOUT")],
                4 => return vec![LfgLine::Key("ERR_LFG_ROLE_CHECK_FAILED_NOT_VIABLE")],
                _ => {}
            }
        }
        JOIN_RESULT_ERRORS
            .iter()
            .find(|(r, _)| *r == result)
            .map(|&(_, key)| vec![LfgLine::Key(key)])
            .unwrap_or_default()
    }

    /// `ParsePartyInfo` (`0x55b080`): each member found or added, its dungeons replaced.
    fn add_party_locks(&mut self, members: Vec<LfgMemberLocksView>) {
        for m in members {
            let i = match self.party_lock.iter().position(|l| l.guid == m.guid) {
                Some(i) => i,
                None => {
                    self.party_lock.push(LockInfo {
                        guid: m.guid,
                        ..Default::default()
                    });
                    self.party_lock.len() - 1
                }
            };
            let lock = &mut self.party_lock[i];
            lock.dungeons.clear();
            lock.valid = true;
            lock.add_locks(&m.locks);
        }
    }

    pub(super) fn queue_status(&mut self, s: LfgQueueStatusView, now: f64) {
        let mut q = QueueStatus {
            lfg_id: s.dungeon,
            my_wait: s.my_wait,
            average_wait: s.average_wait,
            tank_wait: s.tank_wait,
            healer_wait: s.healer_wait,
            damage_wait: s.damage_wait,
            tanks_needed: s.tanks_needed,
            healers_needed: s.healers_needed,
            damage_needed: s.damage_needed,
            queued_at: now - f64::from(s.queued_secs),
        };
        // With every role filled the window still shows one damage dealer wanted.
        if q.tanks_needed == 0 && q.healers_needed == 0 && q.damage_needed == 0 {
            q.damage_needed = 1;
        }
        self.queue = q;
    }

    pub(super) fn update_player(
        &mut self,
        u: LfgUpdateView,
        grouped: bool,
    ) -> (Events, Vec<LfgLine>) {
        let info = LfgInfo::from_update(&u);
        let mut events = Vec::new();
        let mut lines = Vec::new();
        if self.proposal.slot != 0 && (!u.joined || u.queued) && !grouped {
            self.proposal = Proposal::default();
            if u.joined {
                lines.push(LfgLine::Key("ERR_LFG_PROPOSAL_FAILED"));
            }
            events.push(if u.update_type == 10 {
                ("LFG_PROPOSAL_SUCCEEDED", Vec::new())
            } else {
                ("LFG_PROPOSAL_FAILED", Vec::new())
            });
        }
        if info.equals(&self.player_info) {
            return (events, lines);
        }
        let current = &self.player_info;
        let left = Self::join_lines(&info, current, true, &self.catalog, &mut lines);
        if left && matches!(u.update_type, 7 | 15) {
            lines.push(LfgLine::Sound(SOUND_FAILED));
            lines.push(LfgLine::Key(if current.has_raid(&self.catalog) {
                "ERR_LFG_LEFT_LIST"
            } else {
                "ERR_LFG_LEFT_QUEUE"
            }));
        }
        self.player_info.copy_from(&info);
        if !grouped {
            if !info.queued {
                self.queue = QueueStatus::default();
            }
            events.push(("LFG_UPDATE", vec![ScriptValue::Bool(false)]));
        }
        (events, lines)
    }

    pub(super) fn update_party(
        &mut self,
        u: LfgUpdateView,
        grouped: bool,
    ) -> (Events, Vec<LfgLine>) {
        let info = LfgInfo::from_update(&u);
        let mut events = Vec::new();
        let mut lines = Vec::new();
        if self.proposal.slot != 0 && (!u.joined || u.queued) && grouped {
            self.proposal = Proposal::default();
            if u.joined {
                lines.push(LfgLine::Key("ERR_LFG_PROPOSAL_FAILED"));
            }
            events.push(if matches!(u.update_type, 10 | 1 | 16) {
                ("LFG_PROPOSAL_SUCCEEDED", Vec::new())
            } else {
                ("LFG_PROPOSAL_FAILED", Vec::new())
            });
        }
        if info.equals(&self.party_info) {
            return (events, lines);
        }
        let current = &self.party_info;
        let left = Self::join_lines(
            &info,
            current,
            !u.slots.is_empty(),
            &self.catalog,
            &mut lines,
        );
        if left && matches!(u.update_type, 7 | 15) {
            lines.push(LfgLine::Sound(SOUND_FAILED));
            lines.push(LfgLine::Key(if current.has_raid(&self.catalog) {
                "ERR_LFG_LEFT_LIST"
            } else {
                "ERR_LFG_LEFT_QUEUE"
            }));
        }
        self.party_info.copy_from(&info);
        if grouped {
            if !info.queued {
                self.queue = QueueStatus::default();
            }
            events.push(("LFG_UPDATE", vec![ScriptValue::Bool(false)]));
        }
        (events, lines)
    }

    /// The joined line for a fresh join (with dungeons, for the party), and whether the update
    /// is a leave.
    fn join_lines(
        info: &LfgInfo,
        current: &LfgInfo,
        has_dungeons: bool,
        catalog: &Catalog,
        lines: &mut Vec<LfgLine>,
    ) -> bool {
        if info.lfg_joined {
            if !current.lfg_joined && has_dungeons {
                lines.push(LfgLine::Sound(SOUND_JOINED));
                lines.push(LfgLine::Key(if info.has_raid(catalog) {
                    "ERR_LFG_JOINED_LIST"
                } else {
                    "ERR_LFG_JOINED_QUEUE"
                }));
                false
            } else {
                !info.queued && current.queued
            }
        } else {
            current.lfg_joined || (!info.queued && current.queued)
        }
    }

    pub(super) fn player_info_packet(
        &mut self,
        rewards: Vec<LfgDungeonRewardView>,
        locks: Vec<(u32, u32)>,
    ) {
        let mut lock = LockInfo::default();
        for r in rewards {
            let e = lock.entry(r.slot);
            e.reward_done = r.done;
            e.base_money = r.base_money;
            e.base_xp = r.base_xp;
            e.variable_money = r.variable_money;
            e.variable_xp = r.variable_xp;
            e.rewards.extend(r.items);
        }
        lock.add_locks(&locks);
        lock.valid = true;
        self.player_lock = lock;
        self.update_random_dungeons();
    }

    pub(super) fn party_info_packet(&mut self, members: Vec<LfgMemberLocksView>) {
        self.party_lock.clear();
        self.add_party_locks(members);
        self.party_lock_valid = true;
        self.update_random_dungeons();
    }

    /// `SMSG_LFG_PLAYER_REWARD`'s lines: the base and the strangers' bonus, experience then money.
    pub(super) fn completion_reward(&mut self, r: LfgCompletionRewardView) -> Vec<LfgLine> {
        let variable_money = r.variable_money.wrapping_mul(r.strangers);
        let variable_xp = r.variable_xp.wrapping_mul(r.strangers);
        let mut lines = Vec::new();
        if r.base_xp > 0 {
            lines.push(LfgLine::Int("ERR_QUEST_REWARD_EXP_I", i64::from(r.base_xp)));
        }
        if variable_xp > 0 {
            lines.push(LfgLine::Int(
                "ERR_QUEST_REWARD_EXP_I",
                i64::from(variable_xp),
            ));
        }
        if r.base_money > 0 {
            lines.push(LfgLine::Money(
                "ERR_QUEST_REWARD_MONEY_S",
                i64::from(r.base_money),
            ));
        }
        if variable_money > 0 {
            lines.push(LfgLine::Money(
                "ERR_QUEST_REWARD_MONEY_S",
                i64::from(variable_money),
            ));
        }
        self.reward = r;
        lines
    }

    pub(super) fn disabled(&mut self) {
        self.local = LfgInfo::default();
        self.player_info = LfgInfo::default();
        self.party_info = LfgInfo::default();
        self.player_lock = LockInfo::default();
        self.party_lock.clear();
        self.party_lock_valid = false;
        self.queue = QueueStatus::default();
        self.reward = LfgCompletionRewardView::default();
    }

    pub(super) fn offer_continue_args(&self, slot: u32) -> Option<Vec<ScriptValue>> {
        let d = self.catalog.dungeon(slot as i32 & 0xff_ffff)?;
        Some(vec![
            ScriptValue::Str(d.name.clone()),
            ScriptValue::Int(i64::from(d.id)),
            ScriptValue::Int(i64::from(d.type_id)),
        ])
    }

    pub(super) fn wanted_names(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.role_check_members.iter().map(|m| m.guid).collect();
        if self.boot.target != 0 {
            v.push(self.boot.target);
        }
        v.extend(self.party_lock.iter().map(|l| l.guid));
        v.sort_unstable();
        v.dedup();
        v
    }

    pub(super) fn wanted_displays(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .player_lock
            .dungeons
            .iter()
            .flat_map(|d| d.rewards.iter().map(|r| r.display))
            .chain(self.reward.items.iter().map(|r| r.display))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}
