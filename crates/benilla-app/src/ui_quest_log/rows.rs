//! The log's row table (`0x5e6940`) written into the VM, and its per-row answers read at the call.

use std::collections::HashSet;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::messages::{quest_flags, quest_slot_state, PLAYER_QUEST_LOG_SLOTS};
use benilla_ui::script::{
    QuestLogDetail, QuestLogEntryView, QuestLogPlan, QuestLogRow, ScriptValue, UiScript,
};

use super::{
    abandon_items, build_detail, build_objectives, completion_text, no_objectives, order_groups,
    poi_objectives, quest_complete, quest_level, timer_run_out, CompletionInputs, GroupRow,
    ObjectiveExtras, QuestExpiries, QuestHeaderNamesRes, QuestLog, QuestLogMarks, QuestTagNamesRes,
    MISSING_HEADER,
};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_items::{count_of, InventoryScope};
use crate::ui_script::live::{UiEvents, UniqueSignals};

/// A cache record a quest verb asked for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum QuestAsk {
    Quest(u32),
    Creature(u32),
    GameObject(u32),
    Item(u32),
}

/// The records the quest verbs asked for, each answer queuing `QUEST_LOG_UPDATE` (`0x5de94a`, `0x5de9fa`, `0x5dea2a`, `0x5dea49`).
#[derive(Resource, Default)]
pub(crate) struct QuestAsks(std::sync::Mutex<HashSet<QuestAsk>>);

impl QuestAsks {
    fn set(&self) -> std::sync::MutexGuard<'_, HashSet<QuestAsk>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Note a miss a verb's lookup asked for.
    pub(crate) fn asked(&self, ask: QuestAsk) {
        self.set().insert(ask);
    }

    /// Whether a verb waited on the record that just landed.
    pub(crate) fn landed(&self, ask: QuestAsk) -> bool {
        self.set().remove(&ask)
    }

    /// Disconnect: drops every ask.
    pub(crate) fn clear(&self) {
        self.set().clear();
    }
}

/// The rebuild's own state, behind a lock so the rebuild also runs inside a verb's call (`0x5e6e00`).
#[derive(Resource, Default)]
pub(crate) struct RebuildMarks(std::sync::Mutex<RebuildState>);

#[derive(Default)]
struct RebuildState {
    /// The templates the last rebuild missed (`0x5e6a98`); every rebuild skips while one is (`0x5e6973`).
    pending: HashSet<u32>,
    /// The last rebuild's sort types in their sorted order (`0x5e6b47`), which the collapse filter's bits follow.
    sort_types: Vec<i32>,
    /// The quest ids of the last rebuild's quest rows (`0x5e6ad6`), which the stack watch walks (`0x5e1270`).
    quest_rows: Vec<u32>,
}

impl RebuildMarks {
    fn state(&self) -> std::sync::MutexGuard<'_, RebuildState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Disconnect: drops every mark.
    pub(crate) fn clear(&self) {
        *self.state() = RebuildState::default();
    }

    /// The world entry zeroes the pending count (`0x5e6fde`).
    fn clear_pending(&self) {
        self.state().pending.clear();
    }

    /// A template answer's count down (`0x5de913`); whether it was the last pending one (`0x5de91d`).
    fn landed_last(&self, quest_id: u32) -> bool {
        let mut st = self.state();
        st.pending.remove(&quest_id) && st.pending.is_empty()
    }
}

