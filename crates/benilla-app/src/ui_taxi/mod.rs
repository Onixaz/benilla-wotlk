//! The taxi map's app side: `SMSG_SHOWTAXINODES` runs the map's setup (`0x5932e0`) in its
//! handler, which lists the known nodes on the map the current node draws on with their routes
//! (built in [`routing`]) into [`TaxiState`], takes the interact target and fires
//! `TAXIMAP_OPENED`. The verbs read that table through [`TaxiLive`] at their call, price a fare at
//! the call (`0x591350`), and activate or close inside it (`0x5914b0`, `0x590ba0`).

use benilla_protocol::messages::TaxiMask;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{TaxiAct, TaxiOutcome, TaxiUiState};

use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::live::UiEvents;
use crate::ui_session::NpcSession;

mod routing;
pub(crate) use routing::ResolvedTaxiNode;
use routing::{build_nodes, load_taxi_catalogs, TaxiCatalogs, TaxiFares};

/// What the last setup listed: the current node, the mask it kept, the Lua face of each listed
/// node and what its fare and activate read; it stays until the next setup or the world leave.
pub(crate) struct TaxiTable {
    pub(crate) current: u32,
    pub(crate) known: TaxiMask,
    pub(crate) ui: TaxiUiState,
    pub(crate) rows: Vec<ResolvedTaxiNode>,
}

/// The taxi map's globals: the flight master it is open on and the last setup's table.
#[derive(Resource, Default)]
pub(crate) struct TaxiState {
    /// The flight master the map is open on, zeroed by the close (`0x590bc7`).
    pub(crate) unit: Option<u64>,
    /// The listed nodes (`0x593317` zeroes the count, `0x593771` fills it).
    pub(crate) table: Option<TaxiTable>,
    /// The last `SMSG_ACTIVATETAXIREPLY` code, which the taxi probe reads.
    pub(crate) last_reply: Option<u32>,
}

impl TaxiState {
    /// The disconnect clear.
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
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

/// The range guard's zero, its events fired by [`close_map`].
impl NpcSession for TaxiState {
    fn npc(&self) -> Option<u64> {
        self.unit
    }

