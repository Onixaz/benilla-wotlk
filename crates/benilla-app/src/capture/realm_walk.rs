//! The realm-list boundary smoke (`WOW_REALM_SMOKE=1`, run by `scripts/smoke.sh`), against a
//! real server, pressing the stock screens:
//!
//! 1. from character select, Change Realm: the list goes up over the screen, which stays;
//! 2. Okay on the current realm: a world re-dial whose roster arrives with the list down and the
//!    screen still character select;
//! 3. Change Realm again, then Cancel: the list goes down and the session is untouched;
//! 4. one more Change Realm and Okay, proving the IO thread is at the park the app expects.

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::glue_press::{self as press, glue};
use super::probes::ProbeClock;
use crate::net::CharListMessage;

/// How long each leg may take before the walk names the one that stalled.
const LEG_TIMEOUT: f32 = 25.0;
/// How long the walk waits to reach character select; the whole login is inside it.
const ENTRY_TIMEOUT: f32 = 60.0;
/// A beat with the list up, so a stuck frame has time to show.
const DWELL: f32 = 1.0;

pub(crate) struct RealmWalkPlugin;

impl Plugin for RealmWalkPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, walk_realm_list);
    }
}

#[derive(Default)]
struct Walk {
    phase: u8,
    mark: f32,
    /// The roster count to reach: an entered realm sends a new roster.
    rosters: u32,
    seen: u32,
    /// The one-shot startup refusal has run.
    armed: bool,
}

impl Walk {
    fn fail(&mut self, exit: &mut MessageWriter<AppExit>, why: &str) {
        error!("realm-smoke: FAILED — {why}");
        exit.write(AppExit::error());
        self.phase = u8::MAX;
    }
}

/// The walk; every leg logs itself, so a timeout names the boundary that stalled.
fn walk_realm_list(
    mut msgs: MessageReader<CharListMessage>,
    script: Option<NonSend<UiScript>>,
    time: ProbeClock,
    mut walk: Local<Walk>,
    mut exit: MessageWriter<AppExit>,
) {
    walk.seen += msgs.read().count() as u32;
    if walk.phase == u8::MAX {
        return;
    }
    let now = time.elapsed_secs();
    // `WOW_CHAR` and `WOW_RIG` each seat a body and answer the roster this walk drives, so
    // either one, even inherited from the shell, fails the walk on its first frame.
    if !walk.armed {
        walk.armed = true;
        if let Some((key, value)) = ["WOW_CHAR", "WOW_RIG"].into_iter().find_map(|k| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(|v| (k, v))
        }) {
            walk.fail(
                &mut exit,
                &format!(
                    "{key}={} seats a body — this walk drives CHARACTER SELECT and there is no \
                     roster screen to drive once one is seated. Unset it (`scripts/smoke.sh` \
                     does).",
                    value.to_string_lossy(),
                ),
            );
            return;
        }
    }
    let s = glue(script.as_deref());
    let realm_up = s.is_some_and(press::realm_list_up);
    let at_select = s.is_some_and(|s| press::select_up(s) && !press::roster(s).is_empty());
    if walk.phase == 0 && now - walk.mark > ENTRY_TIMEOUT {
        walk.fail(
            &mut exit,
            &format!(
                "never reached character select in {ENTRY_TIMEOUT:.0}s (glue VM {}) — nothing \
                 was driven, so the fault is upstream of the walk",
                if s.is_some() { "up" } else { "absent" },
            ),
        );
        return;
    }
    if walk.phase > 0 && now - walk.mark > LEG_TIMEOUT {
        let why = format!(
            "leg {} stalled for {LEG_TIMEOUT:.0}s (list {}, select {}, rosters {}/{})",
            walk.phase,
            if realm_up { "up" } else { "down" },
            if at_select { "idle" } else { "not idle" },
            walk.seen,
            walk.rosters,
        );
        walk.fail(&mut exit, &why);
        return;
    }
    let Some(s) = s else {
        return;
    };

    match walk.phase {
        // ── 1 · Change Realm: the list goes up over the screen. ──────────────────────────────
        0 if at_select => {
            info!("realm-smoke: at character select → Change Realm");
            press::press_change_realm(s);
            (walk.phase, walk.mark) = (1, now);
        }
        1 if realm_up && now - walk.mark > DWELL => {
            if !press::select_shown(s) {
                walk.fail(
                    &mut exit,
                    "the list replaced the character screen instead of standing over it",
                );
                return;
            }
            info!("realm-smoke: list up, character select still underneath → Okay");
            okay(&mut walk, s, now);
        }
        // ── 2 · Okay has to land: a fresh roster, list down, still at select. ────────────────
        2 if walk.seen >= walk.rosters && !realm_up && at_select => {
            info!("realm-smoke: Okay landed — fresh roster at character select → Change Realm");
            press::press_change_realm(s);
            (walk.phase, walk.mark) = (3, now);
        }
        // ── 3 · Cancel changes nothing. ──────────────────────────────────────────────────────
        3 if realm_up && now - walk.mark > DWELL => {
            info!("realm-smoke: list up → Cancel");
            press::press_realm_cancel(s);
            (walk.phase, walk.mark) = (4, now);
        }
        4 if !realm_up && now - walk.mark > DWELL => {
            if !at_select {
                walk.fail(
                    &mut exit,
                    "Cancel left character select instead of just hiding the list",
                );
                return;
            }
            // ── 4 · And the park is still where the app thinks it is. ────────────────────────
            info!("realm-smoke: Cancel kept the session → one more Change Realm + Okay");
            press::press_change_realm(s);
            (walk.phase, walk.mark) = (5, now);
        }
        5 if realm_up && now - walk.mark > DWELL => okay(&mut walk, s, now),
        6 if walk.seen >= walk.rosters && !realm_up && at_select => {
            info!(
                "realm-smoke: done — {} roster(s), {} char(s) at character select",
                walk.seen,
                press::roster(s).len()
            );
            exit.write(AppExit::Success);
            walk.phase = u8::MAX;
        }
        _ => {}
    }
}

/// `RealmList_OnOk` on the realm the list highlighted; the next leg waits for its roster.
fn okay(walk: &mut Walk, s: &UiScript, now: f32) {
    if press::press_realm_okay(s) == 0 {
        warn!("realm-smoke: the list highlighted no realm, so Okay enters none");
    }
    (walk.phase, walk.mark, walk.rosters) = (walk.phase + 1, now, walk.seen + 1);
}
