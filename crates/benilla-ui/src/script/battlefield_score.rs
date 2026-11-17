//! The scoreboard verbs of `WorldStateFrame.lua` (`0x549e80`-`0x54de00`) over the board the app's
//! handlers write as 12340's do: the log's rows in wire order as it lands (`0x54d280`), the count
//! and the sort once no row's name is pending (`0x54aa30`), the columns as a status turns active
//! (`0x54b026`). A row's name, race and class are the name cache's when called (`0x54be90`). The
//! faction filter sorts its team first rather than compacting, so an index past the filtered count
//! answers a real row.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;

/// One scoreboard row as `MSG_PVP_LOG_DATA` carried it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScoreRow {
    pub guid: u64,
    pub killing_blows: u32,
    pub honorable_kills: u32,
    pub deaths: u32,
    pub bonus_honor: u32,
    /// An arena's team byte as sent; a battleground's side from the race (`0x54aadf`), -1 neither.
    pub faction: i32,
    pub damage_done: u32,
    pub healing_done: u32,
    /// The extra-stat dwords, eight slots (the client's block), unfilled ones zero.
    pub stats: [u32; 8],
}

/// A row's name-cache record as the scoreboard reads it when called.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ScoreName {
    /// The bare name, the sort's key (`0x54a740`).
    pub name: String,
    /// `Name-Realm` for a cross-realm player, the verb's first value.
    pub full_name: String,
    /// The race's display name, `None` for a race the tables lack (a nil return).
    pub race: Option<String>,
    /// The class's display name and `ChrClasses.dbc` file token, both or neither.
    pub class: Option<(String, String)>,
    /// `ChrClasses.dbc`'s id, the class column's sort key.
    pub class_id: u32,
    /// The race's side from its faction group: `0` Horde, `1` Alliance, `-1` neither.
    pub side: i32,
}

/// A column header: a `WorldStateUI.dbc` `Type == 2` row for the map, its text not expanded.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldStatColumn {
    pub text: String,
    pub icon: String,
    pub tooltip: String,
}

/// An arena team's scoreboard line (`0xbea0b0`..).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldTeamView {
    pub name: String,
    pub rating: u32,
    pub new_rating: u32,
    pub matchmaking: u32,
}

/// What one `MSG_PVP_LOG_DATA` writes (`0x54d280`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScores {
    pub rows: Vec<BattlefieldScoreRow>,
    /// The match has ended; gates the winner.
    pub ended: bool,
    /// The winning side, written only on an ended log.
    pub winner: u8,
    /// The two arena teams, written only by an arena's log.
    pub teams: Option<[BattlefieldTeamView; 2]>,
}

/// The sort's seventeen columns (`0xbea2c0`): kills, deaths, honor, name,
/// class, honorable kills, damage, healing, team, then the eight stats.
const SORT_COLUMNS: usize = 17;

/// The board: the rows, the result, the columns, the filter, the sort criteria and the order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScoreBoard {
    pub(crate) rows: Vec<BattlefieldScoreRow>,
    /// `0xbea588`.
    pub(crate) ended: bool,
    /// `0xbea58c`.
    pub(crate) winner: u8,
    pub(crate) teams: [BattlefieldTeamView; 2],
    /// The columns `0xbea5ac` counts.
    pub(crate) columns: Vec<BattlefieldStatColumn>,
    /// `SetBattlefieldScoreFaction`'s store (`0xacd170`): `-1` every team.
    pub(crate) filter: i32,
    /// `(column, descending)`, most significant first; column `i` at `i` to start.
    criteria: [(u32, bool); SORT_COLUMNS],
    /// The rows as the array holds them: wire order until a rebuild sorts it.
    pub(crate) order: Vec<usize>,
    /// The filtered count (`0xbea580`).
    pub(crate) filtered: usize,
}

impl Default for ScoreBoard {
    /// The filter starts at `-1` and each column at its own rank (the game's initialization,
    /// `0x54e720`).
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            ended: false,
            winner: 0,
            teams: Default::default(),
            columns: Vec::new(),
            filter: -1,
            criteria: std::array::from_fn(|i| (i as u32, false)),
            order: Vec::new(),
            filtered: 0,
        }
    }
}

