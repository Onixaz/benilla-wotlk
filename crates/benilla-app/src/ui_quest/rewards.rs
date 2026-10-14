//! The quest rewards 12340 computes client-side, for the quest frame (from the panel packet) and
//! the quest log (from the template): the XP a quest is worth at the player's level, the honor
//! its multiplier scales, the title's text, and the reputation lines with their spillover.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::{FactionCatalog, QuestRewardTables};
use benilla_protocol::messages::{
    ObjectFields, QuestExtraRewards, QuestReputationRewards, QuestTemplate,
};
use benilla_ui::script::QuestRewardExtras;

use crate::net::SelfPlayer;
use crate::ui_action::Spells;
use crate::unit_auras::UnitAuras;

/// The quest reward tables; absent without client data, when every extra reads 0.
#[derive(Resource)]
pub(crate) struct QuestRewardTablesRes(pub(crate) QuestRewardTables);

/// Startup: reads the reward tables off the patch chain.
pub(crate) fn load_quest_reward_tables(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_quest_reward_tables(&mut chain)
    };
    match loaded {
        Ok(t) => commands.insert_resource(QuestRewardTablesRes(t)),
        Err(e) => warn!("ui_quest: quest reward tables failed to load: {e:#}"),
    }
}

/// Aura type 291, `SPELL_AURA_MOD_XP_QUEST_PCT`: the effect `0x7246a0` sums.
const AURA_MOD_XP_QUEST_PCT: u32 = 0x123;

/// What the reward extras read beside the template or panel.
#[derive(SystemParam)]
pub(crate) struct RewardInputs<'w, 's> {
    tables: Option<Res<'w, QuestRewardTablesRes>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    auras: Query<'w, 's, &'static UnitAuras, With<SelfPlayer>>,
    roster: Option<Res<'w, crate::char_select::Roster>>,
}

impl RewardInputs<'_, '_> {
    /// A faction's raw standing, as `CGReputationInfo::GetFactionStanding` (`0x5d05b0`) reads it:
    /// the wire slot plus the race/class base, 0 without a slot.
    fn standing(&self, store: &ObjectFields, faction: u32) -> i32 {
        let Some(catalog) = self.factions.as_ref().map(|f| f.catalog()) else {
            return 0;
        };
        let Some(row) = catalog.faction_row(faction) else {
            return 0;
        };
        let Ok(index) = usize::try_from(row.rep_index) else {
            return 0;
        };
        let wire = self.reputations.0.get(index).map_or(0, |&(_, s)| s);
        let (race, class) = (
            store.unit_race().unwrap_or(0),
            store.unit_class().unwrap_or(0),
        );
        wire + row.base_for(race, class)
    }

    fn factions(
        &self,
        store: &ObjectFields,
        rep: &QuestReputationRewards,
        mask: u32,
    ) -> Vec<(u32, i32)> {
        match (self.tables.as_ref(), self.factions.as_ref()) {
            (Some(t), Some(f)) => {
                reward_factions(rep, mask, &t.0, f.catalog(), &|id| self.standing(store, id))
            }
            _ => Vec::new(),
        }
    }

    fn title(&self, store: &ObjectFields, id: u32, player_name: &str) -> Option<String> {
        let female = store.unit_gender() == Some(1);
        title_text(&self.tables.as_ref()?.0, id, female, player_name)
    }

    /// The quest-XP auras' multiplier (`0x7246a0`): one plus `(basePoints + 1)%` per effect.
    fn aura_bonus(&self, spells: Option<&Spells>) -> f32 {
        let (Ok(auras), Some(spells)) = (self.auras.single(), spells) else {
            return 1.0;
        };
        let mut bonus = 1.0;
        for a in auras.iter() {
            let Some(spell) = spells.catalog.get(a.spell_id) else {
                continue;
            };
            for e in 0..3 {
                if spell.effect_apply_aura[e] == AURA_MOD_XP_QUEST_PCT {
                    bonus += (spell.effect_base_points[e] + 1) as f32 * 0.01;
                }
            }
        }
        bonus
    }

