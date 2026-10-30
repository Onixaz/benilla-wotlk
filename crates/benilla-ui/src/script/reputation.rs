//! The reputation pane's fifteen globals, a port of 12340's reputation info
//! (`0x5cfe40`..`0x5d2e30`): the app pushes every faction with a reputation slot, and the engine
//! keeps the client's own faction list and header tree over it. Indexes are 1-based into that list,
//! whose shown rows come first; every faction lookup goes through one helper (`0x5d0da0`), which
//! answers thirteen values on every path.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// The synthetic header for factions the player moved to the inactive bucket.
const INACTIVE: i64 = -1;
/// The synthetic header for factions with no parent, "Other".
const OTHER: i64 = 0;
/// A guard on the parent walks; 3.3.5a's tree is two headers deep.
const MAX_DEPTH: usize = 16;

/// One reputation faction, resolved app-side from `Faction.dbc` and the player's wire slot.
#[derive(Clone, Debug, PartialEq)]
pub struct FactionEntry {
    /// `Faction.dbc` id: the row's identity, and what a child's [`Self::parent_id`] names.
    pub faction_id: u32,
    /// `reputationIndex`: the slot every send addresses.
    pub rep_list_id: u32,
    /// `Faction.dbc`'s `ParentFactionID`, 0 for none: the header this faction files under.
    pub parent_id: u32,
    /// The localized name: the row's label, a header's included.
    pub name: String,
    /// The localized description, for the detail popup; often empty.
    pub description: String,
    /// The DBC race/class base plus the wire standing (`0x5d05b0`): `barValue`, absolute.
    pub standing: i32,
    /// The rank, 1..=8 (`FACTION_STANDING_LABEL<n>`, `0x5d0600` plus one).
    pub standing_id: u8,
    /// The rank's absolute bounds (`0xa2d2fc`), `barMin`/`barMax`.
    pub bar_min: i32,
    pub bar_max: i32,
    /// One of the four race/class slots fits the player: the login build lists only these
    /// (`0x5d2bc0`).
    pub fits_player: bool,
    /// Flag `0x01`: the player has met the faction, which puts it in the list.
    pub visible: bool,
    /// Flag `0x08`: the list adds this faction as a header row (`0x5d08d0`).
    pub is_header: bool,
    /// Flag `0x02`; the toggle writes it here, unacked.
    pub at_war: bool,
    /// Flag `0x10`: war can never be declared.
    pub peace_forced: bool,
    /// Flag `0x20`; it files the row under "Inactive".
    pub inactive: bool,
    /// The faction's has-reputation test (`0x5d0ce0`): a header that is also a reputation bar.
    pub has_rep: bool,
}

/// A flat, unordered push of every faction with a reputation slot; the engine builds the list.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ReputationState {
    pub entries: Vec<FactionEntry>,
    /// `PLAYER_FIELD_WATCHED_FACTION_INDEX` as a slot, `None` for none: a server field with no
    /// client mirror, never written here.
    pub watched: Option<u32>,
}

/// One outbound reputation verb the pane queued, drained by the app into its `WorldWriter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReputationSend {
    /// `CMSG_SET_FACTION_ATWAR`, carrying the asked state.
    AtWar { rep_list_id: u32, at_war: bool },
    /// `CMSG_SET_FACTION_INACTIVE`.
    Inactive { rep_list_id: u32, inactive: bool },
    /// `CMSG_SET_WATCHED_FACTION`; `None` is the wire's `-1`, not slot 0.
    Watch(Option<u32>),
}

/// One faction list row (`0xc23480`): a faction, or a header, whose `parent` is itself.
#[derive(Clone, Copy, Debug)]
struct Row {
    rep_list_id: i64,
    faction_id: i64,
    is_header: bool,
    parent: i64,
    /// Its slot in [`FactionTree::headers`], set by [`sort`].
    header: usize,
}

/// One header (`0xc23370`): a `Faction.dbc` id, or [`OTHER`] or [`INACTIVE`].
#[derive(Clone, Copy, Debug)]
struct Header {
    faction_id: i64,
    parent: i64,
    /// Rows filed under it, its sub-headers' included (`0x5d1880`).
    count: u32,
    /// Folded with an ancestor (`[+0xc]`), apart from its own fold.
    nested_collapsed: bool,
}

/// The client's faction list and header tree; `pub(crate)` only because [`Model`] stores it.
#[derive(Clone, Debug, Default)]
pub(crate) struct FactionTree {
    built: bool,
    rows: Vec<Row>,
    headers: Vec<Header>,
    /// Headers folded by their own button: the clear bits of the mask at `0xad0ae4`.
    collapsed: HashSet<i64>,
    /// `GetNumFactions` (`0xc23478`): the leading rows of [`Self::rows`] on screen.
    shown: usize,
    /// The selection as a faction id, 0 for none (`0xc2347c`).
    selected: i64,
    /// Faction id to its position in [`ReputationState::entries`]: the `Faction.dbc` lookup.
    index: HashMap<i64, usize>,
}

impl FactionTree {
    fn record<'a>(&self, state: &'a ReputationState, id: i64) -> Option<&'a FactionEntry> {
        state.entries.get(*self.index.get(&id)?)
    }

    fn header_slot(&self, id: i64) -> Option<usize> {
        self.headers.iter().position(|h| h.faction_id == id)
    }

    /// The faction of the row a 1-based index names, 0 past the list: the reference reads
    /// `index - 1 <= count`, one slot further, which holds no row here.
    fn faction_at(&self, index: usize) -> i64 {
        index
            .checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .map_or(0, |r| r.faction_id)
    }

    fn shown_rows(&self) -> &[Row] {
        &self.rows[..self.shown.min(self.rows.len())]
    }
}

