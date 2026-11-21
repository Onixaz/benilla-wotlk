//! The objective tracker's engine side: the quest watch list (`AddQuestWatch` `0x5e64d0`, at most
//! 25, timed or not, sorted by the `trackerSorting` CVar and kept in `trackedQuests`), the quest
//! POI store the tracker and the world map count (`0x5e5a50`, 25 slots fed by
//! `SMSG_QUEST_POI_QUERY_RESPONSE`), and the verbs the stock `WatchFrame.lua` reads per watched
//! quest: the special item, the completion text and the link.

use mlua::{Lua, MultiValue, ObjectLike, Table, Value};

use super::binding_abi::{flag, number_arg};

use super::worldmap::QuestPoiPlace;
use super::Model;

/// The watch list holds 25 (`0x5e6525`, `cmp 0x19`).
const MAX_WATCHES: usize = 25;
/// The quest POI list holds 25 quests (`0x5e7020`).
const POI_SLOTS: usize = 25;

/// One watch: the quest and, for a progress watch, its expiry on the `GetTime` clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct QuestWatch {
    pub(crate) quest_id: u32,
    /// `time() + n` for `AddQuestWatch(index, n)`, 0 for an untimed watch.
    pub(crate) expire_at: f64,
    /// The proximity sort's key (`0x5e5c10`).
    distance: (i32, f32),
}

/// The special item behind a quest's tracker button (`0x5e0da0`), as the live view finds it in the bags at call time: the template's source item, else with flag `0x20000` a drop item, either one with an on-use spell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestSpecialItemView {
    /// The Lua bag and 1-based slot the item sits in.
    pub bag: i64,
    pub slot: u32,
    /// `ITEM_FIELD_SPELL_CHARGES[0]` (`0x5e539d`).
    pub charges: i32,
    /// The on-use spell, the first with trigger 0, the range verb checks (`0x5e5529`).
    pub spell_id: u32,
    /// The instance's hyperlink (`0x5e533d`) and inventory art (`0x5e536a`).
    pub link: String,
    pub texture: String,
}

/// One region of a quest's POI, as the `SMSG_QUEST_POI_QUERY_RESPONSE` handler keeps it
/// (`0x5e7370`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestPoiRegion {
    /// The leaderboard objective it serves, -1 for the turn-in.
    pub objective: i32,
    pub map: u32,
    pub area: u32,
    pub floor: i32,
    pub flags: u32,
    /// The outline's mean point in world yards, integer-divided as the reference sums it.
    pub center: (i32, i32),
    /// The outline in world yards; a one-point region's icon is never pushed aside (`0x5e3165`).
    pub outline: Vec<(i32, i32)>,
    /// The icon placement's priority, the lowest winning (`0x5e2eb0`).
    pub priority: i32,
}

/// One quest's POI as the response delivers it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestPoiView {
    pub quest_id: u32,
    pub regions: Vec<QuestPoiRegion>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PoiSlot {
    pub(super) quest_id: u32,
    pub(super) regions: Vec<QuestPoiRegion>,
    /// `+0x28`: a region shows on the displayed map, set by the last count.
    pub(super) shown: bool,
    /// The icon `QuestPOIUpdateIcons` placed, `None` for none (`0x5e2eb0`).
    pub(super) icon: Option<super::quest_poi::PoiIcon>,
}

/// The tracker's model.
#[derive(Debug, Default)]
pub(crate) struct QuestWatchState {
    pub(crate) watches: Vec<QuestWatch>,
    /// Whether the tracked quests loaded: the CVar is read once, on the first log that holds a
    /// quest.
    loaded: bool,
    pub(super) pois: Vec<PoiSlot>,
}

impl QuestWatchState {
    pub(super) fn poi(&self, quest_id: u32) -> Option<&PoiSlot> {
        self.pois.iter().find(|p| p.quest_id == quest_id)
    }
}

/// The 1-based log row of a quest, a folded one included, 0 when it is not in the log
/// (`0x5deeb0`).
fn quest_index(model: &Model, quest_id: u32) -> usize {
    model
        .quest_log
        .iter()
        .position(|e| !e.is_header && e.quest_id == quest_id)
        .map_or(0, |i| i + 1)
}

/// The quest row at a 1-based index, headers answering none.
fn quest_at(model: &Model, index: i64) -> Option<&super::quest_log::QuestLogRow> {
    usize::try_from(index - 1)
        .ok()
        .and_then(|i| model.quest_log.get(i))
        .filter(|e| !e.is_header && e.quest_id != 0)
}

/// [`quest_index`] for the blob hover (`0x58e9c0`).
pub(super) fn quest_log_index_of(model: &Model, quest_id: u32) -> i64 {
    quest_index(model, quest_id) as i64
}

/// A quest in the log, a folded one included, as `AddQuestWatch` searches (`0x5e64f8`).
fn in_log(model: &Model, quest_id: u32) -> bool {
    super::quest_log::quest_row_of(model, quest_id).is_some()
}

/// The quest a watch verb names: the row at the numeric argument, else the selection (`0x5e67c0`).
fn named_quest(lua: &Lua, model: &Model, v: Value) -> mlua::Result<u32> {
    Ok(match lua.coerce_number(v)? {
        Some(n) => quest_at(model, n as i64).map_or(0, |e| e.quest_id),
        None => quest_at(model, i64::from(model.quest_log_selection)).map_or(0, |e| e.quest_id),
    })
}

