//! The skills-pane bindings behind the stock `SkillFrame`. The client's skill list
//! (`0xc22ae8`-`0xc22b18`, [`SkillList`]) is the game's, not the UI's: the game start sets it up
//! (`0x5ce880`), the rebuild alone writes its rows (`0x5cd9b0`), and only the game shutdown frees it
//! (`0x5cd3a0`), so it outlives a UI reload and the verbs read it through the lent view at the call.
//! The rebuild resolves the known lines from `SkillLine.dbc` and `SkillLineCategory.dbc`, and the
//! list groups, sorts and folds them as the client's list build does: one header per category, by
//! `category_order` (category id breaks a tie), and within a category the untrained lines (rank 0)
//! after the trained ones, each by name. As in `0x5cd5e0`, the visible rows come first and the
//! folded groups' entries after them, which every index but `GetNumSkillLines` reaches; the
//! selection is held by skill id. The ranks and bonuses each row shows are read off the player at
//! the call (`0x5cdfe7`).
//!
//! `numTempPoints` is what `AddSkillUp` has pending on the line, which a rebuild drops. `stepCost` is
//! the tier the line's flags open and `rankCost` the next rank's price; the shipped data gives
//! neither a value on a line a player holds.

use std::collections::{HashMap, HashSet};

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::live::LiveState;

/// One known skill line off the player's `PLAYER_SKILL_INFO` block, resolved by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillEntry {
    pub skill_id: u32,
    /// The `SkillLine.dbc` name.
    pub name: String,
    /// The raw rank off the descriptor; 0 is untrained, which sorts below the trained lines.
    pub value: u32,
    /// The raw max rank off the descriptor; [`Self::mono`] overrides it at the API return.
    pub max: u32,
    /// `SkillRaceClassInfo.flags & 0x400` (`SKILL_FLAG_MONO_VALUE`): a single-rank line, whose
    /// `skillMaxRank` the client reports as 1 whatever the descriptor says, so vmangos's 300/300
    /// Beast Mastery reads as a proficiency.
    pub mono: bool,
    /// The temporary bonus, possibly negative: `skillModifier`, a signed read of the descriptor's
    /// temp half alone (`+0x850`).
    pub temp_bonus: i32,
    /// The permanent bonus (`+0x852`), possibly negative, folded into both ranks when they are
    /// nonzero.
    pub perm_bonus: i32,
    /// `SkillRaceClassInfo.reqLevel`, `GetSkillLineInfo`'s 11th return.
    pub min_level: u32,
    /// `SkillRaceClassInfo.skillCostID`; the 12th return is this plus one, added by the engine.
    pub cost_index: u32,
    /// `SkillLine.dbc` categoryId.
    pub category_id: u32,
    /// The category name, the header text.
    pub category_name: String,
    /// `SkillLineCategory` displayOrder, the group sort key.
    pub category_order: u32,
    /// `SkillLine.dbc` description (enUS column 12), `GetSkillLineInfo`'s 13th return.
    pub description: String,
    /// `isAbandonable`, the 8th return: a nonzero skill step and `SkillRaceClassInfo.flags & 0x20`
    /// (`SKILL_FLAG_UNLEARNABLE`), both checked by the client.
    pub abandonable: bool,
    /// The rank costs for the line's cost index, the first for rank 1 (`0x810f50`).
    pub rank_costs: std::sync::Arc<[u32]>,
    /// The next tier's cost where the line's flags open a purchase, else 0.
    pub tier_cost: u32,
}

/// A flat, unordered push of every known skill line; the engine groups and sorts it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SkillsState {
    pub entries: Vec<SkillEntry>,
    /// `PLAYER_CHARACTER_POINTS2`.
    pub points: u32,
}

/// A skill buy, for the app to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillRequest {
    /// `AcceptSkillUps()` (`0x5ce5d0`): `(line, points)` per visible line with points pending.
    BuyRanks(Vec<(u32, u32)>),
    /// `BuySkillTier(index)` (`0x5ce6f0`).
    BuyTier(u32),
}

/// One category of the display tree, with its lines' positions in [`SkillsState::entries`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SkillGroup {
    category_id: u32,
    name: String,
    order: u32,
    /// Positions into [`SkillsState::entries`], in display order.
    entries: Vec<usize>,
}

/// The client's skill list (`0xc22ae8`-`0xc22b18`): the rebuilt rows, the fold state
/// (`0xc22af8`), the selected line (`0xc22af4`) and the pending skill-ups with their cost
/// (`0xc22b00`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillList {
    state: SkillsState,
    groups: Vec<SkillGroup>,
    /// Collapsed skill categories by id.
    collapsed: HashSet<u32>,
    /// The selection as a skill id, not a row (`0x5cd8ca`).
    selected: Option<u32>,
    /// `AddSkillUp`'s pending points by skill line.
    ups: HashMap<u32, u32>,
    /// The cost of the pending points (`[0xc22b00]`).
    up_points: i32,
}

impl SkillList {
    /// The rebuild (`0x5cd9b0`): the new rows, every group expanded (`0x5cd9e1`) and no points
    /// pending (`0x5cd9fd`); the selected id is not touched, and reads 0 while no visible row holds
    /// it (`0x5cd360`).
    pub fn rebuild(&mut self, state: SkillsState) {
        self.groups = build_groups(&state.entries);
        self.state = state;
        self.collapsed.clear();
        self.ups.clear();
        self.up_points = 0;
    }
}

impl super::UiScript {
    /// Rebuild the list the verbs read where no view lends the game's, for the VM's own tests.
    pub fn set_skills(&mut self, state: SkillsState) {
        self.model_mut().skill_list.rebuild(state);
    }
}

/// One skill slot as `GetSkillLineInfo` reads it off the player at the call (`0x5cdfe7`-`0x5ce00a`): the ranks, the two bonuses, and what the step decides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkillLive {
    pub value: u32,
    pub max: u32,
    pub temp_bonus: i32,
    pub perm_bonus: i32,
    /// A nonzero step and the unlearnable flag (`0x5ce147`).
    pub abandonable: bool,
    /// The tier the flags and the step open, else 0 (`0x5ce16d`).
    pub tier_cost: u32,
}

