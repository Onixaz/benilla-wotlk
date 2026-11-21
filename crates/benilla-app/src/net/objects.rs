//! The bridge's object layer: the streamed world's creates, deltas, moves and destroys, movers'
//! speeds and modes, GameObject templates and anims, and items. Each packet is one handler over
//! [`Scene`] whose commands apply before the next packet's, so a create's store and speeds are
//! live components by the time the next packet reads them.
use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{
    guid, EntityKind, MonsterMoveFacing, MoveSpeeds, ObjectFields, SpeedKind, SplineMode,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use benilla_world::model_fade::DespawnFade;
use benilla_world::vis_chain::VisChainOnly;

use super::motion::{
    create_spline, gameobject_rotation, monster_move_spline, pose_transform, resolve_facing,
    trace_create_spline, trace_move_snap, wire_yaw, write_pose, SplineStopped, ROOT_APPLY_WIPE,
};
use super::parked::{self, ParkedObjects, ALLOC_EXPIRE, PARK_EXPIRE};
use super::{
    merge_store_fields, ActiveMover, FieldChanged, FreshSpawn, Guid, GuidIndex, NetCommands,
    NetEntity, NetHandlerApp, ObjectStore, RemoteMotion, SelfGuid, SpeedChangeMessage, Spline,
    UnitMoveModes, UnitSpeeds,
};
use benilla_protocol::{CreateSpline, MoverState, TransportPose};

/// `SMSG_GAMEOBJECT_CUSTOM_ANIM`, bridged to [`crate::go_anim`], which rejects `anim_id >= 4` and
/// maps the rest to AnimationData 153..156; the fishing bobber's splash is `anim_id` 0.
fn gameobject_custom_anim(
    guid: u64,
    anim_id: u32,
    plays: &mut MessageWriter<crate::go_anim::GoCustomAnim>,
) {
    debug!("net: gameobject {guid:#x} custom anim {anim_id}");
    plays.write(crate::go_anim::GoCustomAnim {
        go_guid: guid,
        anim_id,
    });
}

/// `SMSG_GAMEOBJECT_DESPAWN_ANIM`: AnimationData 157 on the one-shot channel. vmangos sends
/// `SMSG_DESTROY_OBJECT` in the same tick, so the mark is a component queued now, ahead of the
/// closure [`object_destroyed`] queues, not a message.
fn gameobject_despawn_anim(guid: u64, commands: &mut Commands, index: &GuidIndex) {
    debug!("net: gameobject {guid:#x} despawn anim");
    if let Some(&e) = index.0.get(&guid) {
        commands
            .entity(e)
            .insert(crate::go_anim::DespawnAnimAnnounced);
    }
}

/// Registers the object layer's handlers.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.init_resource::<crate::ui_honor::PvpCooldown>()
        .init_resource::<crate::ui_arena_team::ArenaTeams>()
        .init_resource::<crate::ui_arena_team::RosterThrottle>();
    app.net_handler(K::ObjectCreate, on_object_create)
        .net_handler(K::ItemCreate, on_item)
        .net_handler(K::ItemTemplate, on_item)
        .net_handler(K::ItemTime, on_item)
        .net_handler(K::ItemEnchantTime, on_item)
        .net_handler(K::ObjectMove, on_object_move)
        .net_handler(K::MoveTimeSkipped, on_move_time_skipped)
        .net_handler(K::UnitMove, on_unit_move)
        .net_handler(K::ObjectValues, on_object_values)
        .net_handler(K::ObjectDestroyed, on_object_destroyed)
        .net_handler(K::ObjectsRemoved, on_objects_removed)
        .net_handler(K::ObjectsNear, on_objects_near)
        .net_handler(K::MonsterMove, on_monster_move)
        .net_handler(K::GameObjectInfo, on_gameobject_info)
        .net_handler(K::GameObjectCustomAnim, on_gameobject_anim)
        .net_handler(K::GameObjectDespawnAnim, on_gameobject_anim)
        .net_handler(K::GameObjectResetState, on_gameobject_anim)
        .net_handler(K::SplineMoveMode, on_spline_move_mode)
        .net_handler(K::ForceSpeedChange, on_speed)
        .net_handler(K::SpeedChanged, on_speed)
        .net_handler(K::FlightSplineSync, on_flight_spline_sync)
        .net_handler(K::Disconnected, on_session_end);
}

/// The streamed world as one parameter: the components, caches, lifecycle hooks and edges the
/// object packets touch.
#[derive(SystemParam)]
pub(crate) struct Scene<'w, 's> {
    commands: Commands<'w, 's>,
    net: Res<'w, NetCommands>,
    index: ResMut<'w, GuidIndex>,
    parked: ResMut<'w, ParkedObjects>,
    self_guid: Res<'w, SelfGuid>,
    active_movers: Query<'w, 's, (), With<ActiveMover>>,
    real: Res<'w, Time<Real>>,
    transforms: Query<'w, 's, &'static mut Transform>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    countdowns: Query<'w, 's, &'static mut crate::items::Countdowns>,
    remote: Query<'w, 's, &'static mut RemoteMotion>,
    modes: Query<'w, 's, &'static mut UnitMoveModes>,
    riders: Query<'w, 's, &'static mut crate::transport::TransportRider>,
    /// The seated passengers, whose vehicle holds its mount through a dismount (`0x74046f`).
    seated: Query<'w, 's, &'static crate::vehicle::Passenger>,
    speeds: Query<'w, 's, &'static mut UnitSpeeds>,
    field_changes: MessageWriter<'w, FieldChanged>,
    speed_changes: MessageWriter<'w, SpeedChangeMessage>,
    self_moves: MessageWriter<'w, super::SelfMoveMessage>,
    self_creates: MessageWriter<'w, super::SelfCreateMover>,
    control: ResMut<'w, super::ControlFlag>,
    ui: ResMut<'w, crate::ui_script::live::UiEvents>,
    signals: NonSendMut<'w, crate::ui_script::live::UniqueSignals>,
    /// The current target, whose aura state the usable update watches (`0x716810`).
    selection: Option<Res<'w, crate::target::Selection>>,
    hard_landings: MessageWriter<'w, crate::creature_anim::HardLanding>,
    go_custom_anims: MessageWriter<'w, crate::go_anim::GoCustomAnim>,
    go_resets: MessageWriter<'w, crate::go_anim::GoResetState>,
    names: Res<'w, NameCache>,
    /// The pet list whose head the pet number's handler compares (`0x72d041`).
    pets: Option<Res<'w, crate::ui_pet::PetList>>,
    items: ResMut<'w, Items>,
    /// The item records' callbacks, in ask order across every subsystem.
    record_waits: NonSendMut<'w, crate::items::RecordWaits>,
    go_templates: ResMut<'w, GameObjectTemplates>,
    death_net: ResMut<'w, crate::death::DeathNet>,
    death_globals: ResMut<'w, crate::death::DeathGlobals>,
    player_init: ResMut<'w, crate::death::ActivePlayerInit>,
    arena: crate::ui_arena::ArenaInstance<'w>,
    arena_slots: ResMut<'w, crate::ui_arena::ArenaOpponents>,
    pvp: ResMut<'w, crate::ui_honor::PvpCooldown>,
    arena_teams: ResMut<'w, crate::ui_arena_team::ArenaTeams>,
    roster_throttle: ResMut<'w, crate::ui_arena_team::RosterThrottle>,
    group: ResMut<'w, crate::ui_party::GroupState>,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
    /// The templates the item watches test the logged quests' required items against.
    quest_log: Option<Res<'w, crate::ui_quest_log::QuestLog>>,
    /// The log's quest rows the stack watch walks.
    quest_rows: Option<NonSend<'w, crate::ui_quest_log::rows::RebuildMarks>>,
    /// `Item.dbc`'s classes the currency slot leg reads (`0x5d98f2`).
    icons: Option<Res<'w, crate::entities::ItemDisplays>>,
    /// The refund records the post-init asks (`0x70ae34`).
    refunds: Option<NonSendMut<'w, crate::ui_socket::RefundRecords>>,
    /// The possess spell our pet and far-sight watches look up (`0x6d1ab1`, `0x6e5034`).
    possess: (
        ResMut<'w, crate::ui_pet::possess::PossessSpell>,
        Option<Res<'w, crate::ui_action::Spells>>,
        Option<Res<'w, crate::spell::SummonSlots>>,
    ),
}

fn on_object_create(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id,
        position,
        orientation,
        scale,
        speeds,
        mover,
        transport_progress,
        transport,
        spline,
        fields,
        vehicle: _,
        attack_target,
        spawned,
    } = ev
    {
        sc.parked
            .expire_oldest(std::time::Instant::now(), PARK_EXPIRE);
        crate::death::net::note_corpse(guid, kind, &fields, &sc.self_guid, &mut sc.death_net);
        // A live object takes the create's fields alone: its movement block is skipped
        // (`0x4d6c4d`) and pass 2 runs only the field handlers (`0x4d64be`).
        if let Some(&e) = sc.index.0.get(&guid) {
            trace_create(guid, "live");
            let own = sc.self_guid.0 == Some(guid);
            let merged = merge_fields(
                &mut sc.commands,
                &mut sc.stores,
                &mut sc.field_changes,
                e,
                guid,
                fields,
                own,
                Some(&sc.seated),
            );
            pet_token_watch(&mut sc, guid, &merged);
            own_flags_watch(&mut sc, guid, merged);
            return;
        }
        // Our own player's init runs its guild change (`0x6e82c0` -> `0x6df7a0`) before the active player's init (`0x6e8378`).
        if sc.self_guid.0 == Some(guid) && kind == EntityKind::Player {
            sc.ui.fire("TABARD_CANSAVE_CHANGED", Vec::new());
        }
        // Our own create's init replays the known-spell buffer into the companion lists and updates them (`0x6e8378` -> `0x6e7f50`), ahead of the control below.
        if sc.self_guid.0 == Some(guid) {
            let me = (
                fields.unit_race().unwrap_or(0),
                fields.unit_class().unwrap_or(0),
                fields.unit_shapeshift_form(),
            );
            sc.ui.world(move |world| {
                if let Err(e) =
                    world.run_system_cached_with(crate::ui_companion::active_player_init, me)
                {
                    warn!("companions: the active player's init did not run: {e}");
                }
            });
        }
        // Our own create gives us control (`0x6e80b6`), firing `PLAYER_CONTROL_GAINED` on the flag's
        // edge (`0x71c9a0` -> `0x520fe0`) before the silent re-seed (`0x6e8160` -> `0x513880`).
        if sc.self_guid.0 == Some(guid) && !std::mem::replace(&mut sc.control.0, true) {
            sc.ui.fire("PLAYER_CONTROL_GAINED", Vec::new());
            // The control change's usable update (`0x521007`).
            sc.ui.world(crate::ui_action::usable_cache::step(
                crate::ui_action::usable_cache::Pass::Full,
            ));
        }
        // The UI's world entry runs inside our create (`0x6e8186`).
        if sc.self_guid.0 == Some(guid) {
            sc.ui
                .world(move |world| crate::net::enter_world_at_create(world, guid));
        }
        // Our own create's init refreshes the dead or alive signal after its world entry
        // (`0x6e818b`).
        if sc.self_guid.0 == Some(guid) {
            sc.player_init.0 = true;
            // The init's PvP cooldown stamp (`0x6e8378` -> `0x6e81f2`).
            sc.pvp
                .enter(fields.player_flags(), std::time::Instant::now());
        }
        if let Some(m) = mover.filter(|_| sc.self_guid.0 == Some(guid)) {
            sc.self_creates.write(super::SelfCreateMover {
                flags: m.flags,
                pitch: m.pitch,
            });
        }
        // A parked object revives with its fields replaced and, a unit, its movement and attack
        // target re-applied (`0x4d645d` → `0x73c260`); nothing else re-inits.
        let unit = matches!(kind, EntityKind::Unit | EntityKind::Player);
        let pose = unit.then(|| pose_transform(position, mover_placement(orientation, mover)));
        if sc.parked.contains(guid) {
            let Some(e) = revive(&mut sc, guid, Some(fields), pose) else {
                return;
            };
            trace_create(guid, &format!("revived target={attack_target:?}"));
            if unit {
                if let (Some(m), Some(s)) = (mover, speeds) {
                    apply_mover(
                        e,
                        guid,
                        kind,
                        position,
                        orientation,
                        m,
                        s,
                        transport,
                        spline,
                        &mut sc.commands,
                        &mut sc.transforms,
                    );
                }
                seed_attack_target(e, attack_target, &mut sc.commands);
            }
            return;
        }
        trace_create(
            guid,
            &format!(
                "fresh{} kind={kind:?} target={attack_target:?} transport={:?} flags={:?}",
                if spawned { " spawned" } else { "" },
                transport.map(|t| t.guid),
                mover.map(|m| format!("{:#x}", m.flags)),
            ),
        );
        sc.parked.expire_oldest_of(
            parked::park_type_of(kind),
            std::time::Instant::now(),
            ALLOC_EXPIRE,
        );
        let arena_side = (kind == EntityKind::Player && sc.self_guid.0 != Some(guid))
            .then(|| fields.player_arena_faction())
            .flatten();
        let own_pet = {
            let charm = fields.unit_charm().unwrap_or(0);
            if charm != 0 {
                charm
            } else {
                fields.unit_summon().unwrap_or(0)
            }
        };
        object_create(
            guid,
            kind,
            display_id,
            position,
            orientation,
            scale,
            speeds,
            mover,
            transport_progress,
            transport,
            spline,
            fields,
            &mut sc.commands,
            &mut sc.index,
            &sc.names,
            &sc.go_templates,
            &sc.net,
        );
        let Some(&e) = sc.index.0.get(&guid) else {
            return;
        };
        if matches!(kind, EntityKind::Unit | EntityKind::Player) {
            seed_attack_target(e, attack_target, &mut sc.commands);
        }
        if let Some(m) = mover.filter(|_| unit && sc.self_guid.0 != Some(guid)) {
            seed_unit_word(e, m, &mut sc.commands);
        }
        // Another player's init runs its guild change (`0x6e82c0` -> `0x6df7a0`).
        if kind == EntityKind::Player && sc.self_guid.0 != Some(guid) {
            sc.ui.fire("TABARD_CANSAVE_CHANGED", Vec::new());
        }
        // The bool reaches a creature's stand-state seed and an animated GameObject's Spawn.
        if spawned && matches!(kind, EntityKind::Unit | EntityKind::GameObject) {
            sc.commands.entity(e).insert(FreshSpawn);
        }
        // A player of the other arena side, as its init runs inside an arena (`0x6e8300`).
        if let Some(side) = arena_side {
            let (index, stores) = (&sc.index, &sc.stores);
            let pet_of = |g: u64| {
                if g == guid {
                    return Some(own_pet);
                }
                crate::ui_arena::pet_of_store(stores.get(*index.0.get(&g)?).ok()?)
            };
            sc.arena_slots
                .streamed_player(side, &sc.arena, guid, &pet_of, &mut sc.ui);
        }
    }
}

/// Revives a parked guid (`0x4d6ae0`): its entity back with the state it parked with, `delta`
/// merged into its fields with their edges, and `pose` in place of the parked one.
fn revive(
    sc: &mut Scene,
    guid: u64,
    delta: Option<ObjectFields>,
    pose: Option<Transform>,
) -> Option<Entity> {
    let parked = sc.parked.take(guid)?;
    let mut store = parked.store().clone();
    let e = parked::respawn(guid, parked, pose, &mut sc.commands, &mut sc.index);
    if let Some(delta) = delta {
        merge_store_fields(&mut store, delta, e, guid, |edge| {
            sc.field_changes.write(edge);
        });
    }
    sc.commands.entity(e).insert(ObjectStore(store));
    Some(e)
}

/// `UPDATEFLAG_HAS_TARGET`'s guid is the unit's attack target from init (`0x98e560`), the state
/// `SMSG_ATTACKSTART` sets; none clears it.
fn seed_attack_target(e: Entity, target: Option<u64>, commands: &mut Commands) {
    match target {
        Some(t) => {
            commands.entity(e).insert(crate::creature_anim::Engaged(t));
        }
        None => {
            commands.entity(e).remove::<crate::creature_anim::Engaged>();
        }
    }
}

/// A mover's rotation: its flags and pitch tilt a swimmer at once, else the plain yaw.
fn mover_placement(orientation: f32, mover: Option<MoverState>) -> Quat {
    mover.map_or_else(
        || wire_yaw(orientation),
        |m| crate::creature_anim::swim_body_rotation(orientation, m.flags, m.pitch),
    )
}

/// A block replaces a unit's kept word, a creature's as a player's (`0x6f1520` zeroes it, then
/// merges): a flyer streamed in keeps its `CAN_FLY` and gravity-off bits for the idle pick.
fn seed_unit_word(e: Entity, mover: MoverState, commands: &mut Commands) {
    let stated = crate::creature_anim::move_flags::merge_server_authored(0, mover.flags);
    commands
        .entity(e)
        .insert(UnitMoveModes(crate::net::motion::standing_bits(stated)));
}

