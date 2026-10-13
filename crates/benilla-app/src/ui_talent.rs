//! The talent window feed: the class's pages from `Talent.dbc` and `TalentTab.dbc`, each talent
//! group's ranks from 3.3.5a's `SMSG_TALENTS_INFO`, and the learn verbs' sends. 12340 keeps the
//! talent state the packet carries and nothing else (`0x5c9e50`): a rank is the packet's, never a
//! known-spell guess, and the packet fires `PLAYER_TALENT_UPDATE`, plus
//! `ACTIVE_TALENT_GROUP_CHANGED(new, old)` when the active group moved.
//!
//! Only the player's talents are modelled; the pet body is read and dropped (the pet spec tab
//! stays hidden, `GetNumTalentGroups(false, true)` answering 0).

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;

use benilla_formats::TalentCatalog;
use benilla_protocol::messages::TalentsInfo;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    ScriptValue, TalentGroupView, TalentPrereqView, TalentRankView, TalentTabView, TalentUiState,
    TalentView, UiScript,
};

use crate::net::{ClientCommand, NetCommands, NetHandlerApp, ObjectStore, SelfPlayer};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::{gate, UiInput};
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// The talent catalog (`Talent.dbc`, `TalentTab.dbc`), absent when the client data failed to load.
#[derive(Resource)]
pub(crate) struct Talents {
    pub(crate) catalog: TalentCatalog,
}

/// The player's talent groups as the last `SMSG_TALENTS_INFO` named them (`0xc21030`).
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub(crate) struct PlayerTalentGroups {
    /// The packet's unspent points, the active group's.
    pub(crate) unspent: u32,
    /// 0-based.
    pub(crate) active: usize,
    /// Per group, talent id → 0-based rank.
    pub(crate) ranks: Vec<HashMap<u32, u8>>,
    /// The events the packet owes the VM, fired after the snapshot they describe is pushed.
    pending_events: Vec<(&'static str, Vec<ScriptValue>)>,
}

impl PlayerTalentGroups {
    /// One group, nothing learned and `unspent` points: a fresh character's state.
    pub(crate) fn fresh(unspent: u32) -> Self {
        PlayerTalentGroups {
            unspent,
            active: 0,
            ranks: vec![HashMap::new()],
            pending_events: Vec::new(),
        }
    }
}

pub(crate) struct UiTalentPlugin;

impl Plugin for UiTalentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerTalentGroups>()
            .net_handler(SessionEventKind::TalentsInfo, on_talents_info)
            .add_systems(Startup, load_talents.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // Feed before the input pass, so an open this frame sees a populated
                    // window; drain after it, so a click sends the same frame.
                    feed_talents.in_set(UnitFeed),
                    drain_talent_learns.after(UiInput),
                ),
            );
    }
}

fn load_talents(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_talent_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!("ui_talent: {} talent(s) in the catalog", catalog.len());
            commands.insert_resource(Talents { catalog });
        }
        Err(e) => warn!("ui_talent: Talent.dbc failed to load — no talent window: {e:#}"),
    }
}

/// `SMSG_TALENTS_INFO` (`0x6cd770`): the player's groups replace the held ones whole
/// (`0x5c9a30(0)` clears them first); a pet body is not modelled.
fn on_talents_info(In(ev): In<SessionEvent>, mut held: ResMut<PlayerTalentGroups>) {
    let SessionEvent::TalentsInfo(info) = ev else {
        return;
    };
    let TalentsInfo::Player {
        unspent,
        active_group,
        groups,
    } = info
    else {
        debug!("ui_talent: pet talents not modelled");
        return;
    };
    debug!(
        "ui_talent: SMSG_TALENTS_INFO {} group(s), active {active_group}, {unspent} unspent",
        groups.len()
    );
    let (old_active, old_count) = (held.active, held.ranks.len());
    held.unspent = unspent;
    held.active = active_group.into();
    held.ranks = groups
        .iter()
        .map(|g| g.talents.iter().map(|t| (t.talent_id, t.rank)).collect())
        .collect();
    held.pending_events
        .push(("PLAYER_TALENT_UPDATE", Vec::new()));
    // `ACTIVE_TALENT_GROUP_CHANGED(new, old)`, 1-based, a side with no groups reading 0
    // (`0x5c9ebf`).
    if old_active != held.active {
        let new = if held.ranks.is_empty() {
            0
        } else {
            held.active as i64 + 1
        };
        let old = if old_count == 0 {
            0
        } else {
            old_active as i64 + 1
        };
        held.pending_events.push((
            "ACTIVE_TALENT_GROUP_CHANGED",
            vec![ScriptValue::Int(new), ScriptValue::Int(old)],
        ));
    }
}

