//! The 3.3.5a MOHD light flags on the shipped art: `0x2` (the unified drawer `0x7a9380`) and `0x8`
//! (no MOCV fix-up `0x7d7380`). Wyrmrest Temple's root sets both, with `0x1` (no doorway fade).

use benilla_formats::{
    parse_wmo_root, wmo_group_fixed_colors, wmo_group_footprint_tris, wmo_group_raw_colors,
    wmo_group_submeshes, Chain,
};

const WYRMREST: &str = "World\\wmo\\Northrend\\Buildings\\Wyrmrest";

/// Wyrmrest's groups draw their authored MOCV untouched, every batch carries the MOHD ambient, and
/// a unit standing in an interior group samples `2·MOCV + ambient`.
#[test]
fn wyrmrest_lights_by_the_unified_law() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open patch chain");
    let root = parse_wmo_root(&reader.read(&format!("{WYRMREST}.wmo")).expect("read root"))
        .expect("parse root");
    assert_eq!(root.flags(), 0xf);
    let law = root.light_law();
    assert_eq!(
        law.unified,
        Some([94, 112, 119]),
        "MOHD ambColor BGRA (119,112,94)"
    );
    assert!(!law.mocv_fixup && !law.doorway_fade);

    for gi in 0..root.group_count() {
        let gbytes = reader
            .read(&format!("{WYRMREST}_{gi:03}.wmo"))
            .expect("read group");
        for sub in wmo_group_submeshes(&gbytes, &root).expect("group submeshes") {
            assert_eq!(sub.wmo_unified, law.unified, "group {gi:03}");
        }
        // Neither the fix-up nor the fade touches a byte.
        if let Some(raw) = wmo_group_raw_colors(&gbytes) {
            let drawn = wmo_group_fixed_colors(&gbytes, &root).expect("MOCV");
            assert_eq!(raw, drawn, "group {gi:03}");
        }
    }

    // Group 2 is interior (MOGP 0x2805), so it has a footprint.
    let g2 = reader
        .read(&format!("{WYRMREST}_002.wmo"))
        .expect("read group 2");
    let fp = wmo_group_footprint_tris(&g2, &root).expect("interior group with MOCV");
    assert_eq!((fp.sample_scale, fp.sample_ambient), (2, [94, 112, 119]));
    // The near-black floor bake becomes the ambient's light, never darker.
    assert_eq!(fp.sample_light([0, 0, 0]), [94, 112, 119]);
}

/// MOHD `0x8` never appears without `0x2`, so only the unified law reads it. Both are Wrath-era
/// art's: 162 of the 1985 roots, Northrend's and rebuilt sites elsewhere, `Stormwind.wmo` among
/// them.
#[test]
fn mohd_0x8_rides_only_with_0x2() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open patch chain");
    let (mut roots, mut unified) = (0, 0);
    for entry in reader.list().expect("list") {
        let lower = entry.name.to_ascii_lowercase();
        let Some(stem) = lower.strip_suffix(".wmo") else {
            continue;
        };
        if stem
            .rsplit('_')
            .next()
            .is_some_and(|t| t.len() == 3 && t.bytes().all(|b| b.is_ascii_digit()))
        {
            continue; // a group file
        }
        let Ok(Ok(root)) = reader.read(&entry.name).map(|b| parse_wmo_root(&b)) else {
            continue;
        };
        roots += 1;
        let flags = root.flags();
        if flags & 0x8 != 0 {
            assert_ne!(flags & 0x2, 0, "{}: MOHD {flags:#x}", entry.name);
        }
        if flags & 0x2 != 0 {
            unified += 1;
        }
    }
    assert_eq!((unified, roots), (162, 1985));
    let stormwind = "World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind.wmo";
    let root = parse_wmo_root(&reader.read(stormwind).expect("read Stormwind")).expect("parse");
    assert_eq!(root.flags(), 0xf, "Stormwind.wmo lights by the unified law");
}
