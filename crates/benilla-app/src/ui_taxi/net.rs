//! The taxi map's packet handlers: they run the map's setup and close in their turn, show their
//! lines, and keep the flight masters' overhead status.

use benilla_protocol::messages::TaxiMask;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::routing::{taxi_error_key, TaxiCatalogs};
use super::TaxiState;
use crate::net::{GuidIndex, NetHandlerApp};
use crate::ui_action::{show_in_turn, UiError};
use crate::ui_script::live::UiEvents;

/// Register the taxi handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TaxiNodesShown, on_nodes_shown)
        .net_handler(K::TaxiNodeStatus, on_node_status)
        .net_handler(K::ActivateTaxiReply, on_activate_reply)
        .net_handler(K::NewTaxiPath, on_new_path)
        .net_handler(K::Disconnected, on_session_end);
}

/// `SMSG_SHOWTAXINODES` (`0x6d6050`): fewer than two known nodes show `ERR_TAXINOPATHS` and close
/// the gossip menu (`0x6d61bb`, `0x6d61d1`); else the map's setup runs (`0x6d61e7`).
fn on_nodes_shown(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::TaxiNodesShown {
        flightmaster,
        nearest_node,
        known_mask,
    } = ev
    else {
        return;
    };
    debug!("net: taxi map on {flightmaster:#x} — nearest node {nearest_node}");
    if !shows_map(world, &known_mask) {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            show_in_turn(&mut ui, "ui_taxi", UiError::key("ERR_TAXINOPATHS"));
        }
        crate::ui_gossip::set_npc(world, 0);
        return;
    }
    super::setup_map(world, flightmaster, nearest_node, known_mask);
}

/// The known-node mask read at the catalog's word count (`0x593904`).
fn size_mask(known_mask: &mut TaxiMask, catalogs: Option<&TaxiCatalogs>) {
    if let Some(catalogs) = catalogs {
        known_mask.resize(catalogs.mask_words());
    }
}

/// Whether `SMSG_SHOWTAXINODES` shows the map, which takes the interact target (`0x6d61e7` -> `0x59374d`): two known nodes; fewer show `ERR_TAXINOPATHS` and touch only the gossip (`0x6d61b5`-`0x6d61d1`).
fn shows_map(world: &World, known_mask: &TaxiMask) -> bool {
    let mut mask = known_mask.clone();
    size_mask(&mut mask, world.get_resource::<TaxiCatalogs>());
    mask.count() >= 2
}

fn on_node_status(In(ev): In<SessionEvent>, mut commands: Commands, index: Res<GuidIndex>) {
    if let SessionEvent::TaxiNodeStatus { guid, known } = ev {
        taxi_node_status(guid, known, &mut commands, &index);
    }
}

/// `SMSG_ACTIVATETAXIREPLY` (`0x6cee90`): 0 closes the map (`0x6ceeb3`), 1 to 12 show their
/// message row (`0x6ceea7`), 13 up nothing.
fn on_activate_reply(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::ActivateTaxiReply { code } = ev else {
        return;
    };
    debug!("net: activate taxi reply — code {code}");
    if let Some(mut taxi) = world.get_resource_mut::<TaxiState>() {
        taxi.last_reply = Some(code);
    }
    if code == benilla_protocol::messages::taxi_reply::OK {
        super::close_map(world);
    } else if let Some(key) = taxi_error_key(code) {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            show_in_turn(&mut ui, "ui_taxi", UiError::key(key));
        }
    }
}

/// `SMSG_NEW_TAXI_PATH`, an empty body, sent on a first visit: `ERR_NEWTAXIPATH` (`0x6e3054`),
/// then the gossip menu closed by the zero gossip set (`0x6e3063`).
fn on_new_path(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::NewTaxiPath = ev {
        debug!("net: taxi — first-visit node learned (SMSG_NEW_TAXI_PATH)");
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            show_in_turn(&mut ui, "ui_taxi", UiError::key("ERR_NEWTAXIPATH"));
        }
        crate::ui_gossip::set_npc(world, 0);
    }
}

/// The map ends with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut taxi: ResMut<TaxiState>) {
    taxi.clear_session();
}

/// `SMSG_TAXINODE_STATUS`, the answer to `CMSG_TAXINODE_STATUS_QUERY` and the second half of a
/// first-visit learn: upsert [`super::FlightMasterStatus`], so the green icon follows `known`. The
/// reference's handler also requires the flight-master NPC flag (bit 3); only flight masters are
/// queried or answered, so it is not tested here.
fn taxi_node_status(guid: u64, known: bool, commands: &mut Commands, index: &GuidIndex) {
    debug!("net: taxi node status — {guid:#x} known={known}");
    if let Some(&e) = index.0.get(&guid) {
        commands
            .entity(e)
            .insert(super::FlightMasterStatus { known });
    }
}