    /// A PvP realm (`Cfg_Configs.dbc`'s `PlayerKillingAllowed` for the realm's type), where
    /// `QuestFlagsPVP` answers nil (`[0xb2f998]`, set at `0x4055c7`).
    pub(crate) fn realm_is_pvp(&self) -> bool {
        self.roster
            .as_ref()
            .and_then(|r| r.realm_type())
            .is_some_and(|t| crate::realm_select::pvp_rp(t).0)
    }

    /// A quest-log row's extras from its template, at the player's level.
    pub(crate) fn for_template(
        &self,
        t: &QuestTemplate,
        store: &ObjectFields,
        player_name: &str,
        spells: Option<&Spells>,
    ) -> QuestRewardExtras {
        let level = store.unit_level().unwrap_or(0);
        let (xp, honor) = match self.tables.as_ref() {
            Some(tables) => (
                quest_log_xp(
                    &tables.0,
                    t.level,
                    t.xp_id,
                    level,
                    store.player_max_level(),
                    self.aura_bonus(spells),
                ),
                quest_log_honor(&tables.0, t.honor, t.honor_multiplier, level),
            ),
            None => (0, 0),
        };
        QuestRewardExtras {
            xp,
            honor,
            // A negative count reads 0 (`0x5df3a0`, `0x5df400`).
            talents: u32::try_from(t.reward_talents as i32).unwrap_or(0),
            arena_points: u32::try_from(t.reward_arena_points as i32).unwrap_or(0),
            title: self.title(store, t.reward_title, player_name),
            factions: self.factions(store, &t.reputation, t.rep_no_spillover),
        }
    }

    /// A quest panel's extras from its packet: the XP and honor as sent, the honor a tenth of the
    /// wire value (`0x58c110`).
    pub(crate) fn for_panel(
        &self,
        e: &QuestExtraRewards,
        store: &ObjectFields,
        player_name: &str,
    ) -> QuestRewardExtras {
        QuestRewardExtras {
            xp: e.xp,
            honor: e.honor / 10,
            talents: e.talents,
            arena_points: e.arena_points,
            title: self.title(store, e.title, player_name),
            factions: self.factions(store, &e.reputation, e.rep_no_spillover),
        }
    }
}

/// The rank floors `0x58c790` caps spillover with (`0xa2d2fc`, upper bounds at `0xa2d300`, one
/// entry on): hated through exalted, then the top.
const RANK_FLOORS: [i32; 9] = [-42000, -6000, -3000, 0, 3000, 9000, 21000, 42000, 43000];

/// At most ten lines (`0x58c79c`'s `< 10` test before a push).
const MAX_LINES: usize = 10;

/// A quest's XP at `player_level` (`0x5e0420`): `QuestXP.dbc`'s base for the quest's level
/// (the player's for a `-1` quest) and XP id, scaled by `clamp(2·(quest − player) + 20, 1, 10)`
/// tenths, then rounded to 5, 10, 25 or 50 by size.
pub(crate) fn quest_xp(tables: &QuestRewardTables, level: u32, xp_id: u32, player: u32) -> u32 {
    let quest_level = if level == u32::MAX { player } else { level };
    let Some(base) = tables.base_xp(quest_level, xp_id) else {
        return 0;
    };
    let diff = quest_level as i64 - player as i64;
    let tenths = (diff * 2 + 20).clamp(1, 10);
    let xp = base as i64 * tenths / 10;
    let rounded = if xp <= 100 {
        (xp + 2) / 5 * 5
    } else if xp <= 500 {
        (xp + 5) / 10 * 10
    } else if xp <= 1000 {
        (xp + 12) / 25 * 25
    } else {
        (xp + 25) / 50 * 50
    };
    rounded as u32
}

/// `GetQuestLogRewardXP` (`0x5e49b0`): [`quest_xp`] while the player is below the level cap, else
/// 0, times the quest-XP auras' bonus (`0x7246a0`: one plus `(basePoints + 1)%` per effect of aura
/// 291).
pub(crate) fn quest_log_xp(
    tables: &QuestRewardTables,
    level: u32,
    xp_id: u32,
    player: u32,
    max_level: Option<u32>,
    aura_bonus: f32,
) -> u32 {
    if max_level.is_none_or(|cap| player >= cap) {
        return 0;
    }
    let xp = quest_xp(tables, level, xp_id, player);
    (xp as f32 * aura_bonus).round_ties_even() as u32
}

