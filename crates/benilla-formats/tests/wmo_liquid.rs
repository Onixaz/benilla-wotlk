//! WMO MLIQ liquid surfaces: an `xverts × yverts` grid with one tile fewer each way, and a per-tile
//! type nibble where `0xf` is a hole and 4 is lake_a.

use benilla_formats::{
    parse_wmo_root, wmo_group_has_liquid_grid, wmo_group_header, wmo_group_liquid_mesh, Chain,
    LiquidKind, WmoLiquidRule, NO_GROUP_LIQUID,
};

/// The target client's liquid rule for a group file, read off its root.
fn rule(chain: &Chain, group_path: &str) -> WmoLiquidRule {
    WmoLiquidRule::for_group_path(chain, group_path)
        .unwrap_or_else(|| panic!("{group_path}: root unreadable"))
}

#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn stormwind_canal_group_builds_still_water() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");

    // A canal segment: 12×9 vertices, 11×8 tiles, 52 of them wet and 36 holes.
    let g099 = reader
        .read("World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo")
        .expect("read Stormwind_099.wmo");
    let mesh = wmo_group_liquid_mesh(
        &g099,
        rule(
            &reader,
            r"World\wmo\Azeroth\Buildings\Stormwind\Stormwind_099.wmo",
        ),
    )
    .expect("group 099 carries water");
    assert_eq!(mesh.kind, LiquidKind::Still, "canal water is lake_a/still");
    assert_eq!(mesh.positions.len(), 12 * 9, "full 12×9 grid emitted");
    assert_eq!(mesh.uvs.len(), 12 * 9);
    assert_eq!(mesh.depths.len(), 12 * 9);
    assert_eq!(
        mesh.indices.len(),
        52 * 6,
        "52 wet tiles → 2 tris each; the 36 hole tiles are skipped"
    );
    assert!(mesh
        .indices
        .iter()
        .all(|&i| (i as usize) < mesh.positions.len()));
    for p in &mesh.positions {
        assert!(
            p[2].is_finite() && p[2].abs() < 10_000.0,
            "sane height {p:?}"
        );
    }

    let g000 = reader
        .read("World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_000.wmo")
        .expect("read Stormwind_000.wmo");
    assert!(
        wmo_group_liquid_mesh(
            &g000,
            rule(
                &reader,
                r"World\wmo\Azeroth\Buildings\Stormwind\Stormwind_000.wmo"
            ),
        )
        .is_none(),
        "a masonry group carries no liquid"
    );
}

/// MLIQ carries a height for every vertex, wet or not, and the shipped files leave hole interiors
/// at `0.0`; the builder gives every undrawn vertex the lowest drawn height, so the mesh bounds
/// stay on the sheet (186 of 868 such vertices in Blackfathom's pool, 36 of 108 in the canal).
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn hole_corner_heights_never_reach_the_mesh_bounds() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");

    for (path, surface_z) in [
        (
            "World\\wmo\\dungeon\\kl_blackfathom\\blackfathom_instance_007.wmo",
            -58.283_04_f32,
        ),
        (
            "World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo",
            -6.482_947_f32,
        ),
    ] {
        let group = reader
            .read(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mesh = wmo_group_liquid_mesh(&group, rule(&reader, path))
            .unwrap_or_else(|| panic!("{path} carries water"));

        // Both pools are flat, so every vertex, drawn or not, sits on the sheet.
        let (lo, hi) = mesh
            .positions
            .iter()
            .map(|p| p[2])
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), z| {
                (lo.min(z), hi.max(z))
            });
        assert!(
            (lo - surface_z).abs() < 1e-3 && (hi - surface_z).abs() < 1e-3,
            "{path}: mesh z spans [{lo}..{hi}], expected the flat sheet at {surface_z}"
        );

        for &i in &mesh.indices {
            let z = mesh.positions[i as usize][2];
            assert!(
                (z - surface_z).abs() < 1e-3,
                "{path}: drawn vertex {i} at z {z}, expected {surface_z}"
            );
        }
    }
}

