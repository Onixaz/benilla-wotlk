//! Facing: resolving the wire's `MonsterMoveFacing`, and the unit display-facing smoother (the 1.12
//! client's goal chain and `+0xc98` box filter) with the turn-shuffle latch.
//!
//! A unit has a raw wire facing (movement `+0x1c`) and a display facing (unit `+0xc94`) that the
//! client turns every frame toward a goal from an ordered chain; two goals are local and never
//! sent: squaring up on `UNIT_FIELD_TARGET`, and an NPC facing you while its window is open.
//! `Transform.rotation` is the display facing; [`DisplayFacing::wire`] keeps the raw one, the
//! chain's fallback goal, which swings an NPC back when its window closes.
//! Deviation: the reference places collision and culling by the raw facing, ours follow the
//! display facing, because a capsule and a sphere barely change under rotation.

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{EntityKind, MonsterMoveFacing};
use bevy::prelude::*;

use super::super::{ActiveMover, GuidIndex, NetEntity, ObjectStore, SelfPlayer};
use super::{yaw_of, RemoteMotion, Spline};

/// The turn-shuffle latch's sign band, +1e-5 and -1e-5, tested on the yaw the pump applied: above
/// it latches bit `0x800` (ShuffleLeft 11), below it `0x1000` (ShuffleRight 12). Shared with the
/// remote facing interp.
pub(super) const TURN_LATCH_BAND: f32 = 1.0e-5;

/// The goal dead band in radians, tested inclusively on the unfolded goal-minus-current delta;
/// inside it the goal is taken verbatim and the ring resets.
const DEAD_BAND: f32 = 0.01;

/// A plain mean over the 4-sample ring.
const RING_AVG: f32 = 0.25;

/// With the overshoot clamp binding, the error halves every pump.
const STEP_FRACTION: f32 = 0.5;

/// `UNIT_FLAG_STUNNED`, goal-chain row 4.
const UNIT_FLAG_STUNNED: u32 = 0x0004_0000;

/// The `UNIT_FIELD_FLAGS` bit that turns a non-player unit by the damped turn (`0x735f60`).
const UNIT_FLAG_PLAYER_CONTROLLED: u32 = 0x0000_0008;

/// The `UNIT_FIELD_FLAGS` bit the in-combat test reads beside auto-attack (`0x71afb0`).
const UNIT_FLAG_PET_IN_COMBAT: u32 = 0x0000_0800;

/// The smallest `SmoothFacingChaseRate` that chases (`0x735f60`).
const CHASE_RATE_FLOOR: f32 = 0.000_01;

/// The chase rates of a player-controlled unit, in and out of combat (`0x735f60`).
const CONTROLLED_CHASE_RATE: (f32, f32) = (30.0, 20.0);

/// `CreatureMovementInfo.dbc`, the creature templates' smooth-facing chase rates; without it every
/// creature takes the box filter or the player-controlled turn.
#[derive(Resource)]
pub(crate) struct ChaseRates(benilla_formats::CreatureMovementInfo);

pub(crate) fn load_chase_rates(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_creature_movement_info(&mut chain)
    };
    match loaded {
        Ok(table) => {
            info!("facing: {} creature movement rows", table.len());
            commands.insert_resource(ChaseRates(table));
        }
        Err(e) => warn!("facing: CreatureMovementInfo failed to load: {e:#}"),
    }
}

/// The `EmoteFlags` bit that permits the target and interaction facing during a state emote; the
/// name is ours, the tested bit is the reference's.
const EMOTE_PERMITS_FACING: u32 = benilla_formats::EMOTE_FLAG_INTERACTION;

/// A stationary unit's display-facing state, the client's unit `+0xc98` goal and
/// `+0xc9c..+0xca8` delta ring; present only while [`drive_display_facing`] governs the unit.
/// Sparse, as it comes and goes.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(crate) struct DisplayFacing {
    /// The raw wire yaw (movement `+0x1c`), never written client-side.
    wire: f32,
    /// A zero head means the next pump takes the ring-fill leg.
    hist: [f32; 4],
    /// The damped turn's angular velocity (rad/s), which the box filter leaves alone. It starts
    /// at rest each time the unit stops, as the state is dropped while it moves (row 1); the
    /// reference keeps it across movement and zeroes it only for the active mover (`0x735f60`).
    velocity: f32,
    /// The yaw this pump last wrote, the pump's own `+0xc94` float: re-reading it off the
    /// quaternion adds ulps each frame, and the damped turn, which has no dead band, would then
    /// never settle bitwise.
    shown: Option<f32>,
    /// A deck rider's display facing in its deck's frame, which `compose_riders` turns the shown
    /// yaw through; the pump steps it there (`0x7362e3`).
    deck: Option<f32>,
}

impl DisplayFacing {
    /// The deck-local display facing of a rider the pump governs.
    pub(crate) fn deck(&self) -> Option<f32> {
        self.deck
    }
}

/// The signed yaw the facing pump applied this frame (positive turns left), the client's shuffle
/// latch input; removed the frame the body stops moving.
#[derive(Component)]
pub(crate) struct FacingStep(pub(crate) f32);

/// An attack stop's held facing (`0x7567c4`, `0x7567ec`): the bearing to the victim, goal row 5
/// while the unit stands. Kept until the unit comes back to life (`0x73d536`), which the health
/// rising from 0 stands for.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(crate) struct FacingHold {
    yaw: f32,
    /// The health read 0 since the hold was set.
    died: bool,
}

