//! The trajectory aim preview: while the mover we drive has a vehicle whose row arms it
//! (`flags & 0xF800000`), the flight its missile would fly is drawn as a scrolling textured strip
//! from the launch to the landing, with a ground decal and up to two looping models at the
//! landing (`0x6fda20`, drawn by `0x6fdfb0`). The flight is [`super::shot::fly_traced`] at the
//! rows' middles, the reference's preview arm of `0x6fcd60`.
//!
//! Each row bit holds the preview up under one condition: `0x800000` always, `0x1000000` while a
//! turn key is held outside mouselook (`0x5f9830`), `0x2000000` while a pitch key is held or a
//! pitch was just set, `0x4000000` while mouselook may turn the mover (`0x5fa110`), `0x8000000`
//! while the mover casts a missile spell. When none holds, the last condition's lingering column keeps it up and then
//! fades it over up to 0.3 s; a cast or an always-on row never lingers.
//!
//! Not built: the preview's unit hit spheres for a missile row flagged `0x3c` (`0x6fcb60`), so the
//! arc passes through units; the delayed trajectory cast the reference's cast leg reads
//! (`0x7fe500`), which benilla never holds; and the control test's spline, animation and
//! combat-mode terms (`0x5fa060`). The decal's yaw sign against the reference's projector
//! (`0x7e4370`) is untraced, and the models are sized by their authored bounding sphere, not the
//! animated one.

use bevy::prelude::*;

use benilla_assets::M2Model;
use benilla_formats::MissileTargeting;
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::{EffectBlend, EffectFog, EffectVertex, WorldEffectDraw};
use benilla_world::view::WorldCamera;

use benilla_protocol::SessionEvent;

use super::passenger::{is_vehicle_transport, Passenger};

/// The row bits that arm the preview at all (`0x6fe9b0`).
const ARMED: u32 = 0xF80_0000;
/// Always up.
const ALWAYS: u32 = 0x80_0000;
/// Up while a turn key is held outside mouselook (`0x5f9830`).
const ON_TURN: u32 = 0x100_0000;
/// Up while a pitch key is held or a pitch was just set.
const ON_PITCH: u32 = 0x200_0000;
/// Up while mouselook may turn the mover (`0x5fa110`).
const ON_FREE_LOOK: u32 = 0x400_0000;
/// Up while the mover casts a missile spell.
const ON_CAST: u32 = 0x800_0000;

/// `UNIT_FLAG_STUNNED`, which refuses the free-look term.
pub(super) const UNIT_FLAG_STUNNED: u32 = 0x40000;
/// A seat that lets its rider turn: without it, a seated mover's vehicle prevents turning
/// (`0x74b900`).
const SEAT_ALLOWS_TURNING: u32 = 0x400;

/// The preview's modes, each lingering by its own column.
const MODE_ALWAYS: u8 = 0;
const MODE_TURN: u8 = 1;
const MODE_PITCH: u8 = 2;
const MODE_FREE_LOOK: u8 = 3;

/// The longest fade-out, seconds.
const FADE_CAP: f32 = 0.3;

/// The preview's state, the globals `0x6fda20` keeps.
#[derive(Resource, Default)]
pub(crate) struct TrajectoryTargeting {
    /// The aiming mover and its vehicle row; `None` while none aims.
    target: Option<(Entity, u32)>,
    /// The row's preview columns, held from the start.
    rec: MissileTargeting,
    arc_texture: Option<Handle<Image>>,
    impact_texture: Option<Handle<Image>>,
    /// The two impact models, spawned hidden at the start.
    markers: [Option<Marker>; 2],
    /// Frame ms at the start, the arc's scroll origin.
    started_ms: u32,
    /// Frame ms the linger began, 0 for none.
    fade_start: u32,
    mode: u8,
    /// The spell the preview last drew, set too by a trajectory cast's send (`0x6fbf90`).
    spell: u32,
    /// A trajectory cast the reference would hold until its cast animation releases the missile
    /// (`0x809f80`, sent by `0x80de60` from the release `0x72df00`), which the cast leg reads
    /// (`0x7fe500`). benilla sends at the press and parks the go's missile on the caster until the
    /// animation's release event, so the hold runs from the send to that release, or to a
    /// refusal: the reference's span, later by the go's round trip.
    held_cast: Option<HeldCast>,
    /// This frame's opacity, 0 when nothing draws.
    alpha: f32,
    /// The flight, WoW coordinates: the launch, then each step's end, the landing last.
    launch: Vec3,
    points: Vec<Vec3>,
    /// The mover's facing, the strip's and the decal's.
    facing: f32,
    decal: Vec<EffectVertex>,
    decal_key: Option<DecalKey>,
}

