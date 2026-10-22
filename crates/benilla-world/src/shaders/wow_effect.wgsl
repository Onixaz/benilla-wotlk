// The effect-lane pass (particles, ribbons, decals, water foam, precipitation): texture bytes,
// never decoded on sample, times the authored track colour in gamma space, as the reference
// multiplies them. The output stays gamma; the FFXGlow combine decodes the frame once.
//
// Blend variants, one shader def per EffectBlend:
// - BLEND_ADD:      (rgb·α, 0) under (One, 1−srcα), the reference's `src·α` byte add.
// - BLEND_ALPHA:    straight (rgb, α) under standard alpha blending.
// - BLEND_OPAQUE:   (rgb, 1) with blending off.
// - BLEND_ALPHAKEY: BLEND_OPAQUE behind the alpha test (EGxRs id 0x08), a discard below 224/255.
// - BLEND_MULTIPLY: (rgb·α, α) under (Dst, 1−srcα) = `dst·lerp(1, rgb, α)`, the blob shadow's
//   modulate-with-fade and ModelBlend::Mod at α = 1.
// - BLEND_MOD2X:    (rgb, 1) under (Dst, Src) = `2·src·dst`, rain's state; reads no alpha.

#import bevy_render::view::View

// Prefix of `lighting::global_light`'s buffer; keep in sync with wow_model.wgsl's copy.
struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,  // rgb row-7 fog (gamma); w = enable (>0.5)
    fog_params: vec4<f32>, // x=start y=end z=linear-lighting A/B flag w=farclip wall
    sh_c10_r: vec4<f32>,
    sh_c10_g: vec4<f32>,
    sh_c10_b: vec4<f32>,
    sh_c13_r: vec4<f32>,
    sh_c13_g: vec4<f32>,
    sh_c13_b: vec4<f32>,
    sh_c16: vec4<f32>,
    _water: array<vec4<f32>, 4>,
    grade: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var effect_texture: texture_2d<f32>;
@group(1) @binding(1) var effect_sampler: sampler;
@group(1) @binding(2) var<storage, read> wow_light: WowLight;
// Per-draw params. `fog`: x the fog colour policy, `0x70baf0`'s per-blend table (0 off for file
// flag 0x8, 1 scene, 2 black for Add, 3 white for Mod, 4 grey for Mod2x); y rain's forced fog, zw
// its start and end. `clip`: the target rect in pixels (min.xy, max.xy), the whole target when
// z <= x; the reference clips a UI model to its widget's viewport, and model panes share an atlas.
struct EffectParams {
    fog: vec4<f32>,
    clip: vec4<f32>,
};
@group(1) @binding(3) var<uniform> wow_params: EffectParams;

// The rain pass's forced fog colour: 0x80808080 → grey (render-state 0x0d).
const RAIN_FOG_GREY: vec3<f32> = vec3<f32>(0.50196078, 0.50196078, 0.50196078);

struct Vertex {
    // Camera-relative for f32 precision (absolute verts through `clip_from_world` shear thin
    // geometry far from the origin), except decals under DECAL_WORLD_CLIP, which arrive absolute.
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,    // raw authored gamma RGBA; α is the blend weight
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Planar eye-Z for fog and the farclip wall, positive in front of the camera.
    @location(2) view_z: f32,
};

#ifdef EFFECT_PARTICLES
// GPU-expanded particles: `particles/quads.rs::expand_quads` per vertex. A particle owns 12
// vertex ids, its head quad then its tail quad, each the CPU writer's `[0, 1, 2, 0, 2, 3]` over
// its four corners; a quad the CPU would not push collapses to one clipped point.
struct GpuPool {
    mat: array<vec4<f32>, 3>,
    base_r: vec4<f32>,
    base_u: vec4<f32>,
    cam_r: vec4<f32>,
    cam_u: vec4<f32>,
    color_t: vec4<f32>,
    color_v: array<vec4<f32>, 4>,
    alpha_t: vec4<f32>,
    alpha_v: vec4<f32>,
    scale_t: vec4<f32>,
    scale_v: array<vec4<f32>, 2>,
    head_t: vec4<f32>,
    head_v: vec4<f32>,
    tail_t: vec4<f32>,
    tail_v: vec4<f32>,
    counts: vec4<u32>,
    counts2: vec4<u32>,
    params: vec4<f32>,
    params2: vec4<f32>,
    light: vec4<f32>,
};

