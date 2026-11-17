//! The taxi map's app side: [`TaxiState`] holds the `SMSG_SHOWTAXINODES` map and the one-shot
//! replies, [`feed_taxi`] pushes the known nodes on the map the current node draws on with their
//! routes (built in [`routing`]) and fires the map events, and [`drain_taxi`] sends the activate.

use benilla_protocol::messages::TaxiMask;
use bevy::prelude::*;

use benilla_ui::script::{TaxiUiState, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiDrain, UiFeed};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

mod routing;
use routing::{
    build_nodes, load_taxi_catalogs, taxi_error_key, TaxiCatalogs, TaxiFares, TaxiRouteCache,
};

/// The open map, `SMSG_SHOWTAXINODES` as the wire gave it.
pub(crate) struct TaxiOpen {
    pub(crate) flightmaster: u64,
    /// The node nearest the flight master, typed `Current`.
    pub(crate) nearest_node: u32,
    /// The known nodes: the only ones listed, and the only ones a route passes through.
    pub(crate) known: TaxiMask,
}

/// The taxi session: the open map, plus the one-shot replies [`feed_taxi`] consumes.
#[derive(Resource, Default)]
pub(crate) struct TaxiState {
    pub(crate) open: Option<TaxiOpen>,
    /// The last `SMSG_ACTIVATETAXIREPLY` code, for [`feed_taxi`] to show, or on `OK` to close the
    /// map.
    pub(crate) reply: Option<u32>,
    /// A first-visit learn (`SMSG_NEW_TAXI_PATH`) for [`feed_taxi`] to announce.
    pub(crate) discovered: bool,
    /// A map refused for knowing fewer than two nodes, for [`feed_taxi`] to show
    /// `ERR_TAXINOPATHS`.
    pub(crate) no_paths: bool,
}

impl TaxiState {
    pub(crate) fn open(&mut self, flightmaster: u64, nearest_node: u32, known: TaxiMask) {
        self.open = Some(TaxiOpen {
            flightmaster,
            nearest_node,
            known,
        });
    }

    /// A client-side close; the staged replies stay.
    pub(crate) fn clear(&mut self) {
        self.open = None;
    }

    /// The disconnect clear, staged replies included.
    pub(crate) fn clear_session(&mut self) {
        self.open = None;
        self.reply = None;
        self.discovered = false;
        self.no_paths = false;
    }
}

/// A flight master's `SMSG_TAXINODE_STATUS`: `known = false` shows the green `TalkToMeGreen`
/// overhead icon. The query and the teardown belong to [`crate::quest_markers::query`]: the
/// reference's only sender, is called only, the per-unit function that also sends the questgiver
/// query, and both icons share one marker slot (`unit+0xb2c`).
#[derive(Component, Clone, Copy)]
pub(crate) struct FlightMasterStatus {
    pub(crate) known: bool,
}

