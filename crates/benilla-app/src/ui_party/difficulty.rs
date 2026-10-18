//! The unit menu's group gates: the instance difficulties (the player's own pair from
//! `MSG_SET_DUNGEON_DIFFICULTY` and `MSG_SET_RAID_DIFFICULTY`, the group's from its list, and the
//! current one, `0x5138d0`/`0x513900`), the list's LFG-restricted bit, and the player's
//! Refer-a-Friend state, with the lines a difficulty change prints.

use benilla_protocol::messages::{DifficultySet, GroupDifficulties};
use benilla_ui::script::{DifficultyState, ReferAFriendState, UiScript};
use bevy::prelude::*;

use super::{GroupState, GROUPTYPE_RAID};
use crate::net::{ObjectStore, SelfPlayer};
use crate::ui_action::{FillArg, PlayerActions, Spells, UiError};

/// `SPELL_EFFECT_SUMMON_RAF_FRIEND`, the `Effect[0]` that makes a spell `[0xbe7d88]` (`0x5420bf`).
const SPELL_EFFECT_SUMMON_RAF_FRIEND: u32 = 152;

/// The player's own difficulties `[0xbd0898]`/`[0xbd089c]`, 0-based.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerDifficulty {
    pub dungeon: u32,
    pub raid: u32,
}

/// How the current map words a raid difficulty change (`0x524780`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RaidMap {
    /// `Map.dbc` flag `0x100`: the difficulty changes inside it.
    Dynamic,
    /// A raid map (`InstanceType` 2), whose line names its `MapDifficulty.dbc` row.
    Raid,
    /// Anywhere else.
    Other,
}

impl RaidMap {
    /// The current map's kind off `Map.dbc`.
    pub(crate) fn of(map: Option<u32>, maps: Option<&benilla_assets::MapCatalogRes>) -> Self {
        let (Some(map), Some(maps)) = (map, maps) else {
            return Self::Other;
        };
        if maps.0.has_dynamic_difficulty(map) {
            Self::Dynamic
        } else if maps.0.instance_type(map) == Some(2) {
            Self::Raid
        } else {
            Self::Other
        }
    }
}

impl GroupState {
    /// `CGPartyInfo::InParty` (`0x4fb0b0`): party slot 1 holds a member.
    pub(crate) fn in_party(&self) -> bool {
        !self.slots_emptied && self.party_slots().next().is_some()
    }

    /// The raid roster is up, `[0xbeb608]`.
    pub(crate) fn in_raid(&self) -> bool {
        self.group_type == GROUPTYPE_RAID
    }

    /// `GetCurrentDungeonDifficulty` (`0x5138d0`): the group's in a party, else the player's.
    pub(crate) fn current_dungeon_difficulty(&self) -> u32 {
        if self.in_party() && !self.in_raid() {
            u32::from(self.difficulties.dungeon)
        } else {
            self.player_difficulty.dungeon
        }
    }

    /// `GetCurrentRaidDifficulty` (`0x513900`): the group's in a raid, else the player's.
    pub(crate) fn current_raid_difficulty(&self) -> u32 {
        if self.in_raid() {
            u32::from(self.difficulties.raid)
        } else {
            self.player_difficulty.raid
        }
    }

    /// The list's tail: `CGPartyInfo::RemoveAll` clears the LFG bit silently (`0x52d85e`), then
    /// `SetLfgRestricted` signals `PARTY_LFG_RESTRICTED` when it sets it again (`0x52c0f8`); a list
    /// with members writes the group's difficulties (`0x6d8fc3`), without a line.
    pub(crate) fn apply_list_tail(
        &mut self,
        difficulties: Option<GroupDifficulties>,
        lfg_restricted: bool,
    ) {
        self.lfg_restricted = lfg_restricted;
        if lfg_restricted {
            self.lfg_signals = self.lfg_signals.wrapping_add(1);
        }
        if let Some(d) = difficulties {
            self.difficulties = d;
        }
    }

