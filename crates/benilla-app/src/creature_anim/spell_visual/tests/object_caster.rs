//! A GO whose caster is no unit (`0x80ed20`), and the item models column 8's -1 to -5 fly
//! (`0x733820`).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::{VisualKit, VisualStages};

use super::super::super::{CastEvent, CastEventKind, CastHold, EmoteAnim};
use super::super::{
    hand_disarmed, missile_item, AreaKitWait, DelayedAreaKit, MissileItem, MissileSpawn,
    SpellKitSound, ERROR_CUBE,
};
use super::ground_app;
use crate::creature_anim::{ItemModel, SpellGoTargets, Trajectory};

const SPELL: u32 = 5041;
const CAST_KIT: u32 = 11;
const INSTANT_KIT: u32 = 12;
const IMPACT_AREA_KIT: u32 = 13;
const TARGET_KIT: u32 = 14;
const CASTER_KIT: u32 = 15;
const PRECAST_KIT: u32 = 16;
/// Each kit's sound is its id plus this, so a play is told by its sound.
const SOUND: u32 = 900;

/// `OBJECT_FIELD_TYPE` masks: object and game object, object and dynamic object, object and unit.
const GAME_OBJECT: u32 = 0x21;
const DYNAMIC_OBJECT: u32 = 0x41;
const UNIT: u32 = 0x9;

fn kit(id: u32, anim: Option<u16>) -> (u32, VisualKit) {
    (
        id,
        VisualKit {
            sound: Some(id + SOUND),
            anim_id: anim,
            ..Default::default()
        },
    )
}

/// A router over one spell whose row has every kit this file reads, at this Speed.
fn app(speed: f32, missile: bool) -> App {
    ground_app(
        SPELL,
        speed,
        VisualStages {
            precast: PRECAST_KIT,
            cast: CAST_KIT,
            instant_area_kit: INSTANT_KIT,
            impact_area_kit: IMPACT_AREA_KIT,
            target_impact: TARGET_KIT,
            caster_impact: CASTER_KIT,
            missile_gate: u32::from(missile),
            missile_model: u32::from(missile) * 7,
            ..Default::default()
        },
        HashMap::from([
            kit(CAST_KIT, Some(53)),
            kit(PRECAST_KIT, Some(52)),
            kit(INSTANT_KIT, None),
            kit(IMPACT_AREA_KIT, None),
            kit(TARGET_KIT, None),
            kit(CASTER_KIT, None),
        ]),
        HashMap::from([(7, "Spells\\Fireball_Missile_High.mdx".to_string())]),
    )
}

fn object(app: &mut App, mask: u32) -> Entity {
    app.world_mut()
        .spawn(crate::net::ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(2, mask)]),
        ))
        .id()
}

#[allow(clippy::too_many_arguments)] // one GO's whole shape
fn go(
    app: &mut App,
    caster: Entity,
    dest: Option<Vec3>,
    points: &[Vec3],
    hits: Vec<Entity>,
    ammo: Option<ItemModel>,
    reflected: u8,
) {
    go_aimed(app, caster, dest, points, hits, ammo, reflected, None);
}

/// [`go`] with the GO's trajectory.
#[allow(clippy::too_many_arguments)] // one GO's whole shape
fn go_aimed(
    app: &mut App,
    caster: Entity,
    dest: Option<Vec3>,
    points: &[Vec3],
    hits: Vec<Entity>,
    ammo: Option<ItemModel>,
    reflected: u8,
    trajectory: Option<Trajectory>,
) {
    app.world_mut().write_message(CastEvent {
        entity: caster,
        spell_id: SPELL,
        kind: CastEventKind::Go,
        cast_count: 1,
        seq: 1,
    });
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: if dest.is_some() { 0x40 } else { 0x2 },
        src: None,
        src_deck: None,
        caster,
        spell_id: SPELL,
        hits,
        misses: Vec::new(),
        dest,
        dest_deck: None,
        points: points
            .iter()
            .map(|&p| crate::entities::DeckPoint {
                at: p,
                deck: None,
                world: p,
            })
            .collect(),
        ammo,
        seq: 2,
        trajectory,
        reflected,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();
}

