//! The stretch driver's verbs (`WOW_STRETCH=<name>`): a scripted quest stretch (`scripts/stretch.sh`)
//! runs as a `WOW_PROBE_LUA` chunk, and these are the extra `Probe*` verbs it needs to walk, find,
//! use, record and judge. A stretch ends by its verdict, by `WOW_PROBE_EXIT_AT` (a timeout, a fail)
//! or by the wire audit (`crate::net::wire_audit`); the exit code says which ([`exit_code`]).
//!
//! | verb | does |
//! |---|---|
//! | `ProbeStep(text)` | logs `stretch-step: N text`, titles the window, takes a filmstrip shot |
//! | `ProbeShot(label)` | takes a filmstrip shot |
//! | `ProbeVerdict(ok, why)` | records the stretch's verdict and exits |
//! | `ProbePos()` | the driven body's WoW `x, y, z`, whether it is another unit, its facing, and whether input moves it |
//! | `ProbeNearest(entry[, alive])` | the nearest unit or GameObject of that entry (with `alive`, a unit with health left and not skipped): yards, `x, y, z`; nil if none |
//! | `ProbeInteractEntry(entry[, alive])` | right-clicks the nearest unit or GameObject of that entry (with `alive`, as above) |
//! | `ProbeSkip(guid)` | leaves that unit (a `UnitGUID` string) out of the `alive` verbs from now on |
//! | `ProbeTargetPos()` | our target's WoW `x, y, z`; nil with none |
//! | `ProbeWalkTo(x, y, z)` | a ground right-click at that WoW point (click to move) |
//! | `ProbeKeyDown(key)`, `ProbeKeyUp(key)` | a real key press or release (`WOW_PROBE_KEY`'s names) |
//!
//! Steering goes through keys because the movement functions are refused outside a binding body,
//! as the reference's hardware-event gate refuses insecure code.
//!
//! `WOW_STRETCH_DIR=<dir>` keeps the filmstrip, `NNN-<label>.png`, and `WOW_STRETCH_EVERY=<secs>`
//! adds a shot on that period between steps.

use std::sync::Mutex;

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::PrimaryWindow;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};

use super::ProbeClock;
use crate::net::{NetEntity, ObjectStore, SelfPlayer};

/// A verdict the stretch never reached: its timeout.
const CODE_TIMEOUT: i32 = 4;
/// A verdict of fail.
const CODE_FAIL: i32 = 5;

/// What the Lua verbs asked for since the last frame, and what they read.
#[derive(Default)]
struct Shared {
    steps: Vec<String>,
    shots: Vec<String>,
    verdict: Option<(bool, String)>,
    interact: Option<(u32, bool)>,
    walk_to: Option<[f32; 3]>,
    /// Key presses (`true`) and releases asked for, in order.
    keys: Vec<(String, bool)>,
    /// The driven body's WoW pose, whether it is another unit, and its facing, each frame.
    pos: Option<([f32; 3], bool, f32, bool)>,
    /// Every streamed entry's nearest instance: yards and WoW pose, published each frame.
    nearest: std::collections::HashMap<u32, (f32, [f32; 3])>,
    /// The same, among units with health left and GameObjects, less the skipped.
    nearest_alive: std::collections::HashMap<u32, (f32, [f32; 3])>,
    /// Units the `alive` verbs leave out, by guid.
    skip: std::collections::HashSet<u64>,
    /// Our target's WoW position, published each frame.
    target: Option<[f32; 3]>,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
/// Set once a verdict lands; read by [`exit_code`].
static VERDICT: Mutex<Option<bool>> = Mutex::new(None);

fn shared<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    let mut g = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Shared::default))
}

/// Whether a stretch is running.
pub(crate) fn active() -> bool {
    std::env::var("WOW_STRETCH").is_ok_and(|v| !v.is_empty())
}

/// The stretch's own exit code, the wire audit's when it passed: 0 pass, 3 a wire rule broken,
/// 4 no verdict (timeout), 5 a failed verdict.
pub(crate) fn exit_code(audit: i32) -> i32 {
    match *VERDICT.lock().unwrap_or_else(|e| e.into_inner()) {
        None => {
            warn!("stretch: no verdict before the timeout");
            CODE_TIMEOUT
        }
        Some(false) => CODE_FAIL,
        Some(true) => audit,
    }
}

pub(crate) struct StretchPlugin;

impl Plugin for StretchPlugin {
    fn build(&self, app: &mut App) {
        let dir = std::env::var("WOW_STRETCH_DIR").unwrap_or_default();
        if !dir.is_empty() {
            let _ = std::fs::create_dir_all(&dir);
        }
        let every = std::env::var("WOW_STRETCH_EVERY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        app.insert_resource(Stretch {
            name: std::env::var("WOW_STRETCH").unwrap_or_default(),
            dir,
            every,
            ..default()
        })
        .add_systems(Update, (install_verbs, publish, drive).chain())
        .add_systems(
            bevy::app::PreUpdate,
            press_keys
                .after(bevy::input::InputSystems)
                .after(crate::loading_screen::CoverInput),
        );
    }
}

