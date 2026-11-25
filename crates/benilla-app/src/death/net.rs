//! The death packet handlers: each applies its packet to [`DeathNet`] and [`DeathGlobals`] and
//! fires its UI events through [`UiEvents`], as 12340's handlers do; plus the movement-mode
//! forward for our own mover and the corpse-guid latch's hooks on the object lifecycle, called
//! from the drain's object arms.

use crate::ui_chat::ChatSink as _;
use benilla_protocol::{EntityKind, MoveMode, ObjectFields, SessionEvent, SessionEventKind};
use benilla_ui::script::ScriptValue;
use bevy::prelude::*;

use super::{
    corpse_range_event, refresh_active_player, talk_to_spirit_healer, ActivePlayerInit,
    CorpsePoint, DeathGlobals, DeathNet, ReleasePoint, ResurrectOffer,
};
use crate::net::{
    GuidIndex, MoveModeMessage, NetCommands, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer,
    UnitMoveModes,
};
use crate::ui_script::live::UiEvents;

/// Register the death handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::CorpseQuery, on_corpse_query)
        .net_handler(K::DeathReleaseLoc, on_death_release_loc)
        .net_handler(K::CorpseMapPosition, on_corpse_map_position)
        .net_handler(K::CorpseNotInInstance, on_corpse_not_in_instance)
        .net_handler(K::CorpseReclaimDelay, on_corpse_reclaim_delay)
        .net_handler(K::ResurrectRequest, on_resurrect_request)
        .net_handler(K::PlayerName, on_resurrect_name)
        .net_handler(K::PlayerNameMiss, on_resurrect_name)
        .net_handler(K::SpiritHealerConfirm, on_spirit_healer_confirm)
        .net_handler(K::DurabilityDamageDeath, on_durability_damage_death)
        .net_handler(K::MoveMode, on_move_mode)
        .net_handler(K::PlayerSkinned, on_death_refresh)
        .net_handler(K::ResurrectFailed, on_death_refresh)
        .net_handler(K::ForcedDeathUpdate, on_death_refresh)
        .net_handler(K::Disconnected, on_session_end);
}

/// Whether our own player is a ghost now; `None` while it is not streamed.
fn self_ghost(self_q: &Query<&ObjectStore, With<SelfPlayer>>) -> Option<bool> {
    self_q.single().ok().map(|s| s.0.player_is_ghost())
}

