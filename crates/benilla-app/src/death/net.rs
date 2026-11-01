//! The death packet handlers filling [`DeathNet`], the movement-mode forward for our own mover,
//! and the corpse-guid latch's hooks on the object lifecycle, called from the drain's object arms.

use benilla_protocol::{EntityKind, MoveMode, ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{CorpsePoint, DeathNet, ReleasePoint, ResurrectOffer};
use crate::net::{
    GuidIndex, MoveModeMessage, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer, UnitMoveModes,
};

/// Register the death handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::CorpseQuery, on_corpse_query)
        .net_handler(K::DeathReleaseLoc, on_death_release_loc)
        .net_handler(K::CorpseMapPosition, on_corpse_map_position)
        .net_handler(K::CorpseNotInInstance, on_corpse_not_in_instance)
        .net_handler(K::CorpseReclaimDelay, on_corpse_reclaim_delay)
        .net_handler(K::ResurrectRequest, on_resurrect_request)
        .net_handler(K::SpiritHealerConfirm, on_spirit_healer_confirm)
        .net_handler(K::DurabilityDamageDeath, on_durability_damage_death)
        .net_handler(K::MoveMode, on_move_mode)
        .net_handler(K::Disconnected, on_session_end);
}

/// Whether our own player is a ghost now; `None` while it is not streamed.
fn self_ghost(self_q: &Query<&ObjectStore, With<SelfPlayer>>) -> Option<bool> {
    self_q.single().ok().map(|s| s.0.player_is_ghost())
}

fn on_corpse_query(
    In(ev): In<SessionEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut death_net: ResMut<DeathNet>,
) {
    if let SessionEvent::CorpseQuery {
        found,
        display_map,
        position,
        corpse_map,
        transport,
    } = ev
    {
        let answer = found.then_some(CorpsePoint {
            display_map,
            position,
            local: position,
            corpse_map,
            transport,
        });
        corpse_query(answer, self_ghost(&self_q) == Some(true), &mut death_net);
    }
}

fn on_death_release_loc(
    In(ev): In<SessionEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut death_net: ResMut<DeathNet>,
) {
    if let SessionEvent::DeathReleaseLoc { map, position } = ev {
        death_release_loc(map, position, self_ghost(&self_q), &mut death_net);
    }
}

fn on_corpse_map_position(In(ev): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    if let SessionEvent::CorpseMapPosition { position, facing } = ev {
        death_net.corpse_transport_frame = Some((position, facing));
    }
}

/// `SMSG_CORPSE_NOT_IN_INSTANCE`: the message display's `ERR_CORPSE_IS_NOT_IN_INSTANCE`
/// (`0x526530`).
fn on_corpse_not_in_instance(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
) {
    if let SessionEvent::CorpseNotInInstance = ev {
        errors.0.push(crate::ui_action::UiError::key(
            "ERR_CORPSE_IS_NOT_IN_INSTANCE",
        ));
    }
}

fn on_corpse_reclaim_delay(
    In(ev): In<SessionEvent>,
    mut death_net: ResMut<DeathNet>,
    real_clock: Res<Time<Real>>,
) {
    if let SessionEvent::CorpseReclaimDelay { delay_ms } = ev {
        corpse_reclaim_delay(delay_ms, real_clock.elapsed_secs_f64(), &mut death_net);
    }
}

fn on_resurrect_request(In(ev): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    if let SessionEvent::ResurrectRequest {
        caster,
        name,
        sickness,
        has_timer,
    } = ev
    {
        resurrect_request(caster, name, sickness, has_timer, &mut death_net);
    }
}

fn on_spirit_healer_confirm(In(ev): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    if let SessionEvent::SpiritHealerConfirm { npc } = ev {
        spirit_healer_confirm(npc, &mut death_net);
    }
}

fn on_durability_damage_death(In(ev): In<SessionEvent>, mut log: ResMut<crate::ui_chat::ChatLog>) {
    if let SessionEvent::DurabilityDamageDeath = ev {
        durability_damage_death(&mut log);
    }
}

fn on_move_mode(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    mut modes: Query<&mut UnitMoveModes>,
    mut death_net: ResMut<DeathNet>,
    mut out: MessageWriter<MoveModeMessage>,
) {
    if let SessionEvent::MoveMode {
        guid,
        counter,
        mode,
        apply,
    } = ev
    {
        // An unknown unit drops, as `0x732450` does.
        if self_guid.0 != Some(guid) && !index.0.contains_key(&guid) {
            return;
        }
        move_mode(
            guid,
            counter,
            mode,
            apply,
            &self_guid,
            &mut death_net,
            &mut out,
        );
        unit_move_mode(
            guid,
            mode,
            apply,
            &mut commands,
            &self_guid,
            &index,
            &mut modes,
        );
    }
}

/// The death stores are session-scoped; a reconnect while dead re-sends the reclaim delay.
fn on_session_end(In(_): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    *death_net = DeathNet::default();
}