/// The range guard closes the map, as the close button does, when the flight master is out of
/// range or gone.
impl NpcSession for TaxiState {
    fn npc(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.flightmaster)
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// Push the map into the VM and fire its events, show an activate refusal or close on `OK`, and
/// announce a first-visit discovery.
fn feed_taxi(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    mut state: ResMut<TaxiState>,
    catalogs: Option<Res<TaxiCatalogs>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut cache: ResMut<TaxiRouteCache>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    reactions: crate::target::ReactionInputs,
    expansion: Res<crate::net::AccountExpansion>,
    mut last: Local<crate::ui_script::VmMemo<Option<TaxiUiState>>>,
    mut last_name: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_name = last_name.get(&script);

    // The activate reply: a refusal shows where its message row says; `OK` closes the map, as
    // the reference's handler does for code 0.
    if let Some(code) = state.reply.take() {
        match taxi_error_key(code) {
            Some(key) => {
                let text = script
                    .lua()
                    .globals()
                    .get::<String>(key)
                    .unwrap_or_default();
                if !text.is_empty() {
                    crate::ui_action::show_messages(
                        &mut script,
                        &mut sink,
                        "ui_taxi",
                        [crate::ui_action::Shown::keyed(key, text)],
                    );
                }
            }
            None => state.open = None,
        }
    }

    // A map with fewer than two known nodes: message `0xc4` instead of the map (`0x6d61bb`).
    if std::mem::take(&mut state.no_paths) {
        let text = script
            .lua()
            .globals()
            .get::<String>("ERR_TAXINOPATHS")
            .unwrap_or_default();
        if !text.is_empty() {
            crate::ui_action::show_messages(
                &mut script,
                &mut sink,
                "ui_taxi",
                [crate::ui_action::Shown::keyed("ERR_TAXINOPATHS", text)],
            );
        }
    }

    // The first-visit learn: the reference shows message `0xf2`, `ERR_NEWTAXIPATH`, whose row
    // routes it to the yellow `UI_INFO_MESSAGE` and plays its `TaxiNodeDiscovered` sound kit; the
    // catalog row carries both.
    if std::mem::take(&mut state.discovered) {
        let text = script
            .lua()
            .globals()
            .get::<String>("ERR_NEWTAXIPATH")
            .unwrap_or_default();
        if !text.is_empty() {
            crate::ui_action::show_messages(
                &mut script,
                &mut sink,
                "ui_taxi",
                [crate::ui_action::Shown::keyed("ERR_NEWTAXIPATH", text)],
            );
        }
    }

    let Some(catalogs) = catalogs else {
        return;
    };

    // The map is the one the current node draws on (`build_nodes`). The fares need the flight
    // master and the player ([`taxi_fares`]); the player's faction group bars the other team's
    // nodes from the route walk (`0x5926c0`).
    let fresh = state.open.as_ref().and_then(|open| {
        let flightmaster = index
            .0
            .get(&open.flightmaster)
            .and_then(|&e| stores.get(e).ok());
        let me = self_q.iter().next();
        let fares = flightmaster
            .zip(me)
            .map(|(master, me)| taxi_fares(&reactions, master, me));
        let catalog = reactions.factions.as_deref().map(|f| f.catalog());
        let player_group = catalog
            .zip(me.and_then(|me| me.0.unit_faction_template()))
            .and_then(|(c, t)| c.template(t))
            .map(|t| t.group_mask);
        let (map_id, nodes, resolved) =
            build_nodes(open, &catalogs, fares, player_group, expansion.0)?;
        cache.0 = resolved;
        Some(TaxiUiState {
            art: format!("Interface\\TaxiFrame\\TAXIMAP{map_id}"),
            nodes,
        })
    });
    if fresh.is_none() {
        cache.0.clear();
    }

    // The flight master's name is tracked so its landing re-fires `TAXIMAP_OPENED`, whose stock
    // handler titles the map with `UnitName("npc")` (`TaxiFrame.lua:27`); the reference fires it
    // only on open.
    let flightmaster_name = state
        .open
        .as_ref()
        .and_then(|open| names.resolve(open.flightmaster, &commands))
        .map(str::to_string);
    let name_changed = *last_name != flightmaster_name;

    if fresh != *last || (fresh.is_some() && name_changed) {
        script.set_taxi(fresh.clone());
        match (&*last, &fresh) {
            // No arguments: the event's one fire site is a plain event signal.
            (None, Some(_)) | (Some(_), Some(_)) => {
                script.fire_event("TAXIMAP_OPENED", Vec::new());
            }
            (Some(_), None) => script.fire_event("TAXIMAP_CLOSED", vec![]),
            (None, None) => {}
        }
        *last = fresh;
        *last_name = flightmaster_name;
    }
}

/// What the Lua `TaxiNodeCost`'s fare (`0x591350`) reads off the flight master and the player: the
/// price discount (`0x7279a0`), and the fare tier from the flight master's reaction toward the
/// player (`0x7251c0`), read as Neutral when its faction has no reputation slot
/// (`0x718b30`, on its template's faction).
fn taxi_fares(
    reactions: &crate::target::ReactionInputs,
    master: &ObjectStore,
    me: &ObjectStore,
) -> TaxiFares {
    let factions = reactions.factions.as_deref();
    let reaction = crate::target::ring_reaction(
        factions,
        &reactions.reputations,
        reactions.party(),
        Some(master),
        Some(me),
    );
    let has_reputation = factions.is_some_and(|f| {
        master
            .0
            .unit_faction_template()
            .and_then(|t| f.catalog().template(t))
            .is_some_and(|t| f.catalog().reputation_faction(t.faction).is_some())
    });
    TaxiFares {
        discount: crate::target::vendor_price_discount(
            factions,
            &reactions.reputations,
            reactions.party(),
            master,
            me,
        ),
        tier: TaxiFares::tier_of(reaction, has_reputation),
    }
}

/// Drain the Lua intents. `TakeTaxiNode(i)` sends `CMSG_ACTIVATETAXI` when a direct `TaxiPath` edge
/// joins the current node and the target, as its one drawn segment shows, and otherwise
/// `CMSG_ACTIVATETAXIEXPRESS` with the chain, as the reference's test decides; the
/// `Current` node sends nothing. `CloseTaxiMap()` clears locally, with no packet.
fn drain_taxi(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    mut state: ResMut<TaxiState>,
    cache: Res<TaxiRouteCache>,
    catalogs: Option<Res<TaxiCatalogs>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for idx in script.take_taxi_node() {
        let Some(open) = &state.open else { continue };
        let Some(resolved) = idx.checked_sub(1).and_then(|i| cache.0.get(i)) else {
            continue;
        };
        let chain = resolved.chain.as_slice();
        let (Some(&src), Some(&dest)) = (chain.first(), chain.last()) else {
            continue;
        };
        if src == dest {
            debug!("ui_taxi: TakeTaxiNode({idx}) is the current node — ignored");
            continue;
        }
        let direct = catalogs
            .as_ref()
            .is_some_and(|c| c.paths.between(src, dest).is_some());
        if direct {
            debug!(
                "ui_taxi: activate {src} -> {dest} (direct edge, {} copper)",
                resolved.cost
            );
            let _ = commands.0.send(ClientCommand::ActivateTaxi {
                guid: open.flightmaster,
                source_node: src,
                dest_node: dest,
            });
        } else {
            debug!(
                "ui_taxi: activate express {chain:?} ({} copper)",
                resolved.cost
            );
            let _ = commands.0.send(ClientCommand::ActivateTaxiExpress {
                guid: open.flightmaster,
                nodes: chain.to_vec(),
            });
        }
    }
    if script.take_taxi_close() {
        debug!("ui_taxi: client-side close (no packet)");
        state.clear();
    }
}

mod net;

pub(crate) struct UiTaxiPlugin;

impl Plugin for UiTaxiPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TaxiState>()
            .init_resource::<TaxiRouteCache>()
            .add_systems(
                Update,
                (
                    load_taxi_catalogs,
                    // Range-close first so the clear fires `TAXIMAP_CLOSED` the same frame.
                    close_npc_session_out_of_range::<TaxiState>.before(feed_taxi),
                    feed_taxi.in_set(UiFeed),
                    drain_taxi.in_set(UiDrain),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::FIELD_UNIT_FLAGS;
    use benilla_protocol::ObjectFields;

    /// The map's fares take the flight master's discount and the express leg sends the chain alone.
    /// Honored makes 0.1, as no 3.3.5a field carries an honor rank: Sentinel Hill's direct 110
    /// shows 99, and Thelsamar's route through Ironforge, 50 + 110, shows 144 and sends its three
    /// nodes. With the flight master unresolved the fares show 0.
    #[test]
    fn the_map_shows_discounted_fares_and_the_express_sends_the_chain() {
        use benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE;
        use bevy::ecs::system::RunSystemOnce;

        const BYTES_0: u16 = benilla_protocol::field::FIELD_UNIT_BYTES_0;
        const MASTER: u64 = 0xF130_0000_0000_0099;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalogs = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        let (factions, template, reps) = crate::target::stormwind_fixture(&mut chain, 9000);

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TaxiState>()
            .init_resource::<TaxiRouteCache>()
            .init_resource::<GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::net::AccountExpansion(2))
            .insert_resource(catalogs)
            .insert_resource(factions)
            .insert_resource(reps)
            .insert_non_send_resource(UiScript::new().unwrap());
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[(
                BYTES_0,
                crate::target::HUMAN_WARRIOR,
            )])),
        ));
        let master = app
            .world_mut()
            .spawn((
                crate::net::Guid(MASTER),
                ObjectStore(ObjectFields::from_pairs(&[
                    (FIELD_UNIT_FACTIONTEMPLATE, template),
                    (FIELD_UNIT_FLAGS, 0x1000),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(MASTER, master);
        // At Stormwind (2), knowing Sentinel Hill (4), Ironforge (6) and Thelsamar (8).
        app.world_mut()
            .resource_mut::<TaxiState>()
            .open(MASTER, 2, TaxiMask(vec![0b1010_1010]));

        let node = |app: &mut App, name: &str| -> (i64, i64) {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval(&format!(
                    "for i = 1, NumTaxiNodes() do \
                       if TaxiNodeName(i) == {name:?} then return i, TaxiNodeCost(i) end \
                     end"
                ))
                .unwrap()
        };
        app.world_mut().run_system_once(feed_taxi).unwrap();
        assert_eq!(node(&mut app, "Sentinel Hill, Westfall").1, 99);
        assert_eq!(node(&mut app, "Ironforge, Dun Morogh").1, 45);
        let (thelsamar, shown) = node(&mut app, "Thelsamar, Loch Modan");
        assert_eq!(shown, 144);

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(&format!("TakeTaxiNode({thelsamar})"))
            .unwrap();
        app.world_mut().run_system_once(drain_taxi).unwrap();
        // The feed's name lookup queries the flight master; only the activates matter here.
        let sent: Vec<_> = rx
            .try_iter()
            .filter(|c| {
                matches!(
                    c,
                    ClientCommand::ActivateTaxi { .. } | ClientCommand::ActivateTaxiExpress { .. }
                )
            })
            .collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::ActivateTaxiExpress { nodes, .. }] if nodes == &[2, 6, 8]
            ),
            "the express carries the chain, got {sent:?}"
        );

        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .remove(&MASTER);
        app.world_mut().run_system_once(feed_taxi).unwrap();
        assert_eq!(node(&mut app, "Sentinel Hill, Westfall").1, 0);
    }

    #[test]
    fn open_close_and_session_clear() {
        let mut state = TaxiState::default();
        assert_eq!(state.npc(), None);

        state.open(0x42, 2, TaxiMask(vec![0xA]));
        assert_eq!(state.npc(), Some(0x42));
        assert_eq!(state.open.as_ref().unwrap().nearest_node, 2);

        // A client-side close drops the map, nothing else.
        state.reply = Some(3);
        state.discovered = true;
        NpcSession::close(&mut state);
        assert_eq!(state.npc(), None);
        assert_eq!(
            state.reply,
            Some(3),
            "close leaves staged reply/discovery alone"
        );

        // Disconnect drops everything.
        state.open(0x42, 2, TaxiMask::default());
        state.clear_session();
        assert_eq!(state.npc(), None);
        assert_eq!(state.reply, None);
        assert!(!state.discovered);
    }
}
