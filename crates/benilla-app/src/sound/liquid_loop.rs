//! The above-water liquid ambient loop, as 12340's `SI2::HandleWaterSounds` (`0x4d2390`) plays it:
//! one 3D loop at the horizontally nearest liquid within 60 yd of the player, the kit its
//! `LiquidType` row names. A type flagged `0x80` is skipped while the player is inside. The query
//! runs at most every 500 ms and only once the player has moved (`0x4d2230`); a same-kit liquid
//! within 20 yd moves the loop, anything else swaps it with a 1 s fade each way. Submerging stops
//! it outright. It rides the ambience bucket, so `EnableAmbience` and the ambience slider gate it.

use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::{LiquidTables, WorldAssets};
use benilla_world::schedule::WorldStage;

use crate::net::Embodied;

use super::kit::{
    self, play_kit_ext, set_source_kit_gain, source_kit_playing, stop_source_kit, KitRef,
    PlayExtras, SoundCategory, SoundKits,
};
use super::{SoundConfig, SoundOutput};

/// The query radius (yd) around the player (`0x4d2390` → `0x77f390`).
const QUERY_RADIUS: f32 = 60.0;
/// The least time between queries (s, `0x4d2230`).
const QUERY_INTERVAL: f32 = 0.5;
/// A same-kit liquid nearer than this (yd) to the playing loop moves it rather than restarting it
/// (`0x4d2390`'s `< 400` on the squared distance).
const MOVE_RADIUS: f32 = 20.0;
/// The fade-in on start and the fade-out on stop (s, `0x4d2390`).
const FADE_SECS: f32 = 1.0;

/// One loop: its emitter, kit and pump-lane gain.
struct Loop {
    /// The emitter; the pump's tracked-follow reads its `Transform`.
    emitter: Entity,
    kit: u32,
    /// WoW-space position, compared exactly as the reference compares positions.
    at: [f32; 3],
    gain: f32,
}

#[derive(Resource, Default)]
struct LiquidLoopState {
    live: Option<Loop>,
    /// Superseded loops fading out.
    fading: Vec<Loop>,
    since_query: f32,
    last_query_at: Option<[f32; 3]>,
}

impl LiquidLoopState {
    fn stop_all(&mut self, out: &mut SoundOutput, commands: &mut Commands) {
        for l in self.live.take().into_iter().chain(self.fading.drain(..)) {
            stop_source_kit(out, l.emitter, l.kit);
            commands.entity(l.emitter).despawn();
        }
    }
}

/// The per-frame driver: fades every frame, the query on its cadence.
fn drive_liquid_loop(
    mut state: ResMut<LiquidLoopState>,
    tables: Option<Res<LiquidTables>>,
    world: benilla_world::world_point::WorldPoint,
    player: Query<&Transform, With<Embodied>>,
    mut emitters: Query<&mut Transform, Without<Embodied>>,
    time: Res<Time>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<super::AudioListener>,
    mut commands: Commands,
) {
    let (Some(tables), Some(mut kits), Some(assets)) = (tables, kits, assets) else {
        return;
    };
    // No audio device: the choice still runs, for the trace, but nothing starts, as a loop that
    // fails to start would retry every frame. The startup warning already covers it.
    let audible = out.mixer.is_some();
    let state = &mut *state;
    if world.submersion().is_water() {
        state.stop_all(&mut out, &mut commands);
        state.last_query_at = None;
        return;
    }

    let dt = time.delta_secs();
    let step = dt / FADE_SECS;
    if let Some(l) = state.live.as_mut() {
        l.gain = (l.gain + step).min(1.0);
        if audible && !source_kit_playing(&out, l.emitter, l.kit) {
            start_loop(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener.pos,
                l.kit,
                wow_to_bevy(l.at),
                l.emitter,
                l.gain,
            );
        }
        set_source_kit_gain(&mut out, l.emitter, l.kit, l.gain);
    }
    state.fading.retain_mut(|l| {
        l.gain -= step;
        if l.gain > 0.0 {
            set_source_kit_gain(&mut out, l.emitter, l.kit, l.gain);
            return true;
        }
        stop_source_kit(&mut out, l.emitter, l.kit);
        commands.entity(l.emitter).despawn();
        false
    });

    state.since_query += dt;
    if state.since_query < QUERY_INTERVAL {
        return;
    }
    state.since_query = 0.0;
    let Ok(player_tf) = player.single() else {
        return; // no avatar yet
    };
    let here = bevy_to_wow(player_tf.translation);
    if state.last_query_at == Some(here) {
        return;
    }
    state.last_query_at = Some(here);

    let types = &tables.0.types;
    let inside = world.area_interior().is_some();
    let want = world
        .nearest_liquid(here, QUERY_RADIUS, |id| {
            types.sound(id).is_some_and(|(_, muted)| !(muted && inside))
        })
        .and_then(|n| Some((types.sound(n.liquid_type)?.0, n.point, n.liquid_type)));

    if let (Some(l), Some((kit, at, _))) = (state.live.as_mut(), want) {
        if l.kit == kit {
            if l.at == at {
                return;
            }
            let d2: f32 = (0..3).map(|i| (l.at[i] - at[i]).powi(2)).sum();
            if d2 < MOVE_RADIUS * MOVE_RADIUS {
                l.at = at;
                if let Ok(mut tf) = emitters.get_mut(l.emitter) {
                    tf.translation = wow_to_bevy(at);
                }
                return;
            }
        }
    }
    if let Some(old) = state.live.take() {
        state.fading.push(old);
    }
    match want {
        Some((kit, at, id)) => debug!("liquid loop: LiquidType {id} kit {kit} at {at:?}"),
        None => debug!("liquid loop: none within {QUERY_RADIUS} yd"),
    }
    state.live = want.map(|(kit, at, _)| spawn_loop(kit, at, &mut commands));
}

/// A new loop at `at`, started silent by the next frame's fade.
fn spawn_loop(kit: u32, at: [f32; 3], commands: &mut Commands) -> Loop {
    let emitter = commands
        .spawn(Transform::from_translation(wow_to_bevy(at)))
        .id();
    Loop {
        emitter,
        kit,
        at,
        gain: 0.0,
    }
}

/// Start one loop on the ambience bucket, tagged to its emitter, at an initial gain. Force-looped,
/// as the reference plays it in loop mode (`0x4d2390`).
fn start_loop(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    kit_id: u32,
    pos: Vec3,
    emitter: Entity,
    gain: f32,
) {
    if let Err(e) = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit_id),
        Some(pos),
        SoundCategory::Ambience,
        PlayExtras {
            source: Some(emitter),
            force_loop: true,
            ..default()
        },
    ) {
        warn!("liquid loop kit {kit_id}: {e:#}");
        return;
    }
    set_source_kit_gain(out, emitter, kit_id, gain);
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<LiquidLoopState>().add_systems(
        Update,
        // Before the channel pump, which applies this frame's gains and positions.
        drive_liquid_loop
            .in_set(WorldStage::Present)
            .before(kit::pump_channels),
    );
}
