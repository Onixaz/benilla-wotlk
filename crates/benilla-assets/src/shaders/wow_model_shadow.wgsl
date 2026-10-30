// The model lane's shadow-map caster pass (`ShadowMapSL.bls`), depth only: Bevy's shadow passes
// run it as `WowModelExt`'s prepass. What the reference gathers (`0x834660`): a batch of an
// opaque or alpha-key material whose alpha is at least 0.55, the alpha-key one alpha-tested.
// Anything else (blended, additive, Mod, Mod2x, ground clutter, the sky, the depth-prime and
// far-side twins) leaves the clip volume in the vertex stage.
//
// The struct blocks below are wow_model.wgsl's, verbatim (a test holds them equal): the skin
// palette is read from the shared light buffer.

#import bevy_pbr::{
    mesh_functions,
    mesh_view_bindings::view,
    pbr_bindings,
}

// Per-material uniforms at binding 100, wow_model.wgsl's `ModelParams`.
struct ModelParams {
    clutter_fade: vec4<f32>,
    model_flags: vec4<f32>,
    // x = the terrain-shade selector (see the doodad sun), y = the WMO batch order, zw = the
    // UV-scroll seed.
    sun_scale: vec4<f32>,
    // xyz = the animated M2Color tint (identity when static); w = the WMO interior batch class:
    // 0 exterior law, 1 INT, 2 TRANS.
    tint: vec4<f32>,
    // WMO glass, 0 on M2: xyz = the MOMT SIDN (0x10) emissive (gamma /255), w = MOMT WINDOW (0x20).
    sidn: vec4<f32>,
    // Rows of `wow_light.matanim`, 0 = identity: x = UV scroll, y = tint, z = texture-transform
    // affine, w = the UI tile's cell clip.
    anim_slots: vec4<f32>,
    // A WMO batch under its root's MOHD 0x2 (the unified law): xyz = the MOHD ambient (gamma /255),
    // w = 1. Zero elsewhere.
    wmo_unified: vec4<f32>,
    // An M2 batch's pixel combiner (`wow_combine`): x = its shader code + 1, 0 for the one-stage
    // modulate; y = bit 0 stage 1 on the sphere map, bit 1 stage 0 on the second UV set.
    combiner: vec4<f32>,
    // The combiner's stage-1 texture transform: xy = its offset at t = 0, z/w = its UV-scroll and
    // affine rows of `wow_light.matanim`, 0 = identity.
    stage1: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> m: ModelParams;

// The shared light buffer, wow_model.wgsl's `WowLight`.
struct WowLight {
    light_ambient: vec4<f32>, // rgb ambient; w = Mod2x scale
    light_diffuse: vec4<f32>, // rgb sun diffuse; w = clamp-light flag (>0.5 ⇒ saturate)
    light_sun: vec4<f32>,     // xyz sun travel dir (to-light = −xyz); w = directional enable
    light_spec: vec4<f32>,    // terrain's specular (w = its shininess); unread here
    fog_color: vec4<f32>,     // rgb row-7 fog (gamma); w = enable (>0.5)
    fog_params: vec4<f32>,    // x = start, y = end, w = farclip wall; z unused
    // Rows 6-12: the reference's `Model2.bls` order-2 SH probe of the scene light at intensity 1:
    // ambient DC in the c10 `.w` lanes, the sun's bands in c10.xyz, c13 and c16.xyz. Every sun band
    // is linear in the committed colour, so all scale by the intensity (never I²).
    sh_c10_r: vec4<f32>,
    sh_c10_g: vec4<f32>,
    sh_c10_b: vec4<f32>,
    sh_c13_r: vec4<f32>,
    sh_c13_g: vec4<f32>,
    sh_c13_b: vec4<f32>,
    // xyz = the c16 quadratic band (x²−y²) per channel; w unused.
    sh_c16: vec4<f32>,
    _water: array<vec4<f32>, 4>, // rows 13-16: the liquid swatches, unread here
    // x = the SIDN night fraction (1 overnight, 0 by day); yzw = the sun's SH DC at intensity 1.
    grade: vec4<f32>,
    // Rows 18-19: the interior fog, the 4 s camera-in-WMO MFOG crossfade (the scene fog outdoors).
    wmo_fog_color: vec4<f32>,    // rgb interior fog (gamma); w = enable (mirrors fog_color.w)
    wmo_fog_params: vec4<f32>,   // x = start yd, y = end yd; zw unused
    // Row 20 `.x` = the point-light count, then per light `[pos.xyz, range]`, `[rgb, 0]`. Here, not
    // in Bevy's clusterables, which the view layout gives the fragment stage only.
    point_count: vec4<f32>,
    points: array<vec4<f32>, 512>,
    // The interior-prop SH probes, 7 rows per slot (`MAX_PROP_PROBES` = 8192 slots). Only this
    // shader declares this tail; the other shaders bind the same buffer by its prefix.
    prop_probes: array<vec4<f32>, 57344>,
    // Per rig slot (sizes mirrored in rig_palette.rs): the base bone index into `palettes`, whose
    // 3 rows per bone are `rig_from_joint × inverse_bindpose` from the rig's own origin.
    rig_table: array<u32, 2048>,
    // Per rig slot: the M2 body tint (`model+0x184/188/18c`) packed `0xFFRRGGBB` like the
    // reference's node value (`param | 0xff000000`); 0 is identity.
    rig_tint: array<u32, 2048>,
    // Per rig slot: the world origin its palette rows are measured from.
    rig_origin: array<vec4<f32>, 2048>,
    // The mat-anim rows (size mirrored in mat_anim_table.rs), row 0 zero: a UV-scroll delta (xy),
    // a tint delta (xyz), a texture-transform affine `[cos − 1, sin, sx − 1, sy − 1]` or a UI cell.
    matanim: array<vec4<f32>, 2048>,
    // Per rig slot, the straddle waterline (size mirrored in straddle.rs): x = its world height
    // (Bevy Y), y = the side the near copy keeps (+1 above, −1 below, 0 not straddling).
    water_clip: array<vec2<f32>, 2048>,
    palettes: array<vec4<f32>>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

// The M2 alpha-key reference, 224/255, the caster's alpha test as the main pass's.
const VANILLA_ALPHA_KEY: f32 = 0.8784314;

// The gather's alpha floor (`0x834660`).
const CASTER_ALPHA_MIN: f32 = 0.55;

// The main pass's vertex locations, which `WowModelExt::specialize` lays out for this pass too.
struct ShadowVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef WOW_RIG_SKIN
    @location(10) joint_indices: vec4<u32>,
    @location(11) joint_weights: vec4<f32>,
#endif
}

struct ShadowVsOut {
    @builtin(position) position: vec4<f32>,
#ifdef VERTEX_UVS_A
    @location(0) uv: vec2<f32>,
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(1) unclipped_depth: f32,
#endif
}

#ifdef WOW_RIG_SKIN
fn wow_rig_slot(instance_index: u32) -> u32 {
    return (mesh_functions::get_tag(instance_index) >> 19u) & 0x7ffu;
}

// wow_model.wgsl's `wow_skin_model`: the four weighted bones' palette rows, relative to the rig's
// origin.
fn wow_skin_model(instance_index: u32, indices: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    let base = wow_light.rig_table[wow_rig_slot(instance_index)];
    let b0 = 3u * (base + indices.x);
    let b1 = 3u * (base + indices.y);
    let b2 = 3u * (base + indices.z);
    let b3 = 3u * (base + indices.w);
    let r0 = weights.x * wow_light.palettes[b0]
        + weights.y * wow_light.palettes[b1]
        + weights.z * wow_light.palettes[b2]
        + weights.w * wow_light.palettes[b3];
    let r1 = weights.x * wow_light.palettes[b0 + 1u]
        + weights.y * wow_light.palettes[b1 + 1u]
        + weights.z * wow_light.palettes[b2 + 1u]
        + weights.w * wow_light.palettes[b3 + 1u];
    let r2 = weights.x * wow_light.palettes[b0 + 2u]
        + weights.y * wow_light.palettes[b1 + 2u]
        + weights.z * wow_light.palettes[b2 + 2u]
        + weights.w * wow_light.palettes[b3 + 2u];
    return mat4x4<f32>(
        vec4<f32>(r0.x, r1.x, r2.x, 0.0),
        vec4<f32>(r0.y, r1.y, r2.y, 0.0),
        vec4<f32>(r0.z, r1.z, r2.z, 0.0),
        vec4<f32>(r0.w, r1.w, r2.w, 1.0),
    );
}
#endif

// Whether the batch casts at all: the gather's material and alpha gates on the marker bits
// (`clutter_fade.z`, materials.rs) and the instance's fade.
fn casts(instance_index: u32) -> bool {
    let markers = u32(m.clutter_fade.z);
    // Clutter; additive, Mod, Mod2x; the depth-prime and far-side twins; the sky.
    if (m.clutter_fade.w > 0.5 || (markers & (4u | 128u | 256u | 512u | 2048u | 8192u)) != 0u) {
        return false;
    }
    // Opaque intent (bit 3) is an opaque or alpha-key source; a fade twin is one mid-fade.
    if ((markers & 8u) == 0u && m.model_flags.y < 0.5) {
        return false;
    }
    // The tag's fade payload, bits 0-5 (0 untagged, opaque).
    let fade_tag = mesh_functions::get_tag(instance_index) & 0x3fu;
    return fade_tag == 0u || f32(fade_tag) / 63.0 >= CASTER_ALPHA_MIN;
}

@vertex
fn vertex(vertex: ShadowVertex) -> ShadowVsOut {
    var out: ShadowVsOut;
#ifdef WOW_RIG_SKIN
    let frame_from_local = wow_skin_model(
        vertex.instance_index,
        vertex.joint_indices,
        vertex.joint_weights
    );
    let origin = wow_light.rig_origin[wow_rig_slot(vertex.instance_index)].xyz;
    let world = (frame_from_local * vec4<f32>(vertex.position, 1.0)).xyz + origin;
#else
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0)
    ).xyz;
#endif
    out.position = view.clip_from_world * vec4<f32>(world, 1.0);
    if (!casts(vertex.instance_index)) {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    }
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
    return out;
}

// The alpha-key test, on an alpha-key batch or a fade twin of one (marker bit 10).
fn alpha_test(in: ShadowVsOut) {
#ifdef VERTEX_UVS_A
    // Bevy's alpha-mode bits, `STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK`.
    let alpha_mode = pbr_bindings::material.flags & (7u << 29u);
    let cutout = (u32(m.clutter_fade.z) & 1024u) != 0u || alpha_mode == (1u << 29u);
    if (cutout) {
        let a = textureSampleLevel(
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
            in.uv,
            0.0,
        ).a;
        if (a < VANILLA_ALPHA_KEY) {
            discard;
        }
    }
#endif
}

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
@fragment
fn fragment(in: ShadowVsOut) -> @builtin(frag_depth) f32 {
    alpha_test(in);
    return in.unclipped_depth;
}
#else
@fragment
fn fragment(in: ShadowVsOut) {
    alpha_test(in);
}
#endif
