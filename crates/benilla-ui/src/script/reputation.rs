//! The reputation pane's fifteen globals, a port of 12340's reputation info
//! (`0x5cfe40`..`0x5d2e30`). The client's faction list and header tree ([`FactionList`]) is client
//! state the app keeps; every binding reads it, the faction table and the player's reputation store
//! through the lent view at the call ([`FactionTable`]). Indexes are 1-based into that list, whose
//! shown rows come first; every faction lookup goes through one helper (`0x5d0da0`), which answers
//! thirteen values on every path. The packet handlers that write the list live in the app and raise
//! `UPDATE_FACTION` at their steps.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;

/// The synthetic header for factions the player moved to the inactive bucket.
const INACTIVE: i64 = -1;
/// The synthetic header for factions with no parent, "Other".
const OTHER: i64 = 0;
/// A guard on the parent walks; 3.3.5a's tree is two headers deep.
const MAX_DEPTH: usize = 16;

/// One reputation faction as its record and the player's slot read at the call.
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
    /// One of the four race/class slots fits the player: the world entry lists only these
    /// (`0x5d2bc0`).
    pub fits_player: bool,
    /// Flag `0x01`: the player has met the faction, which puts it in the list.
    pub visible: bool,
    /// Flag `0x08`: the list adds this faction as a header row (`0x5d08d0`).
    pub is_header: bool,
    /// Flag `0x02`.
    pub at_war: bool,
    /// Flag `0x10`: war can never be declared.
    pub peace_forced: bool,
    /// Flag `0x20`; it files the row under "Inactive".
    pub inactive: bool,
    /// The faction's has-reputation test (`0x5d0ce0`): a header that is also a reputation bar.
    pub has_rep: bool,
}

/// One outbound reputation verb, sent inside its call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReputationSend {
    /// `CMSG_SET_FACTION_ATWAR`, carrying the asked state (`0x5d0a10`).
    AtWar { rep_list_id: u32, at_war: bool },
    /// `CMSG_SET_FACTION_INACTIVE` (`0x5d1c10`).
    Inactive { rep_list_id: u32, inactive: bool },
    /// `CMSG_SET_WATCHED_FACTION` with the wire slot, `-1` for none (`0x5d0ba0`).
    Watch(i32),
}

/// A stored flag a binding writes in its call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactionFlag {
    /// `0x02`, the at-war set's (`0x5d0a10`).
    AtWar,
    /// `0x20`, `SetFactionInactive`'s (`0x5d1c10`).
    Inactive,
}

/// What the faction list reads and writes beside itself, at the call: the faction table, the
/// player's reputation store (`0xc22b70`) and the wire. The app implements it over the `World`.
pub trait FactionTable {
    /// The record for a `Faction.dbc` id with a reputation slot, read with the slot as it stands.
    fn faction(&self, faction_id: i64) -> Option<FactionEntry>;
    /// The faction-info helper's lookup (`0x5d0da0`): any `Faction.dbc` record, one without a slot
    /// read as slot `0xFFFFFFFF`, Neutral at 0 (`0x5d062d`, `0x5d05dd`).
    fn record(&self, faction_id: i64) -> Option<FactionEntry> {
        self.faction(faction_id)
    }
    /// The faction a reputation slot names (the store's faction id), as [`Self::faction`] reads it.
    fn slot_faction(&self, rep_list_id: u32) -> Option<FactionEntry>;
    /// The world entry's walk (`0x5d2bc0`): from the table's end, the slot of every faction whose
    /// race/class slot fits the player and that the store marks met.
    fn met_slots(&self) -> Vec<u32>;
    /// `PLAYER_FIELD_WATCHED_FACTION_INDEX`, `None` without an active player.
    fn watched(&self) -> Option<i32>;
    /// The active player's in-combat bit (`UNIT_FLAG` `0x80000`).
    fn in_combat(&self) -> bool;
    /// Write one stored flag of a slot.
    fn write_flag(&mut self, rep_list_id: u32, flag: FactionFlag, set: bool);
    /// Send one verb's packet.
    fn send(&mut self, send: ReputationSend);
}

/// A table over fixed rows, the faction table a VM outside the app and the tests lend.
#[derive(Clone, Debug, Default)]
pub struct StaticFactions {
    pub entries: Vec<FactionEntry>,
    /// The watched field; `None` is no active player.
    pub watched: Option<i32>,
    pub in_combat: bool,
    /// The packets sent, in order.
    pub sends: Vec<ReputationSend>,
}

impl FactionTable for StaticFactions {
    fn faction(&self, faction_id: i64) -> Option<FactionEntry> {
        self.entries
            .iter()
            .find(|e| i64::from(e.faction_id) == faction_id)
            .cloned()
    }

    fn slot_faction(&self, rep_list_id: u32) -> Option<FactionEntry> {
        self.entries
            .iter()
            .find(|e| e.rep_list_id == rep_list_id)
            .cloned()
    }

    fn met_slots(&self) -> Vec<u32> {
        let mut met: Vec<&FactionEntry> = self
            .entries
            .iter()
            .filter(|e| e.fits_player && e.visible)
            .collect();
        met.sort_by_key(|e| std::cmp::Reverse(e.faction_id));
        met.into_iter().map(|e| e.rep_list_id).collect()
    }

    fn watched(&self) -> Option<i32> {
        self.watched
    }

    fn in_combat(&self) -> bool {
        self.in_combat
    }

    fn write_flag(&mut self, rep_list_id: u32, flag: FactionFlag, set: bool) {
        for e in self
            .entries
            .iter_mut()
            .filter(|e| e.rep_list_id == rep_list_id)
        {
            match flag {
                FactionFlag::AtWar => e.at_war = set,
                FactionFlag::Inactive => e.inactive = set,
            }
        }
    }

    fn send(&mut self, send: ReputationSend) {
        self.sends.push(send);
    }
}

