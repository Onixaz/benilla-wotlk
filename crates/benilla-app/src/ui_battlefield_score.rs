//! The battleground scoreboard feed, the app half of the stock `WorldStateFrame.lua` score frame.
//!
//! The reference holds a `MSG_PVP_LOG_DATA` board until every row's name has resolved, and the last
//! name arrival rebuilds it (`0x54ca30` → `0x54aa30`); here the board is pushed the frame the last
//! name lands. `UPDATE_BATTLEFIELD_SCORE` fires on a pushed board and on the status-3 arrival,
//! before `UPDATE_BATTLEFIELD_STATUS` (`0x54b148` before `0x54b1a9`).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::PvpLogData;
use benilla_ui::script::{
    BattlefieldScoreRow, BattlefieldScores, BattlefieldStatColumn, BattlefieldTeamView, UiScript,
};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_battlefield::BattlefieldQueue;
use crate::ui_script::{UiFeed, UiInput};
use crate::world_state_ui::WorldStateUiRes;

/// `RequestBattlefieldScoreData`'s throttle (`0x54ce30`).
const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// An arena's `InstanceType`; its board is asked once.
const INSTANCE_TYPE_ARENA: u32 = 4;

/// The last `MSG_PVP_LOG_DATA` and the request stamp, both zeroed when the session ends.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldScoreboard {
    log: Option<PvpLogData>,
    /// The next log update (`0xbea584`): the next request's earliest moment, `None` until the
    /// first.
    next_request: Option<Instant>,
}

impl BattlefieldScoreboard {
    pub(crate) fn apply(&mut self, data: PvpLogData) {
        self.log = Some(data);
    }
}

/// The column headers for `map`: the first contiguous run of matching `WorldStateUI.dbc` rows in
/// table order, as the status-3 arm builds them (`0x54b0c0`).
pub(crate) fn score_columns(catalog: &WorldStateUiRes, map: u32) -> Vec<BattlefieldStatColumn> {
    let mut out = Vec::new();
    for (_, row) in catalog.0.rows() {
        let matches = (row.map_id == map || row.map_id == u32::MAX) && row.ui_type == 2;
        if matches {
            out.push(BattlefieldStatColumn {
                text: row.text.clone(),
                icon: row.icon.clone(),
                tooltip: row.tooltip.clone(),
            });
        } else if !out.is_empty() {
            break;
        }
    }
    out
}

/// Resolve the raw board through the name cache; `None` while any name is still in flight.
/// A row's race and class are the name cache's; a streamed unit's own fields stand in where the
/// cache has none, as for ourselves, whose name the reference queries at login.
fn resolve_board(
    log: &PvpLogData,
    names: &NameCache,
    unit_traits: impl Fn(u64) -> Option<(u8, u8)>,
    commands: &NetCommands,
) -> Option<Vec<BattlefieldScoreRow>> {
    let mut rows = Vec::with_capacity(log.rows.len());
    for r in &log.rows {
        let name = names.resolve(r.guid, commands)?.to_string();
        let (race, class) = names
            .player_traits(r.guid)
            .map(|(race, class, _)| (race, class))
            .or_else(|| unit_traits(r.guid))
            .unwrap_or((0, 0));
        // An arena row keeps its wire team; a battleground's side comes from the race
        // (`0x54aa94`): 0 Horde, 1 Alliance, -1 neither.
        let faction = match r.arena_team {
            Some(team) => i32::from(team),
            None => i32::from(crate::ui_unit::race_pvp_team(race)),
        };
        let mut stats = [0u32; 8];
        for (slot, v) in stats.iter_mut().zip(&r.stats) {
            *slot = *v;
        }
        rows.push(BattlefieldScoreRow {
            name,
            killing_blows: r.killing_blows,
            honorable_kills: r.honorable_kills,
            deaths: r.deaths,
            bonus_honor: r.bonus_honor,
            faction,
            race: crate::ui_unit::race_names(race).map(|(n, _)| n.to_string()),
            class: crate::ui_unit::class_names(class).map(|(n, t)| (n.to_string(), t.to_string())),
            class_id: u32::from(class),
            damage_done: r.damage_done,
            healing_done: r.healing_done,
            stats,
        });
    }
    Some(rows)
}

