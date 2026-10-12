//! 3.3.5a M2 ribbon emitters (weapon trails, wisp streamers, missile trails), read from the
//! version-264 record (header +0x120/+0x124, stride 0xb0; loader `0x833702`), its tracks onto the
//! relayout's sequence bands ([`benilla_m2::Tracks264`]):
//!
//! ```text
//! +0x04 boneIndex(u16)  +0x08 position(C3Vector, bone-local)
//! +0x14 textureIndices(M2Array<u16> → M2 textures)  +0x1c materialIndices(M2Array<u16> → the
//!       M2 render-flags table @MD20+0x70, stride 4: flags u16, blend u16)
//! +0x24 colorTrack(M2Track<C3Vector>)  +0x38 alphaTrack(M2Track<fixed16>)
//! +0x4c heightAboveTrack(M2Track<f32>)  +0x60 heightBelowTrack(M2Track<f32>)
//! +0x74 edgesPerSecond(f32)  +0x78 edgeLifetime(f32, clamped ≥ 0.25)  +0x7c gravity(f32)
//! +0x80 textureRows(u16)  +0x82 textureCols(u16)
//! +0x84 texSlotTrack(M2Track<u16>)  +0x98 visibilityTrack(M2Track<u8>)
//! ```
//!
//! Color, alpha and the two heights are keyed tracks rebased onto the first sequence's band;
//! `texSlot` is its first value. The visibility track sets the enable byte every frame, and a
//! cleared byte skips the ribbon's whole draw. It keys inside a sequence: `G_FrostTrap.m2` lights
//! its upper streamers 200 ms into `Custom0`.
//!
//! The reference samples a single-key track once and latches it, where this resamples every frame
//! (the same for a constant track), and windows keys by the emitter bone's active sequence, where
//! this uses the model's (the same wherever the bone plays the model's sequence).

use anyhow::Result;
use benilla_m2::{md20_version, AnimFiles, M2Track, Tracks264, VERSION_264};

use crate::particles::{texture_paths_264, HDR_RIBBONS};
use crate::value_track::{rebase_keys_to_band, TrackValue, ValueTrack};
use crate::ParticleBlend;

const STRIDE: usize = 0xb0;
/// The M2 render-flags (materials) table: `{flags u16, blend u16}` per entry.
const HDR_RENDER_FLAGS: usize = 0x70;
/// The sequences table, `0x40`-byte records with the animation id at +0.
const HDR_SEQUENCES: usize = 0x1c;

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// A timeline track as a [`ValueTrack`]: sequence keys rebased to `band`, global-sequence ones on
/// their clock, and an empty track `default`.
fn value_track<V: TrackValue>(t: M2Track<V>, default: V, band: (u32, u32)) -> ValueTrack<V> {
    if t.keys.is_empty() {
        return ValueTrack::constant(default);
    }
    let mut keys = t.keys;
    if t.gseq == 0xffff {
        rebase_keys_to_band(&mut keys, band.0, band.1);
    }
    ValueTrack {
        keys,
        interp: t.interp,
    }
}

/// One M2 ribbon emitter, as the renderer needs it.
#[derive(Debug, Clone)]
pub struct RibbonEmitterDef {
    pub bone: u16,
    /// The origin in the host bone's frame, times the bone matrix every frame (`0x718960`).
    pub position: [f32; 3],
    /// `textureIndices[0]`'s path; `None` if unresolved, and the ribbon is skipped.
    pub texture: Option<String>,
    /// `materialIndices[0]`'s render-flags blend, folded into a `ParticleBlend`.
    pub blend: ParticleBlend,
    /// The raw blend behind the lossy [`Self::blend`] fold, when the material resolves.
    pub blend_mode: Option<u16>,
    /// Trail tint (colorTrack, RGB 0..1, keyed on the clip clock; constant white when unkeyed).
    pub color: ValueTrack<[f32; 3]>,
    /// Trail opacity (alphaTrack, int16 / 32767; constant 1.0 when unkeyed).
    pub alpha: ValueTrack,
    /// Half-widths (yards) above and below the path; each edge keeps the width it was born with.
    pub height_above: ValueTrack,
    pub height_below: ValueTrack,
    /// Edges per second; with `edge_lifetime`, sizes the ring.
    pub edges_per_second: f32,
    /// Edge lifetime in seconds, clamped to at least 0.25 at load as the reference does.
    pub edge_lifetime: f32,
    /// Downward sag applied to live edges (the reference's `2·g·dt` per-frame term).
    pub gravity: f32,
    /// Texture atlas tiling (1×1 is the whole texture) and the slot index.
    pub tile_rows: u16,
    pub tile_cols: u16,
    pub tex_slot: u16,
    /// The `+0x98` enable track, `None` when always on; otherwise [`RibbonVisibility::at`] is
    /// sampled on the host's playing sequence every frame.
    pub visible: Option<RibbonVisibility>,
}

