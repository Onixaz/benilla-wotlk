//! The ship loading screen's route: `LoadingScreenTaxiSplines.dbc`'s control points between a
//! transport's two docks, projected onto the world map sheet. 12340 builds it when a far teleport
//! rides a transport (`0x40ad50`) and when a ridden transport changes leg on the current map
//! (`0x40ae30`), and draws it as a run of dots that light with the load's progress (`0x408e10`).
//!
//! Every position here is in the sheet's unit square, x to the right and y up from the bottom edge.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse};
use crate::{Chain, TaxiPathNode, TaxiPathNodes, WorldMapContinentCatalog};

const LOADING_SCREEN_TAXI_SPLINES: &str = "DBFilesClient\\LoadingScreenTaxiSplines.dbc";

/// Control points a row carries (`LocX[8]`, `LocY[8]`).
const CONTROL_POINTS: usize = 8;
/// Dots per unit of route length: one every 0.015 of the sheet (`0x409880`, truncated).
const DOTS_PER_UNIT: f32 = 66.666_67;

/// One `LoadingScreenTaxiSplines.dbc` row: the bend of one leg of a transport's path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TaxiSplineRow {
    pub id: i32,
    /// `TaxiPath.dbc` id, a type-15 template's `data0`.
    pub path_id: i32,
    /// Sheet positions; the list ends at the first point with neither coordinate positive.
    pub loc_x: [f32; CONTROL_POINTS],
    pub loc_y: [f32; CONTROL_POINTS],
    /// Which dock leg of the path this row bends ([`DockLegs`]).
    pub leg_index: i32,
}

/// `LoadingScreenTaxiSplines.dbc` in file order, which a lookup's first match depends on: paths
/// 737 and 964 each carry two rows for leg 0.
pub struct TaxiSplines {
    rows: Vec<TaxiSplineRow>,
}

impl TaxiSplines {
    /// The first row in file order for `(path, leg)` (`0x408bb0`).
    pub fn find(&self, path_id: i32, leg_index: i32) -> Option<&TaxiSplineRow> {
        self.rows
            .iter()
            .find(|r| r.path_id == path_id && r.leg_index == leg_index)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn taxi_splines_schema() -> Schema {
    let mut s = Schema::new("LoadingScreenTaxiSplines");
    for name in ["ID", "PathID"] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    for axis in ["LocX", "LocY"] {
        for i in 0..CONTROL_POINTS {
            s.add_field(SchemaField::new(format!("{axis}_{i}"), FieldType::Float32));
        }
    }
    s.add_field(SchemaField::new("LegIndex", FieldType::Int32));
    s
}

/// Read `LoadingScreenTaxiSplines.dbc` off the patch chain.
pub fn load_taxi_splines(chain: &mut Chain) -> Result<TaxiSplines> {
    let bytes = chain
        .read_file(LOADING_SCREEN_TAXI_SPLINES)
        .context("reading LoadingScreenTaxiSplines.dbc")?;
    taxi_splines_from(&bytes)
}

fn taxi_splines_from(bytes: &[u8]) -> Result<TaxiSplines> {
    let rs = parse(bytes, taxi_splines_schema(), "LoadingScreenTaxiSplines")?;
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| {
            let mut loc_x = [0.0; CONTROL_POINTS];
            let mut loc_y = [0.0; CONTROL_POINTS];
            for i in 0..CONTROL_POINTS {
                loc_x[i] = f32_at(r, 2 + i)?;
                loc_y[i] = f32_at(r, 2 + CONTROL_POINTS + i)?;
            }
            Some(TaxiSplineRow {
                id: i32_at(r, 0)?,
                path_id: i32_at(r, 1)?,
                loc_x,
                loc_y,
                leg_index: i32_at(r, 2 + 2 * CONTROL_POINTS)?,
            })
        })
        .collect();
    Ok(TaxiSplines { rows })
}

/// A dock leg: the first and last stop of a run of a path's nodes (`TaxiPathNode` flag 2). An
/// unset end is the reference's id 0, which makes the leg unusable but keeps its index.
#[derive(Clone, Copy, Debug, Default)]
struct DockLeg {
    first_stop: Option<TaxiPathNode>,
    last_stop: Option<TaxiPathNode>,
}

/// Every path's dock legs, the table the route's endpoints come from (`0x40b0b0`).
pub struct DockLegs {
    paths: HashMap<u32, Vec<DockLeg>>,
}

impl DockLegs {
    /// The reference walks `TaxiPathNode.dbc` in file order, which is path id then node index (no
    /// path's rows are split or out of order in 3.3.5a), so the walk runs over the paths sorted by
    /// id. A leg closes at a teleport node (flag 1), at a node on another map than the one before,
    /// and when the next path starts; the file's last path never gets that last close. A stop is a
    /// flag-2 node that is neither of those. The last stop carries over a path change, so a leg
    /// opened by no stop holds a stale one, but such a leg has no first stop and never resolves.
    pub fn build(nodes: &TaxiPathNodes) -> Self {
        let mut ids: Vec<u32> = nodes.paths().map(|(id, _)| id).collect();
        ids.sort_unstable();
        let mut paths: HashMap<u32, Vec<DockLeg>> = HashMap::new();
        let mut prev: Option<TaxiPathNode> = None;
        let mut first_stop: Option<TaxiPathNode> = None;
        let mut last_stop: Option<TaxiPathNode> = None;
        let walk = ids
            .iter()
            .filter_map(|id| nodes.path(*id))
            .flat_map(|path| path.iter().copied());
        for node in walk {
            if prev.is_none_or(|p| p.path_id != node.path_id) {
                if let (Some(p), Some(_)) = (prev, first_stop) {
                    paths.entry(p.path_id).or_default().push(DockLeg {
                        first_stop,
                        last_stop,
                    });
                }
                first_stop = None;
            }
            if node.flags & 1 != 0 {
                paths.entry(node.path_id).or_default().push(DockLeg {
                    first_stop,
                    last_stop,
                });
                first_stop = None;
            } else if let Some(p) =
                prev.filter(|p| p.path_id == node.path_id && p.map_id != node.map_id)
            {
                paths.entry(p.path_id).or_default().push(DockLeg {
                    first_stop,
                    last_stop,
                });
                first_stop = None;
            } else if node.flags & 2 != 0 {
                last_stop = Some(node);
                if first_stop.is_none() {
                    first_stop = Some(node);
                }
            }
            prev = Some(node);
        }
        Self { paths }
    }

