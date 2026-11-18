//! The objective tracker's feeds: `SMSG_QUEST_POI_QUERY_RESPONSE` into the VM's store from the handler, and the live reads of each quest's special item (`0x5e0da0`), found in the bags with its charges and range at call time.

use bevy::prelude::*;

use benilla_protocol::messages::QuestPoi;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    QuestLogObjectiveView, QuestPoiLines, QuestPoiRegion, QuestPoiView, QuestSpecialItemView,
};

use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp, ObjectStore, SelfPlayer};
use crate::ui_quest_log::QuestLog;
use crate::ui_script::live::{UiEvents, UniqueSignals};

/// A quest template's flag that lets its drop items serve as the special item (`0x5e0df9`).
const QUEST_FLAG_DROP_ITEM_USE: u32 = 0x20000;

pub(crate) struct UiQuestWatchPlugin;

impl Plugin for UiQuestWatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_non_send_resource::<UniqueSignals>()
            .net_handler(SessionEventKind::QuestPoi, on_quest_poi);
    }
}

/// `SMSG_QUEST_POI_QUERY_RESPONSE` (`0x5e7370`): the store takes each quest in turn with the handler's events, and `QUEST_POI_UPDATE` waits as a unique signal (`0x5e7628`).
fn on_quest_poi(
    In(ev): In<SessionEvent>,
    mut ui: ResMut<UiEvents>,
    mut signals: NonSendMut<UniqueSignals>,
) {
    if let SessionEvent::QuestPoi(pois) = ev {
        debug!("ui_quest_watch: POI for {} quest(s)", pois.len());
        let pois: Vec<QuestPoiView> = pois.into_iter().map(view).collect();
        ui.write(move |script| script.apply_quest_pois(pois));
        signals.unique_signal("QUEST_POI_UPDATE");
    }
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
                    outline: b.points,
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
        intermediate: false,
    }
}