impl FacingHold {
    pub(crate) fn new(yaw: f32) -> Self {
        Self { yaw, died: false }
    }
}

/// Which goal-chain row won, for the `face` trace.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GoalSource {
    Raw,
    /// Row 5, an attack stop's [`FacingHold`].
    Held,
    /// Row 7, the bearing to `UNIT_FIELD_TARGET`.
    Target,
    /// Row 9, the bearing to the local player from the interaction NPC.
    Interact,
}

impl GoalSource {
    fn tag(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Held => "held",
            Self::Target => "target",
            Self::Interact => "interact",
        }
    }
}

/// A [`MonsterMoveFacing`] as a WoW orientation for a unit at `unit_pos`: `Spot` and `Target`
/// are the horizontal `atan2(dy, dx)` bearing; `None` also for an unstreamed or coincident target.
pub(in crate::net) fn resolve_facing(
    facing: MonsterMoveFacing,
    unit_pos: [f32; 3],
    target_pos: impl FnOnce(u64) -> Option<[f32; 3]>,
) -> Option<f32> {
    let bear = |to: [f32; 3]| {
        let (dx, dy) = (to[0] - unit_pos[0], to[1] - unit_pos[1]);
        (dx * dx + dy * dy > 1e-6).then(|| dy.atan2(dx))
    };
    match facing {
        MonsterMoveFacing::None => None,
        MonsterMoveFacing::Angle(a) => Some(a),
        MonsterMoveFacing::Spot(spot) => bear(spot),
        MonsterMoveFacing::Target(guid) => target_pos(guid).and_then(bear),
    }
}

/// A finished walk's final facing (`0x6eb0b0`'s done branch), resolved where the walk ended and
/// handed to [`drive_display_facing`] as the raw goal it turns toward.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(crate) struct WalkEndFacing(pub(crate) MonsterMoveFacing);

/// Sets each finished walk's raw facing: an angle as is, a point or a unit as the bearing to it; a
/// target not streamed in leaves the facing alone, as the reference's lookup miss does.
pub(crate) fn apply_walk_end_facing(
    mut commands: Commands,
    index: Res<GuidIndex>,
    ends: Query<(Entity, &WalkEndFacing)>,
    transforms: Query<&Transform>,
) {
    for (e, end) in &ends {
        commands.entity(e).try_remove::<WalkEndFacing>();
        let Ok(t) = transforms.get(e) else {
            continue;
        };
        let target = |g: u64| {
            index
                .0
                .get(&g)
                .and_then(|&te| transforms.get(te).ok())
                .map(|t| bevy_to_wow(t.translation))
        };
        if let Some(wire) = resolve_facing(end.0, bevy_to_wow(t.translation), target) {
            commands.entity(e).try_insert(DisplayFacing {
                wire: wire.rem_euclid(std::f32::consts::TAU),
                hist: [0.0; 4],
                velocity: 0.0,
                shown: None,
                deck: None,
            });
        }
    }
}

/// The horizontal bearing from `from` to `to`, `None` when they coincide.
pub(crate) fn bearing(from: [f32; 3], to: [f32; 3]) -> Option<f32> {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    (dx * dx + dy * dy >= 1.0e-4).then(|| dy.atan2(dx))
}

/// Goal-chain row 3: a state emote (`UNIT_NPC_EMOTESTATE`) whose `Emotes.dbc` flags lack
/// [`EMOTE_PERMITS_FACING`] pins the unit to its raw facing; an absent or unknown emote suppresses
/// nothing.
fn emote_suppresses_facing(emote_state: u32, flags: impl FnOnce(u32) -> Option<u32>) -> bool {
    if emote_state == 0 {
        return false;
    }
    match flags(emote_state) {
        Some(f) => f & EMOTE_PERMITS_FACING == 0,
        None => false, // the client's `rec == 0` leg
    }
}

/// One pump of the client's `+0xc98` box filter. It has no `dt`: the reference halves the error per
/// frame, not per second.
fn filter_step(cur: f32, goal: f32, hist: &mut [f32; 4]) -> f32 {
    use std::f32::consts::TAU;
    let raw_delta = goal - cur;
    if raw_delta.abs() <= DEAD_BAND {
        *hist = [0.0; 4];
        return goal.rem_euclid(TAU);
    }
    let delta = crate::creature_anim::wrap_pi(raw_delta);
    // A reversal empties the ring.
    if delta * hist[0] < 0.0 {
        hist[0] = 0.0;
    }
    let step = if hist[0] == 0.0 {
        *hist = [delta; 4]; // the ring-fill leg, no averaging
        delta
    } else {
        hist.copy_within(0..3, 1); // `memmove(+0xca0, +0xc9c, 12)`
        hist[0] = delta;
        let avg = hist.iter().sum::<f32>() * RING_AVG;
        // Never overshoot the goal.
        if avg.abs() > delta.abs() {
            delta
        } else {
            avg
        }
    };
    (cur + step * STEP_FRACTION).rem_euclid(TAU)
}

/// How a unit's display facing steps toward its goal (`0x735f60`): a damped turn at its
/// template's `CreatureMovementInfo` rate when that is over `1e-5`, else a damped turn at 30 in
/// combat and 20 out of it for a non-player unit with `UNIT_FLAG_PLAYER_CONTROLLED`, else the box
/// filter.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Smoothing {
    Damped(f32),
    Filter,
}

