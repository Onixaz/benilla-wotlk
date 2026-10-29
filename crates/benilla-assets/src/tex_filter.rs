//! The process texture policy: `textureFilteringMode` resolved into a filter and an anisotropy,
//! and `baseMip`, the first mip a world texture samples.
//!
//! The engine-init callback (`0x4047e0`) reads `textureFilteringMode` once and hands the engine the
//! mode's filter (`0xab6128`: LinearMipNearest, LinearMipLinear, then Anisotropic four times) and
//! the mode's anisotropy (`0xab6140`: 1, 1, 2, 4, 8, 16), so the stock slider's six stops are
//! bilinear, trilinear and anisotropic 2x to 16x, and its tooltip says "enabled upon restart". The
//! world's terrain, WMO, M2 and liquid textures take the policy; the interface's keep their own
//! filter.
//!
//! A process global because a sampler is baked into the [`Image`] at load, by an async loader
//! ([`crate::blp`]) and by systems ([`crate::world_assets`]) that share no resource. A change
//! applies live ([`TexPolicyPlugin`]): every image built by [`policy_sampler`] takes the new
//! sampler in place and the world's materials rebuild their bind groups, where the reference
//! reloads its textures (its device-restart texture notice, from the `baseMip` callback
//! `0x78df00`).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::FilterMode;
use bevy::render::renderer::RenderDevice;
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, RenderApp};

/// `textureFilteringMode`'s range, its change callback's (`0x4023c0`: "Texture filtering mode must
/// be in range 0 to 6.", which refuses 6).
pub const TEXTURE_FILTERING_RANGE: std::ops::RangeInclusive<u8> = 0..=5;

/// The anisotropy per mode, the reference's table at `0xab6140`.
const MAX_ANISOTROPY: [u16; 6] = [1, 1, 2, 4, 8, 16];

/// The highest anisotropy a mode asks for, which `GetVideoCaps` reports as the device's.
pub const MAX_ANISOTROPY_CAP: u32 = 16;

/// The policy every world sampler takes.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct TexFilterSetting {
    /// `textureFilteringMode`, 0 to 5.
    pub mode: u8,
    /// `baseMip`, 0 or 1: a 1 samples from the second mip, half the texture's size, as the
    /// reference's device skips the first level of every texture (`TextureCalcGxTexSize
    /// 0x4b6300`).
    pub base_mip: u8,
}

impl Default for TexFilterSetting {
    /// A fresh install: trilinear, every mip. `$WOW_TEXTURE_FILTERING` (0 to 5) overrides the mode
    /// for the session only, like `$WOW_MSAA`, so an A/B never sticks in `config.toml`.
    fn default() -> Self {
        Self {
            mode: std::env::var("WOW_TEXTURE_FILTERING")
                .ok()
                .and_then(|v| v.parse::<u8>().ok())
                .filter(|m| TEXTURE_FILTERING_RANGE.contains(m))
                .unwrap_or(1),
            base_mip: 0,
        }
    }
}

impl TexFilterSetting {
    /// Mode 0 selects the nearer mip; every other blends the two.
    pub fn mipmap_filter(self) -> ImageFilterMode {
        match self.mode {
            0 => ImageFilterMode::Nearest,
            _ => ImageFilterMode::Linear,
        }
    }

    /// The same, for a lane that builds a raw wgpu `SamplerDescriptor`.
    pub fn gpu_mipmap_filter(self) -> FilterMode {
        match self.mode {
            0 => FilterMode::Nearest,
            _ => FilterMode::Linear,
        }
    }

    /// The max anisotropy of the mode. wgpu takes a clamp above 1 only with every filter
    /// `Linear`, which modes 2 to 5 always have.
    pub fn anisotropy_clamp(self) -> u16 {
        MAX_ANISOTROPY[usize::from(self.mode.min(5))]
    }

    /// The lowest mip level the sampler may read, `baseMip`.
    pub fn lod_min_clamp(self) -> f32 {
        f32::from(self.base_mip.min(1))
    }
}

static POLICY: RwLock<Option<TexFilterSetting>> = RwLock::new(None);

