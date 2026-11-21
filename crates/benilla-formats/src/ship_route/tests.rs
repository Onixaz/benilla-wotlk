use super::*;
use crate::WorldMapContinent;

fn node(id: u32, path_id: u32, node_index: u32, map_id: u32, flags: u32) -> TaxiPathNode {
    TaxiPathNode {
        id,
        path_id,
        node_index,
        map_id,
        pos: [id as f32, 0.0, 0.0],
        flags,
        delay: 0,
    }
}

fn row(path_id: i32, leg_index: i32, id: i32, xs: &[f32], ys: &[f32]) -> TaxiSplineRow {
    let mut loc_x = [0.0; CONTROL_POINTS];
    let mut loc_y = [0.0; CONTROL_POINTS];
    loc_x[..xs.len()].copy_from_slice(xs);
    loc_y[..ys.len()].copy_from_slice(ys);
    TaxiSplineRow {
        id,
        path_id,
        loc_x,
        loc_y,
        leg_index,
    }
}

/// A sheet where world `(x, y)` lands at `(0.5 − y·k, 1 − (0.5 − x·k'))`, scale 1, no offset.
fn plain_continents(maps: &[u32]) -> WorldMapContinentCatalog {
    WorldMapContinentCatalog::from_rows(maps.iter().map(|&map_id| WorldMapContinent {
        map_id,
        left_boundary: 0,
        right_boundary: 0,
        top_boundary: 0,
        bottom_boundary: 0,
        offset_x: 0.0,
        offset_y: 0.0,
        scale: 1.0,
        taxi_min: (0.0, 0.0),
        taxi_max: (0.0, 0.0),
        world_map_id: 1,
    }))
}

#[test]
fn legs_close_at_a_map_change_and_a_teleport_but_not_on_the_files_last_path() {
    let nodes = TaxiPathNodes::from_nodes([
        // Path 1: stops 10 and 11 on map 0, a teleport, a stop 13, then map 1 with stop 15.
        node(10, 1, 0, 0, 2),
        node(11, 1, 1, 0, 2),
        node(12, 1, 2, 0, 1),
        node(13, 1, 3, 0, 2),
        node(14, 1, 4, 1, 2),
        node(15, 1, 5, 1, 2),
        // Path 2, the last in the table: one stop, never closed.
        node(20, 2, 0, 0, 2),
    ]);
    let legs = DockLegs::build(&nodes);
    assert_eq!(legs.leg_count(1), 3, "teleport, map change, path change");
    assert_eq!(legs.leg(1, 0).map(|(a, b)| (a.id, b.id)), Some((10, 11)));
    assert_eq!(legs.leg(1, 1).map(|(a, b)| (a.id, b.id)), Some((13, 13)));
    // Node 14 opens map 1 but is not a stop: the map-change branch takes it.
    assert_eq!(legs.leg(1, 2).map(|(a, b)| (a.id, b.id)), Some((15, 15)));
    assert_eq!(
        legs.leg_count(2),
        0,
        "the reference never closes the table's last path"
    );
}

#[test]
fn a_leg_with_no_stop_keeps_its_index_and_never_resolves() {
    let nodes = TaxiPathNodes::from_nodes([
        node(10, 1, 0, 0, 0),
        node(11, 1, 1, 0, 1), // teleport before any stop
        node(12, 1, 2, 0, 2),
        node(20, 2, 0, 0, 0), // closes path 1, and is the last path
    ]);
    let legs = DockLegs::build(&nodes);
    assert_eq!(legs.leg_count(1), 2);
    assert!(legs.leg(1, 0).is_none(), "no first stop: unusable");
    assert_eq!(legs.leg(1, 1).map(|(a, _)| a.id), Some(12));
    // The departure search stops at the unusable leg 0 before it sees leg 1.
    assert_eq!(legs.leg_departing(1, 0), None);
}

#[test]
fn the_first_row_in_file_order_wins() {
    let splines = TaxiSplines {
        rows: vec![
            row(737, 0, 170, &[0.7], &[0.6]),
            row(737, 0, 175, &[0.1], &[0.1]),
        ],
    };
    assert_eq!(splines.find(737, 0).map(|r| r.id), Some(170));
    assert!(splines.find(737, 1).is_none());
}