/// A mover's whole state from a `LIVING` create or a `MOVEMENT` block (`0x6f1520`): its speeds,
/// the path it walks or none, its transport, and its pose; a relayed extrapolation restarts.
fn apply_mover(
    e: Entity,
    guid: u64,
    kind: EntityKind,
    position: [f32; 3],
    orientation: f32,
    mover: MoverState,
    speeds: MoveSpeeds,
    transport: Option<TransportPose>,
    spline: Option<CreateSpline>,
    commands: &mut Commands,
    transforms: &mut Query<&mut Transform>,
) {
    commands
        .entity(e)
        .remove::<RemoteMotion>()
        .insert(UnitSpeeds(speeds));
    seed_unit_word(e, mover, commands);
    match unit_rider(kind, transport) {
        Some(r) => {
            commands.entity(e).insert(r);
        }
        None => {
            commands
                .entity(e)
                .remove::<crate::transport::TransportRider>();
        }
    }
    trace_create_spline(guid, spline.as_ref());
    match spline.and_then(create_spline) {
        Some(s) => {
            commands.entity(e).insert(s);
        }
        None => {
            commands.entity(e).remove::<Spline>();
        }
    }
    write_pose(
        commands,
        transforms,
        e,
        position,
        mover_placement(orientation, Some(mover)),
    );
}

/// A unit's rider pose on its transport, `compose_riders`' input.
fn unit_rider(
    kind: EntityKind,
    transport: Option<TransportPose>,
) -> Option<crate::transport::TransportRider> {
    matches!(kind, EntityKind::Unit | EntityKind::Player)
        .then_some(transport)
        .flatten()
        .map(|t| crate::transport::TransportRider {
            transport_guid: t.guid,
            local_pos: [t.pos.x, t.pos.y, t.pos.z],
            local_orientation: t.orientation,
            seat: t.seat,
        })
}

/// The `ucr` trace tag: which path a create block took (fresh, revived, or a live object's
/// fields only).
fn trace_create(guid: u64, path: &str) {
    if benilla_assets::trace::enabled_for("ucr") {
        benilla_assets::trace::line("ucr", &format!("{guid:#x} {path}"));
    }
}

/// Items: a descriptor-only create, `SMSG_ITEM_QUERY_SINGLE_RESPONSE` (a miss is cached as `None`
/// and never re-asked), and the lifetime and enchant countdowns, written into the item's own
/// [`crate::items::Countdowns`].
fn on_item(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::ItemCreate {
            guid,
            container,
            fields,
        } => {
            sc.parked
                .expire_oldest(std::time::Instant::now(), PARK_EXPIRE);
            // A held item takes the fields alone; a new or re-enabled one runs its post-init (`0x70af80`, `0x70afb0`).
            let fresh = !sc.index.0.contains_key(&guid);
            let post_init = (fields.object_entry().unwrap_or(0), fields.item_owner());
            if fresh && sc.parked.contains(guid) {
                revive(&mut sc, guid, Some(fields), None);
                item_post_init(&mut sc, guid, post_init);
                return;
            }
            item_create(
                guid,
                container,
                fields,
                &mut sc.commands,
                &mut sc.index,
                &mut sc.stores,
                &mut sc.field_changes,
                &mut sc.items,
            );
            if fresh {
                item_post_init(&mut sc, guid, post_init);
            }
        }
        SessionEvent::ItemTemplate { entry, info } => {
            let info = info.map(|b| *b);
            debug!("net: item template {entry} → {info:?}");
            let landed = info.is_some();
            sc.items.insert_template(entry, info);
            // The record's callbacks, in ask order across every subsystem that asked (`0x67ca30`); an item's needs it held and the record found (`0x70aeb6`, `0x70aebc`; `0x7078e7`).
            for wait in sc.record_waits.take(entry) {
                record_callback(&mut sc, entry, landed, wait);
            }
        }
        // The reference's item-time handler has two arms that share the item lookup
        // (`TYPEMASK_ITEM`) and differ on a miss.
        SessionEvent::ItemTime { item_guid, seconds } => {
            // An unheld item drops the update, with no queue (`0x1EA`'s arm returns), so an item
            // created after the login re-send (`Player::SendItemDurations`) waits for the next.
            match sc.index.0.get(&item_guid).copied() {
                Some(e) if sc.countdowns.contains(e) => {
                    if let Ok(mut c) = sc.countdowns.get_mut(e) {
                        c.set_lifetime(seconds);
                    }
                    debug!("item duration: item {item_guid:#x} → {seconds}s");
                }
                _ => debug!("item duration: {item_guid:#x} names an item we do not hold — dropped"),
            }
        }
        SessionEvent::ItemEnchantTime {
            item_guid,
            slot,
            seconds,
        } => match sc.index.0.get(&item_guid).copied() {
            Some(e) if sc.countdowns.contains(e) => {
                if let Ok(mut c) = sc.countdowns.get_mut(e) {
                    if c.set_enchant(slot, seconds) {
                        debug!("enchant timer: item {item_guid:#x} slot {slot} → {seconds}s");
                    } else {
                        debug!("enchant timer: item {item_guid:#x} slot {slot} is past the seventh — refused");
                    }
                }
            }
            // `0x1EB`'s miss arm queues on a resolving player's list, and only the active player's
            // is replayed. vmangos names the owner, us (`Player.cpp:11883`, `:12077`), so it queues
            // on the active player.
            _ if sc.self_guid.0.is_some_and(|g| sc.index.0.contains_key(&g)) => {
                sc.items.queue_enchant_time(item_guid, slot, seconds)
            }
            _ => debug!("enchant timer: {item_guid:#x} unheld and no player to queue on — dropped"),
        },
        _ => {}
    }
}

/// One callback on `entry`'s record, answered or denied (`landed`).
fn record_callback(sc: &mut Scene, entry: u32, landed: bool, wait: crate::items::RecordWait) {
    use crate::items::RecordWait;
    match wait {
        RecordWait::PostInit(item) | RecordWait::Entry(item) => {
            let Some(fields) = sc.index.0.get(&item).and_then(|&e| sc.stores.get(e).ok()) else {
                return;
            };
            let owner = fields.0.item_owner();
            if sc.items.template_cached(entry).is_none() {
                return;
            }
            if wait == RecordWait::PostInit(item) {
                item_record_watch(sc, item, (entry, owner));
            } else {
                sc.ui.world(move |world| {
                    crate::ui_items::watch::entry_record(world, item);
                });
            }
        }
        // The quest log's queues `QUEST_LOG_UPDATE` on a found record (`0x5dea43`, `0x5dea49`).
        RecordWait::QuestLog => {
            if landed {
                sc.signals.unique_signal("QUEST_LOG_UPDATE");
            }
        }
        RecordWait::QuestReward(count) => {
            let info = sc.items.template_cached(entry);
            crate::ui_quest::lines::reward_landed(&mut sc.ui, entry, info, count);
        }
        RecordWait::GuildBank(callback) => sc
            .ui
            .world(move |world| crate::ui_guild_bank::record_callback(world, callback)),
        RecordWait::ItemText(item) => sc
            .ui
            .world(move |world| crate::ui_item_text::item_record_callback(world, item, landed)),
        RecordWait::Merchant => sc.ui.world(crate::ui_merchant::record_callback),
        RecordWait::DressUp(pane) => sc
            .ui
            .world(move |world| crate::ui_dressup::record_callback(world, entry, landed, pane)),
        RecordWait::Buyback => sc.ui.world(crate::ui_merchant::buyback_record_callback),
        // The trainer's callback fires on a found record (`0x593ca0`).
        RecordWait::Trainer => {
            if landed {
                sc.ui.fire("TRAINER_DESCRIPTION_UPDATE", Vec::new());
            }
        }
        // The petition vendor's callback fires on a found record (`0x5a1f30`).
        RecordWait::PetitionVendor => {
            if landed {
                sc.ui.fire("PETITION_VENDOR_UPDATE", Vec::new());
            }
        }
        RecordWait::Ui(wait) => sc
            .ui
            .write(move |script| script.item_record_callback(entry, landed, wait)),
        // A found record runs the bar's item update.
        RecordWait::ActionBar => {
            if landed {
                sc.ui
                    .world(move |world| crate::ui_action::slots::record_landed(world, entry));
            }
        }
        // An unknown item names "UNKNOWN".
        RecordWait::CastFailure(fail) => sc
            .ui
            .world(move |world| crate::ui_action::show_refusal(world, fail)),
    }
}

/// The disconnect: the asks in flight died with the socket, their callbacks with them.
fn on_session_end(In(_): In<SessionEvent>, mut waits: NonSendMut<crate::items::RecordWaits>) {
    waits.clear();
}

/// An `SMSG_UPDATE_OBJECT` movement block: ignored for the mover we control (`0x4d6de5`), else
/// applied whole to the unit (`0x4d6e5c`), a parked one revived first.
fn on_object_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectMove {
        guid,
        position,
        orientation,
        mover,
        speeds,
        transport,
        spline,
    } = ev
    {
        sc.parked
            .expire_oldest(std::time::Instant::now(), PARK_EXPIRE);
        let live = sc.index.0.get(&guid).copied();
        if sc.self_guid.0 == Some(guid) || live.is_some_and(|e| sc.active_movers.contains(e)) {
            return;
        }
        let pose = pose_transform(position, mover_placement(orientation, Some(mover)));
        let Some(e) = live.or_else(|| revive(&mut sc, guid, None, Some(pose))) else {
            return;
        };
        apply_mover(
            e,
            guid,
            EntityKind::Unit,
            position,
            orientation,
            mover,
            speeds,
            transport,
            spline,
            &mut sc.commands,
            &mut sc.transforms,
        );
    }
}

/// An observed mover skipped time: only its relay chain moves, as the reference's handler does
/// (`[mover+0xac] += lag`); an unknown guid is dropped, as there.
fn on_move_time_skipped(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::MoveTimeSkipped { guid, lag_ms } = ev {
        if let Some(mut m) = sc
            .index
            .0
            .get(&guid)
            .and_then(|&e| sc.remote.get_mut(e).ok())
        {
            m.relay.skip_time(lag_ms);
        }
    }
}

fn on_unit_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::UnitMove {
        guid,
        position,
        orientation,
        flags,
        pitch,
        time,
        verb,
        fall_time,
        jump,
        transport,
    } = ev
    {
        let now_ms = sc.real.elapsed_secs_f64() * 1000.0;
        // The active mover takes a speed or height relay's value alone (`0x7406da`): the status it
        // carries is never applied, so our pose stays ours.
        let active = sc.self_guid.0 == Some(guid)
            || sc
                .index
                .0
                .get(&guid)
                .is_some_and(|&e| sc.active_movers.contains(e));
        if verb == benilla_protocol::RelayVerb::Value && active {
            return;
        }
        if let Some(&e) = sc.index.0.get(&guid).filter(|_| !active) {
            relay_takes_word(e, &sc.remote, &mut sc.modes);
        }
        unit_move(
            guid,
            crate::net::motion::RelayMove {
                wire_ms: time,
                position,
                orientation,
                flags,
                pitch,
                fall_time,
                jump,
                transport,
                verb,
            },
            now_ms,
            &mut sc.commands,
            &sc.index,
            &sc.self_guid,
            &mut sc.remote,
            &mut sc.transforms,
            &mut sc.hard_landings,
            &mut sc.self_moves,
        );
    }
}

/// A relay seeding a unit's dead-reckoning merges over its kept word (`0x6eb730`).
fn relay_takes_word(
    e: Entity,
    remote: &Query<&mut RemoteMotion>,
    modes: &mut Query<&mut UnitMoveModes>,
) {
    if remote.contains(e) {
        return;
    }
    if let Ok(mut word) = modes.get_mut(e) {
        word.0 &= !crate::creature_anim::move_flags::SERVER_AUTHORED;
    }
}

/// A path keeps a relayed unit's standing word: its install (`0x6eb680`) writes the direction.
fn path_keeps_word(
    e: Entity,
    remote: &Query<&mut RemoteMotion>,
    modes: &mut Query<&mut UnitMoveModes>,
    commands: &mut Commands,
) {
    let Ok(rm) = remote.get(e) else {
        return;
    };
    let kept = crate::net::motion::standing_bits(rm.flags);
    match modes.get_mut(e) {
        Ok(mut word) => word.0 |= kept,
        Err(_) => {
            commands.entity(e).insert(UnitMoveModes(kept));
        }
    }
}

/// Our corpse's `CORPSE_FIELD_FLAGS` can flip to BONES under a live guid; the reclaim latch is
/// re-asked on that edge, as the reference's `FLAGS` handler does.
fn on_object_values(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectValues { guid, fields } = ev {
        crate::death::net::recheck_corpse(guid, &fields, &sc.self_guid, &mut sc.death_net);
        sc.parked
            .expire_oldest(std::time::Instant::now(), PARK_EXPIRE);
        match sc.index.0.get(&guid).copied() {
            Some(e) => {
                let own = sc.self_guid.0 == Some(guid);
                let merged = merge_fields(
                    &mut sc.commands,
                    &mut sc.stores,
                    &mut sc.field_changes,
                    e,
                    guid,
                    fields,
                    own,
                    Some(&sc.seated),
                );
                pet_token_watch(&mut sc, guid, &merged);
                own_flags_watch(&mut sc, guid, merged);
            }
            // A parked object revives with the delta (`0x4d6e80`); an unknown guid drops.
            None => {
                revive(&mut sc, guid, Some(fields), None);
            }
        }
    }
}

/// The party hook runs first: the reference's deactivate reads the descriptor about to go.
fn on_object_destroyed(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectDestroyed(guid) = ev {
        item_disable_watch(&mut sc, guid);
        sc.ui
            .world(move |world| crate::ui_item_text::item_disabled(world, guid));
        // The item's lock goes with its object.
        sc.ui.world(move |world| {
            if let Some(mut locks) =
                world.get_resource_mut::<crate::pending_item_ops::PendingItemOps>()
            {
                locks.forget(guid);
            }
        });
        crate::death::net::forget_corpse(guid, &mut sc.death_net);
        let held = crate::ui_party::net::held_member(&sc.index, &sc.stores, &sc.auras, guid);
        crate::ui_party::net::member_deactivated(guid, &mut sc.group, held, &sc.net);
        object_destroyed(guid, &mut sc.commands, &mut sc.index);
    }
}

/// OUT_OF_RANGE takes the same deactivate as DESTROY in the reference, so a member walking out of
/// range also gets the snapshot and `CMSG_REQUEST_PARTY_MEMBER_STATS`. Our own guid is skipped in
/// both of its loops (`0x4d7278`, `0x4d733b`).
fn on_objects_removed(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectsRemoved(mut guids) = ev {
        sc.parked
            .expire_oldest(std::time::Instant::now(), PARK_EXPIRE);
        guids.retain(|&g| sc.self_guid.0 != Some(g));
        for &guid in &guids {
            item_disable_watch(&mut sc, guid);
            sc.ui
                .world(move |world| crate::ui_item_text::item_disabled(world, guid));
            let held = crate::ui_party::net::held_member(&sc.index, &sc.stores, &sc.auras, guid);
            crate::ui_party::net::member_deactivated(guid, &mut sc.group, held, &sc.net);
        }
        objects_removed(guids, &mut sc.commands, &mut sc.index);
    }
}

/// `NEAR_OBJECTS`: each parked guid but ours revives as it parked (`0x4d6f40`).
fn on_objects_near(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::ObjectsNear(guids) = ev {
        sc.parked
            .expire_oldest(std::time::Instant::now(), PARK_EXPIRE);
        for guid in guids {
            if sc.self_guid.0 != Some(guid) && !sc.index.0.contains_key(&guid) {
                revive(&mut sc, guid, None, None);
            }
        }
    }
}

fn on_monster_move(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::MonsterMove {
        guid,
        transport,
        seat,
        start,
        spline_id,
        path,
        facing,
        stop,
        duration_ms,
        flying,
        run_mode,
        arc,
        facing_fixed,
        cycle,
    } = ev
    {
        let rooted = modes_of(guid, &sc.index, &sc.modes).rooted();
        // A rooted unit takes no path and keeps its relay state.
        if let Some(&e) = sc.index.0.get(&guid).filter(|_| !rooted) {
            path_keeps_word(e, &sc.remote, &mut sc.modes, &mut sc.commands);
        }
        monster_move(
            guid,
            transport,
            seat,
            start,
            spline_id,
            path,
            facing,
            stop,
            duration_ms,
            flying,
            run_mode,
            arc,
            facing_fixed,
            cycle,
            rooted,
            &mut sc.commands,
            &sc.index,
            &mut sc.transforms,
            &mut sc.riders,
        );
    }
}

fn on_gameobject_info(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::GameObjectInfo {
        entry,
        type_id,
        display_id,
        name,
        data,
    } = ev
    {
        gameobject_info(
            entry,
            type_id,
            display_id,
            name,
            &data,
            &mut sc.go_templates,
        );
        sc.ui
            .world(move |world| crate::ui_item_text::go_template_landed(world, entry));
    }
}

fn on_gameobject_anim(In(ev): In<SessionEvent>, mut sc: Scene) {
    match ev {
        SessionEvent::GameObjectCustomAnim { guid, anim_id } => {
            gameobject_custom_anim(guid, anim_id, &mut sc.go_custom_anims)
        }
        SessionEvent::GameObjectDespawnAnim { guid } => {
            gameobject_despawn_anim(guid, &mut sc.commands, &sc.index)
        }
        SessionEvent::GameObjectResetState { guid } => {
            debug!("net: gameobject {guid:#x} reset state");
            sc.go_resets
                .write(crate::go_anim::GoResetState { go_guid: guid });
        }
        _ => {}
    }
}

