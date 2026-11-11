//! `SMSG_NOTIFY_DEST_LOC_SPELL_CAST` (`0x810050`): a dest-location cast's visuals, which play
//! without the caster. A per-caster cast counter, fed by the notify and by `SMSG_SPELL_GO`'s
//! dest byte (`0x80e442`), keeps one cast from playing twice: a counter not newer than the last
//! one seen for that caster is dropped (`0x8100a1`).

use std::collections::HashMap;

use benilla_protocol::messages::DestLocSpellCast;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::creature_anim::DestLocCast;
use crate::net::NetHandlerApp;

/// How long a recorded counter holds: `now + 10000` ms (`0x80e100`).
const KEEP_SECS: f64 = 10.0;

/// The processed dest-location casts by caster: the last counter and when it lapses.
#[derive(Resource, Default)]
pub(crate) struct ProcessedDestLocCasts(HashMap<u64, (u8, f64)>);

impl ProcessedDestLocCasts {
    /// Record `counter` for `caster` from `now` (`0x80e100`).
    fn record(&mut self, caster: u64, counter: u8, now: f64) {
        self.0.insert(caster, (counter, now + KEEP_SECS));
    }

    /// Whether `counter` is no newer than the caster's live one, compared as a signed byte
    /// difference so it wraps (`0x8100a1`).
    fn stale(&mut self, caster: u64, counter: u8, now: f64) -> bool {
        self.0.retain(|_, &mut (_, until)| until > now);
        self.0
            .get(&caster)
            .is_some_and(|&(seen, _)| (counter.wrapping_sub(seen) as i8) < 1)
    }
}

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.init_resource::<ProcessedDestLocCasts>()
        .net_handler(K::DestLocSpellCast, on_dest_loc_spell_cast)
        .net_handler(K::SpellGo, on_spell_go_dest)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_dest_loc_spell_cast(
    In(ev): In<SessionEvent>,
    mut processed: ResMut<ProcessedDestLocCasts>,
    time: Res<Time<Real>>,
    mut out: MessageWriter<DestLocCast>,
) {
    if let SessionEvent::DestLocSpellCast(c) = ev {
        if let Some(cast) = dest_loc_cast(c, &mut processed, time.elapsed_secs_f64()) {
            out.write(cast);
        }
    }
}

/// A nonzero caster's counter is checked and recorded (`0x810098`-`0x8100ba`); a zero caster
/// always plays.
fn dest_loc_cast(
    c: DestLocSpellCast,
    processed: &mut ProcessedDestLocCasts,
    now: f64,
) -> Option<DestLocCast> {
    if c.caster != 0 {
        if processed.stale(c.caster, c.counter, now) {
            debug!(
                "net: dest-location cast {} by {:#x}, counter {} already seen — dropped",
                c.spell_id, c.caster, c.counter
            );
            return None;
        }
        processed.record(c.caster, c.counter, now);
    }
    Some(DestLocCast {
        spell_id: c.spell_id,
        src: benilla_assets::coords::wow_to_bevy(c.src),
        dest: benilla_assets::coords::wow_to_bevy(c.dest),
        delay_ms: c.delay_ms,
        // The missile's pitch, speed and landing time ride the packet (`0x80264d`-`0x80266f`).
        trajectory: (c.speed > 0.0).then_some(crate::creature_anim::Trajectory {
            pitch: c.elevation,
            speed: c.speed,
            delay: c.delay_ms as f32 / 1000.0,
            caster: c.caster,
            cast_id: c.counter,
        }),
    })
}

/// `SMSG_SPELL_GO`'s nonzero dest byte records the caster's counter (`0x80e433`).
fn on_spell_go_dest(
    In(ev): In<SessionEvent>,
    mut processed: ResMut<ProcessedDestLocCasts>,
    time: Res<Time<Real>>,
) {
    if let SessionEvent::SpellGo {
        caster,
        dest_counter,
        ..
    } = ev
    {
        if dest_counter != 0 {
            processed.record(caster, dest_counter, time.elapsed_secs_f64());
        }
    }
}

fn on_session_end(In(_): In<SessionEvent>, mut processed: ResMut<ProcessedDestLocCasts>) {
    processed.0.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cast(caster: u64, counter: u8) -> DestLocSpellCast {
        DestLocSpellCast {
            caster,
            dest_target: 0,
            spell_id: 42,
            src: [0.0; 3],
            dest: [1.0, 2.0, 3.0],
            elevation: 0.0,
            speed: 0.0,
            delay_ms: 500,
            counter,
        }
    }

    /// `0x8100a1`: a counter not newer than the caster's last plays nothing; a newer one, a
    /// wrapped one, another caster's or a zero caster's play, and a record lapses after 10 s.
    #[test]
    fn a_seen_counter_drops_the_notify() {
        let mut p = ProcessedDestLocCasts::default();
        assert!(dest_loc_cast(cast(7, 5), &mut p, 0.0).is_some());
        assert!(
            dest_loc_cast(cast(7, 5), &mut p, 1.0).is_none(),
            "same counter"
        );
        assert!(dest_loc_cast(cast(7, 4), &mut p, 1.0).is_none(), "older");
        assert!(dest_loc_cast(cast(7, 6), &mut p, 1.0).is_some(), "newer");
        assert!(
            dest_loc_cast(cast(8, 6), &mut p, 1.0).is_some(),
            "another caster"
        );
        assert!(
            dest_loc_cast(cast(0, 6), &mut p, 1.0).is_some(),
            "no caster"
        );
        assert!(
            dest_loc_cast(cast(0, 6), &mut p, 1.0).is_some(),
            "no caster, again"
        );
        p.record(9, 255, 2.0);
        assert!(
            dest_loc_cast(cast(9, 0), &mut p, 2.0).is_some(),
            "wraps past 255"
        );
        assert!(
            dest_loc_cast(cast(7, 6), &mut p, 11.5).is_some(),
            "lapsed after 10 s"
        );
    }

    /// A GO's dest byte suppresses the notify for the same cast (`0x80e442`).
    #[test]
    fn a_spell_go_counter_suppresses_its_notify() {
        let mut p = ProcessedDestLocCasts::default();
        p.record(7, 3, 0.0);
        assert!(dest_loc_cast(cast(7, 3), &mut p, 0.5).is_none());
        let played = dest_loc_cast(cast(7, 4), &mut p, 0.5).unwrap();
        assert_eq!(played.delay_ms, 500);
    }

    /// The packet's trajectory reaches the missile only at a missile speed above 0 (`0x802641`).
    #[test]
    fn a_notify_carries_its_trajectory_at_a_missile_speed() {
        let mut p = ProcessedDestLocCasts::default();
        let flat = dest_loc_cast(cast(7, 1), &mut p, 0.0).unwrap();
        assert_eq!(flat.trajectory, None, "speed 0");
        let aimed = DestLocSpellCast {
            elevation: 0.4,
            speed: 25.0,
            ..cast(7, 2)
        };
        let played = dest_loc_cast(aimed, &mut p, 0.0).unwrap();
        assert_eq!(
            played.trajectory,
            Some(crate::creature_anim::Trajectory {
                pitch: 0.4,
                speed: 25.0,
                delay: 0.5,
                caster: 7,
                cast_id: 2,
            })
        );
    }
}