/// One faction list row (`0xc23480`): a faction, or a header, whose `parent` is itself.
#[derive(Clone, Copy, Debug)]
struct Row {
    rep_list_id: i64,
    faction_id: i64,
    is_header: bool,
    parent: i64,
    /// Its slot in [`FactionList::headers`], set by [`FactionList::sort`].
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

/// The records one sort or add reads, by faction id: each read once at the call.
type Records = HashMap<i64, FactionEntry>;

/// The client's faction list and header tree (`0xc23370`-`0xc2347c`, the fold mask `0xad0ae4`):
/// built at the world entry (`0x5d2bc0`), grown by the visible and standing handlers, sorted by them
/// and the bindings, and cleared at the game shutdown (`0x5d14f0`).
#[derive(Clone, Debug, Default)]
pub struct FactionList {
    rows: Vec<Row>,
    headers: Vec<Header>,
    /// Headers folded by their own button: the clear bits of the mask at `0xad0ae4`.
    collapsed: HashSet<i64>,
    /// `GetNumFactions` (`0xc23478`): the leading rows of [`Self::rows`] on screen.
    shown: usize,
    /// The selection as a faction id, 0 for none (`0xc2347c`).
    selected: i64,
}

impl FactionList {
    /// The rows of the list, the synthetic ones' included.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// `GetNumFactions` (`0xc23478`).
    pub fn shown(&self) -> usize {
        self.shown
    }

    fn header_slot(&self, id: i64) -> Option<usize> {
        self.headers.iter().position(|h| h.faction_id == id)
    }

    /// The faction of a 0-based row, 0 past the list: the reference reads `row <= count`, one
    /// slot further, which holds no row here.
    fn faction_at(&self, row: usize) -> i64 {
        self.rows.get(row).map_or(0, |r| r.faction_id)
    }

    fn shown_rows(&self) -> &[Row] {
        &self.rows[..self.shown.min(self.rows.len())]
    }

    /// The world entry's build (`0x5d2bc0`) up to its fold: every met faction whose slot fits,
    /// walked from the table's end, then the "Inactive" header row, whose 0-based index is
    /// returned for the fold (`0x5d2e0f`) and the sort (`0x5d2e17`) the caller runs next, each
    /// raising `UPDATE_FACTION`. The selection stands.
    pub fn enter_world(&mut self, t: &dyn FactionTable) -> usize {
        self.begin_world_entry();
        for slot in t.met_slots() {
            self.add_visible(t, slot);
        }
        self.end_world_entry()
    }

    /// The world entry's clear (`0x5d2c17`-`0x5d2c31`): no rows, no headers, every fold open.
    pub fn begin_world_entry(&mut self) {
        self.rows.clear();
        self.headers.clear();
        self.collapsed.clear();
        self.shown = 0;
    }

    /// The world entry's "Inactive" header row after its walk (`0x5d2db0`-`0x5d2e04`), its 0-based
    /// index returned for the fold.
    pub fn end_world_entry(&mut self) -> usize {
        self.add_header(INACTIVE, 0);
        self.rows.push(Row {
            rep_list_id: -1,
            faction_id: INACTIVE,
            is_header: true,
            parent: INACTIVE,
            header: 0,
        });
        self.rows.len() - 1
    }

    /// The visible-faction add (`0x5d08d0`): one row per slot, a header by its stored flag, then
    /// the rows of every ancestor header, "Other" for a parentless one. `false` for a slot already
    /// listed; the caller announces a new one (`0x5d09f2`). The announce is the add's last step but
    /// the count's increment (`0x5d09fa`), which no verb reads apart: `GetNumFactions` reads the
    /// sort's count (`0x5cff24`) and the index verbs reach the uncounted row (`0x5d11a5`).
    pub fn add_visible(&mut self, t: &dyn FactionTable, rep_list_id: u32) -> bool {
        let rep = i64::from(rep_list_id);
        if self.rows.iter().any(|r| r.rep_list_id == rep) {
            return false;
        }
        let entry = t.slot_faction(rep_list_id);
        let faction_id = entry.as_ref().map_or(0, |e| i64::from(e.faction_id));
        let is_header = entry.as_ref().is_some_and(|e| e.is_header);
        let parent = entry.as_ref().map_or(0, |e| i64::from(e.parent_id));
        self.rows.push(Row {
            rep_list_id: rep,
            faction_id,
            is_header,
            parent: if is_header { faction_id } else { parent },
            header: 0,
        });
        if is_header {
            self.add_header(faction_id, parent);
        }
        self.add_faction_header(t, parent, 0);
        true
    }

    /// The faction-header add (`0x5d01f0`): the ancestors first, then a header row for `id` unless
    /// one exists.
    fn add_faction_header(&mut self, t: &dyn FactionTable, id: i64, depth: usize) {
        let record = t.faction(id);
        let parent = record.as_ref().map_or(0, |e| i64::from(e.parent_id));
        let rep = record.as_ref().map_or(-1, |e| i64::from(e.rep_list_id));
        if parent != 0 && depth < MAX_DEPTH {
            self.add_faction_header(t, parent, depth + 1);
        }
        if self.rows.iter().any(|r| r.is_header && r.faction_id == id) {
            return;
        }
        self.rows.push(Row {
            rep_list_id: rep,
            faction_id: id,
            is_header: true,
            parent: id,
            header: 0,
        });
        self.add_header(id, parent);
    }

    /// The header add (`0x5cfe70`): every call unfolds every header.
    fn add_header(&mut self, id: i64, parent: i64) {
        if self.header_slot(id).is_none() {
            self.headers.push(Header {
                faction_id: id,
                parent,
                count: 0,
                nested_collapsed: false,
            });
        }
        self.collapsed.clear();
    }

    /// The faction sort (`0x5d1880`): count each header's rows, order the headers, count the shown
    /// rows and order the list, shown rows first. The caller raises the `UPDATE_FACTION` it ends
    /// with (`0x5d1bfa`).
    pub fn sort(&mut self, t: &dyn FactionTable) {
        let ids: HashSet<i64> = self
            .rows
            .iter()
            .map(|r| r.faction_id)
            .chain(self.headers.iter().map(|h| h.faction_id))
            .collect();
        let recs: Records = ids
            .into_iter()
            .filter_map(|id| Some((id, t.faction(id)?)))
            .collect();
        self.sort_with(&recs);
    }

