//! Unit animation: a per-unit state machine that picks an `AnimationData.dbc` clip from movement
//! state (the gait by speed and flags, the stand-state and jump Specials; one-shots), cross-fades
//! into it, and plays a locomotion clip at `speed / sequence.moveSpeed`. A one-shot plays full-body
//! on a standing idle unit and on the SpineLow torso-masked overlay while moving, seated or
//! airborne in combat. Death overrides everything.

use std::time::Instant;

use benilla_assets::{AnimClip, ModelAnimations};
use benilla_formats::AnimDataCatalog;
use bevy::prelude::*;

use benilla_assets::AssetSet;
use benilla_world::schedule::WorldStage;

/// The pure selection logic: the tables, the gait, swing and ready picks, the rate math.
pub(crate) mod select;
pub(crate) use select::{
    ease_strafe_yaw, move_flags, strafe_body_offset, swim_body_rotation, MovementState,
};
use select::{Mode, Special};

/// The strafe body counter-twist, composed onto SpineLow and Head after animation.
mod twist;
pub(crate) use twist::{wrap_pi, BodyTwist};

/// Parks an off-frustum rig's pose while its clocks run, as the reference does not tick one; a
/// parked unit's events fire only for a `MORE_AUDIBLE` template.
mod lod;

/// Wielded `(item class, subclass)` per hand as worn (the weapon getter's worn read, what the
/// paperdoll shows); behaviour reads `armed_main`/`armed_off`, which apply the disarm ladder.
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Wielded {
    pub(crate) main: Option<(u8, u8)>,
    pub(crate) off: Option<(u8, u8)>,
    /// The ranged-slot item's `(class, subclass)`, the auto-repeat idle's selector.
    pub(crate) ranged: Option<(u8, u8)>,
    /// The hand items' sheath types (0 none, 1 back 2H, 2 back staff, 3 hip, 4 shield); each picks
    /// its arm's draw/stow clip, HipSheath 90 for types 3 and 7, else Sheath 89.
    pub(crate) main_sheath: u8,
    pub(crate) off_sheath: u8,
    /// The ranged sheath type: it picks the draw clip, but the stow is always Sheath 89.
    pub(crate) ranged_sheath: u8,
    /// The ranged `InventoryType` (0 when empty): `0x1a` and `0x19` draw with the right arm,
    /// anything else (`0x0f`, a bow) with the left.
    pub(crate) ranged_inv: u32,
    /// Each held slot's `Material` (main, off, ranged), the draw/stow sound's only key; off the
    /// wire, since the weapon class does not determine it.
    pub(crate) materials: [u8; 3],
    /// `UNIT_FIELD_FLAGS & UNIT_FLAG_DISARMED`, replicated for every unit; the reference reads it
    /// inside the weapon getter (`[[unit+0x110]+0xa0] & 0x200000`), for creatures as for players.
    pub(crate) disarmed: bool,
}

/// `UNIT_FIELD_FLAGS` bit 21 (vmangos `UnitDefines.h:566`), tested inside the weapon getter, the
/// player's and the unit's alike.
pub(crate) const UNIT_FLAG_DISARMED: u32 = 0x0020_0000;

/// `ItemClass` 2, weapon: the disarm ladder's whole test, so a shield is never the hand it hides.
pub(crate) const ITEM_CLASS_WEAPON: u8 = 2;

/// The one held slot the disarm hides: the main hand if it holds a weapon, else the off hand if it
/// does (the weapon getter's held read); the ranged slot is never gated, as vmangos agrees
/// (`Unit.h:991`). Over classes so the action bar's equipped-item test can use it on raw inventory.
pub(crate) fn disarmed_hand(main_class: Option<u8>, off_class: Option<u8>) -> Option<usize> {
    if main_class == Some(ITEM_CLASS_WEAPON) {
        return Some(0);
    }
    if off_class == Some(ITEM_CLASS_WEAPON) {
        return Some(1);
    }
    None
}

impl Wielded {
    /// The held slot this unit's disarm hides now, `None` while the flag is down.
    pub(crate) fn disarmed_hand(&self) -> Option<usize> {
        self.disarmed
            .then(|| disarmed_hand(self.main.map(|(c, _)| c), self.off.map(|(c, _)| c)))
            .flatten()
    }

    /// The main hand as the weapon getter's held read sees it, read by the swing, the Ready idle,
    /// the parry table, the sheath machine and the swing sound.
    pub(crate) fn armed_main(&self) -> Option<(u8, u8)> {
        self.main.filter(|_| self.disarmed_hand() != Some(0))
    }

    /// The off hand as the weapon getter's held read sees it.
    pub(crate) fn armed_off(&self) -> Option<(u8, u8)> {
        self.off.filter(|_| self.disarmed_hand() != Some(1))
    }
}

/// Auto-attacking (`SMSG_ATTACKSTART` to `ATTACKSTOP`), holding the target guid, the reference's
/// `[+0xc48]`: the Ready idle gates on it, not on sheath; the reference's reach check reads it.
#[derive(Component)]
pub(crate) struct Engaged(pub(crate) u64);

/// The local player's auto-repeat is armed: the reference's `[+0xd58] & 0x200`, set only by the
/// local cast-send; with the ranged sheath drawn a standing unit idles on the Load clip. Cleared
/// only through `cancel_auto_repeat_local`, including on `SMSG_CANCEL_AUTO_REPEAT`, which vmangos
/// sends when a volley's target dies (`SpellCaster.cpp:1829`). A remote shooter plays LoadBow once
/// at its `SMSG_SPELL_START` (`CastHold`), then the fire clip per GO; the reference holds HoldBow
/// (109) from the end of the Load until the first GO.
#[derive(Component)]
pub(crate) struct AutoRepeatArmed;

/// The ranged weapon-visual hold, `[+0xd58] & 0x400`, set for any caster whose ranged visual plays;
/// cleared by a non-ranged visual, the local cancel, melee attack-start or a spline move, never by
/// a sheath change or volley end. It gates nothing here: the reference re-arms the Hold at each
/// completion while it holds, ours loops the Hold once armed. It is no entry into the drawn idle:
/// read as one, it leaves a shooter aiming after a single Serpent Sting.
#[derive(Component)]
pub(crate) struct RangedHold;

