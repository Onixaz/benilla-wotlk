//! The animation layer's packet handlers: each resolves a guid and writes one message to play on a
//! streamed unit. The swing record and the environmental-damage log also feed
//! [`crate::combat_log`], whose handlers are registered first and so run first.

use benilla_protocol::messages::{AttackSwingError, AttackerState, EnvironmentalDamageLog};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{
    Engaged, EnvDamageTable, KitPush, MountFlourish, PlaySeq, PlayerCombatEdges, RangedHold,
    SheathRequest, SwingFlush, SwingImpact, SwingMessage,
};
use crate::net::{AiReactionMessage, EmoteKind, EmoteMessage, GuidIndex, NetHandlerApp, SelfGuid};
use crate::swing_refusal::SwingRefusalEdge;
use crate::ui_action::{UiError, UiErrorKeys};

/// Register the layer's handlers, from [`super::CreatureAnimPlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::AttackStart, on_attack_start)
        .net_handler(K::AttackStop, on_attack_stop)
        .net_handler(K::AiReaction, on_ai_reaction)
        .net_handler(K::AttackSwingError, on_swing_refusal)
        .net_handler(K::CancelCombat, on_swing_refusal)
        .net_handler(K::FeignDeathResisted, on_feign_death_resisted)
        .net_handler(K::MountSpecial, on_mount_special)
        .net_handler(K::TextEmote, on_text_emote)
        .net_handler(K::Emote, on_emote)
        .net_handler(K::PlaySpellVisual, on_play_spell_visual)
        .net_handler(K::PlaySpellImpact, on_play_spell_visual)
        .net_handler(K::AttackerState, on_attacker_state)
        .net_handler(K::EnvironmentalDamageLog, on_environmental_damage_log)
        .net_handler(K::AchievementEarned, on_achievement_earned);
}

fn on_attack_start(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    mut edges: ResMut<PlayerCombatEdges>,
) {
    if let SessionEvent::AttackStart { attacker, victim } = ev {
        attack_start(
            attacker,
            victim,
            &mut commands,
            &index,
            &self_guid,
            &mut edges,
        );
    }
}

fn on_attack_stop(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    transforms: Query<&Transform>,
    mut edges: ResMut<PlayerCombatEdges>,
    mut flushes: MessageWriter<SwingFlush>,
) {
    if let SessionEvent::AttackStop {
        attacker,
        victim,
        face_victim,
    } = ev
    {
        attack_stop(
            AttackStop {
                attacker,
                victim,
                face_victim,
            },
            &mut commands,
            &index,
            &self_guid,
            &transforms,
            &mut edges,
            &mut flushes,
        );
    }
}

fn on_ai_reaction(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut reactions: MessageWriter<AiReactionMessage>,
) {
    if let SessionEvent::AiReaction { unit, reaction } = ev {
        ai_reaction(unit, reaction, &index, &mut reactions);
    }
}

fn on_swing_refusal(In(ev): In<SessionEvent>, mut edges: MessageWriter<SwingRefusalEdge>) {
    match ev {
        SessionEvent::AttackSwingError(e) => attack_swing_error(e, &mut edges),
        SessionEvent::CancelCombat => cancel_combat(&mut edges),
        _ => {}
    }
}

fn on_feign_death_resisted(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::FeignDeathResisted = ev {
        feign_death_resisted(&mut errors);
    }
}

fn on_mount_special(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    // Absent before a world streams, which reads as dry.
    world: Option<benilla_world::world_point::WorldPoint>,
    units: Query<(&GlobalTransform, Option<&crate::entities::CollisionHeight>)>,
    mut out: MessageWriter<MountFlourish>,
) {
    if let SessionEvent::MountSpecial { guid } = ev {
        let deep = |e: Entity| {
            let Ok((tf, height)) = units.get(e) else {
                return false;
            };
            let wow = benilla_assets::coords::bevy_to_wow(tf.translation());
            let surface = world
                .as_ref()
                .and_then(|w| w.liquid_at(benilla_world::world_point::Subject::Unit(e), wow))
                .map(|hit| hit.surface_z);
            let h = height.copied().unwrap_or_default().0;
            deep_in_liquid(surface, wow[2], h)
        };
        mount_special(guid, &self_guid, &index, deep, &mut out);
    }
}

