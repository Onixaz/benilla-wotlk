//! The raid browser's search list (`0x55b770`-`0x55d2b0`): the server's `SMSG_UPDATE_LFG_LIST`
//! fills the received list (`0xbeaa98`), and the verbs read the shown one (`0xbeab20`), a copy
//! taken on the first list after a join, on `RefreshLFGList` and on a sort. Groups and solo players
//! are kept in arrival order until a header sorts them.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mlua::{Lua, MultiValue, Value};

use super::api::{encounter_info, fire, num, read, text, values};
use super::engine::LfgModel;
use super::{LfgName, LfgRequest};
use crate::script::binding_abi::number_arg;

/// The event every change to the shown list fires (469).
const UPDATE: &str = "UPDATE_LFG_LIST";

/// A player's stats block, in wire order (`0x559050`, flag `0x1`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LfrStatsView {
    pub level: u8,
    pub class: u8,
    pub race: u8,
    pub talents: [u8; 3],
    pub armor: u32,
    pub spell_damage: u32,
    pub spell_heal: u32,
    pub crit_melee: u32,
    pub crit_ranged: u32,
    pub crit_spell: u32,
    pub mp5: f32,
    pub mp5_combat: f32,
    pub attack_power: u32,
    pub agility: u32,
    pub max_health: u32,
    pub max_mana: u32,
    pub unknown: u32,
    pub avg_item_level: f32,
    pub defense: u32,
    pub dodge: u32,
    pub block: u32,
    pub parry: u32,
    pub haste: u32,
    pub expertise: u32,
}

/// A group's record as the wire carries it; `None` is a field its flags left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LfrGroupView {
    pub guid: u64,
    pub comment: Option<String>,
    pub roles: Option<[u8; 3]>,
    pub encounter_mask: Option<u32>,
}

/// A player's record as the wire carries it; `None` is a field its flags left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LfrPlayerView {
    pub guid: u64,
    pub stats: Option<LfrStatsView>,
    pub comment: Option<String>,
    pub leader: Option<bool>,
    pub party: Option<u64>,
    pub roles: Option<u8>,
    pub area: Option<u32>,
    pub status: Option<u8>,
    pub encounter_mask: Option<u32>,
}

