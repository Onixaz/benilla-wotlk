//! Vehicles: a unit's vehicle (the reference's `CVehicle_C`, `0x757fa0`), its seats, and what
//! the interface hears of them.
//!
//! A unit gains its vehicle from the create block's `UPDATEFLAG_VEHICLE` (`0x73c260`) or from
//! `SMSG_PLAYER_VEHICLE_DATA` (`0x716db0`), which takes it away with id 0. Our own player's gain
//! and loss fire `PLAYER_GAINS_VEHICLE_DATA` (`0x74bbd0`) and `PLAYER_LOSES_VEHICLE_DATA`
//! (`0x74bc50`); a create-block vehicle is built before the object's post-init (`0x744a50` sets
//! the gate bit `0x40000`), so only the packet's fires, and world entry fires it again for a
//! player already holding one (`0x757180`).

use std::collections::HashMap;
use std::sync::Arc;

mod control;
mod override_bar;
mod passenger;
mod ride;
mod seats;

pub(crate) use passenger::{Passenger, SEATED};
pub(crate) use ride::SeatPose;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::{VehicleCatalog, VehicleRec, VehicleSeatRec};
use benilla_protocol::messages::{opcode, ObjectType};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    ScriptValue, UiScript, VehicleIndicatorView, VehicleRequest, VehicleSeatView, VehicleUnitView,
    VehicleView,
};

use crate::net::{
    ClientCommand, EnteredWorldMessage, GuidIndex, NetHandlerApp, ObjectStore, SelfGuid,
    VehicleCommand,
};
use crate::transport::TransportRider;

/// The vehicle tables, and the `SoundEntries` names the seats' enter and exit sounds have;
/// empty without client data.
#[derive(Resource, Default, Clone)]
pub(crate) struct VehicleTables {
    pub(crate) catalog: Arc<VehicleCatalog>,
    sound_names: Arc<HashMap<u32, String>>,
    /// The seat indicators as the VM reads them.
    indicators: Arc<HashMap<u32, VehicleIndicatorView>>,
}

impl VehicleTables {
    pub(crate) fn vehicle(&self, id: u32) -> Option<&VehicleRec> {
        self.catalog.vehicle(id)
    }

    /// `0x756ec0`: seat slot `slot` of vehicle `id`.
    pub(crate) fn seat(&self, id: u32, slot: usize) -> Option<&VehicleSeatRec> {
        self.catalog.vehicle_seat(self.catalog.vehicle(id)?, slot)
    }

    /// A seat sound's `SoundEntries` name, `""` for none or an unknown id.
    pub(crate) fn sound_name(&self, id: u32) -> &str {
        self.sound_names.get(&id).map_or("", String::as_str)
    }
}

/// A unit's vehicle: its `Vehicle.dbc` id and the facing its facing limits run from
/// (`m_rawFacing`, the create block's vehicle orientation, else the unit's facing at creation).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Vehicle {
    pub(crate) id: u32,
    pub(crate) raw_facing: f32,
}

/// Our seat's stopped movement (`m_flags & 0x8`), set ahead of a local seat change or dismiss
/// (`0x747fa0`): the frame-time it was set, cleared by `SMSG_ON_CANCEL_EXPECTED_RIDE_VEHICLE_AURA`
/// (`0x747900`), any state change, or 2 s after (`0x74af70`).
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub(crate) struct SeatStop {
    pub(crate) at: Option<f64>,
}

/// How long a seat's stop holds (`0x74af70`).
pub(crate) const SEAT_STOP_SECS: f64 = 2.0;

/// What the frame owes the VM.
#[derive(Resource, Default)]
pub(crate) struct VehicleSignals {
    /// `(unit, gained)`: our player gained (`Some(indicator)`) or lost (`None`) its vehicle.
    data: Vec<(u64, Option<u32>)>,
    /// World entry's slider reset (`0x757180` → `0x74c5a0` with no vehicle): both shows, empty.
    sliders_hidden: bool,
    /// Passenger transitions of our player and group members, in order.
    transitions: Vec<passenger::Transition>,
    /// A transition touched our own vehicle chain (`VEHICLE_PASSENGERS_CHANGED`).
    passengers_changed: bool,
}