    /// How many dock legs `path_id` has.
    pub fn leg_count(&self, path_id: u32) -> usize {
        self.paths.get(&path_id).map_or(0, Vec::len)
    }

    /// A leg's first and last stop, when both are set (`0x408c80`).
    pub fn leg(&self, path_id: u32, leg: u32) -> Option<(TaxiPathNode, TaxiPathNode)> {
        let leg = self.paths.get(&path_id)?.get(leg as usize)?;
        Some((leg.first_stop?, leg.last_stop?))
    }

    /// The first leg whose first stop is on `map_id`, the leg a far teleport leaves from
    /// (`0x408cd0`). The scan stops at the first unusable leg, as the reference's does.
    pub fn leg_departing(&self, path_id: u32, map_id: u32) -> Option<u32> {
        for i in 0..self.leg_count(path_id) as u32 {
            let (first, _) = self.leg(path_id, i)?;
            if first.map_id == map_id {
                return Some(i);
            }
        }
        None
    }
}

/// A world position on `continent`'s place on the world map sheet, y up (`0x5440e0`). `None` for a
/// map with no `WorldMapContinent` row; the last row for a map wins, as the reference scans from
/// the end.
pub fn continent_position(
    continents: &WorldMapContinentCatalog,
    map_id: u32,
    x: f32,
    y: f32,
) -> Option<[f32; 2]> {
    let c = continents.get(map_id)?;
    let top = c.offset_y * 0.023_952_097 + 0.5 - x * 0.000_044_910_183 * c.scale;
    let u = c.offset_x * 0.015_968_064 + 0.5 - y * 0.000_029_940_12 * c.scale;
    Some([u, 1.0 - top])
}

/// The route the screen draws: the two dock markers and the dots between them, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct ShipRoute {
    pub departure: [f32; 2],
    pub arrival: [f32; 2],
    pub dots: Vec<[f32; 2]>,
}