/// Our corpse streaming in: latch its guid for the reclaim send. The reference's latch has one
/// writer, gated on `CORPSE_FIELD_OWNER == me` and `CORPSE_FIELD_FLAGS` bit 0 clear, so bones never
/// arm it; `RetrieveCorpse` sends whatever is in the latch.
pub(crate) fn note_corpse(
    guid: u64,
    kind: EntityKind,
    fields: &ObjectFields,
    self_guid: &SelfGuid,
    death_net: &mut DeathNet,
) {
    if kind == EntityKind::Corpse
        && fields.corpse_owner() == self_guid.0
        && !fields.corpse_is_bones()
    {
        death_net.corpse_guid = Some(guid);
    }
}

/// The same latch, re-asked when `CORPSE_FIELD_FLAGS` changes under a live guid, as the reference's
/// `FLAGS` mirror handler does, so an in-place flip to bones drops it.
pub(crate) fn recheck_corpse(
    guid: u64,
    fields: &ObjectFields,
    self_guid: &SelfGuid,
    death_net: &mut DeathNet,
) {
    // A delta carries only what changed: an absent FLAGS field is not a clear bit.
    let Some(flags) = fields.corpse_flags_present() else {
        return;
    };
    let mine = fields.corpse_owner() == self_guid.0 || death_net.corpse_guid == Some(guid);
    if !mine {
        return;
    }
    if flags & 0x01 != 0 {
        if death_net.corpse_guid == Some(guid) {
            death_net.corpse_guid = None;
        }
    } else if fields.corpse_owner() == self_guid.0 {
        death_net.corpse_guid = Some(guid);
    }
}

/// The corpse-to-bones swap destroys the corpse object; a stale guid must not ride a reclaim.
pub(crate) fn forget_corpse(guid: u64, death_net: &mut DeathNet) {
    if death_net.corpse_guid == Some(guid) {
        death_net.corpse_guid = None;
    }
}

/// `MSG_CORPSE_QUERY`'s answer (`0x526530`): a found corpse is kept only while we are a ghost; a
/// not-found, asked or pushed at bones conversion, drops the marker.
fn corpse_query(answer: Option<CorpsePoint>, ghost: bool, death_net: &mut DeathNet) {
    match answer {
        Some(cp) if ghost => death_net.corpse = Some(cp),
        Some(_) => {}
        None => death_net.corpse = None,
    }
}

/// `SMSG_DEATH_RELEASE_LOC` (`0x6e2e90`): kept unless we are a ghost or not streamed, which store
/// map -1; map -1 is no point.
fn death_release_loc(map: i32, position: [f32; 3], ghost: Option<bool>, death_net: &mut DeathNet) {
    death_net.release = match (ghost, u32::try_from(map)) {
        (Some(false), Ok(map)) => Some(ReleasePoint { map, position }),
        _ => None,
    };
}

/// `SMSG_CORPSE_RECLAIM_DELAY`, anchored at arrival on the clock the feed reads. The 1.12 client
/// re-fires the corpse-range events on each, hence the generation bump.
fn corpse_reclaim_delay(delay_ms: u32, now_secs: f64, death_net: &mut DeathNet) {
    death_net.reclaim_at = Some(now_secs + f64::from(delay_ms) / 1000.0);
    death_net.reclaim_generation = death_net.reclaim_generation.wrapping_add(1);
}

/// `SMSG_RESURRECT_REQUEST`, the RESURRECT popup's data.
fn resurrect_request(
    caster: u64,
    name: String,
    sickness: bool,
    has_timer: bool,
    death_net: &mut DeathNet,
) {
    death_net.resurrect = Some(ResurrectOffer {
        caster,
        name,
        sickness,
        has_timer,
    });
    death_net.resurrect_generation = death_net.resurrect_generation.wrapping_add(1);
}

/// `SMSG_SPIRIT_HEALER_CONFIRM`: the healer's gossip re-sends it on every ask, and the reference
/// fires `CONFIRM_XP_LOSS` per arrival.
fn spirit_healer_confirm(npc: u64, death_net: &mut DeathNet) {
    death_net.ask_spirit_healer(npc);
}

/// `SMSG_DURABILITY_DAMAGE_DEATH`, the 10% death durability loss: a `CHAT_MSG_COMBAT_MISC_INFO`
/// line with no arguments, not an error; the body is ignored.
fn durability_damage_death(log: &mut crate::ui_chat::ChatLog) {
    use crate::ui_chat::combat;
    log.push_combat(combat::ready(
        crate::ui_chat::ChatEventKind::CombatMiscInfo,
        combat::DURABILITYDAMAGE_DEATH,
        combat::Fills::default(),
    ));
}

/// The acked movement modes (root, water-walk, feather-fall, hover, flight, gravity), forwarded to
/// the controller ([`crate::player::wire_in`]), which applies and acks those naming the unit it
/// moves: our own body or a possessed one (`0x732450` resolves any unit by guid). Our own
/// water-walk is also mirrored into [`DeathNet`] as a ghost cue.
fn move_mode(
    guid: u64,
    counter: u32,
    mode: MoveMode,
    apply: bool,
    self_guid: &SelfGuid,
    death_net: &mut DeathNet,
    out: &mut MessageWriter<MoveModeMessage>,
) {
    if self_guid.0 == Some(guid) && mode == MoveMode::WaterWalk {
        death_net.water_walk = apply;
    }
    out.write(MoveModeMessage {
        guid,
        counter,
        mode,
        apply,
    });
}