pub(crate) struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VehicleTables>()
            .init_resource::<control::VehicleAim>()
            .init_resource::<SeatStop>()
            .init_resource::<VehicleSignals>()
            .add_message::<EnteredWorldMessage>()
            // After the object handlers, whose commands spawn the unit this reads.
            .net_handler(SessionEventKind::ObjectCreate, on_object_create)
            .net_handler(SessionEventKind::PlayerVehicleData, on_player_vehicle_data)
            .net_handler(
                SessionEventKind::CancelExpectedRideVehicleAura,
                on_cancel_expected_ride,
            )
            .add_systems(Startup, load_vehicle_tables.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    on_world_enter.after(benilla_world::schedule::WorldStage::Net),
                    passenger::track_passengers
                        .after(benilla_world::schedule::WorldStage::Net)
                        .after(on_world_enter),
                    feed_vehicle_view
                        .in_set(crate::ui_unit::UnitFeed)
                        .after(passenger::track_passengers),
                    feed_vehicle_signals
                        .in_set(crate::ui_unit::UnitFeed)
                        .after(feed_vehicle_view)
                        .after(crate::ui_unit::EnterWorldFeed)
                        .after(on_world_enter)
                        .after(passenger::track_passengers),
                    send_vehicle_requests
                        .in_set(crate::ui_unit::UnitFeed)
                        .after(feed_vehicle_signals),
                    // After the passenger events, as the mover's claim follows the seat.
                    control::on_mover_change
                        .in_set(crate::ui_unit::UnitFeed)
                        .after(feed_vehicle_signals)
                        .before(send_vehicle_requests),
                    // Before the slot feed, which shows the slots it writes.
                    override_bar::update_override_bar
                        .in_set(crate::ui_unit::UnitFeed)
                        .after(crate::ui_action::ActionRanks)
                        .after(crate::ui_pet::PetTempBar)
                        .before(crate::ui_action::SlotIdentityFeed),
                    // With the entity visuals, after the rigs are built and any mount re-seat.
                    (ride::ride_seats, ride::sweep_seat_anchors)
                        .chain()
                        .in_set(crate::entities::EntityVisualsSet)
                        .after(crate::entities::MountReseatSet)
                        .after(passenger::track_passengers),
                ),
            );
    }
}

fn load_vehicle_tables(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let (loaded, sounds) = {
        let mut chain = assets.chain.lock_recover();
        (
            benilla_formats::load_vehicles(&mut chain),
            benilla_formats::load_sound_kit_catalog(&mut chain),
        )
    };
    let catalog = match loaded {
        Ok(catalog) => catalog,
        Err(e) => {
            warn!("vehicle: the vehicle tables failed — no unit holds a vehicle: {e:#}");
            return;
        }
    };
    // Only the seats' sounds are named in an event, so only theirs are kept.
    let sound_names = match sounds {
        Ok(kits) => catalog
            .seats
            .values()
            .flat_map(|s| [s.enter_ui_sound_id, s.exit_ui_sound_id])
            .filter_map(|id| Some((id, kits.get(id)?.name.clone())))
            .collect(),
        Err(e) => {
            warn!("vehicle: SoundEntries failed — the seat events name no sound: {e:#}");
            HashMap::new()
        }
    };
    let indicators = catalog
        .indicators
        .iter()
        .map(|(&id, ind)| {
            let seats = ind
                .seats
                .iter()
                .map(|s| (s.virtual_seat_index, s.x, s.y))
                .collect();
            (
                id,
                VehicleIndicatorView {
                    texture: ind.background_texture.clone(),
                    seats,
                },
            )
        })
        .collect();
    commands.insert_resource(VehicleTables {
        catalog: Arc::new(catalog),
        sound_names: Arc::new(sound_names),
        indicators: Arc::new(indicators),
    });
}