/// The route for `leg` of `path_id` (`0x409880`): from the leg's last stop, where the transport
/// leaves, through the row's control points to the next leg's first stop. `None` when the row,
/// either leg, either stop or either continent is missing; the screen then draws its plain bar.
pub fn ship_route(
    splines: &TaxiSplines,
    legs: &DockLegs,
    continents: &WorldMapContinentCatalog,
    path_id: u32,
    leg: u32,
) -> Option<ShipRoute> {
    // The row lookup comes first and compares signed: the leg a failed departure search hands over
    // is the reference's -1.
    let row = splines.find(path_id as i32, leg as i32)?;
    let leg_count = legs.leg_count(path_id) as u32;
    if leg_count == 0 {
        return None;
    }
    let (_, departing) = legs.leg(path_id, leg)?;
    let (arriving, _) = legs.leg(path_id, leg.wrapping_add(1) % leg_count)?;
    let departure = continent_position(
        continents,
        departing.map_id,
        departing.pos[0],
        departing.pos[1],
    )?;
    let arrival = continent_position(
        continents,
        arriving.map_id,
        arriving.pos[0],
        arriving.pos[1],
    )?;

    // Each end doubles as its own outer control.
    let mut points = vec![departure, departure];
    for i in 0..CONTROL_POINTS {
        if !(row.loc_x[i] > 0.0 || row.loc_y[i] > 0.0) {
            break;
        }
        points.push([row.loc_x[i], row.loc_y[i]]);
    }
    points.push(arrival);
    points.push(arrival);
    let spline = ArcSpline::new(points);

    let segments = ((spline.length * DOTS_PER_UNIT) as i32).max(1);
    let step = 1.0 / segments as f32;
    let mut t = 0.0f32;
    let mut dots = Vec::with_capacity(segments as usize + 1);
    for _ in 0..=segments {
        t = t.clamp(0.0, 1.0);
        dots.push(spline.point_at(t));
        t += step;
    }
    Some(ShipRoute {
        departure,
        arrival,
        dots,
    })
}

/// How many of `dots` dots the screen draws at `progress` (`0x408e10`): those whose share of the
/// route, `i / (dots − 1)`, does not pass the progress.
pub fn lit_dots(dots: usize, progress: f32) -> usize {
    (0..dots)
        .take_while(|&i| i as f32 / (dots as f32 - 1.0) <= progress)
        .count()
}

/// A Catmull-Rom spline through `points[1..n-1]`, the outer two its end controls, walked by arc
/// length; each segment's length is a 20-chord sum (`0x4c3b10`).
struct ArcSpline {
    points: Vec<[f32; 2]>,
    seg_lengths: Vec<f32>,
    length: f32,
}

impl ArcSpline {
    fn new(points: Vec<[f32; 2]>) -> Self {
        let seg_count = points.len().saturating_sub(3);
        let mut s = Self {
            points,
            seg_lengths: Vec::with_capacity(seg_count),
            length: 0.0,
        };
        for seg in 0..seg_count {
            let len = s.seg_length(seg);
            s.seg_lengths.push(len);
        }
        s.length = s.seg_lengths.iter().sum();
        s
    }

    /// The Catmull-Rom basis over `points[seg..seg + 4]` (`0x4c39d0`).
    fn basis_point(&self, seg: usize, t: f32) -> [f32; 2] {
        const BASIS: [[f32; 4]; 4] = [
            [-0.5, 1.0, -0.5, 0.0],
            [1.5, -2.5, 0.0, 1.0],
            [-1.5, 2.0, 0.5, 0.0],
            [0.5, -0.5, 0.0, 0.0],
        ];
        let mut pos = [0.0f32; 2];
        for (row, point) in BASIS.iter().zip(&self.points[seg..seg + 4]) {
            let w = ((row[0] * t + row[1]) * t + row[2]) * t + row[3];
            pos[0] += w * point[0];
            pos[1] += w * point[1];
        }
        pos
    }

    fn seg_length(&self, seg: usize) -> f32 {
        let mut prev = self.basis_point(seg, 0.0);
        let mut step = 0.05f32;
        let mut length = 0.0;
        for _ in 0..20 {
            let cur = self.basis_point(seg, step);
            length += ((cur[0] - prev[0]).powi(2) + (cur[1] - prev[1]).powi(2)).sqrt();
            prev = cur;
            step += 0.05;
        }
        length
    }

    /// The point at arc-length fraction `t`; the ends are the outer controls (`0x4c3870`).
    fn point_at(&self, t: f32) -> [f32; 2] {
        if t <= 0.0 {
            return self.points[0];
        }
        if t >= 1.0 {
            return self.points[self.points.len() - 1];
        }
        let (seg, seg_t) = self.segment_at(t);
        self.basis_point(seg, seg_t)
    }

    /// The segment holding `t` of the length and the fraction into it (`0x4c3bd0`). A zero-length
    /// segment divides by zero there; here it reads as its start.
    fn segment_at(&self, t: f32) -> (usize, f32) {
        let seg_count = self.seg_lengths.len();
        if seg_count <= 1 {
            return (0, t);
        }
        let target = self.length * t;
        let mut before = 0.0;
        let mut seg = 0;
        loop {
            let after = self.seg_lengths[seg] + before;
            if after > target {
                break;
            }
            before = after;
            seg += 1;
            if seg >= seg_count - 1 {
                break;
            }
        }
        let len = self.seg_lengths[seg];
        let seg_t = if len > 0.0 {
            (target - before) / len
        } else {
            0.0
        };
        (seg, seg_t)
    }
}

#[cfg(test)]
mod tests;