struct GpuParticle {
    pos: vec3<f32>,
    age: f32,
    vel: vec3<f32>,
    life: f32,
    size_mul: vec2<f32>,
    base_spin: f32,
    spin: f32,
    phase: u32,
    cell: u32,
    pool: u32,
    pad: u32,
};

@group(2) @binding(0) var<storage, read> pools: array<GpuPool>;
@group(2) @binding(1) var<storage, read> particles: array<GpuParticle>;
@group(2) @binding(2) var<storage, read> twinkle: array<vec4<f32>, 32>;

const DRAWS_HEAD: u32 = 1u;
const DRAWS_TAIL: u32 = 2u;
const ROTATES: u32 = 4u;
const RANDOM_SPIN_SIGN: u32 = 8u;
const TAIL_CLAMPS: u32 = 16u;

// `LifeTrack::locate` (`0x9793b0`): two keys span the life whatever their times say, three split
// at the middle key, four take the binary search over the inner times. (lo, hi, fraction).
fn locate(t: vec4<f32>, n: u32, u: f32) -> vec3<f32> {
    if (n <= 2u) {
        return vec3<f32>(0.0, 1.0, u);
    }
    if (n == 3u) {
        let mid = t.y;
        if (u < mid) {
            return vec3<f32>(0.0, 1.0, u / mid);
        }
        return vec3<f32>(1.0, 2.0, (u - mid) / (1.0 - mid));
    }
    var k = 0u;
    if (t.y <= u) { k = 1u; }
    if (t.z <= u) { k = 2u; }
    var tt = t;
    return vec3<f32>(f32(k), f32(k + 1u), (u - tt[k]) / (tt[k + 1u] - tt[k]));
}

fn track1(t: vec4<f32>, v: vec4<f32>, n: u32, u: f32, def: f32) -> f32 {
    if (n == 0u) { return def; }
    if (n == 1u) { return v.x; }
    let l = locate(t, n, u);
    var vv = v;
    let a = vv[u32(l.x)];
    let b = vv[u32(l.y)];
    return a + (b - a) * l.z;
}

fn color_key(pool: GpuPool, i: u32) -> vec3<f32> {
    return pool.color_v[i].xyz;
}

fn scale_key(pool: GpuPool, i: u32) -> vec2<f32> {
    let row = pool.scale_v[i / 2u];
    if ((i & 1u) == 0u) { return row.xy; }
    return row.zw;
}

// The colour bytes' `fistp` round (`0x979702`): ties to even, then the byte's wrap.
fn byte(v: f32) -> f32 {
    return f32(bitcast<u32>(i32(round(v))) & 255u) / 255.0;
}

// `cell_lerp`: `fistp` of the lerp, the u16 wrap.
fn cell_of(t: vec4<f32>, v: vec4<f32>, n: u32, u: f32) -> u32 {
    if (n == 1u) { return bitcast<u32>(i32(round(v.x))) & 65535u; }
    let l = locate(t, n, u);
    var vv = v;
    let a = vv[u32(l.x)];
    let b = vv[u32(l.y)];
    return bitcast<u32>(i32(round(a + (b - a) * l.z))) & 65535u;
}

fn dead() -> VertexOutput {
    var out: VertexOutput;
    // Every corner of a dropped quad lands on this one clipped point: zero area, nothing drawn.
    out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
    out.uv = vec2<f32>(0.0);
    out.color = vec4<f32>(0.0);
    out.view_z = 0.0;
    return out;
}