/// `SMSG_FLIGHT_SPLINE_SYNC` (`0x716940`): the modifier a cyclic spline adopts at its next wrap.
fn on_flight_spline_sync(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut splines: Query<&mut super::motion::Spline>,
) {
    let SessionEvent::FlightSplineSync { guid, progress } = ev else {
        return;
    };
    if let Some(mut spline) = index.0.get(&guid).and_then(|&e| splines.get_mut(e).ok()) {
        spline.sync_next_lap(progress, std::time::Instant::now());
    }
}

/// The observer movement-mode family: no ack.
fn on_spline_move_mode(In(ev): In<SessionEvent>, mut sc: Scene) {
    if let SessionEvent::SplineMoveMode { guid, mode, apply } = ev {
        spline_move_mode(
            guid,
            mode,
            apply,
            &mut sc.commands,
            &sc.index,
            &mut sc.modes,
            &mut sc.remote,
        );
    }
}

fn on_speed(
    In(ev): In<SessionEvent>,
    mut sc: Scene,
    mut arrivals: ResMut<super::ForcedMoveArrivals>,
) {
    match ev {
        SessionEvent::ForceSpeedChange {
            guid,
            kind,
            counter,
            speed,
        } => force_speed_change(
            guid,
            kind,
            counter,
            speed,
            &sc.index,
            &mut sc.speeds,
            &sc.self_guid,
            &mut sc.speed_changes,
            arrivals.next(),
        ),
        SessionEvent::SpeedChanged { guid, kind, speed } => {
            speed_changed(guid, kind, speed, &sc.index, &mut sc.speeds)
        }
        _ => {}
    }
}

/// A fresh `SMSG_UPDATE_OBJECT` create block: spawns the entity, warms the ask-once caches and
/// seeds its descriptor store.
fn object_create(
    guid: u64,
    kind: EntityKind,
    display_id: Option<u32>,
    position: [f32; 3],
    orientation: f32,
    scale: f32,
    speeds: Option<MoveSpeeds>,
    mover: Option<MoverState>,
    transport_progress: Option<u32>,
    transport: Option<TransportPose>,
    spline: Option<CreateSpline>,
    fields: ObjectFields,
    commands: &mut Commands,
    index: &mut GuidIndex,
    names: &NameCache,
    go_templates: &GameObjectTemplates,
    net_commands: &NetCommands,
) {
    let net = NetEntity {
        kind,
        display_id,
        scale,
    };
    // A transport's cycle anchor: the create block's `UPDATE_FLAG_TRANSPORT` u32 and when it
    // landed. The reference's per-frame tick runs types 15 (boats, a
    // path-progress clock) and 11 (lifts, `time-since-create % period`, `GameObject.cpp:246`).
    let go_type = (kind == EntityKind::GameObject).then(|| fields.gameobject_type_id());
    let transport_anchor = matches!(go_type, Some(11 | 15))
        .then_some(transport_progress)
        .flatten()
        .map(|progress_ms| crate::transport::TransportAnchor {
            progress_ms,
            at: std::time::Instant::now(),
        });
    // The placement, and on a type-11 lift the basis its keyframe offsets rotate through.
    let go_quat = (kind == EntityKind::GameObject)
        .then(|| fields.gameobject_rotation())
        .flatten();
    // A type-11 lift's seed: keyframes keyed by the entry, rotated by the quat, based at the
    // create position. One whose create carried no progress u32 has no clock and stays frozen.
    let elevator_seed = (matches!(go_type, Some(11)) && transport_anchor.is_some())
        .then(|| fields.object_entry())
        .flatten()
        .map(|entry| crate::transport::ElevatorSeed {
            entry,
            base_pos: position,
            yaw: orientation,
            quat: go_quat,
            parent: fields.gameobject_parent_rotation(),
        });
    // A GameObject is placed by its quaternion. A mover's create block also seeds its flags (merge
    // mask `0x75a07dff`) and pitch before the model is built, so a swimmer that streams in
    // nose-down is drawn nose-down from the first frame.
    let placement = match kind {
        EntityKind::GameObject => gameobject_rotation(go_quat, orientation),
        _ => mover_placement(orientation, mover),
    };
    // A unit created on a transport: the living block's rider tail is its local pose, which
    // `compose_riders` carries through the boat each frame; the world `position` is the fallback.
    let rider = unit_rider(kind, transport);
    // Any other object created on a transport rides it from its `POSITION` block's local pose; a
    // GameObject keeps its own rotation in the deck's frame (`0x714250`).
    let deck = transport
        .filter(|_| rider.is_none())
        .map(|t| crate::transport::DeckPassenger {
            transport_guid: t.guid,
            local_pos: [t.pos.x, t.pos.y, t.pos.z],
            local_rotation: match kind {
                EntityKind::GameObject => gameobject_rotation(go_quat, t.orientation),
                _ => wire_yaw(t.orientation),
            },
        });
    // Warm the name cache as a unit streams in, the reference's timing: its create path queries
    // every creature, whose name is the record's field 0. Its `CreatureCache.wdb` answers without
    // the wire; ours lasts a session.
    if matches!(kind, EntityKind::Unit | EntityKind::Player) {
        let _ = names.resolve(guid, net_commands);
        // A pet guid carries a pet number, so `resolve` asks the pet-name query; the template is
        // asked by the descriptor's entry as well, the reference's cache key (`[[unit+8]+0xc]`).
        if let Some(entry) = fields.object_entry().filter(|&e| e != 0) {
            let _ = names.resolve_creature(entry, guid, net_commands);
        }
    }
    // Warm the template cache as a GameObject streams in: only the query carries its lockId,
    // which decides use-vs-cast on a right-click.
    if matches!(kind, EntityKind::GameObject) {
        go_templates.request(guid, fields.object_entry(), net_commands);
        // Where and how big, under `RUST_LOG=benilla_app::net::objects=debug`.
        debug!(
            "gameobject spawn: entry {:?} display {display_id:?} type {go_type:?} \
             pos [{:.2}, {:.2}, {:.2}] scale {scale}",
            fields.object_entry(),
            position[0],
            position[1],
            position[2],
        );
    }
    // The walk this unit is already on, joined at the server's progress. Traced first, so a
    // `WOW_CREATE_SPLINE=off` run still records what the wire offered.
    trace_create_spline(guid, spline.as_ref());
    let walk = spline.and_then(create_spline);
    // A transport spawns hidden: its create pose is the spawn point, not where it is in its
    // cycle; the transport tick unhides it at the first sampled pose.
    let visibility = if transport_anchor.is_some() {
        Visibility::Hidden
    } else {
        Visibility::default()
    };
    let mut entity = commands.spawn((
        Guid(guid),
        net,
        pose_transform(position, placement),
        visibility,
    ));
    // Chain-only visibility: the root renders nothing itself, and culling flips its
    // `Visibility` through `Mut` writes, which do not re-add the sweep row.
    entity.vis_chain_only();
    crate::go_difficulty::tag(
        &mut entity,
        kind,
        fields.gameobject_type_id(),
        fields.gameobject_dynamic_flags(),
    );
    if let Some(s) = speeds {
        entity.insert(UnitSpeeds(s));
    }
    if let Some(anchor) = transport_anchor {
        entity.insert(anchor);
    }
    if let Some(seed) = elevator_seed {
        entity.insert(seed);
    }
    if let Some(r) = rider {
        entity.insert(r);
    }
    if let Some(d) = deck {
        entity.insert(d);
    }
    if let Some(s) = walk {
        entity.insert(s);
    }
    // The seed is not merged: the reference does not notify on create.
    entity.insert(ObjectStore(fields));
    index.0.insert(guid, entity.id());
}

/// A descriptor-only item or container create: an indexed entity with a store and no pose or
/// model; a re-create merges like a values delta.
fn item_create(
    guid: u64,
    container: bool,
    fields: ObjectFields,
    commands: &mut Commands,
    index: &mut GuidIndex,
    stores: &mut Query<&mut ObjectStore>,
    edges: &mut MessageWriter<FieldChanged>,
    items: &mut Items,
) {
    debug!("net: item create {guid:#x} (container: {container})");
    if let Some(&e) = index.0.get(&guid) {
        merge_fields(commands, stores, edges, e, guid, fields, false, None);
    } else {
        let e = crate::items::spawn_item(commands, index, guid, fields, container);
        // Replay the enchant times queued while it was not held, as of now.
        let queued = items.take_enchant_times(guid);
        if !queued.is_empty() {
            let mut countdowns = crate::items::Countdowns::default();
            for (slot, seconds) in queued {
                if !countdowns.set_enchant(slot, seconds) {
                    debug!("enchant timer: queued slot {slot} for {guid:#x} is past the seventh — refused");
                }
            }
            commands.entity(e).insert(countdowns);
        }
    }
}

/// A relayed `MSG_MOVE_*`. The reference schedules a remote's apply: the mover's replay chain gives
/// the packet a fire-time, a due move applies now and a later one queues for `drain_pending_moves`,
/// with dead-reckoning in between. `WOW_REMOTE_SNAP=1` applies at arrival.
fn unit_move(
    guid: u64,
    mv: crate::net::motion::RelayMove,
    now_ms: f64,
    commands: &mut Commands,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    remote_motion: &mut Query<&mut RemoteMotion>,
    transforms: &mut Query<&mut Transform>,
    landings: &mut MessageWriter<crate::creature_anim::HardLanding>,
    self_moves: &mut MessageWriter<crate::net::SelfMoveMessage>,
) {
    use crate::net::motion::{apply_move, arrival_snap, trace_relay, PendingMove, RelayOutcome};
    // Our own guid: the server writing our pose (`SetAsServerSide`, `ctime = 0`), which the
    // reference applies with no mover gate; it goes to `player::wire_in`.
    if self_guid.0 == Some(guid) {
        trace_relay(
            guid,
            &mv,
            &Default::default(),
            now_ms,
            0,
            RelayOutcome::SelfMover,
        );
        self_moves.write(crate::net::SelfMoveMessage {
            position: mv.position,
            orientation: mv.orientation,
            flags: mv.flags,
            pitch: mv.pitch,
            fall_time: mv.fall_time,
            jump: mv.jump,
            transport: mv.transport,
        });
        return;
    }
    let Some(&e) = index.0.get(&guid) else {
        // No entity: traced, since a mover running on while these pile up is a streaming bug.
        trace_relay(
            guid,
            &mv,
            &Default::default(),
            now_ms,
            0,
            RelayOutcome::Unknown,
        );
        return;
    };
    {
        // The server's pose supersedes a stale spline.
        commands.entity(e).remove::<Spline>();
        if let Ok(mut rm) = remote_motion.get_mut(e) {
            // The chain reads the state before this move applies, as the reference times the
            // packet off the live `[esi+0x40]`/`[esi+0x150]`.
            let (flags, queue_empty) = (rm.flags, rm.pending.is_empty());
            let fire_ms = rm.relay.schedule(mv.wire_ms, now_ms, flags, queue_empty);
            let at_arrival = arrival_snap() || rm.fires_at_arrival(fire_ms, now_ms);
            trace_relay(
                guid,
                &mv,
                &rm.relay,
                now_ms,
                rm.pending.len(),
                if at_arrival {
                    RelayOutcome::Now
                } else {
                    RelayOutcome::Queued
                },
            );
            if at_arrival {
                apply_move(e, &mv, &mut rm, now_ms, commands, landings);
            } else {
                // Fire-times are monotone per unit, so the queue stays ordered.
                rm.pending.push_back(PendingMove { fire_ms, mv });
            }
        } else {
            // The first move seeds the chain and applies at arrival; the insert is deferred, so
            // the extrapolator sees it next frame.
            let mut rm = RemoteMotion {
                wow_pos: mv.position,
                orientation: mv.orientation,
                flags: 0,
                pitch: 0.0,
                speed: 0.0,
                vertical_velocity: 0.0,
                jump_xy_vel: [0.0; 2],
                fall_start_z: None,
                pending: std::collections::VecDeque::new(),
                relay: Default::default(),
                last_apply_ms: now_ms,
                last_apply_pos: mv.position,
            };
            let fire_ms = rm.relay.schedule(mv.wire_ms, now_ms, 0, true);
            debug_assert_eq!(fire_ms, now_ms, "a seeding packet fires at arrival");
            trace_relay(guid, &mv, &rm.relay, now_ms, 0, RelayOutcome::Seed);
            apply_move(e, &mv, &mut rm, now_ms, commands, landings);
            write_pose(
                commands,
                transforms,
                e,
                mv.position,
                wire_yaw(mv.orientation),
            );
            commands.entity(e).insert(rm);
        }
    }
}

/// `SMSG_DESTROY_OBJECT`: corpse decay ahead of respawn, or a despawn.
fn object_destroyed(guid: u64, commands: &mut Commands, index: &mut GuidIndex) {
    // The object is freed now and its model fades out, as the reference's object deactivation (on
    // DESTROY and OUT_OF_RANGE alike) hands the model to its model fade-out scheduler. The guid
    // leaves the index at once ([`tear_down`]).
    //
    // A pinned object only gets its pending-destroy bit and is freed when the last pin drops. Our
    // one pin is an announced `SMSG_GAMEOBJECT_DESPAWN_ANIM`, whose end runs the deferred teardown
    // ([`crate::go_anim::release_despawn_pin`]).
    if let Some(e) = index.0.remove(&guid) {
        // An item has no model to fade, so it goes now with its countdown cells.
        if guid::is_item(guid) {
            commands.entity(e).despawn();
            return;
        }
        commands.queue(move |world: &mut bevy::ecs::world::World| {
            if world
                .get::<crate::go_anim::DespawnAnimAnnounced>(e)
                .is_some()
            {
                if let Ok(mut ent) = world.get_entity_mut(e) {
                    ent.insert(crate::go_anim::PendingDestroy);
                }
            } else if let Ok(ent) = world.get_entity_mut(e) {
                tear_down(ent);
            }
        });
    }
}

/// The `OutOfRange` block: each object is disabled, its model faded out as DESTROY's is
/// ([`tear_down`]), and parked for a revive (`0x4d7305`, `0x4d737f`). A transport is torn down
/// unparked: its cycle clock lives on components a revive does not rebuild.
fn objects_removed(guids: Vec<u64>, commands: &mut Commands, index: &mut GuidIndex) {
    let now = std::time::Instant::now();
    for g in guids {
        if let Some(e) = index.0.remove(&g) {
            commands.queue(move |world: &mut bevy::ecs::world::World| {
                if world.get::<crate::transport::Transport>(e).is_none() {
                    parked::park_entity(world, g, e, now);
                }
                if let Ok(ent) = world.get_entity_mut(e) {
                    tear_down(ent);
                }
            });
        }
    }
}

/// An object's teardown, as in the reference: the object is freed and its detached model fades out
/// ([`DespawnFade`]); its deactivation clears the selection and sends `CMSG_SET_SELECTION 0` at
/// once.
///
/// The model stays on the same entity, so removing its [`Guid`] is what ends the object for every
/// consumer; the [`NetEntity`] and [`ObjectStore`] stay so the model draws out (a decaying corpse
/// stays lying down). The caller has already removed the guid from the [`GuidIndex`], so a
/// same-tick re-create spawns a fresh object beside the fading model.
pub(crate) fn tear_down(mut ent: EntityWorldMut) {
    // The disable destroys the totem slot the unit holds (`0x73574a`).
    if let Some(&Guid(guid)) = ent.get::<Guid>() {
        ent.world_scope(|world| crate::ui_totem::unit_disabled(world, guid));
    }
    ent.remove::<Guid>();
    // The movement disable (`0x6ec390` at `0x735198`) deletes the spline and frees the move queue.
    ent.remove::<(Spline, super::motion::WalkEndFacing, RemoteMotion)>();
    ent.insert(DespawnFade::default());
    // The disable releases the effect list, so nothing the object wore fades out with it
    // (`0x7355a7`).
    let entity = ent.id();
    ent.world_scope(|world| {
        use crate::creature_anim::{EffectListClear, UnitEffectClear};
        if world.contains_resource::<bevy::ecs::message::Messages<UnitEffectClear>>() {
            world.write_message(UnitEffectClear {
                entity,
                mode: EffectListClear::Release,
            });
        }
    });
}

