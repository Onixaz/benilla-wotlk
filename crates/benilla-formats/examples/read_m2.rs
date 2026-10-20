//! An M2 through [`read_m2`](benilla_formats::read_m2) and its readers: what a model keeps, and
//! what its timeline drops. Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example read_m2 -- <m2 path>`

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: read_m2 <m2 path>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let chain = benilla_formats::open_chain(&data)?;
    let version = benilla_formats::md20_version(&chain.read(&path)?);
    let bytes = benilla_formats::read_m2(&chain, &path)?;
    println!("{path}: MD20 {version:?}, {} bytes", bytes.bytes().len());
    for d in &bytes.dropped {
        println!("  dropped: {d}");
    }

    let subs = benilla_formats::parse_m2_render_submeshes(&bytes, "", &[])?;
    let tris: usize = subs.iter().map(|s| s.indices.len() / 3).sum();
    println!("  {} batches, {tris} triangles", subs.len());
    for s in subs.iter().take(8) {
        println!(
            "    {:?} {:?} {} verts",
            s.texture.as_deref().unwrap_or("-"),
            s.blend,
            s.positions.len()
        );
    }
    let skeleton = benilla_formats::parse_m2_skeleton(&bytes);
    println!("  {} bones", skeleton.bones.len());
    for a in benilla_formats::parse_m2_animations(&bytes) {
        println!(
            "  sequence {} (anim {}): {}..{} ms, {} posed bones, loops {}",
            a.seq_index,
            a.anim_id,
            a.start_ms,
            a.end_ms,
            a.bones.len(),
            a.looping
        );
    }
    let gseq = benilla_formats::parse_m2_global_sequence_bones(&bytes);
    println!("  {} bones on global sequences", gseq.len());
    for (i, l) in benilla_formats::parse_m2_lights(&bytes).iter().enumerate() {
        println!(
            "  light {i}: type {} bone {} ambient {:?}×{} diffuse {:?}×{} atten {}..{}",
            l.light_type,
            l.bone,
            l.ambient_color,
            l.ambient_intensity,
            l.diffuse_color,
            l.diffuse_intensity,
            l.attenuation_start,
            l.attenuation_end
        );
    }
    match benilla_formats::parse_m2_camera(&bytes, 0) {
        Some(c) => println!(
            "  camera 0: eye {:?} target {:?} fov {} near {} far {}",
            c.position, c.target, c.fov, c.near_clip, c.far_clip
        ),
        None => println!("  no camera 0"),
    }
    let bounds = benilla_formats::parse_m2_bounds(&bytes);
    println!(
        "  bounds {:?}..{:?} r {}",
        bounds.bbox_min, bounds.bbox_max, bounds.sphere_radius
    );
    Ok(())
}