/// `MSG_CORPSE_QUERY`'s answer (`0x526530`): a found corpse is kept only while we are a ghost; a
/// not-found drops it and, on the range latch, fires `CORPSE_OUT_OF_RANGE` outside an arena.
fn on_corpse_query(
    In(ev): In<SessionEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut death_net: ResMut<DeathNet>,
    mut ui: ResMut<UiEvents>,
    arena: crate::ui_arena::ArenaInstance,
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
        let unlatched = corpse_query(answer, self_ghost(&self_q) == Some(true), &mut death_net);
        if unlatched && !arena.active() {
            ui.fire("CORPSE_OUT_OF_RANGE", Vec::new());
        }
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

/// `SMSG_PLAYER_SKINNED` fires its flag (`0x6cdf00` -> `0x512536`); `SMSG_RESURRECT_FAILED`
/// refreshes the dead or alive signal (`0x6ce470`); `SMSG_FORCED_DEATH_UPDATE` derives the release
/// timer first (`0x6e3f13`, `0x6e3f18`).
fn on_death_refresh(
    In(ev): In<SessionEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut globals: ResMut<DeathGlobals>,
    mut ui: ResMut<UiEvents>,
    real_clock: Res<Time<Real>>,
) {
    let health = || self_q.single().ok().map(|s| s.0.unit_health().unwrap_or(0));
    match ev {
        SessionEvent::PlayerSkinned { flag } => {
            ui.fire("PLAYER_SKINNED", vec![ScriptValue::Int(i64::from(flag))]);
        }
        SessionEvent::ResurrectFailed => {
            if let Some(hp) = health() {
                refresh_active_player(hp, &mut ui);
            }
        }
        SessionEvent::ForcedDeathUpdate => {
            if let Ok(store) = self_q.single() {
                globals.update_repop_timer(&store.0, real_clock.elapsed_secs_f64());
            }
            if let Some(hp) = health() {
                refresh_active_player(hp, &mut ui);
            }
        }
        _ => {}
    }
}

/// `SMSG_CORPSE_NOT_IN_INSTANCE`: the message display's `ERR_CORPSE_IS_NOT_IN_INSTANCE`
/// (`0x526530`), in the handler's turn.
fn on_corpse_not_in_instance(
    In(ev): In<SessionEvent>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    if let SessionEvent::CorpseNotInInstance = ev {
        crate::ui_action::signals::SignalSink::line(
            &mut *ui,
            crate::ui_action::UiError::key("ERR_CORPSE_IS_NOT_IN_INSTANCE"),
        );
    }
}

/// `SMSG_CORPSE_RECLAIM_DELAY` (`0x513a80`): the deadline, and on the range latch the in-range
/// event again, so `RECOVER_CORPSE` re-shows with the new delay.
fn on_corpse_reclaim_delay(
    In(ev): In<SessionEvent>,
    death_net: Res<DeathNet>,
    mut globals: ResMut<DeathGlobals>,
    mut ui: ResMut<UiEvents>,
    real_clock: Res<Time<Real>>,
) {
    if let SessionEvent::CorpseReclaimDelay { delay_ms } = ev {
        globals.reclaim_at = Some(real_clock.elapsed_secs_f64() + f64::from(delay_ms) / 1000.0);
        if let Some(cp) = death_net.corpse.filter(|_| death_net.in_range) {
            ui.fire(corpse_range_event(cp), Vec::new());
        }
    }
}

/// `SMSG_RESURRECT_REQUEST` (`0x6dbd00`): the offer latches whatever our state; a named caster
/// asks at once while we are dead or a ghost, an unnamed one once the name cache answers
/// (`0x6cde20`, `0x6dbc60`).
fn on_resurrect_request(
    In(ev): In<SessionEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut globals: ResMut<DeathGlobals>,
    mut ui: ResMut<UiEvents>,
    names: Res<crate::names::NameCache>,
    net: Res<NetCommands>,
) {
    if let SessionEvent::ResurrectRequest {
        caster,
        name,
        sickness,
        has_timer,
    } = ev
    {
        let dead_or_ghost = self_q.single().ok().map(|s| s.0.is_dead_or_ghost());
        let unnamed = name.is_empty();
        // The cache by guid, asked once on a miss, whatever the packet names (`0x515950`).
        let offerer = names.resolve(caster, &net).map(str::to_owned);
        let ask = if unnamed {
            offerer.clone()
        } else {
            Some(name.clone())
        };
        globals.resurrect = Some(ResurrectOffer {
            caster,
            name,
            sickness,
            has_timer,
            awaiting_name: unnamed && ask.is_none(),
            offerer,
        });
        if let Some(name) = ask.filter(|n| !n.is_empty() && dead_or_ghost == Some(true)) {
            ui.fire("RESURRECT_REQUEST", vec![ScriptValue::Str(name)]);
        }
    }
}

/// The name answer an unnamed offer waits on (`0x6dbc60`): a name asks while we are dead or a
/// ghost; a miss clears the offer.
fn on_resurrect_name(
    In(ev): In<SessionEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut globals: ResMut<DeathGlobals>,
    mut ui: ResMut<UiEvents>,
) {
    use benilla_protocol::messages::NameQueryMiss;
    let (guid, name) = match ev {
        SessionEvent::PlayerName { guid, name, .. } => (guid, Some(name)),
        // A retry asks again and keeps waiting; the placeholder lands as `"?"`.
        SessionEvent::PlayerNameMiss { guid, miss } => match miss {
            NameQueryMiss::Retry => return,
            NameQueryMiss::Placeholder => (guid, Some("?".to_owned())),
            NameQueryMiss::Deny => (guid, None),
        },
        _ => return,
    };
    if let (Some(offer), Some(name)) = (
        globals.resurrect.as_mut().filter(|o| o.caster == guid),
        name.as_ref(),
    ) {
        offer.offerer = Some(name.clone());
    }
    let Some(offer) = globals
        .resurrect
        .as_mut()
        .filter(|o| o.awaiting_name && o.caster == guid)
    else {
        return;
    };
    offer.awaiting_name = false;
    let Some(name) = name else {
        globals.resurrect = None;
        return;
    };
    let dead_or_ghost = self_q.single().is_ok_and(|s| s.0.is_dead_or_ghost());
    if !name.is_empty() && dead_or_ghost {
        ui.fire("RESURRECT_REQUEST", vec![ScriptValue::Str(name)]);
    }
}

/// `SMSG_SPIRIT_HEALER_CONFIRM`: the healer's gossip asks through the talk (`0x6e2e90` ->
/// `0x6db010`).
fn on_spirit_healer_confirm(
    In(ev): In<SessionEvent>,
    self_q: Query<(&ObjectStore, &Transform), With<SelfPlayer>>,
    index: Res<GuidIndex>,
    units: Query<(&ObjectStore, &Transform)>,
    mut globals: ResMut<DeathGlobals>,
    mut ui: ResMut<UiEvents>,
    net: Res<NetCommands>,
) {
    if let SessionEvent::SpiritHealerConfirm { npc } = ev {
        talk_to_spirit_healer(
            npc,
            self_q.single().ok(),
            |guid| super::healer_reach(guid, &index, &units),
            &mut globals,
            &mut ui,
            &net,
        );
    }
}

fn on_durability_damage_death(
    In(ev): In<SessionEvent>,
    mut log: ResMut<crate::ui_script::live::UiEvents>,
) {
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
    mut arrivals: ResMut<crate::net::ForcedMoveArrivals>,
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
            MoveModeMessage {
                guid,
                counter,
                mode,
                apply,
                arrival: arrivals.next(),
            },
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
fn on_session_end(
    In(_): In<SessionEvent>,
    mut death_net: ResMut<DeathNet>,
    mut globals: ResMut<DeathGlobals>,
    mut init: ResMut<ActivePlayerInit>,
) {
    *death_net = DeathNet::default();
    *globals = DeathGlobals::default();
    init.0 = false;
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
/// not-found, asked or pushed at bones conversion, drops the marker and the range latch, and
/// says whether the latch was set.
fn corpse_query(answer: Option<CorpsePoint>, ghost: bool, death_net: &mut DeathNet) -> bool {
    match answer {
        Some(cp) if ghost => death_net.corpse = Some(cp),
        Some(_) => {}
        None => {
            death_net.corpse = None;
            return std::mem::take(&mut death_net.in_range);
        }
    }
    false
}

/// `SMSG_DEATH_RELEASE_LOC` (`0x6e2e90`): kept unless we are a ghost or not streamed, which store
/// map -1; map -1 is no point.
fn death_release_loc(map: i32, position: [f32; 3], ghost: Option<bool>, death_net: &mut DeathNet) {
    death_net.release = match (ghost, u32::try_from(map)) {
        (Some(false), Ok(map)) => Some(ReleasePoint { map, position }),
        _ => None,
    };
}

/// `SMSG_DURABILITY_DAMAGE_DEATH`, the 10% death durability loss: a `CHAT_MSG_COMBAT_MISC_INFO`
/// line with no arguments, not an error; the body is ignored.
fn durability_damage_death(log: &mut crate::ui_script::live::UiEvents) {
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
    m: MoveModeMessage,
    self_guid: &SelfGuid,
    death_net: &mut DeathNet,
    out: &mut MessageWriter<MoveModeMessage>,
) {
    if self_guid.0 == Some(m.guid) && m.mode == MoveMode::WaterWalk {
        death_net.water_walk = m.apply;
    }
    out.write(m);
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
    // The swim-to-fly bit is in the second word, which `UnitMoveModes` does not hold.
    if mode == MoveMode::SwimFlyTransition {
        if apply {
            commands.entity(e).insert(crate::net::SwimFlyTransition);
        } else {
            commands.entity(e).remove::<crate::net::SwimFlyTransition>();
        }
        return;
    }
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
        w.init_resource::<crate::net::ForcedMoveArrivals>();
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

        // The swim-to-fly bit is the second word's, kept beside the first.
        send(&mut w, EYE, MoveMode::SwimFlyTransition, true);
        assert!(w.entity(eye).contains::<crate::net::SwimFlyTransition>());
        assert_eq!(w.entity(eye).get::<UnitMoveModes>().unwrap().0, word);
        send(&mut w, EYE, MoveMode::SwimFlyTransition, false);
        assert!(!w.entity(eye).contains::<crate::net::SwimFlyTransition>());
        assert_eq!(w.resource::<Messages<MoveModeMessage>>().len(), 4);

        send(&mut w, 0xDEAD, MoveMode::Root, true);
        assert_eq!(
            w.resource::<Messages<MoveModeMessage>>().len(),
            4,
            "an unknown unit drops"
        );
        send(&mut w, ME, MoveMode::WaterWalk, true);
        assert!(w.resource::<DeathNet>().water_walk, "our own ghost cue");
    }

    /// A world with our player at `health` (a ghost with `ghost`) and every death resource.
    fn handler_world(health: u32, ghost: bool) -> World {
        use benilla_protocol::field::{
            FIELD_PLAYER_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH,
        };
        let mut w = World::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        w.insert_resource(NetCommands(tx));
        w.insert_resource(SelfGuid(Some(ME)));
        w.init_resource::<DeathNet>();
        w.init_resource::<DeathGlobals>();
        w.init_resource::<UiEvents>();
        w.init_resource::<crate::names::NameCache>();
        w.init_resource::<Time<Real>>();
        w.spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_UNIT_MAXHEALTH, 100),
                (FIELD_UNIT_HEALTH, health),
                (FIELD_PLAYER_FLAGS, if ghost { 0x10 } else { 0 }),
            ])),
        ));
        w
    }

    fn fired(w: &mut World) -> Vec<&'static str> {
        let names = w.resource::<UiEvents>().names();
        *w.resource_mut::<UiEvents>() = UiEvents::default();
        names
    }

    fn offer(name: &str) -> SessionEvent {
        SessionEvent::ResurrectRequest {
            caster: 0x77,
            name: name.into(),
            sickness: true,
            has_timer: false,
        }
    }

    /// `0x6dbd00`: the offer always latches; a named caster asks at once only while dead or a
    /// ghost (`0x6cde20`).
    #[test]
    fn a_resurrect_offer_latches_and_asks_only_the_dead() {
        let mut w = handler_world(100, false);
        w.run_system_once_with(on_resurrect_request, offer("Priest"))
            .unwrap();
        assert!(fired(&mut w).is_empty(), "alive: latched, not asked");
        assert!(
            w.resource::<DeathGlobals>()
                .resurrect
                .as_ref()
                .unwrap()
                .sickness
        );
        let mut w = handler_world(0, false);
        w.run_system_once_with(on_resurrect_request, offer("Priest"))
            .unwrap();
        assert_eq!(fired(&mut w), ["RESURRECT_REQUEST"]);
    }

    /// An unnamed caster asks once the name lands (`0x6dbc60`); a miss clears the offer.
    #[test]
    fn an_unnamed_offer_asks_when_its_name_lands() {
        let mut w = handler_world(0, false);
        w.run_system_once_with(on_resurrect_request, offer(""))
            .unwrap();
        assert!(fired(&mut w).is_empty(), "the name query is out");
        let name = SessionEvent::PlayerName {
            guid: 0x77,
            name: "Priest".into(),
            realm: String::new(),
            race: 1,
            gender: 0,
            class: 5,
            declensions: None,
        };
        w.run_system_once_with(on_resurrect_name, name.clone())
            .unwrap();
        assert_eq!(fired(&mut w), ["RESURRECT_REQUEST"]);
        w.run_system_once_with(on_resurrect_name, name).unwrap();
        assert!(fired(&mut w).is_empty(), "once per offer");

        w.run_system_once_with(on_resurrect_request, offer(""))
            .unwrap();
        w.run_system_once_with(
            on_resurrect_name,
            SessionEvent::PlayerNameMiss {
                guid: 0x77,
                miss: benilla_protocol::messages::NameQueryMiss::Deny,
            },
        )
        .unwrap();
        assert!(fired(&mut w).is_empty());
        assert!(w.resource::<DeathGlobals>().resurrect.is_none());
    }

    /// `0x513a80`: the delay re-fires the in-range event on the latch; `0x526530`: a not-found
    /// answer drops the latch with `CORPSE_OUT_OF_RANGE`.
    #[test]
    fn the_reclaim_delay_and_a_lost_corpse_fire_on_the_range_latch() {
        let mut w = handler_world(1, true);
        w.run_system_once_with(
            on_corpse_reclaim_delay,
            SessionEvent::CorpseReclaimDelay { delay_ms: 30_000 },
        )
        .unwrap();
        assert!(fired(&mut w).is_empty(), "no latch, no event");
        {
            let mut d = w.resource_mut::<DeathNet>();
            d.corpse = Some(point(0));
            d.in_range = true;
        }
        w.run_system_once_with(
            on_corpse_reclaim_delay,
            SessionEvent::CorpseReclaimDelay { delay_ms: 30_000 },
        )
        .unwrap();
        assert_eq!(fired(&mut w), ["CORPSE_IN_RANGE"]);
        let lost = SessionEvent::CorpseQuery {
            found: false,
            display_map: -1,
            position: [0.0; 3],
            corpse_map: 0,
            transport: 0,
        };
        w.run_system_once_with(on_corpse_query, lost).unwrap();
        assert_eq!(fired(&mut w), ["CORPSE_OUT_OF_RANGE"]);
        assert!(!w.resource::<DeathNet>().in_range);
    }

    /// `0x6cdf00` fires the skinned flag; `0x6ce470` and the forced update refresh the dead or
    /// alive signal (`0x520f70`).
    #[test]
    fn the_refresh_packets_fire_from_their_handlers() {
        let mut w = handler_world(0, false);
        w.run_system_once_with(on_death_refresh, SessionEvent::PlayerSkinned { flag: true })
            .unwrap();
        w.run_system_once_with(on_death_refresh, SessionEvent::ResurrectFailed)
            .unwrap();
        assert_eq!(
            fired(&mut w),
            ["PLAYER_SKINNED", "PLAYER_DEAD", "<clear cursor>"]
        );
        let mut w = handler_world(100, false);
        w.run_system_once_with(on_death_refresh, SessionEvent::ForcedDeathUpdate)
            .unwrap();
        assert_eq!(fired(&mut w), ["PLAYER_ALIVE"]);
    }
}