/// What the per-row verbs read at the call: our slots, the caches and the bags, none sampled.
#[derive(SystemParam)]
pub(crate) struct QuestRowLive<'w, 's> {
    me: Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    quest_log: Option<Res<'w, QuestLog>>,
    expiries: Option<Res<'w, QuestExpiries>>,
    asks: Option<Res<'w, QuestAsks>>,
    objects: Option<crate::net::Objects<'w, 's>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    net: Option<Res<'w, NetCommands>>,
    names: Option<Res<'w, NameCache>>,
    gos: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    tags: Option<Res<'w, QuestTagNamesRes>>,
    states: Option<Res<'w, crate::world_state::WorldStates>>,
    rewards: crate::ui_quest::rewards::RewardInputs<'w, 's>,
    spells: Option<Res<'w, Spells>>,
    rebuild: Option<Res<'w, RebuildMarks>>,
    headers: Option<Res<'w, QuestHeaderNamesRes>>,
    clock: Option<Res<'w, crate::net::ServerWallClock>>,
    signals: Option<Res<'w, UniqueSignals>>,
}

impl QuestRowLive<'_, '_> {
    /// Our quest-item count of `item` at the call, 0 before our create (`0x58cbd5`).
    pub(crate) fn carried(&self, item: u32) -> Option<u32> {
        let objects = self.objects.as_ref()?;
        Some(self.me.iter().next().map_or(0, |(store, _)| {
            count_of(&store.0, objects, item, InventoryScope::QUEST_ITEMS)
        }))
    }

    /// `GetQuestResetTime`'s due re-ask runs the rebuild in the call (`0x5e6dee`-`0x5e6e00`).
    pub(crate) fn rebuild_if_reask_due(&self) -> Option<QuestLogPlan> {
        let clock = self.clock.as_deref()?;
        if !clock.reask_due(std::time::Instant::now()) {
            return None;
        }
        let (Some((store, _)), Some(objects)) = (self.me.iter().next(), self.objects.as_ref())
        else {
            return None;
        };
        plan(&PlanInputs {
            store,
            quest_log: self.quest_log.as_deref()?,
            marks: self.rebuild.as_deref()?,
            expiries: self.expiries.as_deref()?,
            headers: self.headers.as_deref(),
            net: self.net.as_deref()?,
            clock: Some(clock),
            objects,
            rewards: &self.rewards,
            signals: self.signals.as_deref()?,
        })
    }

    /// Note a miss for its answer's `QUEST_LOG_UPDATE`.
    fn miss(&self, ask: QuestAsk) {
        if let Some(asks) = self.asks.as_deref() {
            asks.asked(ask);
        }
    }

