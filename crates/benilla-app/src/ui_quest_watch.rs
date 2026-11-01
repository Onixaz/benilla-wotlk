//! The objective tracker's feeds: the quest POI wire (`CMSG_QUEST_POI_QUERY` for the quests the VM
//! holds no POI for, `SMSG_QUEST_POI_QUERY_RESPONSE` into its store) and each quest's special item
//! (`0x5e0da0`), found in the bags with its charges and range.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::RangeTargets;
use benilla_protocol::messages::QuestPoi;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    QuestLogObjectiveView, QuestPoiLines, QuestPoiRegion, QuestPoiView, QuestSpecialItemView,
    UiScript,
};

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, ObjectStore, SelfPlayer};
use crate::target::Selection;
use crate::ui_action::Spells;
use crate::ui_quest_log::QuestLog;
use crate::ui_unit::UnitFeed;

/// A quest template's flag that lets its drop items serve as the special item (`0x5e0df9`).
const QUEST_FLAG_DROP_ITEM_USE: u32 = 0x20000;

/// The POI responses the live VM has not taken yet.
#[derive(Resource, Default)]
struct PendingPois(Vec<QuestPoi>);

pub(crate) struct UiQuestWatchPlugin;

impl Plugin for UiQuestWatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingPois>()
            .net_handler(SessionEventKind::QuestPoi, on_quest_poi)
            .net_handler(SessionEventKind::Disconnected, on_session_end)
            .add_systems(
                Update,
                (feed_quest_pois, feed_special_items, feed_quest_poi_lines)
                    .in_set(UnitFeed)
                    .after(crate::ui_quest_log::feed_quest_log),
            );
    }
}

fn on_quest_poi(In(ev): In<SessionEvent>, mut pending: ResMut<PendingPois>) {
    if let SessionEvent::QuestPoi(pois) = ev {
        debug!("ui_quest_watch: POI for {} quest(s)", pois.len());
        pending.0.extend(pois);
    }
}

fn on_session_end(In(_): In<SessionEvent>, mut pending: ResMut<PendingPois>) {
    pending.0.clear();
}

/// The response's outline centres, the integer mean the response's handler sums (`0x5e7370`).
fn view(poi: QuestPoi) -> QuestPoiView {
    QuestPoiView {
        quest_id: poi.quest_id,
        regions: poi
            .blobs
            .into_iter()
            .map(|b| {
                let n = b.points.len() as i64;
                let (sx, sy) = b.points.iter().fold((0i64, 0i64), |(x, y), p| {
                    (x + i64::from(p.0), y + i64::from(p.1))
                });
                let center = if n > 1 {
                    ((sx / n) as i32, (sy / n) as i32)
                } else {
                    (sx as i32, sy as i32)
                };
                QuestPoiRegion {
                    objective: b.objective,
                    map: b.map,
                    area: b.area,
                    floor: b.floor as i32,
                    flags: b.flags,
                    center,
                    points: n as u32,
                    priority: b.priority as i32,
                }
            })
            .collect(),
    }
}

/// A GameObject objective's entry bit (`0x5e2370`'s sign test).
const GO_OBJECTIVE_BIT: u32 = 0x8000_0000;
/// Deviation: an uncached name reads `...` where the reference prints `" "`, as the quest log's
/// lines do.
const NAME_PLACEHOLDER: &str = "...";

/// One POI line from its `%s`/`%d`/`%d` key.
fn poi_line(
    get: &dyn Fn(&str) -> Option<String>,
    key: &str,
    kind: &str,
    name: &str,
    (cur, req): (u32, u32),
) -> QuestLogObjectiveView {
    use benilla_ui::strings::{fill, Arg};
    QuestLogObjectiveView {
        text: get(key).map_or_else(String::new, |f| {
            fill(
                &f,
                &[Arg::S(name), Arg::D(i64::from(cur)), Arg::D(i64::from(req))],
            )
        }),
        kind: kind.into(),
        finished: cur >= req,
        cur,
        req,
    }
}

