//! The client's combat log (`0xadb974`) in the ECS: each entry is allocated and named at its
//! producer's step (`0x750400`), queued (`0x74ff20`) and, once nothing waits ahead of it, sent
//! inside that step (`0x74f910`): `COMBAT_LOG_EVENT` when the filters pass it,
//! `COMBAT_LOG_EVENT_UNFILTERED`, then the combat-log file's line. An entry that waits on a name
//! holds every later one until the name's answer runs its callback (`0x74fe90`).

use benilla_ui::script::{CombatLogEntry, CombatLogStore, UiScript};
use bevy::prelude::*;

use super::entries::{self, CombatLogClock, NameRef, NameSources, NewEntry};
use super::producers::LogCtx;
use crate::names::NameKey;

/// The store and the name callbacks its entries wait on (`0xadb950`), in ask order.
#[derive(Resource, Default)]
pub(crate) struct CombatLog {
    pub(crate) store: CombatLogStore,
    waits: Vec<Wait>,
    next_wait: u32,
}

/// One name an entry waits on (`0x74f350`): the entry, its mask bit and the record asked.
#[derive(Clone, Copy, Debug)]
struct Wait {
    id: u32,
    entry: u64,
    mask: u8,
    key: NameRef,
    guid: u64,
}

/// A login's reset (`0x756bd0` -> `0x7538a0`): the entries, the cursors, the filters, the name
/// callbacks and the combat-text unit cleared, and the timestamp base taken now.
pub(crate) fn reset(world: &mut World) {
    let at_ms = world
        .get_resource::<Time<bevy::time::Real>>()
        .map_or(0, |t| t.elapsed().as_millis() as u64);
    world.insert_resource(CombatLog::default());
    world.insert_resource(CombatLogClock::starting(at_ms));
    if let Some(mut waits) = world.get_non_send_resource_mut::<crate::items::RecordWaits>() {
        waits.cancel(|w| matches!(w, crate::items::RecordWait::CombatLog(_)));
    }
}

/// `0x750400` then `0x74ff20` for `new` at its producer's step: the entry allocated, its flags
/// and names read now, linked at the tail, and sent at once when it is the head and names all.
pub(crate) fn queue(world: &mut World, new: NewEntry) {
    let Ok((entry, asks)) = world.run_system_cached_with(build, new) else {
        return;
    };
    let now = now_ms(world.get_resource::<Time<bevy::time::Real>>());
    let retention_ms = world
        .get_non_send_resource::<UiScript>()
        // Without a VM there is no CVar to read, and nothing reads the log: it keeps every entry.
        .map_or(i64::MAX, |s| s.combat_log_retention_secs() * 1000);
    let mut record_waits = world.remove_non_send_resource::<crate::items::RecordWaits>();
    let send_now = {
        let Some(mut log) = world.get_resource_mut::<CombatLog>() else {
            if let Some(waits) = record_waits {
                world.insert_non_send_resource(waits);
            }
            return;
        };
        // A free entry, else the oldest once past the retention time (`0x750400`).
        if let Some(gone) = log.store.allocate(now, retention_ms) {
            cancel(&mut log, record_waits.as_mut(), &[gone]);
        }
        let waiting = asks.iter().fold(0, |m, a| m | a.0);
        let (id, head) = log.store.link(entry, waiting);
        for (mask, key, guid) in asks {
            let wait = log.next_wait;
            log.next_wait = log.next_wait.wrapping_add(1);
            log.waits.push(Wait {
                id: wait,
                entry: id,
                mask,
                key,
                guid,
            });
            if let (NameRef::Item(item), Some(waits)) = (key, record_waits.as_mut()) {
                waits.wait(item, crate::items::RecordWait::CombatLog(wait), false);
            }
        }
        head && waiting == 0
    };
    if let Some(waits) = record_waits {
        world.insert_non_send_resource(waits);
    }
    if send_now {
        send(world);
    }
}

/// The client clock now, the producer's frame time (`[0xcd76ac]`).
fn now_ms(time: Option<&Time<bevy::time::Real>>) -> u64 {
    time.map_or(0, |t| t.elapsed().as_millis() as u64)
}