/// One `SMSG_UPDATE_LFG_LIST`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LfrListView {
    pub lfg_type: u32,
    pub dungeon: u32,
    /// `Some` for a difference: the guids gone since the last list.
    pub removed: Option<Vec<u64>>,
    pub total_groups: u32,
    pub groups: Vec<LfrGroupView>,
    pub total_players: u32,
    pub players: Vec<LfrPlayerView>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Player {
    guid: u64,
    stats: LfrStatsView,
    comment: String,
    leader: bool,
    party: u64,
    roles: u8,
    area: u32,
    status: u8,
    encounter_mask: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Party {
    guid: u64,
    comment: String,
    encounter_mask: u32,
    /// The leader first (`0x556e60`), the rest in arrival order.
    members: Vec<Player>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct List {
    /// The search key the list answers, `type << 24 | dungeon`; 0 for none.
    key: u32,
    total_players: u32,
    total_parties: u32,
    parties: Vec<Party>,
    /// The players in no listed group.
    players: Vec<Player>,
}

/// Where a player sits in a list.
#[derive(Clone, Copy)]
enum Seat {
    Solo(usize),
    Party(usize, usize),
}

impl List {
    fn seat(&self, guid: u64) -> Option<Seat> {
        if let Some(i) = self.players.iter().position(|p| p.guid == guid) {
            return Some(Seat::Solo(i));
        }
        self.parties.iter().enumerate().find_map(|(pi, party)| {
            let mi = party.members.iter().position(|m| m.guid == guid)?;
            Some(Seat::Party(pi, mi))
        })
    }

    fn take(&mut self, seat: Seat) -> Player {
        match seat {
            Seat::Solo(i) => self.players.remove(i),
            Seat::Party(pi, mi) => self.parties[pi].members.remove(mi),
        }
    }

    fn get_mut(&mut self, seat: Seat) -> &mut Player {
        match seat {
            Seat::Solo(i) => &mut self.players[i],
            Seat::Party(pi, mi) => &mut self.parties[pi].members[mi],
        }
    }

    /// The party add (`0x556e60`): a leader goes first; no listed party leaves the player solo.
    fn seat_player(&mut self, player: Player) {
        match self
            .parties
            .iter_mut()
            .find(|p| p.guid == player.party && player.party != 0)
        {
            Some(party) if player.leader => party.members.insert(0, player),
            Some(party) => party.members.push(player),
            None => self.players.push(player),
        }
    }

    /// The `index`-th row as the verbs count them (`0x553530`): the solo players then the groups
    /// by default, the other way round once "group" is sorted; a group reads as its leader.
    fn row(&self, solo_first: bool, index: usize) -> Option<(Option<&Party>, Option<&Player>)> {
        let solo = |i: usize| self.players.get(i).map(|p| (None, Some(p)));
        let party = |i: usize| self.parties.get(i).map(|p| (Some(p), p.members.first()));
        if solo_first {
            solo(index).or_else(|| party(index.checked_sub(self.players.len())?))
        } else {
            party(index).or_else(|| solo(index.checked_sub(self.parties.len())?))
        }
    }
}

/// A removed guid names a group when its high bits are a group's (`0x55b81d`).
fn is_party_guid(guid: u64) -> bool {
    guid >> 32 & 0xF000_0000 != 0 || guid & 0xF07F_FFFF_FFFF_FFFF == 0
}

/// The raid browser's search state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LfrSearch {
    /// The joined key (`0xbeaa14`), 0 for none.
    joined: u32,
    received: List,
    shown: List,
    /// The seven sort keys in priority order, each with its reverse flag (`0xbea820`).
    sort: [(u8, bool); 7],
    /// Solo players list ahead of groups (`0xacd660`).
    solo_first: bool,
    /// Players whose name the list still waits on (`0x5528a0`).
    unnamed: HashSet<u64>,
    /// `AreaTable.dbc` names the app resolved, for the zone column and its sort.
    pub(crate) area_names: HashMap<u32, String>,
}

impl Default for LfrSearch {
    /// The startup state (`0x55d440`): keys in their natural order, solo players first.
    fn default() -> Self {
        Self {
            joined: 0,
            received: List::default(),
            shown: List::default(),
            sort: std::array::from_fn(|i| (i as u8, false)),
            solo_first: true,
            unnamed: HashSet::new(),
            area_names: HashMap::new(),
        }
    }
}

impl LfrSearch {
    /// The guids whose names the list reads.
    pub(crate) fn guids(&self) -> impl Iterator<Item = u64> + '_ {
        [&self.received, &self.shown].into_iter().flat_map(|l| {
            l.players
                .iter()
                .chain(l.parties.iter().flat_map(|p| p.members.iter()))
                .map(|p| p.guid)
        })
    }

    /// The areas the list names.
    pub(crate) fn areas(&self) -> impl Iterator<Item = u32> + '_ {
        [&self.received, &self.shown].into_iter().flat_map(|l| {
            l.players
                .iter()
                .chain(l.parties.iter().flat_map(|p| p.members.iter()))
                .map(|p| p.area)
                .filter(|&a| a != 0)
        })
    }

    /// The join (`0x558ed0`): a new key empties both lists and is sent.
    fn join(&mut self, key: u32, requests: &mut Vec<LfgRequest>) -> bool {
        if key == self.joined {
            return false;
        }
        self.received = List::default();
        self.joined = key;
        self.shown = List::default();
        requests.push(LfgRequest::LfrJoin(key));
        true
    }

    /// The leave (`0x558f90`).
    fn leave(&mut self, requests: &mut Vec<LfgRequest>) -> bool {
        if self.joined == 0 {
            return false;
        }
        requests.push(LfgRequest::LfrLeave(self.joined));
        self.drop_search();
        true
    }

    fn drop_search(&mut self) {
        self.received = List::default();
        self.joined = 0;
        self.shown = List::default();
    }

    /// `SMSG_LFG_UPDATE_SEARCH` (`0x55c68b`): the server dropped the search. True to fire.
    pub(crate) fn update_search(&mut self, joined: bool) -> bool {
        if joined || self.joined == 0 {
            return false;
        }
        self.drop_search();
        true
    }

    /// `SMSG_UPDATE_LFG_LIST` (`0x55b770`). A list for another key is dropped; the first one after
    /// a join is copied to the shown list, which is then true to fire; later ones edit the
    /// received list alone.
    pub(crate) fn apply(&mut self, list: LfrListView, names: &HashMap<u64, LfgName>) -> bool {
        let key = list.lfg_type << 24 | list.dungeon & 0xFF_FFFF;
        if key != self.joined {
            return false;
        }
        let changed = key != self.received.key;
        let r = &mut self.received;
        match list.removed {
            None => *r = List::default(),
            Some(guids) => {
                for guid in guids {
                    if is_party_guid(guid) {
                        r.parties.retain(|p| p.guid != guid);
                    } else if let Some(seat) = r.seat(guid) {
                        r.take(seat);
                    }
                }
            }
        }
        r.key = key;
        r.total_parties = list.total_groups;
        r.total_players = list.total_players;
        for g in list.groups {
            let i = match r.parties.iter().position(|p| p.guid == g.guid) {
                Some(i) => i,
                None => {
                    r.parties.push(Party {
                        guid: g.guid,
                        ..Party::default()
                    });
                    r.parties.len() - 1
                }
            };
            let party = &mut r.parties[i];
            if let Some(c) = g.comment {
                party.comment = c;
            }
            if let Some(m) = g.encounter_mask {
                party.encounter_mask = m;
            }
        }
        for p in list.players {
            let seat = match r.seat(p.guid) {
                Some(seat) => seat,
                None => {
                    if !names.contains_key(&p.guid) {
                        self.unnamed.insert(p.guid);
                    }
                    r.players.push(Player {
                        guid: p.guid,
                        ..Player::default()
                    });
                    Seat::Solo(r.players.len() - 1)
                }
            };
            let player = r.get_mut(seat);
            if let Some(s) = p.stats {
                player.stats = s;
            }
            if let Some(c) = p.comment {
                player.comment = c;
            }
            let mut reseat = false;
            if let Some(leader) = p.leader {
                // A leader change moves a member to its group's head or off it.
                reseat |= leader != player.leader && player.party != 0;
                player.leader = leader;
            }
            if let Some(party) = p.party {
                reseat |= party != player.party || matches!(seat, Seat::Solo(_)) && party != 0;
                player.party = party;
            }
            if let Some(roles) = p.roles {
                player.roles = roles;
            }
            if let Some(area) = p.area {
                player.area = area;
            }
            if let Some(status) = p.status {
                player.status = status;
            }
            if let Some(mask) = p.encounter_mask {
                player.encounter_mask = mask;
            }
            if reseat {
                let player = r.take(seat);
                r.seat_player(player);
            }
        }
        if changed {
            self.shown = self.received.clone();
        }
        changed
    }

    /// Names landed (`0x5528a0`): the last one awaited fires the list's event. True to fire.
    pub(crate) fn names_landed(&mut self, names: &HashMap<u64, LfgName>) -> bool {
        if self.unnamed.is_empty() {
            return false;
        }
        self.unnamed.retain(|g| !names.contains_key(g));
        self.unnamed.is_empty()
    }

    /// `SearchLFGSort` (`0x55d2b0`): "group" flips the solo-first order; any other key moves to the
    /// front, or reverses when already there, and the received list is sorted and shown.
    fn sort_by(&mut self, kind: &str, names: &HashMap<u64, LfgName>) {
        let key = match kind.to_ascii_lowercase().as_str() {
            "group" => {
                self.solo_first = !self.solo_first;
                return;
            }
            "zone" => 0,
            "level" => 1,
            "class" => 2,
            "tank" => 4,
            "healer" => 5,
            "damage" => 6,
            _ => 3,
        };
        if self.sort[0].0 == key {
            self.sort[0].1 = !self.sort[0].1;
        } else if let Some(i) = self.sort.iter().position(|s| s.0 == key) {
            let entry = self.sort[i];
            self.sort.copy_within(0..i, 1);
            self.sort[0] = entry;
        }
        // The re-sort (`0x55b700`) sorts the received list and shows it.
        let sort = self.sort;
        let areas = &self.area_names;
        let player_cmp = |a: &Player, b: &Player| compare(a, b, &sort, names, areas, true);
        self.received.players.sort_by(player_cmp);
        self.received
            .parties
            .sort_by(|a, b| match (a.members.first(), b.members.first()) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(x), Some(y)) => compare(x, y, &sort, names, areas, false),
            });
        self.shown = self.received.clone();
    }
}