/// A row's record through the lent view, `None` with no name cached or no view lent.
fn score_name(guid: u64) -> Option<ScoreName> {
    super::live::read_live(|l| l.score_name(guid)).flatten()
}

/// Whether a row's name is still being asked for (`0xbea590`), read through the lent view.
fn names_pending() -> bool {
    super::live::read_live(|l| l.battlefield_score_names_pending()).unwrap_or(false)
}

impl ScoreBoard {
    /// The score sort (`0x54a740`): equal with a name uncached, the filtered side first, then each criterion (numbers high, name A to Z, team low).
    fn compare(
        &self,
        (a, an): (&BattlefieldScoreRow, Option<&ScoreName>),
        (b, bn): (&BattlefieldScoreRow, Option<&ScoreName>),
    ) -> std::cmp::Ordering {
        // Deviates: 12340's class and stat columns answer "after" for an equal pair, no ordering; equal keys compare equal here.
        use std::cmp::Ordering;
        let (Some(an), Some(bn)) = (an, bn) else {
            return Ordering::Equal;
        };
        if self.filter != -1 && an.side != bn.side {
            return (an.side != self.filter).cmp(&(bn.side != self.filter));
        }
        let stats = self.columns.len();
        for &(column, descending) in &self.criteria {
            let o = match column {
                0 => b.killing_blows.cmp(&a.killing_blows),
                1 => b.deaths.cmp(&a.deaths),
                2 => b.bonus_honor.cmp(&a.bonus_honor),
                3 => an.name.cmp(&bn.name),
                4 => {
                    let o = bn.class_id.cmp(&an.class_id);
                    return if descending { o.reverse() } else { o };
                }
                5 => b.honorable_kills.cmp(&a.honorable_kills),
                6 => b.damage_done.cmp(&a.damage_done),
                7 => b.healing_done.cmp(&a.healing_done),
                8 => a.faction.cmp(&b.faction),
                c @ 9..=16 if ((c - 9) as usize) < stats => {
                    let i = (c - 9) as usize;
                    let o = b.stats[i].cmp(&a.stats[i]);
                    return if descending { o.reverse() } else { o };
                }
                _ => Ordering::Equal,
            };
            if o != Ordering::Equal {
                return if descending { o.reverse() } else { o };
            }
        }
        Ordering::Equal
    }

    /// The rebuild (`0x54aa30`): nothing while a name is pending (`0x54aa39`); true when it ran.
    fn rebuild(&mut self) -> bool {
        if names_pending() {
            return false;
        }
        let arena = super::live::read_live(|l| l.battlefield_instance().instance_type)
            == Some(super::INSTANCE_TYPE_ARENA);
        let names: Vec<Option<ScoreName>> = self.rows.iter().map(|r| score_name(r.guid)).collect();
        if !arena {
            for (row, name) in self.rows.iter_mut().zip(&names) {
                row.faction = name.as_ref().map_or(0, |n| n.side);
            }
        }
        let filter = self.filter;
        self.filtered = self
            .rows
            .iter()
            .filter(|r| filter == -1 || r.faction == filter)
            .count();
        let mut order = std::mem::take(&mut self.order);
        order.sort_by(|&a, &b| {
            self.compare(
                (&self.rows[a], names[a].as_ref()),
                (&self.rows[b], names[b].as_ref()),
            )
        });
        self.order = order;
        true
    }

    /// The sort-column setter (`0x54cee0`): skipped while a name is pending (`0x54cee3`), else front, flip, rebuild.
    fn sort_by_column(&mut self, column: u32) -> bool {
        if names_pending() {
            return false;
        }
        if let Some(i) = self.criteria.iter().position(|&(c, _)| c == column) {
            let mut descending = self.criteria[i].1;
            if i == 0 {
                descending = !descending;
            }
            self.criteria.copy_within(0..i, 1);
            self.criteria[0] = (column, descending);
        }
        self.rebuild()
    }

    fn row(&self, index_1based: i32) -> Option<&BattlefieldScoreRow> {
        let i = usize::try_from(index_1based).ok()?.checked_sub(1)?;
        // Bounded by the wire count, not the filtered one.
        self.order.get(i).map(|&r| &self.rows[r])
    }
}