/// The displayed nocked ammo, the reference's `[+0xd28]`/`[+0xd2c]` (the ammo display update): an
/// `ItemDisplayInfo` model at HandArrow (35), bow only, set per shot from the `SMSG_SPELL_START`
/// ammo tail. Removed by the un-nock: the local cancel, leaving the ranged sheath, or a weapon
/// change.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NockedAmmo {
    /// Never 0: an id-0 refresh removes the component.
    pub(crate) display_id: u32,
}

/// The nock latch `[+0xd58] & 0x4000`: set at `$BWP` (LoadBow ~0.6 s), cleared at `$BWR` (AttackBow
/// ~0.067 s) and on un-nock; the arrow model shows only while set.
#[derive(Component)]
pub(crate) struct NockLatch;

/// `$BWP` latches a unit with `NockedAmmo` cached, `$BWR` clears; runs after `fire_anim_events`
/// so a tag lands the frame it is crossed.
pub(crate) fn drive_nock_latch(
    mut events: MessageReader<AnimSoundEvent>,
    nocked: Query<Has<NockedAmmo>>,
    mut commands: Commands,
) {
    for ev in events.read() {
        match &ev.ident {
            b"$BWP" if nocked.get(ev.entity) == Ok(true) => {
                // `try_insert`: the unit may be despawned by the time the commands apply.
                commands.entity(ev.entity).try_insert(NockLatch);
            }
            b"$BWR" => {
                if let Ok(mut e) = commands.get_entity(ev.entity) {
                    e.remove::<NockLatch>();
                }
            }
            _ => {}
        }
    }
}

/// The one local auto-repeat cancel, as in the reference: clears the key, both idle bits and the
/// nock and sends `CMSG_CANCEL_AUTO_REPEAT_SPELL`; the sheath stays drawn.
pub(crate) fn cancel_auto_repeat_local(
    entity: Option<Entity>,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    if auto_repeat.0.is_none() {
        // The reference's early-out: nothing running, nothing sent.
        return;
    }
    auto_repeat.0 = None;
    if let Some(e) = entity {
        commands
            .entity(e)
            .remove::<(AutoRepeatArmed, RangedHold, NockedAmmo, NockLatch)>();
    }
    let _ = net.0.send(crate::net::ClientCommand::CancelAutoRepeat);
}

/// The local StopAttack, as in the reference: when attacking, `CMSG_ATTACKSTOP`, then the
/// queued-cast cancel drops a queued on-next-swing spell with `CMSG_CANCEL_CAST`, which is why Auto
/// Shot darkens a Raptor Strike ring. `Engaged` stays until the `SMSG_ATTACKSTOP` echo, as in the
/// reference.
pub(crate) fn stop_attack_local(
    engaged: bool,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    net: &crate::net::NetCommands,
) {
    if !engaged {
        // The reference's not-attacking and `[+0xc50] == 0` early-out (`[+0xc50]`, a locally
        // started attack, is not modelled): nothing sent, a queued strike survives.
        return;
    }
    let _ = net.0.send(crate::net::ClientCommand::AttackStop);
    if let Some(spell_id) = queued_melee.current() {
        let _ = net
            .0
            .send(crate::net::ClientCommand::CancelCast { spell_id });
        queued_melee.clear_if(spell_id);
    }
}

/// The local StartAttack, as in the reference: the swing send is skipped while attacking unless a
/// stop is in flight (`[+0xc54]`, which the target switch passes as `stop_in_flight`); the tail,
/// melee sheath snap then auto-repeat cancel, always runs, so melee and auto-shot are exclusive.
/// Callers own the entry checks: no call from a cast press while engaged, the attackability walk
/// and the world gate.
pub(crate) fn start_attack_local(
    entity: Entity,
    target: u64,
    engaged: bool,
    stop_in_flight: bool,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    sheath: &mut MessageWriter<SheathRequest>,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    // Starting an attack stands you up, after the attackability check but before the range logic
    // and the already-attacking skip, so a re-issued or out-of-reach attack stands you too.
    commands.write_message(crate::player::StandStateRequest { state: 0 });
    if !engaged || stop_in_flight {
        let _ = net
            .0
            .send(crate::net::ClientCommand::AttackSwing { guid: target });
    }
    // Idempotent, matching the reference's unconditional call.
    sheath.write(SheathRequest {
        entity,
        state: 1,
        ceremony: false,
    });
    cancel_auto_repeat_local(Some(entity), auto_repeat, commands, net);
}

/// The Attack toggle, as in the reference (the Attack pseudo-spell): stop when attacking, else
/// start. Only the start arm cancels auto-repeat, so toggling melee off leaves Auto Shot running.
/// The stop arm's world gate is not modelled.
pub(crate) fn toggle_attack_local(
    entity: Entity,
    target: u64,
    engaged: bool,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    sheath: &mut MessageWriter<SheathRequest>,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    if engaged {
        stop_attack_local(engaged, queued_melee, net);
    } else {
        start_attack_local(
            entity,
            target,
            engaged,
            false,
            auto_repeat,
            sheath,
            commands,
            net,
        );
    }
}

/// The attack seams' write set as one param, for the selection writers that run the reference's
/// stop, select, re-swing, and the loot its teardown closes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttackSeam<'w, 's> {
    pub(crate) net: Res<'w, crate::net::NetCommands>,
    pub(crate) queued_melee: ResMut<'w, crate::spell::QueuedMeleeSpell>,
    pub(crate) auto_repeat: ResMut<'w, crate::spell::AutoRepeatActive>,
    pub(crate) sheath: MessageWriter<'w, SheathRequest>,
    pub(crate) ecs: Commands<'w, 's>,
    /// Our own entity.
    pub(crate) me: Query<'w, 's, Entity, With<crate::net::SelfPlayer>>,
    /// The open loot, which the selection teardown closes when it loots the outgoing target.
    pub(crate) loot: ResMut<'w, crate::ui_loot::LootState>,
    /// The loot latch: the teardown's close drops it, and the right-click loot legs arm it.
    pub(crate) loot_latch: ResMut<'w, crate::ui_loot::LootLatch>,
    /// `stopAutoAttackOnTargetChange`; a harness without it reads the default, off.
    pub(crate) stop_on_change: Option<Res<'w, crate::target::StopAttackOnTargetChange>>,
}