/// The reference's category 0 splits on the group's `MOGP.flags & 0x48`: the exterior arm binds
/// `MapObjExtWater0.bls` and lights a vertex normal; the interior arm is fixed-function, unlit, and
/// takes its body colour from `MOMT[materialId].diffColor`. Each pool carries a per-vertex opacity.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn the_two_water_arms_carry_their_own_inputs() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");

    // (group file, is the group interior?, the pool's MLIQ materialId)
    for (path, interior, material_id) in [
        (
            "World\\wmo\\dungeon\\kl_blackfathom\\blackfathom_instance_007.wmo",
            true,
            27_u16,
        ),
        (
            "World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo",
            false,
            115_u16,
        ),
    ] {
        let bytes = reader
            .read(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let header = benilla_formats::wmo_group_header(&bytes)
            .unwrap_or_else(|| panic!("{path} parses a MOGP header"));
        assert_eq!(
            header.flags & 0x48 == 0,
            interior,
            "{path}: MOGP flags {:#010x} put it on the wrong water arm",
            header.flags
        );

        let mesh = wmo_group_liquid_mesh(&bytes, rule(&reader, path))
            .unwrap_or_else(|| panic!("{path} carries water"));
        assert_eq!(
            mesh.material_id,
            Some(material_id),
            "{path}: the pool must name its own MOMT slot — an interior arm reads its body colour \
             from exactly this index"
        );

        // Blackfathom's opacity spans 202 values, the canal's is 91% one: only the range is shared.
        let (lo, hi) = mesh
            .depths
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(
            (0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi),
            "{path}: opacity V out of range [{lo}..{hi}]"
        );
        assert!(
            hi > 0.0,
            "{path}: every vertex opacity is zero — the byte is not being read"
        );
    }
}

/// WMO water repeats its texture once per 4.167-yd grid cell: the reference writes `u = i, v = j`
/// from loop counters that start at 0.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn wmo_water_repeats_once_per_grid_cell() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let g099 = reader
        .read("World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo")
        .expect("read Stormwind_099.wmo");
    let mesh = wmo_group_liquid_mesh(
        &g099,
        rule(
            &reader,
            r"World\wmo\Azeroth\Buildings\Stormwind\Stormwind_099.wmo",
        ),
    )
    .expect("group 099 carries water");
    // The grid is 12 vertices wide, so index 12 starts the next row.
    let du = mesh.uvs[1][0] - mesh.uvs[0][0];
    assert!(
        (du - 1.0).abs() < 1e-4,
        "one repeat per cell: adjacent vertices differ by {du} in u, expected 1.0"
    );
    let dv = mesh.uvs[12][1] - mesh.uvs[0][1];
    assert!(
        (dv - 1.0).abs() < 1e-4,
        "one repeat per cell: adjacent rows differ by {dv} in v, expected 1.0"
    );
}

/// Stormwind's root sets MOHD `0x4`, so `groupLiquid` 0 on its streets is no liquid, and
/// its canal groups (id 5) test their MLIQ height rather than flood: no group is flooded, and the
/// canals keep their water.
#[test]
fn wotlk_stormwind_streets_are_dry_and_its_canals_wet() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open patch chain");
    let stem = r"world\wmo\azeroth\buildings\stormwind\stormwind";
    let root = parse_wmo_root(&reader.read(&format!("{stem}.wmo")).expect("read root"))
        .expect("parse root");
    let rule = WmoLiquidRule::for_root(&root);
    assert_eq!(rule, WmoLiquidRule { mohd_ids: true });

    let (mut flooded, mut pools) = (Vec::new(), 0);
    for gi in 0..root.group_count() {
        let bytes = reader
            .read(&format!("{stem}_{gi:03}.wmo"))
            .expect("read group");
        let header = wmo_group_header(&bytes).expect("group header");
        let flood = rule.flood(
            header.group_liquid,
            header.flags,
            wmo_group_has_liquid_grid(&bytes),
        );
        if flood != NO_GROUP_LIQUID {
            flooded.push(gi);
        }
        if let Some(mesh) = wmo_group_liquid_mesh(&bytes, rule) {
            pools += 1;
            assert_eq!(mesh.kind, LiquidKind::Still, "group {gi:03}: canal water");
        }
    }
    assert!(flooded.is_empty(), "wholly submerged groups: {flooded:?}");
    assert!(pools > 0, "the canals carry water");
}

/// Every `LiquidType` id a group stores under MOHD `0x4` is one the client maps to a WMO
/// kind itself (1..=20); a higher id's class lives in `LiquidType.dbc`, which is not read.
#[test]
fn wotlk_group_liquid_ids_stay_in_the_wmo_range() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open patch chain");
    let mut checked = 0;
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
        let Ok(root) = reader.read(&entry.name).map(|b| parse_wmo_root(&b)) else {
            continue;
        };
        let Ok(root) = root else { continue };
        if !WmoLiquidRule::for_root(&root).mohd_ids {
            continue;
        }
        for gi in 0..root.group_count() {
            let Ok(bytes) = reader.read(&format!("{stem}_{gi:03}.wmo")) else {
                continue;
            };
            let Some(header) = wmo_group_header(&bytes) else {
                continue;
            };
            assert!(
                header.group_liquid <= 20,
                "{stem}_{gi:03}: LiquidType id {}",
                header.group_liquid
            );
            checked += 1;
        }
    }
    assert!(checked > 3000, "only {checked} groups under MOHD 0x4");
}
