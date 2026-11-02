//! The world map's quest POI verbs over the POI store in [`super::quest_watch`]: the icon
//! placement `QuestPOIUpdateIcons` runs (`0x5e5740` -> `0x5e2eb0`) and the icon `QuestPOIGetIconInfo`
//! projects (`0x5e0590`), the map row `GetQuestWorldMapAreaID` picks (`0x5e56c0` -> `0x5e2b60`),
//! and the POI leaderboard and drop lines the map's tooltips read (`0x5e6650`, `0x5e4260`,
//! `0x5e60d0`), which the app builds per quest ([`QuestPoiLines`]).

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{coerced_number, flag, number_arg};
use super::quest_log::QuestLogObjectiveView;
use super::quest_watch::{objective_mask, place, serves};
use super::worldmap::QuestIconFrame;
use super::Model;

/// The icon force pass's iterations (`[0xad1264]`).
const OVERLAP_ITERATIONS: usize = 20;
/// `SetPOIIconOverlapDistance`'s default (`[0xad1014]`).
pub(crate) const OVERLAP_DISTANCE: f32 = 12.0;
/// `SetPOIIconOverlapPushDistance`'s default (`[0xad1018]`).
pub(crate) const OVERLAP_PUSH_DISTANCE: f32 = 18.0;

/// One quest's placed icon: the region's map, floor, objective and its centre, pushed aside by the
/// force pass, in world yards.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PoiIcon {
    map: u32,
    floor: i32,
    objective: i32,
    x: i32,
    y: i32,
}

/// The per-quest lines the POI verbs read, built by the app beside the leaderboard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestPoiLines {
    /// `GetQuestPOILeaderBoard`'s lines by index (`0x5e58c0`): 0-3 the objectives, 4-9 the
    /// required items, 10-15 the drops looked up among the required items, 16 and 17 the end
    /// text; `None` answers three nils.
    pub board: Vec<Option<QuestLogObjectiveView>>,
    /// `GetQuestLogItemDrop`'s lines (`0x5e20a0`): each drop with an item, in objective order.
    pub drops: Vec<QuestLogObjectiveView>,
    /// `GetNumQuestItemDrops` (`0x5e1ff0`): the drops not carried, the source item excepted.
    pub drops_needed: u32,
    /// The top `AreaTable` id over the quest's header zone, `GetQuestWorldMapAreaID`'s fallback
    /// (`0x5e2b60`); 0 for a `QuestSort` header.
    pub top_area: u32,
}

/// The POI verbs' state beside the POI store.
#[derive(Debug)]
pub(crate) struct QuestPoiState {
    pub(crate) lines: HashMap<u32, QuestPoiLines>,
    overlap: f32,
    /// Kept as the reference keeps it, which reads it nowhere else.
    #[allow(dead_code)]
    push: f32,
}

impl Default for QuestPoiState {
    fn default() -> Self {
        Self {
            lines: HashMap::new(),
            overlap: OVERLAP_DISTANCE,
            push: OVERLAP_PUSH_DISTANCE,
        }
    }
}

impl super::UiScript {
    /// Push each logged quest's POI lines, replaced only when they moved.
    pub fn set_quest_poi_lines(&mut self, lines: HashMap<u32, QuestPoiLines>) {
        if self.model_ref().quest_poi.lines != lines {
            self.model_mut().quest_poi.lines = lines;
        }
    }
}

/// The quest at a 1-based log row, 0 for a header or no row.
fn quest_at_row(model: &Model, row: i64) -> u32 {
    usize::try_from(row - 1)
        .ok()
        .and_then(|i| model.quest_log.entries.get(i))
        .filter(|e| !e.is_header)
        .map_or(0, |e| e.quest_id)
}

/// The selected quest, the verbs' default.
fn selected_quest(model: &Model) -> u32 {
    quest_at_row(model, i64::from(model.quest_log_selection))
}

fn line_values(lua: &Lua, line: Option<&QuestLogObjectiveView>) -> mlua::Result<MultiValue> {
    let Some(l) = line else {
        return Ok(MultiValue::from_vec(vec![
            Value::Nil,
            Value::Nil,
            Value::Nil,
        ]));
    };
    Ok(MultiValue::from_vec(vec![
        Value::String(lua.create_string(&l.text)?),
        Value::String(lua.create_string(&l.kind)?),
        flag(l.finished),
    ]))
}

