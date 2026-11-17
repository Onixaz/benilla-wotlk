//! The battleground scoreboard, the app half of the stock `WorldStateFrame.lua` score frame.
//!
//! `MSG_PVP_LOG_DATA`'s handler (`0x54d280`) writes the log into the VM's board in wire order,
//! asks each uncached name, counting it pending (`0x54d469`), and with none pending runs the
//! rebuild that fires `UPDATE_BATTLEFIELD_SCORE` (`0x54d495` -> `0x54aa30`). Each name's answer
//! counts down, the last one running the rebuild (`0x54ca30`). The verbs read a row's name from the
//! name cache when called (`0x54be90`), through the live view.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::PvpLogData;
use benilla_ui::script::{
    BattlefieldScoreRow, BattlefieldScores, BattlefieldStatColumn, BattlefieldTeamView, ScoreName,
    UiScript,
};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_battlefield::BattlefieldQueue;
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;
use crate::world_state_ui::WorldStateUiRes;

/// `RequestBattlefieldScoreData`'s throttle (`0x54ce30`).
const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// The rows' pending names (`0xbea590`) and the finished flag (`0xbea588`).
#[derive(Resource, Default)]
pub(crate) struct BattlefieldScoreboard {
    /// The guids whose answer the rebuild waits on: one callback each (`0x54d460`).
    pending: HashSet<u64>,
    ended: bool,
}

impl BattlefieldScoreboard {
    /// Whether a row's name is still being asked for, which holds the rebuild (`0x54aa39`).
    pub(crate) fn names_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// The next log update (`0xbea584`): the next request's earliest moment, `None` until the first.
#[derive(Resource, Default)]
pub(crate) struct ScoreRequestClock(Option<Instant>);

/// The column headers for `map`: the first contiguous run of matching `WorldStateUI.dbc` rows in
/// table order, as the status-3 arm builds them (`0x54b026`).
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

/// A row's name record as the scoreboard reads it (`0x54be90`), a held unit's race and class standing in.
pub(crate) fn score_name(
    names: &NameCache,
    guid: u64,
    unit_traits: impl Fn(u64) -> Option<(u8, u8)>,
) -> Option<ScoreName> {
    let name = names.peek(guid).filter(|n| !n.is_empty())?;
    let (race, class) = names
        .player_traits(guid)
        .map(|(race, class, _)| (race, class))
        .or_else(|| unit_traits(guid))
        .unwrap_or((0, 0));
    Some(ScoreName {
        name: name.to_string(),
        // `"%s-%s"` with a realm (`0x54b58a`).
        full_name: match names.player_realm(guid) {
            Some(realm) => format!("{name}-{realm}"),
            None => name.to_string(),
        },
        race: crate::ui_unit::race_names(race).map(|(n, _)| n.to_string()),
        class: crate::ui_unit::class_names(class).map(|(n, t)| (n.to_string(), t.to_string())),
        class_id: u32::from(class),
        side: i32::from(crate::ui_unit::race_pvp_team(race)),
    })
}

/// The log for the VM's board: an arena row's team byte, a battleground's side left to the rebuild.
fn board_log(log: PvpLogData) -> BattlefieldScores {
    let rows = log
        .rows
        .into_iter()
        .map(|r| {
            let mut stats = [0u32; 8];
            for (slot, v) in stats.iter_mut().zip(&r.stats) {
                *slot = *v;
            }
            BattlefieldScoreRow {
                guid: r.guid,
                killing_blows: r.killing_blows,
                honorable_kills: r.honorable_kills,
                deaths: r.deaths,
                bonus_honor: r.bonus_honor,
                faction: r.arena_team.map_or(0, i32::from),
                damage_done: r.damage_done,
                healing_done: r.healing_done,
                stats,
            }
        })
        .collect();
    BattlefieldScores {
        rows,
        ended: log.ended,
        winner: log.winner.unwrap_or(0),
        teams: log.arena_teams.map(|t| {
            t.map(|t| BattlefieldTeamView {
                name: t.name,
                rating: t.rating,
                new_rating: t.new_rating,
                matchmaking: t.matchmaking,
            })
        }),
    }
}

/// The score request (`0x54ce30`): once asked, a finished match's or an arena's board is not
/// asked again; otherwise once per 5000 ms.
fn drain_battlefield_score(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    mut clock: ResMut<ScoreRequestClock>,
    board: Res<BattlefieldScoreboard>,
    queue: Res<BattlefieldQueue>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if script.take_battlefield_score_requests() == 0 {
        return;
    }
    // The arena test reads the instance type (`0x54ce4d`).
    if clock.0.is_some() && (board.ended || queue.in_arena()) {
        return;
    }
    let now = Instant::now();
    if clock.0.is_none_or(|t| now >= t) {
        clock.0 = Some(now + REQUEST_THROTTLE);
        let _ = commands.0.send(ClientCommand::RequestBattlefieldScoreData);
    }
}

/// The world entry's board part (`0x54ca50`): clock, finished flag and pending names (`0x54cad9`, `0x54cb03`).
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut board) = world.get_resource_mut::<BattlefieldScoreboard>() {
        *board = BattlefieldScoreboard::default();
    }
    if let Some(mut clock) = world.get_resource_mut::<ScoreRequestClock>() {
        clock.0 = None;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.write(UiScript::reset_battlefield_scores_on_world_enter);
    }
}