impl super::UiScript {
    /// `MSG_PVP_LOG_DATA`'s writes (`0x54d280`); the count and order wait for the rebuild.
    pub fn set_battlefield_log(&mut self, log: BattlefieldScores) {
        let mut model = self.model_mut();
        let board = &mut model.battlefield_board;
        if let Some(teams) = log.teams {
            board.teams = teams;
        }
        board.ended = log.ended;
        if log.ended {
            board.winner = log.winner;
        }
        board.order = (0..log.rows.len()).collect();
        board.rows = log.rows;
    }

    /// The rebuild (`0x54aa30`), which fires `UPDATE_BATTLEFIELD_SCORE` unless a name is pending.
    pub fn rebuild_battlefield_scores(&mut self) {
        let ran = self.model_mut().battlefield_board.rebuild();
        if ran {
            self.fire_event("UPDATE_BATTLEFIELD_SCORE", Vec::new());
        }
    }

    /// The status-3 arm (`0x54b026`-`0x54b0d6`): the columns, the filter reset, the rebuild.
    pub fn activate_battlefield_scores(&mut self, columns: Vec<BattlefieldStatColumn>) {
        {
            let mut model = self.model_mut();
            model.battlefield_board.columns = columns;
            model.battlefield_board.filter = -1;
        }
        self.rebuild_battlefield_scores();
    }

    /// The status handler's queue-0 arm on the active slot zeroes the column count (`0x54b174`).
    pub fn clear_battlefield_score_columns(&mut self) {
        self.model_mut().battlefield_board.columns.clear();
    }

    /// The world entry's board clear (`0x54ca50`).
    pub fn reset_battlefield_scores_on_world_enter(&mut self) {
        let mut model = self.model_mut();
        let board = &mut model.battlefield_board;
        board.ended = false;
        board.winner = 0;
        board.filter = -1;
        board.rows.clear();
        board.order.clear();
        board.filtered = 0;
        board.columns.clear();
        for team in &mut board.teams {
            team.name.clear();
        }
    }

    /// `RequestBattlefieldScoreData()` calls since the last drain.
    pub fn take_battlefield_score_requests(&mut self) -> u32 {
        drain_field!(self, battlefield_score_requests)
    }
}