    fn close(&mut self) {
        self.unit = None;
    }
}

/// The map's setup (`0x5932e0`): the same flight master again does nothing (`0x5932fc`); another
/// closes the open map first (`0x593306`). The table is rebuilt, and with no node listed the map
/// stays shut (`0x593734`); else the interact target is taken (`0x59374d`), the flight master
/// stored, `TAXIMAP_OPENED` fired (`0x59378b`) and the Travel tutorial triggered (`0x593792`);
/// with more than one node listed we stand up, dismount and leave a form the flight master
/// cancels (`0x59379a`-`0x59380a`).
pub(crate) fn setup_map(world: &mut World, unit: u64, current: u32, known: TaxiMask) {
    let held = world.get_resource::<TaxiState>().and_then(|t| t.unit);
    if held == Some(unit) {
        return;
    }
    if held.is_some() {
        close_map(world);
    }
    // The rest once the close's event has fired (`0x593317`).
    crate::ui_script::live::in_turn(world, move |w| list_and_open(w, unit, current, &known));
}

/// The setup past its close: the table rebuilt, then the open with any node listed.
fn list_and_open(world: &mut World, unit: u64, current: u32, known: &TaxiMask) {
    let table = build_table(world, current, known);
    let listed = table.as_ref().map_or(0, |t| t.ui.nodes.len());
    if let Some(mut state) = world.get_resource_mut::<TaxiState>() {
        state.table = table;
    }
    if listed == 0 {
        return;
    }
    crate::ui_session::set_interact_target(world, unit);
    crate::ui_script::live::in_turn(world, move |w| {
        if let Some(mut state) = w.get_resource_mut::<TaxiState>() {
            state.unit = Some(unit);
        }
        if let Some(mut ui) = w.get_resource_mut::<UiEvents>() {
            ui.fire("TAXIMAP_OPENED", vec![]);
        }
        if let Some(mut asks) = w.get_resource_mut::<Messages<crate::tutorial::TutorialEvent>>() {
            asks.write(crate::tutorial::TutorialEvent::trigger(
                crate::tutorial::id::TRAVEL,
            ));
        }
        if listed > 1 {
            leave_for_the_flight(w);
        }
    });
}

/// The listed table off the catalogs, the player's faction group (the walk's team check,
/// `0x5926c0`) and the account's expansion (`0x59356c`); `None` with no catalogs or no map.
fn build_table(world: &mut World, current: u32, known: &TaxiMask) -> Option<TaxiTable> {
    let mut me = world.query_filtered::<&ObjectStore, With<SelfPlayer>>();
    let template = me
        .iter(world)
        .next()
        .and_then(|s| s.0.unit_faction_template());
    let player_group = world
        .get_resource::<crate::target::Factions>()
        .map(|f| f.catalog())
        .zip(template)
        .and_then(|(c, t)| c.template(t))
        .map(|t| t.group_mask);
    let expansion = world
        .get_resource::<crate::net::AccountExpansion>()
        .map_or(0, |e| e.0);
    let cat = world.get_resource::<TaxiCatalogs>()?;
    let mut known = known.clone();
    known.resize(cat.mask_words());
    let (map_id, nodes, rows) = build_nodes(current, &known, cat, None, player_group, expansion)?;
    Some(TaxiTable {
        current,
        known,
        ui: TaxiUiState {
            art: format!("Interface\\TaxiFrame\\TAXIMAP{map_id}"),
            nodes,
        },
        rows,
    })
}

/// The setup's tail with more than one node listed: a stand state but standing stands up
/// (`0x5937dc`), a mount not held for passengers (`0x5937e1`-`0x5937ea`) dismounts unless on a taxi
/// (`0x5937f8` -> `0x7412e0`), and a form
/// the flight master cancels, one without the stance bit or with bit `0x800`, is left (`0x5937ff`
/// -> `0x721e70`, `0x726ce0`).
fn leave_for_the_flight(world: &mut World) {
    let mut me = world
        .query_filtered::<(&ObjectStore, Option<&crate::unit_auras::UnitAuras>), With<SelfPlayer>>(
        );
    let Some((store, auras)) = me.iter(world).next() else {
        return;
    };
    let echoed = store.0.unit_stand_state();
    let mounted = store.0.unit_is_mounted()
        && store.0.unit_flags() & crate::player::UNIT_FLAG_TAXI_FLIGHT == 0;
    let form = store.0.unit_shapeshift_form();
    let spells = world.get_resource::<crate::ui_action::Spells>();
    let cancels = form != 0
        && spells
            .and_then(|s| s.forms.get(&u32::from(form)))
            .is_some_and(|row| row.flags & 0x1 == 0 || row.flags & 0x800 != 0);
    let form_auras = match (cancels, auras, spells) {
        (true, Some(auras), Some(spells)) => {
            crate::ui_shapeshift::form_auras(form, auras, &spells.catalog)
        }
        _ => Vec::new(),
    };
    let standing = world
        .get_resource::<crate::player::Player>()
        .map_or(echoed, |p| p.committed_stand_state(echoed))
        == 0;
    if let Some(net) = world.get_resource::<NetCommands>() {
        if mounted {
            let _ = net.0.send(ClientCommand::CancelMountAura);
        }
    }
    // Each aura cancel: its early form drop, then its send after the drop's listeners (`0x80324d`, `0x803458`).
    for spell_id in form_auras {
        crate::ui_shapeshift::cancel_aura_step(world, spell_id);
    }
    if !standing {
        world.write_message(crate::player::StandStateRequest { state: 0 });
    }
}

/// The flight map's close (`0x590ba0`): with a flight master stored, the interact target cleared on it, whose
/// interaction close runs this leg first when it held it (`0x590bb7` -> `0x512f58`), then the
/// flight master zeroed (`0x590bc7`) and `TAXIMAP_CLOSED` fired.
pub(crate) fn close_map(world: &mut World) {
    let Some(unit) = world.get_resource::<TaxiState>().and_then(|t| t.unit) else {
        return;
    };
    crate::ui_session::clear_interact_target(world, unit);
    zero_and_fire(world);
}

/// The close's tail: zero, then `TAXIMAP_CLOSED`.
fn zero_and_fire(world: &mut World) {
    if let Some(mut t) = world.get_resource_mut::<TaxiState>() {
        t.unit = None;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("TAXIMAP_CLOSED", vec![]);
    }
}

/// The interaction close's taxi leg with the interact target already zeroed (`0x512f58`): the close, whose own clear then finds no target.
pub(crate) fn close_by_interaction(world: &mut World) {
    if world
        .get_resource::<TaxiState>()
        .is_some_and(|t| t.unit.is_some())
    {
        zero_and_fire(world);
    }
}

/// The world leave's taxi part (`0x528cf6` -> `0x591ac0`): the table freed (`0x591a10`), then the close.
pub(crate) fn leave_world(world: &mut World) {
    if let Some(mut t) = world.get_resource_mut::<TaxiState>() {
        t.table = None;
    }
    close_map(world);
}

/// The taxi map the verbs read and act on at their call.
#[derive(SystemParam)]
pub(crate) struct TaxiLive<'w, 's> {
    state: ResMut<'w, TaxiState>,
    commands: Res<'w, NetCommands>,
    index: Res<'w, GuidIndex>,
    units: Query<'w, 's, &'static ObjectStore, Without<SelfPlayer>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    reactions: crate::target::ReactionInputs<'w>,
}

impl TaxiLive<'_, '_> {
    /// The last setup's table.
    pub(crate) fn map(&self) -> Option<&TaxiUiState> {
        self.state.table.as_ref().map(|t| &t.ui)
    }

