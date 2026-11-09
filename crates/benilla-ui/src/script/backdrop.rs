//! The frame backdrop: the data `<Backdrop>` or `SetBackdrop` installs ([`Backdrop`]), the nine
//! Texture regions it builds on the frame ([`set_backdrop`], [`generate`]), and the quad geometry
//! ([`pieces`]) the world chat bubble draws its own backdrop from.

use crate::layout::{Anchor, Point, Rect};
use crate::order::DrawLayer;
use crate::widget::{FrameHandle, HandleMap, RegionHandle, RegionKind};

use super::{BlendMode, Model, RegionData, TexCoords};

/// The default `edgeSize`, logical 32: the constructor writes device 0.025.
pub const DEFAULT_EDGE_SIZE: f32 = 32.0;

/// `BackgroundInsets`: the background's inset from the frame rect; the border is never inset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

/// A frame's installed backdrop (the 0x68-byte struct at `frame+0x1ac`): exactly the keys the
/// reader accepts plus the two colours. The colours default to opaque white; the XML parser
/// defaults a present but partial `<Color>` or `<BorderColor>` to black.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    /// The background texture (`+0x24`); none or empty draws no background.
    pub bg_file: Option<String>,
    /// The 8-slice border texture (`+0x30`); none or empty draws no border.
    pub edge_file: Option<String>,
    /// Tile the background, else stretch it (`+0x40`).
    pub tile: bool,
    /// `tileSize` in logical px (`+0x4c`); 0 tiles at `edge_size`.
    pub tile_size: f32,
    /// `edgeSize` in logical px (`+0x48`): the border piece size and the edge tiling period.
    pub edge_size: f32,
    /// `BackgroundInsets` (`+0x50..0x5c`).
    pub insets: Insets,
    /// The background's vertex colour (`+0x60`, `SetBackdropColor`).
    pub bg_color: [f32; 4],
    /// All 8 border pieces' vertex colour (`+0x64`, `SetBackdropBorderColor`).
    pub border_color: [f32; 4],
    /// The XML `alphaMode` the eight edges take, BLEND by default (`0x4a2780`, `0x481620`).
    pub edge_blend: BlendMode,
}

impl Default for Backdrop {
    fn default() -> Self {
        // The constructor.
        Self {
            bg_file: None,
            edge_file: None,
            tile: false,
            tile_size: 0.0,
            edge_size: DEFAULT_EDGE_SIZE,
            insets: Insets::default(),
            bg_color: [1.0, 1.0, 1.0, 1.0],
            border_color: [1.0, 1.0, 1.0, 1.0],
            edge_blend: BlendMode::Blend,
        }
    }
}

impl Backdrop {
    fn has_bg(&self) -> bool {
        self.bg_file.as_deref().is_some_and(|s| !s.is_empty())
    }
    fn has_border(&self) -> bool {
        self.edge_file.as_deref().is_some_and(|s| !s.is_empty())
    }
}

/// One drawable piece, the background or one of the 8 border pieces: explicit corners, not a rect,
/// so the bottom-right inset quirk can slant it, and per-corner UVs, since the top and bottom edges
/// are rotated 90°. Both arrays run `[TL, TR, BR, BL]`, y-up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackdropPiece {
    /// Corner positions in screen px.
    pub corners: [[f32; 2]; 4],
    /// Corner UVs in texture space.
    pub uvs: [[f32; 2]; 4],
    /// The texture needs repeat addressing. The corners stay in `[0,1]` but share the edge texture,
    /// so they carry `true` too and the border batches as one texture.
    pub tile: bool,
    /// Drawn with `bg_file` and `bg_color`, else with `edge_file` and `border_color`.
    pub is_bg: bool,
}

const SLICE: f32 = 0.125;

/// The nine texture slots of the backdrop object (`0x4a2d40`).
pub(crate) const SLOT_COUNT: usize = 9;

/// A frame's backdrop textures, indexed by slot; a slot the file gate left empty is `None`.
pub(crate) type BackdropSlots = [Option<RegionHandle>; SLOT_COUNT];

/// `0x4a2e00` builds them in this order: background, the four edges, the four corners.
const BG: usize = 0;
const LEFT: usize = 1;
const RIGHT: usize = 2;
const TOP: usize = 3;
const BOTTOM: usize = 4;
const TOP_LEFT: usize = 5;
const TOP_RIGHT: usize = 6;
const BOTTOM_LEFT: usize = 7;
const BOTTOM_RIGHT: usize = 8;