    fn sort_with(&mut self, recs: &Records) {
        let buckets: Vec<i64> = self.rows.iter().map(|r| bucket(recs, r)).collect();
        for h in 0..self.headers.len() {
            let id = self.headers[h].faction_id;
            let count = self
                .rows
                .iter()
                .zip(&buckets)
                .filter(|(r, &b)| !r.is_header && b == id)
                .count();
            self.headers[h].count = u32::try_from(count).unwrap_or(u32::MAX);
        }
        // A sub-header rolls into its parent, plus one if it is a bar itself; the pass starts at 1.
        for i in 0..self.headers.len() {
            for j in 1..self.headers.len() {
                let child = self.headers[j];
                if i == j || child.parent == 0 || self.headers[i].faction_id != child.parent {
                    continue;
                }
                self.headers[i].count += child.count;
                if has_rep(recs, child.faction_id) && child.parent != INACTIVE {
                    self.headers[i].count += 1;
                }
            }
        }

        let mut keyed: Vec<(bool, Vec<i64>, Header)> = self
            .headers
            .iter()
            .map(|h| (header_hidden(recs, h), self.header_path(h), *h))
            .collect();
        keyed.sort_by(|(ah, ap, _), (bh, bp, _)| {
            ah.cmp(bh).then_with(|| {
                ap.iter()
                    .zip(bp)
                    .map(|(&a, &b)| header_order(recs, a, b))
                    .find(|o| o.is_ne())
                    .unwrap_or_else(|| ap.len().cmp(&bp.len()))
            })
        });
        self.headers = keyed.into_iter().map(|(_, _, h)| h).collect();

        let mut shown = self.rows.len();
        for (i, &b) in buckets.iter().enumerate() {
            let Some(slot) = self.header_slot(b) else {
                continue;
            };
            self.rows[i].header = slot;
            let (row, h) = (&self.rows[i], &self.headers[slot]);
            let folded = if row.is_header {
                header_hidden(recs, h)
            } else {
                self.collapsed.contains(&h.faction_id)
            };
            if folded || h.nested_collapsed {
                shown -= 1;
            }
        }
        self.shown = shown;

        // The row comparator (`0x5d02f0`): hidden last, then by header, the header's own row first.
        let keys: Vec<(bool, usize, bool, String, i64)> = self
            .rows
            .iter()
            .map(|r| {
                let hidden = self.headers.get(r.header).is_some_and(|h| {
                    h.nested_collapsed
                        || if r.is_header {
                            h.faction_id <= OTHER && h.count == 0
                        } else {
                            self.collapsed.contains(&h.faction_id)
                        }
                });
                let name = recs
                    .get(&r.faction_id)
                    .map_or(String::new(), |e| e.name.clone());
                (hidden, r.header, !r.is_header, name, r.faction_id)
            })
            .collect();
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (ka, kb) = (&keys[a], &keys[b]);
            (ka.0, ka.1, ka.2)
                .cmp(&(kb.0, kb.1, kb.2))
                .then_with(|| collate(&ka.3, &kb.3))
                .then(ka.4.cmp(&kb.4))
        });
        self.rows = order.into_iter().map(|i| self.rows[i]).collect();
    }

    /// A header's ancestry, root first: sorted as paths, a parent leads its sub-headers and the
    /// roots order the rest, as the reference's comparator does.
    fn header_path(&self, h: &Header) -> Vec<i64> {
        let mut path = vec![h.faction_id];
        let mut parent = h.parent;
        while parent != 0 && path.len() < MAX_DEPTH {
            let Some(slot) = self.header_slot(parent) else {
                break;
            };
            path.push(parent);
            parent = self.headers[slot].parent;
        }
        path.reverse();
        path
    }

    /// The descendant test (`0x5cfee0`): header `slot` sits somewhere under `ancestor`.
    fn is_descendant(&self, mut slot: usize, ancestor: i64) -> bool {
        if ancestor == INACTIVE || ancestor == OTHER {
            return false;
        }
        for _ in 0..MAX_DEPTH {
            let parent = self.headers[slot].parent;
            if parent == ancestor {
                return true;
            }
            if parent == OTHER || parent == INACTIVE {
                return false;
            }
            let Some(next) = self.header_slot(parent) else {
                return false;
            };
            slot = next;
        }
        false
    }

    /// The header collapse (`0x5d1d20`) by 0-based row: a header row folds (`0x5d1dd3`) or opens
    /// (`0x5d1e23`) itself and its sub-headers; any other row, or none, folds or opens every header
    /// (`0x5d1e3a`). `false` when no header matched, the one arm that neither sorts nor raises
    /// `UPDATE_FACTION`; otherwise the caller raises the sort's.
    pub fn collapse(&mut self, t: &dyn FactionTable, row: usize, fold: bool) -> bool {
        match self.rows.get(row).filter(|r| r.is_header) {
            Some(r) => {
                let Some(slot) = self.header_slot(r.parent) else {
                    return false;
                };
                let id = self.headers[slot].faction_id;
                if fold {
                    self.collapsed.insert(id);
                } else {
                    self.collapsed.remove(&id);
                }
                for i in 0..self.headers.len() {
                    if self.is_descendant(i, id) {
                        self.headers[i].nested_collapsed = fold;
                    }
                }
            }
            None if fold => self.collapsed = self.headers.iter().map(|h| h.faction_id).collect(),
            None => self.collapsed.clear(),
        }
        self.sort(t);
        true
    }
}

/// The header a row files under: its parent, or "Inactive" for an inactive faction.
fn bucket(recs: &Records, row: &Row) -> i64 {
    if recs.get(&row.faction_id).is_some_and(|e| e.inactive) {
        INACTIVE
    } else {
        row.parent
    }
}