    /// `TaxiNodeCost` (`0x591350`): 0 with no player or no flight master object; over a direct hop
    /// its fare less the flight master's discount, else the walked fare at the reaction's tier.
    pub(crate) fn cost(&self, slot: u32) -> i64 {
        let (Some(me), Some(master)) = (
            self.me.iter().next(),
            self.state
                .unit
                .and_then(|u| self.index.0.get(&u))
                .and_then(|&e| self.units.get(e).ok()),
        ) else {
            return 0;
        };
        let Some(row) = self
            .state
            .table
            .as_ref()
            .and_then(|t| t.rows.get(slot as usize))
        else {
            return 0;
        };
        row.fare(taxi_fares(&self.reactions, master, me))
    }

    /// `TakeTaxiNode` (`0x5914b0`) and `CloseTaxiMap` (`0x590ba0`) inside their call.
    pub(crate) fn act(
        &mut self,
        act: TaxiAct,
        target: Option<&mut crate::ui_session::InteractTarget>,
    ) -> TaxiOutcome {
        let mut out = TaxiOutcome::default();
        match act {
            TaxiAct::Take(slot) => {
                let Some((current, row)) = self
                    .state
                    .table
                    .as_ref()
                    .and_then(|t| Some((t.current, t.rows.get(slot as usize)?.clone())))
                else {
                    return out;
                };
                let Some(money) = self.me.iter().next().map(|me| {
                    // Mounted, a mount held for passengers not (`0x5914e1`-`0x5914ea`).
                    (me.0.unit_is_mounted(), me.0.player_money().unwrap_or(0))
                }) else {
                    return out;
                };
                if money.0 {
                    out.error = Some("ERR_TAXIPLAYERALREADYMOUNTED");
                    return out;
                }
                let guid = self.state.unit.unwrap_or(0);
                let (src, dest) = (current, row.id);
                if src == dest {
                    out.error = Some("ERR_TAXISAMENODE");
                } else if row.direct.is_some() {
                    debug!("ui_taxi: activate {src} -> {dest}");
                    let _ = self.commands.0.send(ClientCommand::ActivateTaxi {
                        guid,
                        source_node: src,
                        dest_node: dest,
                    });
                } else if let Some(chain) = row.path {
                    // The fare at the call against our coinage (`0x591569`).
                    if self.cost(slot) as u32 <= money.1 {
                        debug!("ui_taxi: activate express {chain:?}");
                        let _ = self
                            .commands
                            .0
                            .send(ClientCommand::ActivateTaxiExpress { guid, nodes: chain });
                    } else {
                        out.error = Some("ERR_TAXINOTENOUGHMONEY");
                    }
                } else {
                    out.error = Some("ERR_TAXINOSUCHPATH");
                }
            }
            TaxiAct::Close => {
                let Some(unit) = self.state.unit else {
                    return out;
                };
                out.closed = 1;
                if target.is_some_and(|t| t.clear_in_call(unit)) {
                    out.closed = 2;
                }
                self.state.unit = None;
            }
        }
        out
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

mod net;

pub(crate) struct UiTaxiPlugin;

impl Plugin for UiTaxiPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TaxiState>()
            .add_systems(Update, load_taxi_catalogs);
    }
}

#[cfg(test)]
mod tests;