/// The last pushed snapshot, and the points pair `CHARACTER_POINTS_CHANGED`'s deltas come from.
#[derive(Default)]
struct FeedMemory {
    pushed: TalentUiState,
    /// `None` until the first read, which gives zero deltas.
    points: Option<(u32, u32)>,
}

#[allow(clippy::too_many_arguments)]
fn feed_talents(
    script: Option<NonSendMut<UiScript>>,
    talents: Option<Res<Talents>>,
    mut held: ResMut<PlayerTalentGroups>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    // Reopened by every input the build reads: the groups, the known spells (a talent's
    // `requiredSpell`), both catalogs (their landing counts as a change) and the self descriptor.
    let self_changed = !changed_self.is_empty();
    let groups_changed = held.is_changed();
    let actions_changed = actions.is_changed();
    let talents_changed = talents.as_ref().is_some_and(|r| r.is_changed());
    let spells_changed = spells.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_talents",
        &[
            ("vm_reset", vm_reset),
            ("self", self_changed),
            ("groups", groups_changed),
            ("actions", actions_changed),
            ("talents", talents_changed),
            ("spells", spells_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || self_changed
            || groups_changed
            || actions_changed
            || talents_changed
            || spells_changed,
    );
    if gate.skip() {
        return;
    }
    let (Some(talents), Some(spells)) = (talents.as_deref(), spells.as_deref()) else {
        return;
    };
    let Ok(store) = self_q.single() else {
        return;
    };
    let race = store.0.unit_race().unwrap_or(0);
    let class = store.0.unit_class().unwrap_or(0);
    let points = (
        store.0.player_talent_points().unwrap_or(0),
        store.0.player_free_professions().unwrap_or(0),
    );
    let fresh = build_pages(
        &talents.catalog,
        &actions.spells,
        &spells.catalog,
        race,
        class,
        points,
        &held,
    );
    if fresh != memory.pushed {
        gate.audit("feed_talents", "the talent pages");
        debug!(
            "ui_talent: fed {} tab(s), {} group(s), {} unspent",
            fresh.tabs.len(),
            fresh.groups.len(),
            held.unspent
        );
        script.set_talents(fresh.clone());
        memory.pushed = fresh;
    }
    // The packet's events, after the snapshot they describe.
    if !held.pending_events.is_empty() {
        gate.audit("feed_talents", "the talent events");
        for (event, args) in std::mem::take(&mut held.bypass_change_detection().pending_events) {
            script.fire_event(event, args);
        }
    }
    // The talent and profession point deltas, as the reference's `SignalEvent2` sends them:
    // `ChatFrame.lua:1326` tests `arg2 > 0` unguarded, so an argless fire raises. The
    // free-professions line is not composed here: the stock `ChatFrame_OnEvent` arm
    // (`ChatFrame.lua:1324-1334`) prints `LEVEL_UP_SKILL_POINTS` itself on `arg2 > 0`.
    if memory.points != Some(points) {
        gate.audit("feed_talents", "the points memo");
        let (talent_delta, profession_delta) = match memory.points {
            Some((old_talent, old_professions)) => (
                i64::from(points.0) - i64::from(old_talent),
                i64::from(points.1) - i64::from(old_professions),
            ),
            None => (0, 0),
        };
        memory.points = Some(points);
        script.fire_event(
            "CHARACTER_POINTS_CHANGED",
            vec![
                ScriptValue::Int(talent_delta),
                ScriptValue::Int(profession_delta),
            ],
        );
    }
}

/// Builds the pushed snapshot: the class's tabs in file order, each tab's talents in row order
/// (`0x5c64d0`), and each group's ranks and unspent points. An inactive group's unspent is the
/// total less its own spent, the total being the packet's unspent plus the active group's spent
/// (`0x5c9ca0`).
pub(crate) fn build_pages(
    catalog: &TalentCatalog,
    known: &BTreeSet<u32>,
    // `Spell.dbc` alone, for names and icons, so the addon-corpus survey can build the snapshot
    // without the other spell catalogs.
    spells: &benilla_formats::SpellCatalog,
    race: u8,
    class: u8,
    points: (u32, u32),
    held: &PlayerTalentGroups,
) -> TalentUiState {
    let class_tabs = catalog.tabs_for_class(race, class);
    let mut tabs = Vec::with_capacity(class_tabs.len());
    let mut pages = Vec::with_capacity(class_tabs.len());
    for tab in &class_tabs {
        let list = catalog.talents_in_tab(tab.id);
        let views = list
            .iter()
            .map(|t| {
                let ranks = t.ranks[..t.max_rank() as usize]
                    .iter()
                    .map(|&spell| {
                        let d = spells.get(spell);
                        TalentRankView {
                            spell,
                            name: d.map(|d| d.name.clone()).unwrap_or_default(),
                            icon: d.and_then(|d| d.icon.clone()),
                        }
                    })
                    .collect();
                // A prerequisite naming no row in the tab draws no arrow (two 12340 rows do).
                let prereqs = t
                    .prereqs
                    .iter()
                    .filter_map(|&(id, rank)| {
                        let index = list.iter().position(|o| o.id == id)?;
                        Some(TalentPrereqView { index, rank })
                    })
                    .collect();
                TalentView {
                    id: t.id,
                    tier: t.row + 1,
                    column: t.col + 1,
                    ranks,
                    exceptional: t.exceptional,
                    prereqs,
                    required_spell_known: t.required_spell == 0
                        || known.contains(&t.required_spell),
                    required_spell_name: (t.required_spell != 0)
                        .then(|| spells.get(t.required_spell).map(|d| d.name.clone()))
                        .flatten(),
                }
            })
            .collect::<Vec<_>>();
        tabs.push(TalentTabView {
            name: tab.name.clone(),
            icon: tab.icon.clone(),
            background: tab.background.clone(),
        });
        pages.push((tab.id, views));
    }

    // A rank that names no rank spell is dropped, as the packet reader drops it (`0x5c9347`).
    let ranks_of = |group: &HashMap<u32, u8>| -> Vec<Vec<i32>> {
        pages
            .iter()
            .map(|(_, views)| {
                views
                    .iter()
                    .map(|v| match group.get(&v.id) {
                        Some(&r) if usize::from(r) < v.ranks.len() => i32::from(r),
                        _ => -1,
                    })
                    .collect()
            })
            .collect()
    };
    let spent = |ranks: &[Vec<i32>]| -> i32 { ranks.iter().flatten().map(|r| r + 1).sum() };
    let group_ranks: Vec<Vec<Vec<i32>>> = held.ranks.iter().map(ranks_of).collect();
    let active = held.active.min(group_ranks.len().saturating_sub(1));
    let total = held.unspent as i32 + group_ranks.get(active).map_or(0, |r| spent(r));
    let groups = group_ranks
        .into_iter()
        .enumerate()
        .map(|(g, ranks)| {
            let unspent = if g == active {
                held.unspent
            } else {
                (total - spent(&ranks)).max(0) as u32
            };
            TalentGroupView { ranks, unspent }
        })
        .collect();
    TalentUiState {
        tabs,
        talents: pages.into_iter().map(|(_, v)| v).collect(),
        groups,
        active_group: active,
        points,
    }
}

/// Drains the learn verbs into `CMSG_LEARN_TALENT` and `CMSG_LEARN_PREVIEW_TALENTS`; the VM chose
/// the talent and rank (`0x5c80e0`, `0x5c6a10`), and the server answers with `SMSG_TALENTS_INFO`.
fn drain_talent_learns(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for (talent_id, rank) in script.take_talent_learns() {
        debug!("ui_talent: learn talent {talent_id} rank {rank}");
        let _ = commands
            .0
            .send(ClientCommand::LearnTalent { talent_id, rank });
    }
    for talents in script.take_talent_preview_learns() {
        debug!("ui_talent: learn preview talents {talents:?}");
        let _ = commands
            .0
            .send(ClientCommand::LearnPreviewTalents { talents });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{LearnedTalent, TalentGroupInfo};

    fn held(unspent: u32, active: u8, groups: Vec<Vec<(u32, u8)>>) -> PlayerTalentGroups {
        let mut app = App::new();
        app.init_resource::<PlayerTalentGroups>();
        let info = TalentsInfo::Player {
            unspent,
            active_group: active,
            groups: groups
                .into_iter()
                .map(|g| TalentGroupInfo {
                    talents: g
                        .into_iter()
                        .map(|(talent_id, rank)| LearnedTalent { talent_id, rank })
                        .collect(),
                    glyphs: Vec::new(),
                })
                .collect(),
        };
        app.world_mut()
            .run_system_cached_with(on_talents_info, SessionEvent::TalentsInfo(info))
            .unwrap();
        app.world_mut()
            .remove_resource::<PlayerTalentGroups>()
            .unwrap()
    }

    #[test]
    fn the_packet_replaces_the_groups_and_owes_the_events() {
        let h = held(3, 1, vec![vec![(26, 4)], vec![]]);
        assert_eq!((h.unspent, h.active, h.ranks.len()), (3, 1, 2));
        assert_eq!(h.ranks[0].get(&26), Some(&4));
        let names: Vec<&str> = h.pending_events.iter().map(|(e, _)| *e).collect();
        assert_eq!(
            names,
            vec!["PLAYER_TALENT_UPDATE", "ACTIVE_TALENT_GROUP_CHANGED"]
        );
        assert_eq!(
            h.pending_events[1].1,
            vec![ScriptValue::Int(2), ScriptValue::Int(0)],
            "new 2, old 0: there were no groups before"
        );
    }

    #[test]
    fn an_inactive_groups_unspent_is_the_total_less_its_spent() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).unwrap();
        let catalog = benilla_formats::load_talent_catalog(&mut chain).unwrap();
        let spells = benilla_formats::load_spell_catalog(&mut chain).unwrap();
        const HUMAN: u8 = 1;
        const MAGE: u8 = 8;
        let first = catalog.talents_in_tab(catalog.tabs_for_class(HUMAN, MAGE)[0].id)[0].id;
        // Active group 1 holds 2 points in the first Arcane talent, 2 unspent: 4 in all. Group 2
        // holds 1 point, so it has 3 to spend.
        let h = held(2, 0, vec![vec![(first, 1)], vec![(first, 0)]]);
        let state = build_pages(&catalog, &BTreeSet::new(), &spells, HUMAN, MAGE, (2, 0), &h);
        assert_eq!(state.tabs.len(), 3);
        assert_eq!(state.groups.len(), 2);
        assert_eq!(state.groups[0].ranks[0][0], 1);
        assert_eq!((state.groups[0].unspent, state.groups[1].unspent), (2, 3));
        assert!(state.tabs.iter().all(|t| t.icon.is_some()));
    }
}
