//! `CGQuestPOIFrame` (`.?AVCGQuestPOIFrame@@`, registered at `0x5210c1`): the world map's quest
//! blob layer, a `Frame` that fills and borders the areas of the quests it is asked to draw. A
//! blob is built from the client's quest POI data (`0x5e3ac0`), which benilla does not read yet
//! (`SMSG_QUEST_POI_QUERY_RESPONSE`, #134), so the layer holds its settings and draws no blob.

/// The blob layer's settings (`+0x29c..+0x2b4`).
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
}

impl Default for QuestPoiState {
    fn default() -> Self {
        Self {
            fill_texture: None,
            border_texture: None,
            fill_alpha: 0,
            border_alpha: 0,
            border_scalar: 0.0,
            smoothing: false,
            merging: false,
            merge_threshold: 0.1,
            spline_points: 8,
        }
    }
}
