// The exterior shadow-map receive (`extShadowQuality` 1 to 5), shared by terrain.wgsl and
// wow_model.wgsl. The maps are the one directional light's cascades (`benilla_world::ext_shadow`):
// cascade 0 the 20 yd dynamic map, cascades 1-3 the 40/160/640 yd bands. With no such light (mode
// 0) every call answers lit and samples nothing.
//
// The reference's receive (`Terrain2_pcf.bls`, `Terrain3_pcf.bls`, the model and map-object pixel
// programs; `hwPCF` defaults to 1):
//   lit(map) = (1/5) Σ hardware-PCF compare at the centre and four of the eight offsets `0x8742e0`
//   dynamic: d = 1 + f·(lit − 1), f = sat(3.4137931 − 3.4482758·max|xy|), so it fades out from
//     0.68 to 0.99 of the box
//   bands:   band 0 inside its box, else band 1 inside its box, else band 2 faded as
//     1 + g·(lit − 1), g = sat(11 − 11.111111·max|xy|); outside every band lit
//   shadow = min(dynamic, bands); the colour's diffuse part is scaled by 0.7 + 0.3·shadow.
#define_import_path benilla_assets::wow_shadow

#import bevy_pbr::mesh_view_bindings::{
    lights,
    directional_shadow_textures,
    directional_shadow_textures_comparison_sampler,
}

// The four PCF taps beside the centre, offsets 0, 2, 4 and 6 of `0x8742e0`, in texels.
const PCF_TAPS = array<vec2<f32>, 4>(
    vec2<f32>(0.8, -1.0),
    vec2<f32>(0.2, -0.6),
    vec2<f32>(-0.6, -0.2),
    vec2<f32>(-1.0, -0.4),
);

// The receiving depth bias per cascade in yards, `0x7bac10` under `hwPCF`: 0.5 + 0.2·{0.5 for the
// dynamic map, 2, 4, 8 for the bands}.
const CASCADE_BIAS = array<f32, 4>(0.6, 0.9, 1.3, 2.1);

// The maps' depth range, [1, 4000] yd (`0x7bac10`), in Bevy's reverse-Z clip depth.
const DEPTH_SPAN: f32 = 3999.0;

// Whether a shadow-map mode is live: the light's shadows are on.
fn shadow_maps_on() -> bool {
    return lights.n_directional_lights > 0u
        && (lights.directional_lights[0].flags & 1u) != 0u;
}

// Cascade `i`'s box coordinate of a world point: xy in [-1, 1] across the box, z its biased
// reverse-Z depth.
fn map_coord(world: vec3<f32>, i: u32) -> vec3<f32> {
    let clip = lights.directional_lights[0].cascades[i].clip_from_world * vec4<f32>(world, 1.0);
    let ndc = clip.xyz / clip.w;
    var bias = CASCADE_BIAS;
    return vec3<f32>(ndc.xy, ndc.z + bias[i] / DEPTH_SPAN);
}

// The lit fraction of cascade `i` at box coordinate `c`, five hardware-PCF taps.
fn map_lit(c: vec3<f32>, i: u32) -> f32 {
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
    for (var k = 0u; k < 4u; k = k + 1u) {
        sum += textureSampleCompareLevel(
            directional_shadow_textures,
            directional_shadow_textures_comparison_sampler,
            uv + taps[k] * texel,
            layer,
            c.z,
        );
    }
    return sum * 0.2;
}

fn box_extent(c: vec3<f32>) -> f32 {
    return max(abs(c.x), abs(c.y));
}

// The dynamic map's term, faded out toward its box edge.
fn dynamic_term(world: vec3<f32>) -> f32 {
    let c = map_coord(world, 0u);
    let f = saturate(3.4137931 - 3.4482758 * box_extent(c));
    if (f <= 0.0) {
        return 1.0;
    }
    return min(1.0 + f * (map_lit(c, 0u) - 1.0), 1.0);
}

// The bands' term: the innermost band whose box holds the point, band 2 faded at its edge.
fn band_term(world: vec3<f32>) -> f32 {
    let c0 = map_coord(world, 1u);
    if (box_extent(c0) < 1.0) {
        return map_lit(c0, 1u);
    }
    let c1 = map_coord(world, 2u);
    if (box_extent(c1) < 1.0) {
        return map_lit(c1, 2u);
    }
    let c2 = map_coord(world, 3u);
    let g = saturate(11.0 - 11.111111 * box_extent(c2));
    if (g <= 0.0) {
        return 1.0;
    }
    return 1.0 + g * (map_lit(c2, 3u) - 1.0);
}

// The shadow term at a world point, 1 lit to 0 shadowed. An interior receiver reads only a map of
// units (`0x8745d0`); past mode 2 the dynamic map also holds the environment, so it takes none.
fn shadow_term(world: vec3<f32>, interior: bool) -> f32 {
    if (!shadow_maps_on()) {
        return 1.0;
    }
    let bands = lights.directional_lights[0].num_cascades >= 4u;
    if (interior && bands) {
        return 1.0;
    }
    var s = dynamic_term(world);
    if (bands) {
        s = min(s, band_term(world));
    }
    return s;
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
