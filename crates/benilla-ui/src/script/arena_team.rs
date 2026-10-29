//! `CGArenaTeamInfo`'s bindings (`0x5a2930`-`0x5a3cf0`) and the arena-team verbs beside them:
//! the PvP window's three team banners, the roster pane, and the captain's commands. The app
//! pushes each slot's `PLAYER_FIELD_ARENA_TEAM_INFO_1_1` words, the team cache's record and the
//! last stats and roster; the VM owns what the bindings write, the roster's sort, its offline
//! filter and the selected member.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, number_arg};

/// The team cache's record (`ArenaTeamCache`, `0x98d220`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ArenaTeamRecordView {
    pub name: String,
    pub team_type: u32,
    /// `0xAARRGGBB` colours and two style indices.
    pub background_color: u32,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
}

/// One roster member (`ArenaTeamMember`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ArenaTeamMemberView {
    pub guid: u64,
    pub name: String,
    /// 0 the captain, 1 a member.
    pub rank: u32,
    pub level: u32,
    /// The class's display name, `None` for a class the tables lack.
    pub class: Option<String>,
    pub online: bool,
    pub week_games: u32,
    pub week_wins: u32,
    pub season_games: u32,
    pub season_wins: u32,
    pub personal_rating: u32,
    pub matchmaking: Option<(f32, f32)>,
}

/// One team slot as the app pushes it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ArenaTeamSlotView {
    /// The slot's seven field words: team id, type, captain flag (0 the captain), games this
    /// week, games this season, wins this season, personal rating.
    pub info: [u32; 7],
    /// The cache's record, `None` until it answers.
    pub record: Option<ArenaTeamRecordView>,
    /// `SMSG_ARENA_TEAM_STATS`: rating, week games, week wins, season games, season wins, rank.
    pub stats: [u32; 6],
    /// The last roster, in wire order; the VM sorts it.
    pub members: Vec<ArenaTeamMemberView>,
}

/// A captain's or member's send, the team id resolved from the slot (0 off the three).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArenaTeamRequest {
    Roster { team_id: u32 },
    Invite { team_id: u32, name: String },
    Remove { team_id: u32, name: String },
    Leader { team_id: u32, name: String },
    Leave { team_id: u32 },
    Disband { team_id: u32 },
    Accept,
    Decline,
}

/// The roster's seven sort columns (`m_sortOrder`, `0xc0f7fc`).
const ROSTER_COLUMNS: usize = 7;

/// The VM's `CGArenaTeamInfo` mirror.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArenaTeams {
    slots: [ArenaTeamSlotView; 3],
    /// Each slot's members in display order, indices into its `members`.
    order: [Vec<usize>; 3],
    /// `m_showOffline` (`0xacf834`), on until a call turns it off.
    show_offline: bool,
    /// `(column, reverse)`, most significant first.
    sort: [(u32, bool); ROSTER_COLUMNS],
    /// `m_selectedMember` (`0xc0f838`), a guid.
    selected: u64,
    requests: Vec<ArenaTeamRequest>,
}

impl Default for ArenaTeams {
    fn default() -> Self {
        Self {
            slots: Default::default(),
            order: Default::default(),
            show_offline: true,
            sort: std::array::from_fn(|i| (i as u32, false)),
            selected: 0,
            requests: Vec::new(),
        }
    }
}

impl ArenaTeams {
    /// `QSortRoster` (`0x5a2bd0`): with offline members hidden, the online first; then each
    /// column in turn, the name and class A to Z, the counts high first.
    fn compare(&self, a: &ArenaTeamMemberView, b: &ArenaTeamMemberView) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        if !self.show_offline && a.online != b.online {
            return b.online.cmp(&a.online);
        }
        for &(column, reverse) in &self.sort {
            let o = match column {
                0 => a.name.cmp(&b.name),
                1 => match (&a.class, &b.class) {
                    (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
                    _ => Ordering::Equal,
                },
                2 => b.week_games.cmp(&a.week_games),
                3 => b.week_wins.cmp(&a.week_wins),
                4 => b.season_games.cmp(&a.season_games),
                5 => b.season_wins.cmp(&a.season_wins),
                6 => b.personal_rating.cmp(&a.personal_rating),
                _ => Ordering::Equal,
            };
            if o != Ordering::Equal {
                return if reverse { o.reverse() } else { o };
            }
        }
        Ordering::Equal
    }

