//! Transport timetable tests against the nine live transports' server periods; they skip without
//! client data.

use super::*;
use crate::taxi::load_taxi_path_nodes;

struct Golden {
    entry: u32,
    path_id: u32,
    period_ms: u32,
    move_speed: f32,
    accel_rate: f32,
    /// A sniffed row, not a `build 0` fallback.
    sniffed: bool,
}

/// The nine live transports' periods from vmangos's `transports` table (the build ≤ 5875 row) and
/// their `gameobject_template` `data1`/`data2`.
const GOLDENS: &[Golden] = &[
    Golden {
        entry: 20808,
        path_id: 241,
        period_ms: 350818,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 164871,
        path_id: 302,
        period_ms: 356284,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 175080,
        path_id: 285,
        period_ms: 303463,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: false,
    },
    Golden {
        entry: 176231,
        path_id: 292,
        period_ms: 329313,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 176244,
        path_id: 293,
        period_ms: 316251,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: false,
    },
    Golden {
        entry: 176310,
        path_id: 295,
        period_ms: 295579,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 176495,
        path_id: 301,
        period_ms: 333044,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: false,
    },
    Golden {
        entry: 177233,
        path_id: 303,
        period_ms: 317040,
        move_speed: 30.0,
        accel_rate: 1.0,
        sniffed: true,
    },
    Golden {
        entry: 181056,
        path_id: 436,
        period_ms: 1_208_014,
        move_speed: 1.0,
        accel_rate: 1.0,
        sniffed: true,
    },
];

const MODES: &[(&str, TimeMode)] = &[("V", TimeMode::Vmangos), ("C", TimeMode::ClientForms)];
const STEPS_VARIANTS: &[u32] = &[3, 10, 20, 100];

/// A report of every mode and chord count against the goldens; it only asserts that each path's
/// best variant is within 5%.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn nine_period_calibration_report() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let variant_count = MODES.len() * STEPS_VARIANTS.len();
    let mut all_nine_exact = vec![true; variant_count];
    let mut all_sniffed_exact = vec![true; variant_count];

    eprintln!(
        "{:>8} {:>5} {:>5} {:>6} {:>12} {:>12} {:>10} {:>9}",
        "entry", "path", "mode", "steps", "computed_ms", "golden_ms", "delta_ms", "pct"
    );

    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));

        let mut best_pct = f64::MAX;
        let mut best_desc = String::new();
        let mut variant_idx = 0;
        for &(mode_name, mode) in MODES {
            for &steps in STEPS_VARIANTS {
                let computed = TransportTimetable::build_with_variant(
                    nodes,
                    g.move_speed,
                    g.accel_rate,
                    mode,
                    steps,
                )
                .map(|tt| tt.period_ms)
                .unwrap_or(0);
                let delta = i64::from(computed) - i64::from(g.period_ms);
                let pct = delta as f64 / f64::from(g.period_ms) * 100.0;
                eprintln!(
                    "{:>8} {:>5} {:>5} {:>6} {:>12} {:>12} {:>10} {:>8.4}%",
                    g.entry, g.path_id, mode_name, steps, computed, g.period_ms, delta, pct
                );

                if computed != g.period_ms {
                    all_nine_exact[variant_idx] = false;
                    if g.sniffed {
                        all_sniffed_exact[variant_idx] = false;
                    }
                }
                if pct.abs() < best_pct {
                    best_pct = pct.abs();
                    best_desc = format!("{mode_name} steps={steps} computed={computed}");
                }
                variant_idx += 1;
            }
        }

        assert!(
            best_pct < 5.0,
            "path {} ({}): best variant ({best_desc}) not within 5% of golden {}",
            g.path_id,
            g.entry,
            g.period_ms
        );
    }

    let mut variant_idx = 0;
    let mut any_all_nine = false;
    let mut any_all_sniffed = false;
    for &(mode_name, _) in MODES {
        for &steps in STEPS_VARIANTS {
            if all_nine_exact[variant_idx] {
                any_all_nine = true;
                eprintln!(
                    "*** variant mode={mode_name} steps={steps} matches ALL NINE golden periods EXACTLY ***"
                );
            }
            if all_sniffed_exact[variant_idx] {
                any_all_sniffed = true;
                eprintln!(
                    "*** variant mode={mode_name} steps={steps} matches ALL SIX SNIFFED golden periods EXACTLY ***"
                );
            }
            variant_idx += 1;
        }
    }
    if !any_all_nine && !any_all_sniffed {
        eprintln!(
            "(no variant matched any golden subset exactly this round — see the table above)"
        );
    }
}