    /// A logged quest's answers at the call (`0x5e0000`, `0x5e0070`, `0x5e0d20`, `0x5e0ea0`, `0x5e1460`); `Some(None)` when uncached.
    pub(crate) fn row(
        &self,
        quest_id: u32,
        slot: u8,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<QuestLogEntryView>> {
        let (quest_log, net, items, names) = (
            self.quest_log.as_deref()?,
            self.net.as_deref()?,
            self.items.as_deref()?,
            self.names.as_deref()?,
        );
        let Some((store, _)) = self.me.iter().next() else {
            return Some(None);
        };
        let Some(log_slot) = store
            .0
            .player_quest_log(slot)
            .filter(|s| s.quest_id == quest_id)
        else {
            return Some(None);
        };
        let Some(t) = quest_log.template(quest_id, net) else {
            self.miss(QuestAsk::Quest(quest_id));
            return Some(None);
        };
        let expired = self
            .expiries
            .as_deref()
            .is_some_and(|e| e.is_expired(quest_id));
        let carried = |id: u32| {
            self.objects.as_ref().map_or(0, |o| {
                count_of(&store.0, o, id, InventoryScope::QUEST_ITEMS)
            })
        };
        let standing = |faction: u32| self.rewards.standing(&store.0, faction);
        let completion = CompletionInputs {
            carried: &carried,
            standing: &standing,
            money: u64::from(store.0.player_money().unwrap_or(0)),
        };
        let done = quest_complete(t, &log_slot, expired, &completion);
        let complete = if log_slot.state & quest_slot_state::FAIL != 0 || expired {
            -1
        } else {
            i32::from(done)
        };
        let default_states;
        let states = match self.states.as_deref() {
            Some(s) => s,
            None => {
                default_states = crate::world_state::WorldStates::default();
                &default_states
            }
        };
        let player = crate::npc_text::player_identity(&self.me, names, net);
        let macros = crate::npc_text::MacroContext {
            subject: player.as_ref(),
            states,
            achievement: None,
        };
        let miss = |ask| self.miss(ask);
        let objectives = build_objectives(
            t,
            &log_slot,
            done,
            &carried,
            items,
            names,
            net,
            get,
            &ObjectiveExtras {
                gos: self.gos.as_deref(),
                reputation: self.rewards.rep_objective(&store.0, t),
                female: store.0.unit_gender() == Some(1),
                objectives_text: crate::npc_text::substitute(&t.objectives_text, &macros),
            },
            &miss,
        );
        // Every logged quest's cached template, for the abandon items' claims.
        let logged: Vec<_> = (0..PLAYER_QUEST_LOG_SLOTS)
            .filter_map(|s| store.0.player_quest_log(s))
            .filter(|s| s.quest_id != 0)
            .filter_map(|s| quest_log.templates.get(s.quest_id))
            .collect();
        let item_info = |id: u32| {
            let info = items
                .template(id, 0, net)
                .map(|t| (t.name.clone(), t.bonding));
            if info.is_none() {
                miss(QuestAsk::Item(id));
            }
            info
        };
        let player_level = store.0.unit_level().unwrap_or(0);
        Some(Some(QuestLogEntryView {
            quest_id,
            title: t.title.clone(),
            level: quest_level(t, player_level),
            // The tag: `Type` is a `QuestInfo.dbc` id, 0 naming none; no table gives `None`.
            tag: self
                .tags
                .as_ref()
                .and_then(|tags| tags.0.resolve(t.quest_type))
                .map(str::to_string),
            complete,
            no_objectives: no_objectives(t, &log_slot, expired),
            abandon_items: abandon_items(t, &logged, &carried, &item_info),
            // `QUEST_FLAGS_SHARABLE` is the client's test alone; the server checks it at the accept.
            pushable: t.flags & quest_flags::SHARABLE != 0,
            suggested_group: t.suggested_players,
            is_daily: t.flags & quest_flags::DAILY != 0,
            // Absolute unix seconds, 0 untimed; the verbs subtract the server clock.
            timer: log_slot.timer,
            objectives,
            rep_objective: t.rep_objective_faction != 0,
            detail: None,
            completion_text: completion_text(t, &macros),
            poi_objectives: self
                .objects
                .as_ref()
                .map_or(0, |o| poi_objectives(t, &log_slot, &store.0, o)),
            ..Default::default()
        }))
    }

    /// A logged quest's detail pane at the call; `Some(None)` when its slot or template is missing.
    pub(crate) fn detail(&self, quest_id: u32, slot: u8) -> Option<Option<QuestLogDetail>> {
        let (quest_log, net, items, names) = (
            self.quest_log.as_deref()?,
            self.net.as_deref()?,
            self.items.as_deref()?,
            self.names.as_deref()?,
        );
        let Some((store, _)) = self.me.iter().next() else {
            return Some(None);
        };
        if store
            .0
            .player_quest_log(slot)
            .is_none_or(|s| s.quest_id != quest_id)
        {
            return Some(None);
        }
        let Some(t) = quest_log.template(quest_id, net) else {
            self.miss(QuestAsk::Quest(quest_id));
            return Some(None);
        };
        let default_states;
        let states = match self.states.as_deref() {
            Some(s) => s,
            None => {
                default_states = crate::world_state::WorldStates::default();
                &default_states
            }
        };
        let player = crate::npc_text::player_identity(&self.me, names, net);
        let macros = crate::npc_text::MacroContext {
            subject: player.as_ref(),
            states,
            achievement: None,
        };
        let miss = |ask| self.miss(ask);
        Some(Some(QuestLogDetail {
            // At the level cap the reward is at least the money paid in place of XP (`0x5e48f0`).
            reward_money: {
                let paid = t.money.max(0) as u32;
                match (store.0.unit_level(), store.0.player_max_level()) {
                    (Some(l), Some(m)) if l >= m => paid.max(t.money_max_level),
                    _ => paid,
                }
            },
            extras: self.rewards.for_template(
                t,
                &store.0,
                player.as_ref().map_or("", |p| p.name.as_str()),
                self.spells.as_deref(),
            ),
            ..build_detail(
                t,
                items,
                self.icons.as_deref(),
                net,
                &macros,
                self.spells.as_deref(),
                &miss,
            )
        }))
    }

    /// The daily-quest fields holding a quest (`0x58db30`), `None` before our create.
    pub(crate) fn daily_completed(&self) -> Option<u32> {
        let (store, _) = self.me.iter().next()?;
        Some(store.0.player_daily_quests_completed())
    }
}

/// What the rebuild reads: our slots, the caches, the bags and standings, the clock.
struct PlanInputs<'a, 'w, 's> {
    store: &'a ObjectStore,
    quest_log: &'a QuestLog,
    marks: &'a RebuildMarks,
    expiries: &'a QuestExpiries,
    headers: Option<&'a QuestHeaderNamesRes>,
    net: &'a NetCommands,
    clock: Option<&'a crate::net::ServerWallClock>,
    objects: &'a crate::net::Objects<'w, 's>,
    rewards: &'a crate::ui_quest::rewards::RewardInputs<'w, 's>,
    signals: &'a UniqueSignals,
}