/// A sent trajectory cast awaiting its release: the caster, its `castCount` and spell, and once
/// its go has come, when, and whether its missile has been seen parked on the caster.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeldCast {
    caster: u64,
    count: u8,
    spell: u32,
    went_s: Option<f32>,
    parked: bool,
}

/// How long after its go a held cast waits for its missile to be parked before it lets go: the
/// router queues it within a frame or two, and a go that parks nothing (no cast animation, no
/// missile) releases at once in the reference's terms.
const PARK_GRACE_S: f32 = 0.25;

/// An impact model's root, which only the preview moves.
#[derive(Component)]
pub(super) struct PreviewMarker;

/// One impact model: its effect root and its asset, for the bounding radius.
struct Marker {
    entity: Entity,
    model: Handle<M2Model>,
    radius: f32,
}

/// The decal's projection inputs: a still landing costs one compare.
#[derive(Clone, Copy, PartialEq)]
struct DecalKey {
    center: Vec3,
    facing: f32,
    surfaces: usize,
}

/// An archive texture path as its `mpq://` URL. The reference's texture create reads a `.blp`
/// whatever the row names (`Arrow.tga` on vehicle 32), and some rows pad the name with spaces.
fn texture_url(path: &str, wrap: bool) -> String {
    let path = path.trim();
    let blp = match path.rsplit_once('.') {
        Some((stem, _)) => format!("{stem}.blp"),
        None => format!("{path}.blp"),
    };
    benilla_assets::texture_url(&blp, (wrap, wrap))
}

impl TrajectoryTargeting {
    /// `0x6fe9b0`: the preview's assets for `rec`, and its clocks reset.
    fn start(
        &mut self,
        commands: &mut Commands,
        asset_server: &AssetServer,
        target: (Entity, u32),
        rec: &MissileTargeting,
        now: u32,
    ) {
        self.stop(commands);
        self.rec = rec.clone();
        // The arc wraps both ways; the decal clamps.
        self.arc_texture = rec
            .arc_texture
            .as_deref()
            .map(|p| asset_server.load(texture_url(p, true)));
        self.impact_texture = rec
            .impact_texture
            .as_deref()
            .map(|p| asset_server.load(texture_url(p, false)));
        for (slot, path) in self.markers.iter_mut().zip(&rec.impact_models) {
            *slot = path.as_deref().map(|p| {
                let p = p.trim().to_string();
                let model = asset_server.load(benilla_assets::m2_url(&p));
                let entity = commands
                    .spawn((
                        crate::entities::dest_fx::GroundFx::marker(p),
                        PreviewMarker,
                        Transform::default(),
                        Visibility::Hidden,
                        benilla_world::model_fade::ModelFade(0.0),
                    ))
                    .id();
                Marker {
                    entity,
                    model,
                    radius: 0.0,
                }
            });
        }
        for (marker, radius) in self.markers.iter_mut().zip(rec.impact_radius) {
            if let Some(m) = marker {
                m.radius = radius;
            }
        }
        self.target = Some(target);
        self.spell = 0;
        self.fade_start = 0;
        self.started_ms = now;
    }

    /// `0x6fbf00`: the assets released and no mover aiming.
    fn stop(&mut self, commands: &mut Commands) {
        for marker in self.markers.iter_mut().filter_map(Option::take) {
            commands.entity(marker.entity).try_despawn();
        }
        self.arc_texture = None;
        self.impact_texture = None;
        self.target = None;
        self.held_cast = None;
        self.alpha = 0.0;
        self.decal_key = None;
        self.decal.clear();
    }

    /// A trajectory cast sent (`0x6fbf90`): the preview's spell, held for the cast leg when the
    /// reference would hold the cast.
    pub(crate) fn sent(&mut self, caster: u64, count: u8, spell: u32, held: bool) {
        self.spell = spell;
        self.held_cast = held.then_some(HeldCast {
            caster,
            count,
            spell,
            went_s: None,
            parked: false,
        });
    }

    /// What `ev` does to the held cast: its go starts the wait for the release, a refusal ends it.
    fn answer(&self, ev: &SessionEvent) -> Answer {
        let Some(h) = self.held_cast else {
            return Answer::None;
        };
        let refused = match *ev {
            SessionEvent::SpellGo {
                caster, cast_count, ..
            } => {
                return if caster == h.caster && cast_count == h.count {
                    Answer::Went
                } else {
                    Answer::None
                };
            }
            SessionEvent::SpellFailed {
                caster, cast_count, ..
            } => caster == h.caster && cast_count == h.count,
            SessionEvent::PetCastFailed {
                cast_count,
                spell_id,
                ..
            }
            | SessionEvent::CastResult {
                cast_count,
                spell_id,
                success: false,
                ..
            } => cast_count == h.count && spell_id == h.spell,
            _ => false,
        };
        if refused {
            Answer::Refused
        } else {
            Answer::None
        }
    }