    /// `MSG_SET_DUNGEON_DIFFICULTY` (`0x525530`): the line when the current difficulty moved.
    pub(crate) fn apply_dungeon_difficulty(&mut self, set: DifficultySet) -> Vec<UiError> {
        let before = self.current_dungeon_difficulty();
        if set.set_player {
            self.player_difficulty.dungeon = set.difficulty;
        }
        if set.set_group {
            self.difficulties.dungeon = set.difficulty as u8;
        }
        let now = self.current_dungeon_difficulty();
        if now == before {
            return Vec::new();
        }
        vec![dungeon_changed_line(now)]
    }

    /// `MSG_SET_RAID_DIFFICULTY` (`0x5255a0`). On a dynamic map the group's value is not written:
    /// the reference asks to change the player difficulty instead (`0x52d8d0`), which is not built.
    pub(crate) fn apply_raid_difficulty(
        &mut self,
        set: DifficultySet,
        map: RaidMap,
    ) -> Vec<UiError> {
        let (raid, dungeon) = (
            self.current_raid_difficulty(),
            self.current_dungeon_difficulty(),
        );
        if set.set_player {
            self.player_difficulty.raid = set.difficulty;
        }
        if set.set_group && map != RaidMap::Dynamic {
            self.difficulties.raid = set.difficulty as u8;
        }
        self.difficulty_lines(dungeon, raid, map)
    }

    /// `CheckInstanceDifficultyMessages` (`0x5248e0`): the lines for whichever current difficulty
    /// moved from `dungeon`/`raid`, raid first.
    pub(crate) fn difficulty_lines(&self, dungeon: u32, raid: u32, map: RaidMap) -> Vec<UiError> {
        let mut lines = Vec::new();
        if self.current_raid_difficulty() != raid {
            lines.extend(self.raid_changed_lines(map));
        }
        let now = self.current_dungeon_difficulty();
        if now != dungeon {
            lines.push(dungeon_changed_line(now));
        }
        lines
    }

    /// `SendRaidDifficultyMessage` (`0x524780`). Not built: in a raid map the line names the
    /// `MapDifficulty.dbc` row of the map's own difficulty, neither of which is held; nothing prints.
    fn raid_changed_lines(&self, map: RaidMap) -> Vec<UiError> {
        let raid = format!("RAID_DIFFICULTY{}", self.current_raid_difficulty() + 1);
        match map {
            RaidMap::Dynamic => vec![
                UiError::args("ERR_RAID_LOCKOUT_CHANGED_S", vec![FillArg::Global(raid)]),
                UiError::args(
                    "ERR_PLAYER_DIFFICULTY_CHANGED_S",
                    vec![FillArg::Global(format!(
                        "PLAYER_DIFFICULTY{}",
                        u32::from(self.difficulties.dynamic) + 1
                    ))],
                ),
            ],
            RaidMap::Raid => Vec::new(),
            RaidMap::Other => vec![UiError::args(
                "ERR_RAID_DIFFICULTY_CHANGED_S",
                vec![FillArg::Global(raid)],
            )],
        }
    }
}

/// `ShowDungeonDifficultyChanged` (`0x524720`): the new difficulty's `DUNGEON_DIFFICULTYn` text.
fn dungeon_changed_line(current: u32) -> UiError {
    UiError::args(
        "ERR_DUNGEON_DIFFICULTY_CHANGED_S",
        vec![FillArg::Global(format!(
            "DUNGEON_DIFFICULTY{}",
            current + 1
        ))],
    )
}

/// What [`feed_menu_gates`] last pushed.
#[derive(Default)]
pub(super) struct FedGates {
    lfg_signals: u32,
}

