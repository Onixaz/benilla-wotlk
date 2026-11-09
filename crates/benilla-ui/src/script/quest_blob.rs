//! The quest blob geometry (`0x58ed80`, `0x58f1a0`, `0x58e310`) and hover test (`0x58e0d0`).

use std::sync::Arc;

use crate::layout::Rect;
use crate::widget::{
    BlobGroup, BlobHover, BlobPaintKey, BlobRender, FrameHandle, QuestBlobMesh, QuestBlobPaint,
    QuestPoiState,
};

use super::quest_watch::{cvar_int, objective_mask, QuestPoiRegion};
use super::Model;

/// A FrameXML unit in frame-rect units, the space of `0x58f1a0`'s thresholds and `0.01` border.
const DDC_PER_UNIT: f32 = 1.0 / 1024.0;
/// The frame slots (`+0x2d0`).
const SLOTS: usize = 4;

/// The frame's rect in the reference's frame-rect units.
#[derive(Clone, Copy)]
struct BlobRect {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

impl From<Rect> for BlobRect {
    fn from(r: Rect) -> Self {
        Self {
            min_x: r.left * DDC_PER_UNIT,
            max_x: r.right * DDC_PER_UNIT,
            min_y: r.bottom * DDC_PER_UNIT,
            max_y: r.top * DDC_PER_UNIT,
        }
    }
}

impl BlobRect {
    /// `0x5900f0`'s empty-rect test, both axes under `0.001`.
    fn is_empty(self) -> bool {
        (self.max_x - self.min_x).abs() < 0.001 && (self.max_y - self.min_y).abs() < 0.001
    }
}

/// The Catmull-Rom basis (`[0xac37b8]`), row per control point.
const BASIS: [[f32; 4]; 4] = [
    [-0.5, 1.0, -0.5, 0.0],
    [1.5, -2.5, 0.0, 1.0],
    [-1.5, 2.0, 0.5, 0.0],
    [0.5, -0.5, 0.0, 0.0],
];

fn evaluate(controls: &[[f32; 2]], seg: usize, t: f32) -> [f32; 2] {
    let mut pos = [0.0f32, 0.0];
    for (i, row) in BASIS.iter().enumerate() {
        let weight = ((row[0] * t + row[1]) * t + row[2]) * t + row[3];
        pos[0] += weight * controls[seg + i][0];
        pos[1] += controls[seg + i][1] * weight;
    }
    pos
}

/// A segment's length (`0x4c3b10`): twenty chords stepped by an accumulating `0.05`.
fn seg_length(controls: &[[f32; 2]], seg: usize) -> f32 {
    let mut step = 0.05f32;
    let mut length = 0.0f32;
    let mut prev = evaluate(controls, seg, 0.0);
    for _ in 0..20 {
        let cur = evaluate(controls, seg, step);
        let (dx, dy) = (cur[0] - prev[0], cur[1] - prev[1]);
        prev = cur;
        length += (dx * dx + dy * dy).sqrt();
        step += 0.05;
    }
    length
}

/// `0x58ed80`: the raw outline, or a closed Catmull-Rom sampled by arc length, truncated to yards.
fn blob_outline(points: &[(i32, i32)], smoothing: bool, samples: i32) -> Vec<(i32, i32)> {
    if !smoothing {
        return points.to_vec();
    }
    let n = points.len();
    let at = |p: (i32, i32)| [p.0 as f32, p.1 as f32];
    let mut controls = Vec::with_capacity(n + 3);
    controls.push(at(points[n - 1]));
    controls.extend(points.iter().map(|&p| at(p)));
    controls.push(at(points[0]));
    controls.push(at(points[1]));
    let segs = controls.len() - 3;
    let lengths: Vec<f32> = (0..segs).map(|s| seg_length(&controls, s)).collect();
    let total: f32 = lengths.iter().sum();
    // `0x4c3870` then `0x4c3bd0`: `t` to a segment and its local parameter by arc length.
    let pos = |t: f32| -> [f32; 2] {
        let target = total * t;
        let (mut seg, mut before) = (0usize, 0.0f32);
        while seg < segs - 1 {
            let after = lengths[seg] + before;
            if after > target {
                break;
            }
            before = after;
            seg += 1;
        }
        let seg_t = if segs <= 1 {
            t
        } else {
            (target - before) / lengths[seg]
        };
        evaluate(&controls, seg, seg_t)
    };
    let step = 1.0f32 / samples as f32;
    let mut t = 0.01f32;
    let mut out = Vec::with_capacity(samples.max(0) as usize);
    for _ in 0..samples {
        let p = pos(t);
        out.push((p[0] as i32, p[1] as i32));
        t += step;
    }
    out
}

/// `0x9830d0` over the x/y axes: `point` inside the closed polygon, the even-odd crossing test.
fn inside(point: [f32; 2], verts: &[[f32; 2]]) -> bool {
    let mut vtx0 = verts[verts.len() - 1];
    let mut yflag0 = point[1] <= vtx0[1];
    let mut hit = false;
    for &vtx1 in verts {
        let yflag1 = point[1] <= vtx1[1];
        if yflag0 != yflag1
            && ((vtx1[0] - point[0]) * (vtx0[1] - vtx1[1])
                <= (vtx0[0] - vtx1[0]) * (vtx1[1] - point[1]))
                == yflag1
        {
            hit = !hit;
        }
        yflag0 = yflag1;
        vtx0 = vtx1;
    }
    hit
}

/// `0x58e5c0`: the crossing of the lines `a0-a1` and `b0-b1`, none when nearly parallel.
fn intersect_lines(a0: [f32; 2], a1: [f32; 2], b0: [f32; 2], b1: [f32; 2]) -> Option<[f32; 2]> {
    let (ax, bx) = (a0[0] - a1[0], b0[0] - b1[0]);
    let (ay, by) = (a0[1] - a1[1], b0[1] - b1[1]);
    let denom = by * ax - ay * bx;
    if denom.abs() < 2.384_185_8e-7 {
        return None;
    }
    let (ca, cb) = (a0[0] * a1[1] - a1[0] * a0[1], b0[0] * b1[1] - b1[0] * b0[1]);
    Some([
        (bx * ca - ax * cb) * (1.0 / denom),
        1.0 / denom * (by * ca - ay * cb),
    ])
}

/// A world point's `(u, v)` on the displayed map, `None` off it or on an edge (`0x58f2f3`).
fn projected(model: &Model, area_map: u32, x: f32, y: f32) -> Option<(f32, f32)> {
    let (uv, outland) = model.worldmap.quest_project(area_map, x, y);
    (!outland && (uv.0 * uv.1).abs() >= 0.001).then_some(uv)
}

/// One blob's render data (`0x58f1a0`), invalid when any outline point is off the map.
fn build_blob(
    model: &Model,
    p: &QuestPoiState,
    region: &QuestPoiRegion,
    rect: BlobRect,
) -> BlobRender {
    let mut data = BlobRender {
        objective: region.objective,
        map: region.map,
        center: region.center,
        objectives: [region.objective, 0, 0, 0],
        num_objectives: 1,
        ..Default::default()
    };
    let Some(area_map) = model.worldmap.quest_blob_area() else {
        return data;
    };
    if region.outline.len() < 3 {
        return data;
    }
    let outline = blob_outline(&region.outline, p.smoothing, p.spline_points);
    let count = outline.len();
    let mut fill = Vec::with_capacity(count + 1);
    let mut center = [0.0f32, 0.0];
    for &(x, y) in &outline {
        let Some((u, v)) = projected(model, area_map, x as f32, y as f32) else {
            return data;
        };
        let vertex = [
            u * (rect.max_x - rect.min_x) + rect.min_x,
            (1.0 - v) * (rect.max_y - rect.min_y) + rect.min_y,
        ];
        fill.push(vertex);
        center[0] += vertex[0];
        center[1] += vertex[1];
    }
    // The maxima seed at `FLT_MIN`, not `-FLT_MAX` (`0x58f1a0`).
    let mut bounds = [f32::MAX, f32::MIN_POSITIVE, f32::MAX, f32::MIN_POSITIVE];
    for v in &fill {
        bounds[0] = if v[0] <= bounds[0] { v[0] } else { bounds[0] };
        bounds[1] = if v[0] >= bounds[1] { v[0] } else { bounds[1] };
        bounds[2] = if v[1] <= bounds[2] { v[1] } else { bounds[2] };
        bounds[3] = if v[1] >= bounds[3] { v[1] } else { bounds[3] };
    }
    let scale = 1.0 / count as f32;
    let center = [center[0] * scale, center[1] * scale];
    fill.push(center);

    // Each edge pushed out `0.01 * scalar` away from the centre, the neighbours' lines mitred.
    let border_width = p.border_scalar * 0.01;
    let mut starts = Vec::with_capacity(count);
    let mut ends = Vec::with_capacity(count);
    for i in 0..count {
        let current = fill[i];
        let following = fill[(i + 1) % count];
        let mut n = [-(following[1] - current[1]), following[0] - current[0]];
        let length = n[1] * n[1] + n[0] * n[0];
        if length > 2.384_185_8e-7 {
            let inverse = 1.0 / length.sqrt();
            n = [n[0] * inverse, n[1] * inverse];
        }
        n = [n[0] * border_width, n[1] * border_width];
        if (center[1] - current[1]) * n[1] + (center[0] - current[0]) * n[0] > 0.0 {
            n = [-n[0], -n[1]];
        }
        starts.push([current[0] + n[0], current[1] + n[1]]);
        ends.push([n[0] + following[0], n[1] + following[1]]);
    }
    let mut border = Vec::with_capacity(2 * count);
    for i in 0..count {
        let next = (i + 1) % count;
        border.push(fill[i]);
        border.push(
            intersect_lines(starts[i], ends[i], starts[next], ends[next]).unwrap_or(starts[i]),
        );
    }
    data.fill = fill;
    data.border = border;
    data.bounds = bounds;
    data.valid = true;
    data
}

/// `1 << objective & mask` as x86 reads it, the shift count masked to five bits (`0x58e310`).
fn serves_objective(objective: i32, mask: u32) -> bool {
    1u32.wrapping_shl(objective as u32) & mask != 0
}

/// `0x58e310`: a blob over the threshold inside a larger one folds into it with its objectives.
fn merge_blobs(blobs: &mut [BlobRender], mask: u32, threshold: f32) {
    if mask == u32::MAX {
        return;
    }
    for i in 0..blobs.len() {
        if !blobs[i].valid || !serves_objective(blobs[i].objective, mask) {
            continue;
        }
        for j in i + 1..blobs.len() {
            if !blobs[j].valid || !serves_objective(blobs[j].objective, mask) {
                continue;
            }
            let (head, tail) = blobs.split_at_mut(j);
            let (first, second) = (&mut head[i], &mut tail[0]);
            let [fx0, fx1, fy0, fy1] = first.bounds;
            let [sx0, sx1, sy0, sy1] = second.bounds;
            let (ox0, ox1) = (fx0.max(sx0), fx1.min(sx1));
            let (oy0, oy1) = (fy0.max(sy0), fy1.min(sy1));
            if oy1 <= oy0 || ox1 <= ox0 {
                continue;
            }
            let area = |[x0, x1, y0, y1]: [f32; 4]| (x1 - x0) * (y1 - y0);
            let (smaller, larger) = if area(second.bounds) <= area(first.bounds) {
                (second, first)
            } else {
                (first, second)
            };
            let rim = smaller.fill.len() - 1;
            let inside_count = smaller.fill[..rim]
                .iter()
                .filter(|&&v| inside(v, &larger.fill[..larger.fill.len() - 1]))
                .count();
            if inside_count as f32 / rim as f32 <= threshold {
                continue;
            }
            smaller.valid = false;
            for k in 0..smaller.num_objectives as usize {
                if larger.num_objectives >= 4 {
                    continue;
                }
                let held = larger.num_objectives as usize;
                if !larger.objectives[..held].contains(&smaller.objectives[k]) {
                    larger.objectives[held] = smaller.objectives[k];
                    larger.num_objectives += 1;
                }
            }
        }
    }
}

/// The frame's resolved rect, `None` while unresolved or empty (`0x5900f0`).
fn frame_rect(model: &Model, frame: FrameHandle) -> Option<Rect> {
    model
        .resolved
        .get(&frame)
        .copied()
        .filter(|r| !BlobRect::from(*r).is_empty())
}

/// `0x5900f0`: one quest's blobs on `rect`, merged when the frame merges.
fn build_group(
    model: &Model,
    p: &QuestPoiState,
    quest_id: u32,
    rect: Rect,
) -> Option<Vec<BlobRender>> {
    let slot = model.quest_watch.poi(quest_id)?;
    let frame_rect = BlobRect::from(rect);
    let mut blobs: Vec<BlobRender> = slot
        .regions
        .iter()
        .map(|r| build_blob(model, p, r, frame_rect))
        .collect();
    if p.merging {
        merge_blobs(
            &mut blobs,
            objective_mask(model, quest_id),
            p.merge_threshold,
        );
    }
    Some(blobs)
}

/// The add (`0x590560`): the first free slot, built now or by the paint on an empty rect.
pub(super) fn add_quest(
    model: &Model,
    p: &QuestPoiState,
    frame: FrameHandle,
    quest_id: u32,
) -> bool {
    if model.quest_watch.poi(quest_id).is_none() {
        return false;
    }
    let mut cache = p.cache.borrow_mut();
    let groups = &mut cache.groups;
    if groups.iter().flatten().any(|g| g.quest_id == quest_id) {
        return false;
    }
    let Some(free) = groups.iter().position(Option::is_none) else {
        return false;
    };
    let built_on = frame_rect(model, frame);
    let blobs = built_on
        .and_then(|rect| build_group(model, p, quest_id, rect))
        .unwrap_or_default();
    groups[free] = Some(BlobGroup {
        quest_id,
        built_on,
        blobs,
    });
    cache.generation += 1;
    true
}

/// The remove (`0x58fb40`).
pub(super) fn remove_quest(p: &QuestPoiState, quest_id: u32) -> bool {
    if quest_id == 0 {
        return false;
    }
    let mut cache = p.cache.borrow_mut();
    let Some(group) = cache
        .groups
        .iter_mut()
        .find(|g| g.as_ref().is_some_and(|g| g.quest_id == quest_id))
    else {
        return false;
    };
    *group = None;
    cache.generation += 1;
    true
}

/// The mesh this walk draws (`0x5901c0`'s gates), reused while nothing it was made from moved.
pub(super) fn paint(
    model: &Model,
    p: &QuestPoiState,
    frame: FrameHandle,
) -> Option<QuestBlobPaint> {
    if cvar_int(model, "questpoi") == 0 {
        return None;
    }
    let fill_texture = p.fill_texture.as_ref()?;
    let border_texture = p.border_texture.as_ref()?;
    let area_map = model.worldmap.quest_blob_area()?;
    let rect = frame_rect(model, frame)?;
    let mut cache = p.cache.borrow_mut();
    // Deviation: 0x5901c0 rebuilds only an empty-rect group; a moved rect's stale vertices are a bug.
    let mut rebuilt = false;
    for group in cache.groups.iter_mut().flatten() {
        if group.built_on != Some(rect) {
            group.blobs = build_group(model, p, group.quest_id, rect).unwrap_or_default();
            group.built_on = Some(rect);
            rebuilt = true;
        }
    }
    if rebuilt {
        cache.generation += 1;
    }
    let mut masks = [0u32; SLOTS];
    for (mask, group) in masks.iter_mut().zip(&cache.groups) {
        *mask = group
            .as_ref()
            .map_or(0, |g| objective_mask(model, g.quest_id));
    }
    let (area, dungeon) = model.worldmap.quest_blob_display();
    let key = BlobPaintKey {
        generation: cache.generation,
        area,
        dungeon,
        masks,
        fill_alpha: p.fill_alpha,
        border_alpha: p.border_alpha,
    };
    if let Some((held, painted)) = &cache.paint {
        if *held == key {
            return Some(painted.clone());
        }
    }
    let mut mesh = QuestBlobMesh {
        fill_texture: fill_texture.clone(),
        border_texture: border_texture.clone(),
        fill_alpha: p.fill_alpha,
        border_alpha: p.border_alpha,
        ..Default::default()
    };
    for (group, &mask) in cache.groups.iter().zip(&masks) {
        let Some(group) = group else { continue };
        for data in &group.blobs {
            if !data.valid
                || !on_current_map(model, data, area_map)
                || !(mask == u32::MAX || serves_objective(data.objective, mask))
            {
                continue;
            }
            let rim = data.fill.len() - 1;
            let centre = data.fill[rim];
            for i in 0..rim {
                let next = (i + 1) % rim;
                mesh.fill.push([data.fill[i], data.fill[next], centre]);
                // The strip's cell `2i, 2i+1, 2next, 2next+1` around its shared `2i+1`-`2next` edge.
                let (r0, o0, r1, o1) = (
                    data.border[2 * i],
                    data.border[2 * i + 1],
                    data.border[2 * next],
                    data.border[2 * next + 1],
                );
                mesh.border.push([o0, r0, r1, o1]);
            }
        }
    }
    let painted = QuestBlobPaint(Arc::new(mesh));
    cache.paint = Some((key, painted.clone()));
    Some(painted)
}

/// The blob's own centre on the displayed map (`0x5e0110`).
fn on_current_map(model: &Model, data: &BlobRender, area_map: u32) -> bool {
    data.map == area_map
        && projected(model, area_map, data.center.0 as f32, data.center.1 as f32).is_some()
}

/// The hover test (`0x58e0d0`): shown, open groups in slot order, the first group hit ending it.
pub(super) fn hover(
    model: &Model,
    p: &QuestPoiState,
    frame: FrameHandle,
    x: f32,
    y: f32,
) -> BlobHover {
    let mut out = BlobHover::default();
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return out;
    }
    let (Some(area_map), Some(rect)) = (
        model.worldmap.quest_blob_area(),
        model.resolved.get(&frame).copied(),
    ) else {
        return out;
    };
    let rect = BlobRect::from(rect);
    let point = [
        (rect.max_x - rect.min_x) * x + rect.min_x,
        (rect.max_y - rect.min_y) * (1.0 - y) + rect.min_y,
    ];
    let cache = p.cache.borrow();
    let mut count = 0u32;
    for group in cache.groups.iter().flatten() {
        if !model
            .quest_watch
            .poi(group.quest_id)
            .is_some_and(|s| s.shown)
        {
            continue;
        }
        let mask = objective_mask(model, group.quest_id);
        for data in &group.blobs {
            if mask == u32::MAX
                || !serves_objective(data.objective, mask)
                || !on_current_map(model, data, area_map)
                || !data.valid
                || !inside(point, &data.fill[..data.fill.len() - 1])
            {
                continue;
            }
            count = data.num_objectives;
            out.quest_id = group.quest_id;
            out.objectives[..count as usize].copy_from_slice(&data.objectives[..count as usize]);
        }
        if count != 0 {
            break;
        }
    }
    if out.quest_id != 0 && count != 0 {
        out.count = count;
    } else {
        out = BlobHover::default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smoothed_outline_closes_through_its_points() {
        let square = [(0, 0), (100, 0), (100, 100), (0, 100)];
        let out = blob_outline(&square, true, 20);
        assert_eq!(out.len(), 20);
        for &(x, y) in &out {
            assert!(
                (-30..=130).contains(&x) && (-30..=130).contains(&y),
                "{x},{y}"
            );
        }
        assert_eq!(blob_outline(&square, false, 20), square.to_vec());
    }

    #[test]
    fn the_crossing_test_is_even_odd() {
        let square = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(inside([0.5, 0.5], &square));
        assert!(!inside([1.5, 0.5], &square));
        assert!(!inside([0.5, -0.5], &square));
    }

    fn blob(objective: i32, min: f32, max: f32) -> BlobRender {
        let mut fill = vec![[min, min], [max, min], [max, max], [min, max]];
        fill.push([(min + max) * 0.5; 2]);
        BlobRender {
            objective,
            fill,
            bounds: [min, max, min, max],
            objectives: [objective, 0, 0, 0],
            num_objectives: 1,
            valid: true,
            ..Default::default()
        }
    }

    /// A blob wholly inside a larger one folds into it, its objective carried over; a lesser
    /// overlap leaves both (`0x58e310`).
    #[test]
    fn a_blob_inside_another_folds_into_it() {
        let mut blobs = vec![blob(0, 0.0, 1.0), blob(1, 0.2, 0.4), blob(2, 0.9, 1.5)];
        merge_blobs(&mut blobs, 0b111, 0.25);
        assert!(blobs[0].valid, "the larger survives");
        assert!(!blobs[1].valid, "the smaller folds in");
        assert!(blobs[2].valid, "under the threshold stays");
        assert_eq!(blobs[0].num_objectives, 2);
        assert_eq!(&blobs[0].objectives[..2], &[0, 1]);
    }

    /// A complete quest's mask merges nothing (`0x58e310`).
    #[test]
    fn a_complete_quest_merges_nothing() {
        let mut blobs = vec![blob(0, 0.0, 1.0), blob(1, 0.2, 0.4)];
        merge_blobs(&mut blobs, u32::MAX, 0.25);
        assert!(blobs.iter().all(|b| b.valid));
    }
}
