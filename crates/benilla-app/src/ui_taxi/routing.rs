//! The taxi map's data: the DBC catalogs, the reference's map projection and route search, and
//! the node list they build.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use benilla_formats::{
    load_map_catalog, load_taxi_nodes, load_taxi_paths, load_world_map_continent_catalog,
    load_world_map_transform_catalog, MapCatalog, TaxiNode, TaxiNodes, TaxiPaths,
    WorldMapContinent, WorldMapContinentCatalog, WorldMapTransformCatalog,
};
use benilla_protocol::messages::{taxi_reply, TaxiMask};
use bevy::prelude::*;

use benilla_ui::script::{TaxiNodeType, TaxiUiNode};

use benilla_assets::{LockRecover, WorldAssets};

use super::TaxiOpen;

/// `TaxiNodes.dbc`, `TaxiPath.dbc` (the direct hops and their fares), `WorldMapContinent.dbc`
/// (each continent's taxi-map rect, its `taxi_min`/`taxi_max`), `WorldMapTransforms.dbc` (the
/// regions of map 530 drawn on the Eastern Kingdoms and Kalimdor) and `Map.dbc` (each map's
/// `ExpansionID`).
#[derive(Resource)]
pub(crate) struct TaxiCatalogs {
    nodes: TaxiNodes,
    pub(super) paths: TaxiPaths,
    continents: WorldMapContinentCatalog,
    transforms: WorldMapTransformCatalog,
    maps: MapCatalog,
}

impl TaxiCatalogs {
    /// The known-node mask's word count, `max TaxiNodes id / 64 + 1` (`0x593904`): 7 on 3.3.5a.
    pub(super) fn mask_words(&self) -> usize {
        (self.nodes.max_id() >> 6) as usize + 1
    }
}

/// Load [`TaxiCatalogs`] once, when the patch chain is open; nothing reads them before a flight
/// master opens the map.
pub(super) fn load_taxi_catalogs(
    mut done: Local<bool>,
    world_assets: Option<Res<WorldAssets>>,
    mut commands: Commands,
) {
    if *done {
        return;
    }
    let Some(assets) = world_assets else {
        return;
    };
    *done = true;
    let mut chain = assets.chain.lock_recover();
    let loaded = TaxiCatalogs::load(&mut chain);
    drop(chain);
    match loaded {
        Ok(cat) => {
            info!(
                "ui_taxi: catalogs loaded — {} nodes, {} paths, {} continents",
                cat.nodes.len(),
                cat.paths.len(),
                cat.continents.len()
            );
            commands.insert_resource(cat);
        }
        Err(e) => error!("ui_taxi: DBC catalogs failed to load, taxi map disabled: {e:#}"),
    }
}

impl TaxiCatalogs {
    /// The five tables off the patch chain.
    pub(super) fn load(chain: &mut benilla_formats::Chain) -> anyhow::Result<Self> {
        Ok(Self {
            nodes: load_taxi_nodes(chain)?,
            paths: load_taxi_paths(chain)?,
            continents: load_world_map_continent_catalog(chain)?,
            transforms: load_world_map_transform_catalog(chain)?,
            maps: load_map_catalog(chain)?,
        })
    }

    /// The map a node draws on and its position there: the world map transform (`0x543920`) on
    /// its `ContinentID` and world `(x, y)`, as the map's setup (`0x5932e0`), its node position
    /// reads (`0x590690`, `0x5906d0`) and its route lines (`0x591ad0`) take it. Silvermoon's map
    /// 530 position lands on the Eastern Kingdoms, the Exodar's on Kalimdor.
    fn node_on_map(&self, node: &TaxiNode) -> (u32, f32, f32) {
        let (map_id, x, y, _dungeon) =
            self.transforms
                .transform_position(node.map_id, node.pos[0], node.pos[1]);
        (map_id, x, y)
    }
}

/// A node's world `(x, y)` on the taxi map's 0..1 space, as the reference computes it, with the
/// rect from `WorldMapContinent.dbc` fields 9-12 of the node's continent:
///
/// ```text
/// u = (Ymax − worldY) / (Xmax − Xmin)    ; world Y (west+) → horizontal, inverted
/// v = (worldX − Xmin) / (Ymax − Ymin)    ; world X (north+) → vertical, from BOTTOMLEFT
/// ```
///
/// The denominators are cross-axis; the reference's route projector uses same-axis ones, and the
/// two agree because every shipped continent's rect is square.
pub(crate) fn project(cont: &WorldMapContinent, world_x: f32, world_y: f32) -> (f32, f32) {
    let (min_x, min_y) = cont.taxi_min;
    let (max_x, max_y) = cont.taxi_max;
    let x = (max_y - world_y) / (max_x - min_x);
    let y = (world_x - min_x) / (max_y - min_y);
    (x, y)
}