/// The create block's vehicle (`0x73c260`), built before post-init, so it fires nothing.
fn on_object_create(In(ev): In<SessionEvent>, index: Res<GuidIndex>, mut commands: Commands) {
    let SessionEvent::ObjectCreate {
        guid,
        vehicle: Some((id, raw_facing)),
        ..
    } = ev
    else {
        return;
    };
    if let Some(&e) = index.0.get(&guid) {
        commands.entity(e).insert(Vehicle { id, raw_facing });
    }
}

/// `SMSG_PLAYER_VEHICLE_DATA` (`0x716db0`) on a held unit: a vehicle id gives it a vehicle, or
/// re-points the one it has (`0x7580f0`, no event); 0 takes it away (`0x74c7b0`).
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn on_player_vehicle_data(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    units: Query<(&ObjectStore, &Transform, Option<&Vehicle>)>,
    self_guid: Res<SelfGuid>,
    tables: Res<VehicleTables>,
    mut signals: ResMut<VehicleSignals>,
    mut commands: Commands,
) {
    let SessionEvent::PlayerVehicleData { guid, vehicle_id } = ev else {
        return;
    };
    let Some(&e) = index.0.get(&guid) else {
        return;
    };
    let Ok((store, tf, held)) = units.get(e) else {
        return;
    };
    if !store.is_unit() {
        return;
    }
    let ours = self_guid.0 == Some(guid);
    match (vehicle_id, held) {
        (0, None) => {}
        (0, Some(_)) => {
            commands.entity(e).remove::<Vehicle>();
            if ours {
                signals.data.push((guid, None));
            }
        }
        (id, Some(v)) => {
            commands.entity(e).insert(Vehicle { id, ..*v });
        }
        (id, None) => {
            let raw_facing = tf.rotation.to_euler(EulerRot::YXZ).0;
            commands.entity(e).insert(Vehicle { id, raw_facing });
            if ours {
                let indicator = tables.vehicle(id).map_or(0, |v| v.ui_indicator_id);
                signals.data.push((guid, Some(indicator)));
            }
        }
    }
}

/// World entry (`0x528010` → `0x757180`): the sliders hide, then a player already holding a
/// vehicle gains it again.
/// `SMSG_ON_CANCEL_EXPECTED_RIDE_VEHICLE_AURA` (`0x800510`): our seat's stop ends (`0x747900`).
fn on_cancel_expected_ride(In(_): In<SessionEvent>, mut stop: ResMut<SeatStop>) {
    stop.at = None;
}

fn on_world_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    vehicles: Query<&Vehicle>,
    tables: Res<VehicleTables>,
    mut signals: ResMut<VehicleSignals>,
) {
    if entered.read().count() == 0 {
        return;
    }
    signals.sliders_hidden = true;
    let Some(me) = self_guid.0 else { return };
    let Some(v) = index.0.get(&me).and_then(|&e| vehicles.get(e).ok()) else {
        return;
    };
    let indicator = tables.vehicle(v.id).map_or(0, |r| r.ui_indicator_id);
    signals.data.push((me, Some(indicator)));
}

/// The seat facts the verbs read.
fn seat_view(s: &VehicleSeatRec) -> VehicleSeatView {
    VehicleSeatView {
        flags: s.flags,
        flags_b: s.flags_b,
        skin: s.ui_skin,
    }
}

/// The units the vehicle walks read: every vehicle and every rider.
type VehicleUnits<'w, 's> = Query<
    'w,
    's,
    (
        &'static crate::net::Guid,
        &'static ObjectStore,
        Option<&'static TransportRider>,
        Option<&'static Passenger>,
        Option<&'static Vehicle>,
    ),
    Or<(With<Vehicle>, With<TransportRider>)>,
>;

