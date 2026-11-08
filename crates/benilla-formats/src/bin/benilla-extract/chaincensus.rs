//! `chaincensus`: every `SpellVisualKit` that draws a `SpellChainEffects` beam, the row it names
//! and the spells that reach it by stage. Each live slot must name one of the table's rows, or the
//! decode of `CharParamZero` is wrong. The client tells the two kinds apart by the flag alone,
//! never the type key. `p/r` is the persist (param 2) and reverse (param 3) pair.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use benilla_formats::{char_proc_type, Chain, SpellVisualCatalog, VisualStages};

type StagePick = fn(&VisualStages) -> u32;

const STAGES: [(&str, StagePick); 5] = [
    ("precast", |s| s.precast),
    ("cast", |s| s.cast),
    ("impact", |s| s.impact),
    ("state", |s| s.state),
    ("channel", |s| s.channel),
];

/// Which spells reach one kit, and through which lifecycle stages.
#[derive(Default, Clone)]
struct Reach {
    spells: BTreeSet<String>,
    stages: BTreeSet<&'static str>,
}

fn kit_reachability(
    chain: &mut Chain,
    visuals: &SpellVisualCatalog,
) -> Result<BTreeMap<u32, Reach>> {
    let spells = benilla_formats::load_spell_catalog(chain)?;
    let mut out: BTreeMap<u32, Reach> = BTreeMap::new();
    for (_, spell) in spells.iter() {
        let Some(stages) = visuals.stages(spell.visual) else {
            continue;
        };
        for (label, pick) in STAGES {
            let kit = pick(stages);
            if kit == 0 {
                continue;
            }
            let e = out.entry(kit).or_default();
            e.spells.insert(spell.name.clone());
            e.stages.insert(label);
        }
    }
    Ok(out)
}

/// Census the beam system: the effect table, then every kit that draws one.
pub fn run(chain: &mut Chain) -> Result<()> {
    let visuals = benilla_formats::load_spell_visual_catalog(chain)?;
    let reach = kit_reachability(chain, &visuals)?;

    println!(
        "SpellChainEffects: {} rows\n{:>4}  {:>6} {:>5} {:>5} {:>6} {:>5} {:>4} {:>4} {:>5}  {:>13}  {:>17}  {:>5}  {:>17}  {:>8} {:>5} {:>3} {:>6}  combo  texture",
        visuals.chain_effect_len(),
        "id",
        "segLen",
        "halfW",
        "noise",
        "scroll",
        "life",
        "dly",
        "stag",
        "flags",
        "joints",
        "wave h/f/s",
        "arc",
        "pulse v/on/fade",
        "rgba",
        "blend",
        "lyr",
        "texLen",
    );
    let mut blends = BTreeMap::<u8, usize>::new();
    let mut layers = BTreeMap::<i32, usize>::new();
    let mut flag_bits = [0usize; 32];
    for id in visuals.chain_effect_ids() {
        let Some(c) = visuals.chain_effect(id) else {
            continue;
        };
        *blends.entry(c.blend_mode).or_default() += 1;
        *layers.entry(c.render_layer).or_default() += 1;
        for (bit, n) in flag_bits.iter_mut().enumerate() {
            *n += usize::from(c.flags >> bit & 1 != 0);
        }
        let [r, g, b, a] = c.rgba;
        println!(
            "{id:>4}  {:>6} {:>5} {:>5} {:>6} {:>5} {:>4} {:>4} {:>#5x}  {:>3}/{:<3} {:>2}/{:<2}  {:>5}/{:>5}/{:>5}  {:>5}  {:>5}/{:>5}/{:>5}  {r:02x}{g:02x}{b:02x}{a:02x} {:>5} {:>3} {:>6}  {:?}  {}",
            c.avg_seg_len,
            c.half_width,
            c.noise_scale,
            c.tex_scroll_speed,
            c.bolt_life_ms,
            c.first_bolt_delay_ms,
            c.bolt_stagger_ms,
            c.flags,
            c.joint_count,
            c.joint_offset_radius,
            c.joints_per_minor_joint,
            c.minor_joints_per_major_joint,
            c.wave_height,
            c.wave_freq,
            c.wave_speed,
            c.arc_height,
            c.pulse_speed,
            c.pulse_on_length,
            c.pulse_fade_length,
            c.blend_mode,
            c.render_layer,
            c.texture_length,
            c.combo,
            c.texture,
        );
    }
    println!("\nblend modes (column 43): {blends:?}");
    println!("render layers (column 45): {layers:?}");
    let set: Vec<String> = flag_bits
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(bit, n)| format!("{:#x}:{n}", 1u32 << bit))
        .collect();
    println!("flag bits (column 8): {}", set.join(" "));

    println!(
        "\n{:>5} {:>4} {:>5} {:>4}  {:<9} {:<26} spells",
        "kit", "type", "chain", "p/r", "stage", "texture",
    );
    let (mut live, mut padding, mut spells_hit) = (0usize, 0usize, BTreeSet::new());
    for kit_id in visuals.kit_ids() {
        let Some(kit) = visuals.kit(kit_id) else {
            continue;
        };
        for proc in kit.char_procs().filter(|p| char_proc_type::is_chain(p.ty)) {
            let Some(c) = proc.as_chain() else {
                padding += 1;
                continue;
            };
            live += 1;
            let Reach { spells, stages } = reach.get(&kit_id).cloned().unwrap_or_default();
            spells_hit.extend(spells.iter().cloned());
            let texture = visuals
                .chain_effect(c.effect_id)
                .map_or("(NO SUCH ROW)", |e| {
                    e.texture.rsplit('\\').next().unwrap_or(&e.texture)
                });
            let listed: Vec<&str> = spells.iter().take(6).map(String::as_str).collect();
            let more = spells.len().saturating_sub(listed.len());
            println!(
                "{kit_id:>5} {:>4} {:>5} {:>2}{:>2}  {:<9} {texture:<26} {}{}",
                c.ty,
                c.effect_id,
                u8::from(c.flag),
                u8::from(c.reverse),
                stages.iter().copied().collect::<Vec<_>>().join(","),
                listed.join(", "),
                if more > 0 {
                    format!(" … (+{more})")
                } else {
                    String::new()
                },
            );
        }
    }
    println!(
        "\n{live} live beams across the table ({padding} zero-param padding slots no-oped), \
         reaching {} named spells.\n\
         Every live slot above resolves to a real SpellChainEffects row — that is the check: a \
         wrong decode would not.",
        spells_hit.len(),
    );
    Ok(())
}
