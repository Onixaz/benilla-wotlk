//! The taxi map's data: the DBC catalogs, the reference's map projection and route search, and
//! the node list they build.

use std::collections::HashMap;

use benilla_formats::{
    load_map_catalog, load_taxi_nodes, load_taxi_path_nodes, load_taxi_paths,
    load_world_map_continent_catalog, load_world_map_transform_catalog, MapCatalog, TaxiNode,
    TaxiNodes, TaxiPathNodes, TaxiPaths, WorldMapContinent, WorldMapContinentCatalog,
    WorldMapTransformCatalog,
};
use benilla_protocol::messages::taxi_reply;
use bevy::prelude::*;

use benilla_ui::script::{TaxiNodeType, TaxiUiNode};

use benilla_assets::{LockRecover, WorldAssets};

use super::TaxiOpen;

/// `TaxiNodes.dbc`, `TaxiPath.dbc` (the direct hops and their fares), `TaxiPathNode.dbc` (each
/// hop's points, which its length sums), `WorldMapContinent.dbc`
/// (each continent's taxi-map rect, its `taxi_min`/`taxi_max`), `WorldMapTransforms.dbc` (the
/// regions of map 530 drawn on the Eastern Kingdoms and Kalimdor) and `Map.dbc` (each map's
/// `ExpansionID`).
#[derive(Resource)]
pub(crate) struct TaxiCatalogs {
    nodes: TaxiNodes,
    pub(super) paths: TaxiPaths,
    path_nodes: TaxiPathNodes,
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
    /// The six tables off the patch chain.
    pub(super) fn load(chain: &mut benilla_formats::Chain) -> anyhow::Result<Self> {
        Ok(Self {
            nodes: load_taxi_nodes(chain)?,
            paths: load_taxi_paths(chain)?,
            path_nodes: load_taxi_path_nodes(chain)?,
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

    /// A hop's length, as the reference measures it (`0x592580`): the length of the direct hop's
    /// `TaxiPathNode` points, each through the world map transform on its own map, summed into
    /// an f32 point by point (`fsqrt`, `fadd dword`, `fstp dword`); [`NO_PATH_LENGTH`] with no hop
    /// or no points. The squares are summed in f64 here, where the reference keeps them on the x87
    /// stack.
    fn path_length(&self, from: u32, to: u32) -> f32 {
        let Some(points) = self
            .paths
            .between(from, to)
            .and_then(|hop| self.path_nodes.path(hop.id))
        else {
            return NO_PATH_LENGTH;
        };
        let at = |p: &benilla_formats::TaxiPathNode| {
            let (_, x, y, _) = self
                .transforms
                .transform_position(p.map_id, p.pos[0], p.pos[1]);
            (f64::from(x), f64::from(y), f64::from(p.pos[2]))
        };
        let mut distance = 0.0f32;
        for w in points.windows(2) {
            let ((px, py, pz), (x, y, z)) = (at(&w[0]), at(&w[1]));
            let (dx, dy, dz) = (x - px, y - py, z - pz);
            distance = ((dy * dy + dz * dz + dx * dx).sqrt() + f64::from(distance)) as f32;
        }
        distance
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

/// The hop length (`0x592580`) of a hop with no `TaxiPath` row or no `TaxiPathNode`
/// points (`fld [0xa13b04]`, 1e11 as an f32).
const NO_PATH_LENGTH: f32 = 1.0e11;

/// The step the route walk (`0x5926c0`) takes off each fare tier's scale (`fsub [0x9f1958]`, the
/// f32 0.05).
const FARE_TIER_STEP: f32 = 0.05;

/// What the map's setup (`0x5932e0`) resets on each listed node: fares 999999, length 1e8, no
/// path.
const UNWALKED: Walked = Walked {
    path: None,
    costs: [999_999; 5],
    distance: 100_000_000.0,
};

/// What the route walk (`0x5926c0`) leaves on one listed node (its record's `+0x10` to `+0x2c`):
/// the chain from the current node (with its hop count, and the reached flag set with it), the
/// chain's fare at each of the five fare tiers and its length.
#[derive(Clone, Debug, PartialEq)]
struct Walked {
    path: Option<Vec<u32>>,
    costs: [i32; 5],
    distance: f32,
}

/// The graph the route walk (`0x592bb0`) covers: the listed nodes alone.
struct TaxiGraph<'a> {
    /// The listed node ids, in the order the walk visits them (the listed table, by id).
    ids: &'a [u32],
    /// The direct hop's fare, when a `TaxiPath` row joins the two (`0x590b60`).
    hop: &'a dyn Fn(u32, u32) -> Option<u32>,
    /// The hop's length (`0x592580`).
    length: &'a dyn Fn(u32, u32) -> f32,
    /// The node the walk returns on at once, its taxi mounts the other team's alone.
    barred: &'a dyn Fn(u32) -> bool,
}

/// The route walk (`0x592bb0`): the current node holds the chain of itself at length 0
/// and fare 0, and every direct hop out of it starts a [`extend_route`]. The start is the first
/// listed node holding the current id, else the first listed node, as the reference's search
/// leaves it.
fn walk_routes(graph: &TaxiGraph, current: u32) -> Vec<Walked> {
    let mut walked = vec![UNWALKED; graph.ids.len()];
    let start = graph.ids.iter().position(|&id| id == current).unwrap_or(0);
    walked[start] = Walked {
        path: Some(vec![current]),
        costs: [0; 5],
        distance: 0.0,
    };
    for (i, &to) in graph.ids.iter().enumerate() {
        if let Some(fare) = (graph.hop)(current, to) {
            extend_route(graph, &mut walked, i, start, fare);
        }
    }
    walked
}

/// One step of the route walk (`0x5926c0`), a depth-first relaxation: the node reached from
/// `parent` by a hop of fare `fare` takes the parent's chain plus itself when it has none yet or
/// the new length is shorter (an equal one keeps the first found), then walks every direct hop
/// out of it, in listed order. A node whose mounts are the other team's alone is never reached.
///
/// The length is the parent's plus the hop's, compared unrounded and stored as an f32 (`fadd`,
/// `fst dword`, `fcomp`). Each fare tier `j` adds the hop's fare times its scale, 1 at tier 0 and
/// 0.05 less at each next one: the product is stored as an f32 and rounded half to even (`fmul`,
/// `fstp dword`, `fistp`), and the next scale is the stored f32 scale less 0.05 (`fst dword`,
/// `fsub`), which the `fmul` takes unrounded. The x87 sums are taken in f64 here.
fn extend_route(graph: &TaxiGraph, walked: &mut [Walked], index: usize, parent: usize, fare: u32) {
    let id = graph.ids[index];
    if (graph.barred)(id) {
        return;
    }
    let Some(mut path) = walked[parent].path.clone() else {
        return; // a parent is always walked before its hops; nothing to extend otherwise
    };
    let (parent_costs, parent_distance) = (walked[parent].costs, walked[parent].distance);
    let distance = f64::from((graph.length)(graph.ids[parent], id)) + f64::from(parent_distance);
    let info = &mut walked[index];
    if info.path.is_some() && distance >= f64::from(info.distance) {
        return;
    }
    path.push(id);
    info.path = Some(path);
    // `fild dword`: the fare as an i32, then stored as an f32 (`fstp dword [ebp-4]`).
    let fare = fare as i32 as f32;
    let mut scale = 1.0f64;
    for (cost, parent_cost) in info.costs.iter_mut().zip(parent_costs) {
        let leg = f64::from((scale * f64::from(fare)) as f32).round_ties_even() as i32;
        *cost = parent_cost.wrapping_add(leg);
        scale = f64::from(scale as f32) - f64::from(FARE_TIER_STEP);
    }
    info.distance = distance as f32;
    for (j, &to) in graph.ids.iter().enumerate() {
        if let Some(next_fare) = (graph.hop)(id, to) {
            extend_route(graph, walked, j, index, next_fare);
        }
    }
}

/// The creature entries the route walk reads as Horde taxi mounts (`0x592755`..`0x59288f`).
const HORDE_TAXI_MOUNTS: [u32; 4] = [2224, 3574, 19917, 34709];
/// The creature entries it reads as Alliance taxi mounts.
const ALLIANCE_TAXI_MOUNTS: [u32; 5] = [3837, 541, 28360, 28361, 31315];

/// The route walk's team check: a player whose `FactionTemplate` group is 3 (player and Alliance)
/// never reaches a node whose two mount slots hold a Horde mount and no Alliance one, and a group
/// of 5 (player and Horde) the reverse. Any other group, or no player, reaches every node.
fn barred_for_team(player_group: Option<u32>, node: &TaxiNode) -> bool {
    let mounts = [node.mount_horde, node.mount_alliance];
    let horde = mounts.iter().any(|m| HORDE_TAXI_MOUNTS.contains(m));
    let alliance = mounts.iter().any(|m| ALLIANCE_TAXI_MOUNTS.contains(m));
    match player_group {
        Some(3) => horde && !alliance,
        Some(5) => alliance && !horde,
        _ => false,
    }
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

/// What the Lua `TaxiNodeCost`'s fare (`0x591350`) reads besides the route, known only once the
/// flight master and the player resolve: the flight master's price discount for the player, which
/// a direct hop's fare takes ([`shown_fare`]), and the fare tier a longer route's sum is read at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TaxiFares {
    pub(super) discount: f64,
    /// `reaction - 3` for a reaction above Neutral, else 0: the flight master's reaction toward
    /// the player (`0x7251c0`), read as Neutral when its faction has no reputation slot
    /// (`0x718b30`). The reaction caps at Revered, so 0 to 3.
    pub(super) tier: usize,
}

impl TaxiFares {
    /// The tier for a reaction on the 0..=6 scale.
    pub(super) fn tier_of(reaction: u8, has_reputation: bool) -> usize {
        let reaction = if has_reputation { reaction } else { 3 };
        usize::from(reaction.saturating_sub(3)).min(4)
    }
}

/// The listed nodes, as the taxi map's setup (`0x5932e0`) lists them: the map is the one the
/// current node draws on, through `WorldMapTransforms`, and its `WorldMapContinent` row gives the
/// rect, refusing the map without a row or with a zero span. A known node is listed, in id order,
/// when it draws on that map and its own map's `ExpansionID` is within the account's; the setup
/// clears every other bit from the node mask, so a route never passes through an unlisted node,
/// as the route walk walks the listed table alone. Positions and route segments are the
/// transformed ones.
///
/// The routes are the walk's ([`walk_routes`]). Each listed node then reads as the stock Lua
/// queries read it:
/// - The current node is `Current`, with no segment and fare 0.
/// - A node the walk never reached is left out, where the reference keeps it typed `NONE`, which
///   stock `TaxiFrame.lua` hides. Its `DISTANT` type is never produced.
/// - A node with a direct `TaxiPath` hop from the current one draws that one segment (`0x591ad0`;
///   `GetNumRoutes` is 1), costs that hop's fare less the discount, and flies it (`TakeTaxiNode`,
///   `0x5914b0`), whatever chain the walk found shorter.
/// - Any other reached node draws its chain's segments, costs the chain's sum at the fare tier,
///   and flies the chain.
///
/// The fares are 0 without [`TaxiFares`], as `TaxiNodeCost` returns 0 when the player or the
/// flight master does not resolve. The route's own fare is the chain's tier-0 sum, the raw fare,
/// for the activate's log.
///
/// `player_group` is the player's `FactionTemplate` group, for the walk's team check
/// ([`barred_for_team`]), and `account_expansion` the account's expansion byte from
/// `SMSG_AUTH_RESPONSE`.
pub(super) fn build_nodes(
    open: &TaxiOpen,
    cat: &TaxiCatalogs,
    fares: Option<TaxiFares>,
    player_group: Option<u32>,
    account_expansion: u8,
) -> Option<(u32, Vec<TaxiUiNode>, Vec<ResolvedTaxiNode>)> {
    let (map_id, _, _) = cat.node_on_map(cat.nodes.get(open.nearest_node)?);
    let cont = cat.continents.get(map_id)?;
    if cont.taxi_max.0 - cont.taxi_min.0 == 0.0 || cont.taxi_max.1 - cont.taxi_min.1 == 0.0 {
        return None;
    }
    // The walk over the mask: each listed node with its transformed `(x, y)`.
    let mut rows: Vec<(&TaxiNode, (f32, f32))> = cat
        .nodes
        .rows()
        .filter(|n| open.known.is_known(n.id))
        .filter(|n| cat.maps.expansion(n.map_id) <= u32::from(account_expansion))
        .filter_map(|n| {
            let (on, x, y) = cat.node_on_map(n);
            (on == map_id).then_some((n, (x, y)))
        })
        .collect();
    rows.sort_by_key(|(n, _)| n.id);
    // No listed node, no map: the setup returns before `TAXIMAP_OPENED`.
    if rows.is_empty() {
        return None;
    }
    let ids: Vec<u32> = rows.iter().map(|(n, _)| n.id).collect();
    let pos: HashMap<u32, (f32, f32)> = rows
        .iter()
        .map(|(n, (x, y))| (n.id, project(cont, *x, *y)))
        .collect();
    let by_id: HashMap<u32, &TaxiNode> = rows.iter().map(|(n, _)| (n.id, *n)).collect();

    let hop = |from: u32, to: u32| cat.paths.between(from, to).map(|p| p.cost);
    let length = |from: u32, to: u32| cat.path_length(from, to);
    let barred = |id: u32| {
        by_id
            .get(&id)
            .is_some_and(|n| barred_for_team(player_group, n))
    };
    let walked = walk_routes(
        &TaxiGraph {
            ids: &ids,
            hop: &hop,
            length: &length,
            barred: &barred,
        },
        open.nearest_node,
    );

    let current = open.nearest_node;
    let segment = |a: u32, b: u32| {
        let ((ax, ay), (bx, by)) = (pos.get(&a)?, pos.get(&b)?);
        Some([*ax, *ay, *bx, *by])
    };
    let mut ui = Vec::new();
    let mut resolved = Vec::new();
    for ((n, _), walk) in rows.iter().zip(walked) {
        let at = pos[&n.id];
        if n.id == current {
            ui.push(TaxiUiNode {
                name: n.name.clone(),
                node_type: TaxiNodeType::Current,
                pos: at,
                cost: 0,
                routes: Vec::new(),
            });
            resolved.push(ResolvedTaxiNode {
                chain: vec![n.id],
                cost: 0,
            });
            continue;
        }
        let Some(path) = walk.path else {
            continue; // never reached: `NONE`, left out
        };
        let (routes, shown, chain, cost) = match cat.paths.between(current, n.id) {
            // The direct hop: one segment, its own fare, flown as `CMSG_ACTIVATETAXI`.
            Some(direct) => (
                segment(current, n.id).into_iter().collect(),
                fares.map_or(0, |f| shown_fare(direct.cost, f.discount)),
                vec![current, n.id],
                direct.cost,
            ),
            // The chain, hop by hop (`0x590c30`: one fewer segment than nodes).
            None => (
                path.windows(2)
                    .filter_map(|w| segment(w[0], w[1]))
                    .collect(),
                fares.map_or(0, |f| tier_fare(&walk.costs, f.tier)),
                path,
                tier_fare(&walk.costs, 0),
            ),
        };
        ui.push(TaxiUiNode {
            name: n.name.clone(),
            node_type: TaxiNodeType::Reachable,
            pos: at,
            cost: shown,
            routes,
        });
        resolved.push(ResolvedTaxiNode { chain, cost });
    }
    Some((map_id, ui, resolved))
}

/// A chain's fare at `tier`, as the `int` `TaxiNodeCost` returns.
fn tier_fare(costs: &[i32; 5], tier: usize) -> u32 {
    u32::try_from(costs[tier.min(4)]).unwrap_or(0)
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
    use benilla_protocol::messages::TaxiMask;

    /// No discount, tier 0: the raw fares.
    const NEUTRAL: Option<TaxiFares> = Some(TaxiFares {
        discount: 0.0,
        tier: 0,
    });

    fn mask_of(ids: &[u32]) -> TaxiMask {
        let mut mask = TaxiMask::default();
        for &id in ids {
            mask.set(id);
        }
        mask
    }

    /// A graph over `ids` whose hops are `(from, to, fare, length)` rows, with `barred` ids.
    fn walk(
        ids: &[u32],
        hops: &[(u32, u32, u32, f32)],
        barred: &[u32],
        current: u32,
    ) -> Vec<Walked> {
        let hop = |a: u32, b: u32| hops.iter().find(|h| (h.0, h.1) == (a, b)).map(|h| h.2);
        let length = |a: u32, b: u32| {
            hops.iter()
                .find(|h| (h.0, h.1) == (a, b))
                .map_or(NO_PATH_LENGTH, |h| h.3)
        };
        let barred = |id: u32| barred.contains(&id);
        walk_routes(
            &TaxiGraph {
                ids,
                hop: &hop,
                length: &length,
                barred: &barred,
            },
            current,
        )
    }

    /// The route walk keeps the shortest chain by path length, whatever its hop count or fare: the
    /// direct 1 to 4 hop is 150 long, 1 to 2 to 4 is 40 + 60, so 4 takes the detour, with its
    /// fare summed and its length kept.
    #[test]
    fn the_walk_keeps_the_shortest_chain_by_path_length() {
        let hops = [(1, 4, 10, 150.0), (1, 2, 5, 40.0), (2, 4, 20, 60.0)];
        let walked = walk(&[1, 2, 4], &hops, &[], 1);
        assert_eq!(
            walked[0].path,
            Some(vec![1]),
            "the current node is its own chain"
        );
        assert_eq!(walked[0].costs, [0; 5]);
        assert_eq!(walked[1].path, Some(vec![1, 2]));
        assert_eq!(walked[2].path, Some(vec![1, 2, 4]));
        assert_eq!(walked[2].costs[0], 25, "tier 0 is the raw sum");
        assert_eq!(walked[2].distance, 100.0);

        // A tie keeps the chain found first: the walk takes 1 to 2 (listed before 4) and reaches
        // 4 through it at 100 before the direct hop, also 100, is tried.
        let tied = [(1, 4, 10, 100.0), (1, 2, 5, 40.0), (2, 4, 20, 60.0)];
        assert_eq!(walk(&[1, 2, 4], &tied, &[], 1)[2].path, Some(vec![1, 2, 4]));
        // Listed the other way round, the direct hop is found first and kept.
        assert_eq!(walk(&[1, 4, 2], &tied, &[], 1)[1].path, Some(vec![1, 4]));
    }

    /// A node the walk bars is never reached, so nothing routes through it; one the walk has
    /// no hop into stays unwalked.
    #[test]
    fn a_barred_node_is_never_reached_nor_routed_through() {
        let hops = [(1, 4, 10, 150.0), (1, 2, 5, 40.0), (2, 4, 20, 60.0)];
        let walked = walk(&[1, 2, 4, 9], &hops, &[2], 1);
        assert_eq!(walked[1], UNWALKED, "2 is barred");
        assert_eq!(
            walked[2].path,
            Some(vec![1, 4]),
            "so 4 flies the direct hop"
        );
        assert_eq!(walked[3], UNWALKED, "no hop reaches 9");
    }

    /// Each fare tier scales every hop's fare, rounding each product half to even: Stormwind to
    /// Ironforge (50) to Thelsamar (110) is 160, 152, 144, 136 and 128 copper. Tier 1's 50 × 0.95 is
    /// 47.4999999627, stored as the f32 47.5 and rounded to 48.
    #[test]
    fn the_fare_tiers_round_each_hop_as_the_reference_does() {
        let hops = [(2, 6, 50, 1.0), (6, 8, 110, 1.0)];
        let walked = walk(&[2, 6, 8], &hops, &[], 2);
        assert_eq!(walked[1].costs, [50, 48, 45, 42, 40]);
        assert_eq!(walked[2].costs, [160, 152, 144, 136, 128]);
        assert_eq!(tier_fare(&walked[2].costs, 2), 144);
        // The smaller fares: 15 and 30, each tier's own rounding.
        let walked = walk(&[1, 2], &[(1, 2, 15, 1.0)], &[], 1);
        assert_eq!(walked[1].costs, [15, 14, 14, 13, 12]);
        let walked = walk(&[1, 2], &[(1, 2, 30, 1.0)], &[], 1);
        assert_eq!(walked[1].costs, [30, 28, 27, 26, 24]);
    }

    /// The tier is `reaction - 3` above Neutral, else 0, and Neutral's 0 for a flight master whose
    /// faction has no reputation slot.
    #[test]
    fn the_fare_tier_is_the_reaction_above_neutral() {
        assert_eq!(TaxiFares::tier_of(1, true), 0, "hostile");
        assert_eq!(TaxiFares::tier_of(3, true), 0, "neutral");
        assert_eq!(TaxiFares::tier_of(4, true), 1, "friendly");
        assert_eq!(TaxiFares::tier_of(5, true), 2, "honored");
        assert_eq!(TaxiFares::tier_of(6, true), 3, "revered, the cap");
        assert_eq!(
            TaxiFares::tier_of(6, false),
            0,
            "no reputation slot reads Neutral"
        );
    }

    /// The walk's team check on the real mounts: Stormwind's Alliance gryphon (541) bars an
    /// Horde player only, a Horde wyvern (2224) an Alliance one, and a node with both bars no one.
    #[test]
    fn the_team_check_bars_the_other_teams_nodes_alone() {
        let node = |horde: u32, alliance: u32| TaxiNode {
            id: 1,
            map_id: 0,
            pos: [0.0; 3],
            name: String::new(),
            mount_horde: horde,
            mount_alliance: alliance,
        };
        let (alliance_only, horde_only, both) = (node(0, 541), node(2224, 0), node(2224, 541));
        for (group, barred) in [
            (Some(3), [false, true, false]),
            (Some(5), [true, false, false]),
            (Some(1), [false; 3]),
            (None, [false; 3]),
        ] {
            assert_eq!(
                [&alliance_only, &horde_only, &both].map(|n| barred_for_team(group, n)),
                barred,
                "group {group:?}"
            );
        }
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
        let (map_id, ui, resolved) =
            build_nodes(&open, &cat, NEUTRAL, None, 2).expect("map builds");
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
        let (_, ui, _) = build_nodes(&open, &cat, NEUTRAL, None, 2).expect("map builds");
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
            build_nodes(&open_at(silvermoon, &known), &cat, NEUTRAL, None, 2).expect("map builds");
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
        let (_, ui, _) = build_nodes(&open_at(silvermoon, &all_known), &cat, NEUTRAL, None, 2)
            .expect("map builds");
        assert!(!names(&ui).contains(&"Shattrath, Terokkar Forest".to_string()));

        // The Exodar: Kalimdor, Blood Watch reachable.
        let known = mask_of(&[exodar, blood_watch]);
        let (map_id, ui, _) =
            build_nodes(&open_at(exodar, &known), &cat, NEUTRAL, None, 2).expect("map builds");
        assert_eq!(map_id, 1, "the Exodar's map 530 position draws on Kalimdor");
        assert_eq!(names(&ui), ["Blood Watch, Bloodmyst Isle", "The Exodar"]);
        assert_eq!(ui[0].node_type, TaxiNodeType::Reachable);

        // Shattrath, every node known: Outland, with none of the transformed nodes.
        let (map_id, ui, _) = build_nodes(&open_at(shattrath, &all_known), &cat, NEUTRAL, None, 2)
            .expect("map builds");
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
            known: mask_of(&[silvermoon, tranquillien]),
        };
        assert!(
            build_nodes(&open, &cat, NEUTRAL, None, 0).is_none(),
            "map 530 is expansion 1, so nothing is listed and the map does not open"
        );
        let (_, ui, _) = build_nodes(&open, &cat, NEUTRAL, None, 1).expect("map builds");
        assert!(ui.iter().any(|n| n.name == "Tranquillien, Ghostlands"));
    }

    /// The direct line (`0x591ad0`): from Ironforge, with every node known, the walk's
    /// shortest chain to Stormwind runs through Thorium Point (the path-length sum), but a direct
    /// `TaxiPath` hop joins the two, so the map draws that one segment, shows its fare and flies
    /// it. Every other node with a direct hop draws one segment too.
    #[test]
    fn a_node_with_a_direct_hop_draws_one_line_whatever_the_shortest_chain() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        let (ironforge, stormwind) = (6, 2);
        let all_known = TaxiMask(vec![u64::MAX; cat.mask_words()]);
        let open = TaxiOpen {
            flightmaster: 0x42,
            nearest_node: ironforge,
            known: all_known,
        };
        let (_, ui, resolved) = build_nodes(&open, &cat, NEUTRAL, None, 2).expect("map builds");
        let at = |name: &str| ui.iter().position(|n| n.name == name).expect(name);
        let (if_idx, sw_idx) = (at("Ironforge, Dun Morogh"), at("Stormwind, Elwynn"));

        // The walk itself detours.
        // Each listed node's id is its chain's last; names repeat (two Thorium Points).
        let ids: Vec<u32> = resolved
            .iter()
            .map(|r| *r.chain.last().expect("a chain"))
            .collect();
        let hop = |a: u32, b: u32| cat.paths.between(a, b).map(|p| p.cost);
        let length = |a: u32, b: u32| cat.path_length(a, b);
        let walked = walk_routes(
            &TaxiGraph {
                ids: &ids,
                hop: &hop,
                length: &length,
                barred: &|_| false,
            },
            ironforge,
        );
        let detour = walked[sw_idx].path.as_ref().expect("Stormwind is reached");
        assert!(
            detour.len() > 2,
            "the shortest chain by path length is not the direct hop: {detour:?}"
        );

        // The map draws, prices and flies the direct hop.
        let direct = cat
            .paths
            .between(ironforge, stormwind)
            .expect("a direct hop");
        let (a, b) = (ui[if_idx].pos, ui[sw_idx].pos);
        assert_eq!(ui[sw_idx].routes, vec![[a.0, a.1, b.0, b.1]]);
        assert_eq!(ui[sw_idx].cost, direct.cost);
        assert_eq!(resolved[sw_idx].chain, vec![ironforge, stormwind]);
        assert_eq!(resolved[sw_idx].cost, direct.cost);

        for (i, n) in ui.iter().enumerate() {
            let id = ids[i];
            if id != ironforge && cat.paths.between(ironforge, id).is_some() {
                assert_eq!(n.routes.len(), 1, "{} has a direct hop", n.name);
                assert_eq!(resolved[i].chain, vec![ironforge, id]);
            }
        }
    }

    /// The chain is the shortest by its hops' `TaxiPathNode` lengths, not by the straight lines
    /// between nodes: Shattrath to the Sanctum of the Stars, every node known, flies Allerian
    /// Stronghold and Wildhammer Stronghold (the straight-line search took Stonebreaker Hold and
    /// Shadowmoon Village).
    #[test]
    fn a_multi_hop_route_is_the_shortest_by_path_length() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = TaxiCatalogs::load(&mut chain).expect("taxi DBCs");
        let shattrath = node_named(&cat, "Shattrath, Terokkar Forest");
        let sanctum = node_named(&cat, "Sanctum of the Stars, Shadowmoon Valley");
        let open = TaxiOpen {
            flightmaster: 0x42,
            nearest_node: shattrath,
            known: TaxiMask(vec![u64::MAX; cat.mask_words()]),
        };
        let (_, ui, resolved) = build_nodes(&open, &cat, NEUTRAL, None, 2).expect("map builds");
        let i = ui
            .iter()
            .position(|n| n.name == "Sanctum of the Stars, Shadowmoon Valley")
            .expect("listed");
        assert_eq!(
            resolved[i].chain,
            vec![
                shattrath,
                node_named(&cat, "Allerian Stronghold, Terokkar Forest"),
                node_named(&cat, "Wildhammer Stronghold, Shadowmoon Valley"),
                sanctum,
            ]
        );
        assert_eq!(ui[i].routes.len(), 3, "one segment a hop");
    }
}
