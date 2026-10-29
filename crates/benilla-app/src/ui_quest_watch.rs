//! The objective tracker's feeds: the quest POI wire (`CMSG_QUEST_POI_QUERY` for the quests the VM
//! holds no POI for, `SMSG_QUEST_POI_QUERY_RESPONSE` into its store) and each quest's special item
//! (`0x5e0da0`), found in the bags with its charges and range.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::RangeTargets;
use benilla_protocol::messages::QuestPoi;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{QuestPoiRegion, QuestPoiView, QuestSpecialItemView, UiScript};

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
                (feed_quest_pois, feed_special_items)
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
                }
            })
            .collect(),
    }
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
