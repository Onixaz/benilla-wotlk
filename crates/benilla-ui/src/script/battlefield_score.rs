//! The scoreboard verbs of `WorldStateFrame.lua` (`CGBattlefieldInfo`, `0x549e80`-`0x54de00`),
//! over a board the app pushes once every row's name resolves, as the reference waits for the last
//! name (`BattlefieldNameCacheCallback`, `0x54aa30`). The faction filter sorts its team first
//! rather than compacting, so an index past the filtered count answers a real row.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;

/// One scoreboard row as the app resolved it, in wire order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScoreRow {
    /// The bare name, or `Name-Realm` for a cross-realm player (`NameCache::FullName`).
    pub name: String,
    pub killing_blows: u32,
    pub honorable_kills: u32,
    pub deaths: u32,
    pub bonus_honor: u32,
    /// A battleground's side from the race, `0` Horde, `1` Alliance, `-1` neither; an arena's
    /// team byte as sent.
    pub faction: i32,
    /// The race's display name, `None` for a race the tables lack (a nil return).
    pub race: Option<String>,
    /// The class's display name and `ChrClasses.dbc` file token, both or neither.
    pub class: Option<(String, String)>,
    /// `ChrClasses.dbc`'s id, the class column's sort key (`0x54a85c`).
    pub class_id: u32,
    pub damage_done: u32,
    pub healing_done: u32,
    /// The extra-stat dwords, eight slots (the client's block), unfilled ones zero.
    pub stats: [u32; 8],
}

/// A column header: a `WorldStateUI.dbc` `Type == 2` row for the map, its text not expanded.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldStatColumn {
    pub text: String,
    pub icon: String,
    pub tooltip: String,
}

/// An arena team's scoreboard line (`m_teamNames`, `m_teamRatings`, `0xbea0b0`..).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldTeamView {
    pub name: String,
    pub rating: u32,
    pub new_rating: u32,
    pub matchmaking: u32,
}

/// The whole board the app pushes.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScores {
    pub rows: Vec<BattlefieldScoreRow>,
    /// The match has ended (`m_mapDone`); gates the winner.
    pub ended: bool,
    /// The winning side, read only when `ended`.
    pub winner: u8,
    pub columns: Vec<BattlefieldStatColumn>,
    /// The two arena teams; empty strings and zeros outside an arena, as the reference's statics.
    pub teams: [BattlefieldTeamView; 2],
}

/// The sort's seventeen columns (`m_scoreSortCriteria`, `0xbea2c0`): kills, deaths, honor, name,
/// class, honorable kills, damage, healing, team, then the eight stats.
const SORT_COLUMNS: usize = 17;

/// The VM's half of the board: the rows, the filter, the sort criteria and the sorted order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScoreBoard {
    pub(crate) scores: BattlefieldScores,
    /// `SetBattlefieldScoreFaction`'s store (`m_logFilterFaction`): `-1` every team.
    pub(crate) filter: i32,
    /// `(column, descending)`, most significant first; column `i` at `i` to start.
    criteria: [(u32, bool); SORT_COLUMNS],
    pub(crate) order: Vec<usize>,
    /// The filtered count (`m_numScores`, `0xbea580`).
    pub(crate) filtered: usize,
}

impl Default for ScoreBoard {
    /// The filter starts at `-1` and each column at its own rank (`InitializeGame`, `0x54e720`).
    fn default() -> Self {
        Self {
            scores: BattlefieldScores::default(),
            filter: -1,
            criteria: std::array::from_fn(|i| (i as u32, false)),
            order: Vec::new(),
            filtered: 0,
        }
    }
}