    fn resort(&mut self, slot: usize) {
        let members = &self.slots[slot].members;
        let mut order: Vec<usize> = (0..members.len()).collect();
        order.sort_by(|&x, &y| self.compare(&members[x], &members[y]));
        self.order[slot] = order;
    }

    fn member(&self, slot: u32, index: u32) -> Option<&ArenaTeamMemberView> {
        let slot = usize::try_from(slot).ok().filter(|&s| s < 3)?;
        let &m = self.order[slot].get(index as usize)?;
        self.slots[slot].members.get(m)
    }

    fn team_id(&self, slot: u32) -> u32 {
        self.slots.get(slot as usize).map_or(0, |s| s.info[0])
    }
}

impl super::UiScript {
    /// Push the three slots; a changed roster re-sorts under the VM's own order.
    pub fn set_arena_teams(&mut self, slots: [ArenaTeamSlotView; 3]) {
        if self.model_ref().arena_teams.slots == slots {
            return;
        }
        let mut model = self.model_mut();
        let teams = &mut model.arena_teams;
        teams.slots = slots;
        for i in 0..3 {
            teams.resort(i);
        }
    }

    pub fn take_arena_team_requests(&mut self) -> Vec<ArenaTeamRequest> {
        drain_field!(self, arena_teams.requests)
    }
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, crate::script::ModelCell> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
}

fn player_exists(model: &super::Model) -> bool {
    model.unit("player").is_some_and(|u| u.exists)
}

/// A banner colour's channels as `GetArenaTeam` scales them, `byte * (1.0f / 255.0f)` in single
/// precision.
fn channels(argb: u32) -> [f64; 3] {
    [16, 8, 0].map(|shift| f64::from(((argb >> shift) & 0xFF) as f32 * (1.0f32 / 255.0)))
}

/// `SortArenaTeamRoster`'s column names (`0x5a3490`); anything else sorts by name.
fn sort_column(kind: &str) -> u32 {
    [
        "name",
        "class",
        "played",
        "won",
        "seasonplayed",
        "seasonwon",
        "rating",
    ]
    .iter()
    .position(|n| kind.eq_ignore_ascii_case(n))
    .map_or(0, |i| i as u32)
}