/// `transport_period` must reproduce every golden exactly, since `build` pins its period to it.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn client_period_bit_exact() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let mut failures = Vec::new();
    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));
        let computed = crate::transport_period::client_period_ms(nodes, g.move_speed, g.accel_rate)
            .unwrap_or(0);
        let delta = i64::from(computed) - i64::from(g.period_ms);
        eprintln!(
            "path {:>4}: client_period={computed:>8}  golden={:>8}  delta={delta:+}",
            g.path_id, g.period_ms
        );
        if delta != 0 {
            failures.push((g.path_id, computed, g.period_ms));
        }
    }
    assert!(
        failures.is_empty(),
        "client period diverges from the server-sniff goldens: {failures:?}"
    );
}

/// Also proves that at least one live transport crosses continents.
#[test]
fn touches_map_matches_the_paths_map_set() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let mut any_cross_map = false;
    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));
        let tt = TransportTimetable::build(nodes, g.move_speed, g.accel_rate)
            .unwrap_or_else(|| panic!("path {} builds", g.path_id));
        let maps: std::collections::HashSet<u32> = nodes.iter().map(|n| n.map_id).collect();
        any_cross_map |= maps.len() > 1;
        for &m in &maps {
            assert!(tt.touches_map(m), "path {} should touch map {m}", g.path_id);
        }
        assert!(
            !tt.touches_map(9999),
            "path {} touches a map no node lives on",
            g.path_id
        );
    }
    assert!(
        any_cross_map,
        "no golden path crosses continents — the worldport spare predicate has nothing to spare"
    );
}

/// From any instant, the answer samples on the asked-for map, on every cross-continent path:
/// path 241 reaches map 1 only across the cycle wrap.
#[test]
fn first_cycle_on_map_lands_on_that_map() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");

    let mut crossed = 0usize;
    for g in GOLDENS {
        let nodes = cat
            .path(g.path_id)
            .unwrap_or_else(|| panic!("path {} exists in TaxiPathNode.dbc", g.path_id));
        let tt = TransportTimetable::build(nodes, g.move_speed, g.accel_rate)
            .unwrap_or_else(|| panic!("path {} builds", g.path_id));
        let maps: std::collections::HashSet<u32> = nodes.iter().map(|n| n.map_id).collect();
        if maps.len() < 2 {
            continue;
        }
        crossed += 1;
        // 64 instants across the cycle, both sides of the wrap included.
        for k in 0..64u32 {
            let from = (u64::from(tt.period_ms) * u64::from(k) / 64) as u32;
            for &m in &maps {
                let at = tt.first_cycle_on_map(from, m).unwrap_or_else(|| {
                    panic!(
                        "path {} touches map {m} but found no cycle on it",
                        g.path_id
                    )
                });
                assert!(
                    at < tt.period_ms,
                    "path {} map {m}: {at} ms is outside the {} ms cycle",
                    g.path_id,
                    tt.period_ms
                );
                assert_eq!(
                    tt.sample(at).map,
                    m,
                    "path {} from {from} ms: first_cycle_on_map({m}) = {at} ms samples map {}",
                    g.path_id,
                    tt.sample(at).map
                );
            }
        }
        // A map the path never visits.
        assert_eq!(tt.first_cycle_on_map(0, 9999), None);
    }
    assert!(
        crossed > 0,
        "no golden path crosses continents — this test proves nothing"
    );
}

/// The server's period replaces the built one: a longer cycle holds the last keyframe past the
/// built end, the sample before it is untouched, and `0` parks the path at its head.
#[test]
fn a_server_period_replaces_the_built_one() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_taxi_path_nodes(&mut chain).expect("load TaxiPathNode");
    let nodes = cat.path(302).expect("taxi path 302 (Orgrimmar–Undercity)");
    let built = TransportTimetable::build(nodes, 30.0, 1.0).expect("path 302 builds");

    let longer = built.with_period(built.period_ms + 5000);
    assert_eq!(longer.period_ms, built.period_ms + 5000);
    let (held, end) = (
        longer.sample(built.period_ms + 2500),
        longer.sample(longer.period_ms - 1),
    );
    assert_eq!((held.map, held.pos), (end.map, end.pos));
    let mid = built.period_ms / 2;
    assert_eq!(longer.sample(mid).pos, built.sample(mid).pos);

    let parked = built.with_period(0);
    assert_eq!(parked.period_ms, 0);
    assert_eq!(parked.sample(12345).pos, built.sample(0).pos);
}