/// Push the difficulties, the LFG bit and the Refer-a-Friend state, and fire one
/// `PARTY_LFG_RESTRICTED` per list that set the bit.
pub(super) fn feed_menu_gates(
    script: Option<NonSendMut<UiScript>>,
    group: Res<GroupState>,
    spells: Option<Res<Spells>>,
    actions: Res<PlayerActions>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut fed: Local<crate::ui_script::VmMemo<FedGates>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (fed, vm_reset) = fed.get_reset(&script);
    script.set_difficulty(DifficultyState {
        dungeon: group.current_dungeon_difficulty(),
        player_dungeon: group.player_difficulty.dungeon,
        raid: group.current_raid_difficulty(),
        player_raid: group.player_difficulty.raid,
    });
    script.set_lfg_restricted(group.lfg_restricted);
    script.set_refer_a_friend(ReferAFriendState {
        grantable_levels: self_q
            .iter()
            .next()
            .map_or(0, |s| s.0.player_raf_grantable_level()),
        summon_spell: spells
            .as_deref()
            .and_then(|s| s.learned_with_effect(&actions.spells, SPELL_EFFECT_SUMMON_RAF_FRIEND))
            .unwrap_or(0),
    });
    if vm_reset {
        fed.lfg_signals = group.lfg_signals;
    }
    while fed.lfg_signals != group.lfg_signals {
        fed.lfg_signals = fed.lfg_signals.wrapping_add(1);
        script.fire_event("PARTY_LFG_RESTRICTED", vec![]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::GroupMemberEntry;

    fn set(difficulty: u32, set_player: bool, set_group: bool) -> DifficultySet {
        DifficultySet {
            difficulty,
            set_player,
            set_group,
        }
    }

    fn in_party() -> GroupState {
        let mut g = GroupState {
            in_group: true,
            ..Default::default()
        };
        g.members.push(GroupMemberEntry {
            name: "Alice".into(),
            guid: 0x11,
            status: 1,
            flags: 0,
            roles: 0,
        });
        g
    }

    fn keys(lines: &[UiError]) -> Vec<&'static str> {
        lines.iter().map(|l| l.key).collect()
    }

    /// Solo, the player's own difficulty is current; a change of it prints the line.
    #[test]
    fn solo_the_players_own_difficulty_rules() {
        let mut g = GroupState::default();
        let lines = g.apply_dungeon_difficulty(set(1, true, false));
        assert_eq!(g.current_dungeon_difficulty(), 1);
        assert_eq!(keys(&lines), ["ERR_DUNGEON_DIFFICULTY_CHANGED_S"]);
        assert_eq!(
            lines[0].args,
            vec![FillArg::Global("DUNGEON_DIFFICULTY2".into())]
        );
        assert!(
            g.apply_dungeon_difficulty(set(1, true, false)).is_empty(),
            "no change, no line"
        );
    }

    /// In a party the group's dungeon difficulty rules, so a player-only write is silent.
    #[test]
    fn in_a_party_the_groups_dungeon_difficulty_rules() {
        let mut g = in_party();
        g.apply_list_tail(
            Some(GroupDifficulties {
                dungeon: 1,
                raid: 0,
                dynamic: 0,
            }),
            false,
        );
        assert_eq!(g.current_dungeon_difficulty(), 1);
        assert!(g.apply_dungeon_difficulty(set(0, true, false)).is_empty());
        assert_eq!(g.player_difficulty.dungeon, 0);
        // Leaving the party hands the current difficulty back to the player's.
        g.slots_emptied = true;
        let lines = g.difficulty_lines(1, 0, RaidMap::Other);
        assert_eq!(keys(&lines), ["ERR_DUNGEON_DIFFICULTY_CHANGED_S"]);
    }

    /// The raid line's wording follows the map; a dynamic map keeps the group value.
    #[test]
    fn the_raid_line_follows_the_map() {
        let mut g = GroupState::default();
        let lines = g.apply_raid_difficulty(set(1, true, true), RaidMap::Other);
        assert_eq!(keys(&lines), ["ERR_RAID_DIFFICULTY_CHANGED_S"]);
        assert_eq!(g.difficulties.raid, 1);

        let mut g = GroupState::default();
        let lines = g.apply_raid_difficulty(set(1, true, true), RaidMap::Dynamic);
        assert_eq!(
            keys(&lines),
            [
                "ERR_RAID_LOCKOUT_CHANGED_S",
                "ERR_PLAYER_DIFFICULTY_CHANGED_S"
            ]
        );
        assert_eq!(
            g.difficulties.raid, 0,
            "a dynamic map does not take the group value"
        );
    }

    /// Every restricted list signals, since the bit was cleared just before.
    #[test]
    fn each_restricted_list_signals_once() {
        let mut g = GroupState::default();
        g.apply_list_tail(None, true);
        g.apply_list_tail(None, true);
        g.apply_list_tail(None, false);
        assert_eq!(g.lfg_signals, 2);
        assert!(!g.lfg_restricted);
    }
}