/// The `v` inset of every edge texcoord (`0x4a29d0`, `0x4a2e00`).
const V_INSET: f32 = 0.0625;
/// The bounded `u` span of the four edges, `[left, right]`, indexed by slot (`0x4a29d0`).
const EDGE_U: [[f32; 2]; 4] = [
    [0.0078125, 0.1171875],
    [0.1328125, 0.2421875],
    [0.2578125, 0.3671875],
    [0.3828125, 0.4921875],
];
/// The `u` span of the four corners, `[left, right]` (`0x4a2e00`).
const CORNER_U: [[f32; 2]; 4] = [
    [0.5078125, 0.6171875],
    [0.6328125, 0.7421875],
    [0.7578125, 0.8671875],
    [0.8828125, 0.9921875],
];

/// Frees the frame's old textures, then links the new ones at the region list's tail (`0x4905a0`).
pub(crate) fn set_backdrop(model: &mut Model, h: FrameHandle, bd: Option<Backdrop>) {
    if let Some(old) = model.backdrop_slots.remove(&h) {
        for rh in old.into_iter().flatten() {
            super::region::free_region(model, rh);
        }
    }
    let Some(bd) = bd else {
        model.backdrops.remove(&h);
        return;
    };
    let slots = build_slots(model, h, &bd);
    model.backdrops.insert(h, bd);
    model.backdrop_slots.insert(h, slots);
    // `SetBackdrop` runs the texcoords over the rect the frame already has (`0x4a15a0`).
    if let Some(&rect) = model.resolved.get(&h) {
        generate(
            &model.backdrops,
            &model.backdrop_slots,
            &mut model.region_data,
            h,
            rect,
        );
    }
}

/// Builds the slots the two files gate (`0x4a2e00`): layer 0 for the background, 1 for the rest.
fn build_slots(model: &mut Model, h: FrameHandle, bd: &Backdrop) -> BackdropSlots {
    let id = model.frame_id(h);
    let at = |p: Point, x: f32, y: f32| Anchor::new(p, id, p, x, y);
    let mut slots: BackdropSlots = [None; SLOT_COUNT];
    if bd.has_bg() {
        let i = bd.insets;
        // `0x4a2e00` loads the top inset (`+0x50`) for the bottom-right anchor's y.
        let anchors = vec![
            at(Point::TopLeft, i.left, -i.top),
            at(Point::TopRight, -i.right, -i.top),
            at(Point::BottomLeft, i.left, i.bottom),
            at(Point::BottomRight, -i.right, i.top),
        ];
        slots[BG] = add_piece(
            model,
            h,
            DrawLayer::Background,
            PieceSpec {
                file: bd.bg_file.clone(),
                size: None,
                anchors,
                coords: None,
                tile: bd.tile,
                color: bd.bg_color,
                blend: BlendMode::Blend,
            },
        );
    }
    if bd.has_border() {
        let e = bd.edge_size;
        let corner = |u: [f32; 2]| {
            let (t, b) = (V_INSET, 1.0 - V_INSET);
            TexCoords::Corners([[u[0], t], [u[1], t], [u[1], b], [u[0], b]])
        };
        let mut rows = vec![
            (
                LEFT,
                Some((e, 0.0)),
                vec![at(Point::TopLeft, 0.0, -e), at(Point::BottomLeft, 0.0, e)],
                None,
            ),
            (
                RIGHT,
                Some((e, 0.0)),
                vec![at(Point::TopRight, 0.0, -e), at(Point::BottomRight, 0.0, e)],
                None,
            ),
            (
                TOP,
                Some((0.0, e)),
                vec![at(Point::TopLeft, e, 0.0), at(Point::TopRight, -e, 0.0)],
                None,
            ),
            (
                BOTTOM,
                Some((0.0, e)),
                vec![
                    at(Point::BottomLeft, e, 0.0),
                    at(Point::BottomRight, -e, 0.0),
                ],
                None,
            ),
        ];
        for (slot, p, u) in [
            (TOP_LEFT, Point::TopLeft, CORNER_U[0]),
            (TOP_RIGHT, Point::TopRight, CORNER_U[1]),
            (BOTTOM_LEFT, Point::BottomLeft, CORNER_U[2]),
            (BOTTOM_RIGHT, Point::BottomRight, CORNER_U[3]),
        ] {
            rows.push((slot, Some((e, e)), vec![at(p, 0.0, 0.0)], Some(corner(u))));
        }
        for (slot, size, anchors, coords) in rows {
            slots[slot] = add_piece(
                model,
                h,
                DrawLayer::Border,
                PieceSpec {
                    file: bd.edge_file.clone(),
                    size,
                    anchors,
                    coords,
                    // Deviation: `0x4a2e00` clamps the corners; wrapping all eight shares one atlas handle.
                    tile: true,
                    color: bd.border_color,
                    blend: bd.edge_blend,
                },
            );
        }
    }
    slots
}