/// The watch add behind `AddQuestWatch` (`0x5e64d0`): a quest in the log, re-timed if already
/// watched with a timer, else appended while fewer than 25 are watched and the list re-sorted.
pub(crate) fn add_watch(model: &mut Model, quest_id: u32, secs: i64) {
    if quest_id == 0 || !in_log(model, quest_id) {
        return;
    }
    let expire = if secs != 0 {
        model.now + secs as f64
    } else {
        0.0
    };
    let state = &mut model.quest_watch;
    if let Some(w) = state.watches.iter_mut().find(|w| w.quest_id == quest_id) {
        if w.expire_at > 0.0 {
            w.expire_at = expire;
        }
    } else if state.watches.len() < MAX_WATCHES {
        state.watches.push(QuestWatch {
            quest_id,
            expire_at: expire,
            distance: (0, 0.0),
        });
        sort_watches(model);
    }
    save(model);
}

fn remove_watch(model: &mut Model, quest_id: u32) -> bool {
    let before = model.quest_watch.watches.len();
    model.quest_watch.watches.retain(|w| w.quest_id != quest_id);
    let removed = model.quest_watch.watches.len() != before;
    if removed {
        save(model);
    }
    removed
}

/// The tracked-quests save (`0x5e0240`): the untimed watches. The reference saves at logout; every
/// change saves here, which leaves the stored value the same.
fn save(model: &mut Model) {
    let ids: Vec<u32> = model
        .quest_watch
        .watches
        .iter()
        .filter(|w| w.expire_at == 0.0)
        .map(|w| w.quest_id)
        .collect();
    let value = super::achievement::encode_tracked(&ids);
    super::cvars::set_from_engine(model, "trackedQuests", value);
}

pub(super) fn cvar_int(model: &Model, name: &str) -> i64 {
    model
        .cvars
        .get(name)
        .and_then(|c| c.value.trim().parse().ok())
        .unwrap_or(0)
}

/// A float CVar by lowercased name, 0 when unset.
pub(super) fn cvar_float(model: &Model, name: &str) -> f32 {
    model
        .cvars
        .get(name)
        .and_then(|c| c.value.trim().parse().ok())
        .unwrap_or(0.0)
}

/// The watch sort behind `SortQuestWatches` (`0x5e5c10`): by `trackerSorting`, 0 manual
/// (untouched), 1 by POI distance, 2 hardest first, 3 easiest first, ties and any other value by
/// quest id. True when the order changed.
pub(crate) fn sort_watches(model: &mut Model) -> bool {
    let sort = cvar_int(model, "trackersorting");
    if sort == 0 || model.quest_watch.watches.len() <= 1 {
        return false;
    }
    if sort == 1 {
        let distances: Vec<(i32, f32)> = model
            .quest_watch
            .watches
            .iter()
            .map(|w| poi_distance(model, w.quest_id))
            .collect();
        for (w, d) in model.quest_watch.watches.iter_mut().zip(&distances) {
            w.distance = *d;
        }
        if distances.windows(2).all(|p| p[0].1 <= p[1].1) {
            return false;
        }
    }
    let levels: std::collections::HashMap<u32, i32> = model
        .quest_watch
        .watches
        .iter()
        .map(|w| {
            let level = super::quest_log::quest_numbers(model, w.quest_id);
            (w.quest_id, level.map_or(0, |e| e.level as i32))
        })
        .collect();
    let level = |id: u32| levels.get(&id).copied().unwrap_or(0);
    let before: Vec<u32> = model
        .quest_watch
        .watches
        .iter()
        .map(|w| w.quest_id)
        .collect();
    let mut watches = model.quest_watch.watches.clone();
    match sort {
        1 => watches.sort_by(|a, b| a.distance.1.total_cmp(&b.distance.1)),
        2 | 3 => watches.sort_by(|a, b| {
            let (la, lb) = (level(a.quest_id), level(b.quest_id));
            let by_level = if sort == 3 { la.cmp(&lb) } else { lb.cmp(&la) };
            by_level.then(a.quest_id.cmp(&b.quest_id))
        }),
        _ => watches.sort_by_key(|w| w.quest_id),
    }
    let changed = watches
        .iter()
        .map(|w| w.quest_id)
        .ne(before.iter().copied());
    model.quest_watch.watches = watches;
    sort != 1 || changed
}

/// The quest's POI distance (`0x5e3b90`): the nearest region on the player's map serving an open
/// objective, by floor delta then distance; `f32::MAX` with no POI.
fn poi_distance(model: &Model, quest_id: u32) -> (i32, f32) {
    let Some(poi) = model.quest_watch.poi(quest_id) else {
        return (i32::MAX, f32::MAX);
    };
    let mask = objective_mask(model, quest_id);
    let mut best = (i32::MAX, f32::MAX);
    for r in poi.regions.iter().filter(|r| serves(r, mask)) {
        if let Some(d) = model.worldmap.quest_poi_distance(&place(r)) {
            if d.0 < best.0 || (d.0 == best.0 && d.1 < best.1) {
                best = d;
            }
        }
    }
    best
}