fn on_text_emote(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut out: MessageWriter<EmoteMessage>,
    mut chat_log: ResMut<crate::ui_chat::ChatLog>,
) {
    if let SessionEvent::TextEmote {
        guid,
        text_emote,
        target_name,
    } = ev
    {
        self::text_emote(
            guid,
            text_emote,
            target_name,
            &index,
            &mut out,
            &mut chat_log,
        );
    }
}

fn on_emote(In(ev): In<SessionEvent>, index: Res<GuidIndex>, mut out: MessageWriter<EmoteMessage>) {
    if let SessionEvent::Emote { guid, emote_id } = ev {
        emote(guid, emote_id, &index, &mut out);
    }
}

fn on_attacker_state(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    mut swings: MessageWriter<SwingMessage>,
    mut impacts: MessageWriter<SwingImpact>,
    mut sheaths: MessageWriter<SheathRequest>,
    mut edges: MessageWriter<SwingRefusalEdge>,
    stores: Query<&mut crate::net::ObjectStore>,
    mut play_seq: ResMut<PlaySeq>,
) {
    if let SessionEvent::AttackerState(s) = ev {
        attacker_state(
            s,
            &index,
            &self_guid,
            &mut swings,
            &mut impacts,
            &mut sheaths,
            &mut edges,
            &stores,
            play_seq.next(),
        );
    }
}

fn on_environmental_damage_log(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    table: Option<Res<EnvDamageTable>>,
    mut play_seq: ResMut<PlaySeq>,
    mut out: MessageWriter<KitPush>,
) {
    if let SessionEvent::EnvironmentalDamageLog(e) = ev {
        environmental_damage_log(e, &index, table.as_deref(), &mut play_seq, &mut out);
    }
}

fn on_play_spell_visual(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut play_seq: ResMut<PlaySeq>,
    mut out: MessageWriter<KitPush>,
) {
    // `SMSG_PLAY_SPELL_IMPACT` plays at stage 1 (`0x8006c0`), the visual's at stage 0 (`0x800610`).
    // Both are the same discrete play; stage 0 adds flag `0x80`, which latches the kit's anim as the
    // unit's special idle (`0x73b38e`).
    match ev {
        SessionEvent::PlaySpellVisual { unit, kit_id } => {
            play_spell_visual(unit, kit_id, 0, &index, &mut play_seq, &mut out);
        }
        SessionEvent::PlaySpellImpact { unit, kit_id } => {
            play_spell_visual(unit, kit_id, 1, &index, &mut play_seq, &mut out);
        }
        _ => {}
    }
}

/// A unit began melee auto-attack (`SMSG_ATTACKSTART`, including our own echo). Ours fires
/// `PLAYER_ENTER_COMBAT` on every packet, a new victim's too (`0x756897`).
fn attack_start(
    attacker: u64,
    victim: u64,
    commands: &mut Commands,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    edges: &mut PlayerCombatEdges,
) {
    // The Ready idle rides this window: the client's gate is the auto-attack target being set.
    debug!("net: attack start {attacker:#x} → {victim:#x}");
    // An unstreamed attacker does nothing (`0x75686c`).
    let Some(&e) = index.0.get(&attacker) else {
        return;
    };
    // Melee start drops the `0x400` weapon-visual hold: a shooter closing to melee leaves the
    // drawn idle.
    commands
        .entity(e)
        .insert(Engaged(victim))
        .remove::<RangedHold>();
    if self_guid.0 == Some(attacker) {
        edges.0.push(true);
    }
}

/// One `SMSG_ATTACKSTOP` or `SMSG_COMBAT_EVENT_FAILED`.
struct AttackStop {
    attacker: u64,
    victim: u64,
    face_victim: bool,
}