/// The icon placement (`0x5e2eb0`): per POI slot the counting region shown on the displayed map
/// with the lowest priority, then the nearest to the player; off a dungeon floor, the open quests'
/// icons pushed apart and the complete ones' stacks fanned out.
fn update_icons(model: &mut Model) {
    if super::quest_watch::cvar_int(model, "questpoi") == 0 {
        return;
    }
    let Some(me) = model.worldmap.player_point() else {
        return;
    };
    let Some(frame) = model.worldmap.quest_icon_frame() else {
        return;
    };
    let mut active = Vec::new();
    let mut complete = Vec::new();
    let mut movable = vec![false; model.quest_watch.pois.len()];
    let mut icons: Vec<Option<PoiIcon>> = Vec::with_capacity(model.quest_watch.pois.len());
    for (i, slot) in model.quest_watch.pois.iter().enumerate() {
        let mask = objective_mask(model, slot.quest_id);
        let mut best: Option<(usize, i32, f32)> = None;
        for (j, r) in slot.regions.iter().enumerate() {
            if !serves(r, mask) || model.worldmap.quest_poi_shown(&place(r)) != Some(true) {
                continue;
            }
            let (dx, dy) = (me.x - r.center.0 as f32, me.y - r.center.1 as f32);
            let d = dx * dx + dy * dy;
            if best.is_none_or(|(_, p, bd)| r.priority < p || (r.priority == p && d < bd)) {
                best = Some((j, r.priority, d));
            }
        }
        let Some((j, _, _)) = best else {
            icons.push(None);
            continue;
        };
        let r = slot.regions[j];
        icons.push(Some(PoiIcon {
            map: r.map,
            floor: r.floor,
            objective: r.objective,
            x: r.center.0,
            y: r.center.1,
        }));
        let done = model
            .quest_log
            .entries
            .iter()
            .find(|e| !e.is_header && e.quest_id == slot.quest_id)
            .is_some_and(|e| e.complete == 1);
        if done {
            complete.push(i);
        } else {
            movable[i] = r.points != 1;
            active.push(i);
        }
    }
    let shift = super::quest_watch::cvar_float(model, "poishiftcomplete");
    let overlap = model.quest_poi.overlap;
    if let QuestIconFrame::Area { loc_rect } = frame {
        spread(&mut icons, &active, &movable, loc_rect, overlap);
        fan_out(&mut icons, &complete, shift);
    }
    for (slot, icon) in model.quest_watch.pois.iter_mut().zip(icons) {
        slot.icon = icon;
    }
}

/// The force pass over the open quests' icons (`0x5e2eb0`): y squeezed to the map's
/// aspect, pairs closer than the overlap radius pushed apart in integer steps, then y restored.
fn spread(
    icons: &mut [Option<PoiIcon>],
    active: &[usize],
    movable: &[bool],
    loc_rect: (f32, f32, f32, f32),
    overlap: f32,
) {
    const NUDGES: [(i32, i32); 3] = [(0, 2), (1, -2), (-2, 1)];
    let span = (loc_rect.0 - loc_rect.1).abs() as i32;
    let radius = span as f32 * (overlap + overlap) * (1.0 / 1024.0);
    let max_step = 0.05 * radius;
    let radius_sq = radius * radius;
    let mut nudge = 0;
    let xy = |icons: &[Option<PoiIcon>], i: usize| icons[i].map_or((0, 0), |c| (c.x, c.y));
    for &i in active {
        if let Some(c) = icons[i].as_mut() {
            c.y = (c.y as f32 * 0.750_018_8) as i32;
        }
    }
    for _ in 0..OVERLAP_ITERATIONS {
        let mut forces = vec![(0i32, 0i32); icons.len()];
        for (k, &a) in active.iter().enumerate() {
            let (ax, ay) = xy(icons, a);
            for &b in &active[k + 1..] {
                let (bx, by) = xy(icons, b);
                let (mut dx, mut dy) = (ax.wrapping_sub(bx), ay.wrapping_sub(by));
                let d_sq = dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy));
                if d_sq as f32 >= radius_sq {
                    continue;
                }
                if d_sq == 0 {
                    (dx, dy) = NUDGES[nudge];
                    nudge = (nudge + 1) % 3;
                }
                let scale =
                    f64::from(radius) / (f64::from(dx).powi(2) + f64::from(dy).powi(2)).sqrt();
                let (fx, fy) = (
                    (f64::from(dx) * scale) as i32,
                    (scale * f64::from(dy)) as i32,
                );
                if movable[a] {
                    forces[a].0 += fx;
                    forces[a].1 += fy;
                }
                if d_sq != 0 && movable[b] {
                    forces[b].0 -= fx;
                    forces[b].1 -= fy;
                }
            }
        }
        let max_force = active
            .iter()
            .flat_map(|&i| [forces[i].0.abs() as f32, forces[i].1.abs() as f32])
            .fold(-9_999_999.0f32, f32::max);
        if max_force == 0.0 {
            break;
        }
        if max_step < max_force {
            let scale = max_step / max_force;
            for &i in active {
                forces[i] = (
                    (forces[i].0 as f32 * scale) as i32,
                    (forces[i].1 as f32 * scale) as i32,
                );
            }
        }
        for &i in active {
            if let Some(c) = icons[i].as_mut() {
                c.x += forces[i].0;
                c.y += forces[i].1;
            }
        }
    }
    for &i in active {
        if let Some(c) = icons[i].as_mut() {
            c.y = (c.y as f32 * 1.3333) as i32;
        }
    }
}