/// One slot's texture as the builder hands it to its region.
struct PieceSpec {
    file: Option<String>,
    size: Option<(f32, f32)>,
    anchors: Vec<Anchor>,
    coords: Option<TexCoords>,
    tile: bool,
    color: [f32; 4],
    blend: BlendMode,
}

fn add_piece(
    model: &mut Model,
    h: FrameHandle,
    layer: DrawLayer,
    spec: PieceSpec,
) -> Option<RegionHandle> {
    let rh = model
        .arena
        .create_region(h, RegionKind::Texture, layer, 0)?;
    model.region_data.insert(
        rh,
        RegionData {
            texture: spec.file,
            size: spec.size,
            anchors: spec.anchors,
            tex_coords: spec.coords,
            vertex_color: Some(spec.color),
            blend: spec.blend,
            backdrop_tile: Some(spec.tile),
            ..Default::default()
        },
    );
    // Minted last, so the cached layout graph adopts the region with its anchors in place.
    model.region_id(rh);
    Some(rh)
}

/// Rewrites a backdrop's size-dependent texcoords and re-applies both colours (`0x4a29d0`).
pub(crate) fn generate(
    backdrops: &HandleMap<FrameHandle, Backdrop>,
    backdrop_slots: &HandleMap<FrameHandle, BackdropSlots>,
    region_data: &mut HandleMap<RegionHandle, RegionData>,
    h: FrameHandle,
    rect: Rect,
) {
    let (Some(bd), Some(slots)) = (backdrops.get(&h), backdrop_slots.get(&h)) else {
        return;
    };
    let (w, ht) = (rect.right - rect.left, rect.top - rect.bottom);
    let period = bd.edge_size;
    let tiles_x = (w / period - 2.0).max(0.0);
    let tiles_y = (ht / period - 2.0).max(0.0);
    if bd.tile && bd.has_bg() {
        let tile = if bd.tile_size != 0.0 {
            bd.tile_size
        } else {
            period
        };
        let (u, v) = (w / tile, ht / tile);
        set_coords(
            region_data,
            slots[BG],
            [[0.0, 0.0], [u, 0.0], [u, v], [0.0, v]],
        );
    }
    if bd.has_border() {
        let v0 = V_INSET;
        for (slot, u) in [(LEFT, EDGE_U[0]), (RIGHT, EDGE_U[1])] {
            let v1 = tiles_y - V_INSET;
            set_coords(
                region_data,
                slots[slot],
                [[u[0], v0], [u[1], v0], [u[1], v1], [u[0], v1]],
            );
        }
        // TOP and BOTTOM are turned: their atlas `u` runs down the screen, `v` right to left.
        for (slot, u) in [(TOP, EDGE_U[2]), (BOTTOM, EDGE_U[3])] {
            let v1 = tiles_x - V_INSET;
            set_coords(
                region_data,
                slots[slot],
                [[u[0], v1], [u[0], v0], [u[1], v0], [u[1], v1]],
            );
        }
    }
    apply_color(region_data, slots, false, bd.bg_color);
    apply_color(region_data, slots, true, bd.border_color);
}

fn set_coords(
    region_data: &mut HandleMap<RegionHandle, RegionData>,
    slot: Option<RegionHandle>,
    corners: [[f32; 2]; 4],
) {
    if let Some(d) = slot.and_then(|rh| region_data.get_mut(&rh)) {
        d.tex_coords = Some(TexCoords::Corners(corners));
    }
}

/// Writes the background's colour, or the eight border pieces', to the slots (`0x4a25f0`, `0x4a2630`).
fn apply_color(
    region_data: &mut HandleMap<RegionHandle, RegionData>,
    slots: &BackdropSlots,
    border: bool,
    color: [f32; 4],
) {
    for rh in color_slots(slots, border).into_iter().flatten() {
        if let Some(d) = region_data.get_mut(&rh) {
            d.vertex_color = Some(color);
        }
    }
}