impl super::UiScript {
    /// Push the reputation snapshot. The first push into a VM builds the list as the login does
    /// (`0x5d2bc0`); later ones add the factions that turned visible, as the standing and visible
    /// handlers do (`0x5d20a0`, `0x5d2050`), and re-sort, so a fold survives a standing tick.
    /// The selection is a faction id and outlives both.
    pub fn set_reputation(&mut self, state: ReputationState) {
        let mut model = self.model_mut();
        let m: &mut Model = &mut model;
        let was_visible: HashSet<u32> = m
            .reputation
            .entries
            .iter()
            .filter(|e| e.visible)
            .map(|e| e.rep_list_id)
            .collect();
        m.reputation = state;
        let (tree, state) = (&mut m.reputation_tree, &m.reputation);
        tree.index = state
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i64::from(e.faction_id), i))
            .collect();
        // A list with no faction yet is rebuilt: the login packet may land after an empty push.
        if !tree.built || tree.rows.iter().all(|r| r.faction_id == INACTIVE) {
            enter_world(tree, state);
            return;
        }
        let mut fresh: Vec<&FactionEntry> = state
            .entries
            .iter()
            .filter(|e| e.visible && !was_visible.contains(&e.rep_list_id))
            .collect();
        fresh.sort_by_key(|e| std::cmp::Reverse(e.faction_id));
        for e in fresh {
            add_visible(tree, state, e.rep_list_id);
        }
        sort(tree, state);
    }

    /// Drain the reputation verbs the pane queued since the last call.
    pub fn take_reputation_sends(&mut self) -> Vec<ReputationSend> {
        drain_field!(self, reputation_sends)
    }
}

/// The login build (`0x5d2bc0`): every visible faction whose slot fits, walked from the table's
/// end, then the folded "Inactive" header.
fn enter_world(tree: &mut FactionTree, state: &ReputationState) {
    tree.rows.clear();
    tree.headers.clear();
    tree.collapsed.clear();
    let mut met: Vec<&FactionEntry> = state
        .entries
        .iter()
        .filter(|e| e.fits_player && e.visible)
        .collect();
    met.sort_by_key(|e| std::cmp::Reverse(e.faction_id));
    for e in met {
        add_visible(tree, state, e.rep_list_id);
    }
    add_header(tree, INACTIVE, 0);
    tree.rows.push(Row {
        rep_list_id: -1,
        faction_id: INACTIVE,
        is_header: true,
        parent: INACTIVE,
        header: 0,
    });
    collapse(tree, state, tree.rows.len() - 1, true);
    sort(tree, state);
    tree.built = true;
}

/// The visible-faction add (`0x5d08d0`): one row per slot, a header by its wire flag, then the rows
/// of every ancestor header, "Other" for a parentless one.
fn add_visible(tree: &mut FactionTree, state: &ReputationState, rep_list_id: u32) {
    let rep = i64::from(rep_list_id);
    if tree.rows.iter().any(|r| r.rep_list_id == rep) {
        return;
    }
    let entry = state.entries.iter().find(|e| e.rep_list_id == rep_list_id);
    let faction_id = entry.map_or(0, |e| i64::from(e.faction_id));
    let is_header = entry.is_some_and(|e| e.is_header);
    let parent = entry.map_or(0, |e| i64::from(e.parent_id));
    tree.rows.push(Row {
        rep_list_id: rep,
        faction_id,
        is_header,
        parent: if is_header { faction_id } else { parent },
        header: 0,
    });
    if is_header {
        add_header(tree, faction_id, parent);
    }
    add_faction_header(tree, state, parent, 0);
}

/// The faction-header add (`0x5d01f0`): the ancestors first, then a header row for `id` unless one
/// exists.
fn add_faction_header(tree: &mut FactionTree, state: &ReputationState, id: i64, depth: usize) {
    let record = tree.record(state, id);
    let parent = record.map_or(0, |e| i64::from(e.parent_id));
    let rep = record.map_or(-1, |e| i64::from(e.rep_list_id));
    if parent != 0 && depth < MAX_DEPTH {
        add_faction_header(tree, state, parent, depth + 1);
    }
    if tree.rows.iter().any(|r| r.is_header && r.faction_id == id) {
        return;
    }
    tree.rows.push(Row {
        rep_list_id: rep,
        faction_id: id,
        is_header: true,
        parent: id,
        header: 0,
    });
    add_header(tree, id, parent);
}

/// The header add (`0x5cfe70`): every call unfolds every header.
fn add_header(tree: &mut FactionTree, id: i64, parent: i64) {
    if tree.header_slot(id).is_none() {
        tree.headers.push(Header {
            faction_id: id,
            parent,
            count: 0,
            nested_collapsed: false,
        });
    }
    tree.collapsed.clear();
}

/// The header a row files under: its parent, or "Inactive" for an inactive faction.
fn bucket(tree: &FactionTree, state: &ReputationState, row: &Row) -> i64 {
    if tree
        .record(state, row.faction_id)
        .is_some_and(|e| e.inactive)
    {
        INACTIVE
    } else {
        row.parent
    }
}