/// The `+0x98` enable track per `AnimationData.dbc` id: step keys in band-local seconds, the
/// first being the value the band opens on.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonVisibility {
    by_anim: std::collections::HashMap<u16, Vec<(f32, bool)>>,
}

impl RibbonVisibility {
    /// Whether the trail emits `t` seconds into `anim`, stepped. An unauthored sequence takes
    /// Stand's answer, and without one the load default, on.
    pub fn at(&self, anim: u16, t: f32) -> bool {
        let Some(keys) = self.by_anim.get(&anim).or_else(|| self.by_anim.get(&0)) else {
            return true;
        };
        keys.iter()
            .take_while(|&&(kt, _)| kt <= t)
            .last()
            .or(keys.first())
            .is_some_and(|&(_, on)| on)
    }

    /// Every `(anim id, keys)` pair, for instruments that census the gate.
    pub fn per_anim(&self) -> impl Iterator<Item = (u16, &[(f32, bool)])> {
        self.by_anim.iter().map(|(&a, k)| (a, k.as_slice()))
    }
}

/// The `+0x98` gate track as a [`RibbonVisibility`]; `None` when keyless, on a global-sequence
/// clock, or on everywhere. `sequences` is each sequence's animation id and timeline band.
fn visibility_by_anim(
    vis: &M2Track<f32>,
    sequences: &[(u16, (u32, u32))],
) -> Option<RibbonVisibility> {
    // A global-sequence clock is not per animation: treated as always on.
    if vis.gseq != 0xffff || vis.keys.is_empty() {
        return None;
    }
    let keys: Vec<(u32, bool)> = vis.keys.iter().map(|&(t, v)| (t, v != 0.0)).collect();
    let mut by_anim: std::collections::HashMap<u16, Vec<(f32, bool)>> =
        std::collections::HashMap::new();
    let mut any_off = false;
    for &(anim, (start, end)) in sequences {
        // The band opens on the nearest-previous key, or the first key before any.
        let opening = keys
            .iter()
            .take_while(|&&(t, _)| t <= start)
            .last()
            .or(keys.first())
            .is_some_and(|&(_, v)| v);
        let mut band: Vec<(f32, bool)> = vec![(0.0, opening)];
        // Then each change strictly inside the band, band-local.
        for &(t, v) in keys.iter().filter(|&&(t, _)| t > start && t <= end) {
            let local = (t - start) as f32 / 1000.0;
            if band.last().is_some_and(|&(_, prev)| prev != v) {
                band.push((local, v));
            }
        }
        any_off |= band.iter().any(|&(_, v)| !v);
        by_anim.entry(anim).or_insert(band); // variations share an id; the first wins
    }
    // Never dark: no gate.
    any_off.then_some(RibbonVisibility { by_anim })
}

/// A version-264 M2's ribbon emitters; empty when it has none or is not version 264. A sequence
/// keyed in a `.anim` file has no keys here ([`parse_m2_ribbon_emitters_with_anims`] reads them).
pub fn parse_m2_ribbon_emitters(bytes: &[u8]) -> Result<Vec<RibbonEmitterDef>> {
    parse_m2_ribbon_emitters_with_anims(bytes, &AnimFiles::new())
}