/// One outgoing edge, `(to, fare)`.
type Edge = (u32, u32);

/// The route from `from` to `to` through known nodes, minimizing summed distance with the fare
/// carried along, as the reference's relaxation does; ties go to fewer hops. `dist` is the per-edge
/// metric: `build_nodes` passes the straight 3-D distance between the two nodes, where the
/// reference sums the length of the path's own points. `from` counts as known: the server opens the
/// map only on a visited node. The chain runs `from` to `to`.
fn shortest_route(
    known: &TaxiMask,
    edges: impl Fn(u32) -> Vec<Edge>,
    dist: impl Fn(u32, u32) -> f32,
    from: u32,
    to: u32,
) -> Option<(Vec<u32>, u32)> {
    if from == to {
        return Some((vec![from], 0));
    }
    // Dijkstra keyed on (distance, hops): a non-negative `f32`'s bits order like its value, and
    // `Reverse` makes the max-heap a min-heap.
    let mut best: HashMap<u32, (u32, u32)> = HashMap::new(); // node → (dist_bits, hops)
    let mut fares: HashMap<u32, u32> = HashMap::new();
    let mut prev: HashMap<u32, u32> = HashMap::new();
    let mut heap = BinaryHeap::new();
    best.insert(from, (0, 0));
    fares.insert(from, 0);
    heap.push(Reverse((0u32, 0u32, from)));

    while let Some(Reverse((dist_bits, hops, node))) = heap.pop() {
        if node == to {
            break; // the first pop of `to` is optimal
        }
        if best.get(&node) != Some(&(dist_bits, hops)) {
            continue; // a stale entry: a better path already won
        }
        for (next, edge_fare) in edges(node) {
            if !known.is_known(next) {
                continue; // the search never steps through an undiscovered node
            }
            let leg = dist(node, next).max(0.0);
            let candidate = ((f32::from_bits(dist_bits) + leg).to_bits(), hops + 1);
            if best.get(&next).is_none_or(|&b| candidate < b) {
                best.insert(next, candidate);
                fares.insert(next, fares[&node] + edge_fare);
                prev.insert(next, node);
                heap.push(Reverse((candidate.0, candidate.1, next)));
            }
        }
    }

    best.get(&to)?;
    let total_fare = *fares.get(&to)?;
    let mut chain = vec![to];
    let mut cur = to;
    while cur != from {
        cur = *prev.get(&cur)?;
        chain.push(cur);
    }
    chain.reverse();
    Some((chain, total_fare))
}

/// `TaxiNodeCost`'s fare: the price times `1 - discount`, both in f64 with nothing stored (`fsubr`,
/// `fmul st,st(1)`), the product stored as an f32 and rounded half to even (`fstp dword`,
/// `fistp dword`).
fn shown_fare(price: u32, discount: f64) -> u32 {
    let product = (f64::from(price) * (1.0 - discount)) as f32;
    f64::from(product).round_ties_even() as u32
}

/// A listed node's chain from the nearest node (the node alone for `Current`) and its fare, for
/// `drain_taxi`.
pub(super) struct ResolvedTaxiNode {
    pub(super) chain: Vec<u32>,
    pub(super) cost: u32,
}

/// The pushed node list's routes, index for index, so `TakeTaxiNode(i)` finds its route.
#[derive(Resource, Default)]
pub(super) struct TaxiRouteCache(pub(super) Vec<ResolvedTaxiNode>);

