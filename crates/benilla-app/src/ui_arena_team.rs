//! The app half of the reference's arena-team info (`0x5a40e0` and its handlers): the player's
//! three arena-team slots off `PLAYER_FIELD_ARENA_TEAM_INFO_1_1`, the team cache, the last stats
//! and roster of each team, the result, event and invitation lines, and the inspected player's
//! teams. The verbs read the slots live ([`ArenaTeamView`]); each handler fires its events as it
//! lands. The VM sorts the roster and owns its selection.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::{
    ArenaTeamCommandResult, ArenaTeamEvent, ArenaTeamMember, ArenaTeamRecord,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{ArenaTeamMemberRef, ArenaTeamRecordView, ArenaTeamRequest};

use crate::net::{ArenaTeamCommand, ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::QueryCache;
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;

/// The roster query's throttle per slot (`0x5a3600`).
const ROSTER_THROTTLE: Duration = Duration::from_secs(10);

/// One team slot's table entry (the reference's `0x38`-byte team record at `0xc0f840`).
#[derive(Clone, Debug, Default, PartialEq)]
struct TeamSlot {
    team_id: u32,
    /// Rating, week games and wins, season games and wins, rank.
    stats: [u32; 6],
    /// The last roster's team type (`0x5a3ea4`), which the rating check compares (`0x5a3823`).
    roster_type: u32,
    members: Vec<ArenaTeamMember>,
}

#[derive(Resource, Default)]
pub(crate) struct ArenaTeams {
    /// The team cache by team id, asked once.
    records: QueryCache<u32, ArenaTeamRecord>,
    slots: [TeamSlot; 3],
}

/// Each slot's next allowed roster query (`0xc0f7f0`), apart from the table the verbs read.
#[derive(Resource, Default)]
pub(crate) struct RosterThrottle([Option<Instant>; 3]);

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

    /// The team update (`0x5a3700`): a team gone from its slot leaves the cache (`0x5a3763`), the slot takes the field's id, held teams ask their roster.
    fn update_teams(
        &mut self,
        info: &[[u32; 7]; 3],
        throttle: &mut RosterThrottle,
        now: Instant,
        commands: &NetCommands,
    ) {
        for (i, words) in info.iter().enumerate() {
            let old = self.slots[i].team_id;
            if old != 0 && words[0] == 0 {
                self.records.evict(old);
            }
            self.slots[i].team_id = words[0];
            if words[0] != 0 {
                query_roster(i, info, throttle, now, commands);
            }
        }
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
}

/// A roster member borrowed as the verbs read it.
fn member_ref(m: &ArenaTeamMember) -> ArenaTeamMemberRef<'_> {
    ArenaTeamMemberRef {
        guid: m.guid,
        name: &m.name,
        rank: m.rank,
        level: u32::from(m.level),
        class: crate::ui_unit::class_names(m.class).map(|(n, _)| n),
        online: m.online,
        week_games: m.week_games,
        week_wins: m.week_wins,
        season_games: m.season_games,
        season_wins: m.season_wins,
        personal_rating: m.personal_rating,
        matchmaking: m.matchmaking,
    }
}

/// The roster query (`0x5a3600`): the slot's team id, at most one ask per slot per 10 s.
fn query_roster(
    slot: usize,
    info: &[[u32; 7]; 3],
    throttle: &mut RosterThrottle,
    now: Instant,
    commands: &NetCommands,
) {
    let team_id = info[slot][0];
    if team_id == 0 || throttle.0[slot].is_some_and(|t| now < t) {
        return;
    }
    throttle.0[slot] = Some(now + ROSTER_THROTTLE);
    let _ = commands
        .0
        .send(ClientCommand::ArenaTeam(ArenaTeamCommand::Roster {
            team_id,
        }));
}

/// The three slots' field words, `None` before our descriptor streams.
fn read_info(fields: &ObjectFields) -> Option<[[u32; 7]; 3]> {
    fields.player_arena_team_info(0, 0)?;
    Some(std::array::from_fn(|slot| {
        std::array::from_fn(|word| {
            fields
                .player_arena_team_info(slot as u16, word as u16)
                .unwrap_or(0)
        })
    }))
}