/// The quest's objective mask (`0x5e2950`), which the app computes per row.
pub(super) fn objective_mask(model: &Model, quest_id: u32) -> u32 {
    super::quest_log::quest_numbers(model, quest_id).map_or(0, |e| e.poi_objectives)
}

/// A region counts for a complete quest (mask all ones) only as the turn-in, else when it serves
/// an objective still open (`0x5e5b46`-`0x5e5b7b`).
pub(super) fn serves(r: &QuestPoiRegion, mask: u32) -> bool {
    if mask == u32::MAX {
        r.objective == -1
    } else {
        r.objective != -1 && (0..32).contains(&r.objective) && mask & (1 << r.objective) != 0
    }
}

pub(super) fn place(r: &QuestPoiRegion) -> QuestPoiPlace {
    QuestPoiPlace {
        map: r.map,
        area: r.area,
        floor: r.floor,
        flags: r.flags,
        x: r.center.0 as f32,
        y: r.center.1 as f32,
    }
}

/// The POI update (`0x5e5a50`): mark each slot shown when one of its counting regions is on
/// the displayed map, and count them; 0 with the `questPOI` CVar off or no area displayed.
fn update_all_pois(model: &mut Model) -> i64 {
    if cvar_int(model, "questpoi") == 0 {
        return 0;
    }
    let mut count = 0;
    let marks: Vec<Option<bool>> = model
        .quest_watch
        .pois
        .iter()
        .map(|slot| {
            let mask = objective_mask(model, slot.quest_id);
            let mut shown = Some(false);
            for r in slot.regions.iter().filter(|r| serves(r, mask)) {
                match model.worldmap.quest_poi_shown(&place(r)) {
                    None => return None,
                    Some(true) => {
                        shown = Some(true);
                        break;
                    }
                    Some(false) => {}
                }
            }
            shown
        })
        .collect();
    if marks.iter().any(Option::is_none) {
        return 0;
    }
    for (slot, shown) in model.quest_watch.pois.iter_mut().zip(marks) {
        slot.shown = shown == Some(true);
        count += i64::from(slot.shown);
    }
    count
}

/// The POI quest by visible index (`0x5e3840`), 0-based: the shown complete or objective-less
/// quests in slot order, then the watched open ones in watch order, then the rest.
fn poi_by_visible_index(model: &Model, index: usize) -> Option<u32> {
    let pois = &model.quest_watch.pois;
    let settled = |id: u32| {
        super::quest_log::quest_numbers(model, id).is_some_and(|e| super::quest_log::settled(&e))
    };
    let shown = |id: u32| pois.iter().any(|p| p.quest_id == id && p.shown);
    let watched = |id: u32| model.quest_watch.watches.iter().any(|w| w.quest_id == id);
    pois.iter()
        .filter(|p| p.shown && settled(p.quest_id))
        .map(|p| p.quest_id)
        .chain(
            model
                .quest_watch
                .watches
                .iter()
                .map(|w| w.quest_id)
                .filter(|&id| !settled(id) && shown(id)),
        )
        .chain(
            pois.iter()
                .filter(|p| p.shown && !settled(p.quest_id) && !watched(p.quest_id))
                .map(|p| p.quest_id),
        )
        .nth(index)
}

/// `GetQuestLogCompletionText`'s row (`0x5e06d0`): a numeric index, else the selection.
fn completion_row(lua: &Lua, model: &Model, v: Value) -> mlua::Result<Option<usize>> {
    let index = match lua.coerce_number(v)? {
        Some(n) => n as i64,
        None => i64::from(model.quest_log_selection),
    };
    Ok(quest_at(model, index).map(|_| (index - 1) as usize))
}

impl super::UiScript {
    /// The watched quest ids, in watch order.
    pub fn quest_log_watched(&self) -> Vec<u32> {
        self.model_ref()
            .quest_watch
            .watches
            .iter()
            .map(|w| w.quest_id)
            .collect()
    }

    /// A quest's 1-based log row, a folded one included, 0 when it is not in the log
    /// (`0x5deeb0`): the `QUEST_ACCEPTED` and `QUEST_WATCH_UPDATE` argument.
    pub fn quest_log_index(&self, quest_id: u32) -> usize {
        quest_index(&self.model_ref(), quest_id)
    }

    /// Whether the POI store holds a response for `quest_id`.
    pub fn quest_poi_held(&self, quest_id: u32) -> bool {
        self.model_ref().quest_watch.poi(quest_id).is_some()
    }

    /// `SMSG_QUEST_POI_QUERY_RESPONSE` (`0x5e7370`): each quest into its slot or the first slot
    /// whose quest left the log (`0x5e7020`); the handler queues `QUEST_POI_UPDATE` beside it.
    pub fn apply_quest_pois(&mut self, pois: Vec<QuestPoiView>) {
        {
            let mut model = self.model_mut();
            let model = &mut *model;
            for poi in pois {
                if poi.quest_id == 0 || model.quest_watch.poi(poi.quest_id).is_some() {
                    continue;
                }
                let held: Vec<bool> = model
                    .quest_watch
                    .pois
                    .iter()
                    .map(|p| p.quest_id != 0 && in_log(model, p.quest_id))
                    .collect();
                let slots = &mut model.quest_watch.pois;
                let at = match held.iter().position(|h| !h) {
                    Some(i) => i,
                    None if slots.len() < POI_SLOTS => {
                        slots.push(PoiSlot::default());
                        slots.len() - 1
                    }
                    None => continue,
                };
                slots[at] = PoiSlot {
                    quest_id: poi.quest_id,
                    regions: poi.regions,
                    shown: false,
                    icon: None,
                };
            }
        }
    }