fn has_rep(recs: &Records, id: i64) -> bool {
    recs.get(&id).is_some_and(|e| e.has_rep)
}

/// The sort's hidden test (`0x5d16b0`): an empty header, unless it is a reputation bar itself.
fn header_hidden(recs: &Records, h: &Header) -> bool {
    h.count == 0 && !has_rep(recs, h.faction_id)
}

/// Case-insensitive name order, raw bytes breaking a tie: the client's case-folding UTF-8 sort.
fn collate(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

/// The header comparator (`0x5d1710`) on two headers of one level: a real faction by name, before
/// the synthetic ones, "Other" before "Inactive".
fn header_order(recs: &Records, a: i64, b: i64) -> Ordering {
    match (recs.get(&a), recs.get(&b)) {
        (Some(x), Some(y)) => collate(&x.name, &y.name).then(a.cmp(&b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => b.cmp(&a),
    }
}

/// `f` over the lent view's list and table at the call; with none lent, as a VM outside the app
/// has, over an empty list and table.
fn with_reputation<R>(f: impl FnOnce(&mut FactionList, &mut dyn FactionTable) -> R) -> R {
    let mut f = Some(f);
    let mut out = None;
    super::world_loan::reputation(&mut |list, table| {
        if let Some(f) = f.take() {
            out = Some(f(list, table));
        }
    });
    match (out, f) {
        (Some(out), _) => out,
        (None, Some(f)) => f(&mut FactionList::default(), &mut StaticFactions::default()),
        (None, None) => unreachable!("the closure ran without an answer"),
    }
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
fn faction_info(
    lua: &Lua,
    list: &FactionList,
    t: &dyn FactionTable,
    faction: i64,
) -> mlua::Result<MultiValue> {
    let Some(e) = t.record(faction) else {
        let synthetic =
            (faction == OTHER || faction == INACTIVE) && list.header_slot(faction).is_some();
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
            out.push(binding_abi::flag(list.collapsed.contains(&faction)));
        }
        out.resize(13, Value::Nil);
        return Ok(MultiValue::from_vec(out));
    };
    let header = list.header_slot(faction).is_some();
    // The child test walks the shown rows (`0x5d06e0`).
    let is_child = list
        .shown_rows()
        .iter()
        .find(|r| r.faction_id == faction)
        .and_then(|r| list.headers.get(r.header))
        .is_some_and(|h| h.parent != 0);
    // The watched test compares the record's slot dword with the player's field at the call
    // (`0x5d0cc5`): a slotless record's -1 matches a player watching nothing.
    let watched = t.watched().is_some_and(|w| w as u32 == e.rep_list_id);
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
        binding_abi::flag(header && list.collapsed.contains(&faction)),
        binding_abi::flag(header && e.has_rep),
        binding_abi::flag(watched),
        binding_abi::flag(is_child),
    ]))
}

/// A header in any slot but the first refuses the war and inactive toggles: the reference tests
/// `index > 0` (`0x5d0a10`, `0x5d1c10`).
fn header_refuses(list: &FactionList, faction: i64) -> bool {
    list.header_slot(faction).is_some_and(|slot| slot > 0)
}

/// The at-war set (`0x5d0a10`): peace needs standing of -3000 or better and no combat; the stored
/// flag takes war only when peace is not forced, written in the call, and the send carries the
/// asked state either way. The combat refusal is handed back for the caller to show at once.
#[must_use]
fn set_at_war(
    list: &FactionList,
    t: &mut dyn FactionTable,
    faction: i64,
    at_war: bool,
) -> Option<&'static str> {
    if header_refuses(list, faction) {
        return None;
    }
    let e = t.faction(faction)?;
    if !at_war && e.standing < -3000 {
        return None;
    }
    if !at_war && t.in_combat() {
        return Some("ERR_NOT_IN_COMBAT");
    }
    t.write_flag(e.rep_list_id, FactionFlag::AtWar, at_war && !e.peace_forced);
    t.send(ReputationSend::AtWar {
        rep_list_id: e.rep_list_id,
        at_war,
    });
    None
}

/// `SetFactionInactive` (`0x5d1c10`): the stored flag first, the send (`0x5d1ce7`), then the sort
/// (`0x5d1cef`). `false` when refused, which raises no `UPDATE_FACTION`.
fn set_inactive(
    list: &mut FactionList,
    t: &mut dyn FactionTable,
    faction: i64,
    inactive: bool,
) -> bool {
    if header_refuses(list, faction) {
        return false;
    }
    let Some(e) = t.faction(faction) else {
        return false;
    };
    t.write_flag(e.rep_list_id, FactionFlag::Inactive, inactive);
    t.send(ReputationSend::Inactive {
        rep_list_id: e.rep_list_id,
        inactive,
    });
    list.sort(t);
    true
}

/// A verb's 1-based index as its 0-based row: `is-number` or the verb's `Usage:` error, the
/// truncation, then `index - 1` read unsigned, so 0 or a negative index lands past the list
/// (`0x5d11a2`-`0x5d11ae`).
fn row_arg(lua: &Lua, index: Value, usage: &'static str) -> mlua::Result<usize> {
    let index = binding_abi::number_arg(lua, index, usage)?;
    Ok(index.wrapping_sub(1) as u32 as usize)
}