impl AttackSeam<'_, '_> {
    /// The selection teardown's loot close: a window looting the outgoing selection closes as
    /// `CloseLoot` does, its release ahead of anything the new selection sends. The close's
    /// dead-unit deselect targets the selection already being torn down, so none is asked; the
    /// reference's nested teardown there also sends a `CMSG_SET_SELECTION` 0, which this does not.
    pub(crate) fn close_loot_on(&mut self, outgoing: u64) {
        if self.loot.source() == Some(outgoing) {
            debug!("target: the selection teardown closes the loot on {outgoing:#x}");
            crate::ui_loot::close_interaction(
                &mut self.loot,
                &mut self.loot_latch,
                &self.net,
                true,
            );
        }
    }

    /// Whether a new target stops the attack instead of swinging on (`0x524f59`).
    pub(crate) fn stops_on_target_change(&self) -> bool {
        self.stop_on_change.as_deref().is_some_and(|s| s.0)
    }

    /// The auto-repeat cancel (`0x807560`).
    pub(crate) fn cancel_auto_repeat(&mut self) {
        let me = self.me.single().ok();
        cancel_auto_repeat_local(me, &mut self.auto_repeat, &mut self.ecs, &self.net);
    }

    /// StopAttack.
    pub(crate) fn stop(&mut self, engaged: bool) {
        stop_attack_local(engaged, &mut self.queued_melee, &self.net);
    }

    /// StartAttack; a no-op before our own entity streams in.
    pub(crate) fn start(&mut self, target: u64, engaged: bool, stop_in_flight: bool) {
        let Ok(e) = self.me.single() else { return };
        start_attack_local(
            e,
            target,
            engaged,
            stop_in_flight,
            &mut self.auto_repeat,
            &mut self.sheath,
            &mut self.ecs,
            &self.net,
        );
    }
}

/// Mid-cast (`SMSG_SPELL_START` to GO or failure), inserted only for a nonzero cast time and
/// removed by the same spell id (the reference's reap is keyed by spell id).
#[derive(Component, Clone, Copy)]
pub(crate) struct Casting {
    pub(crate) spell_id: u32,
    #[allow(dead_code)]
    pub(crate) until: Option<Instant>,
}

/// The reference's animation play order: handlers run in packet order and the later play wins
/// bone 0. benilla's swing and cast pipelines are separate streams, so each animation-bearing
/// message carries a stamp (the wire in packet order, scene-time emitters when they fire), and the
/// driver resolves a same-frame collision by the highest.
#[derive(Resource, Default)]
pub(crate) struct PlaySeq(u64);

impl PlaySeq {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

/// A cast lifecycle edge on a streamed unit, off the wire.
#[derive(Message, Clone, Copy)]
pub(crate) struct CastEvent {
    pub(crate) entity: Entity,
    pub(crate) spell_id: u32,
    pub(crate) kind: CastEventKind,
    /// `PlaySeq` stamp, carried to the kit's `EmoteAnim`.
    pub(crate) seq: u64,
}

/// Which cast edge fired.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum CastEventKind {
    /// `SMSG_SPELL_START`: the precast kit persists from here.
    Start,
    /// `SMSG_SPELL_GO`: reaps the precast, plays the cast kit.
    Go,
    /// Died without release (`SMSG_SPELL_FAILED_OTHER`, or our failed `SMSG_CAST_RESULT`).
    Fail,
    /// The spell landed on this unit (a missile arrival): impact kit, then state kit.
    /// `weapon_visual` is the caster's ranged-weapon `SpellVisual`, carried from the GO since the
    /// caster may despawn.
    Impact { weapon_visual: Option<u32> },
    /// A projectile with no live target at arrival (a despawned target too): the kit plays on the
    /// caster (`entity`) at `pos`, `SpellVisual` field 13, stage 3. Sent here only for a
    /// dest-targeted GO's missile with no hit (`MissileSpawn::ground_aim`).
    GroundImpact { pos: Vec3 },
}

/// The anim held while a cast is in flight (`SpellVisualKit` field 2): full-body when standing
/// (`[unit+0xb4]`), a looping masked overlay when moving; reaped by `spell_id`.
#[derive(Component, Clone, Copy)]
pub(crate) struct CastHold {
    pub(crate) anim_id: u16,
    pub(crate) spell_id: u32,
    /// A ranged wind-up (`Attributes & 0x2`): the reference brackets it with ranged snaps, so the
    /// sheath reconcile's force-stow never survives the frame.
    pub(crate) ranged: bool,
}

/// A hand's fingers close (`HandsClosed`, 15) purely because a weapon occupies its attach point,
/// not on combat or sheath; a shield or empty hand stays open.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HandGrip {
    /// A weapon on the right-hand attach point (id 1).
    pub(crate) right: bool,
    /// A weapon on the left-hand attach point (id 2), not a shield.
    pub(crate) left: bool,
}

/// The grip overlay's weight over the unmasked base on the fingers, about 8:1 so they read closed.
pub(crate) const HAND_GRIP_WEIGHT: f32 = 8.0;

/// One melee swing (`SMSG_ATTACKERSTATEUPDATE`): one packet, one animation.
#[derive(Message, Clone, Copy)]
pub(crate) struct SwingMessage {
    pub(crate) attacker: Entity,
    /// The swing's target, when streamed.
    pub(crate) victim: Option<Entity>,
    pub(crate) hit_info: u32,
    /// vmangos `VictimState`: 1 hit, 2 dodge, 3 parry, 4 interrupt, 5 block, 6 evade, 7 immune,
    /// 8 deflect.
    pub(crate) victim_state: u32,
    pub(crate) damage: u32,
    /// `0x754fd0`'s display verdict: it gates only the floating number (`0x755a60`); animation,
    /// flinch, blood, sounds and timers run regardless.
    pub(crate) displayed: bool,
    /// `PlaySeq` stamp, in packet order.
    pub(crate) seq: u64,
}

