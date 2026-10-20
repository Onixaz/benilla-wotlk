//! The app half of `CGArenaTeamInfo` (`0x5a40e0` and its handlers): the player's three arena-team
//! slots off `PLAYER_FIELD_ARENA_TEAM_INFO_1_1`, the team cache (`DBCache<ArenaTeamCache>`), the
//! last stats and roster of each team, the result, event and invitation lines, and the inspected
//! player's teams. The VM sorts the roster and owns its selection.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::{
    ArenaTeamCommandResult, ArenaTeamEvent, ArenaTeamMember, ArenaTeamRecord, InspectArenaTeam,
};
use benilla_ui::script::{
    ArenaTeamMemberView, ArenaTeamRecordView, ArenaTeamRequest, ArenaTeamSlotView,
    InspectArenaTeamView, ScriptValue, UiScript,
};

use crate::net::{ArenaTeamCommand, ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::QueryCache;
use crate::ui_script::{UiFeed, UiInput};

/// `QueryRoster`'s throttle per team (`0x5a3600`).
const ROSTER_THROTTLE: Duration = Duration::from_secs(10);

/// One team slot's app state (`ArenaTeam`, `0x38` bytes).
#[derive(Clone, Debug, Default, PartialEq)]
struct TeamSlot {
    team_id: u32,
    /// Rating, week games and wins, season games and wins, rank.
    stats: [u32; 6],
    members: Vec<ArenaTeamMember>,
    /// `m_nextAllowedQuery` (`0xc0f7f0`).
    next_query: Option<Instant>,
}

/// The inspected player's teams by slot, the reply's words as they came.
#[derive(Default)]
struct InspectTeams {
    guid: Option<u64>,
    slots: [Option<InspectArenaTeam>; 3],
    /// A reply landed; `INSPECT_HONOR_UPDATE` is owed (`0x5e7bd4`).
    fresh: bool,
}

#[derive(Resource, Default)]
pub(crate) struct ArenaTeams {
    /// The team cache by team id, asked once.
    records: QueryCache<u32, ArenaTeamRecord>,
    slots: [TeamSlot; 3],
    /// The field words as last seen; `None` before the first read after a world enter.
    info: Option<[[u32; 7]; 3]>,
    events: Vec<(&'static str, Vec<ScriptValue>)>,
    /// GlobalStrings keys and their arguments, shown on the feed.
    lines: Vec<(&'static str, Vec<String>)>,
    inspect: InspectTeams,
}

impl crate::query_cache::AskOnce for ArenaTeams {
    fn clear_pending(&mut self) {
        self.records.clear_pending();
    }
}

impl ArenaTeams {
    /// A held team of `size` whose record is cached: the arena charter purchase's refusal
    /// (`0x5a23e0`).
    pub(crate) fn holds_team_of_size(&self, size: u32) -> bool {
        self.slots.iter().any(|s| {
            s.team_id != 0
                && self
                    .records
                    .get(s.team_id)
                    .is_some_and(|r| r.team_type == size)
        })
    }

    fn slot_of(&mut self, team_id: u32) -> Option<&mut TeamSlot> {
        self.slots
            .iter_mut()
            .find(|s| s.team_id == team_id && team_id != 0)
    }

    /// `UpdateTeams` (`0x5a3700`) over freshly read field words: a team that left its slot drops
    /// out of the cache, every held team asks its roster, and a change fires `ARENA_TEAM_UPDATE`.
    fn update_teams(&mut self, info: [[u32; 7]; 3], now: Instant, commands: &NetCommands) {
        let signal = self.info.is_some();
        for (i, words) in info.iter().enumerate() {
            let old = self.slots[i].team_id;
            if old != 0 && words[0] == 0 {
                self.records.evict(old);
            }
            if old != words[0] {
                self.slots[i] = TeamSlot {
                    team_id: words[0],
                    ..Default::default()
                };
            }
            if words[0] != 0 {
                self.query_roster(i, now, commands);
            }
        }
        self.info = Some(info);
        if signal {
            self.events.push(("ARENA_TEAM_UPDATE", Vec::new()));
        }
    }

    /// `QueryRoster` (`0x5a3600`): at most one ask per team per 10 s.
    fn query_roster(&mut self, slot: usize, now: Instant, commands: &NetCommands) {
        let s = &mut self.slots[slot];
        if s.team_id == 0 || s.next_query.is_some_and(|t| now < t) {
            return;
        }
        s.next_query = Some(now + ROSTER_THROTTLE);
        let _ = commands
            .0
            .send(ClientCommand::ArenaTeam(ArenaTeamCommand::Roster {
                team_id: s.team_id,
            }));
    }

    fn apply_record(&mut self, record: ArenaTeamRecord) {
        let id = record.team_id;
        self.records.insert(id, Some(record));
        // `ArenaTeamCacheCallback` (`0x5a28c0`).
        self.events.push(("ARENA_TEAM_UPDATE", Vec::new()));
    }

    /// `SMSG_ARENA_TEAM_STATS` (`0x5a2d50`): dropped for a team in no slot.
    fn apply_stats(&mut self, st: benilla_protocol::messages::ArenaTeamStats) {
        let Some(slot) = self.slot_of(st.team_id) else {
            return;
        };
        slot.stats = [
            st.rating,
            st.week_games,
            st.week_wins,
            st.season_games,
            st.season_wins,
            st.rank,
        ];
        self.events.push(("ARENA_TEAM_UPDATE", Vec::new()));
    }

    /// `SMSG_ARENA_TEAM_ROSTER` (`0x5a3e10`): dropped for a team in no slot.
    fn apply_roster(&mut self, roster: benilla_protocol::messages::ArenaTeamRoster) {
        let Some(slot) = self.slot_of(roster.team_id) else {
            return;
        };
        slot.members = roster.members;
        self.events.push(("ARENA_TEAM_ROSTER_UPDATE", Vec::new()));
    }

    /// `UpdateRoster(i, 1)` for every slot (`0x5a28e0`), after a membership event: the throttle
    /// resets and the roster event fires with its argument 1.
    fn roster_changed(&mut self) {
        for s in &mut self.slots {
            s.next_query = None;
            self.events
                .push(("ARENA_TEAM_ROSTER_UPDATE", vec![ScriptValue::Int(1)]));
        }
    }

    /// `SMSG_ARENA_TEAM_COMMAND_RESULT`'s line (`0x6ccb20`).
    fn command_result(&mut self, r: ArenaTeamCommandResult) {
        let (team, player) = (r.team, r.player);
        let line: Option<(&'static str, Vec<String>)> = if r.error == 0 {
            match r.command {
                0 => Some(("ERR_ARENA_TEAM_CREATE_S", vec![team])),
                1 => Some(("ERR_ARENA_TEAM_INVITE_SS", vec![player, team])),
                3 => Some(("ERR_ARENA_TEAM_QUIT_S", vec![team])),
                14 => Some(("ERR_ARENA_TEAM_FOUNDER_S", vec![team])),
                _ => None,
            }
        } else {
            match r.error {
                1 => Some(("ERR_ARENA_TEAM_INTERNAL", vec![])),
                2 => Some(("ERR_ALREADY_IN_ARENA_TEAM", vec![])),
                3 => Some(("ERR_ALREADY_IN_ARENA_TEAM_S", vec![player])),
                4 => Some(("ERR_INVITED_TO_ARENA_TEAM", vec![])),
                5 => Some(("ERR_ALREADY_INVITED_TO_ARENA_TEAM_S", vec![player])),
                6 => Some(("ERR_ARENA_TEAM_NAME_INVALID", vec![])),
                7 => Some(("ERR_ARENA_TEAM_NAME_EXISTS_S", vec![team])),
                8 if r.command == 3 => Some(("ERR_ARENA_TEAM_LEADER_LEAVE_S", vec![team])),
                8 => Some(("ERR_ARENA_TEAM_PERMISSIONS", vec![])),
                9 => Some(("ERR_ARENA_TEAM_PLAYER_NOT_IN_TEAM", vec![])),
                10 => Some(("ERR_ARENA_TEAM_PLAYER_NOT_IN_TEAM_SS", vec![player, team])),
                11 => Some(("ERR_ARENA_TEAM_PLAYER_NOT_FOUND_S", vec![player])),
                12 => Some(("ERR_ARENA_TEAM_NOT_ALLIED", vec![])),
                19 => Some(("ERR_IGNORING_YOU_S", vec![player])),
                21 => Some(("ERR_ARENA_TEAM_TARGET_TOO_LOW_S", vec![player])),
                22 => Some(("ERR_ARENA_TEAM_TARGET_TOO_HIGH_S", vec![player])),
                23 => Some(("ERR_ARENA_TEAM_TOO_MANY_MEMBERS_S", vec![team])),
                27 => Some(("ERR_ARENA_TEAM_NOT_FOUND", vec![])),
                30 => Some(("ERR_ARENA_TEAMS_LOCKED", vec![])),
                _ => None,
            }
        };
        self.lines.extend(line);
    }

    /// `SMSG_ARENA_TEAM_EVENT`'s line (`0x6cc980`): a join names us apart, then every roster
    /// re-asks.
    fn event(&mut self, e: ArenaTeamEvent, my_name: Option<&str>) {
        let mut strings = e.strings;
        let key = match e.event {
            3 => {
                let me = strings
                    .first()
                    .is_some_and(|s| my_name.is_some_and(|n| n.eq_ignore_ascii_case(s)));
                if me {
                    strings = strings.get(1).cloned().into_iter().collect();
                    "ERR_ARENA_TEAM_YOU_JOIN_S"
                } else {
                    "ERR_ARENA_TEAM_JOIN_SS"
                }
            }
            4 => "ERR_ARENA_TEAM_LEAVE_SS",
            5 => "ERR_ARENA_TEAM_REMOVE_SSS",
            6 => "ERR_ARENA_TEAM_LEADER_IS_SS",
            7 => "ERR_ARENA_TEAM_LEADER_CHANGED_SSS",
            8 => "ERR_ARENA_TEAM_DISBANDED_S",
            _ => "ERR_GUILD_INTERNAL",
        };
        strings.truncate(3);
        self.lines.push((key, strings));
        self.roster_changed();
    }

    /// `SMSG_ARENA_TEAM_INVITE` (`0x6cc910`): the popup's event and the chat line.
    fn invite(&mut self, inviter: String, team: String) {
        self.events.push((
            "ARENA_TEAM_INVITE_REQUEST",
            vec![
                ScriptValue::Str(inviter.clone()),
                ScriptValue::Str(team.clone()),
            ],
        ));
        self.lines.push((
            "ERR_INVITED_TO_GUILD_SSS",
            vec![inviter.clone(), inviter, team],
        ));
    }

    fn record_view(r: &ArenaTeamRecord) -> ArenaTeamRecordView {
        ArenaTeamRecordView {
            name: r.name.clone(),
            team_type: r.team_type,
            background_color: r.background_color,
            emblem_style: r.emblem_style,
            emblem_color: r.emblem_color,
            border_style: r.border_style,
            border_color: r.border_color,
        }
    }

    /// The team cache's record for `team_id`, asked once.
    fn record(&self, team_id: u32, commands: &NetCommands) -> Option<&ArenaTeamRecord> {
        self.records.get_or_ask(team_id, || {
            let _ = commands
                .0
                .send(ClientCommand::ArenaTeam(ArenaTeamCommand::Query {
                    team_id,
                }));
        })
    }

    /// The pushes for the VM's three slots.
    fn slot_views(&self, info: [[u32; 7]; 3], commands: &NetCommands) -> [ArenaTeamSlotView; 3] {
        std::array::from_fn(|i| {
            let s = &self.slots[i];
            let record = (info[i][0] != 0)
                .then(|| self.record(info[i][0], commands))
                .flatten()
                .map(Self::record_view);
            ArenaTeamSlotView {
                info: info[i],
                record,
                stats: s.stats,
                members: s
                    .members
                    .iter()
                    .map(|m| ArenaTeamMemberView {
                        guid: m.guid,
                        name: m.name.clone(),
                        rank: m.rank,
                        level: u32::from(m.level),
                        class: crate::ui_unit::class_names(m.class).map(|(n, _)| n.to_string()),
                        online: m.online,
                        week_games: m.week_games,
                        week_wins: m.week_wins,
                        season_games: m.season_games,
                        season_wins: m.season_wins,
                        personal_rating: m.personal_rating,
                        matchmaking: m.matchmaking,
                    })
                    .collect(),
            }
        })
    }

    /// `GetInspectArenaTeamData`'s views: a slot answers once the cache has its team.
    fn inspect_views(&self, commands: &NetCommands) -> [Option<InspectArenaTeamView>; 3] {
        std::array::from_fn(|i| {
            let t = self.inspect.slots[i].as_ref().filter(|t| t.team_id != 0)?;
            let r = self.record(t.team_id, commands)?;
            let rgb = |c: u32| [16, 8, 0].map(|s| f64::from((c >> s) & 0xFF) * 0.003_921_568_9);
            Some(InspectArenaTeamView {
                name: r.name.clone(),
                team_size: r.team_type,
                rating: t.rating,
                games: t.games,
                wins: t.wins,
                played: t.played,
                personal_rating: t.personal_rating,
                background: rgb(r.background_color),
                emblem: r.emblem_style,
                emblem_color: rgb(r.emblem_color),
                border: r.border_style,
                border_color: rgb(r.border_color),
            })
        })
    }
}

/// The three slots' field words, `None` before our descriptor streams.
fn read_info(store: &ObjectStore) -> Option<[[u32; 7]; 3]> {
    store.0.player_arena_team_info(0, 0)?;
    Some(std::array::from_fn(|slot| {
        std::array::from_fn(|word| {
            store
                .0
                .player_arena_team_info(slot as u16, word as u16)
                .unwrap_or(0)
        })
    }))
}

#[allow(clippy::too_many_arguments)]
fn feed_arena_teams(
    script: Option<NonSendMut<UiScript>>,
    mut teams: ResMut<ArenaTeams>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    inspect_target: Res<crate::ui_inspect::InspectTarget>,
    selection: Res<crate::target::Selection>,
    group: Res<crate::ui_party::GroupState>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    mut memo: Local<crate::ui_script::VmMemo<Option<[Option<InspectArenaTeamView>; 3]>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();
    if let Some(info) = self_q.iter().next().and_then(read_info) {
        if teams.info != Some(info) {
            teams.update_teams(info, now, &commands);
        }
        let views = teams.slot_views(info, &commands);
        script.set_arena_teams(views);
    }

    // The inspected player's teams clear with the inspect target, as `ClearInspect` does.
    let inspected = inspect_target
        .token
        .as_deref()
        .and_then(|t| crate::ui_unit::player_token_guid(t, &selection, &group));
    if teams.inspect.guid.is_some() && teams.inspect.guid != inspected {
        teams.inspect = InspectTeams::default();
    }
    let views = teams.inspect_views(&commands);
    let last = memo.get(&script);
    if last.as_ref() != Some(&views) {
        *last = Some(views.clone());
        script.set_inspect_arena_teams(views);
    }
    if std::mem::take(&mut teams.inspect.fresh) {
        script.fire_event("INSPECT_HONOR_UPDATE", Vec::new());
    }

    for (name, args) in std::mem::take(&mut teams.events) {
        script.fire_event(name, args);
    }
    let lines: Vec<_> = std::mem::take(&mut teams.lines)
        .into_iter()
        .filter_map(|(key, args)| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            crate::ui_action::keyed_line_s(&script, key, &args)
        })
        .collect();
    if !lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_arena_team", lines);
    }
}

/// The VM's sends; the roster ask goes through the throttle.
fn drain_arena_teams(
    script: Option<NonSendMut<UiScript>>,
    mut teams: ResMut<ArenaTeams>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();
    for r in script.take_arena_team_requests() {
        let c = match r {
            ArenaTeamRequest::Roster { team_id } => {
                if let Some(i) = teams.slots.iter().position(|s| s.team_id == team_id) {
                    teams.query_roster(i, now, &commands);
                }
                continue;
            }
            ArenaTeamRequest::Invite { team_id, name } => {
                ArenaTeamCommand::Invite { team_id, name }
            }
            ArenaTeamRequest::Remove { team_id, name } => {
                ArenaTeamCommand::Remove { team_id, name }
            }
            ArenaTeamRequest::Leader { team_id, name } => {
                ArenaTeamCommand::Leader { team_id, name }
            }
            ArenaTeamRequest::Leave { team_id } => ArenaTeamCommand::Leave { team_id },
            ArenaTeamRequest::Disband { team_id } => ArenaTeamCommand::Disband { team_id },
            ArenaTeamRequest::Accept => ArenaTeamCommand::Accept,
            ArenaTeamRequest::Decline => ArenaTeamCommand::Decline,
        };
        let _ = commands.0.send(ClientCommand::ArenaTeam(c));
    }
}

/// The world enter re-reads the slots without an event (`EnterWorld`, `0x5a3dc0`).
fn reset_on_world_enter(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut teams: ResMut<ArenaTeams>,
) {
    if cascades.read().next().is_some() {
        teams.info = None;
    }
}

mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::ArenaTeams;
    use crate::names::NameCache;
    use crate::net::{NetHandlerApp, SelfGuid};

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::ArenaTeamQueryResponse, on_packet)
            .net_handler(K::ArenaTeamRoster, on_packet)
            .net_handler(K::ArenaTeamStats, on_packet)
            .net_handler(K::ArenaTeamCommandResult, on_packet)
            .net_handler(K::ArenaTeamEvent, on_packet)
            .net_handler(K::ArenaTeamInvite, on_packet)
            .net_handler(K::InspectArenaTeam, on_packet)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// `ShutdownGame` frees every team (`0x5a3dd0`); the cache is the session's.
    fn on_session_end(In(_): In<SessionEvent>, mut teams: ResMut<ArenaTeams>) {
        *teams = ArenaTeams::default();
    }

    fn on_packet(
        In(ev): In<SessionEvent>,
        mut teams: ResMut<ArenaTeams>,
        names: Res<NameCache>,
        me: Res<SelfGuid>,
    ) {
        match ev {
            SessionEvent::ArenaTeamQueryResponse(r) => teams.apply_record(r),
            SessionEvent::ArenaTeamRoster(r) => teams.apply_roster(r),
            SessionEvent::ArenaTeamStats(s) => teams.apply_stats(s),
            SessionEvent::ArenaTeamCommandResult(r) => teams.command_result(r),
            SessionEvent::ArenaTeamEvent(e) => {
                let mine = me.0.and_then(|g| names.peek(g)).map(str::to_string);
                teams.event(e, mine.as_deref());
            }
            SessionEvent::ArenaTeamInvite { inviter, team } => teams.invite(inviter, team),
            SessionEvent::InspectArenaTeam(t) if usize::from(t.slot) < 3 => {
                if teams.inspect.guid != Some(t.player_guid) {
                    teams.inspect = super::InspectTeams {
                        guid: Some(t.player_guid),
                        ..Default::default()
                    };
                }
                teams.inspect.slots[usize::from(t.slot)] = Some(t);
                teams.inspect.fresh = true;
            }
            _ => {}
        }
    }
}