/// The comparators (`0x555cd0` players, `0x555a70` group leaders): the keys in priority order,
/// the first difference wins, reversed by its flag. Groups skip the role keys.
fn compare(
    a: &Player,
    b: &Player,
    sort: &[(u8, bool); 7],
    names: &HashMap<u64, LfgName>,
    areas: &HashMap<u32, String>,
    roles: bool,
) -> Ordering {
    let ci = |x: Option<&String>, y: Option<&String>| match (x, y) {
        (Some(x), Some(y)) if !x.is_empty() && !y.is_empty() => {
            x.to_lowercase().cmp(&y.to_lowercase())
        }
        _ => Ordering::Equal,
    };
    let role = |bit: u8| (b.roles & bit).cmp(&(a.roles & bit));
    for &(key, reverse) in sort {
        let o = match key {
            0 => ci(areas.get(&a.area), areas.get(&b.area)),
            1 => a.stats.level.cmp(&b.stats.level),
            2 => ci(
                names.get(&a.guid).map(|n| &n.class_name),
                names.get(&b.guid).map(|n| &n.class_name),
            ),
            3 => match (names.get(&a.guid), names.get(&b.guid)) {
                (Some(x), Some(y)) => x.name.cmp(&y.name),
                _ => Ordering::Equal,
            },
            4 if roles => role(0x2),
            5 if roles => role(0x4),
            6 if roles => role(0x8),
            _ => Ordering::Equal,
        };
        if o != Ordering::Equal {
            return if reverse { o.reverse() } else { o };
        }
    }
    Ordering::Equal
}