/// A one-shot anim emote over the gait; its clip's WeaponFlags (`0x10`) stow a drawn weapon, and
/// the stow persists after it.
#[derive(Message, Clone, Copy)]
pub(crate) struct EmoteAnim {
    pub(crate) entity: Entity,
    pub(crate) anim_id: u16,
    /// `PlaySeq` stamp; a kit anim inherits its `CastEvent`'s.
    pub(crate) seq: u64,
    /// Came through the shared one-shot emote player (`SMSG_EMOTE`, the gesture dispatcher),
    /// which skips an id already armed on the key bone or bone 0. A kit anim and `DoEmote`'s local
    /// play call directly and take no such test.
    pub(crate) via_player: bool,
    /// The play seam's Special remap applies (`0x7385c0`): everything but a kit with flag `0x4` or
    /// `0x10`, which plays its Special1H/2H as authored (`0x73b198`).
    pub(crate) remap_special: bool,
}

/// A state kit's `AnimID`, compared and never played: both field-4 callers pass stage 2, which the
/// kit player diverts, and a mismatch with the armed id recomputes the base. So it only cuts:
/// Charge's Knockdown (121) falls to Stand, since the base selector never yields its Stun (14). An
/// in-place aura refresh emits nothing.
#[derive(Message, Clone, Copy)]
pub(crate) struct BaseAnimRecompute {
    pub(crate) entity: Entity,
    /// The right side of the reference's armed-id compare.
    pub(crate) anim_id: u16,
}

/// Rears a rider's mount: MountSpecial (94) on the mount child while the rider holds Mount (91);
/// dropped when there is no mount child.
#[derive(Message, Clone, Copy)]
pub(crate) struct MountFlourish {
    pub(crate) unit: Entity,
}

/// `MountFlourish` to an `EmoteAnim` on the unit's mount child.
fn flourish_to_anim(
    mut msgs: MessageReader<MountFlourish>,
    mut out: MessageWriter<EmoteAnim>,
    mut play_seq: ResMut<PlaySeq>,
    mounts: Query<&crate::entities::mount::MountChild>,
) {
    for m in msgs.read() {
        let Ok(child) = mounts.get(m.unit) else {
            debug!("flourish: {:?} has no mount child — dropped", m.unit);
            continue; // dismounted before the flourish landed
        };
        debug!(
            "flourish: MountSpecial(94) one-shot on {:?} (mount of {:?})",
            child.0, m.unit
        );
        out.write(EmoteAnim {
            entity: child.0,
            anim_id: select::MOUNT_SPECIAL,
            seq: play_seq.next(),
            via_player: false,
            remap_special: true,
        });
    }
}

/// A spell-side wound flinch, the reference's wound play from the kit player, the instant-hit loop
/// (harmful only) and the missile impact: CombatWound (9) or StandWound (8) by engagement, in the
/// secondary-blend slot.
#[derive(Message, Clone, Copy)]
pub(crate) struct WoundAnim {
    pub(crate) entity: Entity,
}

/// One `SMSG_SPELL_GO`'s target lists: impacts inline for `Spell.dbc` Speed 0, else missiles.
#[derive(Message, Clone)]
pub(crate) struct SpellGoTargets {
    pub(crate) caster: Entity,
    pub(crate) spell_id: u32,
    /// Units the spell landed on; each gets the impact hand-off.
    pub(crate) hits: Vec<Entity>,
    /// Missed units with `SpellMissInfo`: the arrival plays Dodge (30) or ShieldBlock (24), never
    /// Parry, and no impact kit; the deflect glance-off flight is not built.
    pub(crate) misses: Vec<(Entity, u8)>,
    /// A dest-targeted cast's ground point, in bevy coords; not read, the reference's
    /// dest-anchored launch visual is untraced.
    pub(crate) dest: Option<Vec3>,
    /// The GO's ammo block (`castFlags & 0x20`): the ammo's `ItemDisplayInfo` id, the missile model
    /// when the visual chain has none.
    pub(crate) ammo_display_id: Option<u32>,
    /// `PlaySeq` stamp, the GO's `CastEvent`'s.
    pub(crate) seq: u64,
    /// The GO's trajectory (`castFlags & 0x20000`).
    pub(crate) trajectory: Option<Trajectory>,
}

/// A trajectory missile's launch off its GO (`0x732ff0`): the pitch it leaves at and the time it
/// lands, with the caster and cast id `SMSG_SET_PROJECTILE_POSITION` names it by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Trajectory {
    pub(crate) pitch: f32,
    /// Seconds from the GO to the landing.
    pub(crate) delay: f32,
    pub(crate) caster: u64,
    pub(crate) cast_id: u8,
}

/// The sheath policy: requests, ceremony overlays, the `AnimationData.dbc` table.
mod sheath;
use sheath::{load_anim_data, SheathSwap};
pub(crate) use sheath::{
    toggle_sheath_next, AnimData, SheathRequest, SheathSwapMessage, VisualSheath,
};

/// The `SMSG_EMOTE` to `EmoteAnim` consumer.
mod emote_anim;

/// The per-unit emote queue the questgiver panels fill.
mod emote_queue;
/// The client-local gestures (chat, NPC interact), as the reference plays them.
mod gesture;
use emote_anim::emote_to_anim;
pub(crate) use emote_anim::{local_play_eligible, LocalPlay};
pub(crate) use emote_queue::QueueNpcEmotes;
pub(crate) use gesture::{select_gesture, Gesture, GestureQueue};

/// The Bevy systems that execute the state machine `select` picks.
mod driver;
pub(crate) use driver::oneshot_is_live;
use driver::{drive_animations, drive_hand_grip};

