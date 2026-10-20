//! Dump an M2's attachments and event markers with their bone's parent:
//! `cargo run -p benilla-m2 --example dump_attach <file.m2>`, extracted first with `mpqx`
//! (benilla-formats).

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dump_attach <file.m2>");
    let data = std::fs::read(&path).expect("read m2");
    let skin = std::fs::read(benilla_m2::skin_path(&path)).expect("read its 00.skin beside it");
    let model = benilla_m2::parse_m2(data, &skin, Default::default()).expect("parse m2");
    println!("attachments: {}", model.attachments.len());
    for a in &model.attachments {
        let bone = model.bones.get(a.bone as usize);
        let parent = bone.map_or(-1, |b| b.parent);
        println!(
            "id {:2} bone {:3} (parent {:3}) pos ({:+.3},{:+.3},{:+.3})",
            a.id, a.bone, parent, a.position[0], a.position[1], a.position[2]
        );
    }
    println!("event markers: {}", model.event_markers.len());
    for m in &model.event_markers {
        let bone = model.bones.get(m.bone as usize);
        let parent = bone.map_or(-1, |b| b.parent);
        println!(
            "{} bone {:3} (parent {:3}) pos ({:+.3},{:+.3},{:+.3})",
            String::from_utf8_lossy(&m.ident),
            m.bone,
            parent,
            m.position[0],
            m.position[1],
            m.position[2]
        );
    }
}