/// The slots one colour tints: the background alone, or the eight border pieces.
fn color_slots(slots: &BackdropSlots, border: bool) -> Vec<Option<RegionHandle>> {
    if border {
        slots[LEFT..].to_vec()
    } else {
        vec![slots[BG]]
    }
}

/// The XML `alphaMode` on the eight edge textures (`0x4a2780`, `0x481620`).
pub(crate) fn set_edge_blend(model: &mut Model, h: FrameHandle, blend: BlendMode) {
    if let Some(bd) = model.backdrops.get_mut(&h) {
        bd.edge_blend = blend;
    }
    if let Some(slots) = model.backdrop_slots.get(&h).copied() {
        for rh in slots[LEFT..].iter().flatten() {
            if let Some(d) = model.region_data.get_mut(rh) {
                d.blend = blend;
            }
        }
    }
}

/// `SetBackdropColor` or `SetBackdropBorderColor`: the stored colour and its slots, paint only.
pub(crate) fn set_color(
    model: &mut super::model::ModelCell,
    h: FrameHandle,
    border: bool,
    color: [f32; 4],
) {
    let Some(slots) = model.backdrop_slots.get(&h).copied() else {
        return;
    };
    let targets: Vec<crate::order::ZTarget> = color_slots(&slots, border)
        .into_iter()
        .flatten()
        .map(crate::order::ZTarget::Region)
        .collect();
    let inner = model.paint_mut(&targets);
    if let Some(bd) = inner.backdrops.get_mut(&h) {
        if border {
            bd.border_color = color;
        } else {
            bd.bg_color = color;
        }
    }
    apply_color(&mut inner.region_data, &slots, border, color);
}

/// A slot still drawing its plain file, which goes out as one atlas quad; anything else is a Texture.
pub(crate) fn plain_piece(d: &RegionData) -> Option<(&String, bool)> {
    let tile = d.backdrop_tile?;
    if d.fill.is_some()
        || d.gradient.is_some()
        || d.blend != BlendMode::Blend
        || d.circular
        || d.portrait_unit.is_some()
        || d.rotation != 0.0
        || d.desaturated
    {
        return None;
    }
    d.texture.as_ref().map(|p| (p, tile))
}

/// A slot's drawn UVs in `[TL, TR, BR, BL]`: its texcoords, or the full texture.
pub(crate) fn piece_uvs(d: &RegionData) -> [[f32; 2]; 4] {
    match d.tex_coords {
        Some(TexCoords::Corners(c)) => c,
        Some(TexCoords::Rect([l, r, t, b])) => [[l, t], [r, t], [r, b], [l, b]],
        None => [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    }
}

/// Pull a piece's UVs half a texel inward on every axis that does not tile, so bilinear
/// magnification cannot sample the neighbouring slice of the shared border atlas (in
/// `ChatBubble-Backdrop`, a white alpha-0 column: a pale line through the speech bubble). A tiling
/// axis, a strip's `v` past 1, is left exact, or each period would land short and walk the art.
#[must_use]
pub fn inset_atlas_bleed(uvs: [[f32; 2]; 4], tex_w: f32, tex_h: f32) -> [[f32; 2]; 4] {
    let mut out = uvs;
    for (axis, tex) in [(0usize, tex_w), (1usize, tex_h)] {
        if tex < 2.0 {
            continue; // a 1-texel axis has no interior
        }
        let (lo, hi) = uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
            (lo.min(c[axis]), hi.max(c[axis]))
        });
        // A range of exactly 1, the corners' `v`, is bounded and gets the inset.
        if hi - lo > 1.0 + f32::EPSILON {
            continue;
        }
        let half = 0.5 / tex;
        if hi - lo <= 2.0 * half {
            continue; // degenerate: the inset would invert the range
        }
        for c in &mut out {
            c[axis] += if c[axis] <= lo { half } else { -half };
        }
    }
    out
}

/// Axis-aligned corners `[TL, TR, BR, BL]`, y-up.
fn aa_corners(x0: f32, x1: f32, y_bottom: f32, y_top: f32) -> [[f32; 2]; 4] {
    [
        [x0, y_top],    // TL
        [x1, y_top],    // TR
        [x1, y_bottom], // BR
        [x0, y_bottom], // BL
    ]
}