/// One name an entry asked: its mask bit, the record and the guid.
type Ask = (u8, NameRef, u64);

/// The entry's flags and names read at its step (`0x74dcb0`, `0x74fd40`, `0x750210`), and the
/// names still asked.
fn build(
    In(new): In<NewEntry>,
    ctx: LogCtx,
    sources: NameSources,
    clock: Option<Res<CombatLogClock>>,
    time: Option<Res<Time<bevy::time::Real>>>,
) -> (CombatLogEntry, Vec<Ask>) {
    let now = now_ms(time.as_deref());
    let inputs = ctx.flag_inputs();
    let store_of = |g| ctx.store(g);
    let mut asks: Vec<Ask> = Vec::new();
    // The name lookup (`0x74fd40`): the name now, or `UNKNOWNOBJECT` and a callback on its record.
    let mut end = |guid: u64, mask: u8| -> benilla_ui::script::CombatLogUnit {
        if guid == 0 {
            return benilla_ui::script::CombatLogUnit::none();
        }
        let key = if entries::family(guid) == entries::GAME_OBJECT {
            Some(NameRef::GameObject)
        } else {
            NameKey::of(guid, store_of(guid)).map(NameRef::Unit)
        };
        let name = match key {
            Some(key) => match sources.answer(key, guid) {
                Some(name) => name,
                None => {
                    asks.push((mask, key, guid));
                    None
                }
            },
            None => None,
        };
        benilla_ui::script::CombatLogUnit {
            guid,
            name,
            flags: entries::unit_flags(guid, &inputs, store_of),
        }
    };
    let source = end(if new.hide_source { 0 } else { new.source }, 1);
    let dest = end(new.dest, 2);
    let mut info = new.info;
    // The item's name off the item cache (`0x750210`), its callback on mask 4.
    if let Some(item) = info.item.as_mut() {
        match sources.answer(NameRef::Item(item.id), 0) {
            Some(name) => item.name = name,
            None => asks.push((4, NameRef::Item(item.id), 0)),
        }
    }
    // The queue's time: now, less a batched entry's offset (`0x74ff23`-`0x74ff31`).
    let time_ms = now.saturating_add_signed(-i64::from(new.backdate_ms));
    let clock = clock.as_deref().copied().unwrap_or_default();
    let mut entry = CombatLogEntry {
        time_ms,
        timestamp: clock.timestamp(time_ms),
        event: new.event,
        source,
        dest,
        spell: new.spell,
        info,
    };
    // An entry named in full shows its creature guids sanitized before it sends (`0x74ff54`,
    // `0x74ff5f`). 12340 bug: one queued behind a waiting entry skips it (`0x74ff40` returns
    // first, `0x74fa25` sends it as is); every entry is meant to show sanitized, so it is here.
    if asks.is_empty() {
        sanitize(&mut entry, &sources);
    }
    (entry, asks)
}

/// `0x750270` for the recycled entries: their name callbacks and item record waits go.
fn cancel(log: &mut CombatLog, record_waits: Option<&mut crate::items::RecordWaits>, gone: &[u64]) {
    let ids: Vec<u32> = log
        .waits
        .iter()
        .filter(|w| gone.contains(&w.entry))
        .map(|w| w.id)
        .collect();
    if let Some(waits) = record_waits {
        waits.cancel(|w| matches!(w, crate::items::RecordWait::CombatLog(id) if ids.contains(&id)));
    }
    log.waits.retain(|w| !gone.contains(&w.entry));
}

/// `0x74d210` on both endpoints: a creature with template type flag `0x4000` shows no spawn
/// counter.
fn sanitize(entry: &mut CombatLogEntry, sources: &NameSources) {
    entry.source.guid = sources.shown_guid(entry.source.guid);
    entry.dest.guid = sources.shown_guid(entry.dest.guid);
}