/// Whether `index` is one of our arena-team field words, the 0x54-byte watch at `0x6e570f`.
pub(crate) fn is_team_info(index: u16) -> bool {
    use benilla_protocol::field::FIELD_PLAYER_FIELD_ARENA_TEAM_INFO_1_1 as FIRST;
    (FIRST..FIRST + 21).contains(&index)
}

/// Our team words' watch (`0x6cdda0`): the team update with its signal.
pub(crate) fn info_changed(
    teams: &mut ArenaTeams,
    throttle: &mut RosterThrottle,
    fields: &ObjectFields,
    commands: &NetCommands,
    ui: &mut UiEvents,
) {
    let info = read_info(fields).unwrap_or_default();
    teams.update_teams(&info, throttle, Instant::now(), commands);
    // `0x5a37a2`.
    ui.fire("ARENA_TEAM_UPDATE", Vec::new());
}

/// The world-enter cascade's team update (`0x52826c` -> `0x5a3dc0`), with no signal, after the dungeon finder's (`0x528267`).
pub(crate) fn enter_world(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut commands: Commands,
) {
    if cascades.read().next().is_some() {
        commands.queue(|world: &mut World| {
            if let Err(e) = world.run_system_cached(update_teams_on_enter) {
                warn!("ui_arena_team: the world entry's update did not run: {e}");
            }
        });
    }
}

fn update_teams_on_enter(
    mut teams: ResMut<ArenaTeams>,
    mut throttle: ResMut<RosterThrottle>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    if let Some(info) = me.iter().next().and_then(|s| read_info(&s.0)) {
        teams.update_teams(&info, &mut throttle, Instant::now(), &commands);
    }
}

/// What the arena-team verbs read live.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ArenaTeamView<'w> {
    teams: Option<Res<'w, ArenaTeams>>,
    commands: Option<Res<'w, NetCommands>>,
}

impl ArenaTeamView<'_> {
    /// Slot `slot`'s field words off our descriptor, zeros without them.
    pub(crate) fn info(me: Option<&ObjectFields>, slot: usize) -> [u32; 7] {
        let Some(me) = me.filter(|f| f.player_arena_team_info(0, 0).is_some()) else {
            return [0; 7];
        };
        std::array::from_fn(|word| {
            me.player_arena_team_info(slot as u16, word as u16)
                .unwrap_or(0)
        })
    }

    /// The table's slot, held only while our descriptor carries the team words.
    fn slot(&self, me: Option<&ObjectFields>, slot: usize) -> Option<&TeamSlot> {
        me?.player_arena_team_info(0, 0)?;
        self.teams.as_deref()?.slots.get(slot)
    }

    /// Slot `slot`'s last stats, zeros without our team words.
    pub(crate) fn stats(&self, me: Option<&ObjectFields>, slot: usize) -> [u32; 6] {
        self.slot(me, slot).map_or([0; 6], |s| s.stats)
    }

    /// Slot `slot`'s last roster team type, 0 without our team words.
    pub(crate) fn roster_type(&self, me: Option<&ObjectFields>, slot: usize) -> u32 {
        self.slot(me, slot).map_or(0, |s| s.roster_type)
    }

    /// Slot `slot`'s roster length, 0 without our team words.
    pub(crate) fn member_count(&self, me: Option<&ObjectFields>, slot: usize) -> usize {
        self.slot(me, slot).map_or(0, |s| s.members.len())
    }

    /// Slot `slot`'s roster member `index` in wire order, borrowed.
    pub(crate) fn member(
        &self,
        me: Option<&ObjectFields>,
        slot: usize,
        index: usize,
    ) -> Option<ArenaTeamMemberRef<'_>> {
        self.slot(me, slot)?.members.get(index).map(member_ref)
    }

    /// The team cache's record, asked on a miss.
    pub(crate) fn record(&self, team_id: u32) -> Option<ArenaTeamRecordView> {
        let teams = self.teams.as_deref()?;
        let commands = self.commands.as_deref()?;
        (team_id != 0)
            .then(|| teams.record(team_id, commands))
            .flatten()
            .map(ArenaTeams::record_view)
    }
}

