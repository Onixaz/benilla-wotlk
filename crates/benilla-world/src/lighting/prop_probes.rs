//! The interior-prop light-probe table: one 7-row SH probe per lit interior MODD prop, folded at
//! spawn ([`super::sh::prop_probe_coeffs`] over the MODD colour, the fixed-axis lobe and the
//! group's MOLR lights) and uploaded behind the shared light blob. The prop's `MeshTag` payload
//! carries its slot, which the [`PropProbeSlot`] hook frees on despawn.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::math::Vec4;
use bevy::prelude::*;
use bevy::render::extract_resource::ExtractResource;

/// Capacity of the probe table, mirrored by `wow_model.wgsl`'s `prop_probes` array (keep in sync):
/// a city WMO streams in thousands of lit interior props at once.
pub const MAX_PROP_PROBES: usize = 8192;

/// One probe's seven SH rows as the shader reads them.
type Row = [[f32; 4]; 7];

/// A probe's rows, bit for bit: identical props fold to identical bits and share a slot.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ProbeKey([[u32; 4]; 7]);

impl ProbeKey {
    fn of(coeffs: &[Vec4; 7]) -> Self {
        Self(coeffs.map(|v| v.to_array().map(f32::to_bits)))
    }
}

/// The main-world slot table: a slab whose freed slots recycle first, with identical probes sharing
/// a refcounted slot (without it a login scene's prop WMOs overflow the table). Each frame publishes
/// only the rows written since the last publish, so a moving unit's probe costs one row, not a
/// copy of the table.
#[derive(Resource)]
pub struct PropProbes {
    rows: Vec<Row>,
    free: Vec<u16>,
    high: usize,
    /// Probe bits to live slot, for as long as the slot's refcount lasts.
    by_key: HashMap<ProbeKey, u16>,
    /// Per slot `(refcount, key)`, `None` when free; a `None` key marks an owned, unshared slot.
    slots: Vec<Option<(u32, Option<ProbeKey>)>>,
    /// Bumped on every write; the render-world upload watches it.
    generation: u64,
    /// Peak distinct occupancy, logged by the spawner on overflow.
    peak: usize,
    /// Slots written since the last publish, possibly repeated.
    dirty: Vec<u16>,
}

impl Default for PropProbes {
    fn default() -> Self {
        Self {
            rows: vec![[[0.0; 4]; 7]; MAX_PROP_PROBES],
            free: Vec::new(),
            high: 0,
            by_key: HashMap::new(),
            slots: vec![None; MAX_PROP_PROBES],
            generation: 0,
            peak: 0,
            dirty: Vec::new(),
        }
    }
}

impl PropProbes {
    /// Claims a slot, sharing a live identical probe's; `None` when the table is full of distinct
    /// probes, and the caller then lights the prop as exterior.
    pub fn alloc(&mut self, coeffs: [Vec4; 7]) -> Option<u16> {
        let key = ProbeKey::of(&coeffs);
        if let Some(&slot) = self.by_key.get(&key) {
            if let Some(Some((refs, _))) = self.slots.get_mut(slot as usize) {
                *refs += 1;
                return Some(slot);
            }
        }
        let slot = self.take_free_slot()?;
        self.rows[slot as usize] = coeffs.map(|v| v.to_array());
        self.by_key.insert(key.clone(), slot);
        self.slots[slot as usize] = Some((1, Some(key)));
        self.touch(slot);
        self.peak = self.peak.max(self.high - self.free.len());
        Some(slot)
    }

    /// Claims an owned slot for a moving entity's probe, never shared, since two units in one room
    /// ramp independently; the holder rewrites it with [`Self::update_owned`].
    pub(crate) fn alloc_owned(&mut self, coeffs: [Vec4; 7]) -> Option<u16> {
        let slot = self.take_free_slot()?;
        self.rows[slot as usize] = coeffs.map(|v| v.to_array());
        self.slots[slot as usize] = Some((1, None));
        self.touch(slot);
        self.peak = self.peak.max(self.high - self.free.len());
        Some(slot)
    }