fn smoothing(template_rate: Option<f32>, unit_flags: u32, engaged: bool) -> Smoothing {
    if let Some(rate) = template_rate.filter(|&r| CHASE_RATE_FLOOR < r) {
        return Smoothing::Damped(rate);
    }
    if unit_flags & UNIT_FLAG_PLAYER_CONTROLLED != 0 {
        let in_combat = unit_flags & UNIT_FLAG_PET_IN_COMBAT != 0 || engaged;
        let (fighting, idle) = CONTROLLED_CHASE_RATE;
        return Smoothing::Damped(if in_combat { fighting } else { idle });
    }
    Smoothing::Filter
}

/// One step of the critically damped turn (`0x7160b0`) toward `goal`, unfolded to within π of
/// `cur` first, over `dt` seconds; the result is wrapped to `[0, 2π)`.
fn damped_step(cur: f32, goal: f32, rate: f32, dt: f32, velocity: &mut f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let goal = if goal + PI < cur {
        goal + TAU
    } else if goal - PI > cur {
        goal - TAU
    } else {
        goal
    };
    let k = rate * dt;
    let damping = 1.0 / (k + (k * k * k * 0.235 + k * k * 0.48) + 1.0);
    let offset = cur - goal;
    let change = dt * (offset * rate + *velocity);
    let value = (offset + change) * damping + goal;
    *velocity = (*velocity - rate * change) * damping;
    // Deviation: within a millionth of a radian and all but at rest, the turn takes the goal. The
    // reference has no snap; without one a yaw closing on 0 creeps through ever finer floats for
    // most of a minute, rewriting the transform each frame for an invisible change.
    if (value - goal).abs() < SETTLE_BAND && velocity.abs() < SETTLE_BAND * rate {
        *velocity = 0.0;
        return goal.rem_euclid(TAU);
    }
    value.rem_euclid(TAU)
}

/// The damped turn's settle band (rad), far under a visible turn.
const SETTLE_BAND: f32 = 1.0e-6;