/// Push each unit's vehicle facts (`0x613600`): a held passenger's seat on its vehicle and its
/// state, controlling when the vehicle is its own `UNIT_FIELD_CHARM`; an unheld group member's
/// seat by the party record's seat id; the seat trees, the mover and the player's seat flags.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn feed_vehicle_view(
    script: Option<NonSendMut<UiScript>>,
    units: VehicleUnits,
    mover: Query<&crate::net::Guid, With<crate::net::ActiveMover>>,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    group: Option<Res<crate::ui_party::GroupState>>,
    names: Option<Res<crate::names::NameCache>>,
    tables: Res<VehicleTables>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<VehicleView>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let me = self_guid.0.unwrap_or(0);
    let mut view = VehicleView {
        mover: mover.single().map_or(0, |g| g.0),
        ..Default::default()
    };
    let vehicle_of = |guid: u64| {
        index
            .0
            .get(&guid)
            .and_then(|&e| units.get(e).ok())
            .and_then(|(.., v)| v)
    };
    let mut facts = HashMap::new();
    for (guid, store, rider, p, v) in &units {
        let guid = guid.0;
        if let Some(p) = p {
            let seat = vehicle_of(p.vehicle).and_then(|v| tables.seat(v.id, usize::from(p.seat)));
            view.units.insert(
                guid,
                VehicleUnitView {
                    state: p.state,
                    seat: seat.map(seat_view),
                    controlling: p.state == SEATED && store.0.unit_charm() == Some(p.vehicle),
                },
            );
            if guid == me {
                view.player_vehicle = Some(p.vehicle);
                // `0x5fb560`: seated, with the vehicle or ourselves the mover.
                if p.state == SEATED
                    && view.mover != 0
                    && (p.vehicle == view.mover || me == view.mover)
                {
                    view.input_seat_flags = seat.map(|s| s.flags);
                }
            }
        }
        facts.insert(
            guid,
            seats::UnitFacts {
                transport: rider.map_or(0, |r| r.transport_guid),
                seat: rider.map_or(-1, |r| r.seat),
                passenger: p.is_some(),
                vehicle: v.map(|v| v.id).filter(|&id| tables.vehicle(id).is_some()),
                is_player: matches!(store.0.object_type(), Some(ObjectType::Player)),
                name: names
                    .as_deref()
                    .and_then(|n| n.peek_unit(guid, Some(store)))
                    .map(str::to_string),
            },
        );
    }
    (view.roots, view.trees) = seats::build(&facts, &tables, view.mover);
    for (&guid, stats) in group.iter().flat_map(|g| g.stats.iter()) {
        if index.0.contains_key(&guid) {
            continue;
        }
        let seat = stats
            .vehicle_seat
            .filter(|&id| id > 0)
            .and_then(|id| tables.catalog.seats.get(&id));
        if let Some(seat) = seat {
            view.units.insert(
                guid,
                VehicleUnitView {
                    seat: Some(seat_view(seat)),
                    ..Default::default()
                },
            );
        }
    }
    let last = pushed.get(&script);
    if last.is_none() {
        script.set_vehicle_indicators(tables.indicators.clone());
    }
    if last.as_ref() != Some(&view) {
        script.set_vehicles(view.clone());
        *last = Some(view);
    }
}