    /// The watch-timer update (`0x5dfef0`), which the game UI's idle runs ahead of the unique signals' flush (`0x52b229`): drop each timed watch past its expiry, queueing the log's update (`0x5dff1d`).
    pub fn update_quest_watch_timers(&mut self) {
        let mut model = self.model_mut();
        let now = model.now;
        let due: Vec<u32> = model
            .quest_watch
            .watches
            .iter()
            .filter(|w| w.expire_at > 0.0 && now >= w.expire_at)
            .map(|w| w.quest_id)
            .collect();
        for id in &due {
            remove_watch(&mut model, *id);
        }
        if !due.is_empty() {
            super::live::unique_signal(&mut model, "QUEST_LOG_UPDATE");
        }
    }
}

/// After a log write: load the saved watches once the log holds a quest (`0x5e6c2b`), and prune the watches of quests that left it (`0x5e6c9e`).
pub(crate) fn on_quest_log(model: &mut Model) {
    let has_quest = model.quest_log.iter().any(|e| !e.is_header);
    if !model.quest_watch.loaded && has_quest {
        model.quest_watch.loaded = true;
        let saved = model
            .cvars
            .get("trackedquests")
            .map(|c| super::achievement::decode_tracked(&c.value))
            .unwrap_or_default();
        for id in saved {
            add_watch(model, id, 0);
        }
    }
    let gone: Vec<u32> = model
        .quest_watch
        .watches
        .iter()
        .map(|w| w.quest_id)
        .filter(|&id| !in_log(model, id))
        .collect();
    for id in gone {
        remove_watch(model, id);
    }
}

/// The rebuild's POI asks (`0x5e6aab`-`0x5e6abe`): every logged quest the store holds no POI for (`0x5e28b0`), in log-slot order, asked again on every rebuild.
pub(crate) fn rebuild_poi_asks(model: &Model) -> Vec<u32> {
    let mut asks: Vec<(u8, u32)> = model
        .quest_log
        .iter()
        .filter(|e| !e.is_header && e.quest_id != 0)
        .filter(|e| model.quest_watch.poi(e.quest_id).is_none())
        .map(|e| (e.slot, e.quest_id))
        .collect();
    asks.sort_unstable();
    asks.into_iter().map(|(_, id)| id).collect()
}

fn model_ref(lua: &Lua) -> mlua::AppDataRef<'_, super::ModelCell> {
    lua.app_data_ref::<super::ModelCell>()
        .expect("model app_data")
}

fn text(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(s)?))
}

