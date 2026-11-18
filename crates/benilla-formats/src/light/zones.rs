//! The zone lights: 12340's hardcoded Northrend zone outlines, each naming the `Light.dbc` row that
//! lights the zone (`s_zones`, `s_zonePaths` in `WoW/Source/WorldClient/World.cpp`).
//!
//! Northrend's area spheres are sparse: most of its zones are lit by these instead. Each frame
//! `DayNight::Update` (`0x7816f0`) clears the list and `FindAndAddZoneLight` (`0x77eed0`) adds every
//! zone whose outline the camera is inside, or within 50 yards outside of, with its blend
//! distance. `DoAreaLights` (`0x7f1360`) then blends them, in table order, before the area spheres
//! (`BlendZoneLight`, `0x7ee6b0`), and skips any area sphere with a falloff end under 3 yards.

#[path = "zone_paths.rs"]
mod zone_paths;
use zone_paths::{ZONE_LIGHTS, ZONE_PATHS};

/// `s_useZoneLights`, 1 in 12340.
pub(super) const USE_ZONE_LIGHTS: bool = true;

/// `AddZoneLight`'s cap (`m_nearLight[5]`).
const MAX_ZONE_LIGHTS: usize = 5;

/// `FindAndAddZoneLight`'s margin: the bound grows and the distance shrinks by 50 yards.
const MARGIN: f32 = 50.0;

/// One parsed zone outline (`ZonePath` after `ZonesInitialize`, `0x77ed40`).
struct Zone {
    map: u32,
    light: u32,
    points: Vec<[f32; 2]>,
    /// `m_minX, m_minY, m_maxX, m_maxY`, grown by [`MARGIN`].
    bounds: [f32; 4],
}

/// `ZonesInitialize`: read each `M`/`L` point of the path up to the `z`, then map it from the
/// outline's 1024-unit page to the zone plane: `x = (x + offX) · 33.333332`,
/// `y = (y + offY) / 1024 · 34133.332`, with `ZonePath`'s offsets `(−1.66233754, −145.731598)`.
fn parse(path: &str) -> Vec<[f32; 2]> {
    const OFFSET_X: f32 = -1.662_337_5;
    const OFFSET_Y: f32 = -145.731_6;
    let body = path.split('z').next().unwrap_or("");
    let mut out = Vec::new();
    let mut tokens = body.split_whitespace();
    while let Some(t) = tokens.next() {
        if t != "M" && t != "L" {
            continue;
        }
        let Some((x, y)) = tokens.next().and_then(|p| p.split_once(',')) else {
            break;
        };
        let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else {
            continue;
        };
        out.push([
            (x + OFFSET_X) * 33.333_332,
            (y + OFFSET_Y) / 1024.0 * 34_133.332,
        ]);
    }
    out
}

fn zones() -> &'static [Zone] {
    static ZONES: std::sync::OnceLock<Vec<Zone>> = std::sync::OnceLock::new();
    ZONES.get_or_init(|| {
        ZONE_LIGHTS
            .iter()
            .zip(ZONE_PATHS)
            .map(|(&(map, light), path)| {
                let points = parse(path);
                let mut b = [f32::MAX, f32::MAX, -f32::MAX, -f32::MAX];
                for p in &points {
                    b[0] = b[0].min(p[0]);
                    b[1] = b[1].min(p[1]);
                    b[2] = b[2].max(p[0]);
                    b[3] = b[3].max(p[1]);
                }
                Zone {
                    map,
                    light,
                    points,
                    bounds: [b[0] - MARGIN, b[1] - MARGIN, b[2] + MARGIN, b[3] + MARGIN],
                }
            })
            .collect()
    })
}

/// `CWorldMath::DistanceSquaredFromEdge`: the squared distance from `p` to the segment `a`–`b`.
fn distance_sq_from_edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx * dx + dy * dy;
    let t = if len > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a[0] + t * dx, a[1] + t * dy);
    (p[0] - qx).powi(2) + (p[1] - qy).powi(2)
}