/// Another unit's acked mode is that unit's own movement state (`0x6eeff0`), kept through a
/// hand-over: it lands in its [`UnitMoveModes`], the word `monster_move`'s root gate and the
/// controller's take-over read.
fn unit_move_mode(
    guid: u64,
    mode: MoveMode,
    apply: bool,
    commands: &mut Commands,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    modes: &mut Query<&mut UnitMoveModes>,
) {
    if self_guid.0 == Some(guid) {
        return;
    }
    let Some(&e) = index.0.get(&guid) else {
        return;
    };
    let set = |word: &mut UnitMoveModes| {
        if apply {
            word.0 |= mode.flag();
        } else {
            word.0 &= !mode.flag();
        }
    };
    if let Ok(mut word) = modes.get_mut(e) {
        set(&mut word);
    } else {
        let mut word = UnitMoveModes::default();
        set(&mut word);
        commands.entity(e).insert(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 0x45;
    const EYE: u64 = 0xF130_006F_5F29_221A;

    fn world() -> (World, Entity) {
        let mut w = World::new();
        w.insert_resource(SelfGuid(Some(ME)));
        w.init_resource::<DeathNet>();
        w.init_resource::<Messages<MoveModeMessage>>();
        let eye = w.spawn(UnitMoveModes(0x1000)).id();
        let mut index = GuidIndex::default();
        index.0.insert(EYE, eye);
        w.insert_resource(index);
        (w, eye)
    }

    fn send(w: &mut World, guid: u64, mode: MoveMode, apply: bool) {
        w.run_system_once_with(
            on_move_mode,
            SessionEvent::MoveMode {
                guid,
                counter: 1,
                mode,
                apply,
            },
        )
        .unwrap();
    }

    fn point(transport: u32) -> CorpsePoint {
        CorpsePoint {
            display_map: 0,
            position: [1.0, 2.0, 3.0],
            local: [1.0, 2.0, 3.0],
            corpse_map: 0,
            transport,
        }
    }

    /// `0x526530`: a found corpse lands only while a ghost; a not-found always drops it.
    #[test]
    fn a_corpse_answer_is_kept_only_while_a_ghost() {
        let mut death_net = DeathNet::default();
        corpse_query(Some(point(0)), false, &mut death_net);
        assert!(death_net.corpse.is_none(), "alive: the answer is ignored");
        corpse_query(Some(point(7)), true, &mut death_net);
        assert_eq!(death_net.corpse, Some(point(7)));
        corpse_query(Some(point(0)), false, &mut death_net);
        assert_eq!(
            death_net.corpse,
            Some(point(7)),
            "an ignored answer keeps the last"
        );
        corpse_query(None, false, &mut death_net);
        assert!(death_net.corpse.is_none());
    }

    /// `0x6e2e90`: kept while dead and not a ghost; a ghost, no player or map -1 clears it.
    #[test]
    fn the_release_point_is_kept_unless_a_ghost_or_map_minus_one() {
        let mut death_net = DeathNet::default();
        death_release_loc(571, [5.0, 6.0, 7.0], Some(false), &mut death_net);
        assert_eq!(
            death_net.release,
            Some(ReleasePoint {
                map: 571,
                position: [5.0, 6.0, 7.0]
            })
        );
        death_release_loc(-1, [0.0; 3], Some(false), &mut death_net);
        assert!(death_net.release.is_none());
        death_release_loc(571, [5.0, 6.0, 7.0], Some(true), &mut death_net);
        assert!(death_net.release.is_none(), "a ghost stores map -1");
        death_release_loc(571, [5.0, 6.0, 7.0], None, &mut death_net);
        assert!(death_net.release.is_none(), "no player stores map -1");
    }

    /// `0x732450` resolves any unit: a possessed unit's grant reaches the controller and its own
    /// word, which the launch path's root gate reads.
    #[test]
    fn another_unit_s_grant_is_forwarded_and_lands_in_its_own_word() {
        let (mut w, eye) = world();
        send(&mut w, EYE, MoveMode::Root, false);
        send(&mut w, EYE, MoveMode::CanFly, true);
        let word = w.entity(eye).get::<UnitMoveModes>().unwrap().0;
        assert_eq!(word & 0x1000, 0, "unrooted, so its launch path is taken");
        assert_ne!(word & MoveMode::CanFly.flag(), 0);
        let forwarded = w.resource::<Messages<MoveModeMessage>>().len();
        assert_eq!(
            forwarded, 2,
            "the controller decides whether it drives the unit"
        );

        send(&mut w, 0xDEAD, MoveMode::Root, true);
        assert_eq!(
            w.resource::<Messages<MoveModeMessage>>().len(),
            2,
            "an unknown unit drops"
        );
        send(&mut w, ME, MoveMode::WaterWalk, true);
        assert!(w.resource::<DeathNet>().water_walk, "our own ghost cue");
    }
}