fn has_rep(tree: &FactionTree, state: &ReputationState, id: i64) -> bool {
    tree.record(state, id).is_some_and(|e| e.has_rep)
}

/// The sort's hidden test (`0x5d16b0`): an empty header, unless it is a reputation bar itself.
fn header_hidden(tree: &FactionTree, state: &ReputationState, h: &Header) -> bool {
    h.count == 0 && !has_rep(tree, state, h.faction_id)
}

/// Case-insensitive name order, raw bytes breaking a tie: the client's case-folding UTF-8 sort.
fn collate(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

/// The header comparator (`0x5d1710`) on two headers of one level: a real faction by name, before
/// the synthetic ones, "Other" before "Inactive".
fn header_order(tree: &FactionTree, state: &ReputationState, a: i64, b: i64) -> Ordering {
    match (tree.record(state, a), tree.record(state, b)) {
        (Some(x), Some(y)) => collate(&x.name, &y.name).then(a.cmp(&b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => b.cmp(&a),
    }
}

/// A header's ancestry, root first: sorted as paths, a parent leads its sub-headers and the roots
/// order the rest, as the reference's comparator does.
fn header_path(tree: &FactionTree, h: &Header) -> Vec<i64> {
    let mut path = vec![h.faction_id];
    let mut parent = h.parent;
    while parent != 0 && path.len() < MAX_DEPTH {
        let Some(slot) = tree.header_slot(parent) else {
            break;
        };
        path.push(parent);
        parent = tree.headers[slot].parent;
    }
    path.reverse();
    path
}

/// The faction sort (`0x5d1880`): count each header's rows, order the headers, count the shown rows
/// and order the list, shown rows first.
fn sort(tree: &mut FactionTree, state: &ReputationState) {
    for h in 0..tree.headers.len() {
        let id = tree.headers[h].faction_id;
        let count = tree
            .rows
            .iter()
            .filter(|r| !r.is_header && bucket(tree, state, r) == id)
            .count();
        tree.headers[h].count = u32::try_from(count).unwrap_or(u32::MAX);
    }
    // A sub-header rolls into its parent, plus one if it is a bar itself; the pass starts at 1.
    for i in 0..tree.headers.len() {
        for j in 1..tree.headers.len() {
            let child = tree.headers[j];
            if i == j || child.parent == 0 || tree.headers[i].faction_id != child.parent {
                continue;
            }
            tree.headers[i].count += child.count;
            if has_rep(tree, state, child.faction_id) && child.parent != INACTIVE {
                tree.headers[i].count += 1;
            }
        }
    }

    let mut keyed: Vec<(bool, Vec<i64>, Header)> = tree
        .headers
        .iter()
        .map(|h| (header_hidden(tree, state, h), header_path(tree, h), *h))
        .collect();
    keyed.sort_by(|(ah, ap, _), (bh, bp, _)| {
        ah.cmp(bh).then_with(|| {
            ap.iter()
                .zip(bp)
                .map(|(&a, &b)| header_order(tree, state, a, b))
                .find(|o| o.is_ne())
                .unwrap_or_else(|| ap.len().cmp(&bp.len()))
        })
    });
    tree.headers = keyed.into_iter().map(|(_, _, h)| h).collect();

    let mut shown = tree.rows.len();
    for i in 0..tree.rows.len() {
        let b = bucket(tree, state, &tree.rows[i]);
        let Some(slot) = tree.header_slot(b) else {
            continue;
        };
        tree.rows[i].header = slot;
        let (row, h) = (&tree.rows[i], &tree.headers[slot]);
        let folded = if row.is_header {
            header_hidden(tree, state, h)
        } else {
            tree.collapsed.contains(&h.faction_id)
        };
        if folded || h.nested_collapsed {
            shown -= 1;
        }
    }
    tree.shown = shown;

    // The row comparator (`0x5d02f0`): hidden last, then by header, the header's own row first.
    let keys: Vec<(bool, usize, bool, String, i64)> = tree
        .rows
        .iter()
        .map(|r| {
            let hidden = tree.headers.get(r.header).is_some_and(|h| {
                h.nested_collapsed
                    || if r.is_header {
                        h.faction_id <= OTHER && h.count == 0
                    } else {
                        tree.collapsed.contains(&h.faction_id)
                    }
            });
            let name = tree
                .record(state, r.faction_id)
                .map_or(String::new(), |e| e.name.clone());
            (hidden, r.header, !r.is_header, name, r.faction_id)
        })
        .collect();
    let mut order: Vec<usize> = (0..tree.rows.len()).collect();
    order.sort_by(|&a, &b| {
        let (ka, kb) = (&keys[a], &keys[b]);
        (ka.0, ka.1, ka.2)
            .cmp(&(kb.0, kb.1, kb.2))
            .then_with(|| collate(&ka.3, &kb.3))
            .then(ka.4.cmp(&kb.4))
    });
    tree.rows = order.into_iter().map(|i| tree.rows[i]).collect();
}

/// The descendant test (`0x5cfee0`): header `slot` sits somewhere under `ancestor`.
fn is_descendant(tree: &FactionTree, mut slot: usize, ancestor: i64) -> bool {
    if ancestor == INACTIVE || ancestor == OTHER {
        return false;
    }
    for _ in 0..MAX_DEPTH {
        let parent = tree.headers[slot].parent;
        if parent == ancestor {
            return true;
        }
        if parent == OTHER || parent == INACTIVE {
            return false;
        }
        let Some(next) = tree.header_slot(parent) else {
            return false;
        };
        slot = next;
    }
    false
}

/// The header collapse (`0x5d1d20`) by 0-based row: a header row folds itself and its sub-headers;
/// any other row, or none, folds or opens every header. `false` when no header matched, the one arm
/// that neither sorts nor fires `UPDATE_FACTION`.
fn collapse(tree: &mut FactionTree, state: &ReputationState, row: usize, fold: bool) -> bool {
    match tree.rows.get(row).filter(|r| r.is_header) {
        Some(r) => {
            let Some(slot) = tree.header_slot(r.parent) else {
                return false;
            };
            let id = tree.headers[slot].faction_id;
            if fold {
                tree.collapsed.insert(id);
            } else {
                tree.collapsed.remove(&id);
            }
            for i in 0..tree.headers.len() {
                if is_descendant(tree, i, id) {
                    tree.headers[i].nested_collapsed = fold;
                }
            }
        }
        None if fold => tree.collapsed = tree.headers.iter().map(|h| h.faction_id).collect(),
        None => tree.collapsed.clear(),
    }
    sort(tree, state);
    true
}

/// The VM's text for a synthetic header; the enUS literal in a bare test VM.
fn synthetic_header(lua: &Lua, key: i64) -> String {
    let (global, fallback) = if key == INACTIVE {
        ("FACTION_INACTIVE", "Inactive")
    } else {
        ("FACTION_OTHER", "Other")
    };
    lua.globals()
        .get::<String>(global)
        .unwrap_or_else(|_| fallback.into())
}

/// The faction-info helper (`0x5d0da0`): name, description, standingID, barMin, barMax, barValue,
/// atWarWith, canToggleAtWar, isHeader, isCollapsed, hasRep, isWatched, isChild.
fn faction_info(lua: &Lua, model: &Model, faction: i64) -> mlua::Result<MultiValue> {
    let (tree, state) = (&model.reputation_tree, &model.reputation);
    let Some(e) = tree.record(state, faction) else {
        let synthetic =
            (faction == OTHER || faction == INACTIVE) && tree.header_slot(faction).is_some();
        let mut out = if synthetic {
            vec![
                Value::String(lua.create_string(synthetic_header(lua, faction))?),
                Value::String(lua.create_string("")?),
            ]
        } else {
            vec![Value::Nil, Value::Nil]
        };
        // The synthetic headers and the miss both push standingID 1 (`0x5d10a8`, `0x5d100a`).
        out.push(Value::Integer(1));
        out.extend(std::iter::repeat_n(Value::Integer(0), 3));
        out.extend([Value::Nil, Value::Nil]);
        if synthetic {
            out.push(Value::Integer(1));
            out.push(binding_abi::flag(tree.collapsed.contains(&faction)));
        }
        out.resize(13, Value::Nil);
        return Ok(MultiValue::from_vec(out));
    };
    let header = tree.header_slot(faction).is_some();
    let is_child = tree
        .shown_rows()
        .iter()
        .find(|r| r.faction_id == faction)
        .and_then(|r| tree.headers.get(r.header))
        .is_some_and(|h| h.parent != 0);
    Ok(MultiValue::from_vec(vec![
        Value::String(lua.create_string(&e.name)?),
        Value::String(lua.create_string(&e.description)?),
        Value::Integer(i64::from(e.standing_id)),
        Value::Integer(i64::from(e.bar_min)),
        Value::Integer(i64::from(e.bar_max)),
        Value::Integer(i64::from(e.standing)),
        binding_abi::flag(e.at_war),
        binding_abi::flag(e.standing >= -3000 && !e.peace_forced),
        binding_abi::flag(header),
        binding_abi::flag(header && tree.collapsed.contains(&faction)),
        binding_abi::flag(header && e.has_rep),
        binding_abi::flag(state.watched == Some(e.rep_list_id)),
        binding_abi::flag(is_child),
    ]))
}

/// A header in any slot but the first refuses the war and inactive toggles: the reference tests
/// `index > 0` (`0x5d0a10`, `0x5d1c10`).
fn header_refuses(tree: &FactionTree, faction: i64) -> bool {
    tree.header_slot(faction).is_some_and(|slot| slot > 0)
}

/// The at-war set (`0x5d0a10`): peace needs standing of -3000 or better and no combat; the local
/// flag takes war only when peace is not forced, and the send carries the asked state either way.
fn set_at_war(model: &mut Model, faction: i64, at_war: bool) {
    let tree = &model.reputation_tree;
    if header_refuses(tree, faction) {
        return;
    }
    let Some(&i) = tree.index.get(&faction) else {
        return;
    };
    let Some(e) = model.reputation.entries.get(i) else {
        return;
    };
    if !at_war && e.standing < -3000 {
        return;
    }
    if !at_war && model.unit("player").is_some_and(|u| u.in_combat) {
        model.ui_errors.push("ERR_NOT_IN_COMBAT");
        return;
    }
    let e = &mut model.reputation.entries[i];
    e.at_war = at_war && !e.peace_forced;
    let send = ReputationSend::AtWar {
        rep_list_id: e.rep_list_id,
        at_war,
    };
    model.reputation_sends.push(send);
}

/// `SetFactionInactive` (`0x5d1c10`): the flag is local first, then the list re-sorts. `false`
/// when refused, which fires no `UPDATE_FACTION`.
fn set_inactive(model: &mut Model, faction: i64, inactive: bool) -> bool {
    let tree = &model.reputation_tree;
    if header_refuses(tree, faction) {
        return false;
    }
    let Some(&i) = tree.index.get(&faction) else {
        return false;
    };
    let Some(e) = model.reputation.entries.get_mut(i) else {
        return false;
    };
    e.inactive = inactive;
    let send = ReputationSend::Inactive {
        rep_list_id: e.rep_list_id,
        inactive,
    };
    model.reputation_sends.push(send);
    sort(&mut model.reputation_tree, &model.reputation);
    true
}

fn update_faction(lua: &Lua) {
    super::event::fire_global(lua, "UPDATE_FACTION", &[]);
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumFactions() → the shown row count (`0x5cff20`), 0 before any push.
    g.set(
        "GetNumFactions",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.reputation_tree.shown as i64)
        })?,
    )?;

    // GetFactionInfo(index) → the helper on that row's faction, 0 past the list (`0x5d1150`).
    g.set(
        "GetFactionInfo",
        lua.create_function(|lua, index: usize| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            faction_info(lua, &model, model.reputation_tree.faction_at(index))
        })?,
    )?;

    // GetFactionInfoByID(id) → the same helper on a faction id (`0x5d11e0`).
    g.set(
        "GetFactionInfoByID",
        lua.create_function(|lua, id: Value| {
            let id = binding_abi::number_arg(lua, id, "Usage: GetFactionInfoByID(id)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            faction_info(lua, &model, i64::from(id))
        })?,
    )?;

    // GetSelectedFaction() → the selected faction's shown index, or 0 (`0x5d0820`).
    g.set(
        "GetSelectedFaction",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let tree = &model.reputation_tree;
            let found = tree
                .shown_rows()
                .iter()
                .position(|r| r.faction_id == tree.selected);
            Ok(found.map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    // SetSelectedFaction(index): that row's faction, a header's included, or none (`0x5d07b0`).
    g.set(
        "SetSelectedFaction",
        lua.create_function(|lua, index: usize| {
            let mut model = crate::script::model_mut_of(lua);
            let tree = &mut model.reputation_tree;
            tree.selected = tree.faction_at(index);
            Ok(())
        })?,
    )?;

    // Fold verbs (`0x5d1f70`, `0x5d2000`, `0x5d1fc0`, `0x5d1fe0`); the sort fires UPDATE_FACTION.
    for (name, fold, all) in [
        ("CollapseFactionHeader", true, false),
        ("ExpandFactionHeader", false, false),
        ("CollapseAllFactionHeaders", true, true),
        ("ExpandAllFactionHeaders", false, true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Option<usize>| {
                let sorted = {
                    let mut model = crate::script::model_mut_of(lua);
                    let m: &mut Model = &mut model;
                    let tree = &mut m.reputation_tree;
                    let row = if all {
                        tree.rows.len()
                    } else {
                        index.unwrap_or(0).wrapping_sub(1)
                    };
                    collapse(tree, &m.reputation, row, fold)
                };
                if sorted {
                    update_faction(lua);
                }
                Ok(())
            })?,
        )?;
    }

    // FactionToggleAtWar(index) (`0x5d1390`): flips that row's faction.
    g.set(
        "FactionToggleAtWar",
        lua.create_function(|lua, index: usize| {
            let mut model = crate::script::model_mut_of(lua);
            let faction = model.reputation_tree.faction_at(index);
            if faction == 0 {
                return Ok(());
            }
            let at_war = model
                .reputation_tree
                .record(&model.reputation, faction)
                .is_some_and(|e| e.at_war);
            set_at_war(&mut model, faction, !at_war);
            Ok(())
        })?,
    )?;

    // IsFactionInactive(index) → that row's flag `0x20` (`0x5d0740`).
    g.set(
        "IsFactionInactive",
        lua.create_function(|lua, index: usize| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let tree = &model.reputation_tree;
            let inactive = tree
                .record(&model.reputation, tree.faction_at(index))
                .is_some_and(|e| e.inactive);
            Ok(binding_abi::flag(inactive))
        })?,
    )?;

    // SetFactionInactive / SetFactionActive(index) (`0x5d1e50`, `0x5d1ee0`).
    for (name, inactive) in [("SetFactionInactive", true), ("SetFactionActive", false)] {
        g.set(
            name,
            lua.create_function(move |lua, index: usize| {
                let sorted = {
                    let mut model = crate::script::model_mut_of(lua);
                    let faction = model.reputation_tree.faction_at(index);
                    faction != 0 && set_inactive(&mut model, faction, inactive)
                };
                if sorted {
                    update_faction(lua);
                }
                Ok(())
            })?,
        )?;
    }

    // GetWatchedFactionInfo() → name, standingID, min, max, value; nil name for none (`0x5d1240`).
    g.set(
        "GetWatchedFactionInfo",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let watched = model.reputation.watched.and_then(|slot| {
                model
                    .reputation
                    .entries
                    .iter()
                    .find(|e| e.rep_list_id == slot)
            });
            let Some(e) = watched else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&e.name)?),
                Value::Integer(i64::from(e.standing_id)),
                Value::Integer(i64::from(e.bar_min)),
                Value::Integer(i64::from(e.bar_max)),
                Value::Integer(i64::from(e.standing)),
            ]))
        })?,
    )?;

    // SetWatchedFactionIndex(index): sends the row's slot when the field differs (`0x5d0ba0`).
    g.set(
        "SetWatchedFactionIndex",
        lua.create_function(|lua, index: usize| {
            let mut model = crate::script::model_mut_of(lua);
            let tree = &model.reputation_tree;
            let slot = tree
                .record(&model.reputation, tree.faction_at(index))
                .map(|e| e.rep_list_id);
            if slot != model.reputation.watched {
                model.reputation_sends.push(ReputationSend::Watch(slot));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{UiScript, UnitState};

    /// A met faction bar: Friendly, 1000 into its rank window.
    fn entry(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
        FactionEntry {
            faction_id,
            rep_list_id,
            parent_id,
            name: name.into(),
            description: format!("About the {name}."),
            standing: 4000,
            standing_id: 5,
            bar_min: 3000,
            bar_max: 9000,
            fits_player: true,
            visible: true,
            is_header: false,
            at_war: false,
            peace_forced: false,
            inactive: false,
            has_rep: false,
        }
    }

    /// A header faction as the wire seeds one: flag `0x08`, unmet, at Neutral 0.
    fn header(faction_id: u32, rep_list_id: u32, parent_id: u32, name: &str) -> FactionEntry {
        FactionEntry {
            standing: 0,
            standing_id: 4,
            bar_min: 0,
            bar_max: 3000,
            visible: false,
            is_header: true,
            ..entry(faction_id, rep_list_id, parent_id, name)
        }
    }

    /// Two expansion headers, three sub-headers, one of them a met reputation bar, and two
    /// parentless factions; pushed out of order, with one unmet faction.
    fn state() -> ReputationState {
        ReputationState {
            entries: vec![
                entry(72, 19, 469, "Stormwind"),
                entry(529, 13, 0, "Argent Dawn"),
                header(1097, 89, 0, "Wrath of the Lich King"),
                header(469, 11, 1118, "Alliance"),
                entry(1050, 92, 1037, "Valiance Expedition"),
                entry(21, 1, 169, "Booty Bay"),
                entry(1106, 101, 1097, "Argent Crusade"),
                FactionEntry {
                    visible: true,
                    has_rep: true,
                    standing: 21500,
                    standing_id: 7,
                    bar_min: 21000,
                    bar_max: 42000,
                    ..header(1037, 88, 1097, "Alliance Vanguard")
                },
                entry(47, 20, 469, "Ironforge"),
                header(1118, 96, 0, "Classic"),
                header(169, 10, 1118, "Steamwheedle Cartel"),
                entry(87, 0, 0, "Bloodsail Buccaneers"),
                FactionEntry {
                    visible: false,
                    ..entry(1090, 99, 1097, "Kirin Tor")
                },
            ],
            watched: None,
        }
    }

    fn seated() -> UiScript {
        let mut s = UiScript::new().expect("VM");
        s.set_reputation(state());
        s
    }

    fn names(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>(
            "local t = {} for i = 1, GetNumFactions() do t[i] = (GetFactionInfo(i)) end return t",
        )
        .expect("names")
    }

    /// Each shown row's tree shape: `isHeader`, `isChild`, `hasRep`, as the stock pane reads them.
    fn shapes(s: &UiScript) -> Vec<(bool, bool, bool)> {
        s.eval::<Vec<Vec<bool>>>(
            "local t = {} for i = 1, GetNumFactions() do \
               local _,_,_,_,_,_,_,_,h,_,r,_,c = GetFactionInfo(i) \
               t[i] = {h ~= nil, c ~= nil, r ~= nil} end return t",
        )
        .expect("shapes")
        .into_iter()
        .map(|v| (v[0], v[1], v[2]))
        .collect()
    }

    const TREE: [&str; 13] = [
        "Classic",
        "Alliance",
        "Ironforge",
        "Stormwind",
        "Steamwheedle Cartel",
        "Booty Bay",
        "Wrath of the Lich King",
        "Argent Crusade",
        "Alliance Vanguard",
        "Valiance Expedition",
        "Other",
        "Argent Dawn",
        "Bloodsail Buccaneers",
    ];

    /// Expansions by name, then "Other"; under each, its own factions, then its sub-headers by
    /// name, each over its factions.
    #[test]
    fn the_list_nests_sub_headers_under_their_expansion() {
        let s = seated();
        assert_eq!(names(&s), TREE);
        let (h, c, r) = (true, true, true);
        assert_eq!(
            shapes(&s),
            [
                (h, !c, !r), // Classic
                (h, c, !r),  // Alliance
                (!h, c, !r), // Ironforge
                (!h, c, !r), // Stormwind
                (h, c, !r),  // Steamwheedle Cartel
                (!h, c, !r), // Booty Bay
                (h, !c, !r), // Wrath of the Lich King
                (!h, !c, !r),
                (h, c, r), // Alliance Vanguard, a bar of its own
                (!h, c, !r),
                (h, !c, !r), // Other
                (!h, !c, !r),
                (!h, !c, !r),
            ]
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn every_path_answers_thirteen_values_and_headers_carry_a_standing() {
        let s = seated();
        for call in [
            "GetFactionInfo(1)",
            "GetFactionInfo(3)",
            "GetFactionInfo(11)",
            "GetFactionInfo(99)",
            "GetFactionInfoByID(1090)",
            "GetFactionInfoByID(424242)",
        ] {
            assert_eq!(s.arity(call).unwrap(), 13, "{call}");
        }
        // Every row's standingID indexes `FACTION_BAR_COLORS`, which the stock pane does unguarded.
        let ids = s
            .eval::<Vec<i64>>(
                "local t = {} for i = 1, GetNumFactions() do \
                   local _,_,s = GetFactionInfo(i) t[i] = s end return t",
            )
            .unwrap();
        assert!(ids.iter().all(|s| (1..=8).contains(s)), "{ids:?}");
        assert_eq!(ids[0], 4, "a header faction reports its own rank");
        assert_eq!(ids[8], 7, "and a header bar its own");
        assert_eq!(ids[10], 1, "the synthetic header reports 1");

        let (name, sid, header) = s
            .eval::<(Value, i64, Value)>(
                "local n,_,s,_,_,_,_,_,h = GetFactionInfoByID(424242) return n, s, h",
            )
            .unwrap();
        assert!(name.is_nil() && header.is_nil());
        assert_eq!(sid, 1, "the miss's standingID is 1 too");
        // Past the list the reference asks for faction 0, the "Other" header.
        assert_eq!(
            s.eval::<String>("return (GetFactionInfo(99))").unwrap(),
            "Other"
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn an_unmet_faction_answers_by_id_but_is_no_row() {
        let s = seated();
        let (name, child) = s
            .eval::<(String, Value)>(
                "local n,_,_,_,_,_,_,_,_,_,_,_,c = GetFactionInfoByID(1090) return n, c",
            )
            .unwrap();
        assert_eq!(name, "Kirin Tor");
        assert!(child.is_nil(), "isChild asks the shown rows");
        assert!(!names(&s).contains(&"Kirin Tor".to_string()));
    }

    #[test]
    fn folding_an_expansion_hides_its_sub_headers_and_keeps_their_own_folds() {
        let s = seated();
        s.run("CollapseFactionHeader(2)").unwrap(); // Alliance
        assert_eq!(names(&s).len(), 11, "its two factions leave");
        s.run("CollapseFactionHeader(1)").unwrap(); // Classic
        assert_eq!(
            names(&s),
            [
                "Classic",
                "Wrath of the Lich King",
                "Argent Crusade",
                "Alliance Vanguard",
                "Valiance Expedition",
                "Other",
                "Argent Dawn",
                "Bloodsail Buccaneers",
            ]
        );
        s.run("ExpandFactionHeader(1)").unwrap();
        assert_eq!(names(&s).len(), 11, "Alliance comes back still folded");
        let folded = s
            .eval::<Value>("local _,_,_,_,_,_,_,_,_,c = GetFactionInfo(2) return c")
            .unwrap();
        assert_eq!(folded, Value::Integer(1));
        s.run("ExpandFactionHeader(2)").unwrap();
        assert_eq!(names(&s), TREE);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn the_all_headers_verbs_and_a_non_header_index_fold_every_header() {
        let s = seated();
        s.run("CollapseAllFactionHeaders()").unwrap();
        assert_eq!(
            names(&s),
            [
                "Classic",
                "Alliance",
                "Steamwheedle Cartel",
                "Wrath of the Lich King",
                "Alliance Vanguard",
                "Other",
            ],
            "every header row stays, sub-headers too"
        );
        s.run("ExpandAllFactionHeaders()").unwrap();
        assert_eq!(names(&s), TREE);
        s.run("CollapseFactionHeader(3)").unwrap(); // Ironforge, not a header
        assert_eq!(names(&s).len(), 6);
        s.run("ExpandFactionHeader(0)").unwrap();
        assert_eq!(names(&s), TREE);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn folds_survive_a_standing_push_and_a_new_header_unfolds_them() {
        let mut s = seated();
        s.run("CollapseFactionHeader(2)").unwrap(); // Alliance
        let mut st = state();
        st.entries[0].standing = 5000; // Stormwind ticks
        s.set_reputation(st.clone());
        assert_eq!(names(&s).len(), 11, "still folded");

        // Kirin Tor turns visible under a header that exists: the folds stand.
        st.entries[12].visible = true;
        s.set_reputation(st.clone());
        assert_eq!(names(&s).len(), 12);
        assert!(!names(&s).contains(&"Ironforge".to_string()));

        // A faction under a new header adds it, which unfolds every header.
        st.entries.push(header(980, 43, 0, "The Burning Crusade"));
        st.entries.push(entry(932, 50, 980, "The Aldor"));
        s.set_reputation(st);
        let got = names(&s);
        assert!(got.contains(&"Ironforge".to_string()), "{got:?}");
        assert_eq!(&got[6..8], ["The Burning Crusade", "The Aldor"]);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn an_empty_header_is_hidden_unless_it_is_a_bar() {
        let mut s = UiScript::new().expect("VM");
        let mut st = state();
        st.entries.push(FactionEntry {
            visible: true,
            ..header(980, 43, 0, "The Burning Crusade")
        });
        st.entries[4].visible = false; // Valiance Expedition: Alliance Vanguard keeps no member
        s.set_reputation(st);
        let got = names(&s);
        assert!(!got.contains(&"The Burning Crusade".to_string()), "{got:?}");
        assert_eq!(
            &got[6..9],
            [
                "Wrath of the Lich King",
                "Argent Crusade",
                "Alliance Vanguard"
            ]
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn an_inactive_faction_files_under_the_folded_inactive_header() {
        let mut s = UiScript::new().expect("VM");
        let mut st = state();
        st.entries[1].inactive = true; // Argent Dawn
        s.set_reputation(st);
        let got = names(&s);
        assert_eq!(&got[10..], ["Other", "Bloodsail Buccaneers", "Inactive"]);
        s.run("ExpandFactionHeader(13)").unwrap();
        assert_eq!(names(&s).last().unwrap(), "Argent Dawn");

        // The toggle re-sorts and repaints at once.
        s.run(
            "UPDATES = 0 local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_FACTION') \
             f:SetScript('OnEvent', function() UPDATES = UPDATES + 1 end)",
        )
        .unwrap();
        s.run("SetFactionInactive(4)").unwrap(); // Stormwind
        assert_eq!(s.eval::<i64>("return UPDATES").unwrap(), 1);
        let got = names(&s);
        assert_eq!(
            &got[got.len() - 3..],
            ["Inactive", "Argent Dawn", "Stormwind"]
        );
        assert_eq!(
            s.take_reputation_sends(),
            [ReputationSend::Inactive {
                rep_list_id: 19,
                inactive: true
            }]
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn the_selection_is_a_faction_and_a_header_can_hold_it() {
        let s = seated();
        s.run("SetSelectedFaction(4)").unwrap(); // Stormwind
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 4);
        s.run("CollapseFactionHeader(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 0);
        s.run("ExpandFactionHeader(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 4);
        s.run("SetSelectedFaction(9)").unwrap(); // Alliance Vanguard
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 9);
        // None is faction 0, which the "Other" header's row also carries.
        s.run("SetSelectedFaction(99)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSelectedFaction()").unwrap(), 11);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn the_war_toggle_follows_set_at_war() {
        let mut s = UiScript::new().expect("VM");
        let mut st = state();
        st.entries[8].peace_forced = true; // Ironforge
        s.set_reputation(st.clone());
        let at_war = |s: &UiScript, i: usize| {
            !s.eval::<Value>(&format!(
                "local _,_,_,_,_,_,w = GetFactionInfo({i}) return w"
            ))
            .unwrap()
            .is_nil()
        };

        s.run("FactionToggleAtWar(4)").unwrap(); // Stormwind
        assert!(at_war(&s, 4), "written locally, unacked");
        s.run("FactionToggleAtWar(3)").unwrap(); // Ironforge: sent, but peace holds
        assert!(!at_war(&s, 3));
        assert_eq!(
            s.take_reputation_sends(),
            [
                ReputationSend::AtWar {
                    rep_list_id: 19,
                    at_war: true
                },
                ReputationSend::AtWar {
                    rep_list_id: 20,
                    at_war: true
                },
            ]
        );

        // Peace in combat is refused with the reference's error.
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                in_combat: true,
                ..UnitState::default()
            }),
        );
        s.run("FactionToggleAtWar(4)").unwrap();
        assert!(at_war(&s, 4));
        assert_eq!(s.take_ui_errors(), ["ERR_NOT_IN_COMBAT"]);
        s.set_unit("player", None);

        // So is peace below -3000.
        st.entries[0].at_war = true;
        st.entries[0].standing = -5000;
        s.set_reputation(st);
        s.run("FactionToggleAtWar(4)").unwrap();
        assert!(at_war(&s, 4));

        // A header past the first slot refuses; the first does not.
        s.run("FactionToggleAtWar(2)").unwrap(); // Alliance
        assert!(s.take_reputation_sends().is_empty());
        s.run("FactionToggleAtWar(1)").unwrap(); // Classic
        assert_eq!(
            s.take_reputation_sends(),
            [ReputationSend::AtWar {
                rep_list_id: 96,
                at_war: true
            }]
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    #[test]
    fn watching_sends_only_a_change_and_never_moves_the_bar_itself() {
        let mut s = seated();
        assert!(s
            .eval::<Value>("return GetWatchedFactionInfo()")
            .unwrap()
            .is_nil());
        s.run("SetWatchedFactionIndex(4)").unwrap(); // Stormwind
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(Some(19))]);
        assert!(
            s.eval::<Value>("return GetWatchedFactionInfo()")
                .unwrap()
                .is_nil(),
            "the server owns the field"
        );
        s.run("SetWatchedFactionIndex(0) SetWatchedFactionIndex(11)")
            .unwrap();
        assert!(
            s.take_reputation_sends().is_empty(),
            "none, and the synthetic header, are already what is watched"
        );

        let mut watched = state();
        watched.watched = Some(19);
        s.set_reputation(watched);
        assert_eq!(
            s.eval::<(String, i64, i64, i64, i64)>("return GetWatchedFactionInfo()")
                .unwrap(),
            ("Stormwind".into(), 5, 3000, 9000, 4000)
        );
        assert_eq!(
            s.eval::<Value>("local _,_,_,_,_,_,_,_,_,_,_,w = GetFactionInfo(4) return w")
                .unwrap(),
            Value::Integer(1)
        );
        s.run("SetWatchedFactionIndex(4)").unwrap();
        assert!(s.take_reputation_sends().is_empty(), "already watched");
        s.run("SetWatchedFactionIndex(9) SetWatchedFactionIndex(0)")
            .unwrap(); // Alliance Vanguard, then none
        assert_eq!(
            s.take_reputation_sends(),
            [ReputationSend::Watch(Some(88)), ReputationSend::Watch(None)]
        );
        // Slot 0 is a faction, not "none".
        s.run("SetWatchedFactionIndex(13)").unwrap(); // Bloodsail Buccaneers
        assert_eq!(s.take_reputation_sends(), [ReputationSend::Watch(Some(0))]);
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }
}