/// A named send's player argument: the reference raises past 48 bytes (`0x515d3a`).
fn checked_name(name: &str) -> mlua::Result<()> {
    if name.len() > 48 {
        return Err(mlua::Error::runtime("Name too long"));
    }
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetArenaTeam(team) (`0x5a3860`): twenty-two values, nil and twenty-one zeros without a
    // player, a team in the slot or its cache record.
    g.set(
        "GetArenaTeam",
        lua.create_function(|lua, team: Value| {
            let team = (number_arg(lua, team, "Usage: GetArenaTeam(team)")? as u32).wrapping_sub(1);
            let model = read(lua);
            let slot = model
                .arena_teams
                .slots
                .get(team as usize)
                .filter(|s| s.info[0] != 0 && player_exists(&model));
            let Some((s, r)) = slot.and_then(|s| Some((s, s.record.as_ref()?))) else {
                let mut v = vec![Value::Nil];
                v.extend(std::iter::repeat_n(Value::Number(0.0), 21));
                return Ok(MultiValue::from_vec(v));
            };
            let n = |v: u32| Value::Number(f64::from(v));
            let i = |v: u32| Value::Number(f64::from(v as i32));
            let mut out = vec![
                Value::String(lua.create_string(&r.name)?),
                i(r.team_type),
                n(s.stats[0]),
                n(s.stats[1]),
                n(s.stats[2]),
                n(s.stats[3]),
                n(s.stats[4]),
                n(s.info[3]),
                n(s.info[4]),
                n(s.stats[5]),
                n(s.info[6]),
            ];
            out.extend(channels(r.background_color).map(Value::Number));
            out.push(i(r.emblem_style));
            out.extend(channels(r.emblem_color).map(Value::Number));
            out.push(i(r.border_style));
            out.extend(channels(r.border_color).map(Value::Number));
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetNumArenaTeamMembers(team [, showOffline]) (`0x5a2930`): every member when offline ones
    // show (the store's or the call's), else the online count.
    g.set(
        "GetNumArenaTeamMembers",
        lua.create_function(|lua, (team, show): (Value, Option<Value>)| {
            let team = (number_arg(
                lua,
                team,
                "Usage: GetNumArenaTeamMembers(team [,showOffline])",
            )? as u32)
                .wrapping_sub(1);
            let show = bool_or_default(show.as_ref(), false);
            let model = read(lua);
            let teams = &model.arena_teams;
            let Some(s) = teams.slots.get(team as usize) else {
                return Ok(0);
            };
            Ok(if teams.show_offline || show {
                s.members.len() as i64
            } else {
                s.members.iter().filter(|m| m.online).count() as i64
            })
        })?,
    )?;

    // GetArenaTeamRosterInfo(team, index) (`0x5a2fc0`): name, rank, level, class, online, week
    // games and wins, season games and wins, personal rating; ten on every leg.
    g.set(
        "GetArenaTeamRosterInfo",
        lua.create_function(|lua, (team, index): (Value, Value)| {
            let usage = "Usage: GetArenaTeamRosterInfo(team, index)";
            let team = (number_arg(lua, team, usage)? as u32).wrapping_sub(1);
            let index = (number_arg(lua, index, usage)? as u32).wrapping_sub(1);
            let model = read(lua);
            let n = |v: u32| Value::Number(f64::from(v));
            let Some(m) = model.arena_teams.member(team, index) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    n(0),
                    n(0),
                    Value::Nil,
                    Value::Nil,
                    n(0),
                    n(0),
                    n(0),
                    n(0),
                    n(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&m.name)?),
                n(m.rank),
                Value::Number(f64::from(m.level as i32)),
                match &m.class {
                    Some(c) => Value::String(lua.create_string(c)?),
                    None => Value::Nil,
                },
                flag(m.online),
                n(m.week_games),
                n(m.week_wins),
                n(m.season_games),
                n(m.season_wins),
                n(m.personal_rating),
            ]))
        })?,
    )?;

    // GetArenaTeamGdfInfo(team, index) (`0x5a3260`): the matchmaking pair, two nils without.
    g.set(
        "GetArenaTeamGdfInfo",
        lua.create_function(|lua, (team, index): (Value, Value)| {
            let usage = "Usage: GetArenaTeamGdfInfo(team, index)";
            let team = (number_arg(lua, team, usage)? as u32).wrapping_sub(1);
            let index = (number_arg(lua, index, usage)? as u32).wrapping_sub(1);
            let model = read(lua);
            Ok(match model.arena_teams.member(team, index) {
                Some(m) => {
                    let (before, after) = m.matchmaking.unwrap_or_default();
                    MultiValue::from_vec(vec![
                        Value::Number(f64::from(before)),
                        Value::Number(f64::from(after)),
                    ])
                }
                None => MultiValue::from_vec(vec![Value::Nil, Value::Nil]),
            })
        })?,
    )?;

    // SetArenaTeamRosterSelection(team, index) (`0x5a2b80`): the member's guid, 0 off the roster.
    g.set(
        "SetArenaTeamRosterSelection",
        lua.create_function(|lua, (team, index): (Value, Value)| {
            let usage = "Usage: SetArenaTeamRosterSelection(team, index)";
            let team = (number_arg(lua, team, usage)? as u32).wrapping_sub(1);
            let index = (number_arg(lua, index, usage)? as u32).wrapping_sub(1);
            let mut model = crate::script::model_mut_of(lua);
            let guid = model.arena_teams.member(team, index).map_or(0, |m| m.guid);
            model.arena_teams.selected = guid;
            Ok(())
        })?,
    )?;

    // GetArenaTeamRosterSelection(team) (`0x5a2f40`): the selected member's 1-based place, 0.
    g.set(
        "GetArenaTeamRosterSelection",
        lua.create_function(|lua, team: Value| {
            let team = (number_arg(lua, team, "Usage: GetArenaTeamRosterSelection(team)")? as u32)
                .wrapping_sub(1);
            let model = read(lua);
            let teams = &model.arena_teams;
            if team >= 3 || teams.selected == 0 {
                return Ok(0);
            }
            let slot = &teams.slots[team as usize];
            Ok(teams.order[team as usize]
                .iter()
                .position(|&m| slot.members[m].guid == teams.selected)
                .map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    // SortArenaTeamRoster(type) (`0x5a2e80`): the column moves to the front, flipping its
    // direction when it was there; each team with members re-sorts and fires the roster event.
    g.set(
        "SortArenaTeamRoster",
        lua.create_function(|lua, kind: Value| {
            let kind = match &kind {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Integer(_) | Value::Number(_) => lua
                    .coerce_string(kind.clone())?
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
                _ => return Err(mlua::Error::runtime("Usgae: SortArenaTeamRoster(\"type\")")),
            };
            let column = sort_column(&kind);
            let mut model = crate::script::model_mut_of(lua);
            let teams = &mut model.arena_teams;
            if let Some(i) = teams.sort.iter().position(|&(c, _)| c == column) {
                let mut reverse = teams.sort[i].1;
                if i == 0 {
                    reverse = !reverse;
                }
                teams.sort.copy_within(0..i, 1);
                teams.sort[0] = (column, reverse);
            }
            let mut fired = 0;
            for i in 0..3 {
                if !teams.slots[i].members.is_empty() {
                    teams.resort(i);
                    fired += 1;
                }
            }
            for _ in 0..fired {
                model
                    .pending_events
                    .push(("ARENA_TEAM_ROSTER_UPDATE".to_string(), Vec::new()));
            }
            Ok(())
        })?,
    )?;

    // SetArenaTeamRosterShowOffline([show]) (`0x5a2e10`), on by default; a change re-sorts.
    g.set(
        "SetArenaTeamRosterShowOffline",
        lua.create_function(|lua, args: MultiValue| {
            // An explicit nil turns it off; only a missing argument takes the default.
            let show = bool_or_default(args.iter().next(), true);
            let mut model = crate::script::model_mut_of(lua);
            let teams = &mut model.arena_teams;
            if teams.show_offline == show {
                return Ok(());
            }
            teams.show_offline = show;
            let mut fired = 0;
            for i in 0..3 {
                if !teams.slots[i].members.is_empty() {
                    teams.resort(i);
                    fired += 1;
                }
            }
            for _ in 0..fired {
                model
                    .pending_events
                    .push(("ARENA_TEAM_ROSTER_UPDATE".to_string(), Vec::new()));
            }
            Ok(())
        })?,
    )?;

    g.set(
        "GetArenaTeamRosterShowOffline",
        lua.create_function(|lua, ()| Ok(flag(read(lua).arena_teams.show_offline)))?,
    )?;

    // CloseArenaTeamRoster() shares the empty binding `0x8e5250`.
    g.set("CloseArenaTeamRoster", lua.create_function(|_, ()| Ok(()))?)?;

    // ArenaTeamRoster(team) (`0x5a3cf0`): asks the slot's roster; the app holds the 10 s
    // throttle (`0x5a3600`).
    g.set(
        "ArenaTeamRoster",
        lua.create_function(|lua, team: Value| {
            let team =
                (number_arg(lua, team, "Usage: ArenaTeamRoster(team)")? as u32).wrapping_sub(1);
            let mut model = crate::script::model_mut_of(lua);
            let team_id = model.arena_teams.team_id(team);
            if team_id != 0 && player_exists(&model) {
                model
                    .arena_teams
                    .requests
                    .push(ArenaTeamRequest::Roster { team_id });
            }
            Ok(())
        })?,
    )?;

    // GetCurrentArenaSeason() and GetPreviousArenaSeason() (`0x5a2a40`, `0x5a2a70`): world
    // states 3191 and 3901.
    for (name, state) in [
        ("GetCurrentArenaSeason", 3191u32),
        ("GetPreviousArenaSeason", 3901),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                Ok(f64::from(read(lua).worldmap.world_state(state)))
            })?,
        )?;
    }

    // IsArenaTeamCaptain(index) (`0x60dc70`): 1 unless the slot's captain word is set; an empty
    // slot's is 0, so it answers 1.
    g.set(
        "IsArenaTeamCaptain",
        lua.create_function(|lua, index: Value| {
            let index = (number_arg(lua, index, "Usage: IsArenaTeamCaptain(index)")? as u32)
                .wrapping_sub(1);
            let model = read(lua);
            let captain = player_exists(&model)
                && model
                    .arena_teams
                    .slots
                    .get(index as usize)
                    .is_some_and(|s| s.info[2] == 0);
            Ok(flag(captain))
        })?,
    )?;

    // IsInArenaTeam() (`0x60dbf0`): a Lua boolean, any slot with a team.
    g.set(
        "IsInArenaTeam",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            Ok(player_exists(&model) && model.arena_teams.slots.iter().any(|s| s.info[0] != 0))
        })?,
    )?;

    // The captain's named sends (`0x515cc0`, `0x515eb0`, `0x515ff0`); an invite with an empty
    // name sends nothing.
    for (name, usage, kind) in [
        (
            "ArenaTeamInviteByName",
            "Usage: ArenaTeamInviteByName(team, name)",
            0u8,
        ),
        (
            "ArenaTeamUninviteByName",
            "Usage: ArenaTeamUninviteByName(team, name)",
            1,
        ),
        (
            "ArenaTeamSetLeaderByName",
            "Usage: ArenaTeamSetLeaderByName(team, name)",
            2,
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, (team, who): (Value, Value)| {
                let team = (number_arg(lua, team, usage)? as u32).wrapping_sub(1);
                let who = super::binding_abi::string_arg(lua, who, usage)?;
                if kind == 0 && who.is_empty() {
                    return Ok(());
                }
                checked_name(&who)?;
                let mut model = crate::script::model_mut_of(lua);
                let team_id = model.arena_teams.team_id(team);
                let request = match kind {
                    0 => ArenaTeamRequest::Invite { team_id, name: who },
                    1 => ArenaTeamRequest::Remove { team_id, name: who },
                    _ => ArenaTeamRequest::Leader { team_id, name: who },
                };
                model.arena_teams.requests.push(request);
                Ok(())
            })?,
        )?;
    }

    for (name, usage, disband) in [
        ("ArenaTeamLeave", "Usage: ArenaTeamLeave(team)", false),
        ("ArenaTeamDisband", "Usage: ArenaTeamDisband(team)", true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, team: Value| {
                let team = (number_arg(lua, team, usage)? as u32).wrapping_sub(1);
                let mut model = crate::script::model_mut_of(lua);
                let team_id = model.arena_teams.team_id(team);
                model.arena_teams.requests.push(if disband {
                    ArenaTeamRequest::Disband { team_id }
                } else {
                    ArenaTeamRequest::Leave { team_id }
                });
                Ok(())
            })?,
        )?;
    }

    // AcceptArenaTeam() and DeclineArenaTeam() (`0x51ac30`, `0x51ac60`): with a player.
    for (name, accept) in [("AcceptArenaTeam", true), ("DeclineArenaTeam", false)] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                if player_exists(&model) {
                    model.arena_teams.requests.push(if accept {
                        ArenaTeamRequest::Accept
                    } else {
                        ArenaTeamRequest::Decline
                    });
                }
                Ok(())
            })?,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn vm() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(crate::script::UnitState {
                exists: true,
                name: Some("Claude".into()),
                ..Default::default()
            }),
        );
        s
    }

    fn member(guid: u64, name: &str, online: bool, rating: u32) -> ArenaTeamMemberView {
        ArenaTeamMemberView {
            guid,
            name: name.into(),
            rank: u32::from(guid != 1),
            level: 80,
            class: Some("Hunter".into()),
            online,
            week_games: 4,
            week_wins: 2,
            season_games: 10,
            season_wins: 6,
            personal_rating: rating,
            matchmaking: None,
        }
    }

    fn team() -> [ArenaTeamSlotView; 3] {
        let mut slots: [ArenaTeamSlotView; 3] = Default::default();
        slots[0] = ArenaTeamSlotView {
            info: [9, 2, 0, 4, 10, 6, 1520],
            record: Some(ArenaTeamRecordView {
                name: "Probes".into(),
                team_type: 2,
                background_color: 0xFF_FF_80_00,
                emblem_style: 7,
                emblem_color: 0xFF_00_00_FF,
                border_style: 3,
                border_color: 0xFF_00_FF_00,
            }),
            stats: [1500, 4, 2, 10, 6, 12],
            members: vec![
                member(2, "Zed", false, 1490),
                member(1, "Claude", true, 1520),
            ],
        };
        slots
    }

    #[test]
    fn get_arena_team_answers_twenty_two_values() {
        let mut s = vm();
        assert_eq!(s.arity("GetArenaTeam(1)").unwrap(), 22);
        assert!(s.eval::<bool>("return GetArenaTeam(1) == nil").unwrap());
        s.set_arena_teams(team());
        let got = s
            .eval::<String>(
                "local n, size, rating, games, wins, sgames, swins, played, splayed, rank, \
                 personal, r, g, b, emblem = GetArenaTeam(1) \
                 return table.concat({n, size, rating, games, wins, sgames, swins, played, \
                 splayed, rank, personal, emblem}, ',') .. '|' .. r .. ',' .. b",
            )
            .unwrap();
        assert_eq!(got, "Probes,2,1500,4,2,10,6,4,10,12,1520,7|1,0");
        assert!(s.eval::<bool>("return GetArenaTeam(2) == nil").unwrap());
        assert_eq!(s.eval::<i64>("return IsArenaTeamCaptain(1)").unwrap(), 1);
        assert!(s.eval::<bool>("return IsInArenaTeam() == true").unwrap());
    }

    #[test]
    fn the_roster_sorts_by_name_and_selects_by_guid() {
        let mut s = vm();
        s.set_arena_teams(team());
        assert_eq!(
            s.eval::<i64>("return GetNumArenaTeamMembers(1)").unwrap(),
            2
        );
        assert_eq!(
            s.eval::<String>("return (GetArenaTeamRosterInfo(1, 1))")
                .unwrap(),
            "Claude",
            "name A to Z first"
        );
        assert_eq!(s.arity("GetArenaTeamRosterInfo(1, 9)").unwrap(), 10);
        s.run("SetArenaTeamRosterSelection(1, 2)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetArenaTeamRosterSelection(1)")
                .unwrap(),
            2
        );
        s.run("SortArenaTeamRoster('name')").unwrap();
        assert_eq!(
            s.eval::<String>("return (GetArenaTeamRosterInfo(1, 1))")
                .unwrap(),
            "Zed",
            "the front column flips"
        );
        assert_eq!(
            s.eval::<i64>("return GetArenaTeamRosterSelection(1)")
                .unwrap(),
            1,
            "the selection follows its guid"
        );
        s.run("SetArenaTeamRosterShowOffline(nil)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumArenaTeamMembers(1)").unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>("return GetNumArenaTeamMembers(1, 1)")
                .unwrap(),
            2
        );
    }

    #[test]
    fn the_sends_resolve_the_team_id() {
        let mut s = vm();
        s.set_arena_teams(team());
        s.run(
            "ArenaTeamRoster(1) ArenaTeamRoster(2) ArenaTeamInviteByName(1, 'Claudetwo') \
             ArenaTeamInviteByName(1, '') ArenaTeamLeave(3) AcceptArenaTeam()",
        )
        .unwrap();
        assert_eq!(
            s.take_arena_team_requests(),
            vec![
                ArenaTeamRequest::Roster { team_id: 9 },
                ArenaTeamRequest::Invite {
                    team_id: 9,
                    name: "Claudetwo".into()
                },
                ArenaTeamRequest::Leave { team_id: 0 },
                ArenaTeamRequest::Accept,
            ]
        );
        let err = s
            .run(&format!("ArenaTeamInviteByName(1, '{}')", "x".repeat(49)))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Name too long"), "{err}");
    }
}
