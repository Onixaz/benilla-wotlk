// The exterior shadow-map receive (`extShadowQuality` 1 to 5), shared by terrain.wgsl and
// wow_model.wgsl. The maps are the one directional light's cascades (`benilla_world::ext_shadow`):
// cascade 0 the 20 yd dynamic map, cascades 1-3 the 40/160/640 yd bands. With no such light (mode
// 0) every call answers lit and samples nothing.
//
// Modes 1 to 5 load three program groups, 1, 1, 2, 2, 3 (`0xb1d554`, `0x79e5c0`): the arms below.
#define_import_path benilla_assets::wow_shadow

#import bevy_pbr::mesh_view_bindings::{
    lights,
    view,
    directional_shadow_textures,
    directional_shadow_textures_comparison_sampler,
}

// The eight PCF offsets of `0x8742e0` in texels; five taps read the centre and the even ones.
const PCF_TAPS = array<vec2<f32>, 8>(
    vec2<f32>(0.8, -1.0),
    vec2<f32>(-0.2, -0.8),
    vec2<f32>(0.2, -0.6),
    vec2<f32>(1.0, -0.4),
    vec2<f32>(-0.6, -0.2),
    vec2<f32>(0.6, 0.2),
    vec2<f32>(-1.0, -0.4),
    vec2<f32>(-0.4, -0.6),
);

// The stride through `PCF_TAPS` of a five-tap and of a nine-tap read.
const FIVE_TAPS: u32 = 2u;
const NINE_TAPS: u32 = 1u;

// The receiving depth bias per cascade in yards, `0x7bac10` under `hwPCF`: 0.5 + 0.2·{0.5 for the
// dynamic map, 2, 4, 8 for the bands}.
const CASCADE_BIAS = array<f32, 4>(0.6, 0.9, 1.3, 2.1);

// The maps' depth range, [1, 4000] yd (`0x7bac10`), in Bevy's reverse-Z clip depth.
const DEPTH_SPAN: f32 = 3999.0;

// The view depth inside which a model program reads the dynamic map with nine taps.
const NEAR_TAPS_DEPTH: f32 = 10.0;

// Whether a shadow-map mode is live: the light's shadows are on.
fn shadow_maps_on() -> bool {
    return lights.n_directional_lights > 0u
        && (lights.directional_lights[0].flags & 1u) != 0u;
}

// Whether the mode draws the bands, past mode 2.
fn has_bands() -> bool {
    return lights.directional_lights[0].num_cascades >= 4u;
}

// Cascade `i`'s box coordinate of a world point: xy in [-1, 1] across the box, z its biased
// reverse-Z depth.
fn map_coord(world: vec3<f32>, i: u32) -> vec3<f32> {
    let clip = lights.directional_lights[0].cascades[i].clip_from_world * vec4<f32>(world, 1.0);
    let ndc = clip.xyz / clip.w;
    var bias = CASCADE_BIAS;
    return vec3<f32>(ndc.xy, ndc.z + bias[i] / DEPTH_SPAN);
}

// The lit fraction of cascade `i` at box coordinate `c`, hardware-PCF taps at the given stride.
fn map_lit(c: vec3<f32>, i: u32, stride: u32) -> f32 {
    let layer = i32(lights.directional_lights[0].depth_texture_base_index + i);
    let texel = 1.0 / vec2<f32>(textureDimensions(directional_shadow_textures).xy);
    let uv = c.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    var sum = textureSampleCompareLevel(
        directional_shadow_textures,
        directional_shadow_textures_comparison_sampler,
        uv,
        layer,
        c.z,
    );
    var taps = PCF_TAPS;
    for (var k = 0u; k < 8u; k = k + stride) {
        sum += textureSampleCompareLevel(
            directional_shadow_textures,
            directional_shadow_textures_comparison_sampler,
            uv + taps[k] * texel,
            layer,
            c.z,
        );
    }
    return sum / f32(1u + 8u / stride);
}