/// [`parse_m2_ribbon_emitters`], the tracks of a sequence keyed in a `.anim` read from `anims`
/// ([`benilla_m2::anim_paths`]).
pub fn parse_m2_ribbon_emitters_with_anims(
    bytes: &[u8],
    anims: &AnimFiles,
) -> Result<Vec<RibbonEmitterDef>> {
    if md20_version(bytes) != Some(VERSION_264) || bytes.len() < HDR_RIBBONS + 8 {
        return Ok(Vec::new());
    }
    let count = le_u32(bytes, HDR_RIBBONS) as usize;
    let base = le_u32(bytes, HDR_RIBBONS + 4) as usize;
    // `count` sizes the reservation, so a hostile one is refused; 256 is far past any shipped
    // model, and checking it first keeps `count * STRIDE` from overflowing.
    if count == 0 || count > 256 || base + count * STRIDE > bytes.len() {
        return Ok(Vec::new());
    }
    let textures = texture_paths_264(bytes);
    let mut tracks = Tracks264::new(bytes, anims)?;
    let rf_count = le_u32(bytes, HDR_RENDER_FLAGS) as usize;
    let rf_base = le_u32(bytes, HDR_RENDER_FLAGS + 4) as usize;
    let sequences: Vec<(u16, (u32, u32))> = {
        let ofs = le_u32(bytes, HDR_SEQUENCES + 4) as usize;
        tracks
            .sequences()
            .into_iter()
            .enumerate()
            .map(|(i, (band, _))| {
                let at = ofs + i * 0x40;
                let anim = if at + 2 <= bytes.len() {
                    le_u16(bytes, at)
                } else {
                    0
                };
                (anim, band)
            })
            .collect()
    };
    // The first sequence's band, which the keyed look tracks rebase onto, as particle tracks do.
    let band = sequences.first().map_or((0, u32::MAX), |&(_, b)| b);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = base + i * STRIDE;
        // textureIndices[0] → the M2 textures table → path.
        let texture = {
            let n = le_u32(bytes, e + 0x14);
            let ofs = le_u32(bytes, e + 0x18) as usize;
            (n > 0 && ofs + 2 <= bytes.len())
                .then(|| le_u16(bytes, ofs) as usize)
                .and_then(|ti| textures.get(ti).cloned().flatten())
        };
        // materialIndices[0] → render-flags entry → blend u16.
        let blend_mode = {
            let n = le_u32(bytes, e + 0x1c);
            let ofs = le_u32(bytes, e + 0x20) as usize;
            (n > 0 && ofs + 2 <= bytes.len())
                .then(|| le_u16(bytes, ofs) as usize)
                .filter(|&m| m < rf_count && rf_base + m * 4 + 4 <= bytes.len())
                .map(|m| le_u16(bytes, rf_base + m * 4 + 2))
        };
        // `ParticleBlend` has no Mod2x, so a Mod2x trail folds to `Opaque` and draws unblended,
        // where the reference multiplies.
        let blend = match blend_mode {
            Some(3 | 4) => ParticleBlend::Add,
            Some(2) => ParticleBlend::Alpha,
            Some(1) => ParticleBlend::AlphaKey,
            Some(_) => ParticleBlend::Opaque,
            None => ParticleBlend::Add, // unresolved material: trails are near-always additive
        };
        let color = tracks.vec3(e + 0x24, [1.0; 3]).unwrap_or_default();
        // fix16 keys are signed; the draw clamps a negative to 0.
        let alpha = tracks.fixed16(e + 0x38, 0x7fff).unwrap_or_default();
        let above = tracks.float(e + 0x4c, 0.0).unwrap_or_default();
        let below = tracks.float(e + 0x60, 0.0).unwrap_or_default();
        let slot = tracks.short(e + 0x84, 0).unwrap_or_default();
        let vis = tracks.byte(e + 0x98, 1).unwrap_or_default();
        out.push(RibbonEmitterDef {
            bone: le_u16(bytes, e + 0x04),
            position: [
                le_f32(bytes, e + 0x08),
                le_f32(bytes, e + 0x0c),
                le_f32(bytes, e + 0x10),
            ],
            texture,
            blend,
            blend_mode,
            color: value_track(color, [1.0; 3], band),
            alpha: value_track(alpha, 1.0, band),
            height_above: value_track(above, 0.0, band),
            height_below: value_track(below, 0.0, band),
            edges_per_second: le_f32(bytes, e + 0x74),
            edge_lifetime: le_f32(bytes, e + 0x78).max(0.25),
            gravity: le_f32(bytes, e + 0x7c),
            tile_rows: le_u16(bytes, e + 0x80).max(1),
            tile_cols: le_u16(bytes, e + 0x82).max(1),
            tex_slot: slot.keys.first().map_or(0, |&(_, v)| v as u16),
            visible: visibility_by_anim(&vis, &sequences),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostile_ribbon_count_yields_nothing_not_an_abort() {
        let (count, ptr) = (HDR_RIBBONS, HDR_RIBBONS + 4);
        let mut b = vec![0u8; ptr + 4];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&264u32.to_le_bytes());
        b[count..count + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).unwrap().is_empty());
        // A plausible count whose table the file cannot hold is refused too.
        b[count..count + 4].copy_from_slice(&3u32.to_le_bytes());
        b[ptr..ptr + 4].copy_from_slice(&((ptr + 4) as u32).to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).unwrap().is_empty());
    }

    #[test]
    fn visibility_samples_step_and_falls_back_to_stand() {
        let vis = RibbonVisibility {
            by_anim: [
                (0u16, vec![(0.0, false)]),
                (153u16, vec![(0.0, false), (0.2, true), (1.4, false)]),
            ]
            .into_iter()
            .collect(),
        };
        assert!(!vis.at(153, 0.0));
        assert!(!vis.at(153, 0.199));
        assert!(vis.at(153, 0.2), "step: the key's value takes effect AT it");
        assert!(vis.at(153, 1.399));
        assert!(!vis.at(153, 1.4));
        assert!(!vis.at(147, 0.0), "unlisted sequence borrows Stand");
        let no_stand = RibbonVisibility {
            by_anim: [(153u16, vec![(0.0, false)])].into_iter().collect(),
        };
        assert!(no_stand.at(147, 0.0), "no answer at all ⇒ the load default");
    }

    /// Smite's impact slash is two ribbons keyed inside the band [3300, 4433]: height 0 to 0.167
    /// and back over 267 ms, alpha 1 to 0 by 467 ms.
    #[test]
    fn real_holy_smite_slash_ribbons_are_keyed_flares() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Spells\\HolySmite_Low_Chest.m2")
            .expect("read HolySmite_Low_Chest.m2");
        let defs = parse_m2_ribbon_emitters(&bytes).expect("parse ribbons");
        assert_eq!(defs.len(), 2);
        for r in &defs {
            assert_eq!(r.height_above.first(), 0.0, "value[0] bake = no slash");
            assert!((r.height_above.peak() - 0.167).abs() < 1e-3);
            assert!((r.height_above.sample_ms(200.0) - 0.167).abs() < 1e-3);
            assert_eq!(r.height_above.sample_ms(400.0), 0.0, "collapsed by 267 ms");
            assert_eq!(r.height_below.keys, r.height_above.keys, "symmetric slash");
            assert_eq!(r.alpha.sample_ms(0.0), 1.0);
            assert!(r.alpha.sample_ms(600.0) < 1e-6);
            // Rebased: every key is inside the 1133 ms clip.
            assert!(r.height_above.keys.iter().all(|&(t, _)| t <= 1133));
            assert_eq!(r.color.first(), [1.0; 3]);
            // Always on, its alpha doing the fade: no gate to darken it against Stand.
            assert_eq!(
                r.visible, None,
                "an always-on slash carries no visibility gate"
            );
        }
    }

    #[test]
    fn real_thrown_dagger_trail_is_lit_only_in_flight() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Item\\ObjectComponents\\Weapon\\Thrown_1H_Dagger_A_01.m2")
            .expect("read Thrown_1H_Dagger_A_01.m2");
        let defs = parse_m2_ribbon_emitters(&bytes).expect("parse ribbons");
        assert_eq!(defs.len(), 1, "the dagger authors one trail ribbon");
        let vis = defs[0]
            .visible
            .as_ref()
            .expect("the thrown dagger's trail IS visibility-gated");
        assert!(!vis.at(0, 0.0), "Stand (worn in hand): dark");
        assert!(vis.at(144, 0.0), "InFlight (flying): lit");
        assert!(!vis.at(191, 0.0), "Impact (landed): dark");
    }

    /// `G_FrostTrap.m2`: four low ribbons light 534 ms into Spawn and stay lit through Closed;
    /// twelve upper ones are dark at rest and light 200 ms into `Custom0`, when the trap springs.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn real_frost_trap_upper_streamers_light_only_inside_the_trigger() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Goober\\G_FrostTrap.m2")
            .expect("read G_FrostTrap.m2");
        let defs = parse_m2_ribbon_emitters(&bytes).expect("parse ribbons");
        assert_eq!(defs.len(), 16, "the frost trap authors sixteen trails");

        // Ribbons 0..4: key 867 inside Spawn's 333..1067 band.
        for r in &defs[0..4] {
            let v = r.visible.as_ref().expect("the low swirl IS gated");
            assert!(!v.at(145, 0.0), "Spawn opens dark");
            assert!(v.at(145, 0.6), "…and lights 534 ms in");
            assert!(v.at(147, 0.0), "Closed: lit — the placed trap's swirl");
        }
        // Ribbons 4..16: key 4200 inside the trigger's 4000..5400 band.
        for r in &defs[4..16] {
            let v = r.visible.as_ref().expect("the upper streamers ARE gated");
            assert!(!v.at(147, 0.0), "Closed: dark — no column on a placed trap");
            assert!(!v.at(148, 0.0), "Open: dark");
            assert!(!v.at(153, 0.0), "the trigger OPENS dark");
            assert!(v.at(153, 0.5), "…and lights 200 ms in");
            assert!(!v.at(153, 1.45), "…then goes dark again at the band end");
        }
    }
}
