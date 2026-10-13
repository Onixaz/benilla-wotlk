//! A `Cooldown` widget's paint ([`CooldownPaint`]) to screen quads, in its draw order
//! (`0x5ebd20`): the dark wedge and the finish flash on ARTWORK, the edge on OVERLAY. Both arts
//! load clamped (`SetCooldown`'s texture flags, `0x5ecdd0`: no wrap bit), as every UI sprite here.

use bevy::prelude::*;

use benilla_assets::WorldAssets;
use benilla_ui::widget::{CooldownPaint, COOLDOWN_FLASH_RGB, COOLDOWN_SWIPE_ALPHA};

use crate::ui_pass::{UiQuad, UvRect};

/// The finish flash's art, drawn additively (`0x5ebda9`).
const STAR: &str = "Interface\\Cooldown\\star4.blp";
/// The edge line's art (`0x5ebdf3`).
const EDGE: &str = "Interface\\Cooldown\\edge.blp";

/// Push `paint` over `rect` (screen px, y down) at `z`, faded by the frame's `alpha` and cut to
/// `clip`.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit(
    paint: &CooldownPaint,
    rect: Rect,
    z: u64,
    alpha: f32,
    clip: Option<Rect>,
    assets: &mut WorldAssets,
    images: &mut Assets<Image>,
    out: &mut Vec<UiQuad>,
) {
    let at = |[x, y]: [f32; 2]| rect.min + Vec2::new(x, y) * rect.size();
    // The wedge: one triangle per rim step about the centre (`0xad13e8`), each cut to the clip,
    // since explicit corners bypass the quad clip. `star4.blp` is opaque, so its black tint alone
    // shows: the white texture stands in for it.
    let centre = at([0.5, 0.5]);
    let shade = [0.0, 0.0, 0.0, COOLDOWN_SWIPE_ALPHA * alpha];
    for pair in paint.swipe.windows(2) {
        let tri = [centre, at(pair[0]), at(pair[1])];
        for corners in clipped_fan(&tri, clip) {
            out.push(UiQuad {
                rect,
                z_key: z,
                color: shade,
                corners: Some(corners),
                ..default()
            });
        }
    }
    if let Some(flash) = &paint.flash {
        if let Some(texture) = assets.sprite_texture(STAR, images) {
            let [r, g, b] = COOLDOWN_FLASH_RGB;
            out.push(UiQuad {
                rect,
                z_key: z,
                texture: Some(texture),
                uv: UvRect::from_corners(flash.uvs),
                color: [r, g, b, flash.alpha * alpha],
                additive: true,
                clip,
                ..default()
            });
        }
    }
    // Deviation: the reference draws the edge with blend 0, `DISABLE` (`0x5ebdf3`; the names at
    // `0xa43fe8`), which would paint the art's black ground over the whole button; it is blended
    // here, so only the line shows. No stock file calls `SetDrawEdge`.
    if let Some(uvs) = paint.edge {
        if let Some(texture) = assets.sprite_texture(EDGE, images) {
            out.push(UiQuad {
                rect,
                z_key: z,
                texture: Some(texture),
                uv: UvRect::from_corners(uvs),
                color: [1.0, 1.0, 1.0, alpha],
                clip,
                ..default()
            });
        }
    }
}

/// A convex polygon cut to `clip` (Sutherland–Hodgman), as quads of the fan `(0,1,2)(0,2,3)`:
/// the triangle itself when there is no clip, nothing when it falls outside.
fn clipped_fan(tri: &[Vec2; 3], clip: Option<Rect>) -> Vec<[Vec2; 4]> {
    let mut poly: Vec<Vec2> = tri.to_vec();
    if let Some(c) = clip {
        // Each edge keeps the side where `inside` holds; `cross` meets the boundary.
        type Inside = fn(Vec2, Rect) -> bool;
        type Cross = fn(Vec2, Vec2, Rect) -> Vec2;
        let edges: [(Inside, Cross); 4] = [
            (
                |p, c| p.x >= c.min.x,
                |a, b, c| a + (b - a) * ((c.min.x - a.x) / (b.x - a.x)),
            ),
            (
                |p, c| p.x <= c.max.x,
                |a, b, c| a + (b - a) * ((c.max.x - a.x) / (b.x - a.x)),
            ),
            (
                |p, c| p.y >= c.min.y,
                |a, b, c| a + (b - a) * ((c.min.y - a.y) / (b.y - a.y)),
            ),
            (
                |p, c| p.y <= c.max.y,
                |a, b, c| a + (b - a) * ((c.max.y - a.y) / (b.y - a.y)),
            ),
        ];
        for (inside, cross) in edges {
            let input = std::mem::take(&mut poly);
            for (i, &b) in input.iter().enumerate() {
                let a = input[(i + input.len() - 1) % input.len()];
                match (inside(a, c), inside(b, c)) {
                    (true, true) => poly.push(b),
                    (true, false) => poly.push(cross(a, b, c)),
                    (false, true) => {
                        poly.push(cross(a, b, c));
                        poly.push(b);
                    }
                    (false, false) => {}
                }
            }
            if poly.len() < 3 {
                return Vec::new();
            }
        }
    }
    // Fan the polygon from its first point, two triangles per quad.
    let mut quads = Vec::new();
    let mut i = 1;
    while i + 1 < poly.len() {
        let c = poly.get(i + 2).copied().unwrap_or(poly[i + 1]);
        quads.push([poly[0], poly[i], poly[i + 1], c]);
        i += 2;
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unclipped_triangle_is_one_degenerate_quad() {
        let tri = [Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let q = clipped_fan(&tri, None);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0][2], q[0][3], "the fourth corner repeats the third");
    }

    #[test]
    fn a_clip_cuts_the_triangle_and_keeps_its_inside() {
        let tri = [Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let clip = Rect::new(0.0, 0.0, 5.0, 10.0);
        let quads = clipped_fan(&tri, Some(clip));
        assert!(!quads.is_empty());
        for q in &quads {
            for p in q {
                assert!(p.x <= 5.0 + 1e-4 && p.x >= -1e-4, "{p:?} escapes the clip");
            }
        }
        assert!(clipped_fan(&tri, Some(Rect::new(20.0, 20.0, 30.0, 30.0))).is_empty());
    }
}