/// `f` over the skill list at the call and the lent view: the view's list, else the model's.
fn read_list<R>(lua: &Lua, f: impl FnOnce(&SkillList, Option<&dyn LiveState>) -> R) -> R {
    let mut f = Some(f);
    if let Some(Some(out)) = super::read_live(|l| {
        l.skill_list()
            .map(|list| (f.take().expect("unrun"))(list, Some(l)))
    }) {
        return out;
    }
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let out = super::read_live(|l| (f.take().expect("unrun"))(&model.skill_list, Some(l)));
    out.unwrap_or_else(|| (f.take().expect("unrun"))(&model.skill_list, None))
}

/// `f` over the skill list at the call, mutably: the view's list, else the model's.
fn write_list<R>(lua: &Lua, f: impl FnOnce(&mut SkillList) -> R) -> R {
    let mut f = Some(f);
    if let Some(Some(out)) = super::write_live(|l| {
        l.skill_list_mut()
            .map(|list| (f.take().expect("unrun"))(list))
    }) {
        return out;
    }
    (f.take().expect("unrun"))(&mut crate::script::model_mut_of(lua).skill_list)
}

/// `e` with the player's slot read at the call (`0x5cdfe7`): a line the player no longer holds reads slot -1 unchecked (`0x6dc222`); with no view lent, as rebuilt.
fn live_entry<'a>(
    e: &'a SkillEntry,
    live: Option<&dyn LiveState>,
) -> std::borrow::Cow<'a, SkillEntry> {
    match live.and_then(|l| l.skill_slot(e.skill_id)) {
        None => std::borrow::Cow::Borrowed(e),
        Some(slot) => std::borrow::Cow::Owned(SkillEntry {
            value: slot.value,
            max: slot.max,
            temp_bonus: slot.temp_bonus,
            perm_bonus: slot.perm_bonus,
            abandonable: slot.abandonable,
            tier_cost: slot.tier_cost,
            ..e.clone()
        }),
    }
}

/// `PLAYER_CHARACTER_POINTS2` read at the call (`0x5ce815`); with no view lent, as rebuilt.
fn skill_points(list: &SkillList, live: Option<&dyn LiveState>) -> u32 {
    live.and_then(|l| l.skill_points())
        .unwrap_or(list.state.points)
}

/// One visible row: a header (its group index) or an entry (its position in
/// [`SkillsState::entries`]).
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Entry(usize),
}

/// The client's `stricmp`, approximated: case-insensitive, raw bytes as the tie-break.
fn collate(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Build the display tree by the module doc's grouping rule.
fn build_groups(entries: &[SkillEntry]) -> Vec<SkillGroup> {
    let mut map: HashMap<u32, SkillGroup> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        map.entry(e.category_id)
            .or_insert_with(|| SkillGroup {
                category_id: e.category_id,
                name: e.category_name.clone(),
                order: e.category_order,
                entries: Vec::new(),
            })
            .entries
            .push(i);
    }
    let mut groups: Vec<SkillGroup> = map.into_values().collect();
    for g in &mut groups {
        // Untrained below trained, then by name (the reference compares the untrained byte, before
        // the `stricmp`).
        g.entries.sort_by(|&a, &b| {
            (entries[a].value == 0)
                .cmp(&(entries[b].value == 0))
                .then_with(|| collate(&entries[a].name, &entries[b].name))
        });
    }
    groups.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then(a.category_id.cmp(&b.category_id))
    });
    groups
}

/// The whole list: headers and expanded entries, then the folded entries the line sort puts last.
fn rows(list: &SkillList) -> Vec<Row> {
    let mut out = Vec::new();
    let mut folded = Vec::new();
    for (gi, g) in list.groups.iter().enumerate() {
        out.push(Row::Header(gi));
        let collapsed = list.collapsed.contains(&g.category_id);
        let sink = if collapsed { &mut folded } else { &mut out };
        sink.extend(g.entries.iter().map(|&ei| Row::Entry(ei)));
    }
    out.extend(folded);
    out
}

/// `GetNumSkillLines()` (`0x5cd1b0`): the visible rows, the list less the folded entries.
fn num_rows(list: &SkillList) -> usize {
    let folded: usize = list
        .groups
        .iter()
        .filter(|g| list.collapsed.contains(&g.category_id))
        .map(|g| g.entries.len())
        .sum();
    rows(list).len() - folded
}

/// The entry at a 1-based index into the whole list; `None` for a header.
fn entry_at(list: &SkillList, index: usize) -> Option<&SkillEntry> {
    let n = index.checked_sub(1)?;
    match rows(list).get(n)? {
        Row::Entry(ei) => list.state.entries.get(*ei),
        Row::Header(_) => None,
    }
}

/// The category of the row at a 0-based index into the whole list, header or entry.
fn category_at(list: &SkillList, n: usize) -> Option<u32> {
    match *rows(list).get(n)? {
        Row::Header(gi) => list.groups.get(gi).map(|g| g.category_id),
        Row::Entry(ei) => list.state.entries.get(ei).map(|e| e.category_id),
    }
}

/// A rank's cost (`0x810f50`), 0 past the table.
fn rank_cost(e: &SkillEntry, rank: i64) -> u32 {
    usize::try_from(rank - 1)
        .ok()
        .and_then(|i| e.rank_costs.get(i))
        .copied()
        .unwrap_or(0)
}

/// The points `AddSkillUp` has pending on a line.
fn skill_ups(list: &SkillList, skill_id: u32) -> u32 {
    list.ups.get(&skill_id).copied().unwrap_or(0)
}

/// Whether the active player is held, which the skill-up verbs and the adjusted points require.
fn player_held(lua: &Lua) -> bool {
    lua.app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .player_guid()
        != 0
}

/// `AddSkillUp` (`0x5cdbf0`): one more point on the line, if the rank and the points allow it.
fn add_skill_up(lua: &Lua, index: usize) -> bool {
    if !player_held(lua) {
        return false;
    }
    let step = read_list(lua, |list, live| {
        // The ranks and the points are read at the call (`0x5cdca4`, `0x5cdce5`).
        let e = live_entry(entry_at(list, index)?, live);
        let ups = skill_ups(list, e.skill_id);
        if e.value + ups >= e.max {
            return None;
        }
        let cost = i64::from(rank_cost(&e, i64::from(e.value + ups + 1)));
        if cost + i64::from(list.up_points) > i64::from(skill_points(list, live)) {
            return None;
        }
        Some((e.skill_id, ups + 1, cost as i32))
    });
    let Some((id, ups, cost)) = step else {
        return false;
    };
    write_list(lua, |list| {
        list.ups.insert(id, ups);
        list.up_points += cost;
    });
    true
}