    /// The held cast's release: its go has come and its missile has left the caster, or was never
    /// parked within [`PARK_GRACE_S`].
    fn release_held(&mut self, now_s: f32, parked: impl Fn(u32) -> bool) {
        let Some(h) = self.held_cast.as_mut() else {
            return;
        };
        let Some(went) = h.went_s else {
            return;
        };
        if parked(h.spell) {
            h.parked = true;
        } else if h.parked || now_s - went > PARK_GRACE_S {
            self.held_cast = None;
        }
    }

    /// The preview's opacity this frame from the mode `active` holds up, `None` for one that
    /// draws nothing; the linger runs on the last mode's column (`0x6fdb8c`-`0x6fdcf0`).
    fn advance(&mut self, active: Option<u8>, now: u32) -> Option<f32> {
        if let Some(mode) = active {
            self.mode = mode;
            self.fade_start = 0;
            return Some(1.0);
        }
        if self.mode == MODE_ALWAYS {
            self.fade_start = 0;
            return None;
        }
        if self.fade_start == 0 {
            if self.spell == 0 {
                self.reset_linger();
                return None;
            }
            // 0 is the no-linger sentinel, so a linger starting at ms 0 starts at 1.
            self.fade_start = now.max(1);
            return Some(1.0);
        }
        let linger = match self.mode {
            MODE_TURN => self.rec.turn_lingering,
            MODE_PITCH => self.rec.pitch_lingering,
            _ => self.rec.mouse_lingering,
        };
        let fade = (0.5 * linger).min(FADE_CAP);
        // Both spans round to the millisecond, as the reference's float-to-int conversion does.
        let hold = ((linger - fade) * 1000.0).round_ties_even() as i32;
        let fade_ms = (fade * 1000.0).round_ties_even() as i32;
        let elapsed = now.wrapping_sub(self.fade_start.wrapping_add(hold as u32)) as i32;
        if elapsed < 0 {
            return Some(1.0);
        }
        if fade_ms == 0 {
            self.reset_linger();
            return None;
        }
        let alpha = 1.0 - elapsed as f32 / fade_ms as f32;
        if alpha <= 0.0 {
            self.reset_linger();
            return None;
        }
        Some(alpha)
    }

    fn reset_linger(&mut self) {
        self.fade_start = 0;
        self.mode = MODE_ALWAYS;
    }
}

/// The mode a row's bits hold up this frame, first match wins (`0x6fdb1d`-`0x6fdb8a`).
fn active_mode(
    flags: u32,
    turning: bool,
    pitching: bool,
    free_look: bool,
    casting_missile: bool,
) -> Option<u8> {
    if flags & ALWAYS != 0 {
        Some(MODE_ALWAYS)
    } else if flags & ON_TURN != 0 && turning {
        Some(MODE_TURN)
    } else if flags & ON_PITCH != 0 && pitching {
        Some(MODE_PITCH)
    } else if flags & ON_FREE_LOOK != 0 && free_look {
        Some(MODE_FREE_LOOK)
    } else if flags & ON_CAST != 0 && casting_missile {
        Some(MODE_ALWAYS)
    } else {
        None
    }
}

/// The mover's facts the preview reads.
type MoverRow<'a> = (
    Entity,
    Option<&'a super::Vehicle>,
    Option<&'a crate::entities::CollisionHeight>,
    Option<&'a crate::creature_anim::Casting>,
    Option<&'a crate::net::ObjectStore>,
    Option<&'a Passenger>,
);