pub(crate) struct UiArenaTeamPlugin;

impl Plugin for UiArenaTeamPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<ArenaTeams>(app);
        app.init_resource::<ArenaTeams>().add_systems(
            Update,
            (
                // After the net stage, where the cascade is written and the cache releases.
                reset_on_world_enter
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(feed_arena_teams),
                feed_arena_teams
                    .in_set(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
                drain_arena_teams.after(UiInput),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = unbounded();
        (NetCommands(tx), rx)
    }

    fn info(team: u32) -> [[u32; 7]; 3] {
        let mut i = [[0u32; 7]; 3];
        i[0] = [team, 2, 0, 0, 0, 0, 1500];
        i
    }

    #[test]
    fn a_team_in_a_slot_asks_its_roster_once_per_ten_seconds() {
        let (commands, rx) = commands();
        let mut t = ArenaTeams::default();
        let now = Instant::now();
        t.update_teams(info(9), now, &commands);
        assert!(t.events.is_empty(), "the first read fires nothing");
        t.query_roster(0, now + Duration::from_secs(5), &commands);
        t.query_roster(0, now + Duration::from_secs(11), &commands);
        let sent: Vec<_> = rx.try_iter().collect();
        assert_eq!(sent.len(), 2, "the first ask and the one past the throttle");
        t.update_teams(info(0), now, &commands);
        assert_eq!(t.events.len(), 1, "a change fires ARENA_TEAM_UPDATE");
        assert_eq!(t.slots[0].team_id, 0);
    }

    #[test]
    fn stats_and_rosters_land_only_on_a_held_team() {
        let (commands, _rx) = commands();
        let mut t = ArenaTeams::default();
        t.update_teams(info(9), Instant::now(), &commands);
        t.apply_stats(benilla_protocol::messages::ArenaTeamStats {
            team_id: 4,
            rating: 1,
            ..Default::default()
        });
        assert_eq!(t.slots[0].stats, [0; 6], "another team's stats are dropped");
        t.apply_stats(benilla_protocol::messages::ArenaTeamStats {
            team_id: 9,
            rating: 1520,
            week_games: 3,
            ..Default::default()
        });
        assert_eq!(t.slots[0].stats[..2], [1520, 3]);
        t.event(
            ArenaTeamEvent {
                event: 3,
                strings: vec!["Claude".into(), "Probes".into()],
                guid: Some(1),
            },
            Some("claude"),
        );
        assert_eq!(
            t.lines.last(),
            Some(&("ERR_ARENA_TEAM_YOU_JOIN_S", vec!["Probes".to_string()]))
        );
    }
}