/// Each logged quest's POI lines (`0x5e58c0`, `0x5e20a0`, `0x5e1ff0`): the objectives by entry
/// with their raw counters (`0x5e2370`), the items by id with the bag count capped (`0x5e2630`),
/// the drops, and the header zone's top area. Rebuilt only when an input moved.
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
fn feed_quest_poi_lines(
    script: Option<NonSendMut<UiScript>>,
    me: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    quest_log: Res<QuestLog>,
    names: Res<crate::names::NameCache>,
    gos: Option<Res<crate::go_templates::GameObjectTemplates>>,
    items: Res<Items>,
    objects: crate::net::Objects,
    (commands, areas, stirred_items): (
        Res<NetCommands>,
        Option<Res<crate::sound::AreaSounds>>,
        Query<(), (Changed<ObjectStore>, Without<crate::net::NetEntity>)>,
    ),
) {
    use crate::ui_items::{count_of, InventoryScope};
    let Some(mut script) = script else {
        return;
    };
    let Some(store) = me.iter().next() else {
        return;
    };
    let stirred = store.is_changed()
        || quest_log.is_changed()
        || names.is_changed()
        || items.is_changed()
        || gos.as_ref().is_some_and(|g| g.is_changed())
        || !stirred_items.is_empty();
    if !stirred {
        return;
    }
    let get = |key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    };
    let bags = |item: u32| count_of(&store.0, &objects, item, InventoryScope::QUEST_ITEMS);
    let item_name = |item: u32| {
        items
            .template(item, 0, &commands)
            .map_or_else(|| NAME_PLACEHOLDER.to_string(), |t| t.name.clone())
    };
    let mut lines = HashMap::new();
    for slot in 0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS {
        let Some(log) = store.0.player_quest_log(slot).filter(|l| l.quest_id != 0) else {
            continue;
        };
        let Some(t) = quest_log.template(log.quest_id, &commands) else {
            continue;
        };
        // An objective by its entry, its first match and counter (`0x5e2370`).
        let by_object = |entry: u32| -> Option<QuestLogObjectiveView> {
            if entry == 0 {
                return None;
            }
            let j = t
                .objectives
                .iter()
                .position(|o| o.creature_or_go == entry)?;
            let o = &t.objectives[j];
            let count = (u32::from(log.counters[j]), o.required_count);
            let is_go = entry & GO_OBJECTIVE_BIT != 0;
            let kind = if is_go { "object" } else { "monster" };
            Some(if !o.text.is_empty() {
                poi_line(&get, "QUEST_OBJECTS_FOUND", kind, &o.text, count)
            } else if is_go {
                let name = gos
                    .as_deref()
                    .and_then(|g| g.name_by_entry(entry & !GO_OBJECTIVE_BIT, &commands))
                    .unwrap_or(NAME_PLACEHOLDER);
                poi_line(&get, "QUEST_OBJECTS_FOUND", kind, name, count)
            } else {
                let name = names
                    .resolve_creature(entry, 0, &commands)
                    .unwrap_or(NAME_PLACEHOLDER);
                poi_line(&get, "QUEST_MONSTERS_KILLED", kind, name, count)
            })
        };
        // An item by id among the required items, the lone source item skipped (`0x5e2630`).
        let by_item = |item: u32| -> Option<QuestLogObjectiveView> {
            if item == 0 {
                return None;
            }
            let &(_, req) = t
                .required_items
                .iter()
                .find(|&&(id, n)| id == item && !(id == t.src_item_id && n <= 1))?;
            let have = bags(item).min(req);
            Some(poi_line(
                &get,
                "QUEST_ITEMS_NEEDED",
                "item",
                &item_name(item),
                (have, req),
            ))
        };
        let mut board: Vec<Option<QuestLogObjectiveView>> = Vec::with_capacity(18);
        board.extend(t.objectives.iter().map(|o| by_object(o.creature_or_go)));
        board.extend(t.required_items.iter().map(|&(id, _)| by_item(id)));
        board.extend(t.objectives.iter().map(|o| by_item(o.item_drop)));
        let event = (!t.end_text.is_empty()).then(|| {
            let done = log.state & benilla_protocol::messages::quest_slot_state::COMPLETE != 0;
            QuestLogObjectiveView {
                text: t.end_text.clone(),
                kind: "event".into(),
                finished: done,
                cur: u32::from(done),
                req: 1,
            }
        });
        board.push(event.clone());
        board.push(event);
        let drops = t
            .objectives
            .iter()
            .filter(|o| o.item_drop as i32 > 0)
            .map(|o| {
                let have = bags(o.item_drop).min(o.item_drop_count);
                poi_line(
                    &get,
                    "QUEST_ITEMS_NEEDED",
                    "item",
                    &item_name(o.item_drop),
                    (have, o.item_drop_count),
                )
            })
            .collect();
        let drops_needed = t
            .objectives
            .iter()
            .filter(|o| {
                o.item_drop as i32 > 0
                    && (o.item_drop != t.src_item_id || o.item_drop_count as i32 > 1)
                    && bags(o.item_drop) == 0
            })
            .count() as u32;
        let top_area = u32::try_from(t.zone_or_sort)
            .ok()
            .filter(|&z| z != 0)
            .and_then(|z| areas.as_ref()?.0.top_area(z))
            .unwrap_or(0);
        lines.insert(
            log.quest_id,
            QuestPoiLines {
                board,
                drops,
                drops_needed,
                top_area,
            },
        );
    }
    script.set_quest_poi_lines(lines);
}