/// `SMSG_MONSTER_MOVE` or `SMSG_MONSTER_MOVE_TRANSPORT`: settles transport membership, snaps the
/// facing, then attaches or clears the spline. With a `transport`, `start`, the path and an
/// `Angle` facing are deck-local and the unit rides as a [`TransportRider`]; without one, a riding
/// unit leaves the deck, as vmangos drops it (`MoveSplineInit.cpp:156-159`).
fn monster_move(
    guid: u64,
    transport: Option<u64>,
    seat: i8,
    start: [f32; 3],
    spline_id: u32,
    path: Vec<[f32; 3]>,
    facing: MonsterMoveFacing,
    stop: bool,
    duration_ms: u32,
    flying: bool,
    run_mode: bool,
    arc: Option<benilla_protocol::SplineArc>,
    facing_fixed: bool,
    cycle: Option<benilla_protocol::SplineCycle>,
    rooted: bool,
    commands: &mut Commands,
    index: &GuidIndex,
    transforms: &mut Query<&mut Transform>,
    riders: &mut Query<&mut crate::transport::TransportRider>,
) {
    if let Some(&e) = index.0.get(&guid) {
        // The desync readout: the gap between where we draw the unit and where its path begins.
        trace_move_snap(
            guid,
            // Both sides in one frame: a deck packet's `start` is compared to the local pose.
            match transport {
                Some(_) => riders.get(e).ok().map(|r| r.local_pos),
                None => transforms.get(e).ok().map(|t| bevy_to_wow(t.translation)),
            },
            start,
            stop,
            duration_ms,
            arc,
            facing_fixed,
        );
        // Transport membership, settled before any pose is read, and above the root gate, which
        // refuses only the path. An `Angle` facing is deck-local (`MoveSplineInit.cpp:87`); a
        // `Spot` or `Target` is not converted server-side, so it is rebased by the deck's yaw.
        let deck_yaw = |t: u64| {
            index
                .0
                .get(&t)
                .and_then(|&te| transforms.get(te).ok())
                .map(|tf| tf.rotation.to_euler(EulerRot::YXZ).0)
        };
        if let Some(t) = transport {
            let local_facing = match facing {
                MonsterMoveFacing::None => None,
                MonsterMoveFacing::Angle(a) => Some(a),
                other => {
                    let target_pos = |g: u64| {
                        index
                            .0
                            .get(&g)
                            .and_then(|&te| transforms.get(te).ok())
                            .map(|tf| bevy_to_wow(tf.translation))
                    };
                    // The world position anchors the bearing; the wire's `start` is deck-local.
                    let world_pos = transforms.get(e).ok().map(|tf| bevy_to_wow(tf.translation));
                    world_pos
                        .and_then(|p| resolve_facing(other, p, target_pos))
                        .zip(deck_yaw(t))
                        .map(|(world, yaw)| world - yaw)
                }
            };
            // A monster move's transport change animates a vehicle seat's enter (`0x73c8e0`).
            commands.entity(e).insert(crate::vehicle::TransitionCue);
            match riders.get_mut(e) {
                Ok(mut rider) => {
                    rider.transport_guid = t;
                    rider.seat = seat;
                    rider.local_pos = start;
                    if let Some(o) = local_facing {
                        rider.local_orientation = o;
                    }
                }
                Err(_) => {
                    commands.entity(e).insert(crate::transport::TransportRider {
                        transport_guid: t,
                        local_pos: start,
                        local_orientation: local_facing.unwrap_or_default(),
                        seat,
                    });
                }
            }
        } else if riders.get(e).is_ok() {
            commands
                .entity(e)
                .remove::<crate::transport::TransportRider>()
                .insert(crate::vehicle::TransitionCue);
        }
        // The final facing (moveType 2/3/4) snaps the raw facing and resets the display smoother. A
        // following path overrides it with the tangent each frame, so it sticks only on a stop. A
        // deck packet's facing is the rider's `local_orientation`.
        if transport.is_none() && !matches!(facing, MonsterMoveFacing::None) {
            let target_pos = |g: u64| {
                index
                    .0
                    .get(&g)
                    .and_then(|&te| transforms.get(te).ok())
                    .map(|t| bevy_to_wow(t.translation))
            };
            if let Some(orientation) = resolve_facing(facing, start, target_pos) {
                if let Ok(mut t) = transforms.get_mut(e) {
                    t.rotation = Quat::from_rotation_y(orientation);
                    commands
                        .entity(e)
                        .remove::<crate::net::motion::DisplayFacing>();
                }
            }
        }
        // Every spline move un-nocks and drops the weapon hold here. The handler (opcodes 0xDD,
        // 0x2AE) ends through (clear `0x400`) and (un-nock) on every path but one: a stop within 3
        // yd of the stop point jumps past both.
        commands.entity(e).remove::<(
            crate::creature_anim::NockLatch,
            crate::creature_anim::RangedHold,
        )>();
        // A rooted unit takes no path, the only route into the spline installer, returns on ROOT.
        // Only the path is refused: the un-nock tail runs on every path, and the rest for a rooted
        // unit is untraced.
        if rooted {
            return;
        }
        // The spline supersedes the relay stream: a splined player (charge, taxi) would otherwise
        // keep stale flags that outrank the spline in the anim selector.
        commands.entity(e).remove::<RemoteMotion>();
        match monster_move_spline(
            path,
            spline_id,
            stop,
            duration_ms,
            flying,
            run_mode,
            transport,
            arc,
            facing_fixed,
            facing,
            cycle,
        ) {
            Some(spline) => {
                commands.entity(e).insert(spline).remove::<SplineStopped>();
            }
            // A stop keeps the id, which the server awaits back for a player-driven unit.
            None => {
                commands
                    .entity(e)
                    .remove::<Spline>()
                    .insert(SplineStopped(spline_id));
            }
        }
    }
}

/// This unit's granted modes; no component means all bits clear.
fn modes_of(guid: u64, index: &GuidIndex, modes: &Query<&mut UnitMoveModes>) -> UnitMoveModes {
    index
        .0
        .get(&guid)
        .and_then(|&e| modes.get(e).ok().copied())
        .unwrap_or_default()
}

/// `MOVEFLAG_PITCH_UP | PITCH_DOWN`, which the flight stop clears with the flight bits.
const PITCH_BITS: u32 = 0xc0;
/// `MOVEFLAG_SPLINE_ELEVATION`, which the flight start clears.
const SPLINE_ELEVATION: u32 = 0x400_0000;

/// `SMSG_SPLINE_MOVE_*`: a mode on a unit we do not control, with no ack. An unknown guid is
/// dropped, as the reference's handler does; our own guid is not special-cased, as vmangos never
/// sends it for a player's mover.
fn spline_move_mode(
    guid: u64,
    mode: SplineMode,
    apply: bool,
    commands: &mut Commands,
    index: &GuidIndex,
    modes: &mut Query<&mut UnitMoveModes>,
    remote: &mut Query<&mut RemoteMotion>,
) {
    let Some(&e) = index.0.get(&guid) else {
        return; // the reference drops it too
    };
    // Flight and gravity write the one flags word (`0x740ba0`), which for a unit we do not
    // control is the pose's: the flight start takes only on a unit that may fly (`0x989890`).
    if mode == SplineMode::Flying {
        if let Ok(mut rm) = remote.get_mut(e) {
            use crate::creature_anim::move_flags as f;
            let granted = modes_of(guid, index, modes).0;
            if apply && (rm.flags | granted) & f::CAN_FLY != 0 {
                rm.flags &= !(f::FALLING | f::FALLING_FAR | f::SWIMMING | SPLINE_ELEVATION);
                rm.flags |= f::FLYING;
                rm.vertical_velocity = 0.0;
                rm.fall_start_z = None;
            } else if !apply {
                // The flight stop (`0x98c240`): the pitch and the flight bits go.
                rm.flags &= !(PITCH_BITS | f::ASCENDING | f::DESCENDING | f::FLYING);
            }
            return;
        }
    }
    let mut word = modes_of(guid, index, modes);
    if mode == SplineMode::Flying {
        // With no relay state the kept word is the unit's whole word.
        use crate::creature_anim::move_flags as f;
        if apply && word.0 & f::CAN_FLY != 0 {
            word.0 &= !(f::FALLING | f::FALLING_FAR | f::SWIMMING | SPLINE_ELEVATION);
            word.0 |= f::FLYING;
        } else if !apply {
            word.0 &= !(PITCH_BITS | f::ASCENDING | f::DESCENDING | f::FLYING);
        }
    } else {
        word.set(mode, apply);
    }
    if let Ok(mut live) = modes.get_mut(e) {
        *live = word;
    } else {
        commands.entity(e).insert(word);
    }

    // Gravity off stops a fall where it is (`0x6eba60`).
    if mode == SplineMode::DisableGravity && apply {
        if let Ok(mut rm) = remote.get_mut(e) {
            use crate::creature_anim::move_flags;
            rm.flags &= !(move_flags::FALLING | move_flags::FALLING_FAR);
            rm.vertical_velocity = 0.0;
            rm.fall_start_z = None;
        }
    }

    // The root apply's one-shot wipe: it clears `0xffe07f00`'s complement and stops the fall, so
    // the unit fails the integration gate and a relayed player stops dead. A creature has no
    // direction bits; `monster_move` refuses its path instead.
    if mode == SplineMode::Root && apply {
        if let Ok(mut rm) = remote.get_mut(e) {
            use crate::creature_anim::move_flags;
            rm.flags &= ROOT_APPLY_WIPE & !(move_flags::FALLING | move_flags::FALLING_FAR);
            rm.speed = 0.0;
            rm.vertical_velocity = 0.0;
            rm.jump_xy_vel = [0.0; 2];
            rm.fall_start_z = None;
        }
    }
}

/// `SMSG_GAMEOBJECT_QUERY_RESPONSE`: cached, with the lockId resolved from its type's `data[]`
/// slot for use-vs-cast.
fn gameobject_info(
    entry: u32,
    type_id: u32,
    display_id: u32,
    name: String,
    data: &[i32; 24],
    go_templates: &mut GameObjectTemplates,
) {
    debug!("net: gameobject template {entry} type {type_id} display {display_id} {name:?}");
    go_templates.insert(entry, type_id, name, data);
}

/// A pet stat field's unit signal.
#[derive(Clone, Copy)]
enum PetSignal {
    Happiness,
    Experience,
    /// `UNIT_FIELD_FLAGS` `0x800` moved to set or clear: the flags callback's pet arm (`0x73c503`).
    Attack(bool),
    /// `UNIT_FIELD_PET_NAME_TIMESTAMP` moved: the name watcher's callback on offset `0x218`.
    Renamed,
    /// `UNIT_FIELD_PETNUMBER` moved: its field handler (`0x72cff0`, registered on offset `0x114` at `0x741f66`).
    Number,
}

/// The unit signal a pet stat field raises: the field's index is its event slot (`0x614190`), 23 `UNIT_HAPPINESS` and 71-72 `UNIT_PET_EXPERIENCE`.
fn pet_signal(e: &FieldChanged) -> Option<PetSignal> {
    use benilla_protocol::field::{
        FIELD_UNIT_PETEXPERIENCE, FIELD_UNIT_PETNEXTLEVELEXP, FIELD_UNIT_PETNUMBER,
        FIELD_UNIT_PET_NAME_TIMESTAMP, FIELD_UNIT_POWER1,
    };
    if e.unit_field(FIELD_UNIT_PETNUMBER) {
        Some(PetSignal::Number)
    } else if e.unit_field(FIELD_UNIT_PET_NAME_TIMESTAMP) {
        Some(PetSignal::Renamed)
    } else if e.unit_field(FIELD_UNIT_POWER1 + 4) {
        Some(PetSignal::Happiness)
    } else if e.unit_field(FIELD_UNIT_PETEXPERIENCE) || e.unit_field(FIELD_UNIT_PETNEXTLEVELEXP) {
        Some(PetSignal::Experience)
    } else {
        None
    }
}

/// The unit signals of the unit the `"pet"` token names, our charm else our summon, with that token (`0x60bb70`); the fan-out to the other tokens naming it is the units' (#1289). The flags callback's pet arm (`0x73c503`-`0x73c53d`) fires `PET_ATTACK_START`/`STOP` for a unit whose charmer, else summoner, is us (`0x5d30a0`), in field order with the others.
fn pet_token_watch(sc: &mut Scene, guid: u64, merged: &Merged) {
    if merged.pet_signals.is_empty() {
        return;
    }
    let pet = sc
        .self_guid
        .0
        .and_then(|me| sc.index.0.get(&me))
        .and_then(|&e| sc.stores.get(e).ok())
        .and_then(|s| s.0.unit_pet_guid());
    let ours = sc
        .index
        .0
        .get(&guid)
        .and_then(|&e| sc.stores.get(e).ok())
        .is_some_and(|s| {
            sc.self_guid.0.is_some() && crate::ui_pet::pet_combat_flag(s, sc.self_guid.0).is_some()
        });
    use benilla_ui::script::ScriptValue;
    for &event in &merged.pet_signals {
        match event {
            PetSignal::Happiness if pet == Some(guid) => sc
                .ui
                .fire("UNIT_HAPPINESS", vec![ScriptValue::Str("pet".into())]),
            PetSignal::Experience if pet == Some(guid) => sc
                .ui
                .fire("UNIT_PET_EXPERIENCE", vec![ScriptValue::Str("pet".into())]),
            // The signal, then the pet-attacking global (`0x5d30b2` then `0x5d30ba`, `0x5d30c8` then `0x5d30d0`): a handler of the event reads the old value.
            PetSignal::Attack(on) if ours => {
                sc.ui.fire(
                    if on {
                        "PET_ATTACK_START"
                    } else {
                        "PET_ATTACK_STOP"
                    },
                    Vec::new(),
                );
                sc.ui.world(move |world| {
                    if let Some(mut bar) = world.get_resource_mut::<crate::ui_pet::PetBar>() {
                        bar.attacking = on;
                    }
                });
            }
            PetSignal::Renamed => crate::ui_pet::pet_renamed(guid, ours, &mut sc.ui),
            PetSignal::Number => pet_number_watch(sc, guid),
            _ => {}
        }
    }
}

/// The pet number's field handler (`0x72cff0`): a held unit's name re-read, asking on a miss (`0x72d01c`), its scale restamped by `entities::model_scale`'s store watch (`0x72d025`) and its nameplate released (`0x72d02c`); then `UNIT_CLASSIFICATION_CHANGED` to every token naming the guid (`0x72d034` -> `0x60bf10`), and `PET_UI_UPDATE` when it is the pet list's head (`0x72d041`, `0x72d05f`).
fn pet_number_watch(sc: &mut Scene, guid: u64) {
    if let Some(store) = sc.index.0.get(&guid).and_then(|&e| sc.stores.get(e).ok()) {
        let _ = sc.names.resolve_unit(guid, Some(store), &sc.net);
        sc.ui.write(move |script| script.release_nameplate(guid));
    }
    sc.ui.write(move |script| {
        script.fire_unit_event(guid, "UNIT_CLASSIFICATION_CHANGED", Vec::new());
    });
    if sc.pets.as_ref().and_then(|l| l.head()) == Some(guid) {
        sc.ui.fire("PET_UI_UPDATE", Vec::new());
    }
}