/// `GetQuestLogRewardHonor` (`0x5e4b70`): the flat honor plus `TeamContributionPoints.dbc`'s value
/// at the player's level times the multiplier, a tenth of it rounded (`0x7554f0`, `0x9e3004`).
pub(crate) fn quest_log_honor(
    tables: &QuestRewardTables,
    honor: u32,
    multiplier: f32,
    player: u32,
) -> u32 {
    if honor == 0 && multiplier <= 0.0 {
        return 0;
    }
    let per_level = tables.honor_at_level(player).unwrap_or(0.0);
    let extra = (per_level * multiplier * 0.1).round_ties_even() as i64;
    (honor as i64 + extra).max(0) as u32
}

/// A title reward's text (`0x5e4c10`, `0x58d810`): `Name1` for a female player when it is set,
/// else `Name`, either one filled with the player's name.
pub(crate) fn title_text(
    tables: &QuestRewardTables,
    id: u32,
    female: bool,
    player_name: &str,
) -> Option<String> {
    let (male, fem) = tables.title(id)?;
    let format = if fem.is_empty() || (!female && !male.is_empty()) {
        male
    } else {
        fem
    };
    Some(format.replacen("%s", player_name, 1))
}

/// The reputation lines a reward grants (`0x58d050`): each of the five slots names a faction and
/// a `QuestFactionReward.dbc` value id, or an override in hundredths, then walks the spillover to
/// the faction's parent and children (`0x58c790`). `standing` is the faction's raw standing.
///
/// The reference compares that raw standing against cap thresholds in hundredths, the units of
/// the amounts, so a cap never bites in practice; the lines it shows are reproduced as they are.
pub(crate) fn reward_factions(
    rep: &QuestReputationRewards,
    no_spillover: u32,
    tables: &QuestRewardTables,
    catalog: &FactionCatalog,
    standing: &dyn Fn(u32) -> i32,
) -> Vec<(u32, i32)> {
    let mut out = Vec::new();
    for i in 0..rep.factions.len() {
        let faction = rep.factions[i];
        if catalog.faction_row(faction).is_none() {
            continue;
        }
        let value_id = rep.value_ids[i];
        if value_id.unsigned_abs() >= 10 {
            continue;
        }
        let Some(base) = tables.reputation(value_id).filter(|&v| v != 0) else {
            continue;
        };
        let amount = match rep.overrides[i] {
            0 => base * 100,
            o => o,
        };
        let walk = Walk { catalog, standing };
        walk.add(
            &mut out,
            faction,
            amount,
            -1,
            -1,
            0,
            no_spillover & (1 << i) != 0,
        );
    }
    out
}

struct Walk<'a> {
    catalog: &'a FactionCatalog,
    standing: &'a dyn Fn(u32) -> i32,
}

impl Walk<'_> {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &self,
        out: &mut Vec<(u32, i32)>,
        faction: u32,
        amount: i32,
        up_cap: i32,
        down_cap: i32,
        exclude: u32,
        no_spillover: bool,
    ) {
        let Some(row) = self.catalog.faction_row(faction) else {
            return;
        };
        let standing = (self.standing)(faction);
        let mut after = standing.saturating_add(amount);
        if amount < 1 {
            if let Some(floor) = rank_floor(down_cap) {
                if after <= standing.min(floor) {
                    after = floor;
                    if standing < floor {
                        return;
                    }
                }
            }
        } else if let Some(ceil) = rank_floor(up_cap + 1).map(|c| c - 100) {
            if up_cap >= 0 && standing.max(ceil) <= after {
                after = ceil;
                if ceil < standing {
                    return;
                }
            }
        }
        if after == standing || out.len() >= MAX_LINES {
            return;
        }
        out.push((faction, amount));
        if no_spillover {
            return;
        }
        if row.team != 0 && row.team != exclude {
            let share = (amount as f32 * row.spillover_rate_out).round_ties_even() as i32;
            if share != 0 {
                self.add(
                    out,
                    row.team,
                    share,
                    row.spillover_rank as i32,
                    -1,
                    faction,
                    false,
                );
            }
        }
        for &child in self.catalog.children_of(faction).iter().rev() {
            let Some(c) = self.catalog.faction_row(child) else {
                continue;
            };
            if child == exclude {
                continue;
            }
            let share = (amount as f32 * c.spillover_rate_in).round_ties_even() as i32;
            if share != 0 {
                self.add(
                    out,
                    child,
                    share,
                    c.spillover_max_rank_in as i32,
                    -1,
                    faction,
                    false,
                );
            }
        }
    }
}

