//! The taxi-map bindings the stock `TaxiFrame.lua` runs on. The app's map setup (`0x5932e0`)
//! lists the nodes with their types, positions and route lines, which stay until the next setup
//! or the world leave; the verbs read them through the lent view at the call, `TaxiNodeCost`
//! prices at the call (`0x591350`), and `TakeTaxiNode` and `CloseTaxiMap` act inside it. Positions
//! are normalized 0..1 from the map's bottom-left, which the Lua scales by the 316x352 map.

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::region::region_handle_of;

/// Bit 20 of a unit's flags word, the taxi ride `UnitOnTaxi` tests (`0x60f39c`).
const UNIT_FLAG_TAXI_FLIGHT: u32 = 1 << 20;

/// A listed node's icon state (`TaxiNodeGetType`, `0x590c70`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaxiNodeType {
    /// The flight master's own node, "You are here".
    Current,
    /// Known and reached by the route walk; clickable.
    #[default]
    Reachable,
    /// Known but outside the mask. Never produced: the setup lists masked nodes alone; the variant
    /// stays because `TaxiButtonTypes` names it.
    Distant,
    /// Listed but never reached by the route walk (`0x590cae`), a button the stock frame hides.
    Unreached,
}

impl TaxiNodeType {
    /// The type string, a `TaxiButtonTypes` key.
    fn era_str(self) -> &'static str {
        match self {
            TaxiNodeType::Current => "CURRENT",
            TaxiNodeType::Reachable => "REACHABLE",
            TaxiNodeType::Distant => "DISTANT",
            TaxiNodeType::Unreached => "NONE",
        }
    }
}

/// One listed node on the taxi map, as the setup left it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaxiUiNode {
    /// `TaxiNodeName`, the DBC's localized name.
    pub name: String,
    /// `TaxiNodeGetType`.
    pub node_type: TaxiNodeType,
    /// `TaxiNodePosition`: normalized `(x, y)` on the map art, from the bottom-left.
    pub pos: (f32, f32),
    /// `TaxiNodeCost` where no view is lent: the fare in copper, 0 for the current node.
    pub cost: u32,
    /// The route lines as `[src_x, src_y, dest_x, dest_y]` in the same space, which `TaxiGetSrcX`
    /// to `TaxiGetDestY` read (`0x590f40`); empty for an unreached node.
    pub routes: Vec<[f32; 4]>,
    /// `GetNumRoutes` (`0x5911c0`): 1 over a direct hop, else the walked hops less one.
    pub num_routes: i64,
}

/// The listed nodes and the continent art the last setup loaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaxiUiState {
    /// The map art `SetTaxiMap` assigns (e.g. `Interface\TaxiFrame\TAXIMAP1`).
    pub art: String,
    /// The listed nodes in id order (Lua indexes them 1-based).
    pub nodes: Vec<TaxiUiNode>,
}

/// A taxi verb's act inside its call, on a 0-based listed slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaxiAct {
    /// `TakeTaxiNode` (`0x5914b0`).
    Take(u32),
    /// `CloseTaxiMap` (`0x590ba0`).
    Close,
}

/// What a taxi act did inside the call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaxiOutcome {
    /// The message key the call showed.
    pub error: Option<&'static str>,
    /// How many `TAXIMAP_CLOSED`s the close fired: two when the flight master held the interact target, whose clear closes the map inside it (`0x590bb7` -> `0x512f58`).
    pub closed: u32,
}

impl super::UiScript {
    /// A test's taxi map, which the verbs read where the lent view does not answer it.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_taxi(&mut self, state: Option<TaxiUiState>) {
        set_field!(self, taxi, state);
    }
}

/// The listed nodes: the lent view's at the call; with none, a test's, else none.
fn with_map<R>(lua: &Lua, f: impl FnOnce(Option<&TaxiUiState>) -> R) -> R {
    let mut f = Some(f);
    if let Some(Some(r)) = super::live::read_live(|l| l.taxi_map().map(|m| (f.take().unwrap())(m)))
    {
        return r;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model");
        (f.take().unwrap())(model.taxi.as_ref())
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = lua;
        (f.take().unwrap())(None)
    }
}

/// A node by 0-based slot.
fn node(map: Option<&TaxiUiState>, slot: u32) -> Option<&TaxiUiNode> {
    map?.nodes.get(slot as usize)
}

/// The slot argument as the bindings take it: `is-number`, then the cast to an unsigned less one.
fn slot_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<u32> {
    Ok((binding_abi::number_arg(lua, v, usage)? as u32).wrapping_sub(1))
}

/// A bare `lua_tonumber` slot less one, as `GetNumRoutes` and the route-line getters take it.
fn bare_slot(lua: &Lua, v: Option<Value>) -> u32 {
    (binding_abi::coerced_number(lua, v) as i64 as i32 as u32).wrapping_sub(1)
}