/// Our own unit's watches in field order (`0x4d5550`): critter (`0x6cf6c0`), health (`0x73f330`), flags (`0x60c240`), dynamic and player flags (`0x6e103a`, `0x6e10fb`), honor.
fn own_flags_watch(sc: &mut Scene, guid: u64, merged: Merged) {
    use benilla_protocol::field::{
        FIELD_PLAYER_FARSIGHT, FIELD_PLAYER_FLAGS, FIELD_UNIT_DISPLAYID, FIELD_UNIT_FLAGS,
    };
    // The entry watch (`0x707960`), the count watch (`0x707720`), the charges watch (`0x707880`), then the enchant watch (`0x708bd0`), each its own handler in field order.
    if let Some(old) = merged.item_entry {
        item_entry_watch(sc, guid, old);
    }
    // The count watch's quest log and currency legs sit in its own handler, past its container and character page legs (`0x707820`, `0x70782c`).
    if merged.item_stack {
        sc.ui
            .world(move |world| crate::ui_items::watch::stack_changed(world, guid));
        item_quest_watch(sc, guid);
    }
    // The charges watch (`0x707880`), between the count's and the enchants' in field order.
    if merged.item_charges {
        sc.ui
            .world(move |world| crate::ui_items::watch::charges_changed(world, guid));
    }
    if merged.item_enchants {
        sc.ui
            .world(move |world| crate::ui_items::watch::enchants_changed(world, guid));
    }
    // The bag's slot watch (`0x5d9600`), once per slot field that moved, only on a bag the UI watches.
    for (j, old) in merged.bag_slots.clone() {
        sc.ui
            .world(move |world| crate::ui_items::watch::bag_slot_changed(world, guid, j, old));
    }
    // The unit death's interact-target clear with its guid (`0x7293bf` -> `0x512e60`): the window on it closes.
    if merged.died {
        sc.ui
            .world(move |world| crate::ui_session::clear_interact_target(world, guid));
    }
    // A unit's entry watch re-reads its creature record and renames the totem slot it holds
    // (`0x72ceb0`, `0x72cf2b`).
    if merged.unit_entry {
        sc.ui
            .world(move |world| crate::ui_totem::entry_changed(world, guid));
    }
    // The NPC-flag watch's quest leg (`0x71a104`-`0x71a133`): the questgiver bit lost on the open window's giver closes it through the giver's close, nothing owed.
    if let Some((old, new)) = merged.npc_flags {
        if old & NPC_FLAG_QUESTGIVER != 0 && new & NPC_FLAG_QUESTGIVER == 0 {
            sc.ui.world(move |world| {
                let open = world
                    .get_non_send_resource::<crate::ui_quest::QuestGiver>()
                    .is_some_and(|q| q.npc == Some(guid));
                if open {
                    crate::ui_quest::giver_finished(world, false, 0, false);
                }
            });
        }
        // Its taxi leg (`0x71a13b`-`0x71a16d`): the flight master bit lost on the open map's flight master.
        if old & NPC_FLAG_FLIGHTMASTER != 0 && new & NPC_FLAG_FLIGHTMASTER == 0 {
            sc.ui.world(move |world| {
                let open = world
                    .get_resource::<crate::ui_taxi::TaxiState>()
                    .is_some_and(|t| t.unit == Some(guid));
                if open {
                    crate::ui_taxi::close_map(world);
                }
            });
        }
        // Its trainer leg (`0x71a172`-`0x71a1a0`): the trainer bit lost on the open trainer.
        if old & NPC_FLAG_TRAINER != 0 && new & NPC_FLAG_TRAINER == 0 {
            sc.ui.world(move |world| {
                let open = world
                    .get_resource::<crate::ui_trainer::TrainerOpen>()
                    .is_some_and(|t| t.trainer == Some(guid));
                if open {
                    crate::ui_trainer::set_none(world);
                }
            });
        }
        // Its bank leg (`0x71a1a8`-`0x71a1db`): the banker bit lost on the open bank's banker.
        if old & NPC_FLAG_BANKER != 0 && new & NPC_FLAG_BANKER == 0 {
            sc.ui.world(move |world| {
                let open = world
                    .get_non_send_resource::<crate::ui_bank::BankOpen>()
                    .is_some_and(|b| b.banker == Some(guid));
                if open {
                    crate::ui_bank::set_banker_none(world);
                }
            });
        }
        // Its guild bank leg (`0x71a1eb`-`0x71a216`): the guild banker bit lost on the open guild banker.
        if old & NPC_FLAG_GUILD_BANKER != 0 && new & NPC_FLAG_GUILD_BANKER == 0 {
            sc.ui.world(move |world| {
                let open = world
                    .get_non_send_resource::<crate::ui_guild_bank::GuildBankState>()
                    .is_some_and(|b| b.banker() == Some(guid));
                if let (true, Some(mut ui)) = (
                    open,
                    world.get_resource_mut::<crate::ui_script::live::UiEvents>(),
                ) {
                    crate::ui_guild_bank::set_guild_banker_none(&mut ui);
                }
            });
        }
    }
    // Another player's guild id watch (`0x6e1bb0` -> `0x6e1bd9` -> `0x6df7a0`).
    if merged.guild_moved {
        sc.ui.fire("TABARD_CANSAVE_CHANGED", Vec::new());
    }
    // Another player's guild timestamp watch (`0x6e1c60`), after its guild id's (`0x6e1bb0`); ours runs in index order in [`own_player_watches`].
    if let Some(stamp) = merged.guild_stamp {
        guild_stamp_watch(sc, guid, stamp);
    }
    // The target's aura state (`0x716810`), and our pet's power, its predicted copy re-seeded
    // (`0x722c50` -> `0x53d1b0`).
    let target = sc.selection.as_deref().and_then(|s| s.guid);
    if sc.self_guid.0 != Some(guid) {
        if merged.aura_state_moved && target == Some(guid) {
            sc.ui.world(crate::ui_action::usable_cache::step(
                crate::ui_action::usable_cache::Pass::Full,
            ));
        }
        let our_pet = sc
            .self_guid
            .0
            .and_then(|me| sc.index.0.get(&me))
            .and_then(|&e| sc.stores.get(e).ok())
            .and_then(|s| s.0.unit_pet_guid());
        if merged.power_moved && our_pet == Some(guid) {
            sc.ui.world(crate::ui_action::usable_cache::step(
                crate::ui_action::usable_cache::Pass::Power,
            ));
        }
        return;
    }
    if merged.pet_moved {
        possess_lookup(sc, guid);
        // Our charm and summon fields' unit watches, event slots 0 and 2 (`0x614190`): `UNIT_PET` with the `"pet"` token pushed ahead.
        sc.ui.world(crate::ui_pet::push_pet_token);
        sc.ui.fire(
            "UNIT_PET",
            vec![benilla_ui::script::ScriptValue::Str("player".into())],
        );
    }
    if merged.critter_moved {
        // The bar's companion update for the old critter, then the new (`0x6cf6fc`, `0x6cf71b`).
        let new = sc
            .index
            .0
            .get(&guid)
            .and_then(|&e| sc.stores.get(e).ok())
            .and_then(|s| s.0.unit_critter())
            .unwrap_or(0);
        let (lo, hi) = merged.critter_old;
        let old = (u64::from(hi.unwrap_or((new >> 32) as u32)) << 32)
            | u64::from(lo.unwrap_or(new as u32));
        for critter in [old, new] {
            let entry = crate::ui_companion::critter_entry(critter);
            sc.ui
                .world(move |world| crate::ui_action::slots::critter_changed(world, entry));
        }
        crate::ui_companion::critter_update(&mut sc.ui);
    }
    let split = merged
        .death
        .partition_point(|&(index, ..)| index < FIELD_UNIT_FLAGS);
    let (before, after) = merged.death.split_at(split);
    own_death_fields(sc, guid, before);
    // Our level's change, in field order: the trainer's re-evaluation (`0x6d6702`).
    if merged.level_moved {
        sc.ui.world(crate::ui_trainer::refresh);
    }
    if merged.flags_moved {
        sc.ui.fire(
            "UNIT_FLAGS",
            vec![benilla_ui::script::ScriptValue::Str("player".into())],
        );
    }
    let split = after.partition_point(|&(index, ..)| index < FIELD_PLAYER_FLAGS);
    let (unit, player) = after.split_at(split);
    let split = unit.partition_point(|&(index, ..)| index < FIELD_UNIT_DISPLAYID);
    let (unit, past_display) = unit.split_at(split);
    own_death_fields(sc, guid, unit);
    // Our display id's watch (`0x716860`) closes an open barber shop, in field order.
    if merged.display_moved {
        sc.ui.world(crate::ui_barber::display_changed);
    }
    own_death_fields(sc, guid, past_display);
    if let Some(form) = merged.form {
        sc.ui
            .world(move |world| crate::ui_bank::form_entered(world, form));
        // The form watch (`0x6da130`): a form entered with nothing tracked casts the first tracking
        // spell the form admits, then the usable update, the selection update (`0x6da389`), the
        // form's preset bar and the bonus update (`0x6da38e`) and `UPDATE_SHAPESHIFT_FORM`.
        if form != 0 {
            sc.ui.world(move |world| track_in_form(world, form));
        }
        sc.ui.world(crate::ui_action::usable_cache::step(
            crate::ui_action::usable_cache::Pass::Full,
        ));
        crate::ui_action::signals::selection(&mut sc.ui);
        sc.ui.world(crate::ui_action::bonus::form_changed);
        sc.ui.fire("UPDATE_SHAPESHIFT_FORM", Vec::new());
    }
    // Our own fields' usable updates, the aura state's among them (`0x716810`).
    for _ in 0..merged.usable_updates + u8::from(merged.aura_state_moved) {
        sc.ui.world(crate::ui_action::usable_cache::step(
            crate::ui_action::usable_cache::Pass::Full,
        ));
    }
    // The override-spell watch's selection update after its usable update (`0x6d1c58`), then the
    // override bar and the bonus update (`0x6d1c5d`, `0x6d1c62`).
    if merged.override_moved {
        crate::ui_action::signals::selection(&mut sc.ui);
        sc.ui.world(crate::ui_action::bonus::override_changed);
    }
    if merged.power_moved {
        sc.ui.world(crate::ui_action::usable_cache::step(
            crate::ui_action::usable_cache::Pass::Power,
        ));
    }
    for _ in 0..merged.bar_updates {
        sc.ui.world(crate::ui_action::usable_cache::step(
            crate::ui_action::usable_cache::Pass::Bar,
        ));
    }
    // The level watch's book update, no re-sort (`0x6d66fd`), and each visible weapon's: the main
    // hand's or the ranged slot's entry moved updates the book for its weapon face (`0x6e1f13`,
    // `0x6e1f37`).
    for _ in 0..u8::from(merged.level_moved) + merged.weapons_moved {
        sc.ui.fire("SPELLS_CHANGED", Vec::new());
    }
    if let Some((old, new)) = merged.pvp_flags {
        sc.pvp.watch(old, new, std::time::Instant::now());
    }
    let split = player.partition_point(|&(index, ..)| index < FIELD_PLAYER_FARSIGHT);
    let (player, past_farsight) = player.split_at(split);
    own_death_fields(sc, guid, player);
    // The log window's watch (`0x60c280`), then each moved slot's watch (`0x6df370`) in slot order.
    if !merged.quest_slots.is_empty() {
        sc.ui.fire(
            "UNIT_QUEST_LOG_CHANGED",
            vec![benilla_ui::script::ScriptValue::Str("player".into())],
        );
        if let Some(store) = sc.index.0.get(&guid).and_then(|&e| sc.stores.get(e).ok()) {
            let edges = quest_slot_edges(&merged.quest_slots, &store.0);
            sc.ui
                .world(move |world| crate::ui_quest_log::rows::slot_watch(world, edges));
        }
    }
    if merged.farsight_moved {
        farsight_possess(sc, guid);
        // The far-sight callback's temporary bar update (`0x6e504a`).
        sc.ui.world(crate::ui_pet::update_pet_temp_bar_step);
    }
    own_death_fields(sc, guid, past_farsight);
    own_player_watches(sc, guid, &merged.watched);
}

/// The guild timestamp's watch (`0x6e1c60`): a stamp new for the player's guild drops its record and asks it again with the walk as its callback (`0x6e1d5f` -> `0x6e1b90`).
fn guild_stamp_watch(sc: &mut Scene, guid: u64, stamp: u32) {
    let Some(guild_id) = sc
        .index
        .0
        .get(&guid)
        .and_then(|&e| sc.stores.get(e).ok())
        .map(|s| s.0.player_guild_id())
    else {
        return;
    };
    let net = NetCommands(sc.net.0.clone());
    sc.ui.world(move |world| {
        if let Some(mut cache) = world.get_resource_mut::<crate::ui_guild::GuildCache>() {
            cache.stamp_changed(guild_id, stamp, &net);
        }
    });
}

/// The possess lookup (`0x5d62a0`) over our held fields, auras and transport.
fn possess_lookup(sc: &mut Scene, guid: u64) {
    let Some(&e) = sc.index.0.get(&guid) else {
        return;
    };
    let Ok(store) = sc.stores.get(e) else {
        return;
    };
    let riding = sc.riders.get(e).ok().map(|r| r.transport_guid);
    let (held, spells, summons) = &mut sc.possess;
    let catalogs = spells
        .as_deref()
        .map(|s| &s.catalog)
        .zip(summons.as_deref().map(|s| &s.0));
    crate::ui_pet::possess::find_possessed(
        &store.0,
        sc.auras.get(e).ok(),
        riding,
        catalogs,
        held,
        &mut sc.ui,
    );
}

/// Our far-sight watch's possess half: a subject held as an object looks the spell up (`0x6e5034`), any other clears it (`0x6e5042`).
fn farsight_possess(sc: &mut Scene, guid: u64) {
    let subject = sc
        .index
        .0
        .get(&guid)
        .and_then(|&e| sc.stores.get(e).ok())
        .and_then(|s| s.0.player_farsight());
    if subject.is_some_and(|g| sc.index.0.contains_key(&g)) {
        possess_lookup(sc, guid);
    } else {
        crate::ui_pet::possess::set_possess_spell(&mut sc.possess.0, 0, &mut sc.ui);
    }
}

/// Our currency watches: the backpack, keyring and token slots' guids (`0x5d9c5b`, `0x5d9cb9`, `0x5d9ce8`) and the known-currencies pair (`0x6e5401`).
fn is_currency_watch(index: u16) -> bool {
    use benilla_protocol::field::{player_slot_of_field, FIELD_PLAYER_KNOWN_CURRENCIES};
    (FIELD_PLAYER_KNOWN_CURRENCIES..FIELD_PLAYER_KNOWN_CURRENCIES + 2).contains(&index)
        || player_slot_of_field(index)
            .is_some_and(|(slot, _)| crate::ui_currency::watched_slot(slot))
}

/// The buyback prices and stamps, the one range `0x585f80` watches.
fn is_buyback_watch(index: u16) -> bool {
    let first = benilla_protocol::field::FIELD_PLAYER_BUYBACK_PRICE_1;
    (first..first + 24).contains(&index)
}

/// A skill slot's second or third word, whose halves the skill watches cover (`0x6e5426`, `0x6e5450`, `0x6e546a`).
fn is_skill_watch(index: u16) -> bool {
    let first = benilla_protocol::field::FIELD_PLAYER_SKILL_INFO_1_1;
    index.checked_sub(first).is_some_and(|r| {
        r < 3 * u16::from(benilla_protocol::messages::PLAYER_SKILL_SLOTS) && r % 3 != 0
    })
}

/// How many skill watches a moved skill word runs: the second word's value half (`0x6da4a0`) and max half (`0x6cddb0`), the third word's permanent-bonus half (`0x6cddb0`).
fn skill_watch_runs(index: u16, old: u32, new: u32) -> usize {
    let first = benilla_protocol::field::FIELD_PLAYER_SKILL_INFO_1_1;
    let low = old as u16 != new as u16;
    let high = old >> 16 != new >> 16;
    match (index - first) % 3 {
        1 => usize::from(low) + usize::from(high),
        _ => usize::from(high),
    }
}

/// Our slot fields' watches, the inventory watch's (`0x6e51c5`-`0x6e52c3`) and the containers' (`0x5d9c20`): every slot but the buyback's.
fn is_slot_watch(index: u16) -> bool {
    benilla_protocol::field::player_slot_of_field(index)
        .is_some_and(|(slot, _)| !(74..=85).contains(&slot))
}

/// One slot field's watches at `at` in `edges`, the field's handlers once: the containers' first (`0x5d98e3`), then the currency's (`0x5d991f`).
fn own_slot_watches(sc: &mut Scene, guid: u64, edges: &[(u16, u32, u32)], at: usize) {
    use benilla_protocol::field::player_slot_of_field;
    let (index, ..) = edges[at];
    let pair = index & !1;
    if index & 1 == 1 && at > 0 && edges[at - 1].0 == pair {
        return;
    }
    if let Some((slot, old, new)) = slot_edge(sc, guid, edges, at) {
        sc.ui
            .world(move |world| crate::ui_items::watch::inv_slot_changed(world, slot, old, new));
    }
    if is_currency_watch(index) || player_slot_of_field(index).is_none() {
        own_currency_watch(sc, guid, edges, at);
    }
}

/// The slot field at `at` in `edges`: its slot, old guid and new guid.
fn slot_edge(
    sc: &Scene,
    guid: u64,
    edges: &[(u16, u32, u32)],
    at: usize,
) -> Option<(u8, u64, u64)> {
    use benilla_protocol::field::player_slot_of_field;
    let (index, ..) = edges[at];
    let pair = index & !1;
    let (slot, _) = player_slot_of_field(index)?;
    let new = sc
        .index
        .0
        .get(&guid)
        .and_then(|&e| sc.stores.get(e).ok())
        .and_then(|s| s.0.player_slot(slot))?;
    let word = |i: u16, now: u32| {
        edges
            .iter()
            .find(|&&(at, ..)| at == i)
            .map_or(now, |&(_, old, _)| old)
    };
    let (lo_index, hi_index) = if player_slot_of_field(pair) == Some((slot, false)) {
        (pair, pair + 1)
    } else {
        (pair + 1, pair)
    };
    let old =
        u64::from(word(lo_index, new as u32)) | u64::from(word(hi_index, (new >> 32) as u32)) << 32;
    Some((slot, old, new))
}

/// Our currency watches' edges at `at` in `edges`, each 8-byte field's handler once (`0x4d5a80`'s size 8): the known pair's, or a slot's with its old and new guid.
fn own_currency_watch(sc: &mut Scene, guid: u64, edges: &[(u16, u32, u32)], at: usize) {
    use benilla_protocol::field::{player_slot_of_field, FIELD_PLAYER_KNOWN_CURRENCIES};
    let (index, ..) = edges[at];
    let pair = index & !1;
    // The field's low word when it moved, else its high word: the handler runs once per field.
    if index & 1 == 1 && at > 0 && edges[at - 1].0 == pair {
        return;
    }
    if (FIELD_PLAYER_KNOWN_CURRENCIES..FIELD_PLAYER_KNOWN_CURRENCIES + 2).contains(&index) {
        crate::ui_currency::known_update(&mut sc.ui);
        return;
    }
    let Some((slot, _)) = player_slot_of_field(index) else {
        return;
    };
    let Some(new) = sc
        .index
        .0
        .get(&guid)
        .and_then(|&e| sc.stores.get(e).ok())
        .and_then(|s| s.0.player_slot(slot))
    else {
        return;
    };
    let word = |i: u16, now: u32| {
        edges
            .iter()
            .find(|&&(at, ..)| at == i)
            .map_or(now, |&(_, old, _)| old)
    };
    let (lo_index, hi_index) = if player_slot_of_field(pair) == Some((slot, false)) {
        (pair, pair + 1)
    } else {
        (pair + 1, pair)
    };
    let old =
        u64::from(word(lo_index, new as u32)) | u64::from(word(hi_index, (new >> 32) as u32)) << 32;
    // Both items' classes read inside the field's handler, as the update stands now (`0x5d97fd`, `0x5d9813`).
    let class_of = |guid: u64| {
        let entry = sc
            .index
            .0
            .get(&guid)
            .and_then(|&e| sc.stores.get(e).ok())?
            .0
            .object_entry()?;
        let row = sc.icons.as_deref()?.item_dbc.get(&entry)?;
        Some(u32::from(row.class))
    };
    if crate::ui_currency::slot_fires(slot, old, new, class_of) {
        crate::ui_currency::display_update(&mut sc.ui);
    }
}