/// A loop of two stops with cruise between them: each span runs Stand, ShipStart, ShipMoving,
/// ShipStop, Stand, as the reference's ship path hands its props (`0x7f82b0`): Stand only parked
/// at a stop, never at the cycle's wrap, which the ship passes at speed.
#[test]
fn a_span_between_stops_runs_start_moving_stop() {
    // A 2000 x 400 yd rectangle, a node every 200 yd, stops midway along each long side.
    let mut ring: Vec<[f32; 3]> = Vec::new();
    for i in 0..=10 {
        ring.push([i as f32 * 200.0, 0.0, 0.0]);
    }
    ring.push([2000.0, 200.0, 0.0]);
    for i in (0..=10).rev() {
        ring.push([i as f32 * 200.0, 400.0, 0.0]);
    }
    ring.push([0.0, 200.0, 0.0]);
    let stops = [5usize, 17];
    // The builder never travels the first and last node: pad the ring by one on each side.
    let mut nodes = Vec::new();
    let n = ring.len();
    for (k, i) in (0..n + 3).map(|k| (k, (k + n - 1) % n)) {
        nodes.push(crate::taxi::TaxiPathNode {
            id: k as u32,
            path_id: 1,
            node_index: k as u32,
            map_id: 0,
            pos: ring[i],
            flags: if stops.contains(&i) { 2 } else { 0 },
            delay: if stops.contains(&i) { 10 } else { 0 },
        });
    }
    // The accumulated cycle, unpinned: a pinned longer period parks at the last keyframe, which
    // is the sampler's own hold, not the wrap this test walks through.
    let tt = TransportTimetable::build_with_variant(&nodes, 30.0, 1.0, TimeMode::Vmangos, 20)
        .expect("the loop builds");
    let mut runs: Vec<u16> = Vec::new();
    for ms in (0..tt.period_ms).step_by(50) {
        let s = tt.sample(ms);
        if s.moving {
            assert_ne!(s.sequence, SHIP_STAND, "at {ms} ms: {s:?}");
        }
        if runs.last() != Some(&s.sequence) {
            runs.push(s.sequence);
        }
    }
    let cycle = [SHIP_STAND, SHIP_START, SHIP_MOVING, SHIP_STOP];
    let head = runs
        .iter()
        .position(|&q| q == SHIP_STAND)
        .expect("the loop stops");
    // Rotated to start parked; the wrap joins the tail's run to the head's.
    let mut walked: Vec<u16> = runs[head..].to_vec();
    let tail = walked.last().copied();
    walked.extend(runs[..head].iter().skip_while(|&&q| Some(q) == tail));
    assert!(walked.len() >= 8, "two full spans: {runs:?}");
    for (k, q) in walked.iter().enumerate() {
        assert_eq!(*q, cycle[k % 4], "the state order: {runs:?}");
    }
}

/// The reference's ship legs: a new leg after a teleport node and at a map change, the index the
/// ship loading screen's leg-change entry compares (`0x7f90f0`, `0x7134a0`).
#[test]
fn ship_legs_open_after_a_teleport_and_at_a_map_change() {
    let node = |i: u32, map_id: u32, flags: u32| TaxiPathNode {
        id: i,
        path_id: 1,
        node_index: i,
        map_id,
        pos: [i as f32 * 100.0, 0.0, 0.0],
        flags,
        delay: 0,
    };
    let nodes = [
        node(0, 0, 0),
        node(1, 0, 2),
        node(2, 0, 1), // teleport: the next node opens leg 1
        node(3, 0, 0),
        node(4, 0, 2),
        node(5, 1, 0), // another map: leg 2
        node(6, 1, 2),
    ];
    assert_eq!(ship_legs(&nodes), vec![0, 0, 0, 1, 1, 2, 2]);
}

fn node(i: u32, pos: [f32; 3], stop: bool) -> TaxiPathNode {
    TaxiPathNode {
        id: i,
        path_id: 1,
        node_index: i,
        map_id: 0,
        pos,
        flags: if stop { 2 } else { 0 },
        delay: if stop { 10 } else { 0 },
    }
}

/// A straight run between two stops along +x, 2000 yd, and back's first node.
fn straight() -> TransportTimetable {
    let nodes: Vec<_> = (0..12)
        .map(|i| node(i, [i as f32 * 200.0, 0.0, 0.0], i == 1 || i == 10))
        .collect();
    TransportTimetable::build(&nodes, 30.0, 1.0).expect("builds")
}

fn ship_physics() -> TransportPhysics {
    TransportPhysics {
        wave_amp: 0.5,
        wave_time_scale: 0.5,
        roll_amp: 0.1,
        roll_time_scale: 0.25,
        pitch_amp: 0.05,
        pitch_time_scale: 0.1,
        max_bank: 0.05,
        max_bank_turn_speed: 0.7,
        speed_damp_thresh: 25.0,
        speed_damp: 0.1,
    }
}

/// The cycle instant of the middle of the first run, at cruise.
fn cruising(tt: &TransportTimetable) -> u32 {
    (0..tt.period_ms)
        .step_by(500)
        .find(|&ms| {
            let s = tt.sample(ms);
            s.moving && s.pos[0] > 900.0
        })
        .expect("a cruising instant")
}