fn opt(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    Ok(match s {
        Some(t) => Value::String(lua.create_string(t)?),
        None => Value::Nil,
    })
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, crate::script::ModelCell> {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
}

/// `SortBattlefieldScoreData`'s column names (`0x54de00`), case-insensitive; `stat<n>` is column
/// `n + 8` below 17, and anything else sorts by kills.
fn sort_column(kind: &str) -> Option<u32> {
    let named = [
        "kills", "deaths", "cp", "name", "class", "hk", "damage", "healing", "team",
    ];
    if let Some(i) = named.iter().position(|n| kind.eq_ignore_ascii_case(n)) {
        return Some(i as u32);
    }
    let Some(tail) = kind
        .get(..4)
        .filter(|h| h.eq_ignore_ascii_case("stat"))
        .map(|_| &kind[4..])
    else {
        return Some(0);
    };
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let stat = digits.parse::<u32>().unwrap_or(0).saturating_add(8);
    (stat < SORT_COLUMNS as u32).then_some(stat)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumBattlefieldScores",
        lua.create_function(|lua, ()| Ok(read(lua).battlefield_board.filtered as i64))?,
    )?;

    // Twelve values on every leg (`0x54be90`); an uncached row answers the empty leg.
    g.set(
        "GetBattlefieldScore",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldScore(index)")?;
            let model = read(lua);
            let row = model.battlefield_board.row(index);
            let Some((r, n)) = row.and_then(|r| Some((r, score_name(r.guid)?))) else {
                let mut v = vec![Value::Nil];
                v.extend(std::iter::repeat_n(Value::Number(0.0), 6));
                v.extend([Value::Nil, Value::Nil, Value::Nil]);
                v.extend([Value::Number(0.0), Value::Number(0.0)]);
                return Ok(MultiValue::from_vec(v));
            };
            let num = |v: u32| Value::Number(f64::from(v as i32));
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&n.full_name)?),
                num(r.killing_blows),
                num(r.honorable_kills),
                num(r.deaths),
                num(r.bonus_honor),
                Value::Number(f64::from(r.faction)),
                Value::Number(0.0),
                opt(lua, n.race.as_deref())?,
                opt(lua, n.class.as_ref().map(|c| c.0.as_str()))?,
                opt(lua, n.class.as_ref().map(|c| c.1.as_str()))?,
                num(r.damage_done),
                num(r.healing_done),
            ]))
        })?,
    )?;

    g.set(
        "GetBattlefieldWinner",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            let b = &model.battlefield_board;
            Ok(if b.ended {
                Value::Number(f64::from(b.winner))
            } else {
                Value::Nil
            })
        })?,
    )?;

    // GetBattlefieldTeamInfo(index) (`0x54a180`), 0-based: name, rating, new rating, matchmaking.
    g.set(
        "GetBattlefieldTeamInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldTeamInfo(index)")? as u32;
            let model = read(lua);
            let Some(t) = model.battlefield_board.teams.get(index as usize) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&t.name)?),
                Value::Number(f64::from(t.rating)),
                Value::Number(f64::from(t.new_rating)),
                Value::Number(f64::from(t.matchmaking)),
            ]))
        })?,
    )?;

    // Stores a filter of -1, 0 or 1 and runs the rebuild (`0x54c120`); a non-number stores -1.
    g.set(
        "SetBattlefieldScoreFaction",
        lua.create_function(|lua, faction: Option<Value>| {
            let f = match faction.and_then(|v| lua.coerce_number(v).ok().flatten()) {
                Some(n) => n as i64 as i32,
                None => -1,
            };
            if f != -1 && !(0..2).contains(&f) {
                return Ok(());
            }
            let ran = {
                let mut model = crate::script::model_mut_of(lua);
                model.battlefield_board.filter = f;
                model.battlefield_board.rebuild()
            };
            if ran {
                super::tick::fire_event_into(lua, "UPDATE_BATTLEFIELD_SCORE", Vec::new());
            }
            Ok(())
        })?,
    )?;

    // SortBattlefieldScoreData(type) (`0x54de00`); the usage string is the reference's own typo.
    g.set(
        "SortBattlefieldScoreData",
        lua.create_function(|lua, kind: Value| {
            let kind = match &kind {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Integer(i) => i.to_string(),
                Value::Number(n) => super::binding_abi::lua_number_text(*n),
                _ => {
                    return Err(mlua::Error::runtime(
                        "Usgae: SortBattlefieldScoreData(\"type\")",
                    ))
                }
            };
            let Some(column) = sort_column(&kind) else {
                return Ok(());
            };
            let ran = crate::script::model_mut_of(lua)
                .battlefield_board
                .sort_by_column(column);
            if ran {
                super::tick::fire_event_into(lua, "UPDATE_BATTLEFIELD_SCORE", Vec::new());
            }
            Ok(())
        })?,
    )?;

    g.set(
        "GetNumBattlefieldStats",
        lua.create_function(|lua, ()| Ok(read(lua).battlefield_board.columns.len() as i64))?,
    )?;

    // A column the map lacks answers three nils (`0x54c170`).
    g.set(
        "GetBattlefieldStatInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldStatInfo(index)")?;
            let model = read(lua);
            let col = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.battlefield_board.columns.get(i));
            let Some(c) = col else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&c.text)?),
                Value::String(lua.create_string(&c.icon)?),
                Value::String(lua.create_string(&c.tooltip)?),
            ]))
        })?,
    )?;

    // A bad row or a stat outside 1..=8 answers 0. Deviation: the reference (`0x549f60`) admits 9
    // and reads one dword past its block; we answer 0 because that read is out of bounds.
    g.set(
        "GetBattlefieldStatData",
        lua.create_function(|lua, (player, stat): (Value, Value)| {
            let usage = "Usage: GetBattlefieldStatData(playerIndex, statIndex)";
            let player = number_arg(lua, player, usage)?;
            let stat = number_arg(lua, stat, usage)?;
            let model = read(lua);
            let v = model
                .battlefield_board
                .row(player)
                .zip(usize::try_from(stat).ok().and_then(|s| s.checked_sub(1)))
                .and_then(|(r, s)| r.stats.get(s).copied())
                .unwrap_or(0);
            Ok(f64::from(v as i32))
        })?,
    )?;

    // The app sends an empty `MSG_PVP_LOG_DATA` under the client's throttle (`0x54ce30`).
    g.set(
        "RequestBattlefieldScoreData",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).battlefield_score_requests += 1;
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{BattlefieldInstance, LiveState, UiScript};
    use std::collections::HashMap;

    /// The name cache and the pending count, as the app lends them.
    #[derive(Default)]
    struct View {
        names: HashMap<u64, ScoreName>,
        pending: bool,
        instance_type: u32,
    }

    impl LiveState for View {
        fn unit_flags(&self, _: u64) -> Option<u32> {
            None
        }
        fn unit_health(&self, _: u64) -> Option<u32> {
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
        fn battlefield_instance(&self) -> BattlefieldInstance {
            BattlefieldInstance {
                instance_type: self.instance_type,
                ..Default::default()
            }
        }
        fn score_name(&self, guid: u64) -> Option<ScoreName> {
            self.names.get(&guid).cloned()
        }
        fn battlefield_score_names_pending(&self) -> bool {
            self.pending
        }
    }

    fn name(name: &str, side: i32) -> ScoreName {
        ScoreName {
            name: name.into(),
            full_name: name.into(),
            race: Some("Dwarf".into()),
            class: Some(("Hunter".into(), "HUNTER".into())),
            class_id: 3,
            side,
        }
    }

    fn row(guid: u64, kb: u32, deaths: u32, honor: u32) -> BattlefieldScoreRow {
        BattlefieldScoreRow {
            guid,
            killing_blows: kb,
            deaths,
            bonus_honor: honor,
            ..Default::default()
        }
    }

    fn view() -> View {
        View {
            names: [
                (1, name("Cee", 1)),
                (2, name("Bee", 0)),
                (3, name("Aye", 1)),
            ]
            .into(),
            ..Default::default()
        }
    }

    fn names(s: &UiScript) -> String {
        s.eval::<String>(
            "local t = {} for i = 1, 3 do t[#t+1] = tostring((GetBattlefieldScore(i))) end \
             return table.concat(t, ',')",
        )
        .unwrap()
    }

    fn log() -> BattlefieldScores {
        BattlefieldScores {
            rows: vec![row(1, 2, 0, 10), row(2, 5, 1, 0), row(3, 2, 3, 0)],
            ..Default::default()
        }
    }

    #[test]
    fn rows_sort_by_kills_then_deaths_then_honor_then_name() {
        let mut s = UiScript::new().unwrap();
        let view = view();
        s.with_live(&view, |s| {
            s.set_battlefield_log(log());
            s.rebuild_battlefield_scores();
            assert_eq!(
                names(s),
                "Bee,Aye,Cee",
                "more deaths sort first at equal kills"
            );
            assert_eq!(s.arity("GetBattlefieldScore(1)").unwrap(), 12);
            assert_eq!(s.arity("GetBattlefieldScore(9)").unwrap(), 12);
            let got = s
                .eval::<String>(
                    "local n, kb, hk, d, h, f, rank, race, class, token = GetBattlefieldScore(1) \
                     return n .. kb .. d .. f .. rank .. race .. class .. token",
                )
                .unwrap();
            assert_eq!(got, "Bee5100DwarfHunterHUNTER", "the side is the race's");

            s.run("SortBattlefieldScoreData('name')").unwrap();
            assert_eq!(names(s), "Aye,Bee,Cee");
            s.run("SortBattlefieldScoreData('NAME')").unwrap();
            assert_eq!(names(s), "Cee,Bee,Aye", "a second click reverses");

            s.run("SetBattlefieldScoreFaction(0)").unwrap();
            assert_eq!(
                s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
                1
            );
            assert_eq!(names(s), "Bee,Cee,Aye", "the filtered team first");
        });
    }

    /// `0x54d280` writes the rows as it lands; count, sides and sort wait for the rebuild (`0x54aa39`).
    #[test]
    fn a_log_with_names_pending_holds_its_rows_in_wire_order() {
        let mut s = UiScript::new().unwrap();
        let mut view = view();
        s.with_live(&view, |s| {
            s.set_battlefield_log(BattlefieldScores {
                rows: vec![row(9, 1, 0, 0)],
                ..Default::default()
            });
            s.rebuild_battlefield_scores();
        });
        view.names.remove(&3);
        view.pending = true;
        s.with_live(&view, |s| {
            s.set_battlefield_log(log());
            s.rebuild_battlefield_scores();
            assert_eq!(
                s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
                1,
                "the count is the last rebuild's"
            );
            assert_eq!(
                names(s),
                "Cee,Bee,nil",
                "wire order, the uncached row empty"
            );
            s.run("SortBattlefieldScoreData('name') SetBattlefieldScoreFaction(1)")
                .unwrap();
            assert_eq!(
                names(s),
                "Cee,Bee,nil",
                "neither sorts while a name is pending"
            );
        });
        view.names.insert(3, name("Aye", 1));
        view.pending = false;
        s.with_live(&view, |s| {
            s.rebuild_battlefield_scores();
            assert_eq!(
                s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
                2,
                "the filter stored while pending applies at the rebuild"
            );
            assert_eq!(
                names(s),
                "Aye,Cee,Bee",
                "its team first, then kills and deaths"
            );
        });
    }

    /// An arena's rows keep their team byte; a battleground's take the race's side (`0x54aadf`).
    #[test]
    fn an_arena_keeps_the_wire_team() {
        let mut s = UiScript::new().unwrap();
        let view = View {
            instance_type: crate::script::INSTANCE_TYPE_ARENA,
            ..view()
        };
        s.with_live(&view, |s| {
            let mut log = log();
            log.rows[1].faction = 1;
            s.set_battlefield_log(log);
            s.rebuild_battlefield_scores();
            let got = s
                .eval::<String>("local n, _, _, _, _, f = GetBattlefieldScore(1) return n .. f")
                .unwrap();
            assert_eq!(got, "Bee1");
        });
    }

    /// An arena log writes the teams, a battleground's keeps them, only an ended log the winner (`0x54d280`).
    #[test]
    fn the_team_info_is_zero_based() {
        let mut s = UiScript::new().unwrap();
        s.set_battlefield_log(BattlefieldScores {
            teams: Some([
                BattlefieldTeamView {
                    name: "Gold".into(),
                    rating: 1500,
                    new_rating: 1516,
                    matchmaking: 1490,
                },
                Default::default(),
            ]),
            ended: true,
            winner: 1,
            ..Default::default()
        });
        s.set_battlefield_log(BattlefieldScores::default());
        let got = s
            .eval::<String>(
                "local n, r, nr, m = GetBattlefieldTeamInfo(0) return n .. r .. nr .. m",
            )
            .unwrap();
        assert_eq!(got, "Gold150015161490");
        assert!(s
            .eval::<bool>("return GetBattlefieldTeamInfo(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetBattlefieldWinner() == nil")
            .unwrap());
        s.set_battlefield_log(BattlefieldScores {
            ended: true,
            winner: 1,
            ..Default::default()
        });
        assert_eq!(s.eval::<i64>("return GetBattlefieldWinner()").unwrap(), 1);
        s.reset_battlefield_scores_on_world_enter();
        let got = s
            .eval::<String>(
                "local n, r = GetBattlefieldTeamInfo(0) \
                 return n .. r .. tostring(GetBattlefieldWinner())",
            )
            .unwrap();
        assert_eq!(got, "1500nil");
    }

    #[test]
    fn the_sort_names_map_to_columns() {
        assert_eq!(sort_column("kills"), Some(0));
        assert_eq!(sort_column("Healing"), Some(7));
        assert_eq!(sort_column("stat1"), Some(9));
        assert_eq!(sort_column("stat8"), Some(16));
        assert_eq!(sort_column("stat9"), None);
        assert_eq!(sort_column("bogus"), Some(0));
    }
}