/// Our player watches past the flags in field order: the bank's slot byte, honor, the currency watches, then the arena-team words once per update (`0x6cdda0`).
fn own_player_watches(sc: &mut Scene, guid: u64, edges: &[(u16, u32, u32)]) {
    let mut teams_done = false;
    let mut buyback_done = false;
    for (at, &(index, old, new)) in edges.iter().enumerate() {
        // The coinage watch (`0x6da070`): the trainer's re-evaluation (`0x6da085`), then `PLAYER_MONEY` (`0x6da091`); its coin sound plays from [`crate::sound`].
        if index == benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE {
            sc.ui.world(crate::ui_trainer::refresh);
            sc.ui.fire("PLAYER_MONEY", Vec::new());
            continue;
        }
        // The skill slots' value, max and permanent-bonus halves, each its own watch re-evaluating the trainer (`0x6da5f3`, `0x6cddb0`).
        if is_skill_watch(index) {
            for _ in 0..skill_watch_runs(index, old, new) {
                sc.ui.world(crate::ui_trainer::refresh);
            }
            continue;
        }
        // The buyback prices' and stamps' watch, one handler over the range, once per update (`0x585f60`).
        if is_buyback_watch(index) {
            if !std::mem::replace(&mut buyback_done, true) {
                sc.ui.world(crate::ui_merchant::buyback_changed);
            }
            continue;
        }
        if is_slot_watch(index) || is_currency_watch(index) {
            own_slot_watches(sc, guid, edges, at);
            continue;
        }
        // The guild id's watch on the active player: the guild change's event (`0x6e1bd9` -> `0x6df7a0`), then the guild bank's tabs clear (`0x6e1bb0`).
        if index == benilla_protocol::field::FIELD_PLAYER_GUILDID {
            if old != new {
                sc.ui.fire("TABARD_CANSAVE_CHANGED", Vec::new());
                // The merchant's refresh with a vendor set (`0x6e1c09`), ahead of the tabs' clear.
                sc.ui.world(crate::ui_merchant::refresh);
                crate::ui_guild_bank::guild_id_changed(&mut sc.ui);
            }
            continue;
        }
        // Our guild timestamp's watch (`0x6e1c60`), after our guild id's in field order.
        if index == benilla_protocol::field::FIELD_PLAYER_GUILD_TIMESTAMP {
            guild_stamp_watch(sc, guid, new);
            continue;
        }
        // Our look bytes' watches, by byte: skin (`0x6da870`), hair style (`0x6df530`), hair colour (`0x6da7f0`).
        if index == benilla_protocol::field::FIELD_PLAYER_BYTES {
            for (byte, dial) in [(0, 0), (2, 2), (3, 3)] {
                look_watch(sc, old, new, byte, dial);
            }
            continue;
        }
        // The facial hair's watch (`0x6df620`), then the bank's on the purchased-slots byte (`0x57ba60` -> `0x57b8b0`).
        if index == benilla_protocol::field::FIELD_PLAYER_BYTES_2 {
            look_watch(sc, old, new, 0, 4);
            if (old >> 16) as u8 != (new >> 16) as u8 {
                sc.ui.fire("PLAYERBANKBAGSLOTS_CHANGED", Vec::new());
            }
            continue;
        }
        if !crate::ui_arena_team::is_team_info(index) {
            crate::ui_honor::own_field_changed(index, old, new, &mut sc.ui);
            continue;
        }
        if std::mem::replace(&mut teams_done, true) {
            continue;
        }
        let Some(&e) = sc.index.0.get(&guid) else {
            continue;
        };
        let Ok(store) = sc.stores.get(e) else {
            continue;
        };
        crate::ui_arena_team::info_changed(
            &mut sc.arena_teams,
            &mut sc.roster_throttle,
            &store.0,
            &sc.net,
            &mut sc.ui,
        );
    }
}

/// One of our look bytes' watches: a moved byte lands on the barber body's dial and queues `BARBER_SHOP_APPEARANCE_APPLIED` (`0x52e4d0`).
fn look_watch(sc: &mut Scene, old: u32, new: u32, byte: u32, dial: usize) {
    let value = (new >> (byte * 8)) as u8;
    if (old >> (byte * 8)) as u8 == value {
        return;
    }
    sc.ui
        .world(move |world| crate::ui_barber::look_changed(world, dial, value));
    sc.signals.unique_signal("BARBER_SHOP_APPEARANCE_APPLIED");
}

/// Our own death fields' edges, in field order, through [`crate::death::own_fields_changed`].
fn own_death_fields(sc: &mut Scene, guid: u64, edges: &[(u16, u32, u32)]) {
    if edges.is_empty() {
        return;
    }
    let Some(fields) = sc
        .index
        .0
        .get(&guid)
        .and_then(|&e| sc.stores.get(e).ok())
        .map(|s| s.0.clone())
    else {
        return;
    };
    let arena = sc.arena.active();
    let mut cx = crate::death::DeathCx {
        globals: &mut sc.death_globals,
        death_net: &mut sc.death_net,
        ui: &mut sc.ui,
        net: &sc.net,
        now: sc.real.elapsed_secs_f64(),
        arena,
    };
    crate::death::own_fields_changed(edges, &fields, &mut cx);
}

/// Merges a descriptor delta into an object's store and reports its [`FieldChanged`] edges, and
/// whether a unit's flags word moved, and for `own` (our own player) the death fields' edges.
/// An indexed entity with no store should not happen, and is seeded from the delta.
fn merge_fields(
    commands: &mut Commands,
    stores: &mut Query<&mut ObjectStore>,
    edges: &mut MessageWriter<FieldChanged>,
    entity: Entity,
    guid: u64,
    delta: ObjectFields,
    own: bool,
    seated: Option<&Query<&crate::vehicle::Passenger>>,
) -> Merged {
    use benilla_protocol::field::{
        FIELD_PLAYER_FLAGS, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH,
    };
    let mut merged = Merged::default();
    if let Ok(mut s) = stores.get_mut(entity) {
        let copy = s.0.unit_mount_copy();
        let mut mount_zeroed = false;
        merge_store_fields(&mut s.0, delta, entity, guid, |e| {
            mount_zeroed |=
                e.unit_field(benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID) && e.new == 0;
            merged.flags_moved |= e.unit_field(benilla_protocol::field::FIELD_UNIT_FLAGS);
            merged.display_moved |=
                own && e.unit_field(benilla_protocol::field::FIELD_UNIT_DISPLAYID);
            merged.level_moved |= own && e.unit_field(benilla_protocol::field::FIELD_UNIT_LEVEL);
            if e.unit_field(benilla_protocol::field::FIELD_UNIT_NPC_FLAGS) {
                merged.npc_flags = Some((e.old, e.new));
            }
            use benilla_protocol::field::{
                FIELD_PLAYER_FARSIGHT, FIELD_UNIT_CHARM, FIELD_UNIT_CRITTER, FIELD_UNIT_SUMMON,
            };
            let pair = |f: u16| e.unit_field(f) || e.unit_field(f + 1);
            merged.pet_moved |= own && (pair(FIELD_UNIT_CHARM) || pair(FIELD_UNIT_SUMMON));
            merged.farsight_moved |= own && pair(FIELD_PLAYER_FARSIGHT);
            if own && e.unit_field(FIELD_UNIT_CRITTER) {
                merged.critter_moved = true;
                merged.critter_old.0 = Some(e.old);
            }
            if own && e.unit_field(FIELD_UNIT_CRITTER + 1) {
                merged.critter_moved = true;
                merged.critter_old.1 = Some(e.old);
            }
            if let Some(event) = pet_signal(&e) {
                merged.pet_signals.push(event);
            }
            // The flags callback's changed mask, then the new word's bit (`0x73c503`, `0x73c536`).
            if e.unit_field(benilla_protocol::field::FIELD_UNIT_FLAGS)
                && (e.old ^ e.new) & 0x800 != 0
            {
                merged
                    .pet_signals
                    .push(PetSignal::Attack(e.new & 0x800 != 0));
            }
            if own
                && (e.unit_field(FIELD_UNIT_HEALTH)
                    || e.unit_field(FIELD_UNIT_DYNAMIC_FLAGS)
                    || e.unit_field(FIELD_PLAYER_FLAGS))
            {
                merged.death.push((e.index, e.old, e.new));
            }
            // A unit's death: health crossing zero (`0x73f3fa`) or the dead bit set (`0x740623`).
            merged.died |=
                (e.unit_field(FIELD_UNIT_HEALTH) && (e.old as i32) > 0 && (e.new as i32) < 1)
                    || (e.unit_field(FIELD_UNIT_DYNAMIC_FLAGS) && !e.old & e.new & 0x20 != 0);
            // Our form byte's watch (`0x6da130`, byte `0x1d3`).
            if own
                && e.unit_field(benilla_protocol::field::FIELD_UNIT_BYTES_2)
                && (e.old ^ e.new) >> 24 != 0
            {
                merged.form = Some((e.new >> 24) as u8);
            }
            merged.usable_updates += usable_updates(&e, own);
            merged.override_moved |= own
                && e.unit_field(benilla_protocol::field::FIELD_PLAYER_FIELD_BYTES2)
                && (e.old ^ e.new) & 0xffff != 0;
            // Our visible items' entries (`0x6e1ec2`) and our ammo (`0x5e8413`) refresh the bar.
            merged.bar_updates += u8::from(
                own && ((0..19u8).any(|slot| {
                    e.index == benilla_protocol::field::player_visible_item_entry_field(slot)
                }) || e.index == benilla_protocol::field::FIELD_PLAYER_AMMO_ID),
            );
            merged.aura_state_moved |= e.unit_field(benilla_protocol::field::FIELD_UNIT_AURASTATE);
            // A power field moving re-seeds the predicted copy (`0x7234d0` -> `0x722c50`).
            merged.power_moved |=
                (0..7u16).any(|ty| e.unit_field(benilla_protocol::field::FIELD_UNIT_POWER1 + ty));
            if own
                && [15, 17].into_iter().any(|slot| {
                    e.index == benilla_protocol::field::player_visible_item_entry_field(slot)
                })
            {
                merged.weapons_moved += 1;
            }
            if own && e.is_unit() && QUEST_LOG_FIELDS.contains(&e.index) {
                merged.quest_slots.push((e.index, e.old, e.new));
            }
            if matches!(
                e.kind,
                benilla_protocol::messages::ObjectType::Item
                    | benilla_protocol::messages::ObjectType::Container
            ) {
                use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_ITEM_ENCHANTMENT_1_1};
                merged.item_stack |= e.index == benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
                merged.item_charges |= (benilla_protocol::field::FIELD_ITEM_SPELL_CHARGES
                    ..benilla_protocol::field::FIELD_ITEM_SPELL_CHARGES + 5)
                    .contains(&e.index);
                if e.index == benilla_protocol::field::FIELD_OBJECT_ENTRY {
                    merged.item_entry = Some(e.old);
                }
                merged.item_enchants |= (FIELD_ITEM_ENCHANTMENT_1_1
                    ..FIELD_ITEM_ENCHANTMENT_1_1 + 36)
                    .contains(&e.index);
                if e.kind == benilla_protocol::messages::ObjectType::Container
                    && (FIELD_CONTAINER_SLOT_1..FIELD_CONTAINER_SLOT_1 + 72).contains(&e.index)
                {
                    let j = ((e.index - FIELD_CONTAINER_SLOT_1) / 2) as u8;
                    let high = (e.index - FIELD_CONTAINER_SLOT_1) % 2 == 1;
                    if merged.bag_slots.last().map(|&(k, _)| k) != Some(j) {
                        merged.bag_slots.push((j, (None, None)));
                    }
                    if let Some((_, (lo, hi))) = merged.bag_slots.last_mut() {
                        if high {
                            *hi = Some(e.old);
                        } else {
                            *lo = Some(e.old);
                        }
                    }
                }
            }
            merged.unit_entry |=
                e.is_unit() && e.index == benilla_protocol::field::FIELD_OBJECT_ENTRY;
            if own && e.unit_field(FIELD_PLAYER_FLAGS) {
                merged.pvp_flags = Some((e.old, e.new));
            }
            if e.kind == benilla_protocol::messages::ObjectType::Player {
                merged.guild_moved |=
                    !own && e.unit_field(benilla_protocol::field::FIELD_PLAYER_GUILDID);
                if !own && e.unit_field(benilla_protocol::field::FIELD_PLAYER_GUILD_TIMESTAMP) {
                    merged.guild_stamp = Some(e.new);
                }
            }
            if own
                && (crate::ui_honor::is_honor_watch(e.index)
                    || crate::ui_arena_team::is_team_info(e.index)
                    || e.unit_field(benilla_protocol::field::FIELD_PLAYER_BYTES)
                    || e.unit_field(benilla_protocol::field::FIELD_PLAYER_BYTES_2)
                    || e.unit_field(benilla_protocol::field::FIELD_PLAYER_GUILDID)
                    || e.unit_field(benilla_protocol::field::FIELD_PLAYER_GUILD_TIMESTAMP)
                    || is_currency_watch(e.index)
                    || is_slot_watch(e.index)
                    || e.unit_field(benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE)
                    || is_buyback_watch(e.index)
                    || is_skill_watch(e.index))
            {
                merged.watched.push((e.index, e.old, e.new));
            }
            edges.write(e);
        });
        // The mount field's zero while passengers ride holds the copy (`0x74046f`).
        if mount_zeroed && seated.is_some_and(|q| q.iter().any(|p| p.vehicle == guid)) {
            s.0.hold_mount(copy);
        }
    } else {
        warn!(
            "net: values delta for {guid:#x} found no store on its entity — seeding from the delta"
        );
        commands.entity(entity).insert(ObjectStore(delta));
    }
    merged.death.sort_by_key(|&(index, ..)| index);
    merged.watched.sort_by_key(|&(index, ..)| index);
    merged
}

/// `UNIT_NPC_FLAG_QUESTGIVER`, the bit the NPC-flag watch tests (`0x71a104`).
const NPC_FLAG_QUESTGIVER: u32 = 0x2;

/// `UNIT_NPC_FLAG_FLIGHTMASTER`, the bit the NPC-flag watch's taxi leg tests (`0x71a13b`).
const NPC_FLAG_FLIGHTMASTER: u32 = 0x2000;

/// `UNIT_NPC_FLAG_TRAINER`, the bit the NPC-flag watch's trainer leg tests (`0x71a172`).
const NPC_FLAG_TRAINER: u32 = 0x10;

/// `UNIT_NPC_FLAG_BANKER`, the bit the NPC-flag watch's bank leg tests (`0x71a1a8`).
const NPC_FLAG_BANKER: u32 = 0x20000;

/// `UNIT_NPC_FLAG_GUILD_BANKER`, the bit the NPC-flag watch's guild bank leg tests (`0x71a1eb`).
const NPC_FLAG_GUILD_BANKER: u32 = 0x80_0000;

/// A guid field's old halves, low then high, where each moved.
type GuidHalves = (Option<u32>, Option<u32>);