@vertex
fn vertex(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let p = particles[vi / 12u];
    let pool = pools[p.pool];
    let local = vi % 12u;
    let is_tail = local >= 6u;
    let k = local % 6u;
    var corner = k;
    if (k == 3u) { corner = 0u; } else if (k == 4u) { corner = 2u; } else if (k == 5u) { corner = 3u; }
    let flags = pool.counts2.y;
    if ((!is_tail && (flags & DRAWS_HEAD) == 0u) || (is_tail && (flags & DRAWS_TAIL) == 0u)) {
        return dead();
    }
    // The twinklePercent gate (`0x7b2adc`) on the table's `(floor(clamp(speed·age)) + phase)`.
    let idx = (u32(clamp(pool.params.z * p.age, 0.0, 255.0)) + p.phase) & 127u;
    var row = twinkle[idx >> 2u];
    let noise = row[idx & 3u];
    if (pool.params.w < 1.0 && noise > pool.params.w) {
        return dead();
    }
    let u = clamp(p.age / p.life, 0.0, 1.0);
    var rgb = vec3<f32>(255.0);
    let cn = pool.counts.x;
    if (cn == 1u) {
        rgb = color_key(pool, 0u);
    } else if (cn > 1u) {
        let l = locate(pool.color_t, cn, u);
        let a = color_key(pool, u32(l.x));
        let b = color_key(pool, u32(l.y));
        rgb = a + (b - a) * l.z;
    }
    let alpha = track1(pool.alpha_t, pool.alpha_v, pool.counts.y, u, 1.0);
    var color = vec4<f32>(byte(rgb.x), byte(rgb.y), byte(rgb.z), alpha);
    color.w = color.w * pool.params.x;
    var size = vec2<f32>(1.0);
    let sn = pool.counts.z;
    if (sn == 1u) {
        size = scale_key(pool, 0u);
    } else if (sn > 1u) {
        let l = locate(pool.scale_t, sn, u);
        let a = scale_key(pool, u32(l.x));
        let b = scale_key(pool, u32(l.y));
        size = vec2<f32>(a.x + (b.x - a.x) * l.z, a.y + (b.y - a.y) * l.z);
    }
    let center = vec3<f32>(
        dot(pool.mat[0].xyz, p.pos) + pool.mat[0].w,
        dot(pool.mat[1].xyz, p.pos) + pool.mat[1].w,
        dot(pool.mat[2].xyz, p.pos) + pool.mat[2].w,
    );
    let tmin = pool.params2.x;
    let tmax = pool.params2.y;
    var twk = noise * (tmax - tmin) + tmin;
    if (abs(tmax - tmin) < 1e-6) { twk = 1.0; }
    let kk = twk * pool.params.y;
    let half = vec2<f32>(size.x * p.size_mul.x * kk, size.y * p.size_mul.y * kk);
    let cols = pool.counts2.z;
    let rows = pool.counts2.w;
    let inv_cols = 1.0 / f32(cols);
    let inv_rows = 1.0 / f32(rows);
    var cell = 0u;
    var world = center;
    var uv = vec2<f32>(0.0);
    if (!is_tail) {
        cell = p.cell;
        if (pool.counts.w > 0u) {
            cell = cell_of(pool.head_t, pool.head_v, pool.counts.w, u);
        }
        let cx = f32(cell & (cols - 1u));
        let cy = f32(cell >> countTrailingZeros(cols));
        let u0 = cx * inv_cols;
        let u1 = (cx + 1.0) * inv_cols;
        let v0 = cy * inv_rows;
        let v1 = (cy + 1.0) * inv_rows;
        let base_r = pool.base_r.xyz;
        let base_u = pool.base_u.xyz;
        var r = base_r * half.x;
        var up = base_u * half.y;
        if ((flags & ROTATES) != 0u) {
            var angle = p.spin * p.age + p.base_spin;
            if ((flags & RANDOM_SPIN_SIGN) != 0u && (p.phase & 32u) != 0u) {
                angle = -angle;
            }
            let sa = sin(angle);
            let ca = cos(angle);
            r = (base_r * ca + base_u * sa) * half.x;
            up = (base_u * ca - base_r * sa) * half.y;
        }
        if (corner == 0u) { world = center - r - up; uv = vec2<f32>(u0, v1); }
        else if (corner == 1u) { world = center + r - up; uv = vec2<f32>(u1, v1); }
        else if (corner == 2u) { world = center + r + up; uv = vec2<f32>(u1, v0); }
        else { world = center - r + up; uv = vec2<f32>(u0, v0); }
    } else {
        if (pool.counts2.x > 0u) {
            cell = cell_of(pool.tail_t, pool.tail_v, pool.counts2.x, u);
        }
        let cx = f32(cell & (cols - 1u));
        let cy = f32(cell >> countTrailingZeros(cols));
        let u0 = cx * inv_cols;
        let u1 = (cx + 1.0) * inv_cols;
        let v0 = cy * inv_rows;
        let v1 = (cy + 1.0) * inv_rows;
        let half_t = half.x;
        let vel_world = vec3<f32>(
            dot(pool.mat[0].xyz, p.vel),
            dot(pool.mat[1].xyz, p.vel),
            dot(pool.mat[2].xyz, p.vel),
        );
        var t_eff = pool.params2.z;
        if ((flags & TAIL_CLAMPS) != 0u) { t_eff = min(t_eff, p.age); }
        let tail = -vel_world * t_eff;
        let cam_r = pool.cam_r.xyz;
        let cam_u = pool.cam_u.xyz;
        let tr = dot(tail, cam_r);
        let tu = dot(tail, cam_u);
        let l2 = tr * tr + tu * tu;
        if (l2 < 7.7e-4) {
            // Degenerate: the plain-billboard fallback (`0x7b33fa`).
            let r = cam_r * half_t;
            let up = cam_u * half_t;
            if (corner == 0u) { world = center - r - up; uv = vec2<f32>(u0, v1); }
            else if (corner == 1u) { world = center + r - up; uv = vec2<f32>(u1, v1); }
            else if (corner == 2u) { world = center + r + up; uv = vec2<f32>(u1, v0); }
            else { world = center - r + up; uv = vec2<f32>(u0, v0); }
        } else {
            let inv_l = half_t / sqrt(l2);
            let perp = (cam_u * tr - cam_r * tu) * inv_l;
            if (corner == 0u) { world = center - perp; uv = vec2<f32>(u0, v1); }
            else if (corner == 1u) { world = center + perp; uv = vec2<f32>(u0, v0); }
            else if (corner == 2u) { world = center + tail + perp; uv = vec2<f32>(u1, v0); }
            else { world = center + tail - perp; uv = vec2<f32>(u1, v1); }
        }
    }
    // The committed light, folded per channel and clamped, as `fold_committed_light`.
    if (pool.light.w > 0.5) {
        color = vec4<f32>(clamp(color.xyz * pool.light.xyz, vec3<f32>(0.0), vec3<f32>(1.0)), color.w);
    }
    var out: VertexOutput;
    // The CPU writer's rebase, here: the absolute corner less the same `world_position`.
    let rel = world - view.world_position;
    let view_pos = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    ) * rel;
    out.clip_position = view.clip_from_view * vec4<f32>(view_pos, 1.0);
    out.view_z = -view_pos.z;
    out.uv = uv;
    out.color = color;
    return out;
}
#else
@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
#ifdef DECAL_WORLD_CLIP
    // Decals (raster_bias ≠ 0) take absolute verts through the world meshes' `clip_from_world`:
    // their depth must tie the ground within the raster bias, and the cam-relative route rounds
    // differently by more than that at world-scale coordinates.
    out.clip_position = view.clip_from_world * vec4<f32>(v.position, 1.0);
    // Eye-Z via the full affine transform; its rounding is yard-scale and harmless.
    out.view_z = -(view.view_from_world * vec4<f32>(v.position, 1.0)).z;