/// The rebuild's walk (`0x5e6940`): skipped while a template is pending (`0x5e6973`), cached quests only (`0x5e6a98`), `QUEST_LOG_UPDATE` queued (`0x5e6cb9`), then a due re-ask sent (`0x5e6d60`-`0x5e6daa`).
fn plan(p: &PlanInputs) -> Option<QuestLogPlan> {
    let PlanInputs {
        store,
        quest_log,
        expiries,
        net,
        objects,
        rewards,
        ..
    } = *p;
    let mut marks = p.marks.state();
    if !marks.pending.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for slot in 0..PLAYER_QUEST_LOG_SLOTS {
        let Some(log_slot) = store.0.player_quest_log(slot).filter(|s| s.quest_id != 0) else {
            continue;
        };
        if quest_log.template(log_slot.quest_id, net).is_none() {
            marks.pending.insert(log_slot.quest_id);
            continue;
        }
        rows.push((slot, log_slot));
    }
    // A timer's expiry mark lives on its quest's row and leaves with it.
    expiries.retain(|id| {
        (0..PLAYER_QUEST_LOG_SLOTS)
            .filter_map(|s| store.0.player_quest_log(s))
            .any(|s| s.quest_id == id)
    });
    let now = p.clock.and_then(|c| c.now_unix());
    let carried = |id: u32| count_of(&store.0, objects, id, InventoryScope::QUEST_ITEMS);
    let standing = |faction: u32| rewards.standing(&store.0, faction);
    let completion = CompletionInputs {
        carried: &carried,
        standing: &standing,
        money: u64::from(store.0.player_money().unwrap_or(0)),
    };
    for (_, log_slot) in &rows {
        let id = log_slot.quest_id;
        let run_out = !expiries.is_expired(id)
            && timer_run_out(log_slot, now)
            && quest_log
                .templates
                .get(id)
                .is_some_and(|t| !quest_complete(t, log_slot, false, &completion));
        if run_out {
            debug!("ui_quest_log: quest {id}'s timer ran out — expired");
            expiries.mark(id);
            let _ = net
                .0
                .send(crate::net::ClientCommand::QuestgiverStatusMultipleQuery);
        }
    }
    let player_level = store.0.unit_level().unwrap_or(0);
    let keyed: Vec<GroupRow> = rows
        .iter()
        .map(|(slot, log_slot)| {
            let t = quest_log.templates.get(log_slot.quest_id);
            let zos = t.map_or(0, |t| t.zone_or_sort);
            GroupRow {
                zone_or_sort: zos,
                header: if zos == 0 {
                    MISSING_HEADER.to_string()
                } else {
                    p.headers
                        .and_then(|h| h.0.resolve(zos))
                        .unwrap_or_default()
                        .to_string()
                },
                level: t.map_or(0, |t| quest_level(t, player_level)),
                title: t.map(|t| t.title.clone()).unwrap_or_default(),
                slot: *slot,
            }
        })
        .collect();
    let groups = order_groups(&keyed);
    let new_sort_types: Vec<i32> = groups.iter().map(|(zos, _, _)| *zos).collect();
    let mut table = Vec::with_capacity(rows.len() + groups.len());
    for (sort_index, (_, name, idxs)) in groups.iter().enumerate() {
        let sort_index = sort_index as u32;
        table.push(QuestLogRow {
            is_header: true,
            title: name.clone(),
            sort_index,
            ..Default::default()
        });
        for &i in idxs {
            let (slot, log_slot) = &rows[i];
            table.push(QuestLogRow {
                quest_id: log_slot.quest_id,
                slot: *slot,
                sort_index,
                ..Default::default()
            });
        }
    }
    let old_sort_types = std::mem::replace(&mut marks.sort_types, new_sort_types.clone());
    marks.quest_rows = rows.iter().map(|(_, s)| s.quest_id).collect();
    drop(marks);
    p.signals.unique_signal("QUEST_LOG_UPDATE");
    if p.clock
        .is_some_and(|c| c.take_due_reask(std::time::Instant::now()))
    {
        let _ = net.0.send(crate::net::ClientCommand::QueryTime);
    }
    Some(QuestLogPlan {
        rows: table,
        old_sort_types,
        new_sort_types,
    })
}