/// What [`merge_fields`] saw.
#[derive(Default)]
struct Merged {
    flags_moved: bool,
    /// Our own `UNIT_FIELD_DISPLAYID` moved (`0x716860`).
    display_moved: bool,
    /// Our own `UNIT_FIELD_LEVEL` moved (`0x72e40c` -> `0x6d66e0`).
    level_moved: bool,
    /// Our own `UNIT_FIELD_CHARM` or `UNIT_FIELD_SUMMON` moved (`0x6d1970`).
    pet_moved: bool,
    /// Our own `PLAYER_FARSIGHT` moved (`0x6e4fd0`).
    farsight_moved: bool,
    /// Our own `UNIT_FIELD_CRITTER` moved.
    critter_moved: bool,
    /// Our critter guid's old halves, low then high, where they moved.
    critter_old: GuidHalves,
    /// The pet frame's unit signals the delta's fields raise, in field order.
    pet_signals: Vec<PetSignal>,
    /// Our own death fields' `(index, old, new)`, in field order.
    death: Vec<(u16, u32, u32)>,
    /// Our own quest-log slots' fields' `(index, old, new)`.
    quest_slots: Vec<(u16, u32, u32)>,
    /// A unit's `UNIT_NPC_FLAGS` `(old, new)` when it moved (`0x71a0b0`).
    npc_flags: Option<(u32, u32)>,
    /// An item's stack count moved (`0x707720`).
    item_stack: bool,
    /// Our override spell id moved (`0x6d1c30`).
    override_moved: bool,
    /// An item's spell charges moved (`0x707880`).
    item_charges: bool,
    /// An item's entry moved (`0x707960`), from this old entry.
    item_entry: Option<u32>,
    /// An item's enchant fields moved (`0x707f50`).
    item_enchants: bool,
    /// A container's own slots whose fields moved, each once in field order (`0x5d9600`).
    /// Each moved bag slot with its guid halves' old values, low then high, where they moved.
    bag_slots: Vec<(u8, GuidHalves)>,
    /// A unit's entry moved (`0x72ceb0`).
    unit_entry: bool,
    /// Our own `PLAYER_FLAGS` `(old, new)` when it moved.
    pvp_flags: Option<(u32, u32)>,
    /// Our own bank, honor and arena-team watches' `(index, old, new)`, in field order.
    watched: Vec<(u16, u32, u32)>,
    /// The unit died (`0x729220`).
    died: bool,
    /// Our own shapeshift form byte, when it moved.
    form: Option<u8>,
    /// Another player's `PLAYER_GUILDID` moved (`0x6e1bb0`).
    guild_moved: bool,
    /// Another player's `PLAYER_GUILD_TIMESTAMP` moved to this stamp (`0x6e1c60`).
    guild_stamp: Option<u32>,
    /// How many of our own main-hand and ranged visible items' entries moved (`0x6e1e10`, slots
    /// 15 and 17).
    weapons_moved: u8,
    /// How many usable updates our own fields' watches run ([`usable_updates`]).
    usable_updates: u8,
    /// How many bar refreshes (`0x5aa470`) our own visible-item and ammo watches run.
    bar_updates: u8,
    /// A unit's `UNIT_FIELD_AURASTATE` moved (`0x716810`).
    aura_state_moved: bool,
    /// A unit's `UNIT_FIELD_POWER1..7` moved.
    power_moved: bool,
}

/// The form watch's tracking leg (`0x6da130`): with nothing tracked, the first known tracking spell
/// usable in `form` (bit `form - 1` of its `Stances`, `0x7fe850`) is cast.
fn track_in_form(world: &mut World, form: u8) {
    let bit = u64::from(form) - 1;
    let allowed: std::collections::HashSet<u32> = {
        let (Some(spells), Some(book)) = (
            world.get_resource::<crate::ui_action::Spells>(),
            world.get_resource::<crate::ui_spellbook::SpellBook>(),
        ) else {
            return;
        };
        book.live
            .tracking
            .iter()
            .map(|t| t.spell_id)
            .filter(|&id| {
                spells.catalog.get(id).is_some_and(|d| {
                    let mask = u64::from(d.stances) | (u64::from(d.stances_high) << 32);
                    bit < 64 && mask & (1 << bit) != 0
                })
            })
            .collect()
    };
    crate::ui_script::live::with_live_world(world, |script| {
        script.track_in_form(|id| allowed.contains(&id));
    });
}

/// The usable updates (`0x53cf10`) a field change on our own player runs, one per site: health
/// (`0x73f44e`); the flags' combat, control and disarm bits (`0x728f9e`, `0x728fab`, `0x73c4ea`);
/// the off-hand and ranged disarm bits of flags 2 (`0x73c756`, `0x73c7e3`); stealth among the
/// vis flags (`0x73c8a4`); the player flags' `0x1800000` (`0x6e1217`); the override spells
/// (`0x6d1c53`) and the no-reagent-cost words (`0x6cde10`).
fn usable_updates(e: &FieldChanged, own: bool) -> u8 {
    use benilla_protocol::field::{
        FIELD_PLAYER_FIELD_BYTES2, FIELD_PLAYER_FLAGS, FIELD_UNIT_BYTES_1, FIELD_UNIT_FLAGS,
        FIELD_UNIT_FLAGS_2, FIELD_UNIT_HEALTH, UNIT_END,
    };
    if !own {
        return 0;
    }
    let changed = e.old ^ e.new;
    let bit = |mask: u32| u8::from(changed & mask != 0);
    if e.unit_field(FIELD_UNIT_HEALTH) {
        1
    } else if e.unit_field(FIELD_UNIT_FLAGS) {
        bit(0x01C2_0000) + bit(0x0008_0000) + bit(0x0020_0000)
    } else if e.unit_field(FIELD_UNIT_FLAGS_2) {
        bit(0x80) + bit(0x400)
    } else if e.unit_field(FIELD_UNIT_BYTES_1) {
        bit(0x2 << 16)
    } else if e.unit_field(FIELD_PLAYER_FLAGS) {
        bit(0x0180_0000)
    } else if e.unit_field(FIELD_PLAYER_FIELD_BYTES2) {
        bit(0xffff)
    } else if (UNIT_END + 0x489..UNIT_END + 0x48c).contains(&e.index) && e.is_unit() {
        1
    } else {
        0
    }
}

/// An item's entry moved (`0x707960`), on an item we own (`0x70799f`-`0x7079a5`): the old record dropped (`0x7079bb`), the new one asked with the record callback, run now when cached (`0x707a07`), then `QUEST_LOG_UPDATE` (`0x707a14`).
fn item_entry_watch(sc: &mut Scene, guid: u64, old: u32) {
    let Some(item) = sc.index.0.get(&guid).and_then(|&e| sc.stores.get(e).ok()) else {
        return;
    };
    if sc.self_guid.0.is_none() || item.0.item_owner() != sc.self_guid.0 {
        return;
    }
    let entry = item.0.object_entry().unwrap_or(0);
    // The bar's item update for the old entry (`0x7079ad`), before its record is dropped.
    sc.ui
        .world(move |world| crate::ui_action::slots::update_item(world, old));
    sc.items.evict_template(old);
    if sc.items.template(entry, guid, &sc.net).is_some() {
        sc.ui
            .world(move |world| crate::ui_items::watch::entry_record(world, guid));
    } else {
        sc.record_waits
            .wait(entry, crate::items::RecordWait::Entry(guid), false);
    }
    sc.signals.unique_signal("QUEST_LOG_UPDATE");
}

/// An item's stack count past its create, on the quest log: its test of the logged quests' required items (`0x707820` -> `0x5e1250`), then the currency leg.
fn item_quest_watch(sc: &mut Scene, guid: u64) {
    let Some(item) = sc.index.0.get(&guid).and_then(|&e| sc.stores.get(e).ok()) else {
        return;
    };
    let me = sc
        .self_guid
        .0
        .and_then(|g| sc.index.0.get(&g))
        .and_then(|&e| sc.stores.get(e).ok());
    // The quest legs read the log's templates; the currency leg below does not (`0x70782c`).
    if let (Some(rows), Some(log)) = (sc.quest_rows.as_deref(), sc.quest_log.as_deref()) {
        crate::ui_quest_log::rows::item_moved(
            item.0.object_entry().unwrap_or(0),
            me.is_some(),
            log,
            rows,
            &sc.net,
            &mut sc.signals,
        );
    }
    // The currency leg past the quest log's (`0x70782c`).
    let class = item
        .0
        .object_entry()
        .and_then(|e| sc.items.template_cached(e))
        .map(|r| r.class);
    if crate::ui_currency::item_count_fires(item.0.item_owner(), sc.self_guid.0, class) {
        crate::ui_currency::display_update(&mut sc.ui);
    }
}

/// An item's post-init (`0x70af00`): its record asked for, and with it cached the post-init with the record runs now (`0x70af37`), else when the answer lands (`0x70ae90`).
fn item_post_init(sc: &mut Scene, guid: u64, (entry, owner): (u32, Option<u64>)) {
    if entry == 0 {
        return;
    }
    if sc.items.template(entry, guid, &sc.net).is_some() {
        item_record_watch(sc, guid, (entry, owner));
    } else {
        sc.record_waits
            .wait(entry, crate::items::RecordWait::PostInit(guid), false);
    }
}

/// The post-init with the record (`0x70aa40`): in the world (`0x70ad08`) on an item we own (`0x70ad20`-`0x70ad2f`), its unlock (`0x70ad8d`, `0x70ad9e`), the stack watch's walk (`0x70adfc` -> `0x5e1250`), its container's `BAG_UPDATE` (`0x70ae0b`), then the refund ask.
fn item_record_watch(sc: &mut Scene, item: u64, (entry, owner): (u32, Option<u64>)) {
    let me = sc.self_guid.0.filter(|g| sc.index.0.contains_key(g));
    if me.is_none() || owner != me {
        return;
    }
    sc.ui
        .world(move |world| crate::ui_items::watch::post_init_unlock(world, item));
    item_log_walk(sc, entry, true);
    sc.ui
        .world(move |world| crate::ui_items::watch::item_updated(world, item));
    // A refundable purchase without a record or a pending ask asks it (`0x70ae13`-`0x70ae34`).
    if crate::ui_socket::refundable(&sc.items, entry) {
        if let Some(refunds) = sc.refunds.as_deref_mut() {
            refunds.ask(item, &sc.net);
        }
    }
}

/// An item's disable on a destroy or out-of-range (`0x70afc0`): on an item we own (`0x70b0b5`-`0x70b0be`), the stack watch's walk (`0x70b0eb` -> `0x5e1250`).
fn item_disable_watch(sc: &mut Scene, guid: u64) {
    if !guid::is_item(guid) {
        return;
    }
    let Some(item) = sc.index.0.get(&guid).and_then(|&e| sc.stores.get(e).ok()) else {
        return;
    };
    let (entry, owner) = (item.0.object_entry().unwrap_or(0), item.0.item_owner());
    if owner.is_none() || owner != sc.self_guid.0 {
        return;
    }
    // The bar's item update (`0x70b0d0`) before the walk.
    sc.ui
        .world(move |world| crate::ui_action::slots::update_item(world, entry));
    let me = sc.self_guid.0.is_some_and(|g| sc.index.0.contains_key(&g));
    item_log_walk(sc, entry, me);
}

/// The stack watch's walk over the logged quests for `entry` ([`crate::ui_quest_log::rows::item_moved`]).
fn item_log_walk(sc: &mut Scene, entry: u32, me: bool) {
    let (Some(log), Some(rows)) = (sc.quest_log.as_deref(), sc.quest_rows.as_deref()) else {
        return;
    };
    crate::ui_quest_log::rows::item_moved(entry, me, log, rows, &sc.net, &mut sc.signals);
}

/// The slots `fields` moved, in slot order, each slot's `(quest id, state)` before and after.
fn quest_slot_edges(
    fields: &[(u16, u32, u32)],
    store: &benilla_protocol::ObjectFields,
) -> Vec<crate::ui_quest_log::rows::SlotEdge> {
    use benilla_protocol::messages::QUEST_LOG_SLOT_FIELDS;
    let base = benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1;
    let mut slots: Vec<u8> = fields
        .iter()
        .map(|&(index, ..)| ((index - base) / QUEST_LOG_SLOT_FIELDS) as u8)
        .collect();
    slots.sort_unstable();
    slots.dedup();
    slots
        .into_iter()
        .map(|slot| {
            let at = base + u16::from(slot) * QUEST_LOG_SLOT_FIELDS;
            let now = store.player_quest_log(slot);
            let now_id = now.map_or(0, |s| s.quest_id);
            let now_state = now.map_or(0, |s| u32::from(s.state));
            let old = |index: u16, now: u32| {
                fields
                    .iter()
                    .find(|&&(i, ..)| i == index)
                    .map_or(now, |&(_, old, _)| old)
            };
            crate::ui_quest_log::rows::SlotEdge {
                was: (old(at, now_id), old(at + 1, now_state) as u8),
                now: (now_id, now_state as u8),
            }
        })
        .collect()
}

/// Every quest-log slot's fields: the slot watch's range (`0x6e52c5`-`0x6e52fb`).
const QUEST_LOG_FIELDS: std::ops::Range<u16> = benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1
    ..benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1
        + benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS as u16
            * benilla_protocol::messages::QUEST_LOG_SLOT_FIELDS;

/// Writes one slot of a mover's [`UnitSpeeds`]; `false` when there is none (an untracked guid,
/// or a create with no movement block). vmangos sends a self create and a speed change back to
/// back (`HandleMoveWorldportAckOpcode`), and wire order holds as each handler applies in turn.
fn set_speed(
    guid: u64,
    kind: SpeedKind,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
) -> bool {
    let Some(mut s) = index.0.get(&guid).and_then(|&e| speeds.get_mut(e).ok()) else {
        return false;
    };
    let s = &mut s.0;
    match kind {
        SpeedKind::Walk => s.walk = speed,
        SpeedKind::Run => s.run = speed,
        SpeedKind::RunBack => s.run_back = speed,
        SpeedKind::Swim => s.swim = speed,
        SpeedKind::SwimBack => s.swim_back = speed,
        SpeedKind::TurnRate => s.turn_rate = speed,
        SpeedKind::Flight => s.flight = speed,
        SpeedKind::FlightBack => s.flight_back = speed,
        SpeedKind::PitchRate => s.pitch_rate = speed,
    }
    true
}

/// `SMSG_FORCE_*_SPEED_CHANGE`: applied to the unit's speeds (`0x741b00` resolves any unit) and
/// forwarded to the controller, which acks the one it moves: our own body or a possessed unit.
fn force_speed_change(
    guid: u64,
    kind: SpeedKind,
    counter: u32,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
    self_guid: &SelfGuid,
    speed_changes: &mut MessageWriter<SpeedChangeMessage>,
    arrival: u64,
) {
    let applied = set_speed(guid, kind, speed, index, speeds);
    if self_guid.0 == Some(guid) {
        info!("net: force {kind:?} speed change -> {speed} yd/s (counter {counter})");
        // Should be unreachable; the ack still goes, so it warns.
        if !applied {
            warn!(
                "net: force {kind:?} speed change for OUR mover {guid:#x} landed nowhere \
                 (no live speed set) — we still ack, so the server and we now disagree"
            );
        }
    } else {
        debug!("net: force {kind:?} speed change for {guid:#x} -> {speed} — applied {applied}");
    }
    speed_changes.write(SpeedChangeMessage {
        guid,
        kind,
        counter,
        speed,
        arrival,
    });
}