fn sounds(app: &mut App) -> Vec<SpellKitSound> {
    app.world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect()
}

fn delayed(app: &mut App) -> Vec<DelayedAreaKit> {
    app.world_mut()
        .resource_mut::<Messages<DelayedAreaKit>>()
        .drain()
        .collect()
}

fn missiles(app: &mut App) -> Vec<MissileSpawn> {
    app.world_mut()
        .resource_mut::<Messages<MissileSpawn>>()
        .drain()
        .collect()
}

fn played_on(sounds: &[SpellKitSound], who: Entity) -> Vec<u32> {
    sounds
        .iter()
        .filter_map(|s| match *s {
            SpellKitSound::Play { entity, kit_sound } if entity == who => Some(kit_sound - SOUND),
            _ => None,
        })
        .collect()
}

fn played_at(sounds: &[SpellKitSound]) -> Vec<(u32, Vec3)> {
    sounds
        .iter()
        .filter_map(|s| match *s {
            SpellKitSound::PlayAt { pos, kit_sound } => Some((kit_sound - SOUND, pos)),
            _ => None,
        })
        .collect()
}

/// A GameObject's dest and each extra point take the no-caster path (`0x80f051`, `0x80f3d8`):
/// column 23 and then column 24 at once, never delayed by the flight or a trajectory (delay 0 at
/// `0x80f0ec`), and no cast kit.
#[test]
fn a_game_object_plays_its_dest_and_points_through_the_no_caster_path() {
    let (at, p) = (Vec3::new(3.0, 0.0, 4.0), Vec3::new(-6.0, 1.0, 2.0));
    let aimed = Trajectory {
        pitch: 0.6,
        speed: 0.0,
        delay: 1.5,
        caster: 0xF110_0000_0000_0001,
        cast_id: 1,
    };
    for (speed, trajectory) in [(0.0, None), (8.0, None), (8.0, Some(aimed))] {
        let mut app = app(speed, false);
        let trap = object(&mut app, GAME_OBJECT);
        go_aimed(
            &mut app,
            trap,
            Some(at),
            &[p],
            Vec::new(),
            None,
            0,
            trajectory,
        );
        let s = sounds(&mut app);
        assert_eq!(
            played_at(&s),
            vec![(INSTANT_KIT, at), (INSTANT_KIT, p)],
            "speed {speed}: column 23 at the dest and the point"
        );
        assert!(played_on(&s, trap).is_empty(), "speed {speed}: no cast kit");
        let landed: Vec<_> = delayed(&mut app)
            .into_iter()
            .map(|d| (d.kit_id, d.pos, d.wait))
            .collect();
        assert_eq!(
            landed,
            vec![
                (IMPACT_AREA_KIT, at, AreaKitWait::Secs(0.0)),
                (IMPACT_AREA_KIT, p, AreaKitWait::Secs(0.0)),
            ],
            "speed {speed}: column 24 at once, the no-caster path's delay 0"
        );
        assert!(missiles(&mut app).is_empty(), "speed {speed}: no missile");
        let anims = app
            .world_mut()
            .resource_mut::<Messages<EmoteAnim>>()
            .drain()
            .count();
        assert_eq!(anims, 0, "speed {speed}: the cast kit's anim never plays");
    }
}

/// A missile row at Speed > 0 carries column 24, so the GameObject plays neither area kit, and
/// with no unit it flies nothing, ammo or not (`0x80ef53`, `0x80f47b`).
#[test]
fn a_game_object_flies_no_missile_and_skips_the_kits_one_would_carry() {
    let mut app = app(8.0, true);
    let trap = object(&mut app, GAME_OBJECT);
    let ammo = ItemModel {
        display_id: 5996,
        inventory_type: 24,
    };
    let target = object(&mut app, UNIT);
    go(
        &mut app,
        trap,
        Some(Vec3::X),
        &[Vec3::Y],
        vec![target],
        Some(ammo),
        0,
    );
    assert!(missiles(&mut app).is_empty(), "no missile");
    assert!(played_at(&sounds(&mut app)).is_empty(), "no column 23");
    assert!(delayed(&mut app).is_empty(), "no column 24");
}