/// `RemoveSkillUp` (`0x5cdd20`): one point back off the line.
fn remove_skill_up(lua: &Lua, index: usize) -> bool {
    if !player_held(lua) {
        return false;
    }
    let step = read_list(lua, |list, live| {
        let e = live_entry(entry_at(list, index)?, live);
        let ups = skill_ups(list, e.skill_id);
        if ups == 0 {
            return None;
        }
        Some((
            e.skill_id,
            ups - 1,
            rank_cost(&e, i64::from(e.value + ups)) as i32,
        ))
    });
    let Some((id, ups, cost)) = step else {
        return false;
    };
    write_list(lua, |list| {
        list.ups.insert(id, ups);
        list.up_points -= cost;
    });
    true
}

/// Fold or unfold the row's group, any row, or every group below 1 (`0x5cd760`, `0x5ce3dc`).
fn set_collapsed(list: &mut SkillList, index: i32, collapse: bool) -> bool {
    let targets: Vec<u32> = match usize::try_from(index.wrapping_sub(1)) {
        Err(_) => list.groups.iter().map(|g| g.category_id).collect(),
        Ok(n) => match category_at(list, n) {
            Some(c) => vec![c],
            None => return false,
        },
    };
    for c in targets {
        if collapse {
            list.collapsed.insert(c);
        } else {
            list.collapsed.remove(&c);
        }
    }
    true
}

/// `GetSkillLineInfo`'s out-of-range answer, thirteen values: the reference's one unsigned bounds
/// test sends index 0, a negative or past-the-end index, no player and a missing DBC row here.
/// Slots 4 and 5 must be numbers: stock `SkillFrame.lua:194` adds them unguarded, and
/// `SkillFrame_OnLoad` calls `SetSelectedSkill(0)`.
fn out_of_range_tuple(_lua: &Lua) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_vec(vec![
        Value::Nil,        // 1  skillName
        Value::Nil,        // 2  header
        Value::Nil,        // 3  isExpanded
        Value::Integer(0), // 4  skillRank
        Value::Integer(0), // 5  numTempPoints
        Value::Integer(0), // 6  skillModifier
        Value::Integer(0), // 7  skillMaxRank
        Value::Nil,        // 8  isAbandonable
        Value::Nil,        // 9  stepCost
        Value::Nil,        // 10 rankCost
        Value::Integer(0), // 11 minLevel
        Value::Integer(0), // 12 skillCostType
        Value::Nil,        // 13 skillDescription
    ]))
}

/// `SetSelectedSkill(index)` (`0x5cd860`): the row's line id (`0x5cd8ca`), 0 past the list (`0x5cd8d6`).
fn set_selected(list: &mut SkillList, index: i32) {
    list.selected = usize::try_from(index)
        .ok()
        .and_then(|i| entry_at(list, i))
        .map(|e| e.skill_id);
}

/// `GetSelectedSkill()`: the selection's visible index, 0 when none or folded (`0x5cd360`).
fn selected_index(list: &SkillList) -> u32 {
    let Some(sid) = list.selected else {
        return 0;
    };
    rows(list)
        .iter()
        .take(num_rows(list))
        .position(|r| matches!(r, Row::Entry(ei) if list.state.entries[*ei].skill_id == sid))
        .map_or(0, |p| (p + 1) as u32)
}

/// `GetSkillLineInfo`'s `(skillRank, skillMaxRank)`: a nonzero rank folds the permanent bonus, floored at 0, the rank takes `rating_points`, a mono line clamps last (`0x5ce0c8`).
fn displayed_ranks(e: &SkillEntry, rating_points: i64) -> (i64, i64) {
    let fold = |v: u32| {
        if v > 0 {
            (i64::from(v) + i64::from(e.perm_bonus)).max(0)
        } else {
            0
        }
    };
    let (mut rank, mut max) = (fold(e.value) + rating_points, fold(e.max));
    if e.mono {
        max = 1;
        rank = rank.min(1);
    }
    (rank, max)
}

/// One row of `GetSkillLineInfo`, read off the list and the player at the call.
enum LineInfo {
    Header { name: String, expanded: bool },
    Entry { entry: SkillEntry, ups: u32 },
}