/// A unit stopped melee auto-attack (`0x756770`): the swing ends, a set `face_victim` holds the
/// attacker's facing toward a streamed victim, and ours fires `PLAYER_LEAVE_COMBAT` on every packet
/// (`0x756b86`).
fn attack_stop(
    stop: AttackStop,
    commands: &mut Commands,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    transforms: &Query<&Transform>,
    edges: &mut PlayerCombatEdges,
    flushes: &mut MessageWriter<SwingFlush>,
) {
    let AttackStop {
        attacker,
        victim,
        face_victim,
    } = stop;
    debug!("net: attack stop {attacker:#x} → {victim:#x} face={face_victim}");
    let Some(&e) = index.0.get(&attacker) else {
        return;
    };
    commands.entity(e).remove::<Engaged>();
    // A pending swing flushes as text only; death and stun arrive as this packet.
    flushes.write(SwingFlush(e));
    // `0x7567a5`-`0x7567ec`: the bearing from the attacker to the victim.
    let held = (face_victim && victim != 0)
        .then(|| index.0.get(&victim))
        .flatten()
        .and_then(|&v| Some((transforms.get(e).ok()?, transforms.get(v).ok()?)))
        .and_then(|(a, v)| {
            crate::net::facing_bearing(
                benilla_assets::coords::bevy_to_wow(a.translation),
                benilla_assets::coords::bevy_to_wow(v.translation),
            )
        });
    if let Some(yaw) = held {
        commands.entity(e).insert(crate::net::FacingHold::new(yaw));
    }
    if self_guid.0 == Some(attacker) {
        edges.0.push(false);
    }
}

/// The server refused our swing (`SMSG_ATTACKSWING_*`); [`crate::swing_refusal`] owns the rest.
fn attack_swing_error(error: AttackSwingError, edges: &mut MessageWriter<SwingRefusalEdge>) {
    edges.write(SwingRefusalEdge::Refused(error));
}

/// `SMSG_CANCEL_COMBAT`: the server stopped our attack. The reference's handler is the refusal
/// family's fourth arm verbatim.
fn cancel_combat(edges: &mut MessageWriter<SwingRefusalEdge>) {
    edges.write(SwingRefusalEdge::CombatCancelled);
}

/// `SMSG_FEIGN_DEATH_RESISTED`: one red "Resisted" line and nothing else. The reference's handler
/// is a bare error display of 421, with no latch and no state. vmangos sends it together with
/// `SMSG_CANCEL_COMBAT` (`Unit.cpp:9469-9470`).
fn feign_death_resisted(errors: &mut UiErrorKeys) {
    debug!("net: feign death resisted");
    errors.0.push(UiError::key("ERR_FEIGN_DEATH_RESISTED"));
}

/// A creature flared aggro or a stealth pre-aggro alert (`SMSG_AI_REACTION`).
fn ai_reaction(
    unit: u64,
    reaction: u32,
    index: &GuidIndex,
    reactions: &mut MessageWriter<AiReactionMessage>,
) {
    // Audio only: 2 hostile and 0 alert flare; any other value does nothing.
    debug!("net: ai reaction {reaction} on {unit:#x}");
    if matches!(reaction, 0 | 2) {
        if let Some(&e) = index.0.get(&unit) {
            reactions.write(AiReactionMessage {
                unit: e,
                hostile: reaction == 2,
            });
        }
    }
}