/// Only a GameObject takes the no-caster path: another object caster plays no dest kit.
#[test]
fn another_object_caster_plays_no_dest_kit() {
    let mut app = app(0.0, false);
    let other = object(&mut app, DYNAMIC_OBJECT);
    go(&mut app, other, Some(Vec3::X), &[], Vec::new(), None, 0);
    assert!(played_at(&sounds(&mut app)).is_empty());
    assert!(delayed(&mut app).is_empty());
}

/// At Speed 0 every hit takes the target kit, the object itself included, as no hit is the unit
/// caster (`0x801f10`); a reflect plays nothing back on an object (`0x80f41e`).
#[test]
fn an_object_casters_hits_take_the_target_kit_and_its_reflects_nothing() {
    let mut app = app(0.0, false);
    let trap = object(&mut app, GAME_OBJECT);
    let target = object(&mut app, UNIT);
    go(&mut app, trap, None, &[], vec![target, trap], None, 1);
    let s = sounds(&mut app);
    assert_eq!(played_on(&s, target), vec![TARGET_KIT]);
    assert_eq!(
        played_on(&s, trap),
        vec![TARGET_KIT],
        "no caster kit, no reflect"
    );
}

/// The same GO from a unit plays its cast kit and the caster kit on itself, the contrast.
#[test]
fn a_unit_caster_keeps_its_cast_and_caster_kits() {
    let mut app = app(0.0, false);
    let caster = object(&mut app, UNIT);
    go(&mut app, caster, None, &[], vec![caster], None, 0);
    let s = sounds(&mut app);
    assert_eq!(played_on(&s, caster), vec![CAST_KIT, CASTER_KIT]);
}

/// An object's start holds nothing: the start handler returns with no unit (`0x806891`).
#[test]
fn an_object_casters_start_plays_no_precast_kit() {
    let mut app = app(0.0, false);
    let trap = object(&mut app, GAME_OBJECT);
    app.world_mut().write_message(CastEvent {
        entity: trap,
        spell_id: SPELL,
        kind: CastEventKind::Start,
        cast_count: 1,
        seq: 1,
    });
    app.update();
    assert!(app.world().get::<CastHold>(trap).is_none());
    assert!(played_on(&sounds(&mut app), trap).is_empty());
}

/// Column 8 by id (`0x7338e2`): -1 to -3 the held slot, -4 and -5 the ammo by its own folder, 0
/// the ammo model, anything else a file.
#[test]
fn missile_item_reads_column_8() {
    let ammo = ItemModel {
        display_id: 5996,
        inventory_type: 24,
    };
    let held = |slot: u8| {
        Some(ItemModel {
            display_id: 100 + u32::from(slot),
            inventory_type: 13,
        })
    };
    let item = |m: i32, ammo| missile_item(m as u32, ammo, held);
    for (m, slot) in [(-1, 0u8), (-2, 1), (-3, 2)] {
        assert_eq!(
            item(m, Some(ammo)),
            held(slot).map(MissileItem::Held),
            "{m}: slot {slot}"
        );
    }
    for m in [-4, -5] {
        assert_eq!(item(m, Some(ammo)), Some(MissileItem::Wire(ammo)), "{m}");
        assert_eq!(item(m, None), None, "{m} with no ammo");
    }
    assert_eq!(item(0, Some(ammo)), Some(MissileItem::Ammo(5996)));
    assert_eq!(item(0, None), None);
    assert_eq!(item(7, Some(ammo)), None, "a file id");
    assert_eq!(item(-6, Some(ammo)), None, "below the item ids, a file id");
    assert_eq!(
        missile_item(-2i32 as u32, None, |_| None),
        None,
        "an empty slot"
    );
}