pub(crate) fn feed_battlefield_score(
    script: Option<NonSendMut<UiScript>>,
    board: Res<BattlefieldScoreboard>,
    mut queue: ResMut<BattlefieldQueue>,
    catalog: Option<Res<WorldStateUiRes>>,
    names: Res<NameCache>,
    guids: Res<crate::net::GuidIndex>,
    stores: Query<&crate::net::ObjectStore>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<BattlefieldScores>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fresh = board.log.as_ref().and_then(|log| {
        let unit_traits = |guid: u64| {
            let f = &stores.get(*guids.0.get(&guid)?).ok()?.0;
            Some((f.unit_race()?, f.unit_class()?))
        };
        let rows = resolve_board(log, &names, unit_traits, &commands)?;
        let columns = queue
            .active_map()
            .zip(catalog.as_deref())
            .map(|(map, cat)| score_columns(cat, map))
            .unwrap_or_default();
        let teams = log.arena_teams.as_ref().map_or_else(Default::default, |t| {
            t.clone().map(|t| BattlefieldTeamView {
                name: t.name,
                rating: t.rating,
                new_rating: t.new_rating,
                matchmaking: t.matchmaking,
            })
        });
        Some(BattlefieldScores {
            rows,
            ended: log.ended,
            winner: log.winner.unwrap_or(0),
            columns,
            teams,
        })
    });
    let status_rebuild = queue.take_became_active();
    if status_rebuild {
        script.reset_battlefield_score_filter();
    }
    let last = last.get(&script);
    let changed = fresh.is_some() && fresh != *last;
    if changed {
        script.set_battlefield_scores(fresh.clone().expect("checked"));
        *last = fresh;
    }
    if changed || status_rebuild {
        script.fire_event("UPDATE_BATTLEFIELD_SCORE", vec![]);
    }
}

/// The score request (`0x54ce30`): once asked, a finished match's or an arena's board is not
/// asked again; otherwise once per 5000 ms.
fn drain_battlefield_score(
    script: Option<NonSendMut<UiScript>>,
    mut board: ResMut<BattlefieldScoreboard>,
    queue: Res<BattlefieldQueue>,
    maps: Option<Res<benilla_assets::MapCatalogRes>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if script.take_battlefield_score_requests() == 0 {
        return;
    }
    let in_arena = queue
        .active_map()
        .and_then(|m| maps.as_deref()?.0.instance_type(m))
        == Some(INSTANCE_TYPE_ARENA);
    let done = board.log.as_ref().is_some_and(|l| l.ended);
    if board.next_request.is_some() && (done || in_arena) {
        return;
    }
    let now = Instant::now();
    if board.next_request.is_none_or(|t| now >= t) {
        board.next_request = Some(now + REQUEST_THROTTLE);
        let _ = commands.0.send(ClientCommand::RequestBattlefieldScoreData);
    }
}

/// The world-enter reset (`EnterWorld`, `0x54ca50`): the request stamp and the finished flag.
fn reset_score_on_world_enter(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut board: ResMut<BattlefieldScoreboard>,
) {
    if cascades.read().next().is_some() {
        *board = BattlefieldScoreboard::default();
    }
}

/// The scoreboard's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::BattlefieldScoreboard;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PvpLogData, on_pvp_log_data)
            .net_handler(SessionEventKind::Disconnected, on_session_end);
    }

    /// Zeroes the board and the stamp, as the reference's module init does at every login.
    fn on_session_end(In(_): In<SessionEvent>, mut board: ResMut<BattlefieldScoreboard>) {
        *board = BattlefieldScoreboard::default();
    }

    fn on_pvp_log_data(In(ev): In<SessionEvent>, mut board: ResMut<BattlefieldScoreboard>) {
        if let SessionEvent::PvpLogData(data) = ev {
            board.apply(data);
        }
    }
}

pub(crate) struct BattlefieldScorePlugin;

impl Plugin for BattlefieldScorePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BattlefieldScoreboard>().add_systems(
            Update,
            (
                reset_score_on_world_enter
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(feed_battlefield_score),
                feed_battlefield_score.in_set(UiFeed),
                drain_battlefield_score.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};

    fn row(map_id: u32, ui_type: u32, text: &str) -> WorldStateUiRow {
        WorldStateUiRow {
            phase_shift: 0,
            map_id,
            area_id: 0,
            icon: String::new(),
            text: text.into(),
            tooltip: String::new(),
            state_variable: 0,
            ui_type,
            dynamic_icon: String::new(),
            dynamic_tooltip: String::new(),
            extended_ui: String::new(),
            extended_ui_state: [0; 3],
        }
    }

    #[test]
    fn the_columns_are_the_first_contiguous_run_of_type_two_rows_for_the_map() {
        let cat = WorldStateUiRes(WorldStateUiCatalog::from_rows(vec![
            (1, row(489, 0, "always-up")),
            (2, row(529, 2, "other map")),
            (3, row(489, 2, "Flags Captured")),
            (4, row(u32::MAX, 2, "Flags Returned")),
            (5, row(489, 0, "a gap")),
            (6, row(489, 2, "after the gap")),
        ]));
        let cols: Vec<String> = score_columns(&cat, 489)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(cols, ["Flags Captured", "Flags Returned"]);
        // Another map gets the wildcard row alone: `-1` rows serve every map.
        let cols: Vec<String> = score_columns(&cat, 30)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(cols, ["Flags Returned"]);
    }

    #[test]
    fn the_session_end_zeroes_the_scoreboard() {
        let mut app = App::new();
        app.init_resource::<BattlefieldScoreboard>();
        net::register(&mut app);
        {
            let mut board = app.world_mut().resource_mut::<BattlefieldScoreboard>();
            board.apply(PvpLogData {
                ended: true,
                winner: Some(1),
                ..Default::default()
            });
            board.next_request = Some(Instant::now());
        }

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let board = app.world().resource::<BattlefieldScoreboard>();
        assert!(board.log.is_none());
        assert!(board.next_request.is_none());
    }
}