/// The listed nodes, as the taxi map's setup (`0x5932e0`) lists them: the map is the one the
/// current node draws on, through `WorldMapTransforms`, and its `WorldMapContinent` row gives the
/// rect, refusing the map without a row or with a zero span. A known node is listed, in id order,
/// when it draws on that map and its own map's `ExpansionID` is within the account's; the setup
/// clears every other bit from the node mask, so a route never passes through an unlisted node,
/// as the route walk (`0x5926c0`) walks the listed table alone. Positions and route segments are
/// the transformed ones. The current node is `Current`, a routable one `Reachable` with its fare
/// and route segments, and an unroutable one is left out, where the reference keeps it typed
/// `NONE`, which stock `TaxiFrame.lua` hides; its `DISTANT` type is never produced. Returns the
/// continent's map id, the Lua nodes and their routes, in one order.
///
/// A Lua node's fare takes the flight master's price `discount` ([`shown_fare`]), and is 0 with
/// none, when the flight master or the player does not resolve; the route's fare stays the raw sum,
/// which `CMSG_ACTIVATETAXIEXPRESS` carries.
///
/// `account_expansion` is the account's expansion byte from `SMSG_AUTH_RESPONSE`.
pub(super) fn build_nodes(
    open: &TaxiOpen,
    cat: &TaxiCatalogs,
    discount: Option<f64>,
    account_expansion: u8,
) -> Option<(u32, Vec<TaxiUiNode>, Vec<ResolvedTaxiNode>)> {
    let (map_id, _, _) = cat.node_on_map(cat.nodes.get(open.nearest_node)?);
    let cont = cat.continents.get(map_id)?;
    if cont.taxi_max.0 - cont.taxi_min.0 == 0.0 || cont.taxi_max.1 - cont.taxi_min.1 == 0.0 {
        return None;
    }
    // The walk over the mask: each listed node with its transformed `(x, y)` and its own height.
    let mut rows: Vec<(&TaxiNode, [f32; 3])> = cat
        .nodes
        .rows()
        .filter(|n| open.known.is_known(n.id))
        .filter(|n| cat.maps.expansion(n.map_id) <= u32::from(account_expansion))
        .filter_map(|n| {
            let (on, x, y) = cat.node_on_map(n);
            (on == map_id).then_some((n, [x, y, n.pos[2]]))
        })
        .collect();
    rows.sort_by_key(|(n, _)| n.id);
    // No listed node, no map: the setup returns before `TAXIMAP_OPENED`.
    if rows.is_empty() {
        return None;
    }
    let mut listed = TaxiMask::default();
    let mut at: HashMap<u32, [f32; 3]> = HashMap::with_capacity(rows.len());
    for (n, p) in &rows {
        listed.set(n.id);
        at.insert(n.id, *p);
    }
    let dist = |a: u32, b: u32| -> f32 {
        match (at.get(&a), at.get(&b)) {
            (Some(a), Some(b)) => {
                let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
                (dx * dx + dy * dy + dz * dz).sqrt()
            }
            _ => f32::MAX / 4.0, // an edge into an unlisted node never wins
        }
    };

    let mut ui = Vec::new();
    let mut resolved = Vec::new();
    for (n, p) in rows {
        let pos = project(cont, p[0], p[1]);
        if n.id == open.nearest_node {
            ui.push(TaxiUiNode {
                name: n.name.clone(),
                node_type: TaxiNodeType::Current,
                pos,
                cost: 0,
                routes: Vec::new(),
            });
            resolved.push(ResolvedTaxiNode {
                chain: vec![n.id],
                cost: 0,
            });
            continue;
        }
        let Some((chain, cost)) = shortest_route(
            &listed,
            |from| cat.paths.paths_from(from).map(|p| (p.to, p.cost)).collect(),
            dist,
            open.nearest_node,
            n.id,
        ) else {
            continue; // unroutable: left out
        };
        // Each hop's segment for `GetNumRoutes` and `TaxiGetSrcX`..`TaxiGetDestY`, between
        // listed nodes' transformed positions.
        let routes = chain
            .windows(2)
            .filter_map(|w| {
                let (a, b) = (at.get(&w[0])?, at.get(&w[1])?);
                let (ax, ay) = project(cont, a[0], a[1]);
                let (bx, by) = project(cont, b[0], b[1]);
                Some([ax, ay, bx, by])
            })
            .collect();
        ui.push(TaxiUiNode {
            name: n.name.clone(),
            node_type: TaxiNodeType::Reachable,
            pos,
            cost: discount.map_or(0, |d| shown_fare(cost, d)),
            routes,
        });
        resolved.push(ResolvedTaxiNode { chain, cost });
    }
    Some((map_id, ui, resolved))
}

