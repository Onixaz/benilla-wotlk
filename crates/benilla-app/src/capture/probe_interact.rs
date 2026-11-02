//! The NPC-interact live probe (`WOW_PROBE_INTERACT=<npc flag mask>`, hex or decimal): once in
//! the world, finds the nearest streamed unit whose `UNIT_NPC_FLAGS` carry any bit of the mask,
//! hops beside it with `.go xyz` when it is out of reach, and right-clicks it through the shipped
//! service ladder ([`service_arm`], [`service_action`]). Whatever window opens is the run's to
//! read, from probe Lua or the log. One `PROBE interact:` line per step.
//!
//! The switches are `docs/CONTRIBUTING.md`, "Running it unattended".

use benilla_assets::coords::bevy_to_wow;
use bevy::prelude::*;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::target::click::{service_action, service_arm, ServiceAction};

/// Let the world stream before the first scan.
const SETTLE_SECS: f64 = 3.0;
/// After a hop, the wait before the click.
const HOP_SECS: f64 = 3.0;
/// How long the scan waits for a unit with the flag.
const SCAN_TIMEOUT_SECS: f64 = 30.0;

pub(crate) struct ProbeInteractPlugin;

impl Plugin for ProbeInteractPlugin {
    fn build(&self, app: &mut App) {
        let mask = std::env::var("WOW_PROBE_INTERACT")
            .ok()
            .and_then(|v| match v.strip_prefix("0x") {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => v.parse().ok(),
            })
            .unwrap_or(0);
        app.insert_resource(InteractProbe {
            mask,
            phase: Phase::Wait { since: None },
        })
        .add_systems(Update, interact_probe);
    }
}

#[derive(Resource)]
struct InteractProbe {
    mask: u32,
    phase: Phase,
}

enum Phase {
    Wait { since: Option<f64> },
    Hopped { guid: u64, at: f64 },
    Done,
}

#[allow(clippy::type_complexity)]
fn interact_probe(
    time: ProbeClock,
    mut probe: ResMut<InteractProbe>,
    self_q: Query<&Guid, With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
) {
    if self_q.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let mask = probe.mask;
    let say = |text: String| {
        let _ = net.0.send(ClientCommand::Chat {
            kind: ChatKind::Say,
            target: None,
            text,
            language: None,
        });
    };
    match probe.phase {
        Phase::Wait { mut since } => {
            let since = *since.get_or_insert(now);
            probe.phase = Phase::Wait { since: Some(since) };
            if now - since < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let nearest = units
                .iter()
                .filter(|(_, e, store, _)| {
                    e.kind == benilla_protocol::EntityKind::Unit
                        && store.0.unit_npc_flags() & mask != 0
                })
                .min_by(|a, b| {
                    let (da, db) = (a.3.translation.distance(me), b.3.translation.distance(me));
                    da.total_cmp(&db)
                });
            let Some((guid, _, store, tf)) = nearest else {
                if now - since > SCAN_TIMEOUT_SECS {
                    error!("PROBE interact: FAILURE — no unit with npc flags {mask:#x} streamed");
                    probe.phase = Phase::Done;
                }
                return;
            };
            let distance = tf.translation.distance(me);
            info!(
                "PROBE interact: {:#x} (npc flags {:#x}) at {distance:.1} yd",
                guid.0,
                store.0.unit_npc_flags()
            );
            if distance > 3.0 {
                let [x, y, z] = bevy_to_wow(tf.translation);
                say(format!(".go xyz {x} {y} {z}"));
            }
            probe.phase = Phase::Hopped {
                guid: guid.0,
                at: now,
            };
        }
        Phase::Hopped { guid, at } => {
            if now - at < HOP_SECS {
                return;
            }
            probe.phase = Phase::Done;
            let Some((_, _, store, _)) = units.iter().find(|(g, ..)| g.0 == guid) else {
                error!("PROBE interact: FAILURE — {guid:#x} left the stream");
                return;
            };
            let Some(arm) = service_arm(store.0.unit_npc_flags(), None) else {
                error!("PROBE interact: FAILURE — {guid:#x} has no service arm");
                return;
            };
            let _ = net.0.send(ClientCommand::SetSelection { guid });
            match service_action(arm, guid, false, None) {
                ServiceAction::Send(command) => {
                    info!("PROBE interact: clicking {guid:#x} — {arm:?}");
                    let _ = net.0.send(command);
                }
                _ => error!("PROBE interact: FAILURE — {arm:?} sends nothing for {guid:#x}"),
            }
        }
        Phase::Done => {}
    }
}