/// The drawable pieces for a frame's resolved rect (y-up) at its effective `scale`, in paint order:
/// the background (layer `BACKGROUND`), then the 8 border pieces (layer `BORDER`). The insets and
/// `edgeSize` place the frame's own textures (`0x4a2e00`), so they scale with it; the texcoord runs
/// are the scaled rect over the unscaled sizes (`0x4a29d0`), so a scaled frame repeats its art.
pub fn pieces(frame: Rect, bd: &Backdrop, scale: f32) -> Vec<BackdropPiece> {
    let mut out = Vec::with_capacity(9);
    let period = bd.edge_size;
    let e = period * scale;
    let (l, r, b, t) = (frame.left, frame.right, frame.bottom, frame.top);

    // ── The background piece ──
    if bd.has_bg() {
        let (il, ir, it, ib) = (
            bd.insets.left * scale,
            bd.insets.right * scale,
            bd.insets.top * scale,
            bd.insets.bottom * scale,
        );
        // The four anchors, y-up. Bottom-right takes the top inset, as the reference does (it loads
        // `+0x50`, top, not `+0x54`); invisible when the two are equal.
        let corners = [
            [l + il, t - it],
            [r - ir, t - it],
            [r - ir, b + it], // BR: the top inset
            [l + il, b + ib],
        ];
        // Untiled, no `SetTexCoord` runs and the background stretches over `[0,1]`.
        let uvs = if bd.tile {
            let tile = if bd.tile_size != 0.0 {
                bd.tile_size
            } else {
                period
            };
            // The whole rect, insets and all (`0x4a29d0`).
            let (wt, ht) = ((r - l) / tile, (t - b) / tile);
            [[0.0, 0.0], [wt, 0.0], [wt, ht], [0.0, ht]]
        } else {
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        };
        out.push(BackdropPiece {
            corners,
            uvs,
            tile: bd.tile,
            is_bg: true,
        });
    }

    // ── The 8 border pieces ──
    if bd.has_border() {
        // Edge strips tile `run` periods between the corners, 0 below two edges.
        let w_run = ((r - l) / period - 2.0).max(0.0);
        let h_run = ((t - b) / period - 2.0).max(0.0);

        let mut edge = |x0: f32, x1: f32, y0: f32, y1: f32, uvs: [[f32; 2]; 4]| {
            out.push(BackdropPiece {
                corners: aa_corners(x0, x1, y0, y1),
                uvs,
                tile: true,
                is_bg: false,
            });
        };

        // LEFT, slice 0, upright.
        edge(
            l,
            l + e,
            b + e,
            t - e,
            [[0.0, 0.0], [SLICE, 0.0], [SLICE, h_run], [0.0, h_run]],
        );
        // RIGHT, slice 1, upright.
        edge(
            r - e,
            r,
            b + e,
            t - e,
            [
                [SLICE, 0.0],
                [2.0 * SLICE, 0.0],
                [2.0 * SLICE, h_run],
                [SLICE, h_run],
            ],
        );
        // TOP, slice 2, rotated: atlas `u` runs down the screen, atlas `v` right to left.
        edge(
            l + e,
            r - e,
            t - e,
            t,
            [
                [2.0 * SLICE, w_run],
                [2.0 * SLICE, 0.0],
                [3.0 * SLICE, 0.0],
                [3.0 * SLICE, w_run],
            ],
        );
        // BOTTOM, slice 3, rotated as TOP.
        edge(
            l + e,
            r - e,
            b,
            b + e,
            [
                [3.0 * SLICE, w_run],
                [3.0 * SLICE, 0.0],
                [4.0 * SLICE, 0.0],
                [4.0 * SLICE, w_run],
            ],
        );
        // The corners, slices 4 to 7, upright.
        let corner_uv = |i: f32| {
            let u0 = i * SLICE;
            let u1 = (i + 1.0) * SLICE;
            [[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]]
        };
        edge(l, l + e, t - e, t, corner_uv(4.0)); // TOPLEFT
        edge(r - e, r, t - e, t, corner_uv(5.0)); // TOPRIGHT
        edge(l, l + e, b, b + e, corner_uv(6.0)); // BOTTOMLEFT
        edge(r - e, r, b, b + e, corner_uv(7.0)); // BOTTOMRIGHT
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On the `ChatBubble-Backdrop` geometry: 256×32, eight 32-texel slices.
    #[test]
    fn the_bleed_inset_lands_on_texel_centres_and_spares_tiling() {
        const W: f32 = 256.0;
        const H: f32 = 32.0;
        let half_u = 0.5 / W;

        let corner = [[0.5, 0.0], [0.625, 0.0], [0.625, 1.0], [0.5, 1.0]];
        let got = inset_atlas_bleed(corner, W, H);
        // 128.5 and 159.5 are the centres of the slice's first and last texels.
        let us: Vec<f32> = got.iter().map(|c| c[0] * W).collect();
        assert!((us[0] - 128.5).abs() < 1e-3, "left u at {} texels", us[0]);
        assert!((us[1] - 159.5).abs() < 1e-3, "right u at {} texels", us[1]);
        let vs: Vec<f32> = got.iter().map(|c| c[1] * H).collect();
        assert!((vs[0] - 0.5).abs() < 1e-3, "top v at {} texels", vs[0]);
        assert!((vs[2] - 31.5).abs() < 1e-3, "bottom v at {} texels", vs[2]);

        let strip = [[0.25, 8.0], [0.25, 0.0], [0.375, 0.0], [0.375, 8.0]];
        let got = inset_atlas_bleed(strip, W, H);
        for (g, s) in got.iter().zip(strip.iter()) {
            assert_eq!(g[1], s[1], "the tiling axis must be bit-exact");
        }
        assert!((got[0][0] - (0.25 + half_u)).abs() < 1e-6);
        assert!((got[2][0] - (0.375 - half_u)).abs() < 1e-6);

        for i in 0u8..8 {
            let (u0, u1) = (f32::from(i) * SLICE, f32::from(i + 1) * SLICE);
            let piece = [[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]];
            for c in inset_atlas_bleed(piece, W, H) {
                assert!(
                    c[0] > u0 && c[0] < u1,
                    "slice {i}: u {} escaped ({u0},{u1})",
                    c[0]
                );
            }
        }

        assert_eq!(inset_atlas_bleed(corner, 0.0, 0.0), corner);
        let sliver = [[0.0, 0.0], [0.001, 0.0], [0.001, 1.0], [0.0, 1.0]];
        assert_eq!(
            inset_atlas_bleed(sliver, W, H)[0][0],
            0.0,
            "too thin to inset"
        );
    }

    fn tooltip_backdrop(edge_size: f32, inset: f32) -> Backdrop {
        Backdrop {
            bg_file: Some("Interface\\Tooltips\\UI-Tooltip-Background".into()),
            edge_file: Some("Interface\\Tooltips\\UI-Tooltip-Border".into()),
            tile: true,
            tile_size: edge_size,
            edge_size,
            insets: Insets {
                left: inset,
                right: inset,
                top: inset,
                bottom: inset,
            },
            ..Default::default()
        }
    }

    // At 0.5 the pieces are the frame's own textures, half size, while the texcoord runs stay the
    // scaled rect over the unscaled sizes, so the art repeats half as often (`0x4a29d0`).
    #[test]
    fn a_scaled_frame_halves_the_pieces_and_keeps_the_unscaled_divisors() {
        let frame = Rect::new(0.0, 0.0, 50.0, 100.0);
        let bd = tooltip_backdrop(16.0, 4.0);
        let ps = pieces(frame, &bd, 0.5);
        assert_eq!(ps.len(), 9);

        let bg = ps[0];
        assert_eq!(bg.corners[0], [2.0, 48.0]);
        assert_eq!(bg.corners[2], [98.0, 2.0]);
        assert!((bg.uvs[1][0] - 100.0 / 16.0).abs() < 1e-4);
        assert!((bg.uvs[2][1] - 50.0 / 16.0).abs() < 1e-4);

        // LEFT: 8 wide between the 8-tall corners, with a 50/16 - 2 run.
        let left = ps[1];
        assert_eq!(
            left.corners,
            [[0.0, 42.0], [8.0, 42.0], [8.0, 8.0], [0.0, 8.0]]
        );
        assert!((left.uvs[3][1] - 1.125).abs() < 1e-4);
        // TOP: 100/16 - 2.
        assert!((ps[3].uvs[0][1] - 4.25).abs() < 1e-4);
    }

    // The tooltip's shape on a 200×100 frame (`Rect::new` takes bottom, left, top, right).
    #[test]
    fn piece_geometry_200x100_edge16_inset5() {
        let frame = Rect::new(0.0, 0.0, 100.0, 200.0);
        let bd = tooltip_backdrop(16.0, 5.0);
        let ps = pieces(frame, &bd, 1.0);
        assert_eq!(ps.len(), 9);

        // Symmetric insets hide the bottom-right quirk.
        let bg = ps[0];
        assert!(bg.is_bg);
        assert_eq!(bg.corners[0], [5.0, 95.0]); // TL
        assert_eq!(bg.corners[1], [195.0, 95.0]); // TR
        assert_eq!(bg.corners[2], [195.0, 5.0]); // BR
        assert_eq!(bg.corners[3], [5.0, 5.0]); // BL
        assert_eq!(bg.uvs[0], [0.0, 0.0]);
        // The whole rect over the tile, insets and all.
        assert!((bg.uvs[1][0] - 200.0 / 16.0).abs() < 1e-4);
        assert!((bg.uvs[2][1] - 100.0 / 16.0).abs() < 1e-4);

        // Border pieces 1 to 8: LEFT, RIGHT, TOP, BOTTOM, TL, TR, BL, BR, flush inside the frame.
        let left = ps[1];
        assert!(!left.is_bg && left.tile);
        assert_eq!(
            left.corners,
            [[0.0, 84.0], [16.0, 84.0], [16.0, 16.0], [0.0, 16.0]]
        );
        // h_run = 100/16 - 2 = 4.25.
        assert_eq!(left.uvs[0], [0.0, 0.0]);
        assert_eq!(left.uvs[1], [0.125, 0.0]);
        assert!((left.uvs[3][1] - 4.25).abs() < 1e-4);

        // TOP, rotated; w_run = 200/16 - 2 = 10.5.
        let top = ps[3];
        assert_eq!(top.corners[0], [16.0, 100.0]); // TL
        assert_eq!(top.corners[2], [184.0, 84.0]); // BR
        assert_eq!(top.uvs[0][0], 0.25); // TL u
        assert_eq!(top.uvs[3][0], 0.375); // BL u
        assert!((top.uvs[0][1] - 10.5).abs() < 1e-4); // TL v = w_run
        assert_eq!(top.uvs[1][1], 0.0); // TR v = 0

        // BOTTOMRIGHT, slice 7, upright.
        let br = ps[8];
        assert_eq!(br.corners[0], [184.0, 16.0]); // TL
        assert_eq!(br.uvs[0], [0.875, 0.0]);
        assert_eq!(br.uvs[2], [1.0, 1.0]);
    }

    #[test]
    fn bg_bottomright_rides_top_inset_bug() {
        let frame = Rect::new(0.0, 0.0, 100.0, 200.0);
        let bd = Backdrop {
            insets: Insets {
                left: 3.0,
                right: 4.0,
                top: 7.0,
                bottom: 9.0,
            },
            ..tooltip_backdrop(16.0, 0.0)
        };
        let bg = pieces(frame, &bd, 1.0)[0];
        assert_eq!(bg.corners[3], [3.0, 9.0]);
        // BR takes the top inset, 7, not the bottom's 9.
        assert_eq!(bg.corners[2], [196.0, 7.0]);
        assert_eq!(bg.corners[0], [3.0, 93.0]); // TL
        assert_eq!(bg.corners[1], [196.0, 93.0]); // TR
    }

    #[test]
    fn run_clamps_to_zero_below_two_edges() {
        // 20/16 - 2 = -0.75.
        let frame = Rect::new(0.0, 0.0, 20.0, 20.0);
        let bd = tooltip_backdrop(16.0, 5.0);
        let ps = pieces(frame, &bd, 1.0);
        let left = ps[1]; // LEFT edge
        assert_eq!(left.uvs[3][1], 0.0, "h_run clamped to 0"); // BL v
        let top = ps[3]; // TOP edge
        assert_eq!(top.uvs[0][1], 0.0, "w_run clamped to 0"); // TL v
    }

    #[test]
    fn presence_gates_pieces() {
        let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!(pieces(frame, &Backdrop::default(), 1.0).is_empty());
        let bg_only = Backdrop {
            bg_file: Some("bg".into()),
            ..Default::default()
        };
        assert_eq!(pieces(frame, &bg_only, 1.0).len(), 1);
        let border_only = Backdrop {
            edge_file: Some("edge".into()),
            ..Default::default()
        };
        assert_eq!(pieces(frame, &border_only, 1.0).len(), 8);
    }
}