/// Register the tracker's globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumQuestWatches() (`0x5df5d0`).
    g.set(
        "GetNumQuestWatches",
        lua.create_function(|lua, ()| Ok(model_ref(lua).quest_watch.watches.len() as i64))?,
    )?;

    // IsQuestWatched(index) (`0x5df610`) → 1 or nil.
    g.set(
        "IsQuestWatched",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(lua, v, "Usage: IsQuestWatched(index)")?;
            let model = model_ref(lua);
            let watched = quest_at(&model, i64::from(index)).is_some_and(|e| {
                model
                    .quest_watch
                    .watches
                    .iter()
                    .any(|w| w.quest_id == e.quest_id)
            });
            Ok(flag(watched))
        })?,
    )?;

    // AddQuestWatch([index [, seconds]]) (`0x5e67c0`): the row, else the selection; a number of
    // seconds makes the watch expire.
    g.set(
        "AddQuestWatch",
        lua.create_function(|lua, (v, secs): (Value, Value)| {
            let secs = lua.coerce_number(secs)?.map_or(0, |n| n as i64);
            let mut model = super::model_mut_of(lua);
            let id = named_quest(lua, &model, v)?;
            add_watch(&mut model, id, secs);
            Ok(())
        })?,
    )?;

    // RemoveQuestWatch([index]) (`0x5df700`): the row, else the selection.
    g.set(
        "RemoveQuestWatch",
        lua.create_function(|lua, v: Value| {
            let mut model = super::model_mut_of(lua);
            let id = named_quest(lua, &model, v)?;
            if id != 0 {
                remove_watch(&mut model, id);
            }
            Ok(())
        })?,
    )?;

    // GetQuestIndexForWatch(watchIndex) (`0x5df780`) → the watched quest's log row, else nil.
    g.set(
        "GetQuestIndexForWatch",
        lua.create_function(|lua, v: Value| {
            let w = number_arg(lua, v, "Usage: GetQuestIndexForWatch(index)")?;
            let model = model_ref(lua);
            let index = usize::try_from(w - 1)
                .ok()
                .and_then(|i| model.quest_watch.watches.get(i))
                .map_or(0, |w| quest_index(&model, w.quest_id));
            Ok(match index {
                0 => Value::Nil,
                i => Value::Integer(i as i64),
            })
        })?,
    )?;

    // GetQuestWatchIndex(questIndex) (`0x5dfc40`) → the row's place in the watch list, else none.
    g.set(
        "GetQuestWatchIndex",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(lua, v, "Usage: GetQuestWatchIndex(index)")?;
            let model = model_ref(lua);
            let at = quest_at(&model, i64::from(index)).and_then(|e| {
                model
                    .quest_watch
                    .watches
                    .iter()
                    .position(|w| w.quest_id == e.quest_id)
            });
            Ok(match at {
                Some(i) => MultiValue::from_vec(vec![Value::Integer(i as i64 + 1)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // ShiftQuestWatches(from, to) (`0x5dfba0` → `0x5def60`): move one watch, the rest sliding.
    g.set(
        "ShiftQuestWatches",
        lua.create_function(|lua, (a, b): (Value, Value)| {
            const USAGE: &str = "Usage: ShiftQuestWatches(indexFrom, indexTo)";
            let from = number_arg(lua, a, USAGE)? - 1;
            let to = number_arg(lua, b, USAGE)? - 1;
            let mut model = super::model_mut_of(lua);
            let n = model.quest_watch.watches.len() as i32;
            let ok = (to > from && to < n) || (to < from && from < n);
            if ok && from >= 0 && to >= 0 {
                let w = model.quest_watch.watches.remove(from as usize);
                model.quest_watch.watches.insert(to as usize, w);
                save(&mut model);
            }
            Ok(())
        })?,
    )?;

    // SortQuestWatches() (`0x5e64b0`) → whether the order changed.
    g.set(
        "SortQuestWatches",
        lua.create_function(|lua, ()| {
            let mut model = super::model_mut_of(lua);
            let changed = sort_watches(&mut model);
            if changed {
                save(&mut model);
            }
            Ok(changed)
        })?,
    )?;

    // GetQuestSortIndex(questIndex) (`0x5dfa10`) → the row's header position, 1-based; 0 off the
    // list or with no header above it.
    g.set(
        "GetQuestSortIndex",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(lua, v, "Usage: GetQuestSortIndex(questIndex)")?;
            let model = model_ref(lua);
            let entries = &model.quest_log;
            let Some(row) = usize::try_from(index - 1)
                .ok()
                .filter(|&i| i < entries.len())
            else {
                return Ok(0);
            };
            Ok(entries[..=row].iter().filter(|e| e.is_header).count() as i64)
        })?,
    )?;

    // GetQuestLogCompletionText([index]) (`0x5e06d0`) → the parsed end text, nothing for a header.
    g.set(
        "GetQuestLogCompletionText",
        lua.create_function(|lua, v: Value| {
            let model = model_ref(lua);
            let Some(row) = completion_row(lua, &model, v)? else {
                return Ok(MultiValue::new());
            };
            let entry = super::quest_log::entry_at(lua, &model, row).unwrap_or_default();
            let text_value = text(lua, &entry.completion_text)?;
            Ok(MultiValue::from_vec(vec![text_value]))
        })?,
    )?;

    // GetQuestLink(index) (`0x5e51d0`) → `|Hquest:id:level|h[title]|h` in the difficulty colour
    // for the quest's level, the player's for a level -1 quest; nil off a quest row.
    g.set(
        "GetQuestLink",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(lua, v, "Usage: GetQuestLink(questID)")?;
            let model = model_ref(lua);
            let Some(e) =
                quest_at(&model, i64::from(index)).map(|r| super::quest_log::resolve(lua, r))
            else {
                return Ok(Value::Nil);
            };
            let player = i32::try_from(crate::script::item_stats::player_level()).unwrap_or(0);
            let level = e.level as i32;
            let color = quest_color(if level == -1 { player } else { level }, player);
            let link = format!("{color}|Hquest:{}:{level}|h[{}]|h|r", e.quest_id, e.title);
            text(lua, &link)
        })?,
    )?;

    // GetQuestLogSpecialItemInfo(index) (`0x5e52d0`) → link, texture, charges.
    g.set(
        "GetQuestLogSpecialItemInfo",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(lua, v, "Usage: GetQuestLogSpecialItemInfo(index)")?;
            let model = model_ref(lua);
            let Some(item) = special_item(&model, index) else {
                return Ok(MultiValue::new());
            };
            Ok(MultiValue::from_vec(vec![
                text(lua, &item.link)?,
                text(lua, &item.texture)?,
                Value::Integer(i64::from(item.charges)),
            ]))
        })?,
    )?;

    // GetQuestLogSpecialItemCooldown(index) (`0x5e53d0`) → start, duration, enable, as the bag
    // slot's.
    g.set(
        "GetQuestLogSpecialItemCooldown",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(lua, v, "Usage: GetQuestLogSpecialItemCooldown(index)")?;
            let at = special_item(&model_ref(lua), index).map(|i| (i.bag, i.slot));
            let Some((bag, slot)) = at else {
                return Ok(MultiValue::new());
            };
            let cooldown: mlua::Function = lua.globals().get("GetContainerItemCooldown")?;
            cooldown.call((bag, slot))
        })?,
    )?;

    // IsQuestLogSpecialItemInRange(index [, unit]) (`0x5e54c0`) → 1, 0, or nil; the unit defaults
    // to the target (`0x5e557e`).
    g.set(
        "IsQuestLogSpecialItemInRange",
        lua.create_function(|lua, (v, unit): (Value, Value)| {
            let index = number_arg(
                lua,
                v,
                "Usage: IsQuestLogSpecialItemInRange(index [,target])",
            )?;
            // The token as `lua_tolstring` reads it, a number in its string form, anything else none (`0x5e5589`).
            let token = lua
                .coerce_string(unit)?
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            let model = model_ref(lua);
            // The token resolves at the call; a missing or empty one is the target (`0x60b029`).
            let target = if token.is_empty() {
                Some(model.target_guid()).filter(|&g| g != 0)
            } else {
                model.token_guid(&token)
            };
            // The item's spell through the range check at the call (`0x5e55b6`).
            let spell = special_item(&model, index).map(|i| i.spell_id);
            let near = spell.zip(target).and_then(|(id, guid)| {
                super::read_live(|l| l.item_spell_in_range(id, guid))
                    .flatten()
                    .unwrap_or_else(|| model.spell_info.clone()?.spell_in_range(id, false, guid))
            });
            Ok(super::spell_info::range_answer(near))
        })?,
    )?;

    // UseQuestLogSpecialItem(index) (`0x5e5640`): the item's own use (`0x708c20`), with no
    // merchant sale, bank move or equip.
    g.set(
        "UseQuestLogSpecialItem",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(lua, v, "Usage: UseQuestLogSpecialItem(index)")?;
            let item = special_item(&super::model_mut_of(lua), index).map(|i| (i.bag, i.slot));
            if let Some((bag, slot)) = item {
                super::calls::make(lua, super::ScriptCall::UseItem { bag, slot });
            }
            Ok(())
        })?,
    )?;

    // QuestMapUpdateAllQuests() (`0x5e63d0`) → how many quests have a region on the displayed map.
    g.set(
        "QuestMapUpdateAllQuests",
        lua.create_function(|lua, ()| {
            let mut model = super::model_mut_of(lua);
            if !model.worldmap.quest_poi_gate() {
                return Ok(0);
            }
            Ok(update_all_pois(&mut model))
        })?,
    )?;

    // QuestPOIGetQuestIDByVisibleIndex(index) (`0x5e5750`) → questID, questLogIndex.
    g.set(
        "QuestPOIGetQuestIDByVisibleIndex",
        lua.create_function(|lua, v: Value| {
            let index = number_arg(
                lua,
                v,
                "Usage: Script_QuestPOIGetQuestIDByVisibleIndex(index)",
            )?;
            let model = model_ref(lua);
            let id = usize::try_from(index - 1)
                .ok()
                .filter(|&i| i < POI_SLOTS)
                .and_then(|i| poi_by_visible_index(&model, i));
            Ok(match id {
                Some(id) => MultiValue::from_vec(vec![
                    Value::Integer(i64::from(id)),
                    Value::Integer(quest_index(&model, id) as i64),
                ]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    Ok(())
}

/// The special item of the quest at a 1-based row with its link, art and charges, all read from the bags at the call (`0x5e0da0`).
fn special_item(model: &Model, index: i32) -> Option<QuestSpecialItemView> {
    let e = quest_at(model, i64::from(index))?;
    super::read_live(|l| l.quest_special_item(e.quest_id)).flatten()
}

/// The tooltip's quest difficulty colour (`0x61e540`, strings at `0xad2a68`): red five and
/// more levels over the player, orange three, yellow within two, green down to the grey band.
fn quest_color(level: i32, player: i32) -> &'static str {
    let delta = level - player;
    if delta >= 5 {
        "|cffff2020"
    } else if delta >= 3 {
        "|cffff8040"
    } else if delta >= -2 {
        "|cffffff00"
    } else if -delta <= super::unit::grey_band(player.max(0) as u32) as i32 {
        "|cff40c040"
    } else {
        "|cff808080"
    }
}

/// `GameTooltip:SetQuestLogSpecialItem(index)` (`0x630990`): the item instance's hover, as the bag
/// slot's.
pub(super) fn set_tooltip_item(lua: &Lua, this: &Table, index: i32) -> mlua::Result<()> {
    let at = special_item(&model_ref(lua), index).map(|i| (i.bag, i.slot));
    if let Some((bag, slot)) = at {
        this.call_method::<MultiValue>("SetBagItem", (bag, slot))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{QuestLogEntryView, QuestLogState, UiScript};

    fn script(quests: u32) -> UiScript {
        script_with(quests, |_| {})
    }

    /// [`script`] with its seeded log edited first.
    fn script_with(quests: u32, edit: impl FnOnce(&mut QuestLogState)) -> UiScript {
        let s = UiScript::new().unwrap();
        s.register_cvars([
            ("trackedQuests", ""),
            ("trackerSorting", "0"),
            ("questPOI", "1"),
        ]);
        let mut s = s;
        let mut state = QuestLogState::default();
        for q in 1..=quests {
            state.entries.push(QuestLogEntryView {
                quest_id: q,
                title: format!("Quest {q}"),
                level: 30 - q,
                ..Default::default()
            });
        }
        edit(&mut state);
        s.seed_quest_log(state);
        s
    }

    /// `GetQuestLogSpecialItemInfo` (`0x5e52d0`) and the use verb read the item the lent view finds in the bags when called (`0x5e0da0`): nothing with no view, the view's item once one is lent, and nothing again when the view's bags no longer hold it.
    #[test]
    fn the_special_item_verbs_read_the_lent_view_when_called() {
        use crate::script::live::TestView;
        // No container is seeded: the link, art and charges come from the view alone.
        let mut s = script(2);
        let ask = |s: &UiScript| {
            s.eval::<String>(
                "local l, t, c = GetQuestLogSpecialItemInfo(2) return tostring(l) .. tostring(t) .. tostring(c)",
            )
            .unwrap()
        };
        assert_eq!(ask(&s), "nilnilnil");
        let mut carried = TestView {
            special_items: [(
                2,
                QuestSpecialItemView {
                    bag: 0,
                    slot: 3,
                    charges: 4,
                    spell_id: 1000,
                    link: "|Hitem:5000|h[Relic]|h".into(),
                    texture: r"Interface\Icons\Relic".into(),
                },
            )]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            s.with_live(&mut carried, |s| ask(s)),
            r"|Hitem:5000|h[Relic]|hInterface\Icons\Relic4"
        );
        let row_one = |s: &UiScript| {
            s.eval::<i64>("return select('#', GetQuestLogSpecialItemInfo(1))")
                .unwrap()
        };
        assert_eq!(s.with_live(&mut carried, |s| row_one(s)), 0);
        assert_eq!(
            s.with_live(&mut TestView::default(), |s| ask(s)),
            "nilnilnil"
        );
    }

    /// The range check's answer: spell 1000 reaches [`NEAR`] and not [`FAR`]; any other unit or spell has no answer.
    struct Range;

    impl crate::script::SpellInfoSource for Range {
        fn spell_info(&self, _: u32, _: bool) -> Option<crate::script::SpellInfo> {
            None
        }

        fn spell_in_range(&self, spell_id: u32, pet: bool, target: u64) -> Option<bool> {
            assert!(!pet, "the item's spell is the player's");
            (spell_id == 1000 && (target == NEAR || target == FAR)).then_some(target == NEAR)
        }
    }

    const NEAR: u64 = 7;
    const FAR: u64 = 8;

    /// `IsQuestLogSpecialItemInRange(index [, unit])` (`0x5e54c0`): the row's special item (`0x5e5519`), the token's guid (`0x5e5592`), a missing or empty token naming the target (`0x60b029`), a number token read as its string (`0x5e5589`), and the item's spell through the range check (`0x5e55b6`): 1 or 0, nil for no item, no unit or no answer.
    #[test]
    fn the_special_item_range_resolves_its_token_then_checks_the_item_spell() {
        use crate::script::live::TestView;
        let mut s = script(2);
        s.set_spell_info_source(std::rc::Rc::new(Range));
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 1,
            target: NEAR,
            pet: FAR,
            ..Default::default()
        });
        let mut view = TestView {
            special_items: [(
                2,
                QuestSpecialItemView {
                    bag: 0,
                    slot: 3,
                    charges: 0,
                    spell_id: 1000,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        let mut ask = |s: &mut UiScript, call: &str| {
            s.with_live(&mut view, |s| {
                s.eval::<String>(&format!("return tostring({call})"))
                    .unwrap()
            })
        };
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2)"), "1");
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2, nil)"), "1");
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2, '')"), "1");
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2, true)"), "1");
        assert_eq!(
            ask(&mut s, "IsQuestLogSpecialItemInRange('2', 'target')"),
            "1"
        );
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2, 'pet')"), "0");
        assert_eq!(
            ask(&mut s, "IsQuestLogSpecialItemInRange(2, 'party1')"),
            "nil"
        );
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2, 5)"), "nil");
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2, 1)"), "nil");
        // Row 1 holds no special item.
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(1)"), "nil");
        // No target: nobody to range.
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 1,
            pet: FAR,
            ..Default::default()
        });
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2)"), "nil");
        assert_eq!(ask(&mut s, "IsQuestLogSpecialItemInRange(2, 'pet')"), "0");
        // The item gone from the view: nil, whatever the unit.
        let mut none = TestView::default();
        assert_eq!(
            s.with_live(&mut none, |s| s
                .eval::<String>("return tostring(IsQuestLogSpecialItemInRange(2, 'pet'))")
                .unwrap()),
            "nil"
        );
        assert!(s
            .eval::<mlua::Value>("return IsQuestLogSpecialItemInRange()")
            .is_err());
    }

    #[test]
    fn the_watch_list_holds_twenty_five_and_ignores_a_repeat() {
        let s = script(26);
        for i in 1..=26 {
            s.run(&format!("AddQuestWatch({i})")).unwrap();
        }
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 25);
        s.run("AddQuestWatch(1)").unwrap();
        assert_eq!(s.quest_log_watched(), (1..=25).collect::<Vec<_>>());
        assert_eq!(s.eval::<i64>("return IsQuestWatched(25)").unwrap(), 1);
        assert!(s.eval::<bool>("return IsQuestWatched(26) == nil").unwrap());
        assert_eq!(s.eval::<i64>("return GetQuestWatchIndex(3)").unwrap(), 3);
    }

    #[test]
    fn a_timed_watch_expires_and_is_never_saved() {
        let mut s = script(2);
        s.run("AddQuestWatch(1) AddQuestWatch(2, 300)").unwrap();
        assert_eq!(s.quest_log_watched(), vec![1, 2]);
        let saved = s.cvar("trackedQuests").unwrap();
        assert_eq!(super::super::achievement::decode_tracked(&saved), vec![1]);
        // The game UI's idle runs the watch timers after the frame (`0x52b229`).
        s.tick(299.0);
        s.update_quest_watch_timers();
        assert_eq!(s.quest_log_watched(), vec![1, 2]);
        s.tick(2.0);
        s.update_quest_watch_timers();
        assert_eq!(s.quest_log_watched(), vec![1]);
        // A plain add to a timed watch makes it permanent; a timed add never shortens a plain one.
        s.run("AddQuestWatch(2, 5) AddQuestWatch(2) AddQuestWatch(1, 5)")
            .unwrap();
        s.tick(10.0);
        s.update_quest_watch_timers();
        assert_eq!(s.quest_log_watched(), vec![1, 2]);
    }

    #[test]
    fn the_saved_watches_load_with_the_first_log() {
        let s = UiScript::new().unwrap();
        s.register_cvars([("trackedQuests", ""), ("trackerSorting", "0")]);
        let mut s = s;
        s.set_cvar_host(
            "trackedQuests",
            &super::super::achievement::encode_tracked(&[3, 9, 1]),
        );
        s.seed_quest_log(QuestLogState::default());
        let mut state = QuestLogState::default();
        for q in 1..=3u32 {
            state.entries.push(QuestLogEntryView {
                quest_id: q,
                title: format!("Quest {q}"),
                ..Default::default()
            });
        }
        s.seed_quest_log(state);
        // Quest 9 left the log, so its watch is not restored.
        assert_eq!(s.quest_log_watched(), vec![3, 1]);
    }

    #[test]
    fn the_sorts_order_by_level_and_shift_moves_one() {
        let mut s = script(3);
        s.run("AddQuestWatch(2) AddQuestWatch(1) AddQuestWatch(3)")
            .unwrap();
        assert_eq!(s.quest_log_watched(), vec![2, 1, 3]);
        s.run("ShiftQuestWatches(1, 3)").unwrap();
        assert_eq!(s.quest_log_watched(), vec![1, 3, 2]);
        // Levels are 29, 28, 27: hardest first, then easiest first.
        s.set_cvar_host("trackerSorting", "2");
        assert!(s.eval::<bool>("return SortQuestWatches()").unwrap());
        assert_eq!(s.quest_log_watched(), vec![1, 2, 3]);
        s.set_cvar_host("trackerSorting", "3");
        assert!(s.eval::<bool>("return SortQuestWatches()").unwrap());
        assert_eq!(s.quest_log_watched(), vec![3, 2, 1]);
        s.set_cvar_host("trackerSorting", "0");
        assert!(!s.eval::<bool>("return SortQuestWatches()").unwrap());
    }

    /// Every rebuild asks again for each quest still without a POI (`0x5e6aab`), none deduped across rebuilds.
    #[test]
    fn every_rebuild_asks_for_each_quest_without_a_poi() {
        let mut s = script(2);
        assert_eq!(rebuild_poi_asks(&s.model_ref()), vec![1, 2]);
        assert_eq!(rebuild_poi_asks(&s.model_ref()), vec![1, 2]);
        s.apply_quest_pois(vec![QuestPoiView {
            quest_id: 1,
            regions: Vec::new(),
        }]);
        assert_eq!(s.model_ref().quest_watch.pois.len(), 1);
        assert_eq!(rebuild_poi_asks(&s.model_ref()), vec![2]);
    }

    #[test]
    fn the_visible_order_puts_settled_quests_then_watches_then_the_rest() {
        let s = script_with(4, |log| {
            log.entries[0].complete = 1;
            for e in &mut log.entries[1..] {
                e.objectives.push(Default::default());
            }
        });
        {
            let mut m = s.model_mut();
            for id in [4, 3, 2, 1] {
                m.quest_watch.pois.push(PoiSlot {
                    quest_id: id,
                    regions: Vec::new(),
                    shown: true,
                    icon: None,
                });
            }
        }
        s.run("AddQuestWatch(2)").unwrap();
        let ids: Vec<i64> = s
            .eval(
                "local t = {} for i = 1, 4 do t[i] = QuestPOIGetQuestIDByVisibleIndex(i) end \
                 return t",
            )
            .unwrap();
        assert_eq!(ids, vec![1, 2, 4, 3]);
        assert_eq!(
            s.eval::<(i64, i64)>("return QuestPOIGetQuestIDByVisibleIndex(2)")
                .unwrap(),
            (2, 2)
        );
    }

    #[test]
    fn the_link_and_completion_text_read_the_row() {
        let mut s = script_with(1, |log| {
            log.entries[0].completion_text = "Return to Marshal.".into();
        });
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 29,
            ..Default::default()
        });
        assert_eq!(
            s.eval::<String>("return GetQuestLink(1)").unwrap(),
            "|cffffff00|Hquest:1:29|h[Quest 1]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return GetQuestLogCompletionText(1)")
                .unwrap(),
            "Return to Marshal."
        );
        assert!(s.eval::<bool>("return GetQuestLink(2) == nil").unwrap());
    }
}