/// Hand the VM its POI responses and send the queries it raised, at most 25 ids each, as the
/// reference's one query per log update carries.
fn feed_quest_pois(
    script: Option<NonSendMut<UiScript>>,
    mut pending: ResMut<PendingPois>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if !pending.0.is_empty() {
        let pois = std::mem::take(&mut pending.0);
        script.apply_quest_pois(pois.into_iter().map(view).collect());
    }
    let wants = script.take_quest_poi_queries();
    for chunk in wants.chunks(25) {
        debug!("ui_quest_watch: asking POI for {chunk:?}");
        let _ = commands.0.send(ClientCommand::QuestPoiQuery {
            quests: chunk.to_vec(),
        });
    }
}

/// The quest's special item (`0x5e0da0`) for each logged quest: the template's source item, else
/// under flag `0x20000` its drop items in order, the first carried copy with an on-use spell
/// (`0x708c10`); its range against the target as `IsItemInRange` asks it.
#[allow(clippy::too_many_arguments)] // a Bevy system's inputs
fn feed_special_items(
    script: Option<NonSendMut<UiScript>>,
    quest_log: Res<QuestLog>,
    self_q: Query<(&ObjectStore, &Transform), With<SelfPlayer>>,
    units: Query<&Transform, Without<SelfPlayer>>,
    objects: crate::net::Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    spells: Option<Res<Spells>>,
    spell_mods: Res<crate::spell::SpellModifiers>,
    range_units: crate::spell::RangeUnits,
    selection: Res<Selection>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some((store, me)) = self_q.iter().next() else {
        return;
    };
    let target = selection.guid.and_then(|g| objects.entity(g));
    let dist_sq = target
        .and_then(|e| units.get(e).ok())
        .map(|t| me.translation.distance_squared(t.translation));
    let mut found = HashMap::new();
    for quest_id in quest_log.quest_ids() {
        let Some(t) = quest_log.template(quest_id, &commands) else {
            continue;
        };
        let drops = t.flags & QUEST_FLAG_DROP_ITEM_USE != 0;
        if t.src_item_id == 0 && !drops {
            continue;
        }
        let mut candidates = vec![t.src_item_id];
        if drops {
            candidates.extend(t.objectives.iter().map(|o| o.item_drop));
        }
        let hit = candidates.into_iter().filter(|&id| id != 0).find_map(|id| {
            let use_spell = items.template(id, 0, &commands)?.use_spell?;
            let at = crate::ui_items::find_carried_at(&store.0, &objects, |guid| {
                objects
                    .object(guid)
                    .and_then(|o| o.object_entry())
                    .is_some_and(|e| e == id)
            })?;
            Some((at, use_spell.spell_id))
        });
        let Some(((bag, slot, guid), spell_id)) = hit else {
            continue;
        };
        let charges = objects
            .object(guid)
            .and_then(|o| o.item_spell_charges(0))
            .unwrap_or(0);
        let in_range = spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id).map(|d| (s, d)))
            .and_then(|(s, d)| {
                let d2 = dist_sq?;
                let row = s.ranges.get(d.range_index);
                let targets = RangeTargets {
                    target: target.and_then(|e| range_units.unit(e)),
                    attack_target: None,
                };
                spell_mods
                    .min_max_range(d, row, range_units.caster(), targets)
                    .map(|(min, max)| d2 >= min * min && d2 <= max * max)
            });
        found.insert(
            quest_id,
            QuestSpecialItemView {
                bag,
                slot,
                charges,
                in_range,
            },
        );
    }
    script.set_quest_special_items(found);
}
