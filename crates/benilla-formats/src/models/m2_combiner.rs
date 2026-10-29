//! A 3.3.5a M2 batch's pixel combiner, as the skin profile's load decides it (`0x837a40`): the
//! simple substitution (`0x836980`) turns each batch's shader word into its stages' ops, the
//! specialized substitution (`0x837680`) folds a base batch and its layer into one specialized
//! shader and parks the layer (`0x8000`, never drawn, `0x821e97`), and the effect lookup
//! (`0x836600`, `0x836c90`) names the shader.

use benilla_m2::{M2Model, SkinBatch};

use super::types::{CombinerOp, CombinerShader};

/// The shader word of a layer folded into its base: never drawn.
const PARKED: u16 = 0x8000;

/// One batch after the two passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BatchEffect {
    /// Folded into its base batch: not drawn.
    pub parked: bool,
    pub shader: CombinerShader,
    /// Stage 1's texture, a `textures` index, for a two-stage shader.
    pub texture2: Option<u16>,
    /// Stage 0 reads the second UV set (`Diffuse_T2`).
    pub stage0_t2: bool,
    /// Stage 1 reads the generated sphere map (`Diffuse_T1_Env`, `Diffuse_Env_Env`).
    pub stage1_env: bool,
}

/// The working copy of a batch the passes rewrite: its shader word, stage count, and per stage the
/// texture (a `textures` index) and coordinate source (`texture_unit_lookup`), `0xffff` for none.
#[derive(Clone, Copy)]
struct Work {
    material: u16,
    layer: u16,
    count: u16,
    shader: u16,
    tex: [u16; 2],
    coord: [u16; 2],
    weight: u16,
}

/// Every batch's effect, in skin order.
pub(super) fn batch_effects(model: &M2Model, batches: &[SkinBatch]) -> Vec<BatchEffect> {
    let lookup = |table: &[u16], i: usize| table.get(i).copied().unwrap_or(0xffff);
    let textures = &model.raw_data.texture_lookup_table;
    let coords = &model.texture_unit_lookup;
    let mut work: Vec<Work> = batches
        .iter()
        .map(|b| {
            let (t, c) = (
                b.texture_combo_index as usize,
                b.texture_coord_combo_index as usize,
            );
            let two = b.texture_count >= 2;
            Work {
                material: b.material_index,
                layer: b.material_layer,
                count: b.texture_count.min(2),
                shader: b.shader_id,
                tex: [
                    lookup(textures, t),
                    if two { lookup(textures, t + 1) } else { 0xffff },
                ],
                coord: [
                    lookup(coords, c),
                    if two { lookup(coords, c + 1) } else { 0xffff },
                ],
                weight: lookup(&model.transparency_lookup, b.weight_combo_index as usize),
            }
        })
        .collect();
    let blend = |w: &Work| {
        model
            .materials
            .get(w.material as usize)
            .map_or(0, |m| m.blend_mode.bits())
    };
    let unlit = |w: &Work| {
        model
            .materials
            .get(w.material as usize)
            .is_some_and(|m| m.flags.bits() & 1 != 0)
    };

    // The simple substitution: without global flag 0x8 the op is the blend's (opaque or
    // modulate); with it the shader word indexes the combiner ops, stage 0 opaque under blend 0.
    // A stage on generated coordinates (`> 2`) sets its op's bit 3.
    for w in &mut work {
        if w.shader & PARKED != 0 {
            continue;
        }
        let opaque = blend(w) == 0;
        if model.global_flags & 8 == 0 {
            let mut op = u16::from(!opaque);
            if w.coord[0] > 2 {
                op |= 8;
            }
            w.shader = op << 4;
        } else {
            let base = w.shader as usize;
            let mut ops = [0u16; 2];
            for (j, op) in ops.iter_mut().enumerate().take(w.count as usize) {
                let mut v = lookup(&model.texture_combiner_combos, base + j) & 7;
                if j == 0 && opaque {
                    v = 0;
                }
                *op = if w.coord[j] > 2 { v | 8 } else { v };
            }
            w.shader = ops[1] | (ops[0] << 4);
        }
    }

    specialize(&mut work, &blend, &unlit);

    work.iter()
        .map(|w| {
            if w.shader == PARKED {
                return BatchEffect {
                    parked: true,
                    shader: CombinerShader::One(CombinerOp::Mod),
                    texture2: None,
                    stage0_t2: false,
                    stage1_env: false,
                };
            }
            let layered = match w.shader {
                0x8001 => Some(CombinerShader::OpaqueMod2xNaAlpha),
                0x8002 => Some(CombinerShader::OpaqueAddAlpha),
                0x8003 => Some(CombinerShader::OpaqueAddAlphaAlpha),
                _ => None,
            };
            if let Some(shader) = layered {
                // `Diffuse_T1_Env`: stage 0 on the first UV set, stage 1 on the sphere map.
                return BatchEffect {
                    parked: false,
                    shader,
                    texture2: (w.tex[1] != 0xffff).then_some(w.tex[1]),
                    stage0_t2: false,
                    stage1_env: true,
                };
            }
            let op0 = (w.shader >> 4) & 7;
            if w.count <= 1 {
                return BatchEffect {
                    parked: false,
                    shader: CombinerShader::One(match op0 {
                        0 => CombinerOp::Opaque,
                        3 => CombinerOp::Add,
                        4 => CombinerOp::Mod2x,
                        // Decal (2) and Fade (5) are not built: they modulate.
                        _ => CombinerOp::Mod,
                    }),
                    texture2: None,
                    // `Diffuse_T2` for any non-zero coordinate source not on the sphere map.
                    stage0_t2: w.shader & 0x80 == 0 && w.coord[0] != 0,
                    stage1_env: false,
                };
            }
            // Two stages; a pair with no shader falls back to `0x11`, `Mod_Mod` on both UV sets
            // (`0x836dba`).
            let pair = two_stage(op0, w.shader & 7);
            BatchEffect {
                parked: false,
                shader: pair.unwrap_or(CombinerShader::Two(CombinerOp::Mod, CombinerOp::Mod)),
                texture2: (w.tex[1] != 0xffff).then_some(w.tex[1]),
                stage0_t2: false,
                stage1_env: pair.is_some() && w.shader & 8 != 0,
            }
        })
        .collect()
}