fn update_faction(lua: &Lua) {
    super::event::fire_global(lua, "UPDATE_FACTION", &[]);
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumFactions() → the shown row count (`0x5cff20`).
    g.set(
        "GetNumFactions",
        lua.create_function(|_, ()| Ok(with_reputation(|list, _| list.shown as i64)))?,
    )?;

    // GetFactionInfo(index) → the helper on that row's faction, 0 past the list (`0x5d1150`).
    g.set(
        "GetFactionInfo",
        lua.create_function(|lua, index: Value| {
            let row = row_arg(lua, index, "Usage: GetFactionInfo(index)")?;
            with_reputation(|list, t| faction_info(lua, list, t, list.faction_at(row)))
        })?,
    )?;

    // GetFactionInfoByID(id) → the same helper on a faction id (`0x5d11e0`).
    g.set(
        "GetFactionInfoByID",
        lua.create_function(|lua, id: Value| {
            let id = binding_abi::number_arg(lua, id, "Usage: GetFactionInfoByID(id)")?;
            with_reputation(|list, t| faction_info(lua, list, t, i64::from(id)))
        })?,
    )?;

    // GetSelectedFaction() → the selected faction's shown index, or 0 (`0x5d0820`).
    g.set(
        "GetSelectedFaction",
        lua.create_function(|_, ()| {
            Ok(with_reputation(|list, _| {
                let found = list
                    .shown_rows()
                    .iter()
                    .position(|r| r.faction_id == list.selected);
                found.map_or(0, |i| i as i64 + 1)
            }))
        })?,
    )?;

    // SetSelectedFaction(index): that row's faction, a header's included, or none (`0x5d07b0`).
    g.set(
        "SetSelectedFaction",
        lua.create_function(|lua, index: Value| {
            let row = row_arg(lua, index, "Usage: SetSelectedFaction(index)")?;
            with_reputation(|list, _| list.selected = list.faction_at(row));
            Ok(())
        })?,
    )?;

    // Fold verbs (`0x5d1f70`, `0x5d2000`, `0x5d1fc0`, `0x5d1fe0`); the sort raises UPDATE_FACTION.
    // The two by index raise their `Usage:` for a non-number (`0x5d1f8c`, `0x5d201c`).
    for (name, fold, usage) in [
        (
            "CollapseFactionHeader",
            true,
            Some("Usage: CollapseFactionHeader(index)"),
        ),
        (
            "ExpandFactionHeader",
            false,
            Some("Usage: ExpandFactionHeader(index)"),
        ),
        ("CollapseAllFactionHeaders", true, None),
        ("ExpandAllFactionHeaders", false, None),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let row = match usage {
                    Some(usage) => Some(row_arg(lua, index, usage)?),
                    None => None,
                };
                let sorted = with_reputation(|list, t| {
                    list.collapse(t, row.unwrap_or(list.rows.len()), fold)
                });
                if sorted {
                    update_faction(lua);
                }
                Ok(())
            })?,
        )?;
    }

    // FactionToggleAtWar(index) (`0x5d1390`): flips that row's faction's stored war flag.
    g.set(
        "FactionToggleAtWar",
        lua.create_function(|lua, index: Value| {
            let row = row_arg(lua, index, "Usage: FactionToggleAtWar(index)")?;
            let refused = with_reputation(|list, t| {
                let faction = list.faction_at(row);
                if faction == 0 {
                    return None;
                }
                let at_war = t.faction(faction).is_some_and(|e| e.at_war);
                set_at_war(list, t, faction, !at_war)
            });
            if let Some(key) = refused {
                crate::script::ui_errors::show_now(lua, key);
            }
            Ok(())
        })?,
    )?;

    // IsFactionInactive(index) → that row's flag `0x20` (`0x5d0740`).
    g.set(
        "IsFactionInactive",
        lua.create_function(|lua, index: Value| {
            let row = row_arg(lua, index, "Usage: IsFactionInactive(index)")?;
            let inactive = with_reputation(|list, t| {
                t.faction(list.faction_at(row)).is_some_and(|e| e.inactive)
            });
            Ok(binding_abi::flag(inactive))
        })?,
    )?;

    // SetFactionInactive / SetFactionActive(index) (`0x5d1e50`, `0x5d1ee0`).
    for (name, inactive, usage) in [
        (
            "SetFactionInactive",
            true,
            "Usage: SetFactionInactive(index)",
        ),
        ("SetFactionActive", false, "Usage: SetFactionActive(index)"),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let row = row_arg(lua, index, usage)?;
                let sorted = with_reputation(|list, t| {
                    let faction = list.faction_at(row);
                    faction != 0 && set_inactive(list, t, faction, inactive)
                });
                if sorted {
                    update_faction(lua);
                }
                Ok(())
            })?,
        )?;
    }

    // GetWatchedFactionInfo() → name, standingID, min, max, value; nil name for none (`0x5d1240`):
    // the field's slot past 127 names no faction.
    g.set(
        "GetWatchedFactionInfo",
        lua.create_function(|lua, ()| {
            let watched = with_reputation(|_, t| {
                let slot = u32::try_from(t.watched()?).ok().filter(|&s| s <= 127)?;
                t.slot_faction(slot)
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

    // SetWatchedFactionIndex(index) (`0x5d1420`): sends the row's slot, -1 for none, when the
    // field differs, inside the call (`0x5d0ba0`, `0x5d0bfb`).
    g.set(
        "SetWatchedFactionIndex",
        lua.create_function(|lua, index: Value| {
            let row = row_arg(lua, index, "Usage: SetWatchedFactionIndex(index)")?;
            with_reputation(|list, t| {
                let slot = t
                    .faction(list.faction_at(row))
                    .map_or(-1, |e| e.rep_list_id as i32);
                if t.watched().is_some_and(|w| w != slot) {
                    t.send(ReputationSend::Watch(slot));
                }
            });
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::live::TestView;
    use crate::script::UiScript;

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
    /// parentless factions; out of order, with one unmet faction.
    fn rows() -> Vec<FactionEntry> {
        vec![
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
        ]
    }

    /// A VM and the view it is lent: the faction table, a player watching nothing, the list built
    /// as the world entry builds it.
    struct Pane {
        s: UiScript,
        v: TestView,
    }

    impl Pane {
        fn new(entries: Vec<FactionEntry>) -> Self {
            let mut p = Self {
                s: UiScript::new().expect("VM"),
                v: TestView {
                    factions: StaticFactions {
                        entries,
                        watched: Some(-1),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            };
            p.enter_world();
            p
        }

        /// The world entry's build, fold and sort (`0x5d2bc0`), as the app runs it.
        fn enter_world(&mut self) {
            let (list, t) = (&mut self.v.faction_list, &self.v.factions);
            let row = list.enter_world(t);
            list.collapse(t, row, true);
            list.sort(t);
        }

        /// The visible handler's add and sort (`0x5d2050`).
        fn reveal(&mut self, slot: u32) {
            for e in self
                .v
                .factions
                .entries
                .iter_mut()
                .filter(|e| e.rep_list_id == slot)
            {
                e.visible = true;
            }
            let (list, t) = (&mut self.v.faction_list, &self.v.factions);
            list.add_visible(t, slot);
            list.sort(t);
        }

        fn run(&mut self, code: &str) {
            self.s
                .with_live(&mut self.v, |s| s.run(code))
                .unwrap_or_else(|e| panic!("{code}: {e}"));
        }

        fn eval<T: mlua::FromLuaMulti>(&mut self, code: &str) -> T {
            self.s
                .with_live(&mut self.v, |s| s.eval::<T>(code))
                .unwrap_or_else(|e| panic!("{code}: {e}"))
        }

        fn names(&mut self) -> Vec<String> {
            self.eval(
                "local t = {} for i = 1, GetNumFactions() do t[i] = (GetFactionInfo(i)) end return t",
            )
        }

        fn sends(&mut self) -> Vec<ReputationSend> {
            std::mem::take(&mut self.v.factions.sends)
        }

        fn clean(&self) {
            assert!(self.s.errors().is_empty(), "{:#?}", self.s.errors());
        }
    }

    /// Each shown row's tree shape: `isHeader`, `isChild`, `hasRep`, as the stock pane reads them.
    fn shapes(p: &mut Pane) -> Vec<(bool, bool, bool)> {
        p.eval::<Vec<Vec<bool>>>(
            "local t = {} for i = 1, GetNumFactions() do \
               local _,_,_,_,_,_,_,_,h,_,r,_,c = GetFactionInfo(i) \
               t[i] = {h ~= nil, c ~= nil, r ~= nil} end return t",
        )
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
        let mut p = Pane::new(rows());
        assert_eq!(p.names(), TREE);
        let (h, c, r) = (true, true, true);
        assert_eq!(
            shapes(&mut p),
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
        p.clean();
    }

    #[test]
    fn every_path_answers_thirteen_values_and_headers_carry_a_standing() {
        let mut p = Pane::new(rows());
        for call in [
            "GetFactionInfo(1)",
            "GetFactionInfo(3)",
            "GetFactionInfo(11)",
            "GetFactionInfo(99)",
            "GetFactionInfoByID(1090)",
            "GetFactionInfoByID(424242)",
        ] {
            let n: i64 = p.eval(&format!("return select('#', {call})"));
            assert_eq!(n, 13, "{call}");
        }
        // Every row's standingID indexes `FACTION_BAR_COLORS`, which the stock pane does unguarded.
        let ids: Vec<i64> = p.eval(
            "local t = {} for i = 1, GetNumFactions() do \
               local _,_,s = GetFactionInfo(i) t[i] = s end return t",
        );
        assert!(ids.iter().all(|s| (1..=8).contains(s)), "{ids:?}");
        assert_eq!(ids[0], 4, "a header faction reports its own rank");
        assert_eq!(ids[8], 7, "and a header bar its own");
        assert_eq!(ids[10], 1, "the synthetic header reports 1");

        let (name, sid, header): (Value, i64, Value) =
            p.eval("local n,_,s,_,_,_,_,_,h = GetFactionInfoByID(424242) return n, s, h");
        assert!(name.is_nil() && header.is_nil());
        assert_eq!(sid, 1, "the miss's standingID is 1 too");
        // Past the list the reference asks for faction 0, the "Other" header.
        assert_eq!(p.eval::<String>("return (GetFactionInfo(99))"), "Other");
        p.clean();
    }

    #[test]
    fn an_unmet_faction_answers_by_id_but_is_no_row() {
        let mut p = Pane::new(rows());
        let (name, child): (String, Value) =
            p.eval("local n,_,_,_,_,_,_,_,_,_,_,_,c = GetFactionInfoByID(1090) return n, c");
        assert_eq!(name, "Kirin Tor");
        assert!(child.is_nil(), "isChild asks the shown rows");
        assert!(!p.names().contains(&"Kirin Tor".to_string()));
    }

    /// The bindings read the table at the call: a standing the store moves answers at once, with
    /// no push between.
    #[test]
    fn a_standing_written_between_calls_answers_at_the_next_call() {
        let mut p = Pane::new(rows());
        let read = |p: &mut Pane| p.eval::<i64>("local _,_,_,_,_,v = GetFactionInfo(4) return v");
        assert_eq!(read(&mut p), 4000);
        p.v.factions.entries[0].standing = 5100; // Stormwind
        assert_eq!(read(&mut p), 5100);
    }

    #[test]
    fn folding_an_expansion_hides_its_sub_headers_and_keeps_their_own_folds() {
        let mut p = Pane::new(rows());
        p.run("CollapseFactionHeader(2)"); // Alliance
        assert_eq!(p.names().len(), 11, "its two factions leave");
        p.run("CollapseFactionHeader(1)"); // Classic
        assert_eq!(
            p.names(),
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
        p.run("ExpandFactionHeader(1)");
        assert_eq!(p.names().len(), 11, "Alliance comes back still folded");
        let folded: Value = p.eval("local _,_,_,_,_,_,_,_,_,c = GetFactionInfo(2) return c");
        assert_eq!(folded, Value::Integer(1));
        p.run("ExpandFactionHeader(2)");
        assert_eq!(p.names(), TREE);
        p.clean();
    }

    #[test]
    fn the_all_headers_verbs_and_a_non_header_index_fold_every_header() {
        let mut p = Pane::new(rows());
        p.run("CollapseAllFactionHeaders()");
        assert_eq!(
            p.names(),
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
        p.run("ExpandAllFactionHeaders()");
        assert_eq!(p.names(), TREE);
        p.run("CollapseFactionHeader(3)"); // Ironforge, not a header
        assert_eq!(p.names().len(), 6);
        p.run("ExpandFactionHeader(0)");
        assert_eq!(p.names(), TREE);
        p.clean();
    }

    #[test]
    fn folds_survive_a_sort_and_a_new_header_unfolds_them() {
        let mut p = Pane::new(rows());
        p.run("CollapseFactionHeader(2)"); // Alliance
        p.v.factions.entries[0].standing = 5000; // Stormwind ticks: the standing handler sorts
        let (list, t) = (&mut p.v.faction_list, &p.v.factions);
        list.sort(t);
        assert_eq!(p.names().len(), 11, "still folded");

        // Kirin Tor turns visible under a header that exists: the folds stand.
        p.reveal(99);
        assert_eq!(p.names().len(), 12);
        assert!(!p.names().contains(&"Ironforge".to_string()));

        // A faction under a new header adds it, which unfolds every header (`0x5cfe70`).
        p.v.factions
            .entries
            .push(header(980, 43, 0, "The Burning Crusade"));
        p.v.factions.entries.push(entry(932, 50, 980, "The Aldor"));
        p.reveal(50);
        let got = p.names();
        assert!(got.contains(&"Ironforge".to_string()), "{got:?}");
        assert_eq!(&got[6..8], ["The Burning Crusade", "The Aldor"]);
        p.clean();
    }

    /// The world entry rebuilds the list and drops every fold, keeping the selection (`0x5d2bc0`).
    #[test]
    fn the_world_entry_rebuilds_the_folds_and_keeps_the_selection() {
        let mut p = Pane::new(rows());
        p.run("CollapseFactionHeader(2) SetSelectedFaction(3)"); // Alliance; Steamwheedle Cartel
        assert_eq!(p.names().len(), 11);
        p.enter_world();
        assert_eq!(p.names(), TREE);
        assert_eq!(p.eval::<i64>("return GetSelectedFaction()"), 5);
    }

    #[test]
    fn an_empty_header_is_hidden_unless_it_is_a_bar() {
        let mut entries = rows();
        entries.push(FactionEntry {
            visible: true,
            ..header(980, 43, 0, "The Burning Crusade")
        });
        entries[4].visible = false; // Valiance Expedition: Alliance Vanguard keeps no member
        let mut p = Pane::new(entries);
        let got = p.names();
        assert!(!got.contains(&"The Burning Crusade".to_string()), "{got:?}");
        assert_eq!(
            &got[6..9],
            [
                "Wrath of the Lich King",
                "Argent Crusade",
                "Alliance Vanguard"
            ]
        );
        p.clean();
    }

    #[test]
    fn an_inactive_faction_files_under_the_folded_inactive_header() {
        let mut entries = rows();
        entries[1].inactive = true; // Argent Dawn
        let mut p = Pane::new(entries);
        let got = p.names();
        assert_eq!(&got[10..], ["Other", "Bloodsail Buccaneers", "Inactive"]);
        p.run("ExpandFactionHeader(13)");
        assert_eq!(p.names().last().unwrap(), "Argent Dawn");

        // The toggle writes the flag, sends, re-sorts and repaints inside the call.
        p.run(
            "UPDATES = 0 local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_FACTION') \
             f:SetScript('OnEvent', function() UPDATES = UPDATES + 1 \
               SEEN = IsFactionInactive(GetNumFactions()) end)",
        );
        p.run("SetFactionInactive(4)"); // Stormwind
        assert_eq!(p.eval::<i64>("return UPDATES"), 1);
        assert_eq!(p.eval::<Value>("return SEEN"), Value::Integer(1));
        assert!(p.v.factions.entries[0].inactive, "the store's flag");
        let got = p.names();
        assert_eq!(
            &got[got.len() - 3..],
            ["Inactive", "Argent Dawn", "Stormwind"]
        );
        assert_eq!(
            p.sends(),
            [ReputationSend::Inactive {
                rep_list_id: 19,
                inactive: true
            }]
        );
        p.clean();
    }

    #[test]
    fn the_selection_is_a_faction_and_a_header_can_hold_it() {
        let mut p = Pane::new(rows());
        p.run("SetSelectedFaction(4)"); // Stormwind
        assert_eq!(p.eval::<i64>("return GetSelectedFaction()"), 4);
        p.run("CollapseFactionHeader(2)");
        assert_eq!(p.eval::<i64>("return GetSelectedFaction()"), 0);
        p.run("ExpandFactionHeader(2)");
        assert_eq!(p.eval::<i64>("return GetSelectedFaction()"), 4);
        p.run("SetSelectedFaction(9)"); // Alliance Vanguard
        assert_eq!(p.eval::<i64>("return GetSelectedFaction()"), 9);
        // None is faction 0, which the "Other" header's row also carries.
        p.run("SetSelectedFaction(99)");
        assert_eq!(p.eval::<i64>("return GetSelectedFaction()"), 11);
        p.clean();
    }

    #[test]
    fn the_war_toggle_follows_set_at_war() {
        let mut entries = rows();
        entries[8].peace_forced = true; // Ironforge
        let mut p = Pane::new(entries);
        let at_war = |p: &mut Pane, i: usize| {
            !p.eval::<Value>(&format!(
                "local _,_,_,_,_,_,w = GetFactionInfo({i}) return w"
            ))
            .is_nil()
        };

        p.run("FactionToggleAtWar(4)"); // Stormwind
        assert!(at_war(&mut p, 4), "written in the store in the call");
        p.run("FactionToggleAtWar(3)"); // Ironforge: sent, but peace holds
        assert!(!at_war(&mut p, 3));
        assert_eq!(
            p.sends(),
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
        p.v.factions.in_combat = true;
        p.run("FactionToggleAtWar(4)");
        assert!(at_war(&mut p, 4));
        assert_eq!(p.s.take_shown(), ["ERR_NOT_IN_COMBAT"]);
        p.v.factions.in_combat = false;

        // So is peace below -3000.
        p.v.factions.entries[0].standing = -5000;
        p.run("FactionToggleAtWar(4)");
        assert!(at_war(&mut p, 4));
        assert!(p.sends().is_empty());

        // A header past the first slot refuses; the first does not.
        p.run("FactionToggleAtWar(2)"); // Alliance
        assert!(p.sends().is_empty());
        p.run("FactionToggleAtWar(1)"); // Classic
        assert_eq!(
            p.sends(),
            [ReputationSend::AtWar {
                rep_list_id: 96,
                at_war: true
            }]
        );
        p.clean();
    }

    #[test]
    fn watching_sends_only_a_change_and_never_moves_the_bar_itself() {
        let mut p = Pane::new(rows());
        assert!(p.eval::<Value>("return GetWatchedFactionInfo()").is_nil());
        p.run("SetWatchedFactionIndex(4)"); // Stormwind
        assert_eq!(p.sends(), [ReputationSend::Watch(19)]);
        assert!(
            p.eval::<Value>("return GetWatchedFactionInfo()").is_nil(),
            "the server owns the field"
        );
        p.run("SetWatchedFactionIndex(0) SetWatchedFactionIndex(11)");
        assert!(
            p.sends().is_empty(),
            "none, and the synthetic header, are already what is watched"
        );

        p.v.factions.watched = Some(19);
        assert_eq!(
            p.eval::<(String, i64, i64, i64, i64)>("return GetWatchedFactionInfo()"),
            ("Stormwind".into(), 5, 3000, 9000, 4000)
        );
        assert_eq!(
            p.eval::<Value>("local _,_,_,_,_,_,_,_,_,_,_,w = GetFactionInfo(4) return w"),
            Value::Integer(1)
        );
        p.run("SetWatchedFactionIndex(4)");
        assert!(p.sends().is_empty(), "already watched");
        p.run("SetWatchedFactionIndex(9) SetWatchedFactionIndex(0)"); // Alliance Vanguard, then none
        assert_eq!(
            p.sends(),
            [ReputationSend::Watch(88), ReputationSend::Watch(-1)]
        );
        // Slot 0 is a faction, not "none".
        p.run("SetWatchedFactionIndex(13)"); // Bloodsail Buccaneers
        assert_eq!(p.sends(), [ReputationSend::Watch(0)]);

        // Without an active player nothing is sent (`0x5d0bfb`) and nothing is watched.
        p.v.factions.watched = None;
        p.run("SetWatchedFactionIndex(4)");
        assert!(p.sends().is_empty());
        assert!(p.eval::<Value>("return GetWatchedFactionInfo()").is_nil());
        p.clean();
    }

    /// With no view lent the list is empty: the fold verbs still sort and raise `UPDATE_FACTION`
    /// (`0x5d1e3a`).
    #[test]
    fn with_no_view_the_list_is_empty_and_the_fold_still_repaints() {
        let s = UiScript::new().expect("VM");
        s.run(
            "N = 0 local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_FACTION') \
             f:SetScript('OnEvent', function() N = N + 1 end) ExpandAllFactionHeaders()",
        )
        .unwrap();
        assert_eq!(s.eval::<i64>("return N").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetNumFactions()").unwrap(), 0);
    }

    /// Each index verb raises its own `Usage:` for a non-number (`0x5d0756`, `0x5d07c6`,
    /// `0x5d1169`, `0x5d13a9`, `0x5d1436`, `0x5d1e69`, `0x5d1ef9`, `0x5d1f86`, `0x5d2016`) and
    /// takes a numeric string; 0 or a negative index wraps past the list: faction 0 read, nothing
    /// sent, and a header fold opens or folds every header (`0x5d1d2c`).
    #[test]
    fn the_index_verbs_refuse_a_non_number_and_read_a_negative_past_the_list() {
        let mut p = Pane::new(rows());
        for verb in [
            "IsFactionInactive",
            "SetSelectedFaction",
            "GetFactionInfo",
            "FactionToggleAtWar",
            "SetWatchedFactionIndex",
            "SetFactionInactive",
            "SetFactionActive",
            "CollapseFactionHeader",
            "ExpandFactionHeader",
        ] {
            for arg in ["", "nil", "'x'", "{}"] {
                let err =
                    p.s.with_live(&mut p.v, |s| s.run(&format!("{verb}({arg})")))
                        .expect_err(verb)
                        .to_string();
                assert!(
                    err.contains(&format!("Usage: {verb}(index)")),
                    "{verb}({arg}): {err}"
                );
            }
        }
        assert_eq!(p.eval::<String>("return (GetFactionInfo('1'))"), "Classic");
        let shown: i64 = p.eval("return GetNumFactions()");
        for index in ["0", "-1", "-5.5"] {
            assert_eq!(
                p.eval::<String>(&format!("return (GetFactionInfo({index}))")),
                "Other",
                "past the list, faction 0"
            );
            p.run(&format!(
                "SetWatchedFactionIndex({index}) FactionToggleAtWar({index}) \
                 SetFactionInactive({index}) SetSelectedFaction({index})"
            ));
            assert!(p.sends().is_empty(), "{index}");
            // The selection is faction 0, the "Other" header's row (`0x5d07f4`, `0x5d01d3`).
            let other: i64 = p.eval(
                "for i = 1, GetNumFactions() do \
                   if GetFactionInfo(i) == 'Other' then return i end end return 0",
            );
            assert_eq!(p.eval::<i64>("return GetSelectedFaction()"), other);
        }
        p.run("CollapseFactionHeader(-1)");
        let folded: i64 = p.eval("return GetNumFactions()");
        p.run("ExpandFactionHeader(0)");
        let opened: i64 = p.eval("return GetNumFactions()");
        assert!(
            folded < shown && opened == shown,
            "{folded} {shown} {opened}"
        );
        p.run("CollapseAllFactionHeaders()");
        assert_eq!(folded, p.eval::<i64>("return GetNumFactions()"));
        p.run("ExpandAllFactionHeaders()");
        assert_eq!(opened, p.eval::<i64>("return GetNumFactions()"));
    }
}