/// Run `act` inside the call through the lent view; with none lent there is no map to act on.
fn taxi_act(act: TaxiAct) -> TaxiOutcome {
    super::live::write_live(|l| l.taxi_act(act))
        .flatten()
        .unwrap_or_default()
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x590710`: the listed count the last setup left.
    g.set(
        "NumTaxiNodes",
        lua.create_function(
            |lua, ()| Ok(with_map(lua, |m| m.map_or(0, |t| t.nodes.len())) as i64),
        )?,
    )?;

    // `0x590ed0` -> `0x590c70`: "NONE" out of range.
    g.set(
        "TaxiNodeGetType",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: TaxiNodeGetType(slot)")?;
            Ok(with_map(lua, |m| {
                node(m, slot).map_or("NONE", |n| n.node_type.era_str())
            }))
        })?,
    )?;

    // `0x590e00`: an out-of-range slot raises.
    g.set(
        "TaxiNodePosition",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: TaxiNodePosition(slot)")?;
            with_map(lua, |m| node(m, slot).map(|n| n.pos))
                .ok_or_else(|| mlua::Error::RuntimeError("Invalid taxi node slot".into()))
        })?,
    )?;

    // `0x590d60`: "INVALID" out of range.
    g.set(
        "TaxiNodeName",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: TaxiNodeName(slot)")?;
            Ok(with_map(lua, |m| {
                node(m, slot).map_or_else(|| "INVALID".to_string(), |n| n.name.clone())
            }))
        })?,
    )?;

    // `0x5915e0` -> `0x591350`: an out-of-range slot raises; the fare is priced at the call.
    g.set(
        "TaxiNodeCost",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: TaxiNodeCost(slot)")?;
            let Some(fallback) = with_map(lua, |m| node(m, slot).map(|n| n.cost)) else {
                return Err(mlua::Error::RuntimeError("Invalid taxi node slot".into()));
            };
            Ok(super::live::read_live(|l| l.taxi_node_cost(slot))
                .flatten()
                .unwrap_or(i64::from(fallback)))
        })?,
    )?;

    // `0x591e60` -> `0x591ad0`: the hovered node's route lines, which the setup already drew for
    // every node (`0x593771`); only its usage check is left.
    g.set(
        "TaxiNodeSetCurrent",
        lua.create_function(|lua, v: Value| {
            slot_arg(lua, v, "Usage: TaxiNodeSetCurrent(slot)")?;
            Ok(())
        })?,
    )?;

    // `0x5911c0`: 0 out of range.
    g.set(
        "GetNumRoutes",
        lua.create_function(|lua, v: Option<Value>| {
            let slot = bare_slot(lua, v);
            Ok(with_map(lua, |m| node(m, slot).map_or(0, |n| n.num_routes)))
        })?,
    )?;

    // One coordinate of 1-based line `hop` (`0x590f40`, `0x590fe0`, `0x591080`, `0x591120`): 0
    // out of range.
    for (name, pick) in [
        ("TaxiGetSrcX", 0usize),
        ("TaxiGetSrcY", 1),
        ("TaxiGetDestX", 2),
        ("TaxiGetDestY", 3),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, (i, hop): (Option<Value>, Option<Value>)| {
                let slot = bare_slot(lua, i);
                let line = bare_slot(lua, hop);
                Ok(with_map(lua, |m| {
                    node(m, slot)
                        .and_then(|n| n.routes.get(line as usize))
                        .map_or(0.0, |seg| seg[pick])
                }))
            })?,
        )?;
    }

    // `0x591680` -> `0x5914b0`: the activate or its refusal inside the call.
    g.set(
        "TakeTaxiNode",
        lua.create_function(|lua, v: Value| {
            let slot = slot_arg(lua, v, "Usage: TakeTaxiNode(slot)")?;
            if let Some(key) = taxi_act(TaxiAct::Take(slot)).error {
                super::ui_errors::show_now(lua, key);
            }
            Ok(())
        })?,
    )?;

    // `0x590ec0` -> `0x590ba0`: `TAXIMAP_CLOSED` inside the call, twice when the flight master
    // held the interact target.
    g.set(
        "CloseTaxiMap",
        lua.create_function(|lua, ()| {
            for _ in 0..taxi_act(TaxiAct::Close).closed {
                super::tick::fire_event_into(lua, "TAXIMAP_CLOSED", Vec::new());
            }
            Ok(())
        })?,
    )?;

    // `0x590d10`: the art the last setup loaded onto the given texture.
    g.set(
        "SetTaxiMap",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let art = with_map(lua, |m| m.map(|t| t.art.clone()));
            if let Some(art) = art {
                crate::script::model_mut_of(lua)
                    .region_data
                    .entry(rh)
                    .or_default()
                    .texture = Some(art);
            }
            Ok(())
        })?,
    )?;

    // The number 1 or nil, never a boolean; an argument neither string nor number raises the
    // reference's usage. Bit 20 of the named unit's live flags word (`0x60f396`).
    g.set(
        "UnitOnTaxi",
        lua.create_function(|lua, unit: mlua::Value| {
            let unit =
                crate::script::binding_abi::string_arg(lua, unit, r#"Usage: UnitOnTaxi("unit")"#)?;
            let guid = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model")
                .token_guid(&unit);
            let riding = crate::script::read_live(|live| {
                guid.and_then(|g| live.unit_flags(g))
                    .is_some_and(|f| f & UNIT_FLAG_TAXI_FLIGHT != 0)
            });
            Ok(crate::script::binding_abi::flag(riding == Some(true)))
        })?,
    )?;

    Ok(())
}
