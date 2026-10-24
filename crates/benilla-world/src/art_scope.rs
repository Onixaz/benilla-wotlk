//! Within-map art residency: the world-art dedup caches drop what was last wanted more than a
//! radius from the view focus; a cross-map transition still clears them outright.
//!
//! A use (a [`SpatialCache::fetch`] hit or an insert) clears an entry's stamp to `None` and each
//! sweep stamps every `None` with the current focus, so no call site needs the camera and a cache
//! nothing sweeps never expires. Eviction only drops the dedup: a drawn entity keeps its handles.

use std::hash::Hash;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::Real;

use benilla_assets::SpatialCache;
use benilla_formats::TILE_SIZE;

/// Real-clock seconds between sweeps (housekeeping, not world time); also the stamp's resolution.
const SWEEP_SECS: f32 = 1.0;

/// Five ADT tiles (2667 yd), the smallest round tile multiple clear of the residency window at the
/// default view distance; a wider live `farclip` raises the sweep with it ([`live_floor`]).
const DEFAULT_RADIUS_YD: f32 = 5.0 * TILE_SIZE;

/// The far corner of the tile block the residency window keeps at `farclip`, plus a tile: a stamp
/// is where the viewer stood, so no smaller radius is safe.
fn live_floor(farclip: f32) -> f32 {
    crate::terrain_stream::window::max_resident_reach_yd(farclip)
}

/// [`live_floor`] at the widest view distance any map admits.
fn radius_floor() -> f32 {
    live_floor(crate::view::validate_farclip(f32::MAX, u32::MAX))
}

/// `$WOW_ART_RADIUS` in yards (`0` or less turns eviction off); a value under [`radius_floor`] is
/// raised to it with a warning.
fn radius_from_env() -> f32 {
    let floor = radius_floor();
    match std::env::var("WOW_ART_RADIUS")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
    {
        Some(r) if r <= 0.0 => 0.0,
        Some(r) if r < floor => {
            warn!(
                "art-scope: WOW_ART_RADIUS={r} is inside the streamer's own reach ({floor:.0} yd \
                 at the widest view distance) — using {floor:.0}"
            );
            floor
        }
        Some(r) => r,
        None => DEFAULT_RADIUS_YD,
    }
}

/// Which cache a census row is about, in journal-column order: the CSV only grows columns at the
/// end, so this order is part of the file format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArtSlot {
    /// `Placements::materials`: streamed doodad and WMO submesh materials.
    PlaceMats,
    /// `ModelMaterials`: every authored-batch material, units, GameObjects and booth scenes alike.
    ModelMats,
    /// `SkinComposites`: composited character body atlases.
    Skins,
    /// `WorldAssets::model_materials`: the ground-clutter material dedup.
    ClutterMats,
    /// `WorldAssets::textures`: decoded world BLPs by (path, wrap).
    Textures,
    /// `ClutterGeometry`: CPU submesh copies of every decoded detail M2.
    ClutterGeo,
}

impl ArtSlot {
    pub const ALL: [ArtSlot; 6] = [
        ArtSlot::PlaceMats,
        ArtSlot::ModelMats,
        ArtSlot::Skins,
        ArtSlot::ClutterMats,
        ArtSlot::Textures,
        ArtSlot::ClutterGeo,
    ];

    pub(crate) fn column(self) -> &'static str {
        match self {
            ArtSlot::PlaceMats => "pmat",
            ArtSlot::ModelMats => "emat",
            ArtSlot::Skins => "skin",
            ArtSlot::ClutterMats => "cmat",
            ArtSlot::Textures => "tex",
            ArtSlot::ClutterGeo => "cgeo",
        }
    }

    fn idx(self) -> usize {
        match self {
            ArtSlot::PlaceMats => 0,
            ArtSlot::ModelMats => 1,
            ArtSlot::Skins => 2,
            ArtSlot::ClutterMats => 3,
            ArtSlot::Textures => 4,
            ArtSlot::ClutterGeo => 5,
        }
    }
}

/// Per-cache live and dropped counts, refreshed by every [`ArtScope::apply`], so one journal row
/// shows which cache is growing.
#[derive(Resource, Default)]
pub struct ArtCensus {
    live: [usize; ArtSlot::ALL.len()],
    dropped: [usize; ArtSlot::ALL.len()],
}

impl ArtCensus {
    /// Live entries in one cache as of its last [`ArtScope::apply`].
    pub fn live(&self, slot: ArtSlot) -> usize {
        self.live[slot.idx()]
    }

    /// Entries dropped by distance across every cache since the run began.
    pub fn dropped_total(&self) -> usize {
        self.dropped.iter().sum()
    }
}