#else
    // Cam-relative verts: view_from_world is [R | −R·cam] and the rebase already subtracted cam,
    // so only the rotation applies.
    let view_pos = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    ) * v.position;
    out.clip_position = view.clip_from_view * vec4<f32>(view_pos, 1.0);
    out.view_z = -view_pos.z;
#endif
    out.uv = v.uv;
    out.color = v.color;
    return out;
}
#endif

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The target-rect clip; `@builtin(position)` is the framebuffer pixel, a UI tile's atlas texel.
    if (wow_params.clip.z > wow_params.clip.x) {
        let p = in.clip_position.xy;
        if (p.x < wow_params.clip.x || p.y < wow_params.clip.y
            || p.x > wow_params.clip.z || p.y > wow_params.clip.w) {
            discard;
        }
    }
    // `$WOW_PARTICLE_FLAT` debug instrument: solid magenta, no inputs.
#ifdef WOW_PARTICLE_FLAT
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
#else
    // The hard farclip wall (the live `farclip`, 177 to 777 yd): the reference's far plane clips
    // effects and ours is farther, so every world shader discards. The fog ends by the farclip, so
    // this cuts only fog.
    if (wow_light.fog_params.w > 0.0 && in.view_z > wow_light.fog_params.w) {
        discard;
    }
    // Texture bytes × authored vertex colour in gamma space; `view.mip_bias` is the render-scale
    // LOD compensation, 0.0 at native and above.
    let c = textureSampleBias(effect_texture, effect_sampler, in.uv, view.mip_bias) * in.color;