/// The two-stage pixel shader for ops `op0`, `op1` (`0x836600`), `None` where it names none.
fn two_stage(op0: u16, op1: u16) -> Option<CombinerShader> {
    use CombinerOp as Op;
    let second = |op1: u16| match op1 {
        0 => Op::Opaque,
        3 => Op::Add,
        4 => Op::Mod2x,
        6 => Op::Mod2xNa,
        7 => Op::AddNa,
        _ => Op::Mod,
    };
    Some(match (op0, op1) {
        (0, b) => CombinerShader::Two(Op::Opaque, second(b)),
        (1, b) => CombinerShader::Two(Op::Mod, second(b)),
        (3, 1) => CombinerShader::Two(Op::Add, Op::Mod),
        (4, 1) => CombinerShader::Two(Op::Mod, Op::Mod2x),
        (4, 4) => CombinerShader::Two(Op::Mod2x, Op::Mod2x),
        _ => return None,
    })
}

/// The specialized substitution (`0x837680`), on models with a layered batch: an opaque
/// two-stage base whose stage 1 is a sphere map (`Mod2x`/`Mod2xNA`), followed by a one-stage
/// alpha layer of the same texture and weight, becomes `0x8001`; an opaque one-stage base
/// followed by an env layer (blend 4 or 6) takes that layer as its stage 1 (`0x8002`, or `0xE`,
/// `Opaque_Mod2xNA` on the env), and a further alpha layer of its texture makes it `0x8003`
/// (from `0x8002`) or `0x8001`. Batches sharing the previous batch's material copy its result.
fn specialize(work: &mut [Work], blend: &impl Fn(&Work) -> u16, unlit: &impl Fn(&Work) -> bool) {
    if !work.iter().any(|w| w.layer != 0) {
        return;
    }
    let (mut state0, mut state1) = (0u8, 0u8);
    let mut base = 0usize;
    let mut material = u32::MAX;
    let mut shared = false;
    for i in 0..work.len() {
        if u32::from(work[i].material) == material {
            shared = true;
            continue;
        }
        material = u32::from(work[i].material);
        let op = work[i].shader & 7;
        let mode = blend(&work[i]);
        if work[i].layer == 0 {
            state0 = 0;
            state1 = 0;
            if work[i].count != 0 && mode == 0 {
                work[i].shader &= 0xff8f;
            }
            base = i;
        }
        let same_unlit = unlit(&work[i]) == unlit(&work[base]);
        let same_weight = work[i].weight == work[base].weight;
        let alpha_layer = (mode == 1 || mode == 2) && work[i].count == 1 && same_unlit;
        if state0 == 1 {
            if alpha_layer && work[i].tex[0] == work[base].tex[0] && same_weight {
                work[i].shader = PARKED;
                work[base].shader = 0x8001;
                state0 = 3;
                continue;
            }
            state0 = 0;
        }
        if state0 == 0
            && mode == 0
            && work[i].count == 2
            && (op == 4 || op == 6)
            && work[i].coord[0] == 0
            && work[i].coord[1] > 2
        {
            state0 = 1;
        }
        match state1 {
            0 => {}
            1 => {
                if (mode == 4 || mode == 6)
                    && work[i].count == 1
                    && work[i].coord[0] > 2
                    && same_weight
                {
                    work[i].shader = PARKED;
                    let layer_tex = work[i].tex[0];
                    let b = &mut work[base];
                    b.shader = if mode != 4 { 0xe } else { 0x8002 };
                    b.count = 2;
                    b.tex[1] = layer_tex;
                    state1 = 2;
                    continue;
                }
                state1 = 0;
            }
            2 => {
                if alpha_layer && work[i].tex[0] == work[base].tex[0] && same_weight {
                    work[i].shader = PARKED;
                    work[base].shader = if work[base].shader == 0x8002 {
                        0x8003
                    } else {
                        0x8001
                    };
                    state1 = 3;
                    continue;
                }
                state1 = 0;
            }
            _ => continue,
        }
        if mode == 0 && work[i].count == 1 && work[i].coord[0] == 0 {
            state1 = 1;
        }
    }
    if shared {
        let mut material = u32::MAX;
        for i in 0..work.len() {
            if u32::from(work[i].material) == material {
                let prev = work[i - 1];
                let w = &mut work[i];
                w.shader = prev.shader;
                w.count = prev.count;
                w.tex = prev.tex;
            } else {
                material = u32::from(work[i].material);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Northrend login scene authors its cloud layers as `Mod_Mod2x` over a mask on the second
    /// UV set, and its frost wyrm's bone sections as an opaque spec base with a sphere-map layer
    /// and an alpha layer of the same texture, which fold into `Opaque_Mod2xNA_Alpha`.
    #[test]
    fn the_northrend_login_scene_masks_its_clouds_and_folds_the_wyrm_layers() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(
            &chain,
            r"Interface\Glues\Models\UI_MainMenu_Northrend\UI_MainMenu_Northrend.m2",
        )
        .expect("the login scene");
        let model = &bytes;
        assert_ne!(model.global_flags & 8, 0, "the scene uses combiner combos");
        assert!(!model.texture_combiner_combos.is_empty());
        let skin = &model.skin;
        let fx = batch_effects(model, skin.batches());

        let masked = fx
            .iter()
            .filter(|e| {
                e.shader == CombinerShader::Two(CombinerOp::Mod, CombinerOp::Mod2x)
                    && !e.stage1_env
                    && e.texture2.is_some()
            })
            .count();
        assert!(masked >= 4, "{masked} masked Mod_Mod2x layers");
        let folded = fx
            .iter()
            .filter(|e| e.shader == CombinerShader::OpaqueMod2xNaAlpha && e.stage1_env)
            .count();
        let parked = fx.iter().filter(|e| e.parked).count();
        assert!(folded > 0, "no wyrm spec base folded");
        assert_eq!(parked, folded, "each folded base parks its one layer");
    }
}