/// The send loop (`0x74f910`): from the pending entry on, each entry's events fire with
/// the `World` lent, then its file line while `LoggingCombat` is on (the flag read once,
/// `0x74f922`), until the log ends or an entry still waits on a name.
pub(crate) fn send(world: &mut World) {
    let logging = world
        .get_non_send_resource::<UiScript>()
        .is_some_and(|s| s.logging_flags().1);
    loop {
        let unknown = world
            .get_non_send_resource::<UiScript>()
            .map(|s| s.unknown_object_text());
        let Some(log) = world.get_resource::<CombatLog>() else {
            return;
        };
        let Some(e) = log.store.pending().and_then(|id| log.store.get(id)) else {
            return;
        };
        if let Some(unknown) = unknown {
            // `0x74e290`'s values, the filter read at the push (`0x74f95e`).
            let (args, passes) = (e.event_args(&unknown), log.store.passes(e));
            crate::ui_script::live::with_live_world(world, |script| {
                if passes {
                    script.fire_event("COMBAT_LOG_EVENT", args.clone());
                }
                script.fire_event("COMBAT_LOG_EVENT_UNFILTERED", args);
            });
        }
        // The pending entry read again past the listeners (`0x74f9e3`, `0x74fa09`). 12340 bug: a
        // listener's `CombatLogClearEntries` nulls it (`0x7503a8`) and 12340 dereferences the null
        // (`0x74fa0f`); a cleared log has nothing left to send, so the loop ends here.
        let Some(id) = world
            .get_resource::<CombatLog>()
            .and_then(|log| log.store.pending())
        else {
            return;
        };
        if logging {
            let unknown = world
                .get_non_send_resource::<UiScript>()
                .map(|s| s.unknown_object_text())
                .unwrap_or_default();
            let line = world
                .get_resource::<CombatLog>()
                .and_then(|log| Some(log.store.get(id)?.pack(&unknown)));
            if let (Some(line), Some(mut file)) = (
                line,
                world.get_resource_mut::<crate::ui_chat::logging::CombatLogFile>(),
            ) {
                file.record(&line);
            }
        }
        let next_ready = world
            .get_resource_mut::<CombatLog>()
            .is_some_and(|mut log| log.store.advance_pending());
        if !next_ready {
            return;
        }
    }
}

/// What a name answer names: a unit's record, a game object's template by entry, or an item's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Landed {
    Unit(NameKey),
    GameObject(u32),
}

/// A name record's answer runs each callback the log left on it, in ask order (`0x74fe90`).
pub(crate) fn name_landed(world: &mut World, landed: Landed) {
    let Some(log) = world.get_resource::<CombatLog>() else {
        return;
    };
    let hits: Vec<u32> = log
        .waits
        .iter()
        .filter(|w| match (landed, w.key) {
            (Landed::Unit(k), NameRef::Unit(key)) => k == key,
            (Landed::GameObject(entry), NameRef::GameObject) => {
                ((w.guid >> 24) & 0xff_ffff) as u32 == entry
            }
            _ => false,
        })
        .map(|w| w.id)
        .collect();
    for wait in hits {
        callback(world, wait);
    }
}

/// An item record's answer for the record wait `wait` (`0x74fe90` on mask 4).
pub(crate) fn record_landed(world: &mut World, wait: u32) {
    callback(world, wait);
}

/// `0x74fe90` for one wait: the name read again, its bit cleared, and an entry now named in full
/// sanitized, then sent when it is the pending one.
fn callback(world: &mut World, wait: u32) {
    if world
        .run_system_cached_with(land_one, wait)
        .unwrap_or(false)
    {
        send(world);
    }
}

fn land_one(In(wait): In<u32>, mut log: ResMut<CombatLog>, sources: NameSources) -> bool {
    let Some(at) = log.waits.iter().position(|w| w.id == wait) else {
        return false;
    };
    let w = log.waits.remove(at);
    // The name now, `UNKNOWNOBJECT` for an empty answer (`0x74feb1`).
    let name = sources.answer(w.key, w.guid).flatten();
    let Some(entry) = log.store.get_mut(w.entry) else {
        return false;
    };
    match w.mask {
        1 => entry.source.name = name,
        2 => entry.dest.name = name,
        _ => {
            if let Some(item) = entry.info.item.as_mut() {
                item.name = name;
            }
        }
    }
    if !log.store.name_landed(w.entry, w.mask) {
        return false;
    }
    if let Some(entry) = log.store.get_mut(w.entry) {
        sanitize(entry, &sources);
    }
    log.store.pending() == Some(w.entry)
}
