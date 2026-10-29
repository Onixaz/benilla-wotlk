// Terrain splat shader: a custom vertex and fragment stage on Bevy's StandardMaterial. Blends up to
// four tiled layers by a per-chunk alpha map, lit as the reference's fixed-function terrain:
//   diffuse  = clamp(ambient (row 1) + diffuse (row 0)·max(N·L, 0) + Σ point·att·N·L), evaluated
//     and clamped per vertex (GL T&L), Gouraud-interpolated, modulated 1× into the texture;
//   specular = clamp(row 9 · max(N·H, 0)^20) per vertex (local viewer), times the sheen mask (the
//     `_s` texture's per-texel alpha, blended like the colour), added after the modulate.
// Evaluated per pixel, or unmasked, the near-white row 9 would wash lit ground to cream.
//
// Bevy's `VertexOutput` has no slot for the interpolated specular, so the vertex transform and IO
// struct are our own and `main_pass_post_lighting_processing` is dropped; fog is in-shader.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
    mesh_view_bindings::view,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var layer_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var alpha_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var splat_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var shadow_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var env_cubes: texture_cube_array<f32>;

// Per-tile Vec4 uniforms, packed into one buffer (binding 106); the field order must match the Rust
// `TerrainExtension`. params.x = layer tiling factor; params.y = 1 on a big-alpha map; zw unused.
struct TerrainParams {
    params: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var<uniform> t: TerrainParams;

// The shared global light (`lighting::global_light`), updated in place once a frame; this mirrors
// its row layout, which must match.
struct WowLight {
    light_ambient: vec4<f32>, // rgb = row 1 ambient; w = Mod2x scale (×1).
    light_diffuse: vec4<f32>, // rgb = row 0 sun diffuse; w = clamp-light flag (>0.5 ⇒ saturate).
    light_sun: vec4<f32>,     // xyz = world-space sun travel direction (to-light = −xyz); w unused.
    light_spec: vec4<f32>,    // rgb = row 9 specular color; w = shininess (20). rgb == 0 disables.
    fog_color: vec4<f32>,     // rgb = row 7 fog (raw, gamma 0..1); w = enable (>0.5 ⇒ blend).
    fog_params: vec4<f32>,    // x = fog_start yd; y = fog_end yd; z unused; w = farclip wall.
    _sh: array<vec4<f32>, 6>, // rows 6-11: model SH coefficients, unread by terrain.
    sh_c16: vec4<f32>,        // row 12: xyz = the models' c16 quad band; w free.
    _water: array<vec4<f32>, 4>, // rows 13-16: liquid swatches, unread by terrain.
    grade: vec4<f32>,         // row 17: reserved, layout only.
    _wmo_fog: vec4<f32>,      // row 18: interior fog, unread by terrain.
    // Row 19: xy interior fog, unread here; z = `specular` (Terrain Highlights) on; w = 1 at
    // Terrain Texture Low, the half-size alpha and shadow maps (`global_light::build_light_data`).
    terrain_opts: vec4<f32>,
    // The dynamic point-light table (`global_light::build_light_data`): row 20 `.x` = live count,
    // then two rows per light, `[pos.xyz, range]` and `[rgb, 0]`.
    point_count: vec4<f32>,
    points: array<vec4<f32>, 512>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

// The vertex-to-fragment payload; `specular` is clamped per vertex, then Gouraud-interpolated.
struct TerrainVsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(5) specular: vec3<f32>,
    // The Gouraud diffuse, summed and clamped per vertex as GL T&L does: an over-gamut light (a
    // carried torch is (1.4, 0.87, 0.40)) saturates one vertex and falls off linearly from it,
    // where clamping per pixel would pin a wide plateau at white.
    @location(6) primary: vec3<f32>,
    // A reflection base layer's cube lookup, generated per vertex as the reference's texgen is.
    @location(7) env_dir: vec3<f32>,
}

// Terrain's point-light candidacy half-width (yd): `w + 10`, `w` being the chunk's bounding-sphere
// radius `sqrt(2·16.666666² + (zExtent/2)²)`, 23.570166 on flat ground.
// This takes the flat `w` everywhere, so steep chunks gather slightly narrower than the reference;
// the lights that differ are 33 yd out or more, where att ≈ 0.02.
const TERRAIN_REACH: f32 = 33.570166;

// Terrain Texture Low: a chunk's 64² alpha or shadow map as the reference's half-size copy, which
// keeps every second texel of every second row, the last column from texel 63
// (`CMapRenderChunk::UnpackAlphaBitsFixed8888Mip1 0x7b8c70` and its siblings), sampled bilinearly
// at the full map's coordinates. Level 0 is the arrays' only mip.
fn half_map(tex: texture_2d_array<f32>, uv: vec2<f32>, layer: i32) -> vec4<f32> {
    let p = uv * 32.0 - 0.5;
    let base = floor(p);
    let f = p - base;
    let i0 = clamp(vec2<i32>(base), vec2<i32>(0), vec2<i32>(31));
    let i1 = clamp(i0 + vec2<i32>(1), vec2<i32>(0), vec2<i32>(31));
    let src = vec4<i32>(
        select(i0.x * 2, 63, i0.x == 31),
        select(i1.x * 2, 63, i1.x == 31),
        i0.y * 2,
        i1.y * 2,
    );
    let t00 = textureLoad(tex, vec2<i32>(src.x, src.z), layer, 0);
    let t10 = textureLoad(tex, vec2<i32>(src.y, src.z), layer, 0);
    let t01 = textureLoad(tex, vec2<i32>(src.x, src.w), layer, 0);
    let t11 = textureLoad(tex, vec2<i32>(src.y, src.w), layer, 0);
    return mix(mix(t00, t10, f.x), mix(t01, t11, f.x), f.y);
}

// The dynamic point-light term. The reference commits at most three point lights per draw, the
// nearest by squared distance to the receiving unit, into GL slots 1-3; terrain's unit is the MCNK
// chunk, so `anchor` is its cell centre. Candidacy is a 20-yd spatial-hash sweep over
// `[floor((c - w - 10)/20), floor((c + w + 10)/20)]`, a Chebyshev box (`w` is `chunk+0x68`,
// copied) of half-width `w + 10`, at least `TERRAIN_REACH`. A committed light is
// diffuse only (the ambient and specular slots are zeroed), falls off as
// `1/(0.7·d + 0.03·d²)` on the vertex normal, and has no distance cutoff. The M2 lane in
// wow_model.wgsl uses the packed per-light range: the lanes differ on purpose.
fn point_light_sum(P: vec3<f32>, N: vec3<f32>, anchor: vec3<f32>) -> vec3<f32> {
    let count = u32(wow_light.point_count.x);
    var sel = array<u32, 3>(0u, 0u, 0u);
    var sd = array<f32, 3>(1e30, 1e30, 1e30);
    for (var i = 0u; i < count; i = i + 1u) {
        let pos_range = wow_light.points[2u * i];
        let dv = pos_range.xyz - anchor;
        let d2 = dot(dv, dv);
        // The hash sweep is horizontal (the grid keys on WoW x/y = Bevy z/x) and has no vertical
        // bound; ranking below is the full 3-D distance, as the reference does.
        if (max(abs(dv.x), abs(dv.z)) > TERRAIN_REACH) {
            continue;
        }
        if (d2 < sd[0]) {
            sd[2] = sd[1]; sel[2] = sel[1];
            sd[1] = sd[0]; sel[1] = sel[0];
            sd[0] = d2; sel[0] = i;
        } else if (d2 < sd[1]) {
            sd[2] = sd[1]; sel[2] = sel[1];
            sd[1] = d2; sel[1] = i;
        } else if (d2 < sd[2]) {
            sd[2] = d2; sel[2] = i;
        }
    }
    var sum = vec3<f32>(0.0);
    for (var s = 0u; s < 3u; s = s + 1u) {
        if (sd[s] > 9.9e29) {
            break;
        }
        let pos_range = wow_light.points[2u * sel[s]];
        let to_light = pos_range.xyz - P;
        let d = length(to_light);
        let atten = 1.0 / (0.7 * d + 0.03 * d * d);
        let nl = max(dot(N, to_light / max(d, 1e-4)), 0.0);
        sum += wow_light.points[2u * sel[s] + 1u].rgb * (atten * nl);
    }
    return sum;
}

// The MCNK chunk cell centre under a world point: the reference's light anchor, the chunk's world
// AABB centre `CMapChunk+0x5c..0x64`. The grid is fixed world-wide (chunk
// 533.33333/16 yd, half-extent 32 tiles) and symmetric, and WoW x/y are Bevy −z/−x, so snapping
// Bevy x/z lands on the same cells; wow_model.wgsl mirrors it for clutter.
// The anchor height is the vertex's own y, not the chunk's mid-height `(minH + maxH)/2`, so on
// relief one chunk's vertices can select different lights: the per-chunk constant is not plumbed
// to the vertex stage. On flat ground the two agree.
fn mcnk_cell_anchor(P: vec3<f32>) -> vec3<f32> {
    let cell = 533.33333 / 16.0;
    let half = 32.0 * 533.33333;
    let ix = floor((half + P.x) / cell);
    let iz = floor((half + P.z) / cell);
    return vec3<f32>((ix + 0.5) * cell - half, P.y, (iz + 0.5) * cell - half);
}

// WoW axes from Bevy's: x north is −z, y west is −x, z up is y.
fn to_wow(b: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(-b.z, -b.x, b.y);
}

// The cube lookup of an MCLY `0x400` base layer, which `Terrain0_env.bls` samples as a CUBE: GL
// reflection-map texgen `reflect(normalize(P − eye), N)` in the reference's eye space, world-aligned
// with the camera at the origin, then the layer texture matrix (`0x7d06b0`): x and y swapped, the
// camera-to-chunk-origin offset added, scaled by −geoToTex, of which a cube reads only the sign.
// The origin is the chunk's NW corner, from its UV; its height is the vertex's, where the reference
// keeps the MCNK base, which is the surface on the flat ice these layers paint.
fn env_cube_dir(world: vec3<f32>, normal: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let chunk_yd = 533.33333 / 16.0;
    let p = to_wow(world);
    let eye = to_wow(view.world_position.xyz);
    let r = reflect(normalize(p - eye), to_wow(normal));
    let origin = vec3<f32>(p.x + uv.y * chunk_yd, p.y + uv.x * chunk_yd, p.z);
    let q = r + eye - origin;
    return -vec3<f32>(q.y, q.x, q.z);
}

@vertex
fn vertex(in: Vertex) -> TerrainVsOut {
    var out: TerrainVsOut;
    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    out.world_position =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(in.position, 1.0));
    out.clip_position = position_world_to_clip(out.world_position.xyz);
    let world_normal = mesh_functions::mesh_normal_local_to_world(in.normal, in.instance_index);
    out.uv = in.uv;
    out.uv_b = in.uv_b;
    out.color = in.color;