/// The 28 values every result ends with (`0x5529e0`); with no player, eight nils, `false`, then
/// nineteen nils.
fn stats_values(p: Option<&Player>, out: &mut Vec<Value>) {
    let Some(p) = p else {
        out.extend(std::iter::repeat_n(Value::Nil, 8));
        out.push(Value::Boolean(false));
        out.extend(std::iter::repeat_n(Value::Nil, 19));
        return;
    };
    let s = &p.stats;
    for bit in [0x1, 0x2, 0x4, 0x8] {
        out.push(Value::Boolean(p.roles & bit != 0));
    }
    out.push(num(s.unknown));
    out.extend(s.talents.iter().map(|&t| num(t)));
    out.push(Value::Boolean(p.leader));
    for v in [
        s.armor,
        s.spell_damage,
        s.spell_heal,
        s.crit_melee,
        s.crit_ranged,
        s.crit_spell,
        s.mp5 as u32,
        s.mp5_combat as u32,
        s.attack_power,
        s.agility,
        s.max_health,
        s.max_mana,
        s.avg_item_level as u32,
        s.defense,
        s.dodge,
        s.block,
        s.parry,
        s.haste,
        s.expertise,
    ] {
        out.push(num(v));
    }
}

/// The shown list's `LfgDungeons.dbc` row's map and difficulty, `None` without a row.
fn dungeon_scope(lfg: &LfgModel) -> Option<(i32, i32)> {
    let id = (lfg.search.shown.key & 0xFF_FFFF) as i32;
    lfg.catalog.dungeon(id).map(|d| (d.map_id, d.difficulty))
}

fn unknown(lua: &Lua) -> String {
    lua.globals()
        .get::<String>("UNKNOWN")
        .unwrap_or_else(|_| "Unknown".into())
}

fn index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<Option<usize>> {
    let n = number_arg(lua, v, usage)?;
    Ok(usize::try_from((n as i64).wrapping_sub(1)).ok())
}