/// The scoreboard's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{board_log, BattlefieldScoreboard, ScoreRequestClock};
    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp};
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PvpLogData, on_pvp_log_data)
            .net_handler(K::PlayerName, on_name_answer)
            .net_handler(K::PlayerNameMiss, on_name_answer)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// Zeroes the board and the stamp, as the reference's module init does at every login.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut board: ResMut<BattlefieldScoreboard>,
        mut clock: ResMut<ScoreRequestClock>,
    ) {
        *board = BattlefieldScoreboard::default();
        clock.0 = None;
    }

    /// `MSG_PVP_LOG_DATA` (`0x54d280`): the log written, uncached names asked (`0x54d469`), then the rebuild with none pending (`0x54d495`).
    pub(super) fn on_pvp_log_data(
        In(ev): In<SessionEvent>,
        mut board: ResMut<BattlefieldScoreboard>,
        mut names: ResMut<NameCache>,
        commands: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::PvpLogData(data) = ev else {
            return;
        };
        board.pending.clear();
        board.ended = data.ended;
        for row in &data.rows {
            if names.answered_unknown(row.guid) {
                names.invalidate_player(row.guid);
            }
            if names.resolve(row.guid, &commands).is_none() {
                board.pending.insert(row.guid);
            }
        }
        let log = board_log(data);
        ui.write(move |s| s.set_battlefield_log(log));
        if !board.names_pending() {
            ui.write(|s| s.rebuild_battlefield_scores());
        }
    }

    /// A name answer counts its rows down, and the last runs the rebuild (`0x54ca30`); a retry still waits (`0x67a360`).
    fn on_name_answer(
        In(ev): In<SessionEvent>,
        mut board: ResMut<BattlefieldScoreboard>,
        mut ui: ResMut<UiEvents>,
    ) {
        use benilla_protocol::messages::NameQueryMiss;
        let guid = match ev {
            SessionEvent::PlayerName { guid, .. } => guid,
            SessionEvent::PlayerNameMiss {
                miss: NameQueryMiss::Retry,
                ..
            } => return,
            SessionEvent::PlayerNameMiss { guid, .. } => guid,
            _ => return,
        };
        if !board.pending.remove(&guid) {
            return;
        }
        if !board.names_pending() {
            ui.write(|s| s.rebuild_battlefield_scores());
        }
    }
}