    /// Rewrites an owned slot in place. A shared or free slot is refused with a warning: a holder
    /// pointing at one is stale, and a silent no-op would leave it black.
    pub(crate) fn update_owned(&mut self, slot: u16, coeffs: [Vec4; 7]) {
        if !matches!(self.slots.get(slot as usize), Some(Some((_, None)))) {
            warn_once!("update_owned({slot}) on a non-owned slot — the holder's slot is stale");
            return;
        }
        self.rows[slot as usize] = coeffs.map(|v| v.to_array());
        self.touch(slot);
    }

    fn touch(&mut self, slot: u16) {
        self.dirty.push(slot);
        self.generation += 1;
    }

    fn take_free_slot(&mut self) -> Option<u16> {
        match self.free.pop() {
            Some(s) => Some(s),
            None if self.high < MAX_PROP_PROBES => {
                self.high += 1;
                Some((self.high - 1) as u16)
            }
            None => None,
        }
    }

    /// Live and peak distinct occupancy.
    pub fn occupancy(&self) -> (usize, usize) {
        (self.high - self.free.len(), self.peak)
    }

    /// Drops one reference, freeing at zero; besides the [`PropProbeSlot`] hook, the interior
    /// classifier calls it for a slot whose anchor was despawned before its component landed.
    pub(crate) fn release(&mut self, slot: u16) {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return;
        };
        let Some((refs, _)) = entry else {
            return;
        };
        *refs -= 1;
        if *refs > 0 {
            return;
        }
        let Some((_, key)) = entry.take() else {
            return;
        };
        if let Some(key) = key {
            self.by_key.remove(&key);
        }
        // Zeroed, so a stale `MeshTag` in a frame of despawn skew reads black.
        self.rows[slot as usize] = [[0.0; 4]; 7];
        self.touch(slot);
        self.free.push(slot);
    }
}

/// One publish of the probe table to the render world: the rows written since the last one, in
/// slot order, numbered so the render side applies each publish once.
#[derive(Resource, Clone, Default, ExtractResource)]
pub(crate) struct PropProbeExtract {
    changes: Arc<Vec<(u16, Row)>>,
    seq: u64,
}

/// Main world, after the spawners: publishes the rows written this frame, if any.
pub(super) fn publish_prop_probes(
    mut probes: ResMut<PropProbes>,
    mut out: ResMut<PropProbeExtract>,
) {
    if probes.dirty.is_empty() {
        return;
    }
    let probes = &mut *probes;
    probes.dirty.sort_unstable();
    probes.dirty.dedup();
    let changes = probes
        .dirty
        .drain(..)
        .map(|slot| (slot, probes.rows[slot as usize]))
        .collect();
    out.changes = Arc::new(changes);
    out.seq = out.seq.wrapping_add(1);
}

/// Byte offset of the probe region in the shared light buffer, right after the per-frame blob.
pub fn prop_probe_region_offset() -> u64 {
    super::global_light::per_frame_blob_bytes()
}

/// The render world's copy of the table: every publish applied, and the slots not yet on the GPU.
#[derive(Default)]
pub(super) struct ProbeMirror {
    rows: Vec<Row>,
    /// One past the highest slot ever written.
    high: usize,
    seq: u64,
    /// Written since the last upload, possibly repeated.
    pending: Vec<u16>,
    /// The region has had one whole upload; until then the next upload sends all of `rows`.
    uploaded: bool,
}

impl ProbeMirror {
    /// Applies a publish once; a repeat of the last one is ignored.
    fn apply(&mut self, data: &PropProbeExtract) {
        if data.seq == self.seq {
            return;
        }
        self.seq = data.seq;
        if self.rows.is_empty() {
            self.rows = vec![[[0.0; 4]; 7]; MAX_PROP_PROBES];
        }
        for &(slot, row) in data.changes.iter() {
            self.rows[slot as usize] = row;
            self.high = self.high.max(slot as usize + 1);
            self.pending.push(slot);
        }
    }