/// Register the search list's globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // SearchLFGGetNumResults() (`0x553ba0`): the rows, then the server's totals.
    g.set(
        "SearchLFGGetNumResults",
        lua.create_function(|lua, _: MultiValue| {
            read(lua, |m| {
                let l = &m.lfg.search.shown;
                if l.key == 0 {
                    return Ok((0, 0));
                }
                Ok((
                    (l.players.len() + l.parties.len()) as i64,
                    i64::from(l.total_parties) + i64::from(l.total_players),
                ))
            })
        })?,
    )?;

    // SearchLFGGetJoinedID() (`0x5529a0`): the joined dungeon, nothing when none.
    g.set(
        "SearchLFGGetJoinedID",
        lua.create_function(|lua, _: MultiValue| {
            let id = read(lua, |m| m.lfg.search.joined & 0xFF_FFFF);
            Ok(if id == 0 {
                MultiValue::new()
            } else {
                values(vec![num(id)])
            })
        })?,
    )?;

    // SearchLFGGetResults(index) (`0x555fd0`): a row's ten values and its 28 stats, a group's
    // read off its leader; nothing without a dungeon row, past the list or with no player.
    g.set(
        "SearchLFGGetResults",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: SearchLFGGetResults(index)")?;
            let unknown = unknown(lua);
            read(lua, |m| {
                let lfg = &m.lfg;
                let s = &lfg.search;
                let Some((map, difficulty)) = dungeon_scope(lfg) else {
                    return Ok(MultiValue::new());
                };
                let Some((party, Some(p))) = index.and_then(|i| s.shown.row(s.solo_first, i))
                else {
                    return Ok(MultiValue::new());
                };
                let name = lfg.names.get(&p.guid);
                let mask = party.map_or(p.encounter_mask, |party| party.encounter_mask);
                let encounters: Vec<_> = lfg
                    .catalog
                    .view
                    .encounters
                    .iter()
                    .filter(|e| e.map_id == map && e.difficulty == difficulty)
                    .collect();
                let killed = encounters
                    .iter()
                    .filter(|e| mask & 1u32.wrapping_shl(e.bit as u32) != 0)
                    .count();
                let comment = party.map_or(&p.comment, |party| &party.comment);
                let class_file = name
                    .map(|n| n.class_file.as_str())
                    .filter(|f| !f.is_empty());
                let mut out = vec![
                    text(lua, name.map_or(unknown.as_str(), |n| n.name.as_str()))?,
                    num(p.stats.level),
                    text(lua, s.area_names.get(&p.area).unwrap_or(&unknown))?,
                    text(
                        lua,
                        name.map(|n| n.class_name.as_str())
                            .filter(|c| !c.is_empty())
                            .unwrap_or(&unknown),
                    )?,
                    text(lua, comment)?,
                    num(party.map_or(0, |party| party.members.len().saturating_sub(1)) as u32),
                    Value::Boolean(p.status != 0),
                    match class_file {
                        Some(f) => text(lua, f)?,
                        None => Value::Nil,
                    },
                    num(encounters.len() as u32),
                    num(killed as u32),
                ];
                stats_values(Some(p), &mut out);
                Ok(values(out))
            })
        })?,
    )?;

    // SearchLFGGetPartyResults(index, partyIndex) (`0x556280`): a group's other member, or the
    // miss tuple; nothing past the list.
    g.set(
        "SearchLFGGetPartyResults",
        lua.create_function(|lua, (index, member): (Value, Value)| {
            let usage = "Usage: SearchLFGGetPartyResults(index, partyIndex)";
            let index = index_arg(lua, index, usage)?;
            let member = number_arg(lua, member, usage)? as i64;
            let unknown = unknown(lua);
            read(lua, |m| {
                let lfg = &m.lfg;
                let s = &lfg.search;
                let Some((party, _)) = index.and_then(|i| s.shown.row(s.solo_first, i)) else {
                    return Ok(MultiValue::new());
                };
                let hit = party
                    .zip(usize::try_from(member).ok())
                    .and_then(|(party, i)| party.members.get(i))
                    .and_then(|p| Some((p, lfg.names.get(&p.guid)?)));
                let mut out = Vec::with_capacity(34);
                match hit {
                    Some((p, name)) => {
                        let friend = m.social.friends.iter().any(|f| f.guid == p.guid);
                        let ignored = m
                            .social
                            .ignores
                            .iter()
                            .any(|i| i.eq_ignore_ascii_case(&name.name));
                        out.push(text(lua, &name.name)?);
                        out.push(num(p.stats.level));
                        out.push(if ignored {
                            text(lua, "ignored")?
                        } else if friend {
                            text(lua, "friend")?
                        } else {
                            Value::Nil
                        });
                        out.push(text(lua, &name.class_name)?);
                        out.push(text(lua, s.area_names.get(&p.area).unwrap_or(&unknown))?);
                        out.push(text(lua, &p.comment)?);
                        stats_values(Some(p), &mut out);
                    }
                    None => {
                        out.extend([
                            Value::Nil,
                            text(lua, "")?,
                            Value::Nil,
                            Value::Nil,
                            Value::Nil,
                        ]);
                        stats_values(None, &mut out);
                    }
                }
                Ok(values(out))
            })
        })?,
    )?;

    // SearchLFGGetEncounterResults(index, encounterIndex) (`0x553c30`): a boss of the row's raid,
    // its icon and whether the row's lockout killed it.
    g.set(
        "SearchLFGGetEncounterResults",
        lua.create_function(|lua, (index, encounter): (Value, Value)| {
            let usage = "Usage: SearchLFGGetEncounterResults(index, encounterIndex)";
            let index = index_arg(lua, index, usage)?;
            let encounter = number_arg(lua, encounter, usage)? as i64 - 1;
            read(lua, |m| {
                let lfg = &m.lfg;
                let s = &lfg.search;
                let Some((map, difficulty)) = dungeon_scope(lfg) else {
                    return Ok(MultiValue::new());
                };
                let Some((party, player)) = index.and_then(|i| s.shown.row(s.solo_first, i)) else {
                    return Ok(MultiValue::new());
                };
                match player {
                    None => encounter_info(lua, lfg, 0, 0, 0, 0),
                    Some(p) => {
                        let mask = party.map_or(p.encounter_mask, |party| party.encounter_mask);
                        encounter_info(lua, lfg, map, difficulty, encounter, mask)
                    }
                }
            })
        })?,
    )?;

    // SearchLFGJoin(typeID, lfgID) (`0x559400`): a type below 7 and a known row join its search.
    g.set(
        "SearchLFGJoin",
        lua.create_function(|lua, (kind, id): (Value, Value)| {
            let usage = "Usage: SearchLFGJoin(typeID, lfgID)";
            let kind = number_arg(lua, kind, usage)? as i64;
            let id = number_arg(lua, id, usage)? as i64;
            let fired = {
                let mut model = crate::script::model_mut_of(lua);
                let lfg = &mut model.lfg;
                if !(0..7).contains(&kind) || lfg.catalog.dungeon(id as i32).is_none() {
                    return Ok(());
                }
                let key = (kind as u32) << 24 | (id as u32 & 0xFF_FFFF);
                lfg.search.join(key, &mut lfg.requests)
            };
            if fired {
                fire(lua, UPDATE);
            }
            Ok(())
        })?,
    )?;

    // SearchLFGLeave() (`0x5594e0`).
    g.set(
        "SearchLFGLeave",
        lua.create_function(|lua, _: MultiValue| {
            let fired = {
                let mut model = crate::script::model_mut_of(lua);
                let lfg = &mut model.lfg;
                lfg.search.leave(&mut lfg.requests)
            };
            if fired {
                fire(lua, UPDATE);
            }
            Ok(())
        })?,
    )?;

    // RefreshLFGList() (`0x55d280`): the received list is shown as it stands.
    g.set(
        "RefreshLFGList",
        lua.create_function(|lua, _: MultiValue| {
            {
                let mut model = crate::script::model_mut_of(lua);
                let s = &mut model.lfg.search;
                s.shown = s.received.clone();
            }
            fire(lua, UPDATE);
            Ok(())
        })?,
    )?;

    // SearchLFGSort(type) (`0x55d2b0`).
    g.set(
        "SearchLFGSort",
        lua.create_function(|lua, kind: Value| {
            let kind = match kind {
                Value::String(s) => s.to_string_lossy(),
                _ => return Err(mlua::Error::runtime("Usage: SearchLFGSort(type)")),
            };
            {
                let mut model = crate::script::model_mut_of(lua);
                let lfg = &mut model.lfg;
                let names = std::mem::take(&mut lfg.names);
                lfg.search.sort_by(&kind, &names);
                lfg.names = names;
            }
            fire(lua, UPDATE);
            Ok(())
        })?,
    )?;

    Ok(())
}