/// `0x6fda20`, once a frame: start or stop with the mover, pick the mode and the spell, fly the
/// preview and place the impact models.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn update_trajectory_targeting(
    mut commands: Commands,
    mut state: ResMut<TrajectoryTargeting>,
    (time, asset_server, models, pending): (
        Res<Time>,
        Res<AssetServer>,
        Res<Assets<M2Model>>,
        Option<Res<crate::entities::PendingMissiles>>,
    ),
    mover: Query<MoverRow, With<crate::net::ActiveMover>>,
    vehicles: Query<&super::Vehicle>,
    index: Option<Res<crate::net::GuidIndex>>,
    (tables, mut aim): (Res<super::VehicleTables>, ResMut<super::aim::VehicleAim>),
    (spells, visuals, pet_bar): (
        Option<Res<crate::ui_action::Spells>>,
        Option<Res<crate::creature_anim::SpellVisuals>>,
        Option<Res<crate::ui_pet::PetBar>>,
    ),
    (binds, rig, player): (
        Res<crate::bindings::BindingsState>,
        Option<Res<crate::player::CameraControl>>,
        Option<Res<crate::player::Player>>,
    ),
    units: crate::entities::AttachPosQuery,
    joints: Query<&GlobalTransform>,
    world: benilla_world::collision::WorldCollision,
    mut markers: Query<
        (
            &mut Transform,
            &mut Visibility,
            &mut benilla_world::model_fade::ModelFade,
        ),
        With<PreviewMarker>,
    >,
) {
    let state = &mut *state;
    let now = time.elapsed().as_millis() as u32;
    // The start and stop follow the mover and its row (`0x747f40`).
    let row = mover.single().ok();
    let key = row.and_then(|(e, v, ..)| Some((e, v?.id)));
    // A mark set while no preview ran is stale at a start, which clears it (`0x6fe9b0`).
    let pitched = std::mem::take(&mut aim.pitched) && key == state.target;
    if key != state.target {
        state.stop(&mut commands);
        if let Some(rec) = key
            .and_then(|(_, id)| tables.vehicle(id))
            .filter(|r| r.flags & ARMED != 0)
        {
            debug!(
                "vehicle: aim preview for vehicle {} (flags {:#x}): arc {:?}, impact {:?}, models {:?}",
                rec.id,
                rec.flags,
                rec.targeting.arc_texture,
                rec.targeting.impact_texture,
                rec.targeting.impact_models,
            );
            let rec = rec.targeting.clone();
            state.start(&mut commands, &asset_server, key.unwrap(), &rec, now);
        }
    }
    state.alpha = 0.0;
    let drawn = 'draw: {
        let (Some((entity, vehicle, height, casting, store, seat)), Some(player)) =
            (row, player.as_deref())
        else {
            break 'draw false;
        };
        if state.target.is_none() {
            break 'draw false;
        }
        let Some(rec) = vehicle.and_then(|v| tables.vehicle(v.id)) else {
            break 'draw false;
        };
        if rec.flags & ARMED == 0 {
            break 'draw false;
        }
        let missile_of = |spell_id: u32| {
            let def = spells.as_deref()?.catalog.get(spell_id)?;
            (def.missile != 0).then_some(())?;
            visuals.as_deref()?.0.missile(def.missile)
        };
        // A held cast lets go as its missile leaves the caster.
        state.release_held(time.elapsed_secs(), |spell| {
            pending.as_deref().is_some_and(|p| p.queues(entity, spell))
        });
        // The cast leg: the mover's cast, else the held trajectory cast, when its spell has a
        // missile row.
        let cast = casting
            .map(|c| c.spell_id)
            .or(state.held_cast.map(|h| h.spell))
            .filter(|&id| missile_of(id).is_some());
        let turn_or_action = rig.as_deref().is_some_and(|r| r.turn_or_action());
        let look = turn_or_action || binds.steering();
        let turn_keys = binds.pressed(crate::bindings::Input::TurnLeft)
            || binds.pressed(crate::bindings::Input::TurnRight);
        let free_look = look && may_sync_facing(store, seat, index.as_deref(), &vehicles, &tables);
        let active = active_mode(
            rec.flags,
            turn_keys && !look,
            pitched || aim.keys.iter().any(|&k| k),
            free_look,
            cast.is_some(),
        );
        let Some(alpha) = state.advance(active, now) else {
            break 'draw false;
        };
        // The spell: the cast's, else the last drawn, else the first pet-bar spell with a missile.
        let known = |id: u32| {
            spells
                .as_deref()
                .is_some_and(|s| s.catalog.get(id).is_some())
        };
        let spell_id = cast
            .or((state.spell != 0 && known(state.spell)).then_some(state.spell))
            .or_else(|| {
                pet_bar.as_deref()?.spells.bar.iter().find_map(|a| {
                    let id = a.action();
                    (a.is_spell() && spells.as_deref()?.catalog.get(id)?.missile != 0).then_some(id)
                })
            });
        let Some((spell_id, missile)) = spell_id.and_then(|id| Some((id, missile_of(id)?))) else {
            break 'draw false;
        };
        let Some(launch) = crate::entities::aim_launch_pos(entity, &units, &joints) else {
            break 'draw false;
        };
        let from = super::shot::aim_from(missile, Some(rec), &aim, player, launch, height);
        state.points.clear();
        let points = &mut state.points;
        // The rows' middles: every spread at 0.5, the reference's preview arm.
        let shot = super::shot::fly_traced(
            &from,
            || 0.5,
            super::shot::world_hit(&world),
            |p| points.push(p),
        );
        state.launch = shot.launch;
        state.facing = player.facing();
        state.alpha = alpha;
        state.spell = spell_id;
        // The impact models, sized to their radius by the bounding sphere (`0x6fde60`).
        let impact = benilla_assets::coords::wow_to_bevy(shot.impact.to_array());
        for marker in state.markers.iter().flatten() {
            let Ok((mut xf, mut vis, mut fade)) = markers.get_mut(marker.entity) else {
                continue;
            };
            let sphere = models
                .get(&marker.model)
                .and_then(|m| m.bounds.as_ref())
                .map_or(0.0, |b| b.sphere_radius);
            let scale = if sphere <= 0.0 {
                marker.radius
            } else {
                marker.radius / sphere
            };
            *xf = Transform::from_translation(impact)
                .with_rotation(Quat::from_rotation_y(state.facing))
                .with_scale(Vec3::splat(scale));
            fade.0 = alpha;
            vis.set_if_neq(Visibility::Inherited);
        }
        true
    };
    if !drawn {
        state.alpha = 0.0;
        for marker in state.markers.iter().flatten() {
            if let Ok((_, mut vis, _)) = markers.get_mut(marker.entity) {
                vis.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// The free-look test's mover terms (`0x5fa110`): alive, not stunned, standing, and not seated
/// where the vehicle prevents turning (a seat without `0x400`, `0x74b900`).
pub(super) fn may_sync_facing(
    store: Option<&crate::net::ObjectStore>,
    seat: Option<&Passenger>,
    index: Option<&crate::net::GuidIndex>,
    vehicles: &Query<&super::Vehicle>,
    tables: &super::VehicleTables,
) -> bool {
    let Some(store) = store else {
        return false;
    };
    if store.0.unit_is_dead()
        || store.0.unit_flags() & UNIT_FLAG_STUNNED != 0
        || store.0.unit_stand_state() != 0
    {
        return false;
    }
    let prevented = seat
        .filter(|p| is_vehicle_transport(p.vehicle))
        .and_then(|p| {
            let entity = index?.0.get(&p.vehicle)?;
            let id = vehicles.get(*entity).ok()?.id;
            tables.seat(id, usize::from(p.seat))
        })
        .is_some_and(|s| s.flags & SEAT_ALLOWS_TURNING == 0);
    !prevented
}

/// What a cast's answer does to the held one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    None,
    Went,
    Refused,
}

/// Register the held cast's answers, called from [`super::VehiclePlugin`]: its go and its three
/// refusals.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    use benilla_protocol::SessionEventKind as K;
    app.net_handler(K::SpellGo, on_trajectory_cast_answer)
        .net_handler(K::SpellFailed, on_trajectory_cast_answer)
        .net_handler(K::PetCastFailed, on_trajectory_cast_answer)
        .net_handler(K::CastResult, on_trajectory_cast_answer);
}

/// The held trajectory cast waits for its release from its go, and ends with a refusal.
fn on_trajectory_cast_answer(
    In(ev): In<SessionEvent>,
    time: Res<Time>,
    mut state: ResMut<TrajectoryTargeting>,
) {
    // The kinds it is registered for: a cast's go and its three refusals.
    if !matches!(
        ev,
        SessionEvent::SpellGo { .. }
            | SessionEvent::SpellFailed { .. }
            | SessionEvent::PetCastFailed { .. }
            | SessionEvent::CastResult { .. }
    ) {
        return;
    }
    match state.answer(&ev) {
        Answer::Went => {
            if let Some(h) = state.held_cast.as_mut() {
                h.went_s.get_or_insert(time.elapsed_secs());
            }
        }
        Answer::Refused => state.held_cast = None,
        Answer::None => {}
    }
}

/// Whether the reference holds an instant trajectory cast until its animation releases it: the
/// caster casts nothing, the visual has a missile, and its cast kit plays an animation without
/// the visual's `0x4` flag (`0x809f80`).
pub(super) fn held_until_release(
    casting: bool,
    visual: Option<&benilla_formats::VisualStages>,
    cast_kit: Option<&benilla_formats::VisualKit>,
) -> bool {
    !casting
        && visual.is_some_and(|v| v.missile_gate != 0 && v.flags & 0x4 == 0)
        && cast_kit.is_some_and(|k| k.anim_id.is_some_and(|a| a > 0))
}

/// The preview runs while a vehicle mover could aim or a preview is up, to stop it.
pub(super) fn preview_may_run(
    state: Res<TrajectoryTargeting>,
    movers: Query<(), (With<crate::net::ActiveMover>, With<super::Vehicle>)>,
) -> bool {
    state.target.is_some() || !movers.is_empty()
}

/// The draw runs on a frame the preview shows.
pub(super) fn preview_shown(state: Option<Res<TrajectoryTargeting>>) -> bool {
    state.is_some_and(|s| s.alpha > 0.0)
}

/// `0x6fdfb0`: the arc strip and the impact decal, additive and unfogged.
pub(super) fn push_trajectory_targeting(
    time: Res<Time>,
    state: Option<ResMut<TrajectoryTargeting>>,
    cam: Query<Entity, With<WorldCamera>>,
    decals: WorldDecal,
    mut draw: WorldEffectDraw,
) {
    let Some(mut state) = state else {
        return;
    };
    if state.alpha <= 0.0 || state.target.is_none() || state.points.is_empty() {
        return;
    }
    let Ok(cam) = cam.single() else { return };
    let bevy = |v: Vec3| benilla_assets::coords::wow_to_bevy(v.to_array());
    let now = time.elapsed().as_millis() as u32;
    if let Some(texture) = state.arc_texture.as_ref().map(Handle::id) {
        let strip = arc_strip(&state, now);
        let mut batch = draw
            .batch(cam, texture)
            .blend(EffectBlend::Add)
            .fog(EffectFog::Off)
            .anchored(bevy(state.launch));
        // The strip as a triangle list; culling is off, so the winding does not matter.
        batch.extend(strip.windows(3).flat_map(|w| [w[0], w[1], w[2]]));
        batch.tris();
    }
    let Some(texture) = state.impact_texture.as_ref().map(Handle::id) else {
        return;
    };
    let Some(&impact) = state.points.last() else {
        return;
    };
    let radius = state.rec.impact_tex_radius;
    if radius <= 0.0 {
        return;
    }
    let center = bevy(impact);
    let key = DecalKey {
        center,
        facing: state.facing,
        surfaces: decals.receiver_count(),
    };
    let state = &mut *state;
    if state.decal_key != Some(key) {
        state.decal.clear();
        state.decal_key = Some(key);
        let (sin, cos) = state.facing.sin_cos();
        // The impact's cube, `± radius` every way, the reference's box around the landing.
        let frame = DecalFrame {
            center,
            sin,
            cos,
            min_x: -radius,
            max_x: radius,
            min_z: -radius,
            max_z: radius,
            min_y: -radius,
            max_y: radius,
        };
        decals.project(
            &mut state.decal,
            &frame,
            |p| ((radius - p.y.abs()) / radius).clamp(0.0, 1.0),
            |x, z| frame.rect_uv(x, z),
        );
    }
    if state.decal.is_empty() {
        return;
    }
    // The vertex colour's alpha is the preview's, rounded to a byte.
    let alpha = (state.alpha * 255.0 + 0.5).floor() / 255.0;
    let mut batch = draw
        .batch(cam, texture)
        .blend(EffectBlend::Add)
        .anchored(center)
        // The pre-water decal band, the reference's projector flags `0x200122`.
        .rung(
            benilla_world::sky_order::Rung::RETICLE,
            benilla_world::sky_order::Rung::DECAL_RASTER,
        );
    batch.extend(state.decal.iter().map(|v| EffectVertex {
        pos: v.pos,
        uv: v.uv,
        color: [1.0, 1.0, 1.0, v.color[3] * alpha],
    }));
    batch.tris();
}

/// The arc as a triangle strip, two vertices across the launch and each point
/// (`0x6fe0d0`-`0x6fe3a0`): `arcWidth` wide across the facing, the launch pair clear, the opacity
/// ramping to `endOpacity`, the texture repeating `arcRepeat` a step and scrolling `arcSpeed` a
/// second.
fn arc_strip(state: &TrajectoryTargeting, now_ms: u32) -> Vec<EffectVertex> {
    let rec = &state.rec;
    let half = rec.arc_width * 0.5;
    let side = Vec3::new(-state.facing.sin() * half, state.facing.cos() * half, 0.0);
    let count = state.points.len();
    let alpha = state.alpha * 255.0;
    let step = (255.0 * (state.alpha * rec.end_opacity) - alpha) * (1.0 / count as f32);
    let elapsed = now_ms.wrapping_sub(state.started_ms) as f32;
    let start = 1.0 - (elapsed * 0.001 * rec.arc_speed).rem_euclid(1.0);
    let vertex = |at: Vec3, u: f32, v: f32, a: f32| EffectVertex {
        pos: benilla_assets::coords::wow_to_bevy(at.to_array()).into(),
        uv: [u, v],
        color: [1.0, 1.0, 1.0, a],
    };
    let mut out = Vec::with_capacity(2 * count + 2);
    out.push(vertex(state.launch + side, start, 0.0, 0.0));
    out.push(vertex(state.launch - side, start, 1.0, 0.0));
    let mut point_alpha = alpha + step;
    let mut u = start + rec.arc_repeat;
    for &p in &state.points {
        let a = (point_alpha.max(0.0) + 0.5).floor().min(255.0) / 255.0;
        out.push(vertex(p + side, u, 0.0, a));
        out.push(vertex(p - side, u, 1.0, a));
        point_alpha += step;
        u += rec.arc_repeat;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> MissileTargeting {
        MissileTargeting {
            turn_lingering: 1.0,
            pitch_lingering: 2.0,
            mouse_lingering: 3.0,
            end_opacity: 0.5,
            arc_speed: 2.0,
            arc_repeat: 0.25,
            arc_width: 2.0,
            ..Default::default()
        }
    }

    fn state() -> TrajectoryTargeting {
        TrajectoryTargeting {
            rec: rec(),
            spell: 7,
            ..Default::default()
        }
    }

    /// The first set bit wins, and the cast leg shows as the always mode.
    #[test]
    fn the_first_held_bit_picks_the_mode() {
        assert_eq!(
            active_mode(ALWAYS | ON_TURN, true, false, false, false),
            Some(MODE_ALWAYS)
        );
        assert_eq!(
            active_mode(ON_TURN | ON_PITCH, true, true, false, false),
            Some(MODE_TURN)
        );
        assert_eq!(
            active_mode(ON_TURN | ON_PITCH, false, true, false, false),
            Some(MODE_PITCH)
        );
        assert_eq!(
            active_mode(ON_FREE_LOOK, false, false, true, false),
            Some(MODE_FREE_LOOK)
        );
        assert_eq!(
            active_mode(ON_CAST, false, false, false, true),
            Some(MODE_ALWAYS)
        );
        assert_eq!(
            active_mode(ON_TURN | ON_CAST, false, true, true, false),
            None
        );
    }

    /// A released turn holds for `linger − fade`, then fades over `fade`, here 0.3 s of 1.0.
    #[test]
    fn a_released_turn_lingers_then_fades() {
        let mut s = state();
        assert_eq!(s.advance(Some(MODE_TURN), 1000), Some(1.0));
        // The release: the linger starts at full opacity.
        assert_eq!(s.advance(None, 2000), Some(1.0));
        // Held 700 ms, then 300 ms of fade.
        assert_eq!(s.advance(None, 2699), Some(1.0));
        let mid = s.advance(None, 2850).unwrap();
        assert!((mid - 0.5).abs() < 1.0e-5, "{mid}");
        assert_eq!(s.advance(None, 3000), None, "faded out");
        assert_eq!(s.advance(None, 3100), None, "and stays out");
    }

    /// Each mode lingers by its own column: a pitch for 2 s, fading over the 0.3 s cap.
    #[test]
    fn a_pitch_lingers_by_its_own_column() {
        let mut s = state();
        s.advance(Some(MODE_PITCH), 0);
        assert_eq!(s.advance(None, 10), Some(1.0));
        assert_eq!(s.advance(None, 10 + 1699), Some(1.0));
        assert!(s.advance(None, 10 + 1850).unwrap() < 1.0);
        assert_eq!(s.advance(None, 10 + 2000), None);
    }

    /// A cast or an always-on row ends at once; a linger needs a drawn spell.
    #[test]
    fn the_always_mode_and_a_spell_less_preview_never_linger() {
        let mut s = state();
        s.advance(Some(MODE_ALWAYS), 0);
        assert_eq!(s.advance(None, 10), None);
        let mut s = state();
        s.spell = 0;
        s.advance(Some(MODE_TURN), 0);
        assert_eq!(s.advance(None, 10), None);
    }

    /// The strip: the launch pair clear, the points ramping to `endOpacity`, `arcRepeat` a step.
    #[test]
    fn the_strip_ramps_and_repeats() {
        let mut s = state();
        s.alpha = 1.0;
        s.launch = Vec3::ZERO;
        s.points = vec![Vec3::X, Vec3::X * 2.0];
        let strip = arc_strip(&s, 0);
        assert_eq!(strip.len(), 6);
        assert_eq!(strip[0].color[3], 0.0);
        assert_eq!(strip[1].color[3], 0.0);
        // 255 → 127.5 over two points: 191.25, then 127.5.
        assert!((strip[2].color[3] - 191.0 / 255.0).abs() < 1.0e-6);
        assert!((strip[5].color[3] - 128.0 / 255.0).abs() < 1.0e-6);
        assert_eq!(strip[2].uv, [1.25, 0.0]);
        assert_eq!(strip[5].uv, [1.5, 1.0]);
        // One yard either side of the facing (0, along WoW +x): WoW ±y.
        let across = Vec3::from(strip[2].pos) - Vec3::from(strip[3].pos);
        assert!((across.length() - 2.0).abs() < 1.0e-5);
    }

    /// The texture scrolls `arcSpeed` repeats a second toward the landing.
    #[test]
    fn the_strip_scrolls_with_time() {
        let mut s = state();
        s.alpha = 1.0;
        s.points = vec![Vec3::X];
        let u = |ms| arc_strip(&s, ms)[0].uv[0];
        assert_eq!(u(0), 1.0);
        assert!((u(125) - 0.75).abs() < 1.0e-6, "{}", u(125));
    }

    /// A row's texture names read as their `.blp`, trimmed.
    #[test]
    fn a_row_texture_reads_as_its_blp() {
        assert_eq!(
            texture_url("Arrow.tga", false),
            benilla_assets::texture_url("Arrow.blp", (false, false))
        );
        assert_eq!(
            texture_url("Spells\\Textures\\Target_Line.blp  ", true),
            benilla_assets::texture_url("Spells\\Textures\\Target_Line.blp", (true, true))
        );
    }

    /// The held cast ends with its own go or refusal, not another cast's.
    #[test]
    fn the_held_cast_ends_with_its_own_answer() {
        let mut s = state();
        s.sent(0xF150_0000_0000_0001, 3, 66218, true);
        assert_eq!(s.spell, 66218);
        let go = |caster, cast_count| SessionEvent::SpellGo {
            caster,
            cast_count,
            spell_id: 66218,
            cast_flags: 0,
            server_time: 0,
            hits: Vec::new(),
            misses: Vec::new(),
            reflects: Vec::new(),
            target: None,
            go_target: None,
            target_mask: 0,
            src: None,
            dest: None,
            ammo: None,
            item_caster: None,
            trajectory: None,
            dest_counter: 0,
            points: Vec::new(),
        };
        const CAT: u64 = 0xF150_0000_0000_0001;
        assert_eq!(s.answer(&go(CAT, 4)), Answer::None, "another count");
        assert_eq!(s.answer(&go(7, 3)), Answer::None, "another caster");
        assert_eq!(s.answer(&go(CAT, 3)), Answer::Went);
        let refused = SessionEvent::PetCastFailed {
            cast_count: 3,
            spell_id: 66218,
            reason: 0,
            arg: None,
            arg2: None,
            wire_reason: 0,
        };
        assert_eq!(s.answer(&refused), Answer::Refused);
        // A cast the reference sends at once holds nothing.
        s.sent(CAT, 5, 66218, false);
        assert_eq!(s.answer(&go(CAT, 5)), Answer::None);
    }

    /// The hold outlives the go while the missile waits on the caster, and lets go at its
    /// release; a go that parks nothing lets go after the grace.
    #[test]
    fn the_held_cast_lets_go_at_the_release() {
        let mut s = state();
        s.sent(1, 3, 66218, true);
        s.release_held(0.0, |_| false);
        assert!(s.held_cast.is_some(), "no go yet");
        s.held_cast.as_mut().unwrap().went_s = Some(1.0);
        s.release_held(1.01, |_| false);
        assert!(s.held_cast.is_some(), "the missile is not parked yet");
        s.release_held(1.05, |spell| spell == 66218);
        s.release_held(1.6, |_| true);
        assert!(s.held_cast.is_some(), "the throw winds up past the grace");
        s.release_held(1.7, |_| false);
        assert!(s.held_cast.is_none(), "released");
        s.sent(1, 4, 66218, true);
        s.held_cast.as_mut().unwrap().went_s = Some(2.0);
        s.release_held(2.2, |_| false);
        assert!(s.held_cast.is_some());
        s.release_held(2.3, |_| false);
        assert!(s.held_cast.is_none(), "nothing parked within the grace");
    }

    /// The reference holds a cast whose missile visual's cast kit animates, unless a cast runs.
    #[test]
    fn a_cast_with_an_animated_kit_is_held() {
        let stages = benilla_formats::VisualStages {
            missile_gate: 1,
            ..Default::default()
        };
        let kit = benilla_formats::VisualKit {
            anim_id: Some(6),
            ..Default::default()
        };
        assert!(held_until_release(false, Some(&stages), Some(&kit)));
        assert!(!held_until_release(true, Some(&stages), Some(&kit)));
        let still = benilla_formats::VisualKit {
            anim_id: Some(0),
            ..Default::default()
        };
        assert!(!held_until_release(false, Some(&stages), Some(&still)));
        let flagged = benilla_formats::VisualStages {
            flags: 0x4,
            ..stages
        };
        assert!(!held_until_release(false, Some(&flagged), Some(&kit)));
    }
}