/// Turns every stationary unit toward the goal the client's chain picks, in its branch order (the
/// order is the behaviour):
///
/// | row | test | goal |
/// |---|---|---|
/// | 0 | the body we steer | excluded by [`ActiveMover`] |
/// | 1 | moving | raw, excluded by [`Spline`] |
/// | 2 | `standState != 0` | raw |
/// | 3 | a state emote lacking [`EMOTE_PERMITS_FACING`] | raw |
/// | 4 | [`UNIT_FLAG_STUNNED`] | raw |
/// | 5 | an attack stop's hold, `+0xa30 & 1` (`0x7360eb`) | [`FacingHold`]'s bearing |
/// | 6 | a remote player | raw, excluded by [`RemoteMotion`] |
/// | 7 | `UNIT_FIELD_TARGET` resolves | bearing to that unit |
/// | 9 | this unit is the interaction NPC | bearing to the local player |
/// | - | none of the above | raw |
///
/// No range, line-of-sight, faction or alive test is on this path; the window's own range close
/// is the only leash. Deviation: a remote player's facing is the remote interp's, so row 5 never
/// reaches one; a server sets the hold for none on AzerothCore.
///
/// A deck rider turns in its deck's frame: its raw goal is the wire's deck-local facing, and a
/// bearing goal is taken off the deck's yaw (`0x7362e3`). Deviation: the reference reads row 5's
/// world bearing (`0x7567ec`) as deck-local; benilla takes it off the deck's yaw too.
///
/// The step toward the goal is [`Smoothing`]'s: the template's damped turn, the player-controlled
/// damped turn, or the box filter.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(crate) fn drive_display_facing(
    mut commands: Commands,
    index: Res<GuidIndex>,
    time: Option<Res<Time>>,
    names: Option<Res<crate::names::NameCache>>,
    rates: Option<Res<ChaseRates>>,
    // `Option<Res<_>>` throughout: a headless test mounts this system without the UI plugins.
    interact: Option<Res<crate::ui_session::InteractTarget>>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
    // A parked rig is drawn nowhere: its display facing resumes from its state when it wakes, as
    // the reference ticks no pass-2 unit.
    candidates: Query<
        (
            Entity,
            &NetEntity,
            &ObjectStore,
            Option<&crate::transport::TransportRider>,
            Option<&crate::creature_anim::Engaged>,
        ),
        (
            Without<Spline>,
            Without<RemoteMotion>,
            Without<ActiveMover>,
            Without<benilla_world::rig_anim::AnimParked>,
            // A fading model's transform is detached (`0x783630`).
            Without<benilla_world::model_fade::DespawnFade>,
        ),
    >,
    self_q: Query<Entity, With<SelfPlayer>>,
    decks: Query<(), With<crate::transport::Transport>>,
    mut transforms: Query<&mut Transform>,
    mut facings: Query<&mut DisplayFacing>,
    // A remote player's latch belongs to the remote facing interp; sweeping it here would fight it.
    latched: Query<Entity, (With<FacingStep>, Without<RemoteMotion>)>,
    // A unit that starts moving drops its state, so the next seed reads the pose it stops at.
    ungoverned: Query<Entity, (With<DisplayFacing>, Or<(With<Spline>, With<RemoteMotion>)>)>,
    mut holds: Query<(Entity, &ObjectStore, &mut FacingHold)>,
) {
    for (e, store, mut hold) in &mut holds {
        match store.0.unit_health() {
            Some(0) => hold.died = true,
            Some(_) if hold.died => {
                commands.entity(e).remove::<FacingHold>();
            }
            _ => {}
        }
    }
    let self_pos = self_q
        .iter()
        .next()
        .and_then(|e| transforms.get(e).ok())
        .map(|t| bevy_to_wow(t.translation));
    let interact_npc = interact.and_then(|t| crate::ui_session::interact_entity(&t, &index));

    // Collect goals first, then apply, so no transform is borrowed mutably while another is read.
    let mut goals: Vec<(Entity, Option<f32>, GoalSource, Riding, Smoothing)> = Vec::new();
    for (e, net, store, rider, engaged) in &candidates {
        // A GameObject sits at its authored facing; a player is no `Unit`, which keeps
        // `smoothing`'s player-controlled arm to non-players, as the reference's type test does.
        if net.kind != EntityKind::Unit {
            continue;
        }
        // In combat reads the auto-attack guid as non-zero (`0x71afb0`).
        let engaged = engaged.is_some_and(|g| g.0 != 0);
        let riding = match rider {
            None => Riding::No,
            Some(r) => match index.0.get(&r.transport_guid).copied() {
                Some(d) if decks.contains(d) => match transforms.get(d) {
                    Ok(deck) => Riding::Deck(Deck {
                        yaw: deck.rotation.to_euler(EulerRot::YXZ).0,
                        wire: r.local_orientation.rem_euclid(std::f32::consts::TAU),
                    }),
                    Err(_) => continue,
                },
                // A rider whose deck is not streamed holds its last pose, as `compose_riders` does.
                None => continue,
                Some(_) => Riding::Seat,
            },
        };
        let fields = &store.0;
        let template_rate = fields
            .object_entry()
            .and_then(|entry| names.as_ref()?.creature_record(entry))
            .and_then(|rec| rates.as_ref()?.0.chase_rate(rec.movement_id));
        let smooth = smoothing(template_rate, fields.unit_flags(), engaged);
        // Rows 2-4.
        if fields.unit_stand_state() != 0
            || fields.unit_flags() & UNIT_FLAG_STUNNED != 0
            || emote_suppresses_facing(fields.unit_emote_state(), |id| {
                emotes.as_ref().and_then(|c| c.emote_flags(id))
            })
        {
            goals.push((e, None, GoalSource::Raw, riding, smooth));
            continue;
        }
        // Row 5.
        if let Ok((_, _, hold)) = holds.get(e) {
            if !hold.died {
                goals.push((e, Some(hold.yaw), GoalSource::Held, riding, smooth));
                continue;
            }
        }
        let Ok(unit_t) = transforms.get(e) else {
            continue;
        };
        let unit_pos = bevy_to_wow(unit_t.translation);
        // Row 7.
        let target = fields
            .unit_target()
            .and_then(|g| index.0.get(&g).copied())
            .and_then(|te| transforms.get(te).ok())
            .and_then(|t| bearing(unit_pos, bevy_to_wow(t.translation)));
        if let Some(goal) = target {
            goals.push((e, Some(goal), GoalSource::Target, riding, smooth));
            continue;
        }
        // Row 9: the interact target (`0xbd07a8`).
        let facing_me = (interact_npc == Some(e))
            .then_some(self_pos)
            .flatten()
            .and_then(|p| bearing(unit_pos, p));
        match facing_me {
            Some(goal) => goals.push((e, Some(goal), GoalSource::Interact, riding, smooth)),
            None => goals.push((e, None, GoalSource::Raw, riding, smooth)),
        }
    }

    let dt = time.map_or(0.0, |t| t.delta_secs());
    let mut stepping: bevy::ecs::entity::EntityHashSet = default();
    for (e, goal, source, riding, smooth) in goals {
        use std::f32::consts::TAU;
        let deck = match riding {
            Riding::Deck(d) => Some(d),
            _ => None,
        };
        let Ok(mut tf) = transforms.get_mut(e) else {
            continue;
        };
        let Ok(mut state) = facings.get_mut(e) else {
            // First frame governed: the transform still holds the wire's facing, so seed from it
            // with a clean ring.
            commands.entity(e).try_insert(DisplayFacing {
                wire: yaw_of(tf.rotation).rem_euclid(TAU),
                hist: [0.0; 4],
                velocity: 0.0,
                shown: None,
                deck: deck.map(|d| d.wire),
            });
            continue;
        };
        // On a deck the pump runs in the deck's frame, from the wire's deck-local facing.
        let (cur, raw, frame) = match riding {
            Riding::Deck(d) => (state.deck.unwrap_or(d.wire), d.wire, d.yaw),
            // A vehicle seat places its passenger itself, so its raw goal is the pose it sits at.
            Riding::Seat => {
                state.wire = yaw_of(tf.rotation).rem_euclid(TAU);
                (state.wire, state.wire, 0.0)
            }
            // Its own last yaw while the transform still holds it; anything else that turned the
            // body since is read back off the quaternion.
            Riding::No => {
                let cur = state
                    .shown
                    .filter(|&y| tf.rotation == Quat::from_rotation_y(y))
                    .unwrap_or_else(|| yaw_of(tf.rotation).rem_euclid(TAU));
                (cur, state.wire, 0.0)
            }
        };
        // The raw facing is a goal, not a freeze: it swings an NPC back when its window closes.
        let goal = goal.map_or(raw, |g| (g - frame).rem_euclid(TAU));
        let new = match smooth {
            Smoothing::Damped(rate) => damped_step(cur, goal, rate, dt, &mut state.velocity),
            Smoothing::Filter => filter_step(cur, goal, &mut state.hist),
        };
        state.deck = deck.is_some().then_some(new);
        state.shown = (riding == Riding::No).then_some(new);
        // Write only on change: a settled unit's `new` is bitwise the goal, so a standing crowd
        // stops tripping every downstream `Changed<Transform>` gate.
        let rot = Quat::from_rotation_y(new + frame);
        if tf.rotation != rot {
            tf.rotation = rot;
        }
        // The latch tests the yaw this pump applied (the shuffle latch's input, the increment
        // written to `+0xc94`), not the gap left; folded because a pump may cross the wrap.
        let step = crate::creature_anim::wrap_pi(new - cur);
        if step.abs() > TURN_LATCH_BAND {
            commands.entity(e).insert(FacingStep(step));
            stepping.insert(e);
        }
        trace_face(e, source, goal, cur, new, deck.map(|d| d.yaw));
    }
    for e in &latched {
        if !stepping.contains(&e) {
            commands.entity(e).remove::<FacingStep>();
        }
    }
    for e in &ungoverned {
        commands.entity(e).remove::<DisplayFacing>();
    }
}

