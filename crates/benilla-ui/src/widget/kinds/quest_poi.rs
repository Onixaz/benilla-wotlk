//! `QuestPOIFrame` (registered at `0x5210c1`): the world map's quest blob layer and its paint.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use crate::layout::Rect;

/// One blob's render data (`0x58f1a0`), in frame-rect units (1024 to a FrameXML unit's root).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlobRender {
    pub objective: i32,
    pub map: u32,
    pub center: (i32, i32),
    /// The outline, then the mean point last; empty when the blob failed to project.
    pub fill: Vec<[f32; 2]>,
    /// Two per outline point: the rim and the mitred outer edge.
    pub border: Vec<[f32; 2]>,
    /// `min_x, max_x, min_y, max_y`.
    pub bounds: [f32; 4],
    pub objectives: [i32; 4],
    pub num_objectives: u32,
    pub valid: bool,
}

/// One of the frame's four quest slots (`+0x2d0`).
#[derive(Clone, Debug, PartialEq)]
pub struct BlobGroup {
    pub quest_id: u32,
    /// The frame rect the group was built on, `None` while the rect was still empty (`0x5900f0`).
    pub built_on: Option<Rect>,
    pub blobs: Vec<BlobRender>,
}

/// What the cursor is over (`+0x2b8..+0x2c0`), written by `UpdateMouseOverTooltip`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlobHover {
    pub quest_id: u32,
    pub count: u32,
    pub objectives: [i32; 4],
}

/// The geometry the renderer draws: fill triangles about each blob's centre and border quads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestBlobMesh {
    pub fill_texture: String,
    pub border_texture: String,
    pub fill_alpha: u8,
    pub border_alpha: u8,
    /// `rim i, rim i+1, centre`, as the fill's index list reads (`0x58f1a0`).
    pub fill: Vec<[[f32; 2]; 3]>,
    /// One strip cell: `outer i, rim i, rim i+1, outer i+1`, the strip's shared edge first.
    pub border: Vec<[[f32; 2]; 4]>,
}

/// The mesh by pointer: a rebuild is the only thing that makes a new one.
#[derive(Clone, Debug)]
pub struct QuestBlobPaint(pub Arc<QuestBlobMesh>);

impl PartialEq for QuestBlobPaint {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What the held mesh was made from; equal again, the walk reuses it.
#[derive(Clone, Debug, PartialEq)]
pub struct BlobPaintKey {
    pub generation: u64,
    pub area: i32,
    pub dungeon: i32,
    pub masks: [u32; 4],
    pub fill_alpha: u8,
    pub border_alpha: u8,
}

/// The built groups and the mesh made from them, behind a cell as the extract is a read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestBlobCache {
    pub groups: [Option<BlobGroup>; 4],
    /// Moved by every add, remove, rebuild and texture change.
    pub generation: u64,
    pub paint: Option<(BlobPaintKey, QuestBlobPaint)>,
}

/// The blob layer's settings (`+0x29c..+0x2b4`) and its built state.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestPoiState {
    /// `SetFillTexture` and `SetBorderTexture` (`0x58fd30`, `0x58fdc0`).
    pub fill_texture: Option<String>,
    pub border_texture: Option<String>,
    /// `SetFillAlpha`, `SetBorderAlpha` (`+0x2a4`, `+0x2a5`), clamped to `[0, 255]`.
    pub fill_alpha: u8,
    pub border_alpha: u8,
    /// `SetBorderScalar` (`+0x2a8`), clamped to `[0, 10]`.
    pub border_scalar: f32,
    /// `EnableSmoothing`, `EnableMerging` (`+0x2ac`, `+0x2ad`).
    pub smoothing: bool,
    pub merging: bool,
    /// `SetMergeThreshold` (`+0x2b0`), clamped to `[0.1, 0.5]`.
    pub merge_threshold: f32,
    /// `SetNumSplinePoints` (`+0x2b4`), clamped to `[8, 30]`.
    pub spline_points: i32,
    pub cache: RefCell<QuestBlobCache>,
    pub hover: Cell<BlobHover>,
}

impl Default for QuestPoiState {
    /// The constructor's values (`0x58ff50`).
    fn default() -> Self {
        Self {
            fill_texture: None,
            border_texture: None,
            fill_alpha: 255,
            border_alpha: 255,
            border_scalar: 1.0,
            smoothing: true,
            merging: true,
            merge_threshold: 0.25,
            spline_points: 20,
            cache: RefCell::default(),
            hover: Cell::default(),
        }
    }
}