#[derive(Resource, Default)]
struct Stretch {
    name: String,
    dir: String,
    every: f32,
    shots: u32,
    steps: u32,
    last_shot: f32,
    step: String,
    installed_vm: u64,
    exiting: bool,
}

/// Installs the verbs into each new in-game VM.
fn install_verbs(
    mut stretch: ResMut<Stretch>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
) {
    let Some(script) = script else { return };
    if self_player.is_empty() || stretch.installed_vm == script.session() {
        return;
    }
    stretch.installed_vm = script.session();
    let lua = script.lua();
    let g = lua.globals();
    let set = |name: &str, f: mlua::Result<mlua::Function>| match f {
        Ok(f) => {
            if let Err(e) = g.set(name, f) {
                error!("stretch: installing {name}: {e}");
            }
        }
        Err(e) => error!("stretch: creating {name}: {e}"),
    };
    set(
        "ProbeStep",
        lua.create_function(|_, text: String| {
            shared(|s| s.steps.push(text));
            Ok(())
        }),
    );
    set(
        "ProbeShot",
        lua.create_function(|_, label: String| {
            shared(|s| s.shots.push(label));
            Ok(())
        }),
    );
    set(
        "ProbeVerdict",
        lua.create_function(|_, (ok, why): (bool, Option<String>)| {
            shared(|s| {
                s.verdict.get_or_insert((ok, why.unwrap_or_default()));
            });
            Ok(())
        }),
    );
    set(
        "ProbePos",
        lua.create_function(|_, ()| {
            Ok(match shared(|s| s.pos) {
                Some((p, foreign, facing, steerable)) => (
                    Some(p[0]),
                    Some(p[1]),
                    Some(p[2]),
                    Some(foreign),
                    Some(facing),
                    Some(steerable),
                ),
                None => (None, None, None, None, None, None),
            })
        }),
    );
    set(
        "ProbeNearest",
        lua.create_function(|_, (entry, alive): (u32, Option<bool>)| {
            let alive = alive.unwrap_or(false);
            let found = shared(|s| {
                if alive { &s.nearest_alive } else { &s.nearest }
                    .get(&entry)
                    .copied()
            });
            Ok(match found {
                Some((d, p)) => (Some(d), Some(p[0]), Some(p[1]), Some(p[2])),
                None => (None, None, None, None),
            })
        }),
    );
    set(
        "ProbeSkip",
        lua.create_function(|_, guid: String| {
            let hex = guid.trim_start_matches("0x").trim_start_matches("0X");
            if let Ok(g) = u64::from_str_radix(hex, 16) {
                shared(|s| s.skip.insert(g));
            }
            Ok(())
        }),
    );
    set(
        "ProbeTargetPos",
        lua.create_function(|_, ()| {
            Ok(match shared(|s| s.target) {
                Some(p) => (Some(p[0]), Some(p[1]), Some(p[2])),
                None => (None, None, None),
            })
        }),
    );
    set(
        "ProbeInteractEntry",
        lua.create_function(|_, (entry, alive): (u32, Option<bool>)| {
            shared(|s| s.interact = Some((entry, alive.unwrap_or(false))));
            Ok(())
        }),
    );
    for (name, down) in [("ProbeKeyDown", true), ("ProbeKeyUp", false)] {
        set(
            name,
            lua.create_function(move |_, key: String| {
                shared(|s| s.keys.push((key, down)));
                Ok(())
            }),
        );
    }
    set(
        "ProbeWalkTo",
        lua.create_function(|_, (x, y, z): (f32, f32, f32)| {
            shared(|s| s.walk_to = Some([x, y, z]));
            Ok(())
        }),
    );
}