/// The model event-keyframe scanner.
mod events;
use events::fire_anim_events;
pub(crate) use events::{
    advance_track, footfall_culls, footfall_side, is_footstep_sound, scan_events, AnimSoundEvent,
    EventFrame, TrackMemory,
};

/// The `$BTH` breath puffs in a snow zone.
mod breath;
use breath::{classify_breath, fire_breath};

mod impact;
use impact::route_swing_impacts;
pub(crate) use impact::{DefenseAnim, PendingImpacts, SwingFlush, SwingImpact, SwingSlowdown};

/// The melee blood spurt.
mod blood;
mod env_damage;
pub(crate) mod net;
pub(crate) mod spell_visual;
use blood::{blood_spurts, load_blood_tables};
use env_damage::{hard_landing_dust, load_env_damage_table};
pub(crate) use env_damage::{EnvDamageTable, HardLanding, HARD_LANDING_DESCENT};
use spell_visual::{
    arm_aura_state_fx, arm_go_loot_fx, arm_level_up_fx, arm_loot_fx, arm_morph_latch,
    arm_mount_poof_fx, load_spell_visuals, play_delayed_area_kits, play_state_done_kits,
    replay_morph_kit, route_cast_visuals, track_violence_level, MorphLatch, StateDoneKit,
};
// `crate::aura_visual`'s tests drive the aura-slot watcher directly.
#[cfg(test)]
pub(crate) use spell_visual::arm_aura_state_fx as arm_aura_state_fx_for_test;
pub(crate) use spell_visual::{
    held_strike_sound, ChainProcPlay, FxClass, FxStage, KitPush, MissileSpawn, SpellKitFx,
    SpellKitShake, SpellKitSound, SpellVisuals,
};

/// The unit's own variation stream ([`benilla_assets::AnimRng::stream`]), cut on its first frame
/// from the session seed and the entity, so the driver's rolls do not depend on the unit order.
#[derive(Component, Default)]
pub(crate) struct UnitAnimRng(Option<benilla_assets::AnimRng>);

impl UnitAnimRng {
    /// The stream, cut from `session` for `entity` on first use.
    fn stream(
        &mut self,
        session: &benilla_assets::AnimRng,
        entity: Entity,
    ) -> &mut benilla_assets::AnimRng {
        self.0
            .get_or_insert_with(|| session.stream(entity.to_bits()))
    }
}

/// The per-unit animation state machine; `Transform` is required, its scale divides the rate.
#[derive(Component)]
#[require(Transform, UnitAnimRng)]
pub(crate) struct AnimDriver {
    mode: Mode,
    /// The targeted gait id, to cross-fade only on a change; `None` forces a fresh pick.
    gait: Option<u16>,
    /// The movement flags the base was last armed for: the reference has no per-one-shot latch,
    /// so a root landing with a cast one-shot (Ice Block) still lets the base overwrite bone 0.
    gait_flags: u32,
    /// Whether the unit was the interact NPC last frame: the interact-NPC setter re-picks its base
    /// on open and on clear, so a flip cuts a one-shot.
    interacting: bool,
    /// Whether the interaction in `interacting` (or the one just cleared) targets a player, whose
    /// open and clear skip the re-pick (the PLAYER typemask bit).
    interact_player: bool,
    /// The clear's re-pick runs while the reference's interact-NPC global still names the NPC, so
    /// it picks Stand; the global is zeroed after. The state returns at the next re-pick: the Stand
    /// window's end, a movement-flag edge, or leaving Gait.
    interact_hold: bool,
    /// The reference's `[unit+0xd58] & 0xc0000`: while Knockdown, LiftOff or Land holds it, base
    /// requests are refused, so a stun's knockdown survives the root's Stand recompute.
    base_lock: driver::play::BaseAnimLock,
    /// The committed sheath state, the reference's `[unit+0xd40]`: seeded from the descriptor,
    /// re-adopted when it changes, overwritten by requests and the reconcile.
    sheath_cur: Option<u8>,
    /// The last descriptor sheath byte, to detect changes.
    sheath_byte: Option<u8>,
    /// The pending mid-animation weapon swap while a draw/stow one-shot plays.
    sheath_swap: Option<SheathSwap>,
    /// Whether any animation started this frame, for the weapon-trail latch: the reference reads
    /// `unit+0xd1c` in the animation play call, its single entry point, locomotion included, so
    /// starting to run starts Charge's trail.
    started_anim: bool,
    /// A masked upper-body one-shot on the SpineLow overlay while the base drives the legs; a
    /// full-body one lives in `Mode::Swing` until locomotion transplants it here.
    overlay: Option<Overlay>,
    /// The key-bone cross-fade, the reference's per-bone secondary (λ 1 to 0): 150 ms to rest after
    /// a one-shot or a re-arm over the new clip's blendTime; a transplant (`blendFlag = 0`) seeds
    /// none.
    overlay_fade: Option<OverlayFade>,
    /// The wound flinch, the reference's secondary blend slot (λ 0.75 to 0 over the clip), apart
    /// from `overlay` so a masked swing and a wound coexist.
    wound: Option<Wound>,
    /// Whether this arc launched upward (a jump) or is a step-off fall, from vertical speed on
    /// its first airborne frame; the reference carries it as `MSG_MOVE_JUMP`.
    jump_arc: bool,
    /// Last frame's vertical speed: a jump out of a one-frame detachment never toggles FALLING,
    /// so a rise past `JUMP_ARC_MIN_UP` from below also classifies the arc.
    last_vertical_speed: f32,
    /// FALLING last frame: marks takeoff and a step-off fall's landing.
    was_falling: bool,
    /// The reference's `unit+0xd60`: a combat clip requested over another doubles the playing
    /// clip's rate and parks here until no one-shot is live; any normal arm clears it.
    deferred: Option<u16>,
    /// The looping arm's node and rolled budget `R` (`block+0xbc`): the watchdog re-picks after `R`
    /// passes. `None` for a one-shot or a freeze.
    loop_window: Option<(bevy::animation::graph::AnimationNodeIndex, u32)>,
    /// The rate last written to the base slot, recorded for the hover inspector.
    gait_rate: f32,
    /// The last mount display id: any change arms bone 0, like the reference's field watcher: Mount
    /// 91 or Stand 0, displacing a one-shot.
    mount_display: u32,
    /// The Special wanted last frame. Its edge is a play in the reference and clears the deferred
    /// cache; its level must not, so a fast-path park survives mid-air.
    last_special: Option<Special>,
    /// The airborne snapshot node `leave_special` stopped, named so the rate sync does not
    /// restart it (a standing locomotion clip also reads rate 0). A symptom fix for the swim hop's
    /// short kick, its mechanism open: the reference's blend source keeps running, so it stays
    /// scoped to the airborne cut.
    frozen: Option<bevy::animation::graph::AnimationNodeIndex>,
    /// The main node's clip index, so the per-frame rate sync skips the clip search; checked
    /// against the clip's own node before use, as each node belongs to one clip.
    rate_clip: Option<(bevy::animation::graph::AnimationNodeIndex, usize)>,
    /// The main node, the base id requested for it and the direction mode its fallback gave it
    /// (`0x826350`), so the per-frame rate write re-resolves only on a change.
    base_dir: Option<(bevy::animation::graph::AnimationNodeIndex, u16, u16)>,
    /// The special idle (`[unit+0xfa4]`): a stage-0 kit's anim, latched after its play
    /// (`0x73b38e`) until neither the key bone nor bone 0 plays it (`0x73bc81`). A model rebuild
    /// drops it with the driver, as the display reset does (`0x73e702`).
    special_idle: Option<u16>,
}

