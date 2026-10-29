//! The achievement feed: the three DBC tables, pushed once per VM, and the packets of 3.3.5a's
//! achievement engine (handlers `0x5b32f0`, `0x5b3020`, `0x5b3160`, `0x5b34a0`, `0x5b36f0`,
//! `0x5b3610`). The app keeps the book a fresh VM is seeded with and hands each packet on to the
//! live VM, which applies it and fires its events. The book empties when the world session ends, as
//! the game shutdown clears every list (`0x5b4f00`).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::messages::{AllAchievementData, CriteriaProgress};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    AchievementBook, AchievementCategoryRow, AchievementCriteriaRow, AchievementDb, AchievementRow,
    AchievementUpdate, CriteriaWire, UiScript,
};

use crate::char_select::ClientState;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid};
use crate::ui_unit::UnitFeed;

/// The achievement tables, absent when the client data failed to load.
#[derive(Resource, Clone)]
pub(crate) struct Achievements(pub(crate) Arc<AchievementDb>);

/// The player's book as the packets left it, and the packets the live VM has not taken yet.
#[derive(Resource, Default)]
pub(crate) struct PlayerAchievements {
    book: AchievementBook,
    pending: Vec<AchievementUpdate>,
}

pub(crate) struct UiAchievementPlugin;

impl Plugin for UiAchievementPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerAchievements>()
            .add_systems(Startup, load_achievements.after(AssetSet::Open))
            .add_systems(Update, feed_achievements.in_set(UnitFeed))
            .add_systems(OnExit(ClientState::InWorld), end_session)
            .net_handler(SessionEventKind::AllAchievementData, on_achievement_packet)
            .net_handler(SessionEventKind::AchievementEarned, on_achievement_packet)
            .net_handler(SessionEventKind::CriteriaUpdate, on_achievement_packet)
            .net_handler(SessionEventKind::CriteriaDeleted, on_achievement_packet)
            .net_handler(SessionEventKind::AchievementDeleted, on_achievement_packet)
            .net_handler(SessionEventKind::InspectAchievements, on_achievement_packet);
    }
}

fn load_achievements(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_achievement_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!(
                "ui_achievement: {} achievement(s), {} criteria",
                cat.achievements.len(),
                cat.criteria.len()
            );
            commands.insert_resource(Achievements(Arc::new(db_of(cat))));
        }
        Err(e) => warn!("ui_achievement: Achievement.dbc failed to load: {e:#}"),
    }
}

/// The formats crate's rows as the script engine's.
pub(crate) fn db_of(cat: benilla_formats::AchievementCatalog) -> AchievementDb {
    AchievementDb::new(
        cat.achievements
            .into_iter()
            .map(|a| AchievementRow {
                id: a.id,
                faction: a.faction,
                supercedes: a.supercedes,
                title: a.title,
                description: a.description,
                category: a.category,
                points: a.points,
                ui_order: a.ui_order,
                flags: a.flags,
                icon: a.icon,
                reward: a.reward,
                minimum_criteria: a.minimum_criteria,
                shares_criteria: a.shares_criteria,
            })
            .collect(),
        cat.categories
            .into_iter()
            .map(|c| AchievementCategoryRow {
                id: c.id,
                parent: c.parent,
                name: c.name,
            })
            .collect(),
        cat.criteria
            .into_iter()
            .map(|c| AchievementCriteriaRow {
                id: c.id,
                achievement: c.achievement,
                kind: c.kind,
                asset: c.asset,
                quantity: c.quantity,
                description: c.description,
                flags: c.flags,
                timer_time: c.timer_time,
            })
            .collect(),
    )
}

fn wire(c: &CriteriaProgress) -> CriteriaWire {
    CriteriaWire {
        id: c.id,
        counter: c.counter,
        player: c.player,
        flags: c.flags,
        date: c.date,
        timer1: c.timer1,
        timer2: c.timer2,
    }
}

fn list_parts(d: &AllAchievementData) -> (Vec<(u32, u32)>, Vec<CriteriaWire>) {
    (
        d.achievements.clone(),
        d.criteria.iter().map(wire).collect(),
    )
}

