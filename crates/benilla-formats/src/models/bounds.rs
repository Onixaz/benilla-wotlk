//! An M2's authored bounds ([`M2Bounds`]): the header sphere and box, and the Stand-sequence box
//! reads that size the selection ring, the chat bubble and the camera pivot.

use anyhow::Result;
use benilla_m2::{M2Model, M2Sequence};

use crate::Chain;

use super::read_model;

/// An M2's authored bounds and the reads sized from them, in model-local yards before the
/// placement scale.
#[derive(Clone, Copy, Debug)]
pub struct M2Bounds {
    /// The authored bounding-sphere radius; times the placement scale it is the reference's
    /// doodad radius `rec+0x68` (`0x6952a0`).
    pub sphere_radius: f32,
    /// The authored box; its centre is the sphere's.
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// The largest vertex distance from the model origin.
    pub vert_max_from_origin: f32,
    /// The selection ring's footprint: `sqrt(0.5 · sqrt(dx² + dy²))` over the Stand box's X and Y
    /// extents, the reference's living-unit ring input (`0x608e00`/`0x60aee0`); the ring's world
    /// radius is this × `OBJECT_FIELD_SCALE_X`. A model with no sequences uses the header box, and
    /// a box with zero X and Y extents takes [`DEGENERATE_RING_FOOTPRINT`].
    pub ring_footprint: f32,
    /// Model-space Z of attachment 17, the reference's follow-camera pivot height (`0x50ca90`:
    /// `feet + (attach17.z + 0.0972)·scale`); without one the camera uses a fraction of the box.
    pub pivot_z: Option<f32>,
    /// The Stand box's Z extent, the chat bubble's anchor above the feet (times the scale, plus
    /// 0.7 yd): `0x4b0e38` calls `0x711a20`, which reads the MD20 file image with no bone matrix
    /// and returns `out+0x20 − out+0x14`. The overhead name instead uses `0x608640`, the live posed
    /// attachment, which bobs. The header box's Z for a model with no sequences.
    pub stand_box_z: f32,
    /// How far the camera pivot drops while swimming: Stand's box `max.z` less Swim's, floored at
    /// zero, and `0.0` without a Swim sequence. The reference adds the neck height
    /// (`attach17.z + 0.0972222`, `0x808ab0`) to three presets, `cam+0x11c`/`+0x120`/`+0x124`
    /// (`0x50cc0c`–`0x50cc2e`, built by `0x50ca90`), then lowers only the swim one by
    /// `S · (box0.max.z − box42.max.z)` (`0x50ccf6`), and only when `0x711960` finds ids 0 and 42
    /// (`0x50cc67`–`0x50cd02`); `0x50f880` picks a preset per frame, then the `[5/6, 15.0]` clamp
    /// (`0x50d00d`–`0x50d092`). `HumanMale.m2` drops `0.3882572` at scale 1. The reference also
    /// picks zoomed-in `+0x11c` or zoomed-out `+0x120` on `cam+0x198 < 1.8315`; benilla has one
    /// standing preset, and what sets those two apart is untraced.
    pub swim_pivot_drop: f32,
}

/// Read an M2's bounds from the chain, by model or doodad (`.mdx`/`.mdl`) path.
pub fn load_m2_bounds(chain: &mut Chain, raw_path: &str) -> Result<M2Bounds> {
    let (_, model) = read_model(chain, raw_path)?;
    Ok(parse_m2_bounds(&model))
}

/// The ring footprint the reference stores for a box with zero X and Y extents: the writer
/// `0x60aee0` stores the literal 1.2 (`0x3f99999a`) into `[unit+0xcf0]` without running the
/// formula (`0x60af4f..0x60af67`). An `InvisibleStalker` body's sequence boxes are all zero, so
/// this is its ring; the ring also takes it for a unit with no model.
pub const DEGENERATE_RING_FOOTPRINT: f32 = 1.2;

/// The sequence animation id `anim_id` resolves to through the lookup. An id is not a record
/// number: a chicken's record 0 is a flap and its Stand is index 2. `None` for an absent id
/// (`0xffff`) or one out of range, the presence test folded into the box query.
fn sequence(model: &M2Model, anim_id: u16) -> Option<&M2Sequence> {
    model.sequences.get(model.sequence_slot(anim_id)?)
}

/// The Stand sequence: animation id 0 through the lookup, else record 0; `None` only when the
/// model has no sequences.
fn stand(model: &M2Model) -> Option<&M2Sequence> {
    sequence(model, 0).or(model.sequences.first())
}

/// The Stand box's `(dx, dy, dz)`, the input the reference's living-unit ring is sized from;
/// `None` without sequences. Z rides along because the chat bubble anchors on the same box, so the
/// ring and the bubble never read different animations.
fn stand_box(model: &M2Model) -> Option<(f32, f32, f32)> {
    let s = stand(model)?;
    // The ring squares dx and dy, so an unsorted box is harmless.
    let d = |k: usize| s.bounds_max[k] - s.bounds_min[k];
    Some((d(0), d(1), d(2)))
}

/// `AnimationData` id 42, Swim: the second box `0x50ca90` queries (`0x50ccde call 0x711a20`).
const SWIM_ANIM_ID: u16 = 42;

/// The camera's swim pivot drop ([`M2Bounds::swim_pivot_drop`]). The reference requires ids 0 and
/// `0x2a` both present (`0x711960` at `0x50cc43`/`0x50cc4f`); here Stand resolves through
/// [`stand`], the box the ring and the chat bubble read.
fn swim_pivot_drop(model: &M2Model) -> f32 {
    let (Some(stand), Some(swim)) = (stand(model), sequence(model, SWIM_ANIM_ID)) else {
        return 0.0;
    };
    (stand.bounds_max[2] - swim.bounds_max[2]).max(0.0)
}

/// A parsed M2's bounds, for the Bevy `AssetLoader`.
pub fn parse_m2_bounds(model: &M2Model) -> M2Bounds {
    let h = &model.header;
    let vert_max_from_origin = model
        .vertices
        .iter()
        .map(|v| {
            let p = v.position;
            (p.x * p.x + p.y * p.y + p.z * p.z).sqrt()
        })
        .fold(0.0_f32, f32::max);
    // The Stand box, or the header box for a model with no sequences (the reference's static path).
    let (rx, ry, rz) = stand_box(model).unwrap_or((
        h.bounding_box_max[0] - h.bounding_box_min[0],
        h.bounding_box_max[1] - h.bounding_box_min[1],
        h.bounding_box_max[2] - h.bounding_box_min[2],
    ));
    let ring_footprint = if rx == 0.0 && ry == 0.0 {
        DEGENERATE_RING_FOOTPRINT
    } else {
        (0.5 * (rx * rx + ry * ry).sqrt()).sqrt()
    };
    M2Bounds {
        sphere_radius: h.bounding_sphere_radius,
        bbox_min: h.bounding_box_min,
        bbox_max: h.bounding_box_max,
        vert_max_from_origin,
        ring_footprint,
        pivot_z: model.pivot_attach_z,
        stand_box_z: rz.max(0.0),
        swim_pivot_drop: swim_pivot_drop(model),
    }
}