impl AnimDriver {
    /// The requested `(base, masked overlay)` ids, for the inspector.
    pub(crate) fn playing(&self) -> (Option<u16>, Option<u16>) {
        let base = match self.mode {
            Mode::Gait => self.gait,
            Mode::Entering(s) => Some(s.enter()),
            Mode::Looping(s) => Some(s.loop_id()),
            Mode::Exiting(_, id) => Some(id),
            Mode::Land { id, .. } => Some(id),
            Mode::Swing { id, .. } => Some(id),
        };
        (base, self.overlay.map(|o| o.id))
    }

    /// The base slot's playback rate, for the inspector.
    pub(crate) fn rate(&self) -> f32 {
        self.gait_rate
    }

    /// The latched special idle, which holds a caster's missiles (`0x73442f`).
    pub(crate) fn special_idle(&self) -> Option<u16> {
        self.special_idle
    }
}

/// A masked upper-body play: its node and requested id; `looping` marks a cast hold, released
/// only by the driver's hold logic.
#[derive(Clone, Copy)]
struct Overlay {
    node: bevy::animation::graph::AnimationNodeIndex,
    id: u16,
    looping: bool,
}

/// A key-bone cross-fade: `out` holds the outgoing pose, `None` over the base; `left`/`total` span
/// 150 ms or the incoming clip's blendTime.
#[derive(Clone, Copy)]
struct OverlayFade {
    out: Option<bevy::animation::graph::AnimationNodeIndex>,
    /// Seconds left; λ = `select::blend_lambda(left / total)`.
    left: f32,
    total: f32,
}

/// A wound-flinch decay: the reference seeds `λ₀ = 0.75`, `rate = 1/span` and self-releases.
#[derive(Clone, Copy)]
struct Wound {
    node: bevy::animation::graph::AnimationNodeIndex,
    /// The wound clip's length in seconds.
    span: f32,
    /// Masked to the upper body, where the base and a one-shot overlay also blend.
    masked: bool,
}

impl Default for AnimDriver {
    fn default() -> Self {
        Self {
            mode: Mode::Gait,
            gait: None,
            gait_flags: 0,
            interacting: false,
            interact_player: false,
            interact_hold: false,
            base_lock: driver::play::BaseAnimLock::default(),
            sheath_cur: None,
            sheath_byte: None,
            sheath_swap: None,
            started_anim: false,
            overlay: None,
            overlay_fade: None,
            wound: None,
            jump_arc: false,
            last_vertical_speed: 0.0,
            was_falling: false,
            deferred: None,
            loop_window: None,
            gait_rate: 1.0,
            mount_display: 0,
            last_special: None,
            frozen: None,
            rate_clip: None,
            base_dir: None,
            special_idle: None,
        }
    }
}

impl AnimDriver {
    /// The requested id playing now; the mouse pick's bounds follow it.
    pub(crate) fn active_anim(&self) -> Option<u16> {
        match self.mode {
            Mode::Gait => self.gait,
            Mode::Entering(sp) => Some(sp.enter()),
            Mode::Looping(sp) => Some(sp.loop_id()),
            Mode::Exiting(_, exit) => Some(exit),
            Mode::Land { id, .. } => Some(id),
            Mode::Swing { id, .. } => Some(id),
        }
    }

    /// `active_anim` through the model's fallback, the sequence actually playing (what the
    /// reconcile reads); the mode machine keeps comparing the requested id.
    pub(crate) fn resolved_anim(
        &self,
        anims: &ModelAnimations,
        catalog: Option<&AnimDataCatalog>,
    ) -> Option<u16> {
        self.active_anim().map(|id| resolved_id(anims, id, catalog))
    }

    /// The client-side sheath state (0 stowed, 1 melee, 2 ranged), `None` until driven.
    pub(crate) fn sheath_state(&self) -> Option<u8> {
        self.sheath_cur
    }

    /// Whether this unit started an animation this frame.
    pub(crate) fn started_anim(&self) -> bool {
        self.started_anim
    }

    /// Stands in for the driver's write in a consumer's test.
    #[cfg(test)]
    pub(crate) fn set_started_anim(&mut self, played: bool) {
        self.started_anim = played;
    }

    /// A draw/stow ceremony is in flight: the toggle's debounce (`ToggleSheath` guards 11, 12).
    pub(crate) fn sheath_ceremony_active(&self) -> bool {
        self.sheath_swap.is_some()
    }
}

/// Drives unit animation after `WorldStage::Net`, so it reads the current frame's movement.
pub(crate) struct CreatureAnimPlugin;