/// One logged quest's POI lines at the call (`0x5e58c0`, `0x5e20a0`, `0x5e1ff0`): the objectives by entry with their raw counters (`0x5e2370`), the items by id with the bag count capped (`0x5e2630`), the drops, and the header zone's top area; `None` for a quest not logged or not cached.
fn poi_lines(
    v: &QuestView<'_, '_>,
    quest_id: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<QuestPoiLines> {
    use crate::ui_items::{count_of, InventoryScope};
    let (quest_log, objects, items, commands) = (
        v.quest_log.as_deref()?,
        v.objects.as_ref()?,
        v.items.as_deref()?,
        v.net.as_deref()?,
    );
    let store = v.me.iter().next()?;
    if quest_id == 0 {
        return None;
    }
    let log = (0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS)
        .filter_map(|slot| store.0.player_quest_log(slot))
        .find(|l| l.quest_id == quest_id)?;
    let t = quest_log.template(log.quest_id, commands)?;
    let bags = |item: u32| count_of(&store.0, objects, item, InventoryScope::QUEST_ITEMS);
    let item_name = |item: u32| {
        items
            .template(item, 0, commands)
            .map_or_else(|| NAME_PLACEHOLDER.to_string(), |t| t.name.clone())
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
            poi_line(get, "QUEST_OBJECTS_FOUND", kind, &o.text, count)
        } else if is_go {
            let name = v
                .gos
                .as_deref()
                .and_then(|g| g.name_by_entry(entry & !GO_OBJECTIVE_BIT, commands))
                .unwrap_or(NAME_PLACEHOLDER);
            poi_line(get, "QUEST_OBJECTS_FOUND", kind, name, count)
        } else {
            let name = v
                .names
                .as_deref()
                .and_then(|n| n.resolve_creature(entry, 0, commands))
                .unwrap_or(NAME_PLACEHOLDER);
            poi_line(get, "QUEST_MONSTERS_KILLED", kind, name, count)
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
            get,
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
            intermediate: false,
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
                get,
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
        .and_then(|z| v.areas.as_ref()?.0.top_area(z))
        .unwrap_or(0);
    Some(QuestPoiLines {
        board,
        drops,
        drops_needed,
        top_area,
    })
}

/// What the special-item verbs read at call time: the quest's template, our bags and the item instances.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct QuestView<'w, 's> {
    quest_log: Option<Res<'w, QuestLog>>,
    /// Our store, for the bag walk.
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: Option<crate::net::Objects<'w, 's>>,
    items: Option<Res<'w, Items>>,
    net: Option<Res<'w, NetCommands>>,
    /// The objectives' creature and GameObject names, and the header zones' top areas.
    names: Option<Res<'w, crate::names::NameCache>>,
    gos: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    areas: Option<Res<'w, crate::sound::AreaSounds>>,
    /// The special item's inventory art (`0x70aa00`) and its link's suffix name (`0x61e3a0`).
    icons: Option<Res<'w, crate::entities::ItemDisplays>>,
    rolls: Option<Res<'w, crate::items::RandomProperties>>,
}

impl QuestView<'_, '_> {
    /// `GetQuestTimers`' expiry inside the call ([`crate::ui_quest_log::QuestExpiries::expire_in_call`]).
    pub(crate) fn expire_quest(
        &self,
        quest_id: u32,
        expiries: &mut crate::ui_quest_log::QuestExpiries,
    ) -> Option<Option<String>> {
        let (log, net) = (self.quest_log.as_deref()?, self.net.as_deref()?);
        Some(expiries.expire_in_call(quest_id, log, net).flatten())
    }

    /// A questgiver panel row's item from the cache at the call (`0x58d190`), a miss asked; its answer fires `QUEST_ITEM_UPDATE` (`0x58b92d`).
    pub(crate) fn quest_item_record(
        &self,
        item_id: u32,
    ) -> Option<Option<benilla_ui::script::QuestItemRecord>> {
        let (items, net) = (self.items.as_deref()?, self.net.as_deref()?);
        Some(
            items
                .template(item_id, 0, net)
                .map(|t| benilla_ui::script::QuestItemRecord {
                    name: t.name.clone(),
                    quality: t.quality,
                    link: crate::ui_items::item_link(item_id, &t.name, t.quality),
                }),
        )
    }

    /// One logged quest's POI lines at the call, formatted through `get`.
    pub(crate) fn quest_poi_lines(
        &self,
        quest_id: u32,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<QuestPoiLines> {
        poi_lines(self, quest_id, get)
    }

    /// The quest's special item (`0x5e0da0`): the template's starting item, else under flag `0x20000` its drop items in order (`0x5e0df9`), the first carried copy that can be used (`0x708c10`), with the bag slot it sits in, its charges and its on-use spell. A template or item record not cached is asked here, as the reference's cache lookup with a callback asks it (`0x5e0da0`), and reads as no item until it lands.
    pub(crate) fn quest_special_item(&self, quest_id: u32) -> Option<QuestSpecialItemView> {
        let (quest_log, objects, items, net) = (
            self.quest_log.as_deref()?,
            self.objects.as_ref()?,
            self.items.as_deref()?,
            self.net.as_deref()?,
        );
        let store = self.me.iter().next()?;
        let t = quest_log.template(quest_id, net)?;
        let drops = t.flags & QUEST_FLAG_DROP_ITEM_USE != 0;
        if t.src_item_id == 0 && !drops {
            return None;
        }
        let mut candidates = vec![t.src_item_id];
        if drops {
            candidates.extend(t.objectives.iter().map(|o| o.item_drop));
        }
        let ((bag, slot, guid), id, t) =
            candidates
                .into_iter()
                .filter(|&id| id != 0)
                .find_map(|id| {
                    let t = items.template(id, 0, net)?;
                    t.use_spell?;
                    let at = crate::ui_items::find_carried_at(&store.0, objects, |guid| {
                        objects
                            .object(guid)
                            .and_then(|o| o.object_entry())
                            .is_some_and(|e| e == id)
                    })?;
                    Some((at, id, t))
                })?;
        let item = objects.object(guid)?;
        let rolls = crate::items::RollCatalogs {
            props: self.rolls.as_deref(),
            enchants: None,
        };
        // The link from the instance (`0x5e533d`) and the art from its entry (`0x5e536a`), read at the call like the charges (`0x5e539d`).
        let link = crate::ui_items::item_link_full(
            id,
            &crate::ui_items::object_link_fields(item, t.flags),
            &rolls.name(&t.name, item.item_random_properties_id()),
            t.quality,
        );
        let texture = self.icons.as_deref().map_or_else(
            || crate::entities::FALLBACK_ITEM_ICON.to_string(),
            |i| i.carried_icon(id),
        );
        Some(QuestSpecialItemView {
            bag,
            slot,
            charges: item.item_spell_charges(0).unwrap_or(0),
            spell_id: t.use_spell.map_or(0, |u| u.spell_id),
            link,
            texture,
        })
    }
}

/// What the special item's range verb reads at the call (`0x5e54c0`): the spell, our modifiers and the range facts of us and the unit, none of them sampled.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ItemRangeLive<'w, 's> {
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    facts: crate::spell::cast_target::RangeFactsView<'w, 's>,
}

impl ItemRangeLive<'_, '_> {
    /// The item's spell through the range check toward `target` (`0x5e55b6`), `None` with no answer.
    pub(crate) fn in_range(&self, spell_id: u32, target: u64) -> Option<bool> {
        let spells = self.spells.as_deref()?;
        let def = spells.catalog.get(spell_id)?;
        self.facts.player_range_check(
            def,
            spells.ranges.get(def.range_index),
            self.mods.as_deref()?,
            target,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::collections::HashMap;

    const ME: u64 = 1;
    const MOB: u64 = 0xF130_0000_0100_0002;
    const SPELL: u32 = 7;

    fn unit_store() -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x19),
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
        ]))
    }

    /// Us at the origin and a living unit `yards` away, and an item spell that takes any unit at 0-20 yd.
    fn world(yards: f32) -> (World, Entity) {
        let mut world = World::new();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([(
            SPELL,
            benilla_formats::SpellDisplay {
                range_index: 1,
                effect_implicit_target_a: [25, 0, 0],
                ..Default::default()
            },
        )]));
        spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([(
            1,
            benilla_formats::SpellRange {
                min: 0.0,
                max: 20.0,
                friendly_min: 0.0,
                friendly_max: 20.0,
                flags: 0,
            },
        )]));
        world.insert_resource(spells);
        world.insert_resource(crate::spell::SpellModifiers::default());
        world.init_resource::<crate::net::Reputations>();
        world.insert_resource(crate::net::SelfGuid(Some(ME)));
        let me = world
            .spawn((SelfPlayer, unit_store(), Transform::default()))
            .id();
        let mob = world
            .spawn((unit_store(), Transform::from_xyz(yards, 0.0, 0.0)))
            .id();
        let mut index = crate::net::GuidIndex::default();
        index.0.insert(ME, me);
        index.0.insert(MOB, mob);
        world.insert_resource(index);
        (world, mob)
    }

    fn in_range(world: &mut World, target: u64) -> Option<bool> {
        world
            .run_system_once(move |live: ItemRangeLive| live.in_range(SPELL, target))
            .expect("the range view runs")
    }

    /// The special item's range (`0x5e55b6` -> `0x809610`) reads where the unit stands at each call: a move between two calls in one frame answers the new place, and a guid no unit holds answers nil.
    #[test]
    fn the_item_range_reads_the_unit_where_it_stands_at_each_call() {
        let (mut world, mob) = world(10.0);
        assert_eq!(in_range(&mut world, MOB), Some(true), "10 yd is inside 20");
        world.get_mut::<Transform>(mob).unwrap().translation.x = 30.0;
        assert_eq!(
            in_range(&mut world, MOB),
            Some(false),
            "30 yd at the next call"
        );
        world.get_mut::<Transform>(mob).unwrap().translation.x = 19.0;
        assert_eq!(in_range(&mut world, MOB), Some(true));
        assert_eq!(in_range(&mut world, 0x99), None, "nobody holds the guid");
    }

    /// Us with quest 33 in slot 0 at `kills` of 8 wolves, its template cached, the wolf's name `wolf` when known.
    fn poi_world(kills: u16, wolf: Option<&str>) -> World {
        use benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1;
        let mut world = World::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<Items>();
        let mut names = crate::names::NameCache::default();
        if let Some(name) = wolf {
            names.insert_creature(
                299,
                Some(crate::names::CreatureRecord {
                    name: name.into(),
                    ..Default::default()
                }),
            );
        }
        world.insert_resource(names);
        let mut log = QuestLog::default();
        let mut t = benilla_protocol::messages::QuestTemplate {
            quest_id: 33,
            end_text: "Return to Eagan.".into(),
            ..Default::default()
        };
        t.objectives[0].creature_or_go = 299;
        t.objectives[0].required_count = 8;
        log.insert_template(t);
        world.insert_resource(log);
        let me = world
            .spawn((
                SelfPlayer,
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (FIELD_PLAYER_QUEST_LOG_1_1, 33),
                    (FIELD_PLAYER_QUEST_LOG_1_1 + 2, u32::from(kills)),
                ])),
            ))
            .id();
        world.insert_resource(crate::net::GuidIndex([(ME, me)].into()));
        world
    }

    fn poi_board(world: &mut World, quest: u32) -> Option<Vec<Option<String>>> {
        world
            .run_system_once(move |v: QuestView| {
                let get = |key: &str| {
                    (key == "QUEST_MONSTERS_KILLED").then(|| "%s slain: %d/%d".to_string())
                };
                v.quest_poi_lines(quest, &get)
                    .map(|l| l.board.into_iter().map(|b| b.map(|b| b.text)).collect())
            })
            .expect("the quest view runs")
    }

    /// The POI leaderboard builds at the call (`0x5e58c0`) from the slot's counter and the name cache as they stand: a counter moved between two calls reads the new count, a name in flight the placeholder, and a quest not logged no lines.
    #[test]
    fn the_poi_lines_read_the_slot_and_the_names_at_each_call() {
        let mut world = poi_world(3, None);
        let board = poi_board(&mut world, 33).expect("logged and cached");
        assert_eq!(board[0].as_deref(), Some("... slain: 3/8"));
        assert_eq!(
            board.last().cloned().flatten().as_deref(),
            Some("Return to Eagan.")
        );
        let mut world = poi_world(5, Some("Diseased Wolf"));
        assert_eq!(
            poi_board(&mut world, 33).unwrap()[0].as_deref(),
            Some("Diseased Wolf slain: 5/8")
        );
        let me = world.resource::<crate::net::GuidIndex>().0[&ME];
        world.get_mut::<ObjectStore>(me).unwrap().0.merge(
            benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1 + 2,
                6,
            )]),
        );
        assert_eq!(
            poi_board(&mut world, 33).unwrap()[0].as_deref(),
            Some("Diseased Wolf slain: 6/8"),
            "the next call reads the moved counter"
        );
        assert_eq!(poi_board(&mut world, 34), None, "not logged");
    }
}
