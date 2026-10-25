//! The placed frames: 12340's `layout-local.txt` (account-data slot 6, local only), written at the
//! UI shutdown (`SavePlacedFrames` `0x5234a0`) and read back after the UI loads
//! (`LoadPlacedFrames` `0x5183a0`). This is the engine half, a snapshot out and a restore in; the
//! app owns the file (`benilla_app::ui_layout`).

use crate::layout::Anchor;

use super::object::point_from_str;
use super::SCREEN;

/// One user-placed frame as 12340's `layout-local.txt` keeps it (`SavePlacedFrames` `0x5234a0`,
/// `PlacedFrameCallback` `0x517e00`): its level, the nearest of its nine points to the screen's
/// same point, the offset between the two and its size, in the frame's own units, rounded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedFrame {
    pub name: String,
    /// `FrameLevel:`; the reader takes -1 for a block without one and leaves the level.
    pub level: i32,
    /// `Anchor:`, a frame point's name.
    pub anchor: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The nine points in the frame-point enum's order, `TOPLEFT` first, as fractions of the rect's
/// width (from the left) and height (from the bottom).
const POINTS: [(&str, f32, f32); 9] = [
    ("TOPLEFT", 0.0, 1.0),
    ("TOP", 0.5, 1.0),
    ("TOPRIGHT", 1.0, 1.0),
    ("LEFT", 0.0, 0.5),
    ("CENTER", 0.5, 0.5),
    ("RIGHT", 1.0, 0.5),
    ("BOTTOMLEFT", 0.0, 0.0),
    ("BOTTOM", 0.5, 0.0),
    ("BOTTOMRIGHT", 1.0, 0.0),
];

impl super::UiScript {
    /// The frames `SavePlacedFrames` writes, in frame order: named, user-placed, movable or
    /// resizable, with a resolved rect. Each takes the point, of the nine, that lies nearest the
    /// screen's same point (the first on a tie), as `PlacedFrameCallback` picks it.
    pub fn placed_frames(&mut self) -> Vec<PlacedFrame> {
        let mut model = self.model_mut();
        super::UiScript::resolve_layout(&mut model);
        let screen = model.screen;
        model
            .arena
            .iter_frames()
            .filter(|(_, f)| f.user_placed && (f.movable || f.resizable))
            .filter_map(|(h, f)| {
                let name = f.name.clone()?;
                let r = *model.resolved.get(&h)?;
                let inv = 1.0 / super::object::layout_methods::eff_scale(&model, h);
                let (w, ht) = ((r.right - r.left) * inv, (r.top - r.bottom) * inv);
                let (sw, sh) = (
                    (screen.right - screen.left) * inv,
                    (screen.top - screen.bottom) * inv,
                );
                let (left, bottom) = (r.left * inv, r.bottom * inv);
                let mut best = (f32::MAX, POINTS[0].0, 0.0, 0.0);
                for (point, fx, fy) in POINTS {
                    let (dx, dy) = (left + fx * w - fx * sw, bottom + fy * ht - fy * sh);
                    let d = dx * dx + dy * dy;
                    if d < best.0 {
                        best = (d, point, dx, dy);
                    }
                }
                Some(PlacedFrame {
                    name,
                    level: i32::from(f.level),
                    anchor: best.1.to_owned(),
                    x: best.2.round_ties_even() as i32,
                    y: best.3.round_ties_even() as i32,
                    w: w.round_ties_even() as i32,
                    h: ht.round_ties_even() as i32,
                })
            })
            .collect()
    }

    /// `LoadPlacedFrames` (`0x5183a0`) seating each row on the frame of its name, if it exists: the
    /// level when the row has one, the anchor to the screen's same point when the frame is movable,
    /// the size when it is resizable, each arm marking the frame user-placed.
    pub fn place_frames(&mut self, rows: impl IntoIterator<Item = PlacedFrame>) {
        let mut model = self.model_mut();
        for row in rows {
            let Some(h) = model.arena.lookup(&row.name) else {
                continue; // a window this build, or this login, does not have
            };
            let Some((movable, resizable)) = model.arena.frame(h).map(|f| (f.movable, f.resizable))
            else {
                continue;
            };
            if let Ok(level) = u16::try_from(row.level) {
                model.arena.set_frame_level(h, level, true);
            }
            let point = point_from_str(&row.anchor);
            let Some(input) = model.layout_inputs.get_mut(&h) else {
                continue;
            };
            let mut placed = false;
            if let (true, Some(point)) = (movable, point) {
                input.anchors = vec![Anchor::new(
                    point,
                    SCREEN,
                    point,
                    row.x as f32,
                    row.y as f32,
                )];
                placed = true;
            }
            if resizable && (row.w != 0 || row.h != 0) {
                input.width = row.w as f32;
                input.height = row.h as f32;
                placed = true;
            }
            if placed {
                if let Some(f) = model.arena.frame_mut(h) {
                    f.user_placed = true;
                }
                model.touch_layout();
            }
        }
    }
}