impl Plugin for CreatureAnimPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        twist::plugin(app);
        lod::plugin(app);
        breath::register(app);
        // The breath classifier runs at its own 10 s cadence, outside the chain.
        app.add_systems(
            Update,
            classify_breath.after(benilla_world::terrain_stream::AreaAuthoritySet),
        );
        app.add_message::<AnimSoundEvent>()
            .add_message::<SwingMessage>()
            .add_message::<SwingImpact>()
            .add_message::<SwingFlush>()
            .add_message::<DefenseAnim>()
            .add_message::<SwingSlowdown>()
            .init_resource::<PendingImpacts>()
            .init_resource::<PlaySeq>()
            .init_resource::<gesture::GestureQueue>()
            .add_message::<EmoteAnim>()
            .add_message::<QueueNpcEmotes>()
            .add_message::<BaseAnimRecompute>()
            .add_message::<MountFlourish>()
            .add_message::<WoundAnim>()
            .add_message::<SheathSwapMessage>()
            .add_message::<SheathRequest>()
            .add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<SpellKitSound>()
            .add_message::<SpellKitShake>()
            .add_message::<SpellKitFx>()
            .add_message::<StateDoneKit>()
            .add_message::<MissileSpawn>()
            .add_message::<spell_visual::DelayedAreaKit>()
            .add_message::<ChainProcPlay>()
            .add_message::<KitPush>()
            // The aura CharProc edges, drained by `crate::aura_visual`.
            .add_message::<crate::aura_visual::AuraProc>()
            .add_message::<HardLanding>()
            // The pending-morph latch, the reference's per-unit `[+0xd54]`.
            .init_resource::<MorphLatch>()
            .add_systems(
                Startup,
                (
                    load_anim_data,
                    load_spell_visuals,
                    load_blood_tables,
                    load_env_damage_table,
                )
                    .after(AssetSet::Open),
            )
            // Ahead of the frame's kit resolves; a `violenceLevel` change applies next frame.
            .add_systems(First, track_violence_level)
            .add_systems(
                Update,
                (
                    route_cast_visuals,
                    (
                        arm_loot_fx,
                        arm_go_loot_fx,
                        play_delayed_area_kits,
                        arm_level_up_fx,
                        arm_mount_poof_fx,
                    )
                        .chain(),
                    // The removal core reaps the state kit, then plays the state-done kit.
                    (arm_aura_state_fx, play_state_done_kits)
                        .chain()
                        .before(WorldStage::Present),
                    // The reference arms the latch and replays it on a display swap; arm first,
                    // since the swap crosses from last frame's teardown, armed by the same wire
                    // burst.
                    (arm_morph_latch, replay_morph_kit).chain(),
                    blood_spurts,
                    hard_landing_dust,
                    emote_to_anim,
                    gesture::drive_gestures,
                    emote_queue::set_emote_queues,
                    emote_queue::drive_emote_queues,
                    // Before the driver, so the flourish lands the frame it arrived.
                    flourish_to_anim,
                    drive_animations,
                    // Right after the driver, which rewrites `AnimDriver::started_anim` each pass.
                    crate::weapon_trail::fire_weapon_trails,
                    drive_hand_grip,
                    fire_anim_events,
                    route_swing_impacts,
                    // Before the entity visuals, so the arrow changes on the keyframe's frame.
                    drive_nock_latch,
                    // The ranged prop's clip off the same keys: the bow bends on `$BWP`, a gun's
                    // BowRelease (161) blast plays on `$BWR`.
                    crate::ranged_flex::flex_ranged_props
                        // The material samplers must see the clip armed this frame.
                        .before(benilla_world::model_render::ModelVisSet),
                    // The un-nock reset, off the same `$BWR`: after the arm, or its
                    // skip-while-releasing guard cancels a gun's blast on its first frame.
                    crate::ranged_flex::reset_ranged_props_on_unnock
                        .before(benilla_world::model_render::ModelVisSet),
                    fire_breath,
                )
                    .chain()
                    .after(WorldStage::Net)
                    // Same frame as the loot kneel: the reference force-plays Loot 50 in the
                    // handler that arms the latch.
                    .after(crate::ui_loot::resolve_loot_kneel)
                    // `DoEmote`'s local play, written in the chat drain.
                    .after(crate::ui_chat::ChatDrain)
                    // After Input so a sheath request (the Z toggle) executes the same frame.
                    .after(WorldStage::Input)
                    // `VisualSheath` must land before `resolve_equipment` reads it, or a sheath
                    // change double-swaps the weapon placement for a frame.
                    .before(crate::entities::EntityVisualsSet),
            )
            // The CharProc 11 freeze, after the driver so this frame's arms are in when it holds
            // the clocks: Ice Block's cast one-shot is armed, then never advances.
            .add_systems(
                Update,
                crate::aura_visual::apply_aura_anim_rate
                    .after(drive_animations)
                    .after(crate::aura_visual::drain_aura_procs),
            );
    }
}

/// The id this model plays for `id` (`0x826350`); identity until `AnimationData.dbc` loads.
fn resolved_id(anims: &ModelAnimations, id: u16, catalog: Option<&AnimDataCatalog>) -> u16 {
    resolved(anims, id, catalog).id
}

/// The id and direction mode this model plays for `id` (`0x826350`); forward identity until
/// `AnimationData.dbc` loads.
fn resolved(
    anims: &ModelAnimations,
    id: u16,
    catalog: Option<&AnimDataCatalog>,
) -> benilla_assets::ResolvedAnim {
    catalog.map_or(benilla_assets::ResolvedAnim { id, dir_flags: 0 }, |cat| {
        anims.resolve(id, cat)
    })
}

/// The clip for a requested id, through the `AnimationData.dbc` fallback.
fn find_resolved<'a>(
    anims: &'a ModelAnimations,
    id: u16,
    catalog: Option<&AnimDataCatalog>,
) -> Option<&'a AnimClip> {
    anims.find(resolved_id(anims, id, catalog))
}

#[cfg(test)]
mod attack_stand_tests {
    use super::*;