/// Send the vehicle verbs' requests. With the player's transport the mover, a request is the
/// controlled vehicle's own, sent with the mover's pose after stopping our seat (`0x747fa0`): a
/// seat switch or step (`0x74ca90`, `0x74c8b0`, `0x74c9a0`), or the dismiss, which the seat must
/// allow (`0x5d46f0`, flag `0x2000000`). Otherwise the plain request.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn send_vehicle_requests(
    script: Option<NonSendMut<UiScript>>,
    net: Option<Res<crate::net::NetCommands>>,
    me: Query<Option<&Passenger>, With<crate::net::SelfPlayer>>,
    mover: Query<(&crate::net::Guid, Option<&Vehicle>), With<crate::net::ActiveMover>>,
    player: Option<Res<crate::player::Player>>,
    tables: Res<VehicleTables>,
    time: Res<Time<Real>>,
    mut stop: ResMut<SeatStop>,
) {
    let (Some(mut script), Some(net)) = (script, net) else {
        return;
    };
    let requests = script.take_vehicle_requests();
    if requests.is_empty() {
        return;
    }
    let (mover, mover_vehicle) = mover.single().map_or((0, None), |(g, v)| (g.0, v.copied()));
    let seated = me.single().ok().flatten().copied();
    let controlled = mover != 0 && seated.is_some_and(|p| p.vehicle == mover);
    // The controlled vehicle's report: the mover's pose, our seat stopped first.
    let mut controlled_cmd = |seat: Option<(u64, u8)>| {
        let p = player.as_deref()?;
        if seated.is_some_and(|s| s.state == SEATED) {
            stop.at = Some(time.elapsed_secs_f64());
        }
        Some(VehicleCommand::Controlled {
            mover,
            flags: p.move_flags(),
            pos: benilla_assets::coords::bevy_to_wow(p.pos),
            orientation: p.facing().rem_euclid(std::f32::consts::TAU),
            seat,
        })
    };
    for r in requests {
        let cmd = match r {
            VehicleRequest::Eject(guid) => Some(VehicleCommand::ByGuid {
                opcode: opcode::CMSG_EJECT_PASSENGER,
                guid,
            }),
            VehicleRequest::SwitchSeat { vehicle, seat } if controlled => {
                controlled_cmd(Some((vehicle, seat)))
            }
            VehicleRequest::SwitchSeat { vehicle, seat } => {
                Some(VehicleCommand::SwitchSeat { vehicle, seat })
            }
            // A zero guid with seat `0xff` or 1 steps back or on (`0x71f0c0`).
            VehicleRequest::PrevSeat if controlled => controlled_cmd(Some((0, 0xff))),
            VehicleRequest::NextSeat if controlled => controlled_cmd(Some((0, 1))),
            VehicleRequest::PrevSeat => Some(VehicleCommand::Empty(
                opcode::CMSG_REQUEST_VEHICLE_PREV_SEAT,
            )),
            VehicleRequest::NextSeat => Some(VehicleCommand::Empty(
                opcode::CMSG_REQUEST_VEHICLE_NEXT_SEAT,
            )),
            VehicleRequest::Exit if controlled => {
                let our_seat =
                    seated.and_then(|p| tables.seat(mover_vehicle?.id, usize::from(p.seat)));
                if our_seat.is_some_and(|s| s.flags & 0x200_0000 != 0) {
                    controlled_cmd(None)
                } else {
                    None
                }
            }
            VehicleRequest::Exit => Some(VehicleCommand::Empty(opcode::CMSG_REQUEST_VEHICLE_EXIT)),
        };
        if let Some(cmd) = cmd {
            let _ = net.0.send(ClientCommand::Vehicle(cmd));
        }
    }
}