    /// The `[lo, hi)` slot runs the next upload writes: everything written so far on the first,
    /// then each run of adjacent pending slots.
    fn take_runs(&mut self) -> Vec<(usize, usize)> {
        if !self.uploaded {
            self.uploaded = true;
            self.pending.clear();
            return if self.high > 0 {
                vec![(0, self.high)]
            } else {
                Vec::new()
            };
        }
        self.pending.sort_unstable();
        self.pending.dedup();
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for slot in self.pending.drain(..).map(usize::from) {
            match runs.last_mut() {
                Some((_, hi)) if *hi == slot => *hi += 1,
                _ => runs.push((slot, slot + 1)),
            }
        }
        runs
    }
}

/// Render world, in `PrepareResources`: folds this frame's publish into the mirror and writes the
/// changed rows; the per-frame upload writes only the prefix, so the region persists.
pub(super) fn upload_prop_probes(
    queue: Res<bevy::render::renderer::RenderQueue>,
    buffer: Option<Res<super::SharedLightBuffer>>,
    data: Option<Res<PropProbeExtract>>,
    mut mirror: Local<ProbeMirror>,
) {
    if let Some(data) = data {
        mirror.apply(&data);
    }
    // Without a buffer yet the rows wait in the mirror; the first upload sends them all.
    let Some(buffer) = buffer else {
        return;
    };
    for (lo, hi) in mirror.take_runs() {
        queue.write_buffer(
            &buffer.0,
            prop_probe_region_offset() + (lo * std::mem::size_of::<Row>()) as u64,
            bytemuck::cast_slice(&mirror.rows[lo..hi]),
        );
    }
}

/// On one entity per lit interior prop; its hook returns the slot to the table. `on_replace`, not
/// `on_remove`: an insert over a live slot must free the old one, and `on_remove` never fires on
/// an overwrite.
#[derive(Component)]
#[component(on_replace = free_prop_probe_slot)]
pub struct PropProbeSlot(pub u16);