/// Seconds since the epoch, the `time(NULL)` the handlers stamp timers with.
fn unix_now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
}

/// One packet into the app's book and the VM's queue.
fn on_achievement_packet(
    In(ev): In<SessionEvent>,
    mut held: ResMut<PlayerAchievements>,
    db: Option<Res<Achievements>>,
    self_guid: Res<SelfGuid>,
) {
    let now = unix_now();
    let update = match ev {
        SessionEvent::AllAchievementData(d) => {
            debug!(
                "ui_achievement: list of {} achievement(s), {} criteria",
                d.achievements.len(),
                d.criteria.len()
            );
            let (achievements, criteria) = list_parts(&d);
            AchievementUpdate::List {
                achievements,
                criteria,
                now,
            }
        }
        SessionEvent::AchievementEarned(e) => {
            debug!("ui_achievement: {:#x} earned {}", e.player, e.achievement);
            AchievementUpdate::Earned {
                player: e.player,
                id: e.achievement,
                date: e.date,
                silent: e.silent,
                own: self_guid.0 == Some(e.player),
            }
        }
        SessionEvent::CriteriaUpdate(c) => AchievementUpdate::Criteria {
            criterion: wire(&c),
            now,
        },
        SessionEvent::CriteriaDeleted(id) => AchievementUpdate::CriteriaDeleted(id),
        SessionEvent::AchievementDeleted(id) => AchievementUpdate::AchievementDeleted(id),
        SessionEvent::InspectAchievements(i) => {
            let (achievements, criteria) = list_parts(&i.data);
            AchievementUpdate::Inspect {
                guid: i.guid,
                achievements,
                criteria,
                now,
            }
        }
        _ => return,
    };
    if let Some(db) = db {
        // The tracked list lives in the VM; the app's copy only needs the lists.
        let mut tracked = Vec::new();
        held.book.apply(&db.0, update.clone(), &mut tracked);
    }
    held.pending.push(update);
}

fn end_session(mut held: ResMut<PlayerAchievements>) {
    *held = PlayerAchievements::default();
}

/// What the feed last pushed, so a frame with nothing new does no work.
#[derive(Default)]
struct FeedMemory {
    names_generation: u64,
    /// The guids whose names the VM reads: the player, the criteria owners and the tooltips'
    /// asks.
    named: BTreeSet<u64>,
    /// The tooltips' asks, kept so a later cache answer reaches the VM.
    asked: BTreeSet<u64>,
}

#[allow(clippy::too_many_arguments)]
fn feed_achievements(
    script: Option<NonSendMut<UiScript>>,
    db: Option<Res<Achievements>>,
    mut held: ResMut<PlayerAchievements>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(db) = db else { return };
    let (memory, vm_reset) = memory.get_reset(&script);
    let player = self_guid.0.unwrap_or(0);
    script.set_achievement_player(player);
    if vm_reset {
        script.set_achievement_db(db.0.clone());
        script.set_achievement_book(held.book.clone());
        held.pending.clear();
        memory.names_generation = u64::MAX;
    } else if !held.pending.is_empty() {
        for update in std::mem::take(&mut held.pending) {
            script.apply_achievement(update);
        }
    }

    for guid in script.take_inspect_achievement_queries() {
        let _ = commands
            .0
            .send(ClientCommand::QueryInspectAchievements { target: guid });
    }
    let wants = script.take_achievement_name_wants();
    let asked = !wants.is_empty();
    for guid in wants {
        names.resolve(guid, &commands);
        memory.asked.insert(guid);
    }
    if !asked && !held.is_changed() && names.generation() == memory.names_generation {
        return;
    }
    let named: BTreeSet<u64> = held
        .book
        .criteria_players()
        .chain([player])
        .chain(memory.asked.iter().copied())
        .filter(|&g| g != 0)
        .collect();
    if named != memory.named || names.generation() != memory.names_generation {
        let map: HashMap<u64, String> = named
            .iter()
            .filter_map(|&g| names.peek(g).map(|n| (g, n.to_string())))
            .collect();
        script.set_achievement_names(map);
        memory.names_generation = names.generation();
        memory.named = named;
    }
}