/// [`plan`] over the `World`'s resources.
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
fn plan_rows(
    me: Query<&ObjectStore, With<SelfPlayer>>,
    quest_log: Res<QuestLog>,
    marks: Res<RebuildMarks>,
    expiries: Res<QuestExpiries>,
    headers: Option<Res<QuestHeaderNamesRes>>,
    net: Res<NetCommands>,
    clock: Option<Res<crate::net::ServerWallClock>>,
    objects: crate::net::Objects,
    rewards: crate::ui_quest::rewards::RewardInputs,
    signals: Res<UniqueSignals>,
) -> Option<QuestLogPlan> {
    plan(&PlanInputs {
        store: me.iter().next()?,
        quest_log: &quest_log,
        marks: &marks,
        expiries: &expiries,
        headers: headers.as_deref(),
        net: &net,
        clock: clock.as_deref(),
        objects: &objects,
        rewards: &rewards,
        signals: &signals,
    })
}

#[cfg(test)]
pub(super) use benilla_ui::script::remap_selection;

/// The rebuild with argument 1 (`0x5e6940`), over the `World` with the VM lent.
pub(crate) fn rebuild(world: &mut World) {
    let plan = match world.run_system_cached(plan_rows) {
        Ok(plan) => plan,
        Err(e) => {
            warn!("ui_quest_log: the rebuild did not run: {e}");
            return;
        }
    };
    if let Some(plan) = plan {
        crate::ui_script::live::with_live_world(world, |script: &mut UiScript| {
            script.apply_quest_log_plan(plan)
        });
    }
}

/// One quest-log slot's `(quest id, state)` before and after a values update.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SlotEdge {
    pub(crate) was: (u32, u8),
    pub(crate) now: (u32, u8),
}

impl SlotEdge {
    /// The watch's accept test (`0x6df3ef`-`0x6df40c`): a new quest in the slot, or its fail bit cleared.
    pub(super) fn takes_quest(&self) -> bool {
        let ((id, state), (was_id, was_state)) = (self.now, self.was);
        id != 0
            && (was_id != id
                || (state & quest_slot_state::FAIL == 0 && was_state & quest_slot_state::FAIL != 0))
    }
}