/// The complete quests' stacking (`0x5e2eb0`): an icon within `POIShiftComplete` × 100
/// of an earlier one chains on it, and each chained icon steps along four fixed offsets.
fn fan_out(icons: &mut [Option<PoiIcon>], complete: &[usize], shift: f32) {
    const OFFSETS: [(i32, i32); 4] = [(50, 86), (-86, 50), (-50, 86), (-90, 10)];
    if shift <= 0.001 {
        return;
    }
    let xy = |icons: &[Option<PoiIcon>], i: usize| icons[i].map_or((0, 0), |c| (c.x, c.y));
    let mut chain = vec![0u32; icons.len()];
    let threshold = shift * 100.0;
    for c in 0..complete.len().saturating_sub(1) {
        let index = complete[c + 1];
        let (cx, cy) = xy(icons, index);
        for d in (0..=c).rev() {
            let (ox, oy) = xy(icons, complete[d]);
            let d_sq = (cx - ox) * (cx - ox) + (cy - oy) * (cy - oy);
            if (d_sq as f32) < threshold {
                chain[index] = chain[complete[d]] + 1;
                break;
            }
        }
    }
    let mut offset = 0;
    for &index in complete.iter().skip(1) {
        if chain[index] == 0 {
            continue;
        }
        let (ox, oy) = OFFSETS[offset];
        offset = (offset + 1) & 3;
        if let Some(c) = icons[index].as_mut() {
            c.x += (ox as f32 * shift) as i32;
            c.y += (shift * oy as f32) as i32;
        }
    }
}

