//! The mirror-timer feed: breath, fatigue and feign-death edges off the wire, kept in the VM's
//! timer table and fired as `MIRROR_TIMER_START`, `_PAUSE` and `_STOP` (`UIParent.lua:134`,
//! `MirrorTimer.lua:72-75`), as the reference's mirror-timer handler (`0x519a50`) does. The stock
//! bars integrate `value + scale * elapsed` every `OnUpdate`, so a packet every few seconds paints
//! a smooth countdown, and a world entry reads the table back through `GetMirrorTimerInfo`.
//!
//! The client computes nothing: vmangos's `Player::UpdateMirrorTimers` runs the timers off its own
//! liquid checks and ships a value and a signed rate, so there is no local underwater predicate to
//! disagree with the drowning damage that follows.

use benilla_ui::script::{MirrorTimer, ScriptValue, UiScript, MIRROR_TIMER_TOKENS};
use bevy::prelude::*;

use benilla_protocol::messages::{MirrorTimerKind, MirrorTimerStart};

use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// One mirror-timer edge off the wire, queued by the net bridge for the bars.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MirrorTimerEdge {
    /// `SMSG_START_MIRROR_TIMER`: starts or wholly restates a timer; vmangos re-sends it on every
    /// change of direction, remaining time or freeze.
    Start(MirrorTimerStart),
    /// `SMSG_PAUSE_MIRROR_TIMER`: vmangos never sends it, restating with a `Start` instead.
    Pause { kind: u32, paused: bool },
    /// `SMSG_STOP_MIRROR_TIMER`: the bar hides.
    Stop { kind: u32 },
}

/// The net bridge's mirror-timer queue.
#[derive(Resource, Default)]
pub(crate) struct MirrorTimerFeed(pub(crate) Vec<MirrorTimerEdge>);

/// The three records as the client keeps them (`0xbd0b80`), C-side in 12340, so a rebuilt VM (a
/// `/reload`) gets them back for its world-entry poll; emptied when the client leaves the world
/// (`0x513ea0`).
#[derive(Resource, Default)]
struct MirrorRecords([Option<MirrorTimer>; 3]);

/// The timer's arg1 and `MirrorTimerColors` key, the client's word by wire type (`0x513e00`). Type
/// 0 is `EXHAUSTION` on the client and `FATIGUE` on the server; the Lua keys on the client's word.
fn script_name(kind: MirrorTimerKind) -> &'static str {
    MIRROR_TIMER_TOKENS[record(kind)]
}

/// The kind's record in the VM's table, its wire type.
fn record(kind: MirrorTimerKind) -> usize {
    match kind {
        MirrorTimerKind::Fatigue => 0,
        MirrorTimerKind::Breath => 1,
        MirrorTimerKind::FeignDeath => 2,
    }
}

