//! The achievement engine's client side: the three DBC tables and the client's lists, which the
//! bindings read at the call through the live view, and the packet handlers of 3.3.5a's engine
//! (`0x5b32f0`, `0x5b3020`, `0x5b3160`, `0x5b34a0`, `0x5b36f0`, `0x5b3610`), which write the lists
//! and fire their events at their steps. A UI reload keeps the lists; the game shutdown clears them
//! (`0x5b4f00`).

use std::sync::Arc;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::messages::{AllAchievementData, CriteriaProgress};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    handle_achievement_packet, AchievementBook, AchievementCategoryRow, AchievementCriteriaRow,
    AchievementDb, AchievementEvent, AchievementHost, AchievementRow, AchievementUpdate,
    CriteriaWire,
};

use crate::char_select::ClientState;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid};

/// The achievement tables, absent when the client data failed to load.
#[derive(Resource, Clone)]
pub(crate) struct Achievements(pub(crate) Arc<AchievementDb>);

/// The client's achievement lists (`0xacfd98`-`0xacfde0`), which the handlers and the bindings
/// write.
#[derive(Resource, Default)]
pub(crate) struct PlayerAchievements(pub(crate) AchievementBook);

pub(crate) struct UiAchievementPlugin;

impl Plugin for UiAchievementPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerAchievements>()
            .add_systems(Startup, load_achievements.after(AssetSet::Open))
            // After the UI shutdown's tracked-list save (`0x529160`: `0x528f00`, then `0x5b4f00`).
            .add_systems(
                OnExit(ClientState::InWorld),
                end_session.after(crate::ui_script::end_ui_session),
            )
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

/// The six achievement handlers (`0x5b32f0`, `0x5b3020`, `0x5b3160`, `0x5b36f0`, `0x5b3610`,
/// `0x5b34a0`): exclusive, so each event fires at its step with the `World` lent, the book written
/// up to it and the rest of the handler after it. `own` tests the active player at the call
/// (`0x5b3125`).
fn on_achievement_packet(In(ev): In<SessionEvent>, world: &mut World) {
    let active_player = world.get_resource::<SelfGuid>().and_then(|g| g.0);
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
                own: active_player == Some(e.player),
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
    apply_update(world, update);
}

/// Run one packet's handler steps over the world's book.
pub(crate) fn apply_update(world: &mut World, update: AchievementUpdate) {
    // No tables, no lists: the game start indexes nothing to update (`0x5b6df0`).
    let Some(db) = world.get_resource::<Achievements>().map(|a| a.0.clone()) else {
        return;
    };
    world.init_resource::<PlayerAchievements>();
    handle_achievement_packet(&db, update, &mut WorldHost { world });
}

/// The handler's host: the book is the world's, and each raise fires with the `World` lent.
struct WorldHost<'w> {
    world: &'w mut World,
}

impl AchievementHost for WorldHost<'_> {
    fn book(&mut self) -> &mut AchievementBook {
        &mut self
            .world
            .resource_mut::<PlayerAchievements>()
            .into_inner()
            .0
    }

    fn raise(&mut self, (event, args): AchievementEvent) {
        crate::ui_script::live::with_live_world(self.world, |s| s.fire_event(event, args));
    }
}

/// The game shutdown's clear (`0x5b4f00`), after the UI shutdown has saved the tracked list.
fn end_session(mut held: ResMut<PlayerAchievements>) {
    *held = PlayerAchievements::default();
}

/// A player name's answer: the achievement tooltips waiting on it run their callback, which
/// rebuilds only on a name (`0x61de50`).
pub(crate) fn name_landed(world: &mut World, guid: u64, named: bool) {
    crate::ui_script::live::with_live_world(world, |s| s.achievement_name_landed(guid, named));
}

/// What the achievement bindings read and write at the call.
#[derive(SystemParam)]
pub(crate) struct AchievementLive<'w> {
    db: Option<Res<'w, Achievements>>,
    book: Option<ResMut<'w, PlayerAchievements>>,
    names: Option<Res<'w, NameCache>>,
    commands: Option<Res<'w, NetCommands>>,
}

impl AchievementLive<'_> {
    pub(crate) fn db(&self) -> Option<Arc<AchievementDb>> {
        self.db.as_ref().map(|d| d.0.clone())
    }

    pub(crate) fn book(&self) -> Option<&AchievementBook> {
        self.book.as_deref().map(|b| &b.0)
    }

    pub(crate) fn book_mut(&mut self) -> Option<&mut AchievementBook> {
        self.book.as_deref_mut().map(|b| &mut b.0)
    }

    /// The name cache's record with no callback: no query (`0x5b58b0`).
    pub(crate) fn cached_name(&self, guid: u64) -> Option<String> {
        self.names.as_ref()?.peek(guid).map(str::to_owned)
    }

    /// The name cache's record with a callback: a miss asks (`0x627220`).
    pub(crate) fn name_or_ask(&self, guid: u64) -> Option<String> {
        let (names, commands) = (self.names.as_ref()?, self.commands.as_ref()?);
        names.resolve(guid, commands).map(str::to_owned)
    }

    /// `CMSG_QUERY_INSPECT_ACHIEVEMENTS` (`0x5b1ab0`).
    pub(crate) fn query_inspect(&self, guid: u64) {
        if let Some(commands) = &self.commands {
            let _ = commands
                .0
                .send(ClientCommand::QueryInspectAchievements { target: guid });
        }
    }
}

/// A view lending only the tables and the lists, for the stock-UI tests whose other state the VM
/// seeds itself.
#[cfg(test)]
pub(crate) mod book_view {
    use std::sync::Arc;

    use benilla_ui::script::{
        handle_achievement_packet, AchievementBook, AchievementDb, AchievementEvent,
        AchievementHost, AchievementUpdate, LiveState, UiScript,
    };

    pub(crate) struct BookView {
        pub(crate) db: Arc<AchievementDb>,
        pub(crate) book: AchievementBook,
    }

    impl BookView {
        pub(crate) fn new(db: Arc<AchievementDb>) -> Self {
            Self {
                db,
                book: AchievementBook::default(),
            }
        }

        /// `f` on `s` with this view lent.
        pub(crate) fn lend<R>(
            &mut self,
            s: &mut UiScript,
            f: impl FnOnce(&mut UiScript) -> R,
        ) -> R {
            s.with_live(self, f)
        }

        /// One packet through the handler's steps, each raise with this view lent.
        pub(crate) fn apply(&mut self, s: &mut UiScript, update: AchievementUpdate) {
            let db = self.db.clone();
            handle_achievement_packet(&db, update, &mut Host { view: self, s });
        }
    }

    struct Host<'a> {
        view: &'a mut BookView,
        s: &'a mut UiScript,
    }

    impl AchievementHost for Host<'_> {
        fn book(&mut self) -> &mut AchievementBook {
            &mut self.view.book
        }

        fn raise(&mut self, (event, args): AchievementEvent) {
            self.s.with_live(self.view, |s| s.fire_event(event, args));
        }
    }

    impl LiveState for BookView {
        fn unit_flags(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn unit_health(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn active_player(&self) -> Option<u64> {
            None
        }

        fn has_control(&self) -> bool {
            true
        }

        fn far_sight(&self) -> bool {
            false
        }

        fn achievement_db(&self) -> Option<Arc<AchievementDb>> {
            Some(self.db.clone())
        }

        fn achievement_book(&self) -> Option<&AchievementBook> {
            Some(&self.book)
        }

        fn achievement_book_mut(&mut self) -> Option<&mut AchievementBook> {
            Some(&mut self.book)
        }
    }
}