/// `GetQuestWorldMapAreaID`'s pick (`0x5e2b60`), after the move to the player's zone: a counting
/// region on the displayed row and floor, else the nearest on the same map by floor delta then
/// distance, else the first elsewhere; with none, the row of the header zone's top area.
fn quest_area(model: &Model, quest_id: u32) -> (u32, i32) {
    let wm = &model.worldmap;
    if let (Some((here, here_map, here_floor)), Some(slot)) =
        (wm.quest_area_here(), model.quest_watch.poi(quest_id))
    {
        let mask = objective_mask(model, quest_id);
        let me = wm.player_point();
        let mut best: Option<(u32, i32)> = None;
        let mut best_key = (i32::MAX, f32::MAX);
        for r in &slot.regions {
            let counts = if mask == u32::MAX {
                r.objective == -1
            } else {
                r.objective != -1 && serves(r, mask)
            };
            if !counts {
                continue;
            }
            if r.area == here && r.floor == here_floor {
                return (r.area, r.floor);
            }
            let Some(map) = wm.area_map(r.area) else {
                continue;
            };
            if map == here_map {
                let (dx, dy) = me.map_or((0.0, 0.0), |p| {
                    (p.x - r.center.0 as f32, p.y - r.center.1 as f32)
                });
                let key = ((here_floor - r.floor).abs(), dx * dx + dy * dy);
                if key.0 < best_key.0 || (key.0 == best_key.0 && key.1 < best_key.1) {
                    best_key = key;
                    best = Some((r.area, r.floor));
                }
            } else if best.is_none() {
                best = Some((r.area, r.floor));
            }
        }
        if let Some(found) = best.filter(|b| b.0 != 0) {
            return found;
        }
    }
    let top = model
        .quest_poi
        .lines
        .get(&quest_id)
        .map_or(0, |l| l.top_area);
    if top == 0 {
        return (0, 0);
    }
    (wm.area_row_for(top).unwrap_or(0), 0)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `QuestPOIUpdateIcons()` (`0x5e5740`).
    g.set(
        "QuestPOIUpdateIcons",
        lua.create_function(|lua, ()| {
            update_icons(&mut crate::script::model_mut_of(lua));
            Ok(())
        })?,
    )?;

    // `QuestPOIGetIconInfo(questID)` (`0x5e0590`): `isComplete, x, y, objective` for the slot's
    // icon on the displayed map, nothing off it or with no slot.
    g.set(
        "QuestPOIGetIconInfo",
        lua.create_function(|lua, quest: Value| {
            let quest = number_arg(lua, quest, "Usage: QuestPOIGetIconInfo(questID)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(slot) = u32::try_from(quest)
                .ok()
                .and_then(|q| model.quest_watch.poi(q))
            else {
                return Ok(MultiValue::new());
            };
            let icon = slot.icon.unwrap_or(PoiIcon {
                x: 999_999,
                y: 999_999,
                ..Default::default()
            });
            let ((u, v), outland) =
                model
                    .worldmap
                    .quest_project(icon.map, icon.x as f32, icon.y as f32);
            if outland || (u * v).abs() < 0.001 {
                return Ok(MultiValue::new());
            }
            let complete = objective_mask(&model, slot.quest_id) == u32::MAX;
            Ok(MultiValue::from_vec(vec![
                Value::Boolean(complete),
                Value::Number(f64::from(u)),
                Value::Number(f64::from(v)),
                Value::Integer(i64::from(icon.objective)),
            ]))
        })?,
    )?;

    // `GetQuestWorldMapAreaID(questID)` (`0x5e56c0`): `areaID, floor`, after moving the map to
    // the player's zone, whose setter fires `WORLD_MAP_UPDATE` before the verb returns.
    g.set(
        "GetQuestWorldMapAreaID",
        lua.create_function(|lua, quest: Value| {
            let quest = number_arg(lua, quest, "Usage: GetQuestWorldMapAreaID(questID)")?;
            let has_player = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.worldmap.player_point().is_some()
            };
            if !has_player || quest <= 0 {
                return Ok((0i64, 0i64));
            }
            crate::script::model_mut_of(lua)
                .worldmap
                .set_to_player_zone();
            super::tick::fire_event_into(lua, "WORLD_MAP_UPDATE", Vec::new());
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let (area, floor) = quest_area(&model, quest as u32);
            Ok((i64::from(area), i64::from(floor)))
        })?,
    )?;

    // `GetQuestPOILeaderBoard(index, questIndex)` (`0x5e6650`): the line at `index`, not minus 1,
    // of the row's quest, else the selection's; the usage line is the log verb's.
    g.set(
        "GetQuestPOILeaderBoard",
        lua.create_function(|lua, (index, row): (Value, Option<Value>)| {
            let index = number_arg(lua, index, "Usage: GetQuestLogLeaderBoard(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let mut quest = quest_at_row(&model, coerced_number(lua, row) as i64);
            if quest == 0 {
                quest = selected_quest(&model);
            }
            let line = usize::try_from(index).ok().and_then(|i| {
                model
                    .quest_poi
                    .lines
                    .get(&quest)
                    .and_then(|l| l.board.get(i))
                    .and_then(Option::as_ref)
            });
            line_values(lua, line)
        })?,
    )?;

    // `GetNumQuestItemDrops([questIndex])` (`0x5e4260`): the row's quest, else the selection's.
    g.set(
        "GetNumQuestItemDrops",
        lua.create_function(|lua, row: Option<Value>| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let quest = match row.and_then(|v| lua.coerce_number(v).ok().flatten()) {
                Some(n) => quest_at_row(&model, n as i64),
                None => selected_quest(&model),
            };
            Ok(i64::from(
                model
                    .quest_poi
                    .lines
                    .get(&quest)
                    .map_or(0, |l| l.drops_needed),
            ))
        })?,
    )?;

    // `GetQuestLogItemDrop(index, questIndex)` (`0x5e60d0`): drop `index` (1-based) of the row's
    // quest, else the selection's.
    g.set(
        "GetQuestLogItemDrop",
        lua.create_function(|lua, (index, row): (Value, Option<Value>)| {
            let index = number_arg(lua, index, "Usage: GetQuestLogItemDrop(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let mut quest = quest_at_row(&model, coerced_number(lua, row) as i64);
            if quest == 0 {
                quest = selected_quest(&model);
            }
            let line = usize::try_from(index - 1).ok().and_then(|i| {
                model
                    .quest_poi
                    .lines
                    .get(&quest)
                    .and_then(|l| l.drops.get(i))
            });
            line_values(lua, line)
        })?,
    )?;

    // `SetPOIIconOverlapDistance(n)` (`0x5dfb00`) and `SetPOIIconOverlapPushDistance(n)`
    // (`0x5dfb40`), the latter kept only at or above the first.
    g.set(
        "SetPOIIconOverlapDistance",
        lua.create_function(|lua, n: Value| {
            let n = lua.coerce_number(n)?.ok_or_else(|| {
                mlua::Error::RuntimeError("Usage: SetPOIIconOverlapDistance(index)".into())
            })?;
            crate::script::model_mut_of(lua).quest_poi.overlap = n as f32;
            Ok(())
        })?,
    )?;
    g.set(
        "SetPOIIconOverlapPushDistance",
        lua.create_function(|lua, n: Value| {
            let n = lua.coerce_number(n)?.ok_or_else(|| {
                mlua::Error::RuntimeError("Usage: SetPOIIconOverlapPushDistance(index)".into())
            })? as f32;
            let mut model = crate::script::model_mut_of(lua);
            if model.quest_poi.overlap <= n {
                model.quest_poi.push = n;
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn icon(x: i32, y: i32) -> Option<PoiIcon> {
        Some(PoiIcon {
            x,
            y,
            ..Default::default()
        })
    }

    /// Two open icons on one spot are pushed apart; a one-point region's icon stays put
    /// (`0x5e3165`).
    #[test]
    fn overlapping_open_icons_are_pushed_apart_unless_pinned() {
        let rect = (1000.0, -1000.0, 0.0, 0.0);
        let mut icons = vec![icon(100, 100), icon(100, 100)];
        spread(&mut icons, &[0, 1], &[true, true], rect, OVERLAP_DISTANCE);
        let (a, b) = (icons[0].unwrap(), icons[1].unwrap());
        assert_ne!((a.x, a.y), (b.x, b.y), "the pair separated");
        let mut icons = vec![icon(100, 100), icon(104, 100)];
        spread(&mut icons, &[0, 1], &[false, true], rect, OVERLAP_DISTANCE);
        assert_eq!(icons[0].unwrap().x, 100, "a pinned icon does not move");
        assert!(
            icons[1].unwrap().x > 104,
            "the other is pushed away from it"
        );
        // Far apart: only the aspect squeeze's rounding moves them.
        let mut icons = vec![icon(-900, 0), icon(900, 0)];
        spread(&mut icons, &[0, 1], &[true, true], rect, OVERLAP_DISTANCE);
        assert_eq!((icons[0].unwrap().x, icons[1].unwrap().x), (-900, 900));
    }

    /// A complete icon stacked on an earlier one steps along the first offset, scaled.
    #[test]
    fn stacked_complete_icons_fan_out() {
        let mut icons = vec![icon(0, 0), icon(1, 1)];
        fan_out(&mut icons, &[0, 1], 0.6);
        assert_eq!(icons[0].unwrap().x, 0);
        assert_eq!((icons[1].unwrap().x, icons[1].unwrap().y), (31, 52));
        let mut icons = vec![icon(0, 0), icon(1, 1)];
        fan_out(&mut icons, &[0, 1], 0.0);
        assert_eq!(icons[1].unwrap().x, 1, "POIShiftComplete 0 leaves them");
    }

    fn with_lines() -> UiScript {
        use crate::script::{QuestLogEntryView, QuestLogState};
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(QuestLogState {
            num_quests: 1,
            entries: vec![
                QuestLogEntryView {
                    title: "Elwynn Forest".into(),
                    is_header: true,
                    ..Default::default()
                },
                QuestLogEntryView {
                    quest_id: 33,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let line = |t: &str, k: &str, done| QuestLogObjectiveView {
            text: t.into(),
            kind: k.into(),
            finished: done,
            ..Default::default()
        };
        let mut board = vec![None; 18];
        board[4] = Some(line("Diseased Wolf Pelt: 3/8", "item", false));
        board[16] = Some(line("Return to Eagan.", "event", true));
        s.set_quest_poi_lines(
            [(
                33,
                QuestPoiLines {
                    board,
                    drops: vec![line("Venom Sac: 0/1", "item", false)],
                    drops_needed: 1,
                    top_area: 12,
                },
            )]
            .into(),
        );
        s
    }

    /// `0x5e6650` reads `index` as is; `0x5e60d0` minus one; both fall back to the selection.
    #[test]
    fn the_poi_lines_answer_by_row_or_selection() {
        let s = with_lines();
        assert_eq!(
            s.eval::<(String, String, Option<i64>)>("return GetQuestPOILeaderBoard(4, 2)")
                .unwrap(),
            ("Diseased Wolf Pelt: 3/8".into(), "item".into(), None)
        );
        assert_eq!(
            s.eval::<(String, String, i64)>("return GetQuestPOILeaderBoard(16, 2)")
                .unwrap(),
            ("Return to Eagan.".into(), "event".into(), 1)
        );
        assert_eq!(
            s.eval::<i64>("return select('#', GetQuestPOILeaderBoard(0, 2))")
                .unwrap(),
            3
        );
        assert!(
            s.eval::<bool>("return GetQuestPOILeaderBoard(4) == nil")
                .unwrap(),
            "no selection"
        );
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert_eq!(
            s.eval::<String>("return (GetQuestPOILeaderBoard(4))")
                .unwrap(),
            "Diseased Wolf Pelt: 3/8"
        );
        assert_eq!(s.eval::<i64>("return GetNumQuestItemDrops()").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetNumQuestItemDrops(1)").unwrap(),
            0,
            "a header row"
        );
        assert_eq!(
            s.eval::<String>("return (GetQuestLogItemDrop(1, 2))")
                .unwrap(),
            "Venom Sac: 0/1"
        );
        assert!(s
            .run("GetQuestLogItemDrop()")
            .unwrap_err()
            .to_string()
            .contains("Usage: GetQuestLogItemDrop(index)"));
        assert!(s
            .run("GetQuestPOILeaderBoard()")
            .unwrap_err()
            .to_string()
            .contains("Usage: GetQuestLogLeaderBoard(index)"));
        assert!(s
            .run("QuestPOIGetIconInfo()")
            .unwrap_err()
            .to_string()
            .contains("Usage: QuestPOIGetIconInfo(questID)"));
        assert_eq!(
            s.eval::<i64>("return select('#', QuestPOIGetIconInfo(33))")
                .unwrap(),
            0,
            "no POI slot"
        );
    }

    /// The push distance only takes values at or above the overlap distance (`0x5dfb40`).
    #[test]
    fn the_overlap_verbs_set_the_spacing() {
        let s = UiScript::new().unwrap();
        s.run("SetPOIIconOverlapDistance(20) SetPOIIconOverlapPushDistance(10)")
            .unwrap();
        assert_eq!(s.model_ref().quest_poi.overlap, 20.0);
        assert_eq!(s.model_ref().quest_poi.push, OVERLAP_PUSH_DISTANCE);
        s.run("SetPOIIconOverlapPushDistance(25)").unwrap();
        assert_eq!(s.model_ref().quest_poi.push, 25.0);
    }
}