/// Presses and releases the asked keys as the OS would deliver them, before this frame's readers.
fn press_keys(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut events: MessageWriter<bevy::input::keyboard::KeyboardInput>,
) {
    for (name, down) in shared(|s| std::mem::take(&mut s.keys)) {
        let Some(key) = super::act::probe_key_by_name(&name) else {
            warn!("stretch: no key named {name:?}");
            continue;
        };
        let state = if down {
            if keys.pressed(key) {
                continue;
            }
            keys.press(key);
            bevy::input::ButtonState::Pressed
        } else {
            if !keys.pressed(key) {
                continue;
            }
            keys.release(key);
            bevy::input::ButtonState::Released
        };
        events.write(bevy::input::keyboard::KeyboardInput {
            key_code: key,
            logical_key: bevy::input::keyboard::Key::Unidentified(
                bevy::input::keyboard::NativeKey::Unidentified,
            ),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }
}

/// Whether an object counts as alive for the `alive` verbs: a unit with health left, or any
/// GameObject (which has no health field), and not skipped.
fn alive(store: &benilla_protocol::ObjectFields, guid: Option<&crate::net::Guid>) -> bool {
    store.unit_health().is_none_or(|h| h > 0)
        && guid.is_none_or(|g| !shared(|s| s.skip.contains(&g.0)))
}

/// Publishes the driven body's pose and each streamed entry's nearest instance for the read verbs.
fn publish(
    player: Option<Res<crate::player::Player>>,
    selection: Option<Res<crate::target::Selection>>,
    objects: Query<
        (
            &NetEntity,
            &ObjectStore,
            &Transform,
            Option<&crate::net::Guid>,
        ),
        Without<SelfPlayer>,
    >,
) {
    let Some(player) = player else { return };
    if !player.active {
        return;
    }
    let at = player.pos;
    let mut nearest: std::collections::HashMap<u32, (f32, [f32; 3])> = Default::default();
    let mut nearest_alive = nearest.clone();
    for (net, store, t, guid) in &objects {
        if !matches!(
            net.kind,
            benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::GameObject
        ) {
            continue;
        }
        let Some(entry) = store.0.object_entry() else {
            continue;
        };
        let d = t.translation.distance(at);
        let here = (d, bevy_to_wow(t.translation));
        let slot = nearest.entry(entry).or_insert((f32::MAX, [0.0; 3]));
        if d < slot.0 {
            *slot = here;
        }
        if alive(&store.0, guid) {
            let slot = nearest_alive.entry(entry).or_insert((f32::MAX, [0.0; 3]));
            if d < slot.0 {
                *slot = here;
            }
        }
    }
    let pos = (
        bevy_to_wow(at),
        player.foreign_mover.is_some(),
        player.facing().rem_euclid(std::f32::consts::TAU),
        player.steerable(),
    );
    let target = selection
        .and_then(|sel| sel.target)
        .and_then(|e| objects.get(e).ok())
        .map(|(_, _, t, _)| bevy_to_wow(t.translation));
    shared(|s| {
        s.pos = Some(pos);
        s.target = target;
        s.nearest = nearest;
        s.nearest_alive = nearest_alive;
    });
}

/// Runs what the verbs asked for: steps, shots, the right-click, the walk and the verdict.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn drive(
    mut stretch: ResMut<Stretch>,
    time: ProbeClock,
    mut commands: Commands,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut out: MessageWriter<crate::target::Interact>,
    mut auto: crate::player::AutoMove,
    objects: Query<
        (Entity, &ObjectStore, &Transform, Option<&crate::net::Guid>),
        Without<SelfPlayer>,
    >,
    player: Option<Res<crate::player::Player>>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    let (steps, mut shots, verdict, interact, walk_to) = shared(|s| {
        (
            std::mem::take(&mut s.steps),
            std::mem::take(&mut s.shots),
            s.verdict.take(),
            s.interact.take(),
            s.walk_to.take(),
        )
    });
    for text in steps {
        stretch.steps += 1;
        info!("stretch-step: {} {text}", stretch.steps);
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line("step", &format!("{} {text}", stretch.steps));
        }
        stretch.step = text.clone();
        if let Ok(mut w) = windows.single_mut() {
            w.title = format!("{}: {} — {text}", stretch.name, stretch.steps);
        }
        shots.push(format!("step{}", stretch.steps));
    }
    if stretch.every > 0.0 && now - stretch.last_shot >= stretch.every && stretch.steps > 0 {
        shots.push("tick".into());
    }
    if !stretch.dir.is_empty() {
        for label in shots {
            stretch.shots += 1;
            stretch.last_shot = now;
            let slug: String = label
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let path = format!("{}/{:03}-{slug}.png", stretch.dir, stretch.shots);
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line("shot", &format!("{path} step={}", stretch.steps));
            }
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        }
    }
    if let Some((entry, living)) = interact {
        let at = player.as_ref().map(|p| p.pos).unwrap_or_default();
        let nearest = objects
            .iter()
            .filter(|(_, s, _, g)| {
                s.0.object_entry() == Some(entry) && (!living || alive(&s.0, *g))
            })
            .min_by(|a, b| {
                a.2.translation
                    .distance_squared(at)
                    .total_cmp(&b.2.translation.distance_squared(at))
            });
        match nearest {
            Some((e, ..)) => {
                info!("stretch: right-click entry {entry} ({e:?})");
                out.write(crate::target::Interact(e));
            }
            None => info!("stretch: no entry {entry} in view"),
        }
    }
    if let Some(p) = walk_to {
        let armed = auto.walk_to_ground(wow_to_bevy(p));
        info!("stretch: walk to {p:?}, armed {armed}");
    }
    if let Some((ok, why)) = verdict {
        if !stretch.exiting {
            stretch.exiting = true;
            *VERDICT.lock().unwrap_or_else(|e| e.into_inner()) = Some(ok);
            info!(
                "stretch-verdict: {} {} at step {} ({}): {why}",
                stretch.name,
                if ok { "PASS" } else { "FAIL" },
                stretch.steps,
                stretch.step
            );
            let code = exit_code(crate::net::wire_audit::exit_code());
            exit.write(match std::num::NonZeroU8::new(code as u8) {
                Some(c) => AppExit::Error(c),
                None => AppExit::Success,
            });
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                std::process::exit(code);
            });
        }
    }
}