/// The message an `SMSG_ACTIVATETAXIREPLY` code shows: the reference's handler closes the map on 0,
/// shows the code's row of its message table through the error display for 1-12, and does nothing
/// from 13 up. The message row picks the surface, the yellow info line for seven of the twelve, and
/// the sound: `ERR_TAXINOTENOUGHMONEY` speaks line `0x36`.
pub(super) fn taxi_error_key(code: u32) -> Option<&'static str> {
    Some(match code {
        taxi_reply::OK => return None,
        taxi_reply::UNSPECIFIED_SERVER_ERROR => "ERR_TAXIUNSPECIFIEDSERVERERROR", // 0xac
        taxi_reply::NO_SUCH_PATH => "ERR_TAXINOSUCHPATH",                         // 0xab
        taxi_reply::NOT_ENOUGH_MONEY => "ERR_TAXINOTENOUGHMONEY",                 // 0xad
        taxi_reply::TOO_FAR => "ERR_TAXITOOFARAWAY",                              // 0xae
        taxi_reply::NO_VENDOR_NEARBY => "ERR_TAXINOVENDORNEARBY",                 // 0xaf
        taxi_reply::NOT_VISITED => "ERR_TAXINOTVISITED",                          // 0xb0
        taxi_reply::BUSY => "ERR_TAXIPLAYERBUSY",                                 // 0xb1
        taxi_reply::ALREADY_MOUNTED => "ERR_TAXIPLAYERALREADYMOUNTED",            // 0xb2
        taxi_reply::SHAPESHIFTED => "ERR_TAXIPLAYERSHAPESHIFTED",                 // 0xb3
        taxi_reply::PLAYER_MOVING => "ERR_TAXIPLAYERMOVING",                      // 0xb4
        taxi_reply::SAME_NODE => "ERR_TAXISAMENODE",                              // 0xaa
        taxi_reply::NOT_STANDING => "ERR_TAXINOTSTANDING",                        // 0xb6
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_of(ids: &[u32]) -> TaxiMask {
        let mut mask = TaxiMask::default();
        for &id in ids {
            mask.set(id);
        }
        mask
    }

    #[test]
    fn shortest_route_minimizes_geo_distance_and_carries_fare() {
        // Nodes 1, 2 and 4 at 0, 40 and 100 on a line. The direct 1→4 edge is 150 long for fare
        // 10; 1→2→4 is 40 + 60 = 100 long for fare 5 + 20 = 25, and wins.
        let positions: HashMap<u32, f32> = HashMap::from([(1, 0.0), (2, 40.0), (4, 100.0)]);
        let dists: HashMap<(u32, u32), f32> = HashMap::from([((1, 4), 150.0)]);
        let dist = |a: u32, b: u32| {
            dists
                .get(&(a, b))
                .copied()
                .unwrap_or_else(|| (positions[&a] - positions[&b]).abs())
        };
        let mut edges: HashMap<u32, Vec<Edge>> = HashMap::new();
        edges.insert(1, vec![(4, 10), (2, 5)]);
        edges.insert(2, vec![(4, 20)]);
        let known = mask_of(&[1, 2, 4]);
        let (chain, fare) = shortest_route(
            &known,
            |n| edges.get(&n).cloned().unwrap_or_default(),
            dist,
            1,
            4,
        )
        .expect("reachable");
        assert_eq!(
            chain,
            vec![1, 2, 4],
            "the geographically shorter detour wins regardless of fare"
        );
        assert_eq!(
            fare, 25,
            "the fare is the chosen chain's sum, not a minimum"
        );

        // An exact distance tie (direct 100 vs 40+60) breaks toward fewer hops.
        let (chain, fare) = shortest_route(
            &known,
            |n| edges.get(&n).cloned().unwrap_or_default(),
            |a, b| (positions[&a] - positions[&b]).abs(),
            1,
            4,
        )
        .expect("reachable");
        assert_eq!(chain, vec![1, 4], "an exact-tie breaks toward fewer hops");
        assert_eq!(fare, 10);

        // Node 5 has no edge from the known graph at all.
        assert!(
            shortest_route(
                &known,
                |n| edges.get(&n).cloned().unwrap_or_default(),
                dist,
                1,
                5
            )
            .is_none(),
            "a truly disconnected node has no route"
        );

        // An edge to node 3 exists, but 3 is not known.
        let mut gated: HashMap<u32, Vec<Edge>> = HashMap::new();
        gated.insert(1, vec![(3, 5)]);
        let known_without_3 = mask_of(&[1]);
        assert!(
            shortest_route(
                &known_without_3,
                |n| gated.get(&n).cloned().unwrap_or_default(),
                |_, _| 1.0,
                1,
                3
            )
            .is_none(),
            "an edge into an undiscovered node is not a usable route"
        );
    }

    /// The reference's rounding: `1 - discount` stays f64 and only the product is stored as f32, so
    /// a near-tie becomes an exact `.5` and rounds to even where the f64 shortcut rounds down.
    #[test]
    fn the_shown_fare_stores_only_the_product_as_f32() {
        let (d05, d10) = (f64::from(0.05f32), f64::from(0.1f32));
        let d15 = d10 + d05;
        // 0.1: 15 × 0.89999999850988388 = 13.4999999776, stored 13.5, to even 14; the f64
        // shortcut gives 13.
        assert_eq!(shown_fare(15, d10), 14);
        // 0.15: 30 × 0.84999999776482582 = 25.4999999329, stored 25.5, to even 26; and 110
        // (Stormwind to Sentinel Hill) = 93.4999997541, stored 93.5, to even 94.
        assert_eq!(shown_fare(30, d15), 26);
        assert_eq!(shown_fare(110, d15), 94);
        // 0.15 at 786437 stores 668471.4375, so 668471; repair's f32 multiplier gives 668472.
        assert_eq!(shown_fare(786_437, d15), 668_471);
        // 0.15 at 2097157 stores 1782583.5, to even 1782584; the discount narrowed to f32 first
        // (0.15000000596) would store 1782583.375, so 1782583.
        assert_eq!(shown_fare(2_097_157, d15), 1_782_584);
        // A plain case: 0.2 at 110 is 88, and no discount is the price.
        assert_eq!(shown_fare(110, d15 + d05), 88);
        assert_eq!(shown_fare(110, 0.0), 110);
    }

    /// A non-square rect, where cross-axis and same-axis denominators differ.
    #[test]
    fn projection_uses_cross_axis_denominators() {
        let cont = WorldMapContinent {
            map_id: 0,
            left_boundary: 0,
            right_boundary: 0,
            top_boundary: 0,
            bottom_boundary: 0,
            offset_x: 0.0,
            offset_y: 0.0,
            scale: 1.0,
            taxi_min: (0.0, 0.0),    // (Xmin, Ymin)
            taxi_max: (100.0, 50.0), // (Xmax, Ymax)
            world_map_id: 1,
        };
        // worldX 50, worldY 0: u = (50 - 0) / 100 = 0.5 and v = (50 - 0) / 50 = 1.0; same-axis
        // denominators would give the reverse.
        let (u, v) = project(&cont, 50.0, 0.0);
        assert!((u - 0.5).abs() < 1e-6, "u divides by the X-span (got {u})");
        assert!((v - 1.0).abs() < 1e-6, "v divides by the Y-span (got {v})");
    }

    /// Every node on maps 0 and 1 but id 3 (Programmer Isle, a debug row with no `TaxiPath` edge)
    /// projects inside the map, Ironforge above Stormwind and Menethil Harbor west of Lakeshire.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn real_taxi_projection_matches_geography() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let nodes = load_taxi_nodes(&mut chain).expect("load TaxiNodes");
        let continents = load_world_map_continent_catalog(&mut chain).expect("load WMC");

        for map_id in [0u32, 1u32] {
            let cont = continents.get(map_id).expect("continent row");
            for n in nodes.rows().filter(|n| n.map_id == map_id) {
                if n.id == 3 {
                    continue; // Programmer Isle
                }
                let (x, y) = project(cont, n.pos[0], n.pos[1]);
                assert!(
                    (0.02..=0.98).contains(&x) && (0.02..=0.98).contains(&y),
                    "{} (id {}, map {map_id}) projects out of bounds: ({x}, {y})",
                    n.name,
                    n.id
                );
            }
        }

        let ek = continents.get(0).expect("Eastern Kingdoms");
        let stormwind = nodes.get(2).expect("Stormwind");
        let ironforge = nodes.get(6).expect("Ironforge");
        let menethil = nodes.get(7).expect("Menethil Harbor");
        let lakeshire = nodes.get(5).expect("Lakeshire");
        assert_eq!(stormwind.name, "Stormwind, Elwynn");
        assert_eq!(ironforge.name, "Ironforge, Dun Morogh");
        assert_eq!(menethil.name, "Menethil Harbor, Wetlands");
        assert_eq!(lakeshire.name, "Lakeshire, Redridge");

        let (_, sw_y) = project(ek, stormwind.pos[0], stormwind.pos[1]);
        let (_, if_y) = project(ek, ironforge.pos[0], ironforge.pos[1]);
        assert!(
            if_y > sw_y,
            "Ironforge ({if_y}) should project above Stormwind ({sw_y}) — Dun Morogh is north"
        );

        let (men_x, _) = project(ek, menethil.pos[0], menethil.pos[1]);
        let (lake_x, _) = project(ek, lakeshire.pos[0], lakeshire.pos[1]);
        assert!(
            men_x < lake_x,
            "Menethil Harbor ({men_x}) should project west of Lakeshire ({lake_x}) — Wetlands is west"
        );
    }

    /// The real Stormwind (2) to Sentinel Hill (4) hop, `TaxiPath` id 6 at 110 copper; with every
    /// node known, the ones Stormwind cannot route to are left out.
    #[test]
    fn build_nodes_classifies_a_real_known_hop() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        let open = TaxiOpen {
            flightmaster: 0x42,
            nearest_node: 2,
            known: mask_of(&[2, 4]),
        };
        let (map_id, ui, resolved) = build_nodes(&open, &cat, Some(0.0), 2).expect("map builds");
        assert_eq!(map_id, 0, "the continent is the nearest node's own map");
        assert_eq!(ui.len(), 2, "only the two known EK nodes are visible");

        let sw_idx = ui
            .iter()
            .position(|n| n.name == "Stormwind, Elwynn")
            .expect("Stormwind visible");
        assert_eq!(ui[sw_idx].node_type, TaxiNodeType::Current);
        assert_eq!(resolved[sw_idx].chain, vec![2]);

        let sh_idx = ui
            .iter()
            .position(|n| n.name == "Sentinel Hill, Westfall")
            .expect("Sentinel Hill visible");
        assert_eq!(ui[sh_idx].node_type, TaxiNodeType::Reachable);
        assert_eq!(ui[sh_idx].cost, 110);
        assert_eq!(ui[sh_idx].routes.len(), 1);
        assert_eq!(resolved[sh_idx].chain, vec![2, 4]);
        assert_eq!(resolved[sh_idx].cost, 110);

        // Every node known: the Horde-only stops (Grom'gol, Kargath) have no route from Stormwind.
        let all_known = TaxiMask(vec![u64::MAX; 7]);
        let open = TaxiOpen {
            flightmaster: 0x42,
            nearest_node: 2,
            known: all_known,
        };
        let (_, ui, _) = build_nodes(&open, &cat, Some(0.0), 2).expect("map builds");
        let ek_known_total = cat.nodes.rows().filter(|n| n.map_id == 0).count();
        assert!(
            ui.len() < ek_known_total,
            "some all-known EK nodes are unroutable from Stormwind and must be dropped \
             ({} shown of {ek_known_total})",
            ui.len()
        );
        assert!(
            ui.iter().all(|n| n.node_type != TaxiNodeType::Distant),
            "the DISTANT classification is a dead branch — never produced"
        );
    }

    /// A node's id by its `TaxiNodes` name.
    fn node_named(cat: &TaxiCatalogs, name: &str) -> u32 {
        cat.nodes
            .rows()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("no TaxiNodes row {name:?}"))
            .id
    }

    /// The map's setup (`0x5932e0`) through `WorldMapTransforms`: Silvermoon's flight master, on
    /// map 530, opens the Eastern Kingdoms map with Tranquillien, the Exodar's opens Kalimdor with
    /// Blood Watch, and Shattrath's Outland map lists neither even with every node known. Each
    /// listed node sits where its transformed position projects on its map's rect.
    #[test]
    fn the_blood_elf_and_draenei_flight_points_open_on_their_continents() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        let silvermoon = node_named(&cat, "Silvermoon City");
        let tranquillien = node_named(&cat, "Tranquillien, Ghostlands");
        let exodar = node_named(&cat, "The Exodar");
        let blood_watch = node_named(&cat, "Blood Watch, Bloodmyst Isle");
        let shattrath = node_named(&cat, "Shattrath, Terokkar Forest");
        let all_known = TaxiMask(vec![u64::MAX; cat.mask_words()]);
        let open_at = |node: u32, known: &TaxiMask| TaxiOpen {
            flightmaster: 0x42,
            nearest_node: node,
            known: known.clone(),
        };
        let names = |ui: &[TaxiUiNode]| ui.iter().map(|n| n.name.clone()).collect::<Vec<_>>();

        // Silvermoon: the Eastern Kingdoms, Tranquillien reachable, at its transformed spot.
        let known = mask_of(&[silvermoon, tranquillien]);
        let (map_id, ui, resolved) =
            build_nodes(&open_at(silvermoon, &known), &cat, Some(0.0), 2).expect("map builds");
        assert_eq!(
            map_id, 0,
            "Silvermoon's map 530 position draws on the Eastern Kingdoms"
        );
        assert_eq!(names(&ui), ["Silvermoon City", "Tranquillien, Ghostlands"]);
        assert_eq!(ui[0].node_type, TaxiNodeType::Current);
        assert_eq!(ui[1].node_type, TaxiNodeType::Reachable);
        assert_eq!(resolved[1].chain, vec![silvermoon, tranquillien]);
        let ek = cat.continents.get(0).expect("the Eastern Kingdoms row");
        let n = cat.nodes.get(tranquillien).unwrap();
        let (_, x, y, _) = cat
            .transforms
            .transform_position(n.map_id, n.pos[0], n.pos[1]);
        assert_eq!(
            ui[1].pos,
            project(ek, x, y),
            "the transformed position, on the EK rect"
        );
        for (u, v) in ui.iter().map(|n| n.pos) {
            assert!(
                (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v),
                "on the map ({u}, {v})"
            );
        }
        assert_eq!(
            ui[1].routes,
            vec![[ui[0].pos.0, ui[0].pos.1, ui[1].pos.0, ui[1].pos.1]]
        );

        // Every node known from Silvermoon: Outland's stay off the Eastern Kingdoms map.
        let (_, ui, _) =
            build_nodes(&open_at(silvermoon, &all_known), &cat, Some(0.0), 2).expect("map builds");
        assert!(!names(&ui).contains(&"Shattrath, Terokkar Forest".to_string()));

        // The Exodar: Kalimdor, Blood Watch reachable.
        let known = mask_of(&[exodar, blood_watch]);
        let (map_id, ui, _) =
            build_nodes(&open_at(exodar, &known), &cat, Some(0.0), 2).expect("map builds");
        assert_eq!(map_id, 1, "the Exodar's map 530 position draws on Kalimdor");
        assert_eq!(names(&ui), ["Blood Watch, Bloodmyst Isle", "The Exodar"]);
        assert_eq!(ui[0].node_type, TaxiNodeType::Reachable);

        // Shattrath, every node known: Outland, with none of the transformed nodes.
        let (map_id, ui, _) =
            build_nodes(&open_at(shattrath, &all_known), &cat, Some(0.0), 2).expect("map builds");
        assert_eq!(map_id, 530);
        let listed = names(&ui);
        for gone in [
            "Silvermoon City",
            "Tranquillien, Ghostlands",
            "The Exodar",
            "Blood Watch, Bloodmyst Isle",
            "Shattered Sun Staging Area",
        ] {
            assert!(
                !listed.contains(&gone.to_string()),
                "{gone} is not on Outland's map"
            );
        }
        assert!(listed.contains(&"Shattrath, Terokkar Forest".to_string()));
    }

    /// The setup clears a known node whose own map's `ExpansionID` is past the account's: on a
    /// classic account (0) Silvermoon's map lists no node at all, and a map with none never opens.
    #[test]
    fn a_node_on_a_map_past_the_accounts_expansion_is_not_listed() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        let silvermoon = node_named(&cat, "Silvermoon City");
        let tranquillien = node_named(&cat, "Tranquillien, Ghostlands");
        let open = TaxiOpen {
            flightmaster: 0x42,
            nearest_node: silvermoon,
            known: mask_of(&[silvermoon, tranquillien, 2]),
        };
        assert!(
            build_nodes(&open, &cat, Some(0.0), 0).is_none(),
            "map 530 is expansion 1, so nothing is listed and the map does not open"
        );
        let (_, ui, _) = build_nodes(&open, &cat, Some(0.0), 1).expect("map builds");
        assert!(ui.iter().any(|n| n.name == "Tranquillien, Ghostlands"));
    }
}