    // Sun specular per vertex: white material, row-9 colour, shininess `light_spec.w` (20), local
    // viewer; clamped here, then Gouraud-interpolated.
    let n = normalize(world_normal);
    out.env_dir = env_cube_dir(out.world_position.xyz, n, in.uv);
    let l = -normalize(wow_light.light_sun.xyz); // to-light (the sun travels along +light_sun)
    let v = normalize(view.world_position.xyz - out.world_position.xyz); // to-eye (local viewer)
    let h = normalize(l + v);
    let ndoth = max(dot(n, h), 0.0);
    out.specular = clamp(wow_light.light_spec.rgb * pow(ndoth, wow_light.light_spec.w), vec3<f32>(0.0), vec3<f32>(1.0));

    // The whole diffuse sum on the MCNR normal, clamped per vertex: ambient, sun N·L, and the
    // chunk's three nearest point lights with their over-gamut colours raw.
    let ndotl = max(dot(n, l), 0.0);
    let points = point_light_sum(out.world_position.xyz, n, mcnk_cell_anchor(out.world_position.xyz));
    out.primary = clamp(
        wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * ndotl + points,
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    return out;
}

@fragment
fn fragment(in: TerrainVsOut) -> @location(0) vec4<f32> {
    // The far-clip wall: the reference clips the detailed world per pixel at its far plane
    // (`farclip`, about 777 yd). Discard beyond `fog_params.w` (0 disables) on planar eye-Z.
    if (wow_light.fog_params.w > 0.0) {
        let clip_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        if (clip_z > wow_light.fog_params.w) {
            discard;
        }
    }

    // Per-chunk array indices baked per vertex, COLOR the 4 layers and UV1.x the alpha; constant
    // across a chunk, so they interpolate back to the integer.
    let li = vec4<i32>(round(in.color));
    let ai = i32(round(in.uv_b.x));

    let tiled = in.uv * t.params.x;
    // `.a` is the `_s` texture's sheen mask, 0 without an `_s`. `view.mip_bias` undoes the coarser
    // mip a render scale below 1 picks; the alpha and shadow arrays have one mip, so only the
    // layers take it.
    // A negative base index names a reflection cube (`-1 - index`). Both are sampled, since the
    // choice varies per vertex and implicit-derivative sampling must stay in uniform control flow.
    let layer_bias = view.mip_bias;
    let flat0 = textureSampleBias(layer_array, splat_samp, tiled, max(li.x, 0), layer_bias);
    let env0 =
        textureSampleBias(env_cubes, splat_samp, in.env_dir, max(-li.x - 1, 0), view.mip_bias);
    let s0 = select(flat0, env0, li.x < 0);
    let s1 = textureSampleBias(layer_array, splat_samp, tiled, li.y, layer_bias);
    let s2 = textureSampleBias(layer_array, splat_samp, tiled, li.z, layer_bias);
    let s3 = textureSampleBias(layer_array, splat_samp, tiled, li.w, layer_bias);
    // The alpha and shadow maps (one untiled 64² grid per chunk) share the layers' repeat sampler,
    // so a linear footprint at a chunk edge would wrap to the far edge; a half-texel inset clamps.
    let auv = clamp(in.uv, vec2<f32>(0.5 / 64.0), vec2<f32>(1.0 - 0.5 / 64.0));
    let low = wow_light.terrain_opts.w > 0.5;
    var a: vec4<f32>;
    if (low) {
        a = half_map(alpha_array, auv, ai);
    } else {
        a = textureSampleLevel(alpha_array, splat_samp, auv, ai, 0.0);
    }

    // The splat. On a big-alpha map the layers are a weighted sum: the base pass scales layer 0 by
    // `1 − saturate(a.r + a.g + a.b)` and each later pass adds its layer times its own weight
    // (`GxBlend_NoAlphaAdd`, `0x7d13f0`). Otherwise each layer lerps over the result so far
    // (`GxBlend_Alpha`, `0x7d1ad0`). The `.a` sheen mask blends with the colour's weights (the
    // fragment program's `secondary · texture[0].w`).
    var texel: vec4<f32>;
    if (t.params.y > 0.5) {
        let base_w = 1.0 - saturate(a.r + a.g + a.b);
        texel = s0 * base_w + s1 * a.r + s2 * a.g + s3 * a.b;
    } else {
        texel = mix(mix(mix(s0, s1, a.r), s2, a.g), s3, a.b);
    }
    let color = texel.rgb;
    let specmask = texel.a;

    let primary = in.primary;

    // MCSH baked shadow. With `pixelShaders` and `specular` on, terrain is one pass through
    // `terrainp_s.bls`, which reads the MCSH bit from the blend texture's alpha (1 lit, 0 shadowed)
    // and uses it twice, below; with them off the reference draws a separate ambient-tint overlay.
    // Our `shadow_array` is 255 shadowed, 0 lit; a chunk with no MCSH map (`uv_b.y < 0`) is lit.
    var shadow_lit = 1.0;
    let si = in.uv_b.y;
    if (si >= 0.0) {
        var mcsh: f32;
        if (low) {
            mcsh = half_map(shadow_array, auv, i32(round(si))).r;
        } else {
            mcsh = textureSampleLevel(shadow_array, splat_samp, auv, i32(round(si)), 0.0).r;
        }
        shadow_lit = 1.0 - mcsh;
    }

    // The `terrainp_s.bls` combine, then an LDR clamp:
    //   diffuse  = tex · primary · (0.3·shadow + 0.7)   (a flat −30% in shadow, no tint)
    //   specular = sheen · mask · shadow                 (none in shadow)
    let diffuse_term = color * primary * (0.3 * shadow_lit + 0.7);
    // Terrain Highlights off draws no sheen (`CMap::enableSpecular`, `0x7bd8a0`).
    let spec_term = in.specular * specmask * shadow_lit * wow_light.terrain_opts.z;
    var tuned = clamp(diffuse_term + spec_term, vec3<f32>(0.0), vec3<f32>(1.0));

    // Gamma-space GL_LINEAR fog on planar eye-Z, as the reference computes `fogcoord`, not radial
    // distance. Per pixel matches its per-vertex fog, since a linear factor interpolates exactly.
    if (wow_light.fog_color.w > 0.5) {
        let eye_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        let denom = max(wow_light.fog_params.y - wow_light.fog_params.x, 0.001);
        let factor = clamp((wow_light.fog_params.y - eye_z) / denom, 0.0, 1.0);
        tuned = mix(wow_light.fog_color.xyz, tuned, factor);
    }

    // Raw gamma out: the framebuffer holds gamma bytes and blends in gamma like the reference's;
    // the frame's one decode is the FFXGlow combine.
    return vec4<f32>(tuned, 1.0);
}