/// What a candidate rides: nothing, a boat or zeppelin deck, or a vehicle seat.
#[derive(Clone, Copy, PartialEq)]
enum Riding {
    No,
    Deck(Deck),
    Seat,
}

/// A deck rider's frame for the pump: the deck's yaw and the wire's deck-local facing.
#[derive(Clone, Copy, PartialEq)]
struct Deck {
    yaw: f32,
    wire: f32,
}

/// The `face` trace tag: one line per turning unit per frame, naming the goal-chain row that won;
/// a deck rider's facings are deck-local, with the deck's yaw beside them.
fn trace_face(e: Entity, source: GoalSource, goal: f32, cur: f32, new: f32, deck: Option<f32>) {
    if !benilla_assets::trace::enabled_for("face") || (new - cur).abs() < 1.0e-6 {
        return;
    }
    benilla_assets::trace::line(
        "face",
        &format!(
            "e={e} src={} goal={goal:.4} cur={cur:.4} new={new:.4} step={:.4}{}",
            source.tag(),
            new - cur,
            deck.map(|d| format!(" deck={d:.4}")).unwrap_or_default()
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::coords::wow_to_bevy;

    /// It comes and goes with the driver's grip: a table column would split the unit tables.
    #[test]
    fn display_facing_is_a_sparse_set() {
        use bevy::ecs::component::StorageType;
        assert_eq!(
            <DisplayFacing as Component>::STORAGE_TYPE,
            StorageType::SparseSet
        );
    }
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

    /// The dead-band is inclusive.
    #[test]
    fn inside_the_dead_band_the_goal_is_taken_verbatim() {
        let mut hist = [0.3; 4];
        let out = filter_step(1.0, 1.0 + DEAD_BAND, &mut hist);
        assert!((out - (1.0 + DEAD_BAND)).abs() < 1.0e-6, "snapped: {out}");
        assert_eq!(hist, [0.0; 4], "the ring resets inside the dead-band");
    }

    #[test]
    fn the_first_pump_fills_the_ring_and_halves_the_error() {
        let mut hist = [0.0; 4];
        let out = filter_step(0.0, FRAC_PI_2, &mut hist);
        assert!((out - FRAC_PI_2 / 2.0).abs() < 1.0e-5, "half way: {out}");
        assert_eq!(hist, [FRAC_PI_2; 4], "the ring fills with the delta");
    }

    #[test]
    fn the_error_halves_per_pump_and_pi_settles_in_nine() {
        let mut hist = [0.0; 4];
        let (goal, mut cur) = (PI, 0.0);
        let mut pumps = 0;
        while crate::creature_anim::wrap_pi(goal - cur).abs() > DEAD_BAND && pumps < 64 {
            let prev = crate::creature_anim::wrap_pi(goal - cur).abs();
            cur = filter_step(cur, goal, &mut hist);
            let now = crate::creature_anim::wrap_pi(goal - cur).abs();
            assert!(now < prev, "each pump closes the gap: {prev} -> {now}");
            pumps += 1;
        }
        assert!(
            (8..=9).contains(&pumps),
            "pi settles in 8-9 pumps, took {pumps}"
        );
    }

    #[test]
    fn a_reversal_resets_the_ring_head() {
        let mut hist = [0.4; 4];
        filter_step(0.0, -1.0, &mut hist);
        assert!(
            hist[0] < 0.0,
            "the head carries the new direction: {hist:?}"
        );
        assert_eq!(
            hist[1], hist[0],
            "and the ring refilled rather than averaged"
        );
    }

    /// The step picker's order (`0x735f60`): a template rate over `1e-5` first, then the
    /// player-controlled turn at 30 in combat (the `0x800` flag or auto-attack) and 20 out of it,
    /// else the box filter.
    #[test]
    fn the_template_rate_then_player_control_pick_the_damped_turn() {
        assert_eq!(smoothing(Some(5.0), 0, false), Smoothing::Damped(5.0));
        assert_eq!(smoothing(Some(5.0), 0x8, true), Smoothing::Damped(5.0));
        assert_eq!(smoothing(Some(0.0), 0, false), Smoothing::Filter);
        assert_eq!(smoothing(None, 0, false), Smoothing::Filter);
        assert_eq!(smoothing(Some(0.0), 0x8, false), Smoothing::Damped(20.0));
        assert_eq!(smoothing(None, 0x8 | 0x800, false), Smoothing::Damped(30.0));
        assert_eq!(smoothing(None, 0x8, true), Smoothing::Damped(30.0));
        assert_eq!(smoothing(None, 0x800, true), Smoothing::Filter);
    }

    /// Fed its own last yaw, as the pump keeps it, the damped turn comes to a bitwise rest, so a
    /// settled unit stops writing its transform; north (0) at the combat rate included.
    #[test]
    fn the_damped_turn_settles_bitwise_on_its_own_yaw() {
        for goal in [0.0, 1.0, 3.0, 6.2] {
            for rate in [2.0, 20.0, 30.0] {
                let (mut cur, mut velocity) = (goal + 2.5_f32, 0.0);
                let settled = (0..2_000).any(|_| {
                    let new = damped_step(cur, goal, rate, 1.0 / 60.0, &mut velocity);
                    let still = new == cur;
                    cur = new;
                    still
                });
                assert!(settled, "goal {goal} rate {rate}: still turning at {cur}");
            }
        }
    }

    /// The damped turn closes on the goal over time, the short way round the wrap, and holds still
    /// for a zero `dt`.
    #[test]
    fn the_damped_turn_closes_on_its_goal_the_short_way() {
        let mut v = 0.0;
        let mut cur = TAU - 0.1;
        for _ in 0..120 {
            cur = damped_step(cur, 0.1, 10.0, 1.0 / 60.0, &mut v);
        }
        let left = crate::creature_anim::wrap_pi(0.1 - cur);
        assert!(
            left.abs() < 0.01,
            "settled near the goal: {cur} ({left} left)"
        );
        let mut v = 0.0;
        let first = damped_step(TAU - 0.1, 0.1, 10.0, 1.0 / 60.0, &mut v);
        let moved = crate::creature_anim::wrap_pi(first - (TAU - 0.1));
        assert!(moved > 0.0 && moved < 0.2, "short way, part way: {moved}");
        let mut v = 0.0;
        assert_eq!(damped_step(1.0, 2.0, 10.0, 0.0, &mut v), 1.0);
    }

    /// The reference's damped turn (`0x7160b0`) on known numbers: one 0.1 s step at rate 2 from
    /// rest, 1 rad short of the goal.
    #[test]
    fn one_damped_step_matches_the_reference_formula() {
        let mut v = 0.0;
        let out = damped_step(1.0, 2.0, 2.0, 0.1, &mut v);
        let k = 0.2f32;
        let damping = 1.0 / (k + (k * k * k * 0.235 + k * k * 0.48) + 1.0);
        // `dt · (offset · rate + velocity)`, offset −1, rate 2, velocity 0.
        let change = 0.1 * -2.0;
        assert!((out - ((-1.0 + change) * damping + 2.0)).abs() < 1e-6);
        assert!((v - (-2.0 * change) * damping).abs() < 1e-6);
    }

    #[test]
    fn the_turn_takes_the_short_arc() {
        let mut hist = [0.0; 4];
        // The goal is 0.2 rad counterclockwise of `cur`, across the wrap.
        let out = filter_step(TAU - 0.1, 0.1, &mut hist);
        let moved = crate::creature_anim::wrap_pi(out - (TAU - 0.1));
        assert!(moved > 0.0 && moved < 0.2, "short way, part way: {moved}");
    }

    #[test]
    fn the_emote_state_gate_matches_the_clients_bl() {
        assert!(!emote_suppresses_facing(0, |_| Some(0)), "no emote state");
        assert!(!emote_suppresses_facing(7, |_| None), "unknown record");
        assert!(
            emote_suppresses_facing(7, |_| Some(0)),
            "a valid record without the bit suppresses"
        );
        assert!(
            !emote_suppresses_facing(7, |_| Some(EMOTE_PERMITS_FACING)),
            "the bit permits the facing"
        );
    }

    /// A finished walk's final facing is the pump's raw goal: an angle as is, a unit as the
    /// bearing to it, turned toward rather than snapped.
    #[test]
    fn a_walk_end_facing_becomes_the_raw_goal() {
        let mut app = App::new();
        app.init_resource::<GuidIndex>().add_systems(
            Update,
            (apply_walk_end_facing, drive_display_facing).chain(),
        );
        let unit = |app: &mut App, facing| {
            app.world_mut()
                .spawn((
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    ObjectStore::default(),
                    Transform::default(),
                    WalkEndFacing(facing),
                ))
                .id()
        };
        let angled = unit(&mut app, MonsterMoveFacing::Angle(FRAC_PI_2));
        let target = app
            .world_mut()
            .spawn(Transform::from_translation(wow_to_bevy([0.0, -5.0, 0.0])))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0x42, target);
        let facing_it = unit(&mut app, MonsterMoveFacing::Target(0x42));
        app.update();
        let wire =
            |app: &App, e: Entity| app.world().entity(e).get::<DisplayFacing>().unwrap().wire;
        assert!((wire(&app, angled) - FRAC_PI_2).abs() < 1e-6);
        assert!((wire(&app, facing_it) - 1.5 * PI).abs() < 1e-5, "due -y");
        let yaw = |app: &App, e: Entity| {
            yaw_of(app.world().entity(e).get::<Transform>().unwrap().rotation)
        };
        let first = yaw(&app, angled);
        assert!(
            first > 0.0 && first < FRAC_PI_2,
            "the pump turns part way, no snap: {first}"
        );
        assert!(app.world().entity(angled).get::<WalkEndFacing>().is_none());
    }

    /// The interact target on `npc`, under a guid the index maps to it.
    fn interact(app: &mut App, npc: Option<Entity>) {
        let guid = npc.map(Entity::to_bits);
        if let (Some(e), Some(g)) = (npc, guid) {
            app.world_mut().resource_mut::<GuidIndex>().0.insert(g, e);
        }
        app.world_mut()
            .resource_mut::<crate::ui_session::InteractTarget>()
            .0 = guid;
    }

    #[test]
    fn the_interaction_npc_turns_to_face_us_and_swings_back_on_close() {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractTarget>()
            .add_systems(Update, drive_display_facing);
        // Us at the origin; the vendor 5 yd along +x, facing +x, away from us.
        let me = app
            .world_mut()
            .spawn((SelfPlayer, ActiveMover, Transform::default()))
            .id();
        let _ = me;
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform {
                    translation: wow_to_bevy([5.0, 0.0, 0.0]),
                    rotation: Quat::from_rotation_y(0.0),
                    ..default()
                },
            ))
            .id();

        let yaw = |app: &App| yaw_of(app.world().entity(npc).get::<Transform>().unwrap().rotation);
        app.update();
        assert!(
            (yaw(&app) - 0.0).abs() < 1.0e-4,
            "the seeding frame does not turn: {}",
            yaw(&app)
        );
        assert!(
            app.world().entity(npc).contains::<DisplayFacing>(),
            "the seeding frame inserts the smoother state"
        );
        for _ in 0..16 {
            app.update();
        }
        assert!(
            (yaw(&app) - 0.0).abs() < 1.0e-4,
            "no window, no turn: {}",
            yaw(&app)
        );

        // We are due west of the vendor: a bearing of pi.
        interact(&mut app, Some(npc));
        for _ in 0..16 {
            app.update();
        }
        let facing_us = yaw(&app);
        assert!(
            crate::creature_anim::wrap_pi(facing_us - PI).abs() <= DEAD_BAND,
            "the vendor faces us within the dead-band, got {facing_us}"
        );

        interact(&mut app, None);
        for _ in 0..16 {
            app.update();
        }
        let back = yaw(&app);
        assert!(
            crate::creature_anim::wrap_pi(back - 0.0).abs() <= DEAD_BAND,
            "the vendor swings back to its authored heading, got {back}"
        );
    }

    /// The latch is held while under 0.05 rad of the turn remains, a gap a "close enough"
    /// threshold would release.
    #[test]
    fn the_latch_carries_the_step_applied_not_the_gap_remaining() {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractTarget>()
            .add_systems(Update, drive_display_facing);
        // A bearing of +2.601 rad, reached counterclockwise: every step is a left turn.
        app.world_mut().spawn((
            SelfPlayer,
            ActiveMover,
            Transform::from_translation(wow_to_bevy([0.0, 3.0, 0.0])),
        ));
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform {
                    translation: wow_to_bevy([5.0, 0.0, 0.0]),
                    rotation: Quat::from_rotation_y(0.0),
                    ..default()
                },
            ))
            .id();
        let yaw = |app: &App| yaw_of(app.world().entity(npc).get::<Transform>().unwrap().rotation);
        let step = |app: &App| app.world().entity(npc).get::<FacingStep>().map(|f| f.0);

        app.update(); // the seeding frame
        interact(&mut app, Some(npc));

        let goal = 3.0f32.atan2(-5.0);
        let (mut latched_inside_the_old_band, mut pumps) = (false, 0);
        for _ in 0..24 {
            let before = yaw(&app);
            app.update();
            let after = yaw(&app);
            let applied = crate::creature_anim::wrap_pi(after - before);
            match step(&app) {
                Some(s) => {
                    pumps += 1;
                    assert!(
                        (s - applied).abs() < 1.0e-6,
                        "the latch carries the applied step: {s} vs {applied}"
                    );
                    assert!(s > 0.0, "a counterclockwise turn latches LEFT: {s}");
                    if crate::creature_anim::wrap_pi(goal - after).abs() < 0.05 {
                        latched_inside_the_old_band = true;
                    }
                }
                None => assert!(
                    applied.abs() <= TURN_LATCH_BAND,
                    "unlatched, but the body moved {applied}"
                ),
            }
        }
        assert!(
            (9..=11).contains(&pumps),
            "this bearing closes in ten pumps, got {pumps}"
        );
        assert!(
            latched_inside_the_old_band,
            "the latch outlives the old ~3° stand-in — that gap IS the missing half of the shuffle"
        );
        assert!(
            !app.world().entity(npc).contains::<FacingStep>(),
            "and drops once the body stops moving"
        );
    }

    /// Row 7 sits above row 9: a fighting vendor keeps facing its victim.
    #[test]
    fn a_target_outranks_the_interaction_face_me() {
        use benilla_protocol::ObjectFields;
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractTarget>()
            .add_systems(Update, drive_display_facing);
        app.world_mut()
            .spawn((SelfPlayer, ActiveMover, Transform::default()));
        // The victim is due north of the vendor: a bearing of +pi/2.
        let victim = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform::from_translation(wow_to_bevy([5.0, 5.0, 0.0])),
            ))
            .id();
        // UNIT_FIELD_TARGET, a guid pair; the victim is guid 7.
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore(ObjectFields::from_pairs(&[
                    (benilla_protocol::field::FIELD_UNIT_TARGET, 7),
                    (benilla_protocol::field::FIELD_UNIT_TARGET + 1, 0),
                ])),
                Transform::from_translation(wow_to_bevy([5.0, 0.0, 0.0])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(7, victim);
        interact(&mut app, Some(npc));
        for _ in 0..24 {
            app.update();
        }
        let yaw = yaw_of(app.world().entity(npc).get::<Transform>().unwrap().rotation);
        assert!(
            crate::creature_anim::wrap_pi(yaw - FRAC_PI_2).abs() <= DEAD_BAND,
            "the vendor squares up on its victim (+pi/2), not on us (pi): {yaw}"
        );
    }

    #[test]
    fn a_settled_unit_stops_dirtying_its_transform() {
        #[derive(Resource, Default)]
        struct Dirty(usize);
        fn spy(q: Query<Entity, (Changed<Transform>, With<NetEntity>)>, mut out: ResMut<Dirty>) {
            out.0 = q.iter().count();
        }
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<crate::ui_session::InteractTarget>()
            .init_resource::<Dirty>()
            .add_systems(Update, (drive_display_facing, spy).chain());
        // Off the vendor's axis: a goal at exactly pi sits on the wrap, where the unfolded-delta
        // dead-band never snaps and the ease only goes quiet by underflow, ~16 pumps later.
        app.world_mut().spawn((
            SelfPlayer,
            ActiveMover,
            Transform::from_translation(wow_to_bevy([0.0, 3.0, 0.0])),
        ));
        let npc = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform {
                    translation: wow_to_bevy([5.0, 0.0, 0.0]),
                    rotation: Quat::from_rotation_y(0.0),
                    ..default()
                },
            ))
            .id();
        // The control: a turning vendor dirties its transform.
        interact(&mut app, Some(npc));
        app.update(); // seeding frame (spawn itself also reads as Changed here)
        app.update();
        assert!(
            app.world().resource::<Dirty>().0 > 0,
            "a turning unit dirties its transform"
        );
        // The turn settles in about nine pumps.
        for _ in 0..16 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<Dirty>().0,
            0,
            "a settled unit must stop dirtying its transform"
        );
    }

    #[test]
    fn a_coincident_pair_has_no_bearing() {
        assert!(bearing([1.0, 2.0, 3.0], [1.0, 2.0, 9.0]).is_none());
        let b = bearing([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]).expect("a real bearing");
        assert!((b - std::f32::consts::FRAC_PI_4).abs() < 1.0e-5, "{b}");
    }

    /// Row 5 (`0x7360eb`): an attack stop's hold outranks the target row and stays until the
    /// unit comes back to life (`0x73d536`).
    #[test]
    fn an_attack_stop_hold_outranks_the_target_until_the_unit_lives_again() {
        use benilla_protocol::field::FIELD_UNIT_HEALTH;
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .add_systems(Update, drive_display_facing);
        let health = |hp: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                FIELD_UNIT_HEALTH,
                hp,
            )]))
        };
        let unit = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                health(100),
                Transform::default(),
                FacingHold::new(FRAC_PI_2),
            ))
            .id();
        for _ in 0..16 {
            app.update();
        }
        let yaw = |app: &App| {
            yaw_of(
                app.world()
                    .entity(unit)
                    .get::<Transform>()
                    .unwrap()
                    .rotation,
            )
        };
        assert!((yaw(&app) - FRAC_PI_2).abs() < 1e-4, "held: {}", yaw(&app));
        app.world_mut().entity_mut(unit).insert(health(0));
        app.update();
        assert!(
            app.world().entity(unit).contains::<FacingHold>(),
            "death keeps it"
        );
        app.world_mut().entity_mut(unit).insert(health(1));
        app.update();
        app.update();
        assert!(
            !app.world().entity(unit).contains::<FacingHold>(),
            "life releases it"
        );
    }

    /// A deck rider with no goal turns in its deck's frame toward the wire's deck-local facing:
    /// it follows the turning deck, never turns back toward the yaw it seeded, nor latches a
    /// shuffle.
    #[test]
    fn a_deck_rider_follows_its_deck_without_shuffling() {
        const DECK: u64 = 0x1fc0_0000_0000_0006;
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .add_systems(Update, drive_display_facing);
        let deck = app
            .world_mut()
            .spawn((crate::transport::Transport::parked(), Transform::default()))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(DECK, deck);
        let crew = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore::default(),
                Transform::default(),
                crate::transport::TransportRider {
                    transport_guid: DECK,
                    local_pos: [0.0; 3],
                    local_orientation: 0.0,
                    seat: -1,
                },
            ))
            .id();
        app.update(); // the seeding frame
        for k in 1..=8 {
            // The deck has turned since the seed.
            let deck_yaw = 0.3 * k as f32;
            app.world_mut()
                .entity_mut(deck)
                .get_mut::<Transform>()
                .unwrap()
                .rotation = Quat::from_rotation_y(deck_yaw);
            app.update();
            let yaw = yaw_of(
                app.world()
                    .entity(crew)
                    .get::<Transform>()
                    .unwrap()
                    .rotation,
            );
            assert!(
                crate::creature_anim::wrap_pi(yaw - deck_yaw).abs() < 1.0e-5,
                "the pump turned the rider off its deck: {yaw} vs {deck_yaw}"
            );
            assert!(
                !app.world().entity(crew).contains::<FacingStep>(),
                "a rider standing still on a turning deck latched a shuffle"
            );
        }
    }
}