/// Fire what the frame owes, each once per token naming the unit (`0x60bb70`).
fn feed_vehicle_signals(script: Option<NonSendMut<UiScript>>, mut signals: ResMut<VehicleSignals>) {
    let Some(mut script) = script else {
        return;
    };
    if std::mem::take(&mut signals.sliders_hidden) {
        script.fire_event("VEHICLE_ANGLE_SHOW", vec![]);
        script.fire_event("VEHICLE_POWER_SHOW", vec![]);
    }
    // `0x749650` fires a transition's events bit by bit (`0x748810` for the two enters).
    for t in std::mem::take(&mut signals.transitions) {
        let bits = passenger::event_bits(t.from, t.to);
        let enter = |name: &str, script: &mut UiScript| {
            script.fire_unit_event(
                t.unit,
                name,
                vec![
                    ScriptValue::Bool(t.vehicle_ui),
                    ScriptValue::Str(t.skin.to_string()),
                    ScriptValue::Str(t.sound.clone()),
                    ScriptValue::Bool(t.control_seat),
                    ScriptValue::Int(i64::from(t.indicator)),
                ],
            )
        };
        if bits & 1 != 0 {
            enter("UNIT_ENTERING_VEHICLE", &mut script);
        }
        if bits & 2 != 0 {
            enter("UNIT_ENTERED_VEHICLE", &mut script);
        }
        if bits & 4 != 0 {
            script.fire_unit_event(t.unit, "UNIT_EXITING_VEHICLE", vec![]);
        }
        if bits & 8 != 0 {
            script.fire_unit_event(
                t.unit,
                "UNIT_EXITED_VEHICLE",
                vec![ScriptValue::Str(t.sound.clone())],
            );
        }
    }
    if std::mem::take(&mut signals.passengers_changed) {
        script.fire_event("VEHICLE_PASSENGERS_CHANGED", vec![]);
    }
    for (guid, gained) in std::mem::take(&mut signals.data) {
        match gained {
            Some(indicator) => script.fire_unit_event(
                guid,
                "PLAYER_GAINS_VEHICLE_DATA",
                vec![ScriptValue::Int(i64::from(indicator))],
            ),
            None => script.fire_unit_event(guid, "PLAYER_LOSES_VEHICLE_DATA", vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 0x10;
    const CART: u64 = 0xF150_0000_0000_0001;

    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        // `OBJECT_FIELD_TYPE` (2): object and unit, and player for a player.
        let unit = |w: &mut World, t: ObjectType| {
            let mask = if t == ObjectType::Player { 0x19 } else { 0x9 };
            w.spawn((
                ObjectStore(ObjectFields::from_pairs(&[(2, mask)]).into_created(t)),
                Transform::from_rotation(Quat::from_rotation_y(0.75)),
            ))
            .id()
        };
        let me = unit(&mut w, ObjectType::Player);
        let cart = unit(&mut w, ObjectType::Unit);
        let mut index = GuidIndex::default();
        index.0.insert(ME, me);
        index.0.insert(CART, cart);
        w.insert_resource(index);
        w.insert_resource(SelfGuid(Some(ME)));
        let mut cat = VehicleCatalog::default();
        cat.vehicles.insert(
            36,
            VehicleRec {
                id: 36,
                ui_indicator_id: 5,
                ..Default::default()
            },
        );
        w.insert_resource(VehicleTables {
            catalog: Arc::new(cat),
            ..Default::default()
        });
        w.init_resource::<VehicleSignals>();
        (w, me, cart)
    }

    fn data(w: &mut World, guid: u64, vehicle_id: u32) {
        w.run_system_once_with(
            on_player_vehicle_data,
            SessionEvent::PlayerVehicleData { guid, vehicle_id },
        )
        .unwrap();
    }

    /// Our player gains a vehicle with its indicator, a second id re-points it with no event, and
    /// 0 takes it away; another unit's vehicle fires nothing.
    #[test]
    fn the_packet_gives_and_takes_a_vehicle() {
        let (mut w, me, cart) = world();
        data(&mut w, ME, 36);
        let v = *w.entity(me).get::<Vehicle>().expect("a vehicle");
        assert_eq!(v.id, 36);
        assert!(
            (v.raw_facing - 0.75).abs() < 1.0e-6,
            "the unit's facing: {v:?}"
        );
        assert_eq!(w.resource::<VehicleSignals>().data, [(ME, Some(5))]);

        data(&mut w, ME, 37);
        assert_eq!(w.entity(me).get::<Vehicle>().unwrap().id, 37);
        assert_eq!(
            w.resource::<VehicleSignals>().data.len(),
            1,
            "a re-point fires nothing"
        );

        data(&mut w, ME, 0);
        assert!(w.entity(me).get::<Vehicle>().is_none());
        assert_eq!(w.resource::<VehicleSignals>().data[1], (ME, None));

        data(&mut w, CART, 36);
        assert!(w.entity(cart).get::<Vehicle>().is_some());
        assert_eq!(
            w.resource::<VehicleSignals>().data.len(),
            2,
            "not our player"
        );
    }

    /// World entry hides the sliders and re-fires a held vehicle's gain.
    #[test]
    fn world_entry_refires_the_gain() {
        let (mut w, me, _) = world();
        w.entity_mut(me).insert(Vehicle {
            id: 36,
            raw_facing: 0.0,
        });
        w.init_resource::<Messages<EnteredWorldMessage>>();
        w.write_message(EnteredWorldMessage {
            billing_time_rested: 0,
            tutorial_flags: None,
        });
        w.run_system_once(on_world_enter).unwrap();
        let s = w.resource::<VehicleSignals>();
        assert!(s.sliders_hidden);
        assert_eq!(s.data, [(ME, Some(5))]);
    }

    /// A held passenger reads its seat on the vehicle and controls the vehicle it charms; an
    /// unheld group member reads the party record's seat id.
    #[test]
    fn the_view_carries_seats_to_the_verbs() {
        let (mut w, me, cart) = world();
        let mut cat = (*w.resource::<VehicleTables>().catalog).clone();
        cat.vehicles.get_mut(&36).unwrap().seat_ids[0] = 7;
        for (id, skin) in [(7, 1), (8, 0)] {
            cat.seats.insert(
                id,
                VehicleSeatRec {
                    id,
                    flags: 0x2000_0800,
                    ui_skin: skin,
                    ..Default::default()
                },
            );
        }
        w.resource_mut::<VehicleTables>().catalog = Arc::new(cat);
        w.entity_mut(cart).insert(Vehicle {
            id: 36,
            raw_facing: 0.0,
        });
        // `UNIT_FIELD_CHARM` (6-7) on the cart.
        let charm =
            ObjectFields::from_pairs(&[(2, 0x19), (6, CART as u32), (7, (CART >> 32) as u32)])
                .into_created(ObjectType::Player);
        w.entity_mut(me).insert((
            crate::net::Guid(ME),
            ObjectStore(charm),
            Passenger {
                vehicle: CART,
                seat: 0,
                state: SEATED,
            },
            TransportRider {
                transport_guid: CART,
                local_pos: [0.0; 3],
                local_orientation: 0.0,
                seat: 0,
            },
        ));
        w.entity_mut(cart).insert(crate::net::Guid(CART));
        const FAR: u64 = 0x21;
        let mut group = crate::ui_party::GroupState::default();
        group.stats.insert(
            FAR,
            benilla_protocol::messages::PartyMemberStatsInfo {
                vehicle_seat: Some(8),
                ..Default::default()
            },
        );
        w.insert_resource(group);
        let mut script = UiScript::new().unwrap();
        script.set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                has_object: true,
                guid: ME,
                ..Default::default()
            }),
        );
        script.set_unit(
            "party1",
            Some(benilla_ui::script::UnitState {
                exists: true,
                guid: FAR,
                ..Default::default()
            }),
        );
        w.insert_non_send_resource(script);
        w.run_system_once(feed_vehicle_view).unwrap();
        let script = w.non_send_resource::<UiScript>();
        let eval = |code: &str| {
            script
                .eval::<String>(&format!("return tostring({code})"))
                .unwrap()
        };
        assert_eq!(eval("UnitControllingVehicle('player')"), "true");
        assert_eq!(eval("UnitVehicleSkin('player')"), "Mechanical");
        assert_eq!(eval("UnitInVehicleControlSeat('party1')"), "true");
        assert_eq!(eval("UnitVehicleSkin('party1')"), "Natural");
        // The cart is our root, its one slot ours; with no mover the seat flags stay unread.
        assert_eq!(eval("UnitVehicleSeatCount('player')"), "1");
        assert_eq!(eval("CanSwitchVehicleSeat()"), "false");
    }
}