/// `CWorldMath::PointInPolygon` (`0x7f9c90`): the crossing test (a half-open `y` span per edge) and
/// the distance to the nearest edge.
fn point_in_polygon(points: &[[f32; 2]], p: [f32; 2]) -> (bool, f32) {
    let mut nearest = f32::MAX;
    let mut inside = false;
    for i in 0..points.len() {
        let cur = points[i];
        let prev = points[if i == 0 { points.len() - 1 } else { i - 1 }];
        nearest = nearest.min(distance_sq_from_edge(cur, prev, p));
        let spans = (p[1] > cur[1] && p[1] <= prev[1]) || (p[1] > prev[1] && p[1] <= cur[1]);
        if spans && (p[1] - cur[1]) / (prev[1] - cur[1]) * (prev[0] - cur[0]) + cur[0] < p[0] {
            inside = !inside;
        }
    }
    (inside, nearest.sqrt())
}

/// `FindAndAddZoneLight` at camera `pos` (world yards) on `map`: each zone light's `Light.dbc` id
/// and its blend distance, in table order, at most five. The distance is `50 + d` inside an outline
/// (`d` to its nearest edge) and `50 − d` within 50 yards outside it. `exists` is
/// `g_lightDB.GetRecord`: `AddZoneLight` drops an id with no row.
pub(super) fn zone_lights(
    map: u32,
    pos: [f32; 3],
    exists: impl Fn(u32) -> bool,
) -> Vec<(u32, f32)> {
    let point = [-(pos[1] - 17_066.666), -(pos[0] - 17_066.666)];
    let mut out = Vec::new();
    for z in zones() {
        let [min_x, min_y, max_x, max_y] = z.bounds;
        if z.map != map
            || min_x > point[0]
            || point[1] < min_y
            || max_x < point[0]
            || point[1] > max_y
        {
            continue;
        }
        let (inside, edge) = point_in_polygon(&z.points, point);
        let distance = if inside { -edge } else { edge } - MARGIN;
        if distance < 0.0 && out.len() < MAX_ZONE_LIGHTS && exists(z.light) {
            out.push((z.light, -distance));
        }
    }
    out
}

/// `BlendZoneLight`'s weight (`0x7ee6b0`) for blend distance `dist`: `1 − (100 − dist) / 100`,
/// 1 from 100 on, so a zone fades in over the 100 yards across its edge; capped by `maxFade` 1.0.
pub(super) fn zone_weight(dist: f32) -> f32 {
    let fade = (100.0 - dist) * 0.01;
    if fade <= 0.0 {
        1.0
    } else {
        (1.0 - fade).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eleven_outlines_parse() {
        let z = zones();
        assert_eq!(z.len(), 11);
        assert!(z.iter().all(|z| z.points.len() >= 3 && z.map == 571));
    }

    #[test]
    fn the_camera_finds_its_zone_light() {
        let any = |_| true;
        // Storm Peaks (K3), Dragonblight (Star's Rest), Crystalsong, Howling Fjord (Valgarde).
        let ids = |pos| {
            zone_lights(571, pos, any)
                .iter()
                .map(|z| z.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids([6150.0, -1050.0, 410.0]), [1777]);
        assert_eq!(ids([3500.0, 2000.0, 64.0]), [825]);
        assert_eq!(ids([5500.0, -500.0, 180.0]), [1740]);
        assert_eq!(ids([570.0, -5000.0, 10.0]), [862]);
        // Another map, or a missing row: nothing.
        assert!(zone_lights(0, [6150.0, -1050.0, 410.0], any).is_empty());
        assert!(zone_lights(571, [6150.0, -1050.0, 410.0], |_| false).is_empty());
        // Deep inside: well past the 100-yard ramp.
        let k3 = zone_lights(571, [6150.0, -1050.0, 410.0], any);
        assert!(k3[0].1 > 300.0 && zone_weight(k3[0].1) == 1.0, "{k3:?}");
    }

    #[test]
    fn the_zone_weight_ramps_across_the_edge() {
        assert_eq!(zone_weight(0.0), 0.0);
        assert_eq!(zone_weight(25.0), 0.25);
        assert_eq!(zone_weight(50.0), 0.5, "on the outline itself");
        assert_eq!(zone_weight(100.0), 1.0);
        assert_eq!(zone_weight(400.0), 1.0);
    }

    #[test]
    fn the_polygon_test_counts_crossings_and_measures_the_nearest_edge() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(point_in_polygon(&square, [5.0, 2.0]), (true, 2.0));
        assert_eq!(point_in_polygon(&square, [13.0, 5.0]), (false, 3.0));
    }
}