fn box_extent(c: vec3<f32>) -> f32 {
    return max(abs(c.x), abs(c.y));
}

// The dynamic map's edge fade, 0 from 0.99 of its box out.
fn dynamic_fade(c: vec3<f32>) -> f32 {
    return saturate(3.4137931 - 3.4482758 * box_extent(c));
}

// The dynamic map's term below mode 5, sampled only past the program's fade floor.
fn faded_dynamic(c: vec3<f32>, floor: f32, stride: u32) -> f32 {
    let f = dynamic_fade(c);
    if (f <= floor) {
        return 1.0;
    }
    return 1.0 + f * (map_lit(c, 0u, stride) - 1.0);
}

// The bands' term: the innermost band whose box holds the point, band 2 faded at its edge.
fn band_term(world: vec3<f32>) -> f32 {
    let c0 = map_coord(world, 1u);
    if (box_extent(c0) < 1.0) {
        return map_lit(c0, 1u, FIVE_TAPS);
    }
    let c1 = map_coord(world, 2u);
    if (box_extent(c1) < 1.0) {
        return map_lit(c1, 2u, FIVE_TAPS);
    }
    let c2 = map_coord(world, 3u);
    let g = saturate(11.0 - 11.111111 * box_extent(c2));
    if (g <= 0.0) {
        return 1.0;
    }
    return 1.0 + g * (map_lit(c2, 3u, FIVE_TAPS) - 1.0);
}

// The terrain term: past mode 2 no baked bit; at mode 5 the dynamic map is the first cascade.
fn terrain_term(world: vec3<f32>, baked: f32, cascaded: bool) -> f32 {
    if (!shadow_maps_on()) {
        return baked;
    }
    let c = map_coord(world, 0u);
    if (!has_bands()) {
        return min(baked, faded_dynamic(c, 0.0, FIVE_TAPS));
    }
    if (!cascaded) {
        return min(faded_dynamic(c, 0.0, FIVE_TAPS), band_term(world));
    }
    if (dynamic_fade(c) > 0.0) {
        return map_lit(c, 0u, NINE_TAPS);
    }
    return band_term(world);
}

// The model and map-object term, 1 lit to 0 shadowed; past mode 2 an interior takes none (`0x8745d0`).
fn model_term(world: vec3<f32>, interior: bool, cascaded: bool, cutoff: f32) -> f32 {
    if (!shadow_maps_on()) {
        return 1.0;
    }
    let bands = has_bands();
    if (interior && bands) {
        return 1.0;
    }
    let c = map_coord(world, 0u);
    if (bands && cascaded) {
        var d = 1.0;
        if (dynamic_fade(c) > 0.0) {
            d = map_lit(c, 0u, FIVE_TAPS);
        }
        return min(d, band_term(world));
    }
    let eye_z = -(view.view_from_world * vec4<f32>(world, 1.0)).z;
    let d = faded_dynamic(c, 0.01, select(FIVE_TAPS, NINE_TAPS, eye_z <= NEAR_TAPS_DEPTH));
    if (bands) {
        return min(d, band_term(world));
    }
    // The plane of `0x7bb670`: at modes 1 and 2 the unit map fades out over the yard above `cutoff`.
    return mix(d, 1.0, saturate(world.y - cutoff));
}

// The model and map-object programs' grazing fade: the shadow eases off as the surface turns
// edge-on to the shadow sun, by `sat(|1.2 − |N·L||)^4`.
fn grazing_fade(shadow: f32, world_normal: vec3<f32>) -> f32 {
    let nl = abs(dot(world_normal, lights.directional_lights[0].direction_to_light));
    let k = pow(saturate(abs(1.2 - nl)), 4.0);
    return k * (1.0 - shadow) + shadow;
}

// The diffuse scale a shadow term gives: 0.7 in full shadow.
fn shadow_scale(shadow: f32) -> f32 {
    return 0.3 * shadow + 0.7;
}
