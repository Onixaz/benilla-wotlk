//! A `QuestPOIFrame`'s cached mesh to screen quads (`0x5901c0`), its own alphas alone.

use bevy::prelude::*;

use benilla_assets::WorldAssets;
use benilla_ui::widget::QuestBlobMesh;

use crate::ui_pass::{UiQuad, UvRect};

/// A frame-rect vertex (`0x58f1a0`) to a window pixel at the seam scale.
fn px(s: f32, h: f32, v: [f32; 2]) -> Vec2 {
    Vec2::new(v[0] * 1024.0 * s, h - v[1] * 1024.0 * s)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit(
    mesh: &QuestBlobMesh,
    rect: Rect,
    z: u64,
    s: f32,
    h: f32,
    assets: &mut WorldAssets,
    images: &mut Assets<Image>,
    out: &mut Vec<UiQuad>,
) {
    let (Some(fill), Some(border)) = (
        assets.sprite_texture(&mesh.fill_texture, images),
        assets.sprite_texture(&mesh.border_texture, images),
    ) else {
        return;
    };
    let fill_color = [1.0, 1.0, 1.0, f32::from(mesh.fill_alpha) / 255.0];
    let border_color = [1.0, 1.0, 1.0, f32::from(mesh.border_alpha) / 255.0];
    const RIM: [f32; 2] = [1.0, 1.0];
    const CENTRE: [f32; 2] = [0.0, 0.0];
    for tri in &mesh.fill {
        out.push(UiQuad {
            rect,
            z_key: z,
            texture: Some(fill.clone()),
            uv: UvRect::from_corners([RIM, RIM, CENTRE, CENTRE]),
            color: fill_color,
            corners: Some([
                px(s, h, tri[0]),
                px(s, h, tri[1]),
                px(s, h, tri[2]),
                px(s, h, tri[2]),
            ]),
            ..default()
        });
    }
    const INNER: [f32; 2] = [0.0, 0.0];
    const OUTER: [f32; 2] = [1.0, 1.0];
    for cell in &mesh.border {
        out.push(UiQuad {
            rect,
            z_key: z,
            texture: Some(border.clone()),
            uv: UvRect::from_corners([OUTER, INNER, INNER, OUTER]),
            color: border_color,
            corners: Some([
                px(s, h, cell[0]),
                px(s, h, cell[1]),
                px(s, h, cell[2]),
                px(s, h, cell[3]),
            ]),
            ..default()
        });
    }
}
