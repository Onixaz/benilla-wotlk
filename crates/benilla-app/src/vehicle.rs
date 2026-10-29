//! Vehicles: a unit's vehicle (the reference's `CVehicle_C`, `0x757fa0`), its seats, and what
//! the interface hears of them.
//!
//! A unit gains its vehicle from the create block's `UPDATEFLAG_VEHICLE` (`0x73c260`) or from
//! `SMSG_PLAYER_VEHICLE_DATA` (`0x716db0`), which takes it away with id 0. Our own player's gain
//! and loss fire `PLAYER_GAINS_VEHICLE_DATA` (`0x74bbd0`) and `PLAYER_LOSES_VEHICLE_DATA`
//! (`0x74bc50`); a create-block vehicle is built before the object's post-init (`0x744a50` sets
//! the gate bit `0x40000`), so only the packet's fires, and world entry fires it again for a
//! player already holding one (`0x757180`).

use std::sync::Arc;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::{VehicleCatalog, VehicleRec};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, UiScript};

use crate::net::{EnteredWorldMessage, GuidIndex, NetHandlerApp, ObjectStore, SelfGuid};

/// The vehicle tables; empty without client data.
#[derive(Resource, Default, Clone)]
pub(crate) struct VehicleTables(pub(crate) Arc<VehicleCatalog>);

impl VehicleTables {
    pub(crate) fn vehicle(&self, id: u32) -> Option<&VehicleRec> {
        self.0.vehicle(id)
    }
}

/// A unit's vehicle: its `Vehicle.dbc` id and the facing its facing limits run from
/// (`m_rawFacing`, the create block's vehicle orientation, else the unit's facing at creation).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Vehicle {
    pub(crate) id: u32,
    pub(crate) raw_facing: f32,
}

/// What the frame owes the VM.
#[derive(Resource, Default)]
pub(crate) struct VehicleSignals {
    /// `(unit, gained)`: our player gained (`Some(indicator)`) or lost (`None`) its vehicle.
    data: Vec<(u64, Option<u32>)>,
    /// World entry's slider reset (`0x757180` → `0x74c5a0` with no vehicle): both shows, empty.
    sliders_hidden: bool,
}

pub(crate) struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VehicleTables>()
            .init_resource::<VehicleSignals>()
            .add_message::<EnteredWorldMessage>()
            // After the object handlers, whose commands spawn the unit this reads.
            .net_handler(SessionEventKind::ObjectCreate, on_object_create)
            .net_handler(SessionEventKind::PlayerVehicleData, on_player_vehicle_data)
            .add_systems(Startup, load_vehicle_tables.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    on_world_enter.after(benilla_world::schedule::WorldStage::Net),
                    feed_vehicle_signals
                        .in_set(crate::ui_unit::UnitFeed)
                        .after(crate::ui_unit::EnterWorldFeed)
                        .after(on_world_enter),
                ),
            );
    }
}

fn load_vehicle_tables(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_vehicles(&mut chain)
    };
    match loaded {
        Ok(catalog) => commands.insert_resource(VehicleTables(Arc::new(catalog))),
        Err(e) => warn!("vehicle: the vehicle tables failed — no unit holds a vehicle: {e:#}"),
    }
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

/// Fire what the frame owes, each once per token naming the unit (`0x60bb70`).
fn feed_vehicle_signals(script: Option<NonSendMut<UiScript>>, mut signals: ResMut<VehicleSignals>) {
    let Some(mut script) = script else {
        return;
    };
    if std::mem::take(&mut signals.sliders_hidden) {
        script.fire_event("VEHICLE_ANGLE_SHOW", vec![]);
        script.fire_event("VEHICLE_POWER_SHOW", vec![]);
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
        w.insert_resource(VehicleTables(Arc::new(cat)));
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
}