#ifdef BLEND_ALPHAKEY
    // The fixed-function alpha test (EGxRs id 0x08, `glAlphaFunc(GL_GEQUAL, ref/255)`): `0x70c256`
    // sets ref = round(instanceAlpha × 224), 224 at full alpha, and `c.a` is the reference's
    // fragment alpha (texture α × track α, GL_MODULATE). GEQUAL passes at the ref.
    if (c.a < 224.0 / 255.0) {
        discard;
    }
#endif
    var rgb = c.rgb;
#ifdef EFFECT_LIT
    // Scene lighting (EGxRs id 0x0e, `0x70d8b0`) when the emitter clears the unlit flag 0x1 and
    // its blend is not Mod or Mod2x (`0x70bb00`): the fixed-function matte with N = world up for
    // the whole draw, as `0x7b3fd0` takes the normal from view row 2 and `0x71bce0` moves the
    // light into that frame, so N·L holds as the camera orbits. Bevy +Y is WoW +Z.
    let L = -normalize(wow_light.light_sun.xyz);
    let N = vec3<f32>(0.0, 1.0, 0.0);
    let lit = clamp(
        wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * max(dot(N, L), 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    rgb = rgb * lit;
#endif
    // Scene fog: the world's linear fog (same start and end, planar eye-Z) in gamma space before
    // the blend; only its colour follows the per-blend policy in `wow_params.fog.x`.
    if (wow_light.fog_color.w > 0.5 && wow_params.fog.x > 0.5) {
        let denom = max(wow_light.fog_params.y - wow_light.fog_params.x, 0.001);
        let factor = clamp((wow_light.fog_params.y - in.view_z) / denom, 0.0, 1.0);
        var fog_rgb = wow_light.fog_color.xyz;
        if (wow_params.fog.x > 1.5 && wow_params.fog.x < 2.5) { fog_rgb = vec3<f32>(0.0); }
        else if (wow_params.fog.x > 2.5 && wow_params.fog.x < 3.5) { fog_rgb = vec3<f32>(1.0); }
        else if (wow_params.fog.x > 3.5) { fog_rgb = RAIN_FOG_GREY; }
        rgb = mix(fog_rgb, rgb, factor);
    }
    // Rain's forced fog: grey over the draw's own start/end, whatever the scene fog; grey is
    // neutral under Mod2x, so this is the streaks' distance fade.
    if (wow_params.fog.y > 0.5) {
        let denom = max(wow_params.fog.w - wow_params.fog.z, 0.001);
        let factor = clamp((wow_params.fog.w - in.view_z) / denom, 0.0, 1.0);
        rgb = mix(RAIN_FOG_GREY, rgb, factor);
    }
#ifdef BLEND_ADD
    // Premultiplied in gamma, so stacked quads sum like the reference's bytes.
    return vec4<f32>(rgb * c.a, 0.0);
#else
#ifdef BLEND_OPAQUE
    return vec4<f32>(rgb, 1.0);
#else
#ifdef BLEND_ALPHAKEY
    // Blending is off (EGxBlend 1 → `glDisable(GL_BLEND)`); the surviving fragments are solid.
    return vec4<f32>(rgb, 1.0);
#else
#ifdef BLEND_MULTIPLY
    return vec4<f32>(rgb * c.a, c.a);
#else
#ifdef BLEND_MOD2X
    return vec4<f32>(rgb, 1.0);
#else
    return vec4<f32>(rgb, c.a);
#endif
#endif
#endif
#endif
#endif
#endif
}