fn free_prop_probe_slot(mut world: DeferredWorld, ctx: HookContext) {
    let slot = world
        .get::<PropProbeSlot>(ctx.entity)
        .map(|s| s.0)
        .expect("on_replace runs with the outgoing component still present");
    world.resource_mut::<PropProbes>().release(slot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_probes_share_one_slot_and_refcount() {
        let mut t = PropProbes::default();
        let c = [Vec4::splat(0.5); 7];
        let a = t.alloc(c).unwrap();
        let b = t.alloc(c).unwrap();
        assert_eq!(a, b); // identical content shares one slot
        let other = t.alloc([Vec4::splat(0.25); 7]).unwrap();
        assert_ne!(a, other);
        // The first release keeps the shared slot alive; the second frees it.
        t.release(a);
        assert_ne!(t.rows[a as usize], [[0.0; 4]; 7]);
        t.release(b);
        assert_eq!(t.rows[a as usize], [[0.0; 4]; 7]); // freed reads black, not stale light
        let c2 = t.alloc(c).unwrap(); // the freed slot recycles
        assert_eq!(c2, a);
        assert_eq!(t.high, 2);
    }

    #[test]
    fn overwriting_a_probe_slot_frees_the_old_one() {
        let mut w = World::new();
        let mut t = PropProbes::default();
        let a = t.alloc([Vec4::splat(0.5); 7]).unwrap();
        let b = t.alloc([Vec4::splat(0.25); 7]).unwrap();
        w.insert_resource(t);
        let e = w.spawn(PropProbeSlot(a)).id();
        w.entity_mut(e).insert(PropProbeSlot(b));
        let t = w.resource::<PropProbes>();
        assert_eq!(
            t.rows[a as usize], [[0.0; 4]; 7],
            "the overwritten slot is freed"
        );
        assert_ne!(t.rows[b as usize], [[0.0; 4]; 7], "the new slot is live");
        w.entity_mut(e).despawn();
        let t = w.resource::<PropProbes>();
        assert_eq!(t.rows[b as usize], [[0.0; 4]; 7], "despawn still frees");
    }

    #[test]
    fn owned_slots_never_dedup_and_update_in_place() {
        let mut t = PropProbes::default();
        let c = [Vec4::splat(0.5); 7];
        let a = t.alloc_owned(c).unwrap();
        let b = t.alloc_owned(c).unwrap();
        assert_ne!(a, b); // identical content still gets its own slot
        let g0 = t.generation;
        t.update_owned(a, [Vec4::splat(0.7); 7]);
        assert_eq!(t.rows[a as usize], [[0.7; 4]; 7]);
        assert!(t.generation > g0, "in-place update must republish");
        // A shared slot ignores `update_owned`.
        let d = t.alloc(c).unwrap();
        let before = t.rows[d as usize];
        t.update_owned(d, [Vec4::splat(0.9); 7]);
        assert_eq!(t.rows[d as usize], before);
        // An owned slot frees through the same release, zeroes, and recycles.
        t.release(a);
        assert_eq!(t.rows[a as usize], [[0.0; 4]; 7]);
        assert_eq!(t.alloc_owned(c).unwrap(), a);
    }

    #[test]
    fn alloc_fails_gracefully_at_capacity() {
        let mut t = PropProbes::default();
        for i in 0..MAX_PROP_PROBES {
            let unique = [Vec4::splat(i as f32 + 1.0); 7];
            assert!(t.alloc(unique).is_some());
        }
        assert!(t.alloc([Vec4::splat(-1.0); 7]).is_none());
        // A duplicate of a live probe still succeeds at capacity, costing no slot.
        assert!(t.alloc([Vec4::splat(1.0); 7]).is_some());
    }

    /// A publish carries each written row once, in slot order, and the render mirror ends equal
    /// to the main table: whole on its first upload, then only the runs written since.
    #[test]
    fn publishes_carry_only_written_rows_and_the_mirror_follows() {
        let mut w = World::new();
        w.insert_resource(PropProbes::default());
        w.insert_resource(PropProbeExtract::default());
        let publish = |w: &mut World| {
            w.run_system_cached(publish_prop_probes).unwrap();
            w.resource::<PropProbeExtract>().clone()
        };
        let mut mirror = ProbeMirror::default();
        let (a, b, c) = {
            let mut t = w.resource_mut::<PropProbes>();
            let a = t.alloc_owned([Vec4::splat(0.1); 7]).unwrap();
            let b = t.alloc_owned([Vec4::splat(0.2); 7]).unwrap();
            let c = t.alloc_owned([Vec4::splat(0.3); 7]).unwrap();
            t.update_owned(a, [Vec4::splat(0.4); 7]);
            (a, b, c)
        };
        let first = publish(&mut w);
        let slots: Vec<u16> = first.changes.iter().map(|(s, _)| *s).collect();
        assert_eq!(slots, vec![a, b, c], "each written slot once, in order");
        mirror.apply(&first);
        assert_eq!(
            mirror.take_runs(),
            vec![(0, 3)],
            "the first upload sends everything"
        );

        // A frame with no write publishes nothing; a repeat extract is ignored.
        let idle = publish(&mut w);
        assert_eq!(idle.seq, first.seq);
        mirror.apply(&idle);
        assert!(mirror.take_runs().is_empty());

        w.resource_mut::<PropProbes>()
            .update_owned(c, [Vec4::splat(0.9); 7]);
        w.resource_mut::<PropProbes>().release(a);
        let next = publish(&mut w);
        assert_eq!(next.changes.len(), 2, "only the two written rows travel");
        mirror.apply(&next);
        assert_eq!(
            mirror.take_runs(),
            vec![(0, 1), (2, 3)],
            "adjacent slots merge into runs"
        );
        let t = w.resource::<PropProbes>();
        assert_eq!(
            &mirror.rows[..3],
            &t.rows[..3],
            "the mirror matches the table"
        );
    }
}