/// One melee swing (`SMSG_ATTACKERSTATEUPDATE`): the attacker's swing plays now and the victim's
/// feedback waits for the clip's attack-hit key; the log entry and the centre text are the combat
/// log's (`0x7512b0`).
fn attacker_state(
    mut s: AttackerState,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    swings: &mut MessageWriter<SwingMessage>,
    impacts: &mut MessageWriter<SwingImpact>,
    sheaths: &mut MessageWriter<SheathRequest>,
    edges: &mut MessageWriter<SwingRefusalEdge>,
    stores: &Query<&mut crate::net::ObjectStore>,
    seq: u64,
) {
    let victim = index.0.get(&s.victim).copied();
    // Our landed swing clears the refusal latch when we are the attacker and the victim resolves;
    // sent from here to keep packet order.
    if self_guid.0 == Some(s.attacker) && victim.is_some() {
        edges.write(SwingRefusalEdge::Landed);
    }
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "fct",
            &format!(
                "recv swing atk={:#x} victim={:#x} dmg={} vstate={} hit={:#x}",
                s.attacker, s.victim, s.damage, s.victim_state, s.hit_info
            ),
        );
    }
    // The full-block synthesis: a resolved victim, no damage and a blocked amount read as state 5,
    // BLOCK, before any consumer; a partial block stays state 1.
    if victim.is_some() && s.damage == 0 && s.blocked != 0 {
        s.victim_state = 5;
    }
    let swing = SwingMessage {
        attacker: Entity::PLACEHOLDER, // filled per branch below
        victim,
        hit_info: s.hit_info,
        victim_state: s.victim_state,
        damage: s.damage,
        displayed: s.displayed(),
        seq,
    };
    if let Some(&e) = index.0.get(&s.attacker) {
        // A resolved attacker draws melee (sheath state 1) before any hit-info handling, even when
        // `HitInfo & 0x10000` suppresses the swing's animation; the per-animation reconcile never
        // forces melee over a drawn bow.
        sheaths.write(SheathRequest {
            entity: e,
            state: 1,
            ceremony: false,
        });
        swings.write(SwingMessage {
            attacker: e,
            ..swing
        });
    } else if swing.victim.is_some_and(|v| {
        // The unresolved-attacker leg calls the gated dispatcher, so the lootable gate applies here
        // too.
        !stores.get(v).is_ok_and(|s| s.0.unit_lootable())
    }) {
        // An attacker we cannot resolve cannot swing, so the victim's feedback fires now, in
        // full. The placeholder attacker resolves nowhere (blood defaults to the front).
        impacts.write(SwingImpact {
            swing,
            text_only: false,
            natural: None,
            pos: None,
        });
    }
}

/// `SMSG_TEXT_EMOTE`: the animation needs the performer streamed, the chat line only needs their
/// name, so an off-screen emote still prints.
fn text_emote(
    guid: u64,
    text_emote: u32,
    target_name: String,
    index: &GuidIndex,
    out: &mut MessageWriter<EmoteMessage>,
    chat_log: &mut crate::ui_chat::ChatLog,
) {
    out.write(EmoteMessage {
        guid,
        source: index.0.get(&guid).copied(),
        kind: EmoteKind::Text(text_emote),
    });
    chat_log.push_text_emote(guid, text_emote, target_name);
}

/// `SMSG_EMOTE`: an `Emotes.dbc` id to play on a unit (NPC scripts, a `/`-emote's animation).
fn emote(guid: u64, emote_id: u32, index: &GuidIndex, out: &mut MessageWriter<EmoteMessage>) {
    out.write(EmoteMessage {
        guid,
        source: index.0.get(&guid).copied(),
        kind: EmoteKind::Anim(emote_id),
    });
}

/// `SMSG_PLAY_SPELL_VISUAL` (stage 0: eating, drinking, mid-channel swaps) and
/// `SMSG_PLAY_SPELL_IMPACT` (stage 1: a trainer's learn effect): a kit play on the unit. Only a
/// streamed unit takes a [`PlaySeq`] stamp (unit typemask).
fn play_spell_visual(
    unit: u64,
    kit_id: u32,
    stage: u8,
    index: &GuidIndex,
    play_seq: &mut PlaySeq,
    out: &mut MessageWriter<KitPush>,
) {
    if let Some(&e) = index.0.get(&unit) {
        out.write(KitPush {
            entity: e,
            kit_id,
            seq: play_seq.next(),
            stage,
        });
    }
}

/// `SMSG_ENVIRONMENTALDAMAGELOG`'s visual: the damage type's kit plays on the victim like any kit
/// push. The packet plays no sound: a fall's pain grunt is the client's own landing predictor
/// (`creature_anim::env_damage`).
fn environmental_damage_log(
    e: EnvironmentalDamageLog,
    index: &GuidIndex,
    table: Option<&EnvDamageTable>,
    play_seq: &mut PlaySeq,
    out: &mut MessageWriter<KitPush>,
) {
    if let Some(&ent) = index.0.get(&e.victim) {
        if let Some(kit_id) = table.and_then(|t| t.0.kit_id(e.damage_type)) {
            debug!(
                "net: environmental damage on {:#x} (type {}, {} dmg) → kit {kit_id}",
                e.victim, e.damage_type, e.damage
            );
            // The damage handler plays its kit at stage 0 (`0x755365`).
            out.write(KitPush {
                entity: ent,
                kit_id,
                seq: play_seq.next(),
                stage: 0,
            });
        }
    }
}