/// A rank's floor in hundredths, `None` for a negative or past-the-table rank.
fn rank_floor(rank: i32) -> Option<i32> {
    usize::try_from(rank)
        .ok()
        .and_then(|r| RANK_FLOORS.get(r))
        .map(|f| f * 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tables() -> QuestRewardTables {
        let mut xp = HashMap::new();
        // Level 10's row: XP id 3 is 410, id 5 is 820.
        xp.insert(10, [0, 85, 170, 410, 615, 820, 1025, 1230, 1640, 0]);
        let mut rep = HashMap::new();
        rep.insert(1, [0, 10, 25, 75, 150, 250, 350, 500, 1000, 5]);
        rep.insert(2, [0, -10, -25, -75, -150, -250, -350, -500, -1000, -5]);
        let mut titles = HashMap::new();
        titles.insert(3, ("Private %s".to_string(), String::new()));
        titles.insert(4, ("Lord %s".to_string(), "Lady %s".to_string()));
        let mut honor = HashMap::new();
        honor.insert(10, 28.0);
        QuestRewardTables::from_rows(xp, rep, titles, honor)
    }

    /// `0x5e0420`: a same-level quest pays its base, two levels under pays 60%, and a grey quest
    /// pays a tenth; each rounds by its size.
    #[test]
    fn quest_xp_scales_by_level_difference_and_rounds() {
        let t = tables();
        assert_eq!(quest_xp(&t, 10, 3, 10), 410);
        // Player 12, quest 10: 2·(−2) + 20 = 16 → capped at 10 tenths: still the full base.
        assert_eq!(quest_xp(&t, 10, 3, 12), 410);
        // Player 17: 2·(−7) + 20 = 6 tenths: 246 → nearest 10 → 250.
        assert_eq!(quest_xp(&t, 10, 3, 17), 250);
        // Player 30: clamped to 1 tenth: 41 → nearest 5 → 40.
        assert_eq!(quest_xp(&t, 10, 3, 30), 40);
        // A `-1` quest takes the player's level.
        assert_eq!(quest_xp(&t, u32::MAX, 5, 10), 820);
    }

    #[test]
    fn the_log_pays_no_xp_at_the_cap_and_applies_the_aura_bonus() {
        let t = tables();
        assert_eq!(quest_log_xp(&t, 10, 3, 10, Some(80), 1.0), 410);
        assert_eq!(quest_log_xp(&t, 10, 3, 10, Some(10), 1.0), 0);
        assert_eq!(quest_log_xp(&t, 10, 3, 10, Some(80), 1.1), 451);
    }

    #[test]
    fn honor_adds_a_tenth_of_the_scaled_level_value() {
        let t = tables();
        assert_eq!(quest_log_honor(&t, 0, 0.0, 10), 0);
        // 28 × 2.5 × 0.1 = 7.
        assert_eq!(quest_log_honor(&t, 5, 2.5, 10), 12);
    }

    #[test]
    fn a_title_takes_the_female_form_only_when_it_has_one() {
        let t = tables();
        assert_eq!(
            title_text(&t, 3, true, "Claude").as_deref(),
            Some("Private Claude")
        );
        assert_eq!(
            title_text(&t, 4, true, "Elza").as_deref(),
            Some("Lady Elza")
        );
        assert_eq!(
            title_text(&t, 4, false, "Claude").as_deref(),
            Some("Lord Claude")
        );
        assert_eq!(title_text(&t, 9, false, "Claude"), None);
    }
}