/// The SPLINE_SET and MOVE_SET families: an observed unit's speed, no ack. MOVE_SET's pose
/// arrives as its own `UnitMove`.
fn speed_changed(
    guid: u64,
    kind: SpeedKind,
    speed: f32,
    index: &GuidIndex,
    speeds: &mut Query<&mut UnitSpeeds>,
) {
    if !set_speed(guid, kind, speed, index, speeds) {
        // A speed broadcast can outrun the create block, which carries the same set.
        debug!("net: {kind:?} speed change for untracked mover {guid:#x} — nothing to write");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The teardown is the disable: it releases the object's effect list (`0x7355a7`).
    #[test]
    fn a_teardown_releases_the_effect_list() {
        use crate::creature_anim::{EffectListClear, UnitEffectClear};
        let mut world = World::new();
        world.init_resource::<bevy::ecs::message::Messages<UnitEffectClear>>();
        let e = world.spawn(Guid(7)).id();
        tear_down(world.entity_mut(e));
        let sent: Vec<UnitEffectClear> = world
            .resource_mut::<bevy::ecs::message::Messages<UnitEffectClear>>()
            .drain()
            .collect();
        assert_eq!(
            sent,
            vec![UnitEffectClear {
                entity: e,
                mode: EffectListClear::Release,
            }]
        );
    }

    /// `0x1EB` for an unheld item waits on the active player and replays at its create; `0x1EA`
    /// for one is dropped; the destroy takes the cells.
    #[test]
    fn item_countdowns_live_on_the_item_and_a_queued_enchant_waits_for_it() {
        use crate::items::Countdowns;
        const ME: u64 = 0x0000_0000_0000_0007;
        const SWORD: u64 = 0x4000_0000_0000_0042;
        const STONE: u64 = 0x4000_0000_0000_0043;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let create = |guid| SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_OBJECT_ENTRY, 117)]),
        };
        let cells = |world: &World, guid: u64| -> Option<Countdowns> {
            let e = *world.resource::<GuidIndex>().0.get(&guid)?;
            world.get::<Countdowns>(e).cloned()
        };

        // No active player: the enchant update has nowhere to wait.
        super::super::handlers::dispatch(
            world,
            vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: STONE,
                    slot: 1,
                    seconds: 60,
                },
                create(STONE),
            ],
        );
        assert_eq!(
            cells(world, STONE),
            Some(Countdowns::default()),
            "dropped, not queued"
        );

        // With the active player resolving, the enchant waits for the item; the lifetime does not.
        let me = world.spawn_empty().id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        super::super::handlers::dispatch(
            world,
            vec![
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 1,
                    seconds: 90,
                },
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
            ],
        );
        assert_eq!(cells(world, SWORD), None, "nothing held yet");
        super::super::handlers::dispatch(world, vec![create(SWORD)]);
        let c = cells(world, SWORD).expect("the create spawned the item with its cells");
        assert!(
            c.enchant_remaining_ms(1)
                .is_some_and(|ms| (89_000..=90_000).contains(&ms)),
            "the queued enchant replayed as of the create"
        );
        assert_eq!(
            c.lifetime_remaining_ms(),
            None,
            "`0x1EA` for an unheld item has no queue"
        );

        // Held now: both arms write the cells directly; a slot past the array writes nothing.
        super::super::handlers::dispatch(
            world,
            vec![
                SessionEvent::ItemTime {
                    item_guid: SWORD,
                    seconds: 600,
                },
                SessionEvent::ItemEnchantTime {
                    item_guid: SWORD,
                    slot: 9,
                    seconds: 30,
                },
            ],
        );
        let c = cells(world, SWORD).unwrap();
        assert!(c.lifetime_remaining_ms().is_some());
        assert!(c.enchant_remaining_ms(1).is_some());

        // The destroy takes the object and its cells with it.
        let e = world.resource::<GuidIndex>().0[&SWORD];
        super::super::handlers::dispatch(world, vec![SessionEvent::ObjectDestroyed(SWORD)]);
        assert!(!world.resource::<GuidIndex>().0.contains_key(&SWORD));
        assert!(world.get_entity(e).is_err(), "the cells died with the item");
    }

    /// The observer movement-mode family, on a bare `World` with one indexed unit.
    mod spline_modes {
        use super::*;
        use benilla_protocol::SplineMode;
        use bevy::ecs::system::RunSystemOnce;

        const MOB: u64 = 0x0000_0000_0000_00F0;

        /// A world holding one indexed unit, optionally already dead-reckoning as a relayed player.
        fn world_with_unit(remote: Option<RemoteMotion>) -> (World, Entity) {
            let mut w = World::new();
            let mut e = w.spawn_empty();
            if let Some(rm) = remote {
                e.insert(rm);
            }
            let entity = e.id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, entity);
            w.insert_resource(index);
            (w, entity)
        }

        fn remote_walking_forward() -> RemoteMotion {
            RemoteMotion {
                wow_pos: [0.0; 3],
                pending: std::collections::VecDeque::new(),
                orientation: 0.0,
                flags: crate::creature_anim::move_flags::FORWARD,
                pitch: 0.0,
                speed: 7.0,
                vertical_velocity: 0.0,
                jump_xy_vel: [0.0; 2],
                fall_start_z: None,
                relay: Default::default(),
                last_apply_ms: 0.0,
                last_apply_pos: [0.0; 3],
            }
        }

        /// Runs one grant and flushes its commands.
        fn grant(w: &mut World, mode: SplineMode, apply: bool) {
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut modes: Query<&mut UnitMoveModes>,
                      mut remote: Query<&mut RemoteMotion>| {
                    spline_move_mode(
                        MOB,
                        mode,
                        apply,
                        &mut commands,
                        &index,
                        &mut modes,
                        &mut remote,
                    );
                },
            )
            .unwrap();
        }

        /// `SET_FLYING` takes only on a unit that may fly (`0x989890`), ending its fall;
        /// `UNSET_FLYING` drops flight and pitch whatever it held; gravity off stops a fall.
        #[test]
        fn flight_needs_can_fly_and_gravity_off_stops_the_fall() {
            use crate::creature_anim::move_flags as f;
            let falling = RemoteMotion {
                flags: f::FALLING | f::FORWARD,
                vertical_velocity: -5.0,
                ..remote_walking_forward()
            };
            let (mut w, e) = world_with_unit(Some(falling.clone()));
            grant(&mut w, SplineMode::Flying, true);
            let rm = w.entity(e).get::<RemoteMotion>().unwrap();
            assert_eq!(rm.flags & f::FLYING, 0, "no CAN_FLY, no flight");

            let (mut w, e) = world_with_unit(Some(RemoteMotion {
                flags: falling.flags | f::CAN_FLY,
                ..falling.clone()
            }));
            grant(&mut w, SplineMode::Flying, true);
            let rm = w.entity(e).get::<RemoteMotion>().unwrap();
            assert_eq!(
                rm.flags & (f::FLYING | f::FALLING),
                f::FLYING,
                "flying, not falling"
            );
            assert_eq!(rm.vertical_velocity, 0.0);
            w.entity_mut(e).get_mut::<RemoteMotion>().unwrap().flags |= 0x40 | f::ASCENDING;
            grant(&mut w, SplineMode::Flying, false);
            let rm = w.entity(e).get::<RemoteMotion>().unwrap();
            assert_eq!(rm.flags & (f::FLYING | f::ASCENDING | 0x40), 0);
            assert_ne!(rm.flags & f::CAN_FLY, 0, "it may still fly");

            let (mut w, e) = world_with_unit(Some(falling));
            grant(&mut w, SplineMode::DisableGravity, true);
            let rm = w.entity(e).get::<RemoteMotion>().unwrap();
            assert_eq!(rm.flags & f::FALLING, 0, "the fall stops");
            assert_eq!(rm.vertical_velocity, 0.0);
            assert_ne!(
                w.entity(e).get::<UnitMoveModes>().unwrap().0 & f::LEVITATING,
                0,
                "and the unit keeps gravity off"
            );
        }

        /// With no relay state flight writes the kept word, under the `CAN_FLY` gate (`0x989890`).
        #[test]
        fn flight_without_relay_state_writes_the_kept_word() {
            use crate::creature_anim::move_flags as f;
            let (mut w, e) = world_with_unit(None);
            grant(&mut w, SplineMode::Flying, true);
            let word = w.entity(e).get::<UnitMoveModes>().unwrap().0;
            assert_eq!(word & f::FLYING, 0, "no CAN_FLY, no flight");

            w.entity_mut(e)
                .insert(UnitMoveModes(f::CAN_FLY | f::FALLING));
            grant(&mut w, SplineMode::Flying, true);
            let word = w.entity(e).get::<UnitMoveModes>().unwrap().0;
            assert_eq!(word, f::CAN_FLY | f::FLYING, "flying, not falling");
            grant(&mut w, SplineMode::Flying, false);
            let word = w.entity(e).get::<UnitMoveModes>().unwrap().0;
            assert_eq!(word, f::CAN_FLY, "it may still fly");
        }

        /// A path keeps the standing word; the relay that seeds again takes it back.
        #[test]
        fn a_path_keeps_the_word_and_the_next_relay_takes_it_back() {
            use crate::creature_anim::move_flags as f;
            let hovering = RemoteMotion {
                flags: f::FLYING | f::CAN_FLY | f::FORWARD | f::ASCENDING,
                ..remote_walking_forward()
            };
            let (mut w, e) = world_with_unit(Some(hovering));
            w.entity_mut(e).insert(UnitMoveModes(f::WATER_WALKING));
            w.run_system_once(
                move |mut commands: Commands,
                      remote: Query<&mut RemoteMotion>,
                      mut modes: Query<&mut UnitMoveModes>| {
                    path_keeps_word(e, &remote, &mut modes, &mut commands);
                },
            )
            .unwrap();
            assert_eq!(
                w.entity(e).get::<UnitMoveModes>().unwrap().0,
                f::WATER_WALKING | f::FLYING | f::CAN_FLY,
                "the flight outlives the path; the motion does not"
            );

            let take = move |w: &mut World| {
                w.run_system_once(
                    move |remote: Query<&mut RemoteMotion>,
                          mut modes: Query<&mut UnitMoveModes>| {
                        relay_takes_word(e, &remote, &mut modes);
                    },
                )
                .unwrap();
            };
            take(&mut w);
            assert_eq!(
                w.entity(e).get::<UnitMoveModes>().unwrap().0,
                f::WATER_WALKING | f::FLYING | f::CAN_FLY,
                "a unit still dead-reckoning is not re-seeded"
            );
            w.entity_mut(e).remove::<RemoteMotion>();
            take(&mut w);
            assert_eq!(
                w.entity(e).get::<UnitMoveModes>().unwrap().0,
                0,
                "the seeding relay's word is the whole word"
            );
        }

        /// A block replaces a unit's kept word with its standing bits (`0x6f1520`), a creature's
        /// as a player's.
        #[test]
        fn a_block_seeds_a_units_word_and_drops_its_motion() {
            use crate::creature_anim::move_flags as f;
            let mover = MoverState {
                flags: f::FLYING | f::CAN_FLY | f::FORWARD | f::FALLING,
                pitch: 0.0,
                flags2: 0,
            };
            let mut w = World::new();
            let player = w.spawn(UnitMoveModes(f::ROOT)).id();
            let mob = w.spawn_empty().id();
            w.run_system_once(move |mut commands: Commands| {
                seed_unit_word(player, mover, &mut commands);
                seed_unit_word(mob, mover, &mut commands);
            })
            .unwrap();
            for e in [player, mob] {
                assert_eq!(
                    w.entity(e).get::<UnitMoveModes>().unwrap().0,
                    f::FLYING | f::CAN_FLY
                );
            }
        }

        /// A gravity-off creature streamed in reaches the selector with its word, the bits its
        /// Hover idle reads (`0x71c6c0`).
        #[test]
        fn a_gravity_off_creature_block_reaches_the_selector() {
            use crate::creature_anim::move_flags as f;
            let mover = MoverState {
                flags: f::LEVITATING | f::CAN_FLY | f::FORWARD,
                pitch: 0.0,
                flags2: 0,
            };
            let mut w = World::new();
            let mob = w.spawn_empty().id();
            w.run_system_once(move |mut commands: Commands| {
                seed_unit_word(mob, mover, &mut commands);
            })
            .unwrap();
            let modes = w.entity(mob).get::<UnitMoveModes>().copied();
            let mv = crate::creature_anim::select::unify(None, None, None, false, modes.as_ref());
            assert_eq!(mv.flags, f::LEVITATING | f::CAN_FLY);
        }

        /// Every one of the twelve 1.12.1-shaped opcodes sets or clears one bit.
        #[test]
        fn a_grant_sets_the_bit_and_its_pair_clears_it() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(None);

            grant(&mut w, SplineMode::Hover, true);
            grant(&mut w, SplineMode::WaterWalk, true);
            let m = *w
                .entity(e)
                .get::<UnitMoveModes>()
                .expect("first grant inserts");
            assert_eq!(
                m.0,
                move_flags::HOVER | move_flags::WATER_WALKING,
                "two grants accumulate in one word — the reference has one flags word"
            );

            grant(&mut w, SplineMode::Hover, false);
            let m = *w.entity(e).get::<UnitMoveModes>().unwrap();
            assert_eq!(
                m.0,
                move_flags::WATER_WALKING,
                "UNSET_HOVER takes back hover and nothing else"
            );
        }

        /// `SET_RUN_MODE` clears the walk bit: the pair is inverted against its names
        /// (`SetRunMode`), folded at the parse.
        #[test]
        fn the_walk_bit_round_trips_with_the_run_opcodes_sense() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(None);
            grant(&mut w, SplineMode::WalkMode, true); // SMSG_SPLINE_MOVE_SET_WALK_MODE
            assert_eq!(
                w.entity(e).get::<UnitMoveModes>().unwrap().0,
                move_flags::WALK_MODE
            );
            grant(&mut w, SplineMode::WalkMode, false); // SMSG_SPLINE_MOVE_SET_RUN_MODE
            assert_eq!(w.entity(e).get::<UnitMoveModes>().unwrap().0, 0);
        }

        /// The root apply wipes the direction bits, so the unit fails the integration gate
        /// (`test [esi+0x40], 0x20ff`) and is not stepped.
        #[test]
        fn a_root_wipes_the_direction_bits_the_dead_reckon_runs_on() {
            use crate::creature_anim::move_flags;
            let (mut w, e) = world_with_unit(Some(remote_walking_forward()));

            grant(&mut w, SplineMode::Root, true);

            let rm = w.entity(e).get::<RemoteMotion>().unwrap();
            assert_eq!(
                rm.flags & move_flags::INTEGRATED,
                0,
                "nothing the integration gate tests may survive the root — this is what stops it"
            );
            assert_eq!(rm.speed, 0.0, "and it is not still travelling at run speed");
            assert!(w.entity(e).get::<UnitMoveModes>().unwrap().rooted());
        }

        /// The unroot only clears the bit.
        #[test]
        fn an_unroot_clears_the_bit_and_invents_no_movement() {
            let (mut w, e) = world_with_unit(Some(remote_walking_forward()));
            grant(&mut w, SplineMode::Root, true);
            grant(&mut w, SplineMode::Root, false);
            assert!(!w.entity(e).get::<UnitMoveModes>().unwrap().rooted());
            assert_eq!(
                w.entity(e).get::<RemoteMotion>().unwrap().flags,
                0,
                "still no direction bits — the wipe is one-shot and the unroot does not undo it"
            );
        }

        /// As in the reference's handler; the create carries the modes itself.
        #[test]
        fn a_grant_for_a_unit_we_do_not_have_is_dropped() {
            let (mut w, e) = world_with_unit(None);
            w.run_system_once(
                |mut commands: Commands,
                 index: Res<GuidIndex>,
                 mut modes: Query<&mut UnitMoveModes>,
                 mut remote: Query<&mut RemoteMotion>| {
                    spline_move_mode(
                        0xDEAD_BEEF,
                        SplineMode::Root,
                        true,
                        &mut commands,
                        &index,
                        &mut modes,
                        &mut remote,
                    );
                },
            )
            .unwrap();
            assert!(w.entity(e).get::<UnitMoveModes>().is_none());
        }

        /// The reference's server-position apply returns on ROOT, so a queued path does not move a
        /// pinned body.
        #[test]
        fn a_rooted_unit_refuses_the_path_the_server_queued() {
            let mut w = World::new();
            let e = w
                .spawn((
                    Transform::default(),
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                ))
                .id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, e);
            w.insert_resource(index);

            let path = vec![[0.0, 0.0, 0.0], [30.0, 0.0, 0.0]];
            for (rooted, expect_spline) in [(true, false), (false, true)] {
                let path = path.clone();
                w.run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          mut transforms: Query<&mut Transform>,
                          mut riders: Query<&mut crate::transport::TransportRider>| {
                        monster_move(
                            MOB,
                            None,
                            -1,
                            [0.0, 0.0, 0.0],
                            7,
                            path.clone(),
                            MonsterMoveFacing::None,
                            false,
                            3000,
                            false,
                            true,
                            None,
                            false,
                            None,
                            rooted,
                            &mut commands,
                            &index,
                            &mut transforms,
                            &mut riders,
                        );
                    },
                )
                .unwrap();
                assert_eq!(
                    w.entity(e).get::<Spline>().is_some(),
                    expect_spline,
                    "rooted={rooted}: a pinned body takes no path, an unpinned one does"
                );
            }
        }

        /// A plain path takes the unit off the deck, where vmangos drops the passenger
        /// (`MoveSplineInit.cpp:156-159`).
        #[test]
        fn a_transport_path_attaches_the_rider_and_a_plain_one_lets_it_go() {
            const BOAT: u64 = 0x2000_0000_0000_0007;
            let mut w = World::new();
            let e = w
                .spawn((
                    Transform::default(),
                    NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                ))
                .id();
            let boat = w.spawn(Transform::default()).id();
            let mut index = GuidIndex::default();
            index.0.insert(MOB, e);
            index.0.insert(BOAT, boat);
            w.insert_resource(index);

            let deck_path = vec![[1.5, -2.5, 0.75], [4.0, -2.5, 0.75]];
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut transforms: Query<&mut Transform>,
                      mut riders: Query<&mut crate::transport::TransportRider>| {
                    monster_move(
                        MOB,
                        Some(BOAT),
                        -1,
                        [1.5, -2.5, 0.75],
                        7,
                        deck_path.clone(),
                        MonsterMoveFacing::Angle(1.25),
                        false,
                        3000,
                        false,
                        true,
                        None,
                        false,
                        None,
                        false,
                        &mut commands,
                        &index,
                        &mut transforms,
                        &mut riders,
                    );
                },
            )
            .unwrap();
            let rider = w
                .entity(e)
                .get::<crate::transport::TransportRider>()
                .expect("a transport path attaches its rider");
            assert_eq!(rider.transport_guid, BOAT);
            assert_eq!(rider.local_pos, [1.5, -2.5, 0.75], "seeded deck-local");
            assert!(
                (rider.local_orientation - 1.25).abs() < 1e-6,
                "Angle is deck-local"
            );
            assert_eq!(
                w.entity(e).get::<Spline>().expect("a path").deck,
                Some(BOAT),
                "the spline rides the deck frame, not the world"
            );
            // The deck-local start is never written as a world position.
            assert_eq!(
                w.entity(e).get::<Transform>().unwrap().translation,
                Vec3::ZERO
            );

            let world_path = vec![[100.0, 200.0, 30.0], [110.0, 200.0, 30.0]];
            w.run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      mut transforms: Query<&mut Transform>,
                      mut riders: Query<&mut crate::transport::TransportRider>| {
                    monster_move(
                        MOB,
                        None,
                        -1,
                        [100.0, 200.0, 30.0],
                        8,
                        world_path.clone(),
                        MonsterMoveFacing::None,
                        false,
                        3000,
                        false,
                        true,
                        None,
                        false,
                        None,
                        false,
                        &mut commands,
                        &index,
                        &mut transforms,
                        &mut riders,
                    );
                },
            )
            .unwrap();
            assert!(
                w.entity(e)
                    .get::<crate::transport::TransportRider>()
                    .is_none(),
                "a plain path takes the unit off the deck"
            );
            assert_eq!(w.entity(e).get::<Spline>().expect("a path").deck, None);
        }
    }
}

#[cfg(test)]
mod relay_tests;
#[cfg(test)]
mod update_tests;