/// `SMSG_MOUNTSPECIAL_ANIM`: a nearby rider rears their mount (MountSpecial, 94), unless the rider
/// is deep in liquid (`0x71cb67`). Our own guid is dropped, as we played it at send
/// (`0x71cb70`-`0x71cb81`): vmangos asks for no echo (`MovementHandler.cpp:972`) but delivers one
/// anyway unless its per-player broadcaster is on (`Object.cpp:2274`).
fn mount_special(
    guid: u64,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    deep: impl Fn(Entity) -> bool,
    out: &mut MessageWriter<MountFlourish>,
) {
    if self_guid.0 != Some(guid) {
        if let Some(&e) = index.0.get(&guid) {
            if !deep(e) {
                out.write(MountFlourish { unit: e });
            }
        }
    }
}

/// The deep-in-liquid test (`0x718a20`): the liquid surface over the unit's feet stands higher
/// than half its collision height (`0x718a65`-`0x718a6c`); no liquid is a depth of 0.
pub(crate) fn deep_in_liquid(surface_z: Option<f32>, feet_z: f32, collision_height: f32) -> bool {
    let depth = surface_z.map_or(0.0, |s| s - feet_z);
    collision_height * 0.5 < depth
}

/// `SMSG_ACHIEVEMENT_EARNED`'s earn effect on the earning player, any player in view (`0x5b3020`).
pub(super) fn on_achievement_earned(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    db: Option<Res<crate::ui_achievement::Achievements>>,
    visuals: Option<Res<super::spell_visual::SpellVisuals>>,
    mut fx: MessageWriter<super::spell_visual::SpellKitFx>,
) {
    let SessionEvent::AchievementEarned(e) = ev else {
        return;
    };
    let plays = super::spell_visual::earn_effect_plays(
        db.as_deref().map(|d| &*d.0),
        e.achievement,
        e.silent,
    );
    let (true, Some(&entity), Some(visuals)) = (plays, index.0.get(&e.player), visuals) else {
        return;
    };
    if let Some(begin) = super::spell_visual::achievement_earn_fx(&visuals, entity) {
        debug!(
            "anim: {:#x} earned {}, the earn effect plays ({entity})",
            e.player, e.achievement
        );
        fx.write(begin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// `0x718a20`: deep once the surface stands more than half the collision height over the feet.
    #[test]
    fn a_rider_is_deep_past_half_its_height_under_a_surface() {
        assert!(
            !deep_in_liquid(None, 10.0, 2.0),
            "no liquid is a depth of 0"
        );
        assert!(
            !deep_in_liquid(Some(11.0), 10.0, 2.0),
            "exactly half is not deep"
        );
        assert!(deep_in_liquid(Some(11.01), 10.0, 2.0));
        assert!(
            !deep_in_liquid(Some(9.0), 10.0, 2.0),
            "a surface below the feet"
        );
    }

    /// `0x71cb67`: a rider deep in liquid does not rear; one on land does; our own echo never.
    #[test]
    fn a_deep_rider_does_not_rear_its_mount() {
        const ME: u64 = 0x7;
        const RIDER: u64 = 0x8;
        let mut world = World::new();
        world.init_resource::<GuidIndex>();
        world.insert_resource(SelfGuid(Some(ME)));
        world.init_resource::<Messages<MountFlourish>>();
        let rider = world.spawn_empty().id();
        let me = world.spawn_empty().id();
        world.resource_mut::<GuidIndex>().0.insert(RIDER, rider);
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        for (guid, deep) in [(RIDER, true), (RIDER, false), (ME, false)] {
            world
                .run_system_once(
                    move |s: Res<SelfGuid>,
                          i: Res<GuidIndex>,
                          mut out: MessageWriter<MountFlourish>| {
                        mount_special(guid, &s, &i, |_| deep, &mut out);
                    },
                )
                .unwrap();
        }
        assert_eq!(world.resource::<Messages<MountFlourish>>().len(), 1);
    }

    /// `0x8006c0` and `0x800610` both play on a streamed unit only, and AzerothCore's trainer buy
    /// sends kit 362 on the buyer through the impact.
    #[test]
    fn the_impact_and_the_visual_both_push_their_kit_on_a_streamed_unit() {
        const BUYER: u64 = 0x42;
        let mut world = World::new();
        world.init_resource::<GuidIndex>();
        world.init_resource::<PlaySeq>();
        world.init_resource::<Messages<KitPush>>();
        let e = world.spawn_empty().id();
        world.resource_mut::<GuidIndex>().0.insert(BUYER, e);

        for ev in [
            SessionEvent::PlaySpellImpact {
                unit: BUYER,
                kit_id: 362,
            },
            SessionEvent::PlaySpellVisual {
                unit: BUYER,
                kit_id: 179,
            },
            SessionEvent::PlaySpellImpact {
                unit: 0x99,
                kit_id: 362,
            },
        ] {
            world
                .run_system_once_with(on_play_spell_visual, ev)
                .expect("the handler runs as a one-shot system");
        }
        let pushes = world.resource::<Messages<KitPush>>();
        let got: Vec<(Entity, u32, u8)> = pushes
            .iter_current_update_messages()
            .map(|p| (p.entity, p.kit_id, p.stage))
            .collect();
        assert_eq!(
            got,
            [(e, 362, 1), (e, 179, 0)],
            "the impact is stage 1, the visual stage 0, and an unstreamed unit plays nothing"
        );
    }

    /// `0x756800`: every start and stop of ours queues its combat event, a new victim's start
    /// too; a stop with its flag set holds the attacker's facing toward a streamed victim
    /// (`0x7567a5`); an unstreamed attacker does nothing.
    #[test]
    fn each_attack_start_and_stop_of_ours_queues_its_event_and_a_flagged_stop_holds_the_facing() {
        use benilla_assets::coords::wow_to_bevy;
        const ME: u64 = 10;
        let mut world = World::new();
        world.init_resource::<GuidIndex>();
        world.init_resource::<SelfGuid>();
        world.init_resource::<PlayerCombatEdges>();
        world.init_resource::<Messages<SwingFlush>>();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let me = world.spawn(Transform::default()).id();
        let mob = world.spawn(Transform::default()).id();
        let victim = world
            .spawn(Transform::from_translation(wow_to_bevy([0.0, 5.0, 0.0])))
            .id();
        for (g, e) in [(ME, me), (0x42, mob), (0x43, victim)] {
            world.resource_mut::<GuidIndex>().0.insert(g, e);
        }
        let start = |attacker, victim| SessionEvent::AttackStart { attacker, victim };
        let stop = |attacker, victim, face_victim| SessionEvent::AttackStop {
            attacker,
            victim,
            face_victim,
        };
        for ev in [
            start(ME, 0x42),
            start(ME, 0x43),
            stop(ME, 0x43, false),
            start(ME, 0x42),
            start(0x42, ME),
            stop(0x42, 0x43, true),
            stop(0x99, 0x43, true),
            stop(0x43, 0x99, true),
        ] {
            match ev {
                SessionEvent::AttackStart { .. } => world.run_system_once_with(on_attack_start, ev),
                _ => world.run_system_once_with(on_attack_stop, ev),
            }
            .expect("the handler runs as a one-shot system");
        }
        assert_eq!(
            world.resource::<PlayerCombatEdges>().0,
            [true, true, false, true],
            "ENTER per start, a new victim's too, LEAVE per stop; another unit's fire nothing"
        );
        assert!(world.entity(mob).contains::<crate::net::FacingHold>());
        assert!(
            !world.entity(victim).contains::<crate::net::FacingHold>(),
            "an unstreamed victim holds nothing"
        );
        assert!(!world.entity(me).contains::<crate::net::FacingHold>());
    }
}