/// What the verbs read live: the name cache and the pending count.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScoreView<'w> {
    board: Option<Res<'w, BattlefieldScoreboard>>,
    names: Option<Res<'w, NameCache>>,
}

impl ScoreView<'_> {
    pub(crate) fn names_pending(&self) -> bool {
        self.board.as_ref().is_some_and(|b| b.names_pending())
    }

    pub(crate) fn name(
        &self,
        guid: u64,
        unit_traits: impl Fn(u64) -> Option<(u8, u8)>,
    ) -> Option<ScoreName> {
        score_name(self.names.as_deref()?, guid, unit_traits)
    }
}

pub(crate) struct BattlefieldScorePlugin;

impl Plugin for BattlefieldScorePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BattlefieldScoreboard>()
            .init_resource::<ScoreRequestClock>()
            .add_systems(Update, drain_battlefield_score.in_set(UiDrain));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{WorldStateUiCatalog, WorldStateUiRow};

    fn wotlk_score_frame() -> UiScript {
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        script.register_cvars(crate::cvars::registered_pairs());
        let mut errors = Vec::new();
        for entry in crate::ui_script::test_ui::production_order(&[&[
            r"Interface\FrameXML\GlobalStrings.lua",
            r"Interface\FrameXML\Fonts.xml",
            r"Interface\FrameXML\FontStyles.xml",
            r"Interface\FrameXML\BasicControls.xml",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\GameTooltip.xml",
            r"Interface\FrameXML\UIPanelTemplates.lua",
            r"Interface\FrameXML\UIPanelTemplates.xml",
            r"Interface\FrameXML\ChatFrame.lua",
            r"Interface\FrameXML\UIDropDownMenu.xml",
            r"Interface\FrameXML\WorldStateFrame.xml",
        ]]) {
            let path = entry.replace('\\', "/");
            let bytes = crate::ui_script::test_ui::read(&path)
                .unwrap_or_else(|| panic!("{entry}: not found"));
            if path.to_ascii_lowercase().ends_with(".lua") {
                if let Err(e) = script.run_chunk_named(&bytes, &format!("@{entry}")) {
                    errors.push(e.to_string());
                }
                continue;
            }
            let doc = benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes))
                .unwrap_or_else(|e| panic!("{entry}: {e}"));
            let provider = |req: &str| crate::ui_script::test_ui::read(req);
            errors.extend(benilla_ui::loader::load_in(&script, &doc, &path, &provider).errors);
        }
        errors.extend(script.take_errors());
        let score_errors: Vec<_> = errors.iter().filter(|e| e.contains("WorldState")).collect();
        assert!(
            score_errors.is_empty(),
            "score frame load: {score_errors:?}"
        );
        script
    }

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

    fn app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<NameCache>()
            .init_resource::<BattlefieldScoreboard>()
            .init_resource::<ScoreRequestClock>();
        crate::names::net::register(&mut app);
        net::register(&mut app);
        (app, rx)
    }

    fn log(guids: &[u64]) -> PvpLogData {
        PvpLogData {
            ended: true,
            winner: Some(1),
            rows: guids
                .iter()
                .map(|&guid| benilla_protocol::messages::PvpLogRow {
                    guid,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn name_answer(guid: u64) -> benilla_protocol::SessionEvent {
        benilla_protocol::SessionEvent::PlayerName {
            guid,
            name: format!("P{guid:x}"),
            realm: String::new(),
            race: 1,
            class: 1,
            gender: 0,
            declensions: None,
        }
    }

    /// `0x54d280` counts each uncached row; each answer counts down and the last ends the wait (`0x54ca30`).
    #[test]
    fn the_rebuild_waits_for_the_last_name() {
        let (mut app, rx) = app();
        app.world_mut().resource_mut::<NameCache>().insert_player(
            0x11,
            "Known".into(),
            Some((1, 1, 0)),
        );
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::PvpLogData(log(&[
                0x11, 0x12, 0x13,
            ]))],
        );
        let asked: Vec<_> = rx.try_iter().collect();
        assert_eq!(asked.len(), 2, "one query per uncached row: {asked:?}");
        assert!(app
            .world()
            .resource::<BattlefieldScoreboard>()
            .names_pending());
        assert!(app.world().resource::<BattlefieldScoreboard>().ended);

        for ev in [
            name_answer(0x99),
            benilla_protocol::SessionEvent::PlayerNameMiss {
                guid: 0x12,
                miss: benilla_protocol::messages::NameQueryMiss::Retry,
            },
            name_answer(0x12),
        ] {
            crate::net::handlers::dispatch(app.world_mut(), vec![ev]);
            assert!(app
                .world()
                .resource::<BattlefieldScoreboard>()
                .names_pending());
        }
        // A deny counts down as a name does (`0x67a32d` -> `0x54ca30`).
        let deny = || benilla_protocol::SessionEvent::PlayerNameMiss {
            guid: 0x13,
            miss: benilla_protocol::messages::NameQueryMiss::Deny,
        };
        crate::net::handlers::dispatch(app.world_mut(), vec![deny()]);
        assert!(!app
            .world()
            .resource::<BattlefieldScoreboard>()
            .names_pending());

        // The next log asks the denied row again and waits for it, then the deny ends the wait.
        rx.try_iter().for_each(drop);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::PvpLogData(log(&[
                0x11, 0x12, 0x13,
            ]))],
        );
        let asked: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(asked.as_slice(), [ClientCommand::NameQuery { guid: 0x13 }]),
            "the denied row asks again: {asked:?}"
        );
        assert!(app
            .world()
            .resource::<BattlefieldScoreboard>()
            .names_pending());
        crate::net::handlers::dispatch(app.world_mut(), vec![deny()]);
        assert!(!app
            .world()
            .resource::<BattlefieldScoreboard>()
            .names_pending());
    }

    #[test]
    fn the_session_end_zeroes_the_scoreboard() {
        let (mut app, _rx) = app();
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::PvpLogData(log(&[0x12]))],
        );
        app.world_mut().resource_mut::<ScoreRequestClock>().0 = Some(Instant::now());

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let board = app.world().resource::<BattlefieldScoreboard>();
        assert!(!board.names_pending());
        assert!(!board.ended);
        assert!(app.world().resource::<ScoreRequestClock>().0.is_none());
    }

    /// `0x54ca50` zeroes the result, rows and counts and fires no score event.
    #[test]
    fn world_entry_drops_an_ended_board() {
        benilla_formats::wow_data_or_skip!();
        let mut script = wotlk_score_frame();
        script.set_battlefield_log(BattlefieldScores {
            ended: true,
            winner: 1,
            rows: vec![BattlefieldScoreRow {
                guid: 0x11,
                ..Default::default()
            }],
            ..Default::default()
        });
        script.rebuild_battlefield_scores();
        assert!(script
            .eval::<bool>("return WorldStateScoreFrame:IsShown()")
            .unwrap());
        assert_eq!(
            script.eval::<i64>("return GetBattlefieldWinner()").unwrap(),
            1
        );

        let mut world = World::new();
        world.init_resource::<UiEvents>();
        world.init_resource::<BattlefieldScoreboard>();
        world.insert_resource(ScoreRequestClock(Some(Instant::now())));
        enter_world(&mut world);
        assert!(world.resource::<ScoreRequestClock>().0.is_none());
        script.reset_battlefield_scores_on_world_enter();
        assert!(script
            .eval::<bool>(
                "return GetBattlefieldWinner() == nil and GetNumBattlefieldScores() == 0 \
                 and GetBattlefieldScore(1) == nil",
            )
            .unwrap());
        let errors = script.take_errors();
        assert!(
            errors.iter().all(|e| !e.contains("WorldState")),
            "score frame errors: {errors:?}"
        );
    }
}