/// Register the skills-pane globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumSkillLines() → the visible row count (0 before the first rebuild).
    g.set(
        "GetNumSkillLines",
        lua.create_function(|lua, ()| Ok(read_list(lua, |list, _| num_rows(list)) as i64))?,
    )?;

    // GetSkillLineInfo(index) → 13 values on a skill row, 12 on a header; out of range, the
    // 13-value tuple, whose nil first slot drives stock `SkillFrame.lua:211`'s early return.
    g.set(
        "GetSkillLineInfo",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::number_arg(lua, index, "Usage: GetSkillLineInfo(index)")?;
            let Some(n) = usize::try_from(index).ok().and_then(|i| i.checked_sub(1)) else {
                return out_of_range_tuple(lua);
            };
            let info = read_list(lua, |list, live| match *rows(list).get(n)? {
                Row::Header(gi) => {
                    let grp = &list.groups[gi];
                    Some(LineInfo::Header {
                        name: grp.name.clone(),
                        expanded: !list.collapsed.contains(&grp.category_id),
                    })
                }
                // The slot is read off the player at the call (`0x5cdfe7`).
                Row::Entry(ei) => {
                    let entry = live_entry(&list.state.entries[ei], live).into_owned();
                    let ups = skill_ups(list, entry.skill_id);
                    Some(LineInfo::Entry { entry, ups })
                }
            });
            match info {
                None => out_of_range_tuple(lua),
                Some(LineInfo::Header { name, expanded }) => Ok(MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&name)?),
                    Value::Integer(1), // isHeader
                    binding_abi::flag(expanded),
                    Value::Integer(0), // skillRank
                    Value::Integer(0), // numTempPoints
                    Value::Integer(0), // skillModifier
                    Value::Integer(0), // skillMaxRank
                    Value::Nil,        // isAbandonable
                    Value::Nil,        // stepCost
                    Value::Nil,        // rankCost
                    Value::Integer(0), // minLevel
                    Value::Integer(0), // skillCostType, the last header return
                ])),
                Some(LineInfo::Entry { entry: e, ups }) => {
                    // The hand ratings are read off the player at the call (`0x5ce037`).
                    let rating_points = {
                        let model = lua
                            .app_data_ref::<crate::script::ModelCell>()
                            .expect("model app_data");
                        super::char_stats::player_combat_stats(&model)
                            .map_or(0, |s| s.skill_line_rating_points(e.skill_id))
                    };
                    let (rank, max) = displayed_ranks(&e, i64::from(rating_points));
                    let cost = |c: u32| match c {
                        0 => Value::Nil,
                        c => Value::Integer(i64::from(c)),
                    };
                    Ok(MultiValue::from_vec(vec![
                        Value::String(lua.create_string(&e.name)?),
                        Value::Nil, // isHeader
                        Value::Nil, // isExpanded
                        Value::Integer(rank),
                        Value::Integer(i64::from(ups)), // numTempPoints
                        Value::Integer(i64::from(e.temp_bonus)), // skillModifier: temp only
                        Value::Integer(max),
                        binding_abi::flag(e.abandonable), // isAbandonable
                        // stepCost, rankCost: nil for none, since Lua reads `0` as true.
                        cost(e.tier_cost),
                        cost(rank_cost(&e, rank + i64::from(ups) + 1)),
                        Value::Integer(i64::from(e.min_level)), // minLevel
                        // skillCostType: the cost index plus one.
                        Value::Integer(i64::from(e.cost_index) + 1),
                        Value::String(lua.create_string(&e.description)?), // skillDescription
                    ]))
                }
            }
        })?,
    )?;

    // Collapse/ExpandSkillHeader(index): the rebuild fires `SKILL_LINES_CHANGED` (`0x5cd701`).
    for (verb, usage, collapse) in [
        (
            "CollapseSkillHeader",
            "Usage: CollapseSkillHeader(index)",
            true,
        ),
        (
            "ExpandSkillHeader",
            "Usage: ExpandSkillHeader(index)",
            false,
        ),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, index: Value| {
                let index = binding_abi::number_arg(lua, index, usage)?;
                if write_list(lua, |list| set_collapsed(list, index, collapse)) {
                    super::tick::fire_event_into(lua, "SKILL_LINES_CHANGED", Vec::new());
                }
                Ok(())
            })?,
        )?;
    }

    // SetSelectedSkill(index) (`0x5cd860`) / GetSelectedSkill(): held by skill id;
    // `GetSelectedSkill` always answers one number, 0 included.
    g.set(
        "SetSelectedSkill",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::number_arg(lua, index, "Usage: SetSelectedSkill(index)")?;
            write_list(lua, |list| set_selected(list, index));
            Ok(())
        })?,
    )?;
    g.set(
        "GetSelectedSkill",
        lua.create_function(|lua, ()| {
            Ok(i64::from(read_list(lua, |list, _| selected_index(list))))
        })?,
    )?;

    // GetAdjustedSkillPoints() (`0x5ce540`): `PLAYER_CHARACTER_POINTS2` less the points
    // `AddSkillUp` has pending; 0 without a player.
    g.set(
        "GetAdjustedSkillPoints",
        lua.create_function(|lua, ()| {
            if !player_held(lua) {
                return Ok(0);
            }
            Ok(read_list(lua, |list, live| {
                i64::from(skill_points(list, live)) - i64::from(list.up_points)
            }))
        })?,
    )?;

    // AddSkillUp(index) (`0x5ce460`) and RemoveSkillUp(index) (`0x5ce4d0`): a change fires
    // `SKILL_LINES_CHANGED`.
    for (verb, usage, apply) in [
        (
            "AddSkillUp",
            "Usage: AddSkillUp(index)",
            add_skill_up as fn(&Lua, usize) -> bool,
        ),
        (
            "RemoveSkillUp",
            "Usage: RemoveSkillUp(index)",
            remove_skill_up,
        ),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, index: Value| {
                let index = binding_abi::number_arg(lua, index, usage)?;
                if usize::try_from(index).is_ok_and(|i| apply(lua, i)) {
                    super::tick::fire_event_into(lua, "SKILL_LINES_CHANGED", Vec::new());
                }
                Ok(())
            })?,
        )?;
    }

    // AcceptSkillUps() (`0x5ce5d0`): with a cost pending, one packet for the visible lines, sent in
    // the call (`0x5ce686`); every line's points clear, with no event.
    g.set(
        "AcceptSkillUps",
        lua.create_function(|lua, ()| {
            let lines = write_list(lua, |list| {
                if list.up_points == 0 {
                    return None;
                }
                let lines = rows(list)
                    .into_iter()
                    .filter_map(|row| match row {
                        Row::Entry(ei) => list.state.entries.get(ei),
                        Row::Header(_) => None,
                    })
                    .map(|e| (e.skill_id, skill_ups(list, e.skill_id)))
                    .filter(|&(_, ups)| ups != 0)
                    .collect();
                list.ups.clear();
                list.up_points = 0;
                Some(lines)
            });
            if let Some(lines) = lines {
                super::write_live(|l| l.send_skill_request(SkillRequest::BuyRanks(lines)));
            }
            Ok(())
        })?,
    )?;

    // BuySkillTier(index) (`0x5ce6f0`): the line's open tier off its step at the call (`0x5ce7ea`), when the points cover it (`0x5ce815`), sent in the call (`0x5ce849`).
    g.set(
        "BuySkillTier",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::number_arg(lua, index, "Usage: BuySkillTier(index)")?;
            let line = read_list(lua, |list, live| {
                let points = skill_points(list, live);
                usize::try_from(index)
                    .ok()
                    .and_then(|i| entry_at(list, i))
                    .map(|e| live_entry(e, live))
                    .filter(|e| e.tier_cost != 0 && e.tier_cost <= points)
                    .map(|e| e.skill_id)
            });
            if let Some(line) = line {
                super::write_live(|l| l.send_skill_request(SkillRequest::BuyTier(line)));
            }
            Ok(())
        })?,
    )?;

    // AbandonSkill(index) (`0x5ce2c0`): `CMSG_UNLEARN_SKILL` with the row's line, a header's 0 included, sent in the call (`0x5ce368`); nothing changes locally until the skill fields move.
    g.set(
        "AbandonSkill",
        lua.create_function(|lua, index: Value| {
            let index = binding_abi::number_arg(lua, index, "Usage: AbandonSkill(index)")?;
            let line = read_list(lua, |list, _| {
                let n = usize::try_from(index).ok()?.checked_sub(1)?;
                Some(match *rows(list).get(n)? {
                    Row::Entry(ei) => list.state.entries[ei].skill_id,
                    Row::Header(_) => 0,
                })
            });
            if let Some(id) = line {
                super::write_live(|l| l.send_unlearn_skill(id));
            }
            Ok(())
        })?,
    )?;

    // CancelSkillUps() (`0x5cd820`): zeroes every `numTempPoints` and fires, whatever was pending;
    // the stock Close button calls it (`SkillFrame.xml:352`).
    g.set(
        "CancelSkillUps",
        lua.create_function(|lua, ()| {
            write_list(lua, |list| {
                list.ups.clear();
                list.up_points = 0;
            });
            super::tick::fire_event_into(lua, "SKILL_LINES_CHANGED", Vec::new());
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod skill_up_tests {
    use super::*;
    use crate::script::{UiScript, UnitGuids};

    fn priced(points: u32, tier_cost: u32) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&UnitGuids {
            player: 1,
            ..Default::default()
        });
        s.set_skills(SkillsState {
            entries: vec![SkillEntry {
                skill_id: 40,
                name: "Herbs".into(),
                value: 1,
                max: 3,
                mono: false,
                temp_bonus: 0,
                perm_bonus: 0,
                min_level: 0,
                cost_index: 0,
                category_id: 1,
                category_name: "Trade".into(),
                category_order: 1,
                description: String::new(),
                abandonable: false,
                rank_costs: vec![0, 2, 5].into(),
                tier_cost,
            }],
            points,
        });
        s
    }

    /// `0x5cdbf0`: a point costs the next rank's price, within the points and under the max; the
    /// line is row 2, under its header.
    #[test]
    fn a_skill_up_spends_what_the_next_rank_costs() {
        let s = priced(7, 0);
        s.run("AddSkillUp(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetAdjustedSkillPoints()").unwrap(), 5);
        s.run("AddSkillUp(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetAdjustedSkillPoints()").unwrap(), 0);
        s.run("AddSkillUp(2)").unwrap();
        let temp: i64 = s.eval("return select(4, GetSkillLineInfo(2))").unwrap();
        assert_eq!(temp, 1, "the rank is the first return past the header pair");
        s.run("RemoveSkillUp(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetAdjustedSkillPoints()").unwrap(), 5);
        assert!(s.run("AddSkillUp('x')").is_err());
    }

    /// `0x5ce5d0`: the pending points go out once and clear, with nothing pending nothing goes.
    #[test]
    fn accept_sends_the_pending_points_once() {
        let mut s = priced(7, 0);
        let mut v = crate::script::live::TestView::default();
        s.with_live(&mut v, |s| {
            s.run("AddSkillUp(2) AcceptSkillUps()").unwrap();
            // Sent inside the call (`0x5ce686`).
            assert_eq!(
                crate::script::live::test_view(|v| v.skill_sends.clone()),
                vec![SkillRequest::BuyRanks(vec![(40, 1)])]
            );
            s.run("AcceptSkillUps()").unwrap();
        });
        assert_eq!(v.skill_sends, vec![SkillRequest::BuyRanks(vec![(40, 1)])]);
    }

    /// `0x5ce6f0`: a tier needs a cost the points cover, and goes out in the call (`0x5ce849`).
    #[test]
    fn a_tier_needs_the_points() {
        let mut s = priced(4, 5);
        let mut v = crate::script::live::TestView::default();
        s.with_live(&mut v, |s| s.run("BuySkillTier(2)").unwrap());
        assert!(v.skill_sends.is_empty());
        let mut s = priced(5, 5);
        s.with_live(&mut v, |s| s.run("BuySkillTier(2)").unwrap());
        assert_eq!(v.skill_sends, vec![SkillRequest::BuyTier(40)]);
    }

    /// `0x5cdfe7`: the ranks, bonuses, step and points are read off the player at the call, not the rebuilt rows.
    #[test]
    fn the_line_info_reads_the_slot_at_the_call() {
        let mut s = priced(7, 0);
        let mut v = crate::script::live::TestView {
            skills: Some(std::collections::HashMap::from([(
                40,
                SkillLive {
                    value: 2,
                    max: 3,
                    temp_bonus: 4,
                    perm_bonus: 0,
                    abandonable: true,
                    tier_cost: 9,
                },
            )])),
            ..Default::default()
        };
        let (rank, modifier, max, abandon, step) = s.with_live(&mut v, |s| {
            s.eval::<(i64, i64, i64, Option<i64>, Option<i64>)>(
                "local _,_,_,r,_,m,x,a,st = GetSkillLineInfo(2) return r, m, x, a, st",
            )
            .unwrap()
        });
        assert_eq!(
            (rank, modifier, max, abandon, step),
            (2, 4, 3, Some(1), Some(9))
        );
        // A line the player no longer holds reads slot -1 (`0x6dc222`) until the next rebuild drops it.
        v.skills = Some(Default::default());
        v.skill_slot_minus_one = SkillLive {
            value: 1234,
            max: 3,
            ..Default::default()
        };
        let (rank, max) = s.with_live(&mut v, |s| {
            s.eval::<(i64, i64)>("local _,_,_,r,_,_,x = GetSkillLineInfo(2) return r, x")
                .unwrap()
        });
        assert_eq!((rank, max), (1234, 3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn entry(
        skill_id: u32,
        name: &str,
        value: u32,
        max: u32,
        temp_bonus: i32,
        category_id: u32,
        category_name: &str,
        category_order: u32,
    ) -> SkillEntry {
        SkillEntry {
            skill_id,
            name: name.into(),
            value,
            max,
            temp_bonus,
            perm_bonus: 0,
            min_level: 0,
            cost_index: 0,
            category_id,
            category_name: category_name.into(),
            category_order,
            description: format!("About {name}."),
            // Fixture rule: Professions (id 2) is abandonable, as the 0x20 flag splits it.
            abandonable: category_id == 2,
            rank_costs: Default::default(),
            tier_cost: 0,
            // Fixture rule: Class Skills (id 3) is single-rank, as the 0x400 flag splits it.
            mono: category_id == 3,
        }
    }

    /// Weapon Skills (order 1) and Professions (order 2), pushed unordered: Swords before Defense.
    fn state() -> SkillsState {
        SkillsState {
            points: 0,
            entries: vec![
                entry(43, "Swords", 200, 300, 0, 1, "Weapon Skills", 1),
                entry(95, "Defense", 180, 300, 5, 1, "Weapon Skills", 1),
                entry(129, "First Aid", 57, 75, 3, 2, "Professions", 2),
                entry(356, "Fishing", 1, 300, 0, 2, "Professions", 2),
            ],
        }
    }

    /// Read `(name, type)` at a visible index, `type` = "header"/"entry".
    fn row_kind(s: &mut UiScript, i: i64) -> (String, String) {
        s.eval::<(String, String)>(&format!(
            "local n,h = GetSkillLineInfo({i}) local t = h and 'header' or 'entry' return n,t"
        ))
        .unwrap()
    }

    #[test]
    fn grouped_visible_rows_interleave_headers_ordered_by_category() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 0);

        s.set_skills(state());
        // 2 headers + 4 entries, category_order ascending, name ascending within.
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);
        assert_eq!(
            row_kind(&mut s, 1),
            ("Weapon Skills".into(), "header".into())
        );
        assert_eq!(row_kind(&mut s, 2), ("Defense".into(), "entry".into()));
        assert_eq!(row_kind(&mut s, 3), ("Swords".into(), "entry".into()));
        assert_eq!(row_kind(&mut s, 4), ("Professions".into(), "header".into()));
        assert_eq!(row_kind(&mut s, 5), ("First Aid".into(), "entry".into()));
        assert_eq!(row_kind(&mut s, 6), ("Fishing".into(), "entry".into()));

        // Every group starts expanded.
        let (_, h1, e1) = s
            .eval::<(String, i64, Option<i64>)>("local n,h,e = GetSkillLineInfo(1) return n,h,e")
            .unwrap();
        assert_eq!((h1, e1), (1, Some(1)));
    }

    #[test]
    fn abandon_skill_queues_the_entrys_skill_id_and_mutates_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());
        // The 8th return, 1/nil: only Professions rows are abandonable (fixture rule).
        let ab = |s: &mut UiScript, i: i64| {
            s.eval::<Option<i64>>(&format!(
                "local _,_,_,_,_,_,_,ab = GetSkillLineInfo({i}) return ab"
            ))
            .unwrap()
        };
        assert_eq!(ab(&mut s, 2), None, "Defense is not abandonable");
        assert_eq!(ab(&mut s, 5), Some(1), "First Aid is abandonable");
        assert_eq!(ab(&mut s, 1), None, "a header never is");

        // Sent by skill id in the call (`0x5ce368`); a header's row sends its line 0 (`0x5ce356`),
        // an out-of-range index nothing (`0x5ce31d`); the list stays.
        let mut v = crate::script::live::TestView::default();
        s.with_live(&mut v, |s| {
            s.run("AbandonSkill(5)").unwrap();
            assert_eq!(
                crate::script::live::test_view(|v| v.unlearn_sends.clone()),
                vec![129],
                "the send leaves inside the call"
            );
            s.run("AbandonSkill(1)").unwrap();
            s.run("AbandonSkill(99)").unwrap();
        });
        assert_eq!(v.unlearn_sends, vec![129, 0]);
        assert_eq!(
            s.eval::<i64>("return GetNumSkillLines()").unwrap(),
            6,
            "no local removal — the visible tree is unchanged"
        );
    }

    #[test]
    fn collapse_hides_a_groups_entries_and_remaps_indices() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());

        // Fold Weapon Skills (row 1): its two entries go, the header stays with isExpanded nil.
        s.run("CollapseSkillHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 4);
        let (name, _, expanded) = s
            .eval::<(String, i64, Option<i64>)>("local n,h,e = GetSkillLineInfo(1) return n,h,e")
            .unwrap();
        assert_eq!((name.as_str(), expanded), ("Weapon Skills", None));
        assert_eq!(
            row_kind(&mut s, 2),
            ("Professions".into(), "header".into()),
            "Weapon Skills' entries are folded; Professions is now row 2"
        );

        // Expand it back.
        s.run("ExpandSkillHeader(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);

        // Collapse-all (id 0), then expand-all (id 0).
        s.run("CollapseSkillHeader(0)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 2);
        s.run("ExpandSkillHeader(0)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);
    }

    /// A fold fires the repaint, any row folds its group, folded rows stay addressable.
    #[test]
    fn a_fold_fires_the_repaint_and_any_row_names_its_group() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());
        s.run(
            "SKILL_FIRES = 0 \
             local f = CreateFrame('Frame') f:RegisterEvent('SKILL_LINES_CHANGED') \
             f:SetScript('OnEvent', function() SKILL_FIRES = SKILL_FIRES + 1 end)",
        )
        .unwrap();
        let fires = |s: &mut UiScript| s.eval::<i64>("return SKILL_FIRES").unwrap();

        // Swords (row 3), an entry, folds Weapon Skills; its lines move past the visible four.
        s.run("CollapseSkillHeader(3)").unwrap();
        assert_eq!(fires(&mut s), 1, "the rebuild fires once");
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 4);
        assert_eq!(row_kind(&mut s, 2), ("Professions".into(), "header".into()));
        assert_eq!(row_kind(&mut s, 5), ("Defense".into(), "entry".into()));
        assert_eq!(row_kind(&mut s, 6), ("Swords".into(), "entry".into()));

        // A folded line's index unfolds its group too.
        s.run("ExpandSkillHeader(6)").unwrap();
        assert_eq!(fires(&mut s), 2);
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);

        // Past the list nothing is rebuilt; below 1 is every group, rebuilt either way.
        s.run("CollapseSkillHeader(7)").unwrap();
        assert_eq!(fires(&mut s), 2, "no rebuild past the end");
        s.run("CollapseSkillHeader(-3)").unwrap();
        assert_eq!(fires(&mut s), 3);
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 2);
        s.run("ExpandSkillHeader(0)").unwrap();
        assert_eq!(fires(&mut s), 4);

        // Not a number: the binding's usage error.
        let err = s.run("CollapseSkillHeader(nil)").unwrap_err().to_string();
        assert!(err.contains("Usage: CollapseSkillHeader(index)"), "{err}");
        let err = s.run("ExpandSkillHeader('x')").unwrap_err().to_string();
        assert!(err.contains("Usage: ExpandSkillHeader(index)"), "{err}");
    }

    /// A selection inside a folded group reads 0 until the group unfolds.
    #[test]
    fn a_folded_selection_reads_zero() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());
        s.run("SetSelectedSkill(5)").unwrap(); // First Aid
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 5);
        s.run("CollapseSkillHeader(4)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);
        s.run("ExpandSkillHeader(4)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 5);
    }

    #[test]
    fn a_repush_keeps_the_selection_and_re_expands_every_group() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());

        // Selecting a header clears the selection.
        s.run("SetSelectedSkill(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);

        // Select Swords (row 3), fold Professions (row 4).
        s.run("SetSelectedSkill(3)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 3);
        s.run("CollapseSkillHeader(4)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 4);

        // A re-push unfolds every group and keeps Swords selected at row 3.
        let mut ticked = state();
        ticked.entries[0].value = 201; // Swords
        s.set_skills(ticked);
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 6);
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 3);
        let (name, rank) = s
            .eval::<(String, i64)>("local n,_,_,r = GetSkillLineInfo(3) return n,r")
            .unwrap();
        assert_eq!((name.as_str(), rank), ("Swords", 201));

        // A rebuild that drops the selected line reads 0 (`0x5cd391`) but keeps its id
        // (`0xc22af4` is written only by `SetSelectedSkill`), so the line's return is selected again.
        let mut without_swords = state();
        without_swords.entries.remove(0);
        s.set_skills(without_swords);
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);
        s.set_skills(state());
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 3);
    }

    #[test]
    fn header_and_entry_tuple_shapes() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(state());

        // Header row 1: twelve values, no description.
        let (name, is_header, is_expanded, rank, temp, modifier, max) = s
            .eval::<(String, i64, Option<i64>, i64, i64, i64, i64)>(
                "local n,h,e,r,t,m,mx = GetSkillLineInfo(1) return n,h,e,r,t,m,mx",
            )
            .unwrap();
        assert_eq!(
            (
                name.as_str(),
                is_header,
                is_expanded,
                rank,
                temp,
                modifier,
                max
            ),
            ("Weapon Skills", 1, Some(1), 0, 0, 0, 0)
        );
        let (abandon_nil, step_nil, rank_cost_nil, min_level, cost_type) = s
            .eval::<(bool, bool, bool, i64, i64)>(
                "local _,_,_,_,_,_,_,a,st,rc,ml,ct = GetSkillLineInfo(1) \
                 return a==nil, st==nil, rc==nil, ml, ct",
            )
            .unwrap();
        let count = s.arity("GetSkillLineInfo(1)").unwrap();
        assert!(abandon_nil);
        assert!(
            step_nil && rank_cost_nil,
            "stepCost/rankCost are nil, not 0"
        );
        assert_eq!((min_level, cost_type), (0, 0));
        assert_eq!(count, 12, "a header row returns 12 values, not 13");

        // Entry row 2 (Defense 180/300, temp +5): thirteen values.
        let (name, is_header, is_expanded, rank, temp, modifier, max) = s
            .eval::<(String, Option<i64>, Option<i64>, i64, i64, i64, i64)>(
                "local n,h,e,r,t,m,mx = GetSkillLineInfo(2) return n,h,e,r,t,m,mx",
            )
            .unwrap();
        assert_eq!(
            (
                name.as_str(),
                is_header,
                is_expanded,
                rank,
                temp,
                modifier,
                max
            ),
            ("Defense", None, None, 180, 0, 5, 300)
        );
        let count = s.arity("GetSkillLineInfo(2)").unwrap();
        let desc = s
            .eval::<String>("local _,_,_,_,_,_,_,_,_,_,_,_,d = GetSkillLineInfo(2) return d")
            .unwrap();
        assert_eq!((count, desc.as_str()), (13, "About Defense."));
        // The cost type is the cost index plus one, so index 0 reads 1.
        let (min_level, cost_type) = s
            .eval::<(i64, i64)>(
                "local _,_,_,_,_,_,_,_,_,_,ml,ct = GetSkillLineInfo(2) return ml,ct",
            )
            .unwrap();
        assert_eq!((min_level, cost_type), (0, 1));
    }

    #[test]
    fn the_permanent_bonus_folds_into_the_numbers_and_the_temporary_one_does_not() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.entries[1].perm_bonus = 10; // Defense 180/300, temp +5
        st.entries
            .push(entry(182, "Herbalism", 0, 0, 0, 2, "Professions", 2));
        st.entries.last_mut().unwrap().perm_bonus = 10;
        s.set_skills(st);

        // Defense (row 2): 190/310, the modifier still the temp +5 alone.
        let (rank, modifier, max) = s
            .eval::<(i64, i64, i64)>("local _,_,_,r,_,m,mx = GetSkillLineInfo(2) return r,m,mx")
            .unwrap();
        assert_eq!((rank, modifier, max), (190, 5, 310));

        // Herbalism 0/0 (row 7, untrained so last): the bonus is not added to a zero.
        let (name, rank, max) = s
            .eval::<(String, i64, i64)>("local n,_,_,r,_,_,mx = GetSkillLineInfo(7) return n,r,mx")
            .unwrap();
        assert_eq!((name.as_str(), rank, max), ("Herbalism", 0, 0));
    }

    #[test]
    fn untrained_lines_sort_under_the_trained_ones_of_their_category() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        // "Alchemy" would sort first in Professions by name; at rank 0 it goes last.
        st.entries
            .push(entry(171, "Alchemy", 0, 300, 0, 2, "Professions", 2));
        s.set_skills(st);
        let names: Vec<String> = (5..=7)
            .map(|i| {
                s.eval::<String>(&format!("return (GetSkillLineInfo({i}))"))
                    .unwrap()
            })
            .collect();
        assert_eq!(names, ["First Aid", "Fishing", "Alchemy"]);
    }

    #[test]
    fn a_mono_line_reports_max_rank_one_whatever_the_server_said() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        // Beast Mastery: category 3, so mono by the fixture rule; 300/300 from the server.
        st.entries.push(entry(
            50,
            "Beast Mastery",
            300,
            300,
            0,
            3,
            "Class Skills",
            0,
        ));
        s.set_skills(st);

        // Rows 1 and 2: the Class Skills header and its entry (category_order 0 sorts first).
        let (name, rank, modifier, max) = s
            .eval::<(String, i64, i64, i64)>(
                "local n,_,_,r,_,m,mx = GetSkillLineInfo(2) return n,r,m,mx",
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), rank, modifier, max),
            ("Beast Mastery", 1, 0, 1),
            "the descriptor's 300/300 reads 1/1 — max overridden, rank clamped under it"
        );
        // Defense, not mono, still reports its real 300.
        assert_eq!(
            s.eval::<i64>("local _,_,_,_,_,_,mx = GetSkillLineInfo(4) return mx")
                .unwrap(),
            300
        );
    }

    /// The count is the assertion: `GetSkillLineInfo(0) == nil` holds for a single nil too, since
    /// Lua compares only the first value.
    #[test]
    fn out_of_range_answers_the_thirteen_value_tuple() {
        let s = UiScript::new().unwrap();
        for idx in ["0", "1", "99"] {
            assert_eq!(
                s.arity(&format!("GetSkillLineInfo({idx})")).unwrap(),
                13,
                "GetSkillLineInfo({idx}) must answer 13 values"
            );
            assert_eq!(
                s.eval::<i64>(&format!(
                    "local _,_,_,r,t = GetSkillLineInfo({idx}) return r + t"
                ))
                .unwrap(),
                0,
                "GetSkillLineInfo({idx}) slots 4+5 must be NUMBERS — the reference adds them unguarded"
            );
            assert!(
                s.eval::<bool>(&format!("return (GetSkillLineInfo({idx})) == nil"))
                    .unwrap(),
                "slot 1 must stay nil — it drives the stock file's own `if (not skillName)` early-out"
            );
        }
    }

    /// The player's hands hold Swords, Daggers and Bows, on these rating percentages.
    fn rated_hands() -> crate::script::UnitCombatStats {
        let mut s = crate::script::UnitCombatStats {
            hand_skill_lines: [43, 173, 45],
            ..Default::default()
        };
        s.combat_rating_bonus[0] = 2.5; // 2 points
        s.combat_rating_bonus[20] = 4.0; // 4 points
        s.combat_rating_bonus[21] = 6.0; // 6 points
        s.combat_rating_bonus[22] = 8.0; // 8 points
        s
    }

    /// `(name, skillRank, skillMaxRank)` of every line.
    fn ranks(s: &UiScript) -> Vec<(String, i64, i64)> {
        let n = s.eval::<i64>("return GetNumSkillLines()").unwrap();
        (1..=n)
            .filter_map(|i| {
                let (name, header, rank, max) = s
                    .eval::<(String, Option<i64>, i64, i64)>(&format!(
                        "local n, h, _, r, _, _, m = GetSkillLineInfo({i}) return n, h, r, m"
                    ))
                    .unwrap();
                header.is_none().then_some((name, rank, max))
            })
            .collect()
    }

    /// Each hand's line takes weapon skill plus that hand's rating; every other line stays bare.
    #[test]
    fn a_line_in_hand_takes_the_weapon_and_the_hand_rating() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(SkillsState {
            points: 0,
            entries: vec![
                entry(43, "Swords", 200, 300, 0, 1, "Weapon Skills", 1),
                entry(173, "Daggers", 200, 300, 0, 1, "Weapon Skills", 1),
                entry(45, "Bows", 200, 300, 0, 1, "Weapon Skills", 1),
                entry(44, "Axes", 200, 300, 0, 1, "Weapon Skills", 1),
            ],
        });
        s.set_player_combat_stats(Some(rated_hands()));
        assert_eq!(
            ranks(&s),
            vec![
                ("Axes".to_string(), 200, 300),
                ("Bows".to_string(), 200 + 2 + 8, 300),
                ("Daggers".to_string(), 200 + 2 + 6, 300),
                ("Swords".to_string(), 200 + 2 + 4, 300),
            ]
        );
    }

    /// One line in two hands takes the first hand's rating.
    #[test]
    fn a_line_in_two_hands_takes_the_first_hands_rating() {
        let mut s = UiScript::new().unwrap();
        s.set_skills(SkillsState {
            points: 0,
            entries: vec![entry(43, "Swords", 200, 300, 0, 1, "Weapon Skills", 1)],
        });
        s.set_player_combat_stats(Some(crate::script::UnitCombatStats {
            hand_skill_lines: [43, 43, 45],
            ..rated_hands()
        }));
        assert_eq!(ranks(&s), vec![("Swords".to_string(), 206, 300)]);
        s.set_player_combat_stats(Some(crate::script::UnitCombatStats {
            hand_skill_lines: [0, 43, 43],
            ..rated_hands()
        }));
        assert_eq!(ranks(&s), vec![("Swords".to_string(), 208, 300)]);
    }

    /// The single-rank clamp runs after the rating, and the next rank's price reads the rated rank.
    #[test]
    fn the_mono_clamp_and_the_rank_cost_read_the_rated_rank() {
        let mut s = UiScript::new().unwrap();
        let mut priced = entry(173, "Daggers", 1, 300, 0, 1, "Weapon Skills", 2);
        priced.rank_costs = (1..=12).map(|i| i * 10).collect();
        s.set_skills(SkillsState {
            points: 0,
            entries: vec![entry(43, "Swords", 1, 300, 0, 3, "Class Skills", 1), priced],
        });
        s.set_player_combat_stats(Some(rated_hands()));
        assert_eq!(
            ranks(&s),
            vec![
                ("Swords".to_string(), 1, 1),
                ("Daggers".to_string(), 9, 300)
            ]
        );
        assert_eq!(
            s.eval::<i64>("return (select(10, GetSkillLineInfo(4)))")
                .unwrap(),
            100,
            "the tenth rank's price, the rated 9 plus one"
        );
    }

    #[test]
    fn no_push_reports_zero_rows() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 0);
        // The empty pane answers the out-of-range tuple; this pins slot 1.
        assert!(s
            .eval::<bool>("return (GetSkillLineInfo(1)) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetSelectedSkill()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetAdjustedSkillPoints()").unwrap(), 0);
        // Collapse, expand and select on an empty pane do nothing.
        s.run("CollapseSkillHeader(0) ExpandSkillHeader(1) SetSelectedSkill(1)")
            .unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSkillLines()").unwrap(), 0);
    }
}