    /// StartAttack stands a seated player even on the already-engaged skip, which jumps past the
    /// stand to the auto-repeat cancel and sends no `CMSG_ATTACKSWING`.
    #[test]
    fn starting_an_attack_stands_you_even_when_the_swing_is_skipped() {
        let mut app = App::new();
        app.add_message::<SheathRequest>()
            .add_message::<crate::player::StandStateRequest>();
        // A dead-letter net channel: no packet is under test.
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.insert_resource(crate::spell::AutoRepeatActive(None));

        let me = app.world_mut().spawn_empty().id();
        app.add_systems(
            Update,
            move |mut commands: Commands,
                  mut auto_repeat: ResMut<crate::spell::AutoRepeatActive>,
                  mut sheath: MessageWriter<SheathRequest>,
                  net: Res<crate::net::NetCommands>| {
                start_attack_local(
                    me,
                    0xdead_beef,
                    true, // already engaged: the skip leg
                    false,
                    &mut auto_repeat,
                    &mut sheath,
                    &mut commands,
                    &net,
                );
            },
        );
        app.update();

        let stands: Vec<u8> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .map(|r| r.state)
            .collect();
        assert_eq!(
            stands,
            vec![0],
            "StartAttack asks for stand state 0 exactly once, even on the leg that sends no swing"
        );
    }
}

#[cfg(test)]
mod nock_latch_tests {
    use super::*;

    /// The `$BWP`/`$BWR` nock cycle.
    #[test]
    fn bwp_latches_and_bwr_clears_the_nock() {
        let mut app = App::new();
        app.add_message::<AnimSoundEvent>();
        app.add_systems(Update, drive_nock_latch);
        let unit = app.world_mut().spawn(NockedAmmo { display_id: 5996 }).id();
        app.world_mut().write_message(AnimSoundEvent {
            entity: unit,
            ident: *b"$BWP",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            app.world().entity(unit).contains::<NockLatch>(),
            "$BWP latches a unit with a cached ammo display"
        );
        app.world_mut().write_message(AnimSoundEvent {
            entity: unit,
            ident: *b"$BWR",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            !app.world().entity(unit).contains::<NockLatch>(),
            "$BWR clears — the arrow leaves with the shot"
        );
        let bare = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(AnimSoundEvent {
            entity: bare,
            ident: *b"$BWP",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            !app.world().entity(bare).contains::<NockLatch>(),
            "no ammo display, no latch"
        );
    }
}

#[cfg(test)]
mod disarm_tests {
    use super::*;

    /// The ladder hides one weapon, main hand first: a disarmed dual-wielder punches with the main
    /// hand and still swings its off-hand weapon.
    #[test]
    fn disarm_hides_one_weapon_main_hand_first() {
        let worn = Wielded {
            main: Some((2, 0x7)),   // 1H sword
            off: Some((2, 0xf)),    // dagger
            ranged: Some((2, 0x2)), // bow
            ..Default::default()
        };
        let disarmed = Wielded {
            disarmed: true,
            ..worn
        };

        assert_eq!(worn.disarmed_hand(), None);
        assert_eq!(select::swing_anim_main(worn.armed_main()), 17); // Attack1H
        assert_eq!(select::swing_anim_off(worn.armed_off()), 88); // AttackOffPierce
        assert_eq!(select::ready_anim(worn.armed_main()), 26); // Ready1H

        // The main hand's claim skips the off-hand probe.
        assert_eq!(disarmed.disarmed_hand(), Some(0));
        assert_eq!(disarmed.armed_main(), None);
        assert_eq!(disarmed.armed_off(), Some((2, 0xf)), "the off hand is KEPT");
        assert_eq!(select::swing_anim_main(disarmed.armed_main()), 16); // AttackUnarmed
        assert_eq!(select::swing_anim_off(disarmed.armed_off()), 88); // still the dagger
        assert_eq!(select::ready_anim(disarmed.armed_main()), 25); // ReadyUnarmed

        // The ranged slot has no gate.
        assert_eq!(disarmed.ranged, Some((2, 0x2)));
        assert_eq!(select::ranged_load_anim(disarmed.ranged), 105); // LoadBow

        assert_eq!(disarmed.main, worn.main);
    }

    /// Only a weaponless main hand passes the disarm to the off hand: AttackUnarmedOff (117,
    /// `AnimationData.dbc` fallback 87).
    #[test]
    fn an_empty_main_hand_hands_the_disarm_to_the_off_hand() {
        let off_only = Wielded {
            main: None,
            off: Some((2, 0xf)),
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(off_only.disarmed_hand(), Some(1));
        assert_eq!(off_only.armed_off(), None);
        assert_eq!(select::swing_anim_off(off_only.armed_off()), 117);

        let torch = Wielded {
            main: Some((15, 0)), // class 15 = miscellaneous
            off: Some((2, 0xf)),
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(torch.disarmed_hand(), Some(1));
        assert_eq!(
            torch.armed_main(),
            Some((15, 0)),
            "a non-weapon is not taken"
        );
        assert_eq!(torch.armed_off(), None);
    }

    /// Neither hand holds a weapon: the flag hides nothing.
    #[test]
    fn the_ladder_takes_weapons_and_leaves_the_rest() {
        let shielded = Wielded {
            main: None,
            off: Some((4, 6)), // class 4 = ARMOR, subclass 6 = shield
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(shielded.disarmed_hand(), None);
        assert_eq!(shielded.armed_off(), Some((4, 6)));
    }

    /// The pure ladder over classes, as the action bar's equipped-item test reads it.
    #[test]
    fn the_ladder_over_classes() {
        assert_eq!(disarmed_hand(Some(2), Some(2)), Some(0));
        assert_eq!(disarmed_hand(Some(2), None), Some(0));
        assert_eq!(disarmed_hand(None, Some(2)), Some(1));
        assert_eq!(disarmed_hand(Some(4), Some(2)), Some(1));
        assert_eq!(disarmed_hand(None, None), None);
        assert_eq!(disarmed_hand(Some(4), Some(4)), None);
    }
}