/// Drains the queue into the VM, one event per edge.
///
/// Deviation: a type with no bar is dropped, because no bar appears either way: the reference
/// fires it as `"UNKNOWN"` and keeps no record (`0x519a50`), and `MirrorTimer_Show` errors on
/// its nil colour before `Show()`. vmangos's `NUM_CLIENT_TIMERS` keeps its fourth timer off the
/// wire.
fn feed_mirror_timers(
    script: Option<NonSendMut<UiScript>>,
    mut feed: ResMut<MirrorTimerFeed>,
    mut records: ResMut<MirrorRecords>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
    spells: Option<Res<Spells>>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        // No VM (a capture or headless run): drop the edges so the queue stays bounded.
        feed.0.clear();
        return;
    };
    let seeded = seeded.get(&script);
    if !*seeded {
        *seeded = true;
        for (i, record) in records.0.iter().enumerate() {
            script.restore_mirror_timer(i, record.clone());
        }
    }
    // The VM has no spell-catalog binding, so the owning spell's name resolves here; 0 is none.
    let spell_name = |id: u32| -> Option<String> {
        (id != 0)
            .then(|| spells.as_ref()?.catalog.get(id).map(|d| d.name.clone()))
            .flatten()
    };
    for edge in feed.0.drain(..) {
        let raw = match edge {
            MirrorTimerEdge::Start(start) => start.kind,
            MirrorTimerEdge::Pause { kind, .. } | MirrorTimerEdge::Stop { kind } => kind,
        };
        let Some(kind) = MirrorTimerKind::from_wire(raw) else {
            continue;
        };
        // The handler's two tutorial arms, type 0 and type 1 (`0x519c11`-`0x519c20`).
        if matches!(edge, MirrorTimerEdge::Start(_)) {
            match kind {
                MirrorTimerKind::Fatigue => {
                    if let Some(t) = tutorials.as_mut() {
                        t.write(crate::tutorial::TutorialEvent::trigger(
                            crate::tutorial::id::FATIGUE,
                        ));
                    }
                }
                MirrorTimerKind::Breath => {
                    if let Some(t) = tutorials.as_mut() {
                        t.write(crate::tutorial::TutorialEvent::trigger(
                            crate::tutorial::id::BREATH,
                        ));
                    }
                }
                _ => {}
            }
        }
        let name = ScriptValue::Str(script_name(kind).into());
        let (event, args): (&str, Vec<ScriptValue>) = match edge {
            MirrorTimerEdge::Start(start) => {
                // The owning spell's name when its row exists (`0x5199a0`).
                let spell = spell_name(start.spell_id);
                let label = script.mirror_timer_label(record(kind), spell.as_deref());
                // The fire (`0x519c06`), then the record (`0x519c40`).
                let args = vec![
                    name,
                    ScriptValue::Int(i64::from(start.remaining_ms)),
                    ScriptValue::Int(i64::from(start.duration_ms)),
                    ScriptValue::Int(i64::from(start.scale)),
                    ScriptValue::Int(i64::from(start.paused)),
                    ScriptValue::Str(label),
                ];
                debug!("net: mirror timer MIRROR_TIMER_START {edge:?}");
                script.fire_event("MIRROR_TIMER_START", args);
                script.start_mirror_timer(
                    record(kind),
                    MirrorTimer {
                        value: start.remaining_ms as i32,
                        max_value: start.duration_ms as i32,
                        scale: start.scale,
                        paused: i32::from(start.paused),
                        spell_name: spell,
                        ..Default::default()
                    },
                );
                records.0[record(kind)] = script.mirror_timer(record(kind));
                continue;
            }
            // The stock handler raises on this: it matches `arg1` as the timer name, then tests
            // `arg1 > 0` (`MirrorTimer.lua:97-99`), the reference's own bug. vmangos never sends
            // it (`Player::SendMirrorTimers`); a repair belongs over the stock body, not here.
            // Its fire (`0x519b58`) writes no record.
            MirrorTimerEdge::Pause { paused, .. } => (
                "MIRROR_TIMER_PAUSE",
                vec![name, ScriptValue::Int(i64::from(paused))],
            ),
            // The fire (`0x519abc`), then the record empties (`0x519ad9`).
            MirrorTimerEdge::Stop { .. } => {
                debug!("net: mirror timer MIRROR_TIMER_STOP {edge:?}");
                script.fire_event("MIRROR_TIMER_STOP", vec![name]);
                script.stop_mirror_timer(record(kind));
                records.0[record(kind)] = None;
                continue;
            }
        };
        // One log line per edge: the bars live inside the VM, so this is their only outside trace.
        debug!("net: mirror timer {event} {edge:?}");
        script.fire_event(event, args);
    }
}

/// The mirror timers' packet handlers, which only queue. The drain runs before the VM ticks, so
/// an edge and its first `OnUpdate` land on the same frame.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{MirrorTimerEdge, MirrorTimerFeed};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::MirrorTimerStart, on_edge)
            .net_handler(K::MirrorTimerPause, on_edge)
            .net_handler(K::MirrorTimerStop, on_edge)
            .net_handler(K::LoggedOut, on_logged_out);
    }

    /// Leaving the world empties every record (`0x513ea0`).
    fn on_logged_out(In(ev): In<SessionEvent>, mut records: ResMut<super::MirrorRecords>) {
        if matches!(ev, SessionEvent::LoggedOut) {
            *records = super::MirrorRecords::default();
        }
    }

    fn on_edge(In(ev): In<SessionEvent>, mut feed: ResMut<MirrorTimerFeed>) {
        let edge = match ev {
            SessionEvent::MirrorTimerStart(start) => MirrorTimerEdge::Start(start),
            SessionEvent::MirrorTimerPause { kind, paused } => {
                MirrorTimerEdge::Pause { kind, paused }
            }
            SessionEvent::MirrorTimerStop { kind } => MirrorTimerEdge::Stop { kind },
            _ => return,
        };
        feed.0.push(edge);
    }
}

pub(crate) struct UiMirrorPlugin;

impl Plugin for UiMirrorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<MirrorTimerFeed>()
            .init_resource::<MirrorRecords>()
            .add_systems(
                Update,
                // Ahead of the world-entry fire, whose stock handler polls the records.
                feed_mirror_timers
                    .in_set(UnitFeed)
                    .before(crate::ui_unit::EnterWorldFeed),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wrong name leaves `MirrorTimerColors[timer]` nil and the bar's colour read errors.
    #[test]
    fn arg1_is_the_clients_name_not_the_servers() {
        assert_eq!(script_name(MirrorTimerKind::Fatigue), "EXHAUSTION");
        assert_eq!(script_name(MirrorTimerKind::Breath), "BREATH");
        assert_eq!(script_name(MirrorTimerKind::FeignDeath), "FEIGNDEATH");
    }
}