/// The quest-slot watch (`0x6df370`) per moved slot: the rebuild (`0x6df3bb`), then the accept, or its wait on the template (`0x6df429`).
pub(crate) fn slot_watch(world: &mut World, edges: Vec<SlotEdge>) {
    for edge in edges {
        rebuild(world);
        if !edge.takes_quest() {
            continue;
        }
        let id = edge.now.0;
        let cached = {
            let (Some(log), Some(net)) = (
                world.get_resource::<QuestLog>(),
                world.get_resource::<NetCommands>(),
            ) else {
                continue;
            };
            log.template(id, net).is_some()
        };
        if cached {
            announce_accept(world, id);
        } else if let Some(mut marks) = world.get_resource_mut::<QuestLogMarks>() {
            marks.accepts.insert(id);
        }
    }
}

/// The accept's line and tutorials, then `QUEST_ACCEPTED` with the quest's row (`0x6df440`-`0x6df49c`).
pub(crate) fn announce_accept(world: &mut World, quest_id: u32) {
    if let Err(e) = world.run_system_cached_with(crate::ui_quest::lines::accept_line, quest_id) {
        warn!("ui_quest_log: the accept line did not run: {e}");
    }
    crate::ui_script::live::fire_handler_events(world);
    crate::ui_script::live::with_live_world(world, |script| {
        let index = script.quest_log_index(quest_id) as i64;
        script.fire_event("QUEST_ACCEPTED", vec![ScriptValue::Int(index)]);
    });
}

/// The world entry's log part (`0x5e6fa0`): the pending count zeroed (`0x5e6fde`), then the rebuild (`0x5e6fe4`).
pub(crate) fn queue_enter_world(commands: &mut Commands) {
    commands.queue(|world: &mut World| {
        if let Some(marks) = world.get_resource::<RebuildMarks>() {
            marks.clear_pending();
        }
        rebuild(world);
        crate::ui_script::live::fire_handler_events(world);
    });
}

/// A template answer's callbacks: a verb's ask (`0x5de94a`), a pending accept (`0x6df2b0`), the rebuild's counter (`0x5de921`).
pub(super) fn template_landed(
    quest_id: u32,
    marks: &mut QuestLogMarks,
    rebuild_marks: &RebuildMarks,
    asks: &QuestAsks,
    signals: &UniqueSignals,
    ui: &mut UiEvents,
) {
    if asks.landed(QuestAsk::Quest(quest_id)) {
        signals.unique_signal("QUEST_LOG_UPDATE");
    }
    // The rebuild's ask (`0x6df3bb`) was made before the accept's (`0x6df429`), so its callback runs first and `QUEST_ACCEPTED` reads the rebuilt rows.
    if rebuild_marks.landed_last(quest_id) {
        ui.world(rebuild);
    }
    if marks.accepts.remove(&quest_id) {
        ui.world(move |world| announce_accept(world, quest_id));
    }
}

/// The stack watch (`0x5e1250`): a quest row whose template requires the item queues `QUEST_LOG_UPDATE` (`0x5e12bc`) and, with our player in, re-asks every giver's status (`0x5e12e5`). The header rows' sort types, which the walk also looks up as quest ids (`0x5e1285`), are not.
pub(crate) fn item_moved(
    entry: u32,
    me: bool,
    log: &QuestLog,
    marks: &RebuildMarks,
    net: &NetCommands,
    signals: &UniqueSignals,
) {
    let required = entry != 0
        && marks
            .state()
            .quest_rows
            .iter()
            .filter_map(|&id| log.templates.get(id))
            .any(|t| t.required_items.iter().any(|&(id, _)| id == entry));
    if !required {
        return;
    }
    signals.unique_signal("QUEST_LOG_UPDATE");
    if me {
        let _ = net
            .0
            .send(crate::net::ClientCommand::QuestgiverStatusMultipleQuery);
    }
}
