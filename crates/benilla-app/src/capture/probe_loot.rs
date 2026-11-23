//! The corpse-loot live probe (`WOW_PROBE_LOOT=1`): once in the world, loots each lootable corpse
//! within reach, once, as the corpse click's loot leg does (`0x6dac40`): the selection, the loot
//! latch and `CMSG_LOOT`. What the window and any group roll raise is the run's to read, from
//! probe Lua or the log. One `PROBE loot:` line per corpse.
//!
//! The switches are `docs/CONTRIBUTING.md`, "Running it unattended".

use std::collections::HashSet;

use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::{ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;

/// Let the world stream before the first scan.
const SETTLE_SECS: f64 = 3.0;
/// The gap between two loots, so one window answers before the next send.
const GAP_SECS: f64 = 4.0;
/// How far a corpse may lie, in yards; the server refuses one out of its own range.
const REACH_YD: f32 = 10.0;

pub(crate) struct ProbeLootPlugin;

impl Plugin for ProbeLootPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LootProbe>()
            .add_systems(Update, loot_probe);
    }
}

#[derive(Resource, Default)]
struct LootProbe {
    /// When the world was first seen.
    since: Option<f64>,
    /// When the last loot went out.
    last: Option<f64>,
    looted: HashSet<u64>,
}

fn loot_probe(
    time: ProbeClock,
    mut probe: ResMut<LootProbe>,
    self_q: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut latch: ResMut<crate::ui_loot::LootLatch>,
) {
    if self_q.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let since = *probe.since.get_or_insert(now);
    if now - since < SETTLE_SECS || probe.last.is_some_and(|t| now - t < GAP_SECS) {
        return;
    }
    let me = player.pos;
    for (guid, e, store, tf) in &units {
        let distance = tf.translation.distance(me);
        if e.kind == benilla_protocol::EntityKind::Unit
            && store.0.unit_health() == Some(0)
            && distance <= 40.0
            && !probe.looted.contains(&guid.0)
        {
            debug!(
                "PROBE loot: dead {:#x} at {distance:.1} yd, lootable {}",
                guid.0,
                store.0.unit_lootable()
            );
        }
    }
    let corpse = units
        .iter()
        .filter(|(guid, e, store, tf)| {
            e.kind == benilla_protocol::EntityKind::Unit
                && store.0.unit_health() == Some(0)
                && store.0.unit_lootable()
                && tf.translation.distance(me) <= REACH_YD
                && !probe.looted.contains(&guid.0)
        })
        .min_by(|a, b| {
            let (da, db) = (a.3.translation.distance(me), b.3.translation.distance(me));
            da.total_cmp(&db)
        });
    let Some((guid, ..)) = corpse else {
        return;
    };
    let guid = guid.0;
    info!("PROBE loot: looting corpse {guid:#x}");
    probe.looted.insert(guid);
    probe.last = Some(now);
    let _ = net.0.send(ClientCommand::SetSelection { guid });
    let _ = net.0.send(ClientCommand::Loot { guid });
    latch.0 = Some(guid);
}