/// A `GlobalStrings` line shown through the message table, in turn with the handler's events.
fn show_line(ui: &mut UiEvents, key: &'static str, args: Vec<String>) {
    let args = args.into_iter().map(crate::ui_action::FillArg::S).collect();
    crate::ui_action::show_in_turn(
        ui,
        "ui_arena_team",
        crate::ui_action::UiError::args(key, args),
    );
}

/// `SMSG_ARENA_TEAM_COMMAND_RESULT`'s line (`0x6ccb20`).
fn command_result_line(r: ArenaTeamCommandResult) -> Option<(&'static str, Vec<String>)> {
    let (team, player) = (r.team, r.player);
    if r.error == 0 {
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
    }
}

/// `SMSG_ARENA_TEAM_EVENT`'s line (`0x6cc980`): a join names us apart.
fn event_line(e: ArenaTeamEvent, my_name: Option<&str>) -> (&'static str, Vec<String>) {
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
    (key, strings)
}

/// The VM's sends; the roster ask goes through the throttle.
fn drain_arena_teams(
    mut script: crate::ui_script::InTurn,
    mut throttle: ResMut<RosterThrottle>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script.turn() else {
        return;
    };
    let now = Instant::now();
    let info = me.iter().next().and_then(|s| read_info(&s.0));
    for r in script.take_arena_team_requests() {
        let c = match r {
            ArenaTeamRequest::Roster { team_id } => {
                if let Some(info) = info.as_ref() {
                    if let Some(i) = info.iter().position(|w| w[0] == team_id) {
                        query_roster(i, info, &mut throttle, now, &commands);
                    }
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

mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use benilla_ui::script::{InspectArenaTeamWords, ScriptValue};
    use bevy::prelude::*;

    use super::{command_result_line, event_line, show_line, ArenaTeams, RosterThrottle};
    use crate::names::NameCache;
    use crate::net::{NetHandlerApp, SelfGuid};
    use crate::ui_script::live::UiEvents;

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

    /// The game shutdown frees every team (`0x5a3dd0`); the cache is the session's.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut teams: ResMut<ArenaTeams>,
        mut throttle: ResMut<RosterThrottle>,
    ) {
        *teams = ArenaTeams::default();
        *throttle = RosterThrottle::default();
    }

    fn on_packet(
        In(ev): In<SessionEvent>,
        mut teams: ResMut<ArenaTeams>,
        mut throttle: ResMut<RosterThrottle>,
        names: Res<NameCache>,
        me: Res<SelfGuid>,
        target: Res<crate::ui_inspect::InspectGuid>,
        mut ui: ResMut<UiEvents>,
    ) {
        match ev {
            // The team cache's answer callback (`0x5a28c0`).
            SessionEvent::ArenaTeamQueryResponse(r) => {
                teams.records.insert(r.team_id, Some(r));
                ui.fire("ARENA_TEAM_UPDATE", Vec::new());
            }
            // `SMSG_ARENA_TEAM_ROSTER` (`0x5a3e10`): dropped for a team in no slot.
            SessionEvent::ArenaTeamRoster(r) => {
                if let Some(slot) = teams.slot_of(r.team_id) {
                    slot.roster_type = r.team_type;
                    slot.members = r.members;
                    ui.fire("ARENA_TEAM_ROSTER_UPDATE", Vec::new());
                }
            }
            // `SMSG_ARENA_TEAM_STATS` (`0x5a2d50`): dropped for a team in no slot.
            SessionEvent::ArenaTeamStats(st) => {
                if let Some(slot) = teams.slot_of(st.team_id) {
                    slot.stats = [
                        st.rating,
                        st.week_games,
                        st.week_wins,
                        st.season_games,
                        st.season_wins,
                        st.rank,
                    ];
                    ui.fire("ARENA_TEAM_UPDATE", Vec::new());
                }
            }
            SessionEvent::ArenaTeamCommandResult(r) => {
                if let Some((key, args)) = command_result_line(r) {
                    show_line(&mut ui, key, args);
                }
            }
            // The line, then the roster update with force 1 for every slot (`0x5a28e0`).
            SessionEvent::ArenaTeamEvent(e) => {
                let mine = me.0.and_then(|g| names.peek(g)).map(str::to_string);
                let (key, args) = event_line(e, mine.as_deref());
                show_line(&mut ui, key, args);
                throttle.0 = Default::default();
                for _ in 0..3 {
                    ui.fire("ARENA_TEAM_ROSTER_UPDATE", vec![ScriptValue::Int(1)]);
                }
            }
            // `SMSG_ARENA_TEAM_INVITE` (`0x6cc94f`): the popup's event and the chat line.
            SessionEvent::ArenaTeamInvite { inviter, team } => {
                ui.fire(
                    "ARENA_TEAM_INVITE_REQUEST",
                    vec![
                        ScriptValue::Str(inviter.clone()),
                        ScriptValue::Str(team.clone()),
                    ],
                );
                show_line(
                    &mut ui,
                    "ERR_INVITED_TO_GUILD_SSS",
                    vec![inviter.clone(), inviter, team],
                );
            }
            // `MSG_INSPECT_ARENA_TEAMS` (`0x5e7b00`) for the target alone (`0x5e7b18`); deviation: slot 3 is dropped, where 12340 writes past the array.
            SessionEvent::InspectArenaTeam(t) => {
                if t.player_guid == 0 || t.player_guid != target.get() || t.slot >= 3 {
                    return;
                }
                let words = InspectArenaTeamWords {
                    team_id: t.team_id,
                    rating: t.rating,
                    games: t.games,
                    wins: t.wins,
                    played: t.played,
                    personal_rating: t.personal_rating,
                };
                let slot = usize::from(t.slot);
                ui.write(move |s| s.set_inspect_arena_team(slot, words));
                ui.fire("INSPECT_HONOR_UPDATE", Vec::new());
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
        app.init_resource::<ArenaTeams>()
            .init_resource::<RosterThrottle>()
            .init_resource::<crate::ui_inspect::InspectGuid>()
            .add_systems(
                Update,
                (
                    // The world entry's list runs it after the dungeon finder's (`0x528267`, `0x52826c`).
                    enter_world
                        .in_set(crate::ui_script::UiFeed)
                        .after(crate::ui_lfg::lfg_enter_world)
                        .after(crate::query_cache::release_on_enter::<ArenaTeams>),
                    crate::ui_script::raises::then_fire(drain_arena_teams).in_set(UiDrain),
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

    /// `0x5a3600`'s per-slot throttle; `0x5a376a` takes the field's id, the stats and roster kept.
    #[test]
    fn a_team_in_a_slot_asks_its_roster_once_per_ten_seconds() {
        let (commands, rx) = commands();
        let mut t = ArenaTeams::default();
        let mut throttle = RosterThrottle::default();
        let now = Instant::now();
        t.update_teams(&info(9), &mut throttle, now, &commands);
        t.slots[0].stats[0] = 1520;
        query_roster(
            0,
            &info(9),
            &mut throttle,
            now + Duration::from_secs(5),
            &commands,
        );
        query_roster(
            0,
            &info(9),
            &mut throttle,
            now + Duration::from_secs(11),
            &commands,
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert_eq!(sent.len(), 2, "the first ask and the one past the throttle");
        t.update_teams(&info(0), &mut throttle, now, &commands);
        assert_eq!(t.slots[0].team_id, 0);
        assert_eq!(t.slots[0].stats[0], 1520, "the table keeps the stats");
    }

    #[test]
    fn a_join_names_us_apart() {
        let (key, args) = event_line(
            ArenaTeamEvent {
                event: 3,
                strings: vec!["Claude".into(), "Probes".into()],
                guid: Some(1),
            },
            Some("claude"),
        );
        assert_eq!(
            (key, args),
            ("ERR_ARENA_TEAM_YOU_JOIN_S", vec!["Probes".to_string()])
        );
    }
}