/// The sweep's shared state: the radius, this tick's focus, and whether this frame is a sweep frame.
#[derive(Resource, Default)]
pub struct ArtScopeState {
    /// Eviction radius in yards, `0` for none; zero until [`configure_art_scope`] runs.
    radius: f32,
    /// The residency window's reach at the live `farclip`, which the sweep never cuts inside.
    floor: f32,
    /// The view focus in WoW coords; `None` (no avatar, no camera) suspends stamping and sweeping.
    focus: Option<[f32; 3]>,
    /// Set on the one frame per [`SWEEP_SECS`] that sweeps.
    due: bool,
    last_sweep: f32,
    /// Real seconds since app start this frame; the cache's dwell floor is measured on it.
    now: f32,
}

impl ArtScopeState {
    /// This frame's view focus in WoW coords, which the sweep measures from. The FPS journal logs
    /// it because its avatar columns stand still through a detached free-fly.
    pub fn focus(&self) -> Option<[f32; 3]> {
        self.focus
    }
}

/// What an owning module needs to scope its caches: the policy, plus the census to report into.
#[derive(SystemParam)]
pub struct ArtScope<'w> {
    state: Res<'w, ArtScopeState>,
    census: ResMut<'w, ArtCensus>,
}

impl ArtScope<'_> {
    /// Sweeps one cache on sweep frames and records its residency.
    pub fn apply<K: Eq + Hash, V>(&mut self, cache: &mut SpatialCache<K, V>, slot: ArtSlot) {
        let dropped = match self.state.focus {
            Some(focus) if self.state.due && self.state.radius > 0.0 => cache.scope(
                focus,
                self.state.radius.max(self.state.floor),
                self.state.now,
            ),
            _ => 0,
        };
        let i = slot.idx();
        self.census.live[i] = cache.len();
        self.census.dropped[i] += dropped;
        if dropped > 0 {
            debug!(
                "art-scope: dropped {dropped} {} entries beyond {:.0} yd ({} live)",
                slot.column(),
                self.state.radius,
                cache.len()
            );
        }
    }
}

fn configure_art_scope(mut state: ResMut<ArtScopeState>) {
    state.radius = radius_from_env();
    if state.radius > 0.0 {
        info!(
            "art-scope: within-map art evicts beyond {:.0} yd of the view focus",
            state.radius
        );
    } else {
        warn!("art-scope: eviction OFF ($WOW_ART_RADIUS=0) — within-map residency is unbounded");
    }
}

/// Publish this frame's focus and whether it sweeps. Unordered against every consumer: a stamp is
/// at most one sweep stale, and reading `due` a frame early only moves the sweep a frame.
fn track_art_scope(
    mut state: ResMut<ArtScopeState>,
    view: Option<Res<crate::view::ViewDistance>>,
    time: Res<Time<Real>>,
    focus: Res<crate::terrain_stream::ViewFocus>,
    camera: Query<&Transform, With<crate::view::WorldCamera>>,
) {
    let cam = camera.single().ok().map(|c| c.translation);
    state.focus = (focus.body_pos().is_some() || cam.is_some()).then(|| focus.resolve(cam));
    let now = time.elapsed_secs();
    state.now = now;
    state.due = now - state.last_sweep >= SWEEP_SECS;
    if state.due {
        state.last_sweep = now;
        state.floor = view.map_or(0.0, |v| live_floor(v.farclip));
    }
}

/// Owns the sweep policy and the census; each cache's owner calls [`ArtScope::apply`].
pub(crate) struct ArtScopePlugin;

impl Plugin for ArtScopePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArtCensus>()
            .init_resource::<ArtScopeState>()
            .add_systems(Startup, configure_art_scope)
            .add_systems(Update, track_art_scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_radius_clears_the_streamers_own_reach() {
        let at_default = live_floor(crate::view::FARCLIP_DEFAULT);
        assert!(
            DEFAULT_RADIUS_YD > at_default,
            "default {DEFAULT_RADIUS_YD} must clear the floor {at_default}"
        );
        // (26 + 1 + 16) chunks each way at farclip 777, to the corner, plus a tile: ~2560 yd.
        assert!((at_default - 2560.0).abs() < 1.0, "{at_default}");
        // (50 + 1 + 16) at the 1583 yd cap of the newer continents: ~3692 yd.
        assert!((radius_floor() - 3692.0).abs() < 1.0, "{}", radius_floor());
    }

    /// `$WOW_ART_RADIUS` is unset in the test binary, as in every ordinary run.
    #[test]
    fn an_unconfigured_run_still_bounds_itself() {
        assert_eq!(radius_from_env(), DEFAULT_RADIUS_YD);
    }
}