#[test]
fn the_route_runs_from_the_legs_last_stop_to_the_next_legs_first_through_the_rows_points() {
    let nodes = TaxiPathNodes::from_nodes([
        node(10, 7, 0, 0, 2),
        node(11, 7, 1, 0, 2),
        node(12, 7, 2, 1, 2),
        node(13, 7, 3, 1, 2),
        node(99, 8, 0, 0, 0),
    ]);
    let legs = DockLegs::build(&nodes);
    let continents = plain_continents(&[0, 1]);
    // A point with both coordinates zero ends the list; the 0.9 after it is never read.
    let splines = TaxiSplines {
        rows: vec![row(7, 0, 1, &[0.3, 0.0, 0.9], &[0.6, 0.0, 0.9])],
    };
    let route = ship_route(&splines, &legs, &continents, 7, 0).expect("route");
    let at = |id: u32| continent_position(&continents, 0, id as f32, 0.0).unwrap();
    assert_eq!(route.departure, at(11), "leg 0's last stop");
    // Node 12 opens map 1 and is taken by the map-change branch, not counted as a stop.
    assert_eq!(route.arrival, at(13), "leg 1's first stop");
    assert_eq!(route.dots.first(), Some(&route.departure));
    assert_eq!(route.dots.last(), Some(&route.arrival));
    let nearest = route
        .dots
        .iter()
        .map(|d| ((d[0] - 0.3).powi(2) + (d[1] - 0.6).powi(2)).sqrt())
        .fold(f32::MAX, f32::min);
    assert!(
        nearest < 0.01,
        "the curve passes the row's control point: {nearest}"
    );
    // The last leg wraps to the first.
    let wrap = TaxiSplines {
        rows: vec![row(7, 1, 2, &[], &[])],
    };
    let back = ship_route(&wrap, &legs, &continents, 7, 1).expect("wrapped route");
    assert_eq!((back.departure, back.arrival), (at(13), at(10)));
    assert!(ship_route(&splines, &legs, &continents, 7, u32::MAX).is_none());
}

#[test]
fn a_straight_spline_measures_its_chord_and_dots_light_by_their_share() {
    let spline = ArcSpline::new(vec![[0.0, 0.0], [0.0, 0.0], [0.3, 0.0], [0.3, 0.0]]);
    assert!((spline.length - 0.3).abs() < 1e-4);
    assert_eq!(lit_dots(11, 0.0), 1, "the first dot is lit from the start");
    assert_eq!(lit_dots(11, 0.5), 6);
    assert_eq!(lit_dots(11, 1.0), 11);
}

#[test]
fn the_projection_is_the_world_sheets_with_y_up() {
    let continents = plain_continents(&[0]);
    let [u, v] = continent_position(&continents, 0, 1000.0, 2000.0).unwrap();
    assert!((u - (0.5 - 2000.0 * 0.000_029_940_12)).abs() < 1e-6);
    assert!(
        (v - (0.5 + 1000.0 * 0.000_044_910_183)).abs() < 1e-6,
        "world +x is up the sheet"
    );
    assert!(continent_position(&continents, 530, 0.0, 0.0).is_none());
}

/// Menethil Harbor to Theramore (path 292): the route leaves Menethil on the Eastern Kingdoms'
/// side of the sheet and lands at Theramore on Kalimdor's.
#[test]
fn real_menethil_to_theramore_route() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let splines = load_taxi_splines(&mut chain).expect("load LoadingScreenTaxiSplines");
    assert_eq!(splines.len(), 27, "3.3.5a ships 27 rows");
    let legs = DockLegs::build(&crate::load_taxi_path_nodes(&mut chain).expect("nodes"));
    let continents = crate::load_world_map_continent_catalog(&mut chain).expect("continents");
    // A far teleport from map 0 leaves on leg 0.
    assert_eq!(legs.leg_departing(292, 0), Some(0));
    let route = ship_route(&splines, &legs, &continents, 292, 0).expect("route");
    assert!(
        route.departure[0] > 0.7 && route.arrival[0] < 0.3,
        "{route:?}"
    );
    assert!(route.dots.len() > 20, "{} dots", route.dots.len());
    // Undercity to Grom'gol (path 301) teleports on map 0: both its legs have rows.
    assert!(ship_route(&splines, &legs, &continents, 301, 0).is_some());
    assert!(ship_route(&splines, &legs, &continents, 301, 1).is_some());
}

/// [`DockLegs::build`] walks the paths sorted by id where the reference walks the file
/// (`0x40b0b0`): the same walk only while the 3.3.5a file runs path id then node index, unsplit.
#[test]
fn real_taxi_path_node_file_runs_path_then_index() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let bytes = chain
        .read_file("DBFilesClient\\TaxiPathNode.dbc")
        .expect("read TaxiPathNode.dbc");
    let rs = crate::dbc::parse(&bytes, crate::taxi::taxi_path_node_schema(), "TaxiPathNode")
        .expect("parse TaxiPathNode.dbc");
    let order: Vec<(u32, u32)> = rs
        .records()
        .iter()
        .map(|r| {
            (
                crate::dbc::u32_at(r, 1).expect("PathID"),
                crate::dbc::u32_at(r, 2).expect("NodeIndex"),
            )
        })
        .collect();
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "TaxiPathNode.dbc is not in path id, node index order"
    );
}