/// The rocked sample is the plain one with the wave on its height and the roll and pitch under
/// its heading; with no turn the roll is the bare sine (`0x7f7b30`).
#[test]
fn a_rocked_ship_lifts_and_tilts_under_its_heading() {
    let tt = straight();
    let p = ship_physics();
    let ms = cruising(&tt);
    let (plain, rocked) = (tt.sample(ms), tt.sample_rocked(ms, &p));
    let r = p.rocking(ms, 30.0, 0.0);
    assert!((rocked.pos[2] - plain.pos[2] - r.wave).abs() < 1e-4);
    assert_eq!(&rocked.pos[..2], &plain.pos[..2]);
    let want = physics::yaw_pitch_roll(plain.heading, r.pitch, r.roll);
    let got = rocked.rotation.expect("rocked");
    assert!(
        got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4),
        "{got:?} vs {want:?}"
    );
}

/// Docked, the ship rocks at its damped amplitude, all three motions, and never banks.
#[test]
fn a_docked_ship_rocks_damped() {
    let tt = straight();
    let p = ship_physics();
    let ms = (0..tt.period_ms)
        .step_by(100)
        .find(|&ms| !tt.sample(ms).moving)
        .expect("a stop");
    let (plain, rocked) = (tt.sample(ms), tt.sample_rocked(ms, &p));
    let r = p.rocking(ms, 0.0, 0.0);
    assert!((rocked.pos[2] - plain.pos[2] - r.wave).abs() < 1e-5);
    let want = physics::yaw_pitch_roll(plain.heading, r.pitch, r.roll);
    let got = rocked.rotation.expect("rocked");
    assert!(
        got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-5),
        "{got:?} vs {want:?}"
    );
}

/// The look-back's heading lookup by arc length agrees with the sample's own heading wherever
/// the ship travels.
#[test]
fn the_heading_by_arc_length_is_the_sampled_heading() {
    let nodes: Vec<_> = (0..24)
        .map(|i| {
            let a = i as f32 * 0.12;
            node(
                i,
                [3000.0 * a.sin(), 3000.0 * (1.0 - a.cos()), 0.0],
                i == 1 || i == 22,
            )
        })
        .collect();
    let tt = TransportTimetable::build(&nodes, 30.0, 1.0).expect("builds");
    let mut checked = 0;
    for ms in (0..tt.period_ms).step_by(250) {
        let located = tt.locate(ms);
        let Some(on) = located.on_leg else { continue };
        let by_dist = tt.leg_heading_at(on.leg, on.leg_dist);
        let mut d = (by_dist - located.sample.heading).abs();
        d = d.min(std::f32::consts::TAU - d);
        assert!(
            d < 1e-3,
            "at {ms} ms: {by_dist} vs {}",
            located.sample.heading
        );
        checked += 1;
    }
    assert!(checked > 100, "{checked} travelling instants");
}

/// On a left turn the bank adds to the roll (`0x7f7dd0`'s turn estimate is positive), on a
/// straight it adds nothing.
#[test]
fn a_turning_ship_banks_into_the_turn() {
    // A long arc turning left (counter-clockwise) between two far stops.
    let nodes: Vec<_> = (0..24)
        .map(|i| {
            let a = i as f32 * 0.12;
            node(
                i,
                [3000.0 * a.sin(), 3000.0 * (1.0 - a.cos()), 0.0],
                i == 1 || i == 22,
            )
        })
        .collect();
    let tt = TransportTimetable::build(&nodes, 30.0, 1.0).expect("builds");
    let p = ship_physics();
    let ms = (0..tt.period_ms)
        .step_by(500)
        .find(|&ms| {
            let s = tt.sample(ms);
            s.moving && s.pos[1] > 600.0
        })
        .expect("mid-arc");
    let level = p.rocking(ms, 30.0, 0.0);
    let rocked = tt.sample_rocked(ms, &p);
    // Recover the roll: R(q)·(0,1,0)'s z is sin(roll)·cos(pitch) for Rz·Ry·Rx.
    let [x, y, z, w] = rocked.rotation.unwrap();
    let side_z = 2.0 * (y * z + w * x);
    let roll = (side_z / level.pitch.cos()).asin();
    assert!(roll > level.roll, "banked {roll} vs level {}", level.roll);
    // Straight: no bank.
    let tt = straight();
    let ms = cruising(&tt);
    let r = tt.sample_rocked(ms, &p).rotation.unwrap();
    let want = physics::yaw_pitch_roll(
        tt.sample(ms).heading,
        p.rocking(ms, 30.0, 0.0).pitch,
        p.rocking(ms, 30.0, 0.0).roll,
    );
    assert!(r.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4));
}