/// Bumped on each publish that moves the policy.
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Publish the resolved policy: first from the CVar load (`benilla_app::cvars::load_config`, the
/// `CvarLoad` set), before any system can request a texture, then on each change.
pub fn publish_tex_filter(setting: TexFilterSetting) {
    let mut policy = POLICY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *policy != Some(setting) {
        let first = policy.is_none();
        *policy = Some(setting);
        if !first {
            GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The policy every sampler takes; unpublished (the world viewer, a headless test), it is
/// [`TexFilterSetting::default`], a fresh install's value.
pub fn tex_filter() -> TexFilterSetting {
    let policy = *POLICY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    policy.unwrap_or_default()
}

/// How many times the published policy has moved since the first publish.
pub fn tex_filter_generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

/// The label a policy sampler carries, by which [`TexPolicyPlugin`] finds the images to re-sample.
pub const POLICY_SAMPLER_LABEL: &str = "benilla:world-texture-policy";

/// A world texture's sampler: linear magnify and minify, the given address modes, and the live
/// policy's mip filter, anisotropy and base mip.
pub fn policy_sampler(u: ImageAddressMode, v: ImageAddressMode) -> ImageSamplerDescriptor {
    let filter = tex_filter();
    ImageSamplerDescriptor {
        label: Some(POLICY_SAMPLER_LABEL.to_string()),
        address_mode_u: u,
        address_mode_v: v,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: filter.mipmap_filter(),
        anisotropy_clamp: filter.anisotropy_clamp(),
        lod_min_clamp: filter.lod_min_clamp(),
        ..Default::default()
    }
}

/// Carries a policy change to the images already uploaded: their samplers are rebuilt in the
/// render world at the next extract, and the world's materials are marked changed so their bind
/// groups take the new samplers.
pub struct TexPolicyPlugin;

impl Plugin for TexPolicyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, refresh_policy_materials);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(ExtractSchedule, swap_policy_samplers);
        }
    }
}

/// Re-prepare every world material once per policy move.
fn refresh_policy_materials(
    mut seen: Local<Option<u32>>,
    models: Option<ResMut<Assets<crate::materials::WowModelMaterial>>>,
    terrain: Option<ResMut<Assets<crate::materials::TerrainMaterial>>>,
    liquid: Option<ResMut<Assets<crate::materials::LiquidMaterial>>>,
) {
    let generation = tex_filter_generation();
    if seen.replace(generation).is_none_or(|g| g == generation) {
        return;
    }
    let mut n = 0usize;
    if let Some(mut m) = models {
        n += m.iter_mut().count();
    }
    if let Some(mut m) = terrain {
        n += m.iter_mut().count();
    }
    if let Some(mut m) = liquid {
        n += m.iter_mut().count();
    }
    info!("texture filter: policy moved; {n} world material(s) rebinding");
}

/// Rebuild the render-world sampler of every image a [`policy_sampler`] built, once per policy
/// move, keeping each image's own address modes.
fn swap_policy_samplers(
    images: Extract<Res<Assets<Image>>>,
    mut gpu: ResMut<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    mut seen: Local<Option<u32>>,
) {
    let generation = tex_filter_generation();
    if seen.replace(generation).is_none_or(|g| g == generation) {
        return;
    }
    let mut n = 0usize;
    for (id, image) in images.iter() {
        let ImageSampler::Descriptor(old) = &image.sampler else {
            continue;
        };
        if old.label.as_deref() != Some(POLICY_SAMPLER_LABEL) {
            continue;
        }
        let Some(gpu_image) = gpu.get_mut(id) else {
            continue;
        };
        let fresh = ImageSamplerDescriptor {
            address_mode_w: old.address_mode_w,
            ..policy_sampler(old.address_mode_u, old.address_mode_v)
        };
        gpu_image.sampler = device.create_sampler(&fresh.as_wgpu());
        n += 1;
    }
    info!("texture filter: {n} world texture sampler(s) rebuilt");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registered "1", trilinear with no anisotropy; `Default` reads the env, so a set lever
    /// skips the test.
    #[test]
    fn what_ships_is_trilinear_with_anisotropy_off() {
        if std::env::var_os("WOW_TEXTURE_FILTERING").is_some() {
            return;
        }
        let shipped = TexFilterSetting::default();
        assert_eq!(shipped.mode, 1);
        assert_eq!(shipped.mipmap_filter(), ImageFilterMode::Linear);
        assert_eq!(shipped.anisotropy_clamp(), 1);
        assert_eq!(shipped.lod_min_clamp(), 0.0);
    }

    /// The six stops, from the two tables at `0xab6128` and `0xab6140`.
    #[test]
    fn each_mode_is_the_references_filter_and_anisotropy() {
        for (mode, mip, aniso) in [
            (0, FilterMode::Nearest, 1),
            (1, FilterMode::Linear, 1),
            (2, FilterMode::Linear, 2),
            (3, FilterMode::Linear, 4),
            (4, FilterMode::Linear, 8),
            (5, FilterMode::Linear, 16),
        ] {
            let s = TexFilterSetting { mode, base_mip: 0 };
            assert_eq!(s.gpu_mipmap_filter(), mip, "mode {mode}");
            assert_eq!(s.anisotropy_clamp(), aniso, "mode {mode}");
        }
    }

    /// wgpu refuses an anisotropy clamp above 1 with a nearest mip filter.
    #[test]
    fn a_clamp_above_one_only_ever_rides_all_linear_filters() {
        for mode in TEXTURE_FILTERING_RANGE {
            let s = TexFilterSetting { mode, base_mip: 0 };
            if s.anisotropy_clamp() > 1 {
                assert_eq!(s.gpu_mipmap_filter(), FilterMode::Linear);
            }
        }
    }

    #[test]
    fn base_mip_one_skips_the_first_level() {
        let s = TexFilterSetting {
            mode: 1,
            base_mip: 1,
        };
        assert_eq!(s.lod_min_clamp(), 1.0);
    }
}