impl ScoreBoard {
    /// `QSortScoreData` (`0x54a740`): the filtered team first, then each criterion in turn; the
    /// numbers sort high first, the name A to Z, the team low first.
    ///
    /// Deviation: the reference's class and stat columns answer "after" for an equal pair, which
    /// is no ordering; equal keys here compare equal and the sort keeps wire order.
    fn compare(&self, a: &BattlefieldScoreRow, b: &BattlefieldScoreRow) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        if self.filter != -1 && a.faction != b.faction {
            return (a.faction != self.filter).cmp(&(b.faction != self.filter));
        }
        let stats = self.scores.columns.len();
        for &(column, descending) in &self.criteria {
            let o = match column {
                0 => b.killing_blows.cmp(&a.killing_blows),
                1 => b.deaths.cmp(&a.deaths),
                2 => b.bonus_honor.cmp(&a.bonus_honor),
                3 => a.name.cmp(&b.name),
                4 => {
                    let o = b.class_id.cmp(&a.class_id);
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

    /// The recount and sort (`0x54aa30`).
    fn rebuild(&mut self) {
        let filter = self.filter;
        let rows = &self.scores.rows;
        self.filtered = if filter == -1 {
            rows.len()
        } else {
            rows.iter().filter(|r| r.faction == filter).count()
        };
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| self.compare(&rows[a], &rows[b]));
        self.order = order;
    }

    /// `SetScoreSortColumn` (`0x54cee0`): the column moves to the front, flipping its direction
    /// if it was already there.
    fn sort_by_column(&mut self, column: u32) {
        let Some(i) = self.criteria.iter().position(|&(c, _)| c == column) else {
            return;
        };
        let mut descending = self.criteria[i].1;
        if i == 0 {
            descending = !descending;
        }
        self.criteria.copy_within(0..i, 1);
        self.criteria[0] = (column, descending);
        self.rebuild();
    }

    fn row(&self, index_1based: i32) -> Option<&BattlefieldScoreRow> {
        let i = usize::try_from(index_1based).ok()?.checked_sub(1)?;
        // Bounded by the wire count (`m_numScoreEntries`), not the filtered one.
        self.order.get(i).map(|&r| &self.scores.rows[r])
    }
}

impl super::UiScript {
    /// Push the resolved board; the filter and the sort survive. The app fires
    /// `UPDATE_BATTLEFIELD_SCORE` itself.
    pub fn set_battlefield_scores(&mut self, scores: BattlefieldScores) {
        let mut model = self.model_mut();
        model.battlefield_board.scores = scores;
        model.battlefield_board.rebuild();
    }

    /// The status-3 reset of the filter (`0x54b0a6`), as the slot turns active.
    pub fn reset_battlefield_score_filter(&mut self) {
        let mut model = self.model_mut();
        model.battlefield_board.filter = -1;
        model.battlefield_board.rebuild();
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

    // Twelve values on every leg (`0x54be90`): name, kills, honorable kills, deaths, honor,
    // faction, a constant 0, race, class, class token, damage, healing.
    g.set(
        "GetBattlefieldScore",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldScore(index)")?;
            let model = read(lua);
            let Some(r) = model.battlefield_board.row(index) else {
                let mut v = vec![Value::Nil];
                v.extend(std::iter::repeat_n(Value::Number(0.0), 6));
                v.extend([Value::Nil, Value::Nil, Value::Nil]);
                v.extend([Value::Number(0.0), Value::Number(0.0)]);
                return Ok(MultiValue::from_vec(v));
            };
            let num = |v: u32| Value::Number(f64::from(v as i32));
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.name)?),
                num(r.killing_blows),
                num(r.honorable_kills),
                num(r.deaths),
                num(r.bonus_honor),
                Value::Number(f64::from(r.faction)),
                Value::Number(0.0),
                opt(lua, r.race.as_deref())?,
                opt(lua, r.class.as_ref().map(|c| c.0.as_str()))?,
                opt(lua, r.class.as_ref().map(|c| c.1.as_str()))?,
                num(r.damage_done),
                num(r.healing_done),
            ]))
        })?,
    )?;

    g.set(
        "GetBattlefieldWinner",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            let b = &model.battlefield_board.scores;
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
            let Some(t) = model.battlefield_board.scores.teams.get(index as usize) else {
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

    // Stores a filter of -1, 0 or 1 and re-sorts (`0x54c120`); a non-number stores -1.
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
            let mut model = crate::script::model_mut_of(lua);
            model.battlefield_board.filter = f;
            model.battlefield_board.rebuild();
            model
                .pending_events
                .push(("UPDATE_BATTLEFIELD_SCORE".to_string(), Vec::new()));
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
            let mut model = crate::script::model_mut_of(lua);
            model.battlefield_board.sort_by_column(column);
            model
                .pending_events
                .push(("UPDATE_BATTLEFIELD_SCORE".to_string(), Vec::new()));
            Ok(())
        })?,
    )?;

    g.set(
        "GetNumBattlefieldStats",
        lua.create_function(|lua, ()| Ok(read(lua).battlefield_board.scores.columns.len() as i64))?,
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
                .and_then(|i| model.battlefield_board.scores.columns.get(i));
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
    use crate::script::UiScript;

    fn row(name: &str, faction: i32, kb: u32, deaths: u32, honor: u32) -> BattlefieldScoreRow {
        BattlefieldScoreRow {
            name: name.into(),
            killing_blows: kb,
            deaths,
            bonus_honor: honor,
            faction,
            race: Some("Dwarf".into()),
            class: Some(("Hunter".into(), "HUNTER".into())),
            class_id: 3,
            ..Default::default()
        }
    }

    fn names(s: &UiScript) -> String {
        s.eval::<String>(
            "local t = {} for i = 1, 3 do t[#t+1] = (GetBattlefieldScore(i)) end \
             return table.concat(t, ',')",
        )
        .unwrap()
    }

    #[test]
    fn rows_sort_by_kills_then_deaths_then_honor_then_name() {
        let mut s = UiScript::new().unwrap();
        s.set_battlefield_scores(BattlefieldScores {
            rows: vec![
                row("Cee", 1, 2, 0, 10),
                row("Bee", 0, 5, 1, 0),
                row("Aye", 1, 2, 3, 0),
            ],
            ..Default::default()
        });
        assert_eq!(
            names(&s),
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
        assert_eq!(got, "Bee5100DwarfHunterHUNTER");

        s.run("SortBattlefieldScoreData('name')").unwrap();
        assert_eq!(names(&s), "Aye,Bee,Cee");
        s.run("SortBattlefieldScoreData('NAME')").unwrap();
        assert_eq!(names(&s), "Cee,Bee,Aye", "a second click reverses");

        s.run("SetBattlefieldScoreFaction(0)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
            1
        );
        assert_eq!(names(&s), "Bee,Cee,Aye", "the filtered team first");
    }

    #[test]
    fn the_team_info_is_zero_based() {
        let mut s = UiScript::new().unwrap();
        s.set_battlefield_scores(BattlefieldScores {
            teams: [
                BattlefieldTeamView {
                    name: "Gold".into(),
                    rating: 1500,
                    new_rating: 1516,
                    matchmaking: 1490,
                },
                Default::default(),
            ],
            ended: true,
            winner: 1,
            ..Default::default()
        });
        let got = s
            .eval::<String>(
                "local n, r, nr, m = GetBattlefieldTeamInfo(0) return n .. r .. nr .. m",
            )
            .unwrap();
        assert_eq!(got, "Gold150015161490");
        assert!(s
            .eval::<bool>("return GetBattlefieldTeamInfo(2) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetBattlefieldWinner()").unwrap(), 1);
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