/// A creature's slot is its `Item.dbc` row (`0x732400`); an empty slot or no ammo is the error
/// cube, and the item rides the spawn with no file path.
#[test]
fn a_units_item_missile_flies_its_held_item() {
    const BOW: u32 = 2504;
    for (model, want) in [
        (
            -3i32,
            Some(MissileItem::Held(ItemModel {
                display_id: 8106,
                inventory_type: 15,
            })),
        ),
        (-2, None),
        (-4, None),
        (
            -5,
            Some(MissileItem::Wire(ItemModel {
                display_id: 5996,
                inventory_type: 24,
            })),
        ),
    ] {
        let mut app = ground_app(
            SPELL,
            20.0,
            VisualStages {
                missile_gate: 1,
                missile_model: model as u32,
                ..Default::default()
            },
            HashMap::new(),
            HashMap::new(),
        );
        let mut displays = crate::entities::ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        displays.item_dbc.insert(
            BOW,
            benilla_formats::ItemDbcRow {
                display: 8106,
                inventory_type: 15,
                ..Default::default()
            },
        );
        app.insert_resource(displays);
        let caster = app
            .world_mut()
            .spawn((
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                // `UNIT_VIRTUAL_ITEM_SLOT_ID + 2`, the ranged slot.
                crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, UNIT),
                    (39, BOW),
                ])),
            ))
            .id();
        let target = object(&mut app, UNIT);
        let ammo = (model == -5).then_some(ItemModel {
            display_id: 5996,
            inventory_type: 24,
        });
        go(&mut app, caster, None, &[], vec![target], ammo, 0);
        let spawns = missiles(&mut app);
        assert_eq!(spawns.len(), 1, "{model}");
        assert_eq!(spawns[0].item, want, "{model}");
        let path = want.is_none().then(|| ERROR_CUBE.to_string());
        assert_eq!(spawns[0].path, path, "{model}");
        assert_eq!(spawns[0].scale, 1.0, "{model}: an item flies at scale 1");
    }
}

/// A creature's head item keeps its slot only when its sex byte names a race row (`0x7321ad`).
#[test]
fn a_head_item_needs_the_race_row_its_sex_byte_names() {
    const HELM: u32 = 9001;
    for (sex, want) in [(0u32, false), (1, true)] {
        let mut app = ground_app(
            SPELL,
            20.0,
            VisualStages {
                missile_gate: 1,
                missile_model: -3i32 as u32,
                ..Default::default()
            },
            HashMap::new(),
            HashMap::new(),
        );
        let mut displays = crate::entities::ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        displays.item_dbc.insert(
            HELM,
            benilla_formats::ItemDbcRow {
                display: 4000,
                inventory_type: 1,
                ..Default::default()
            },
        );
        displays.helm_prefixes.insert(1, "Hu".into());
        app.insert_resource(displays);
        let caster = app
            .world_mut()
            .spawn((
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                // The ranged slot and `UNIT_FIELD_BYTES_0`'s sex byte.
                crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, UNIT),
                    (39, HELM),
                    (36, sex << 16),
                ])),
            ))
            .id();
        let target = object(&mut app, UNIT);
        go(&mut app, caster, None, &[], vec![target], None, 0);
        let spawns = missiles(&mut app);
        let held = Some(MissileItem::Held(ItemModel {
            display_id: 4000,
            inventory_type: 1,
        }));
        assert_eq!(spawns[0].item, held.filter(|_| want), "sex {sex}");
    }
}

/// A player's main and off hand go empty under a disarm, the ranged slot never (`0x718fc0`).
#[test]
fn a_disarm_empties_the_hand_it_takes() {
    const DISARMED: u32 = 0x20_0000;
    // (slot, flags, flags2, main is a weapon, own is a weapon) → disarmed
    let cases = [
        ((0, DISARMED, 0, true, true), true),
        ((0, DISARMED, 0, false, false), false),
        ((0, 0, 0, true, true), false),
        ((1, DISARMED, 0, true, true), false),
        ((1, DISARMED, 0, false, true), true),
        ((1, DISARMED, 0, false, false), false),
        ((1, 0, 0x80, true, false), true),
        ((2, DISARMED, 0x80, true, true), false),
    ];
    for ((slot, f, f2, main, own), want) in cases {
        assert_eq!(
            hand_disarmed(slot, f, f2, main, own),
            want,
            "slot {slot} flags {f:#x} {f2:#x}"
        );
    }
}
