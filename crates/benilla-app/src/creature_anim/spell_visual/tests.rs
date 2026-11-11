//! Headless tests for the cast-edge router [`super::route_cast_visuals`] and the aura and mount
//! watchers, over synthetic visual chains and, where an install is present, the shipped tables.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualKit, VisualStages};

use super::super::{
    CastEvent, CastEventKind, CastHold, EmoteAnim, RangedHold, SheathRequest, WoundAnim,
};
use super::{
    route_cast_visuals, KitPush, MissileSpawn, SpellKitFx, SpellKitShake, SpellKitSound,
    SpellVisuals, StateDoneKit,
};
use crate::creature_anim::SpellGoTargets;

/// Demon Armor's shipped chain: visual 130 → precast kit 217 with anim 52, an instant self-buff.
const SPELL: u32 = 706;
const VISUAL: u32 = 130;
const PRECAST_KIT: u32 = 217;
const HOLD_ANIM: u16 = 52;

/// A ranged-slot spell (`Attributes & 0x2`) whose own visual's cast kit plays the fire clip.
const RANGED_SPELL: u32 = 19434;
const RANGED_VISUAL: u32 = 3180;
const RANGED_CAST_KIT: u32 = 900;
const FIRE_ANIM: u16 = 46;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                VISUAL,
                VisualStages {
                    precast: PRECAST_KIT,
                    ..Default::default()
                },
            ),
            (
                RANGED_VISUAL,
                VisualStages {
                    cast: RANGED_CAST_KIT,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::from([
            (
                PRECAST_KIT,
                VisualKit {
                    anim_id: Some(HOLD_ANIM),
                    ..Default::default()
                },
            ),
            (
                RANGED_CAST_KIT,
                VisualKit {
                    anim_id: Some(FIRE_ANIM),
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                SPELL,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            ),
            (
                RANGED_SPELL,
                SpellDisplay {
                    visual: RANGED_VISUAL,
                    attributes: 0x2, // USES_RANGED_SLOT, the `0x400` hold's gate
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);
    app
}

fn cast_event(entity: Entity, spell_id: u32, kind: CastEventKind) -> CastEvent {
    CastEvent {
        entity,
        spell_id,
        kind,
        cast_count: 0,
        seq: 1,
    }
}

fn hold(app: &App, unit: Entity) -> Option<u32> {
    app.world().entity(unit).get::<CastHold>().map(|h| {
        assert_eq!(h.anim_id, HOLD_ANIM);
        h.spell_id
    })
}

#[test]
fn timed_cast_hold_arms_and_releases_across_frames() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.update();
    assert_eq!(hold(&app, unit), Some(SPELL), "START arms the hold");

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Go));
    app.update();
    assert_eq!(hold(&app, unit), None, "GO releases it");
}

/// An instant cast's START and GO drain in one frame: the GO must see the hold its own batch
/// inserted, or the hold leaks and the cast pose loops forever.
#[test]
fn same_frame_start_and_go_leave_no_hold() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Go));
    app.update();
    assert_eq!(
        hold(&app, unit),
        None,
        "the instant cast's hold is released"
    );
}

/// The hold is keyed by spell id, so a proc's GO never drops it, in the same frame or later.
#[test]
fn a_foreign_go_never_drops_the_hold() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.world_mut()
        .write_message(cast_event(unit, 999, CastEventKind::Go));
    app.update();
    assert_eq!(
        hold(&app, unit),
        Some(SPELL),
        "same-frame foreign GO ignored"
    );

    app.world_mut()
        .write_message(cast_event(unit, 999, CastEventKind::Go));
    app.update();
    assert_eq!(hold(&app, unit), Some(SPELL), "later foreign GO ignored");
}

/// The precast kit's sound (kit field 13) rings once at START, not again at GO. Herb Gathering's
/// shipped chain: visual 91 → precast kit 64, anim 123 (UseStandingLoop), sound 1104 (Gather_Herb).
#[test]
fn precast_kit_sound_rings_once_at_start() {
    const HERB: u32 = 2366;
    const HERB_VISUAL: u32 = 91;
    const HERB_KIT: u32 = 64;
    const HERB_SOUND: u32 = 1104;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            HERB_VISUAL,
            VisualStages {
                precast: HERB_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            HERB_KIT,
            VisualKit {
                anim_id: Some(123),
                sound: Some(HERB_SOUND),
                ..Default::default()
            },
        )]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            HERB,
            SpellDisplay {
                visual: HERB_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, HERB, CastEventKind::Start));
    app.update();
    let played: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect();
    assert!(
        matches!(
            played.as_slice(),
            [
                SpellKitSound::StopHold { .. },
                SpellKitSound::Play { kit_sound, .. }
            ] if *kit_sound == HERB_SOUND
        ),
        "START rings the precast kit's sound once (got {played:?})"
    );

    app.world_mut()
        .write_message(cast_event(unit, HERB, CastEventKind::Go));
    app.update();
    let after_go: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect();
    assert!(
        !after_go
            .iter()
            .any(|s| matches!(s, SpellKitSound::Play { .. })),
        "the GO plays no second kit sound (got {after_go:?})"
    );
}

/// The `$TRD` strike sound is the held spell's `SpellVisual` field 14: Mining's visual 93 names
/// 1143 (Mining Impact), Fireball's visual 67 none.
#[test]
fn held_strike_sound_reads_the_visuals_field_14() {
    const MINING: u32 = 2575;
    const FIREBALL: u32 = 133;
    let visuals = SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                93,
                VisualStages {
                    precast: 166,
                    strike_sound: Some(1143),
                    ..Default::default()
                },
            ),
            (
                67,
                VisualStages {
                    precast: 30,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::new(),
    );
    let spells = crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                MINING,
                SpellDisplay {
                    visual: 93,
                    ..Default::default()
                },
            ),
            (
                FIREBALL,
                SpellDisplay {
                    visual: 67,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    };
    assert_eq!(
        super::held_strike_sound(&spells, &visuals, MINING),
        Some(1143)
    );
    assert_eq!(super::held_strike_sound(&spells, &visuals, FIREBALL), None);
    assert_eq!(super::held_strike_sound(&spells, &visuals, 999), None);
}

#[test]
fn same_frame_start_and_fail_leave_no_hold() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.world_mut().write_message(cast_event(
        unit,
        SPELL,
        CastEventKind::Fail {
            interrupt: false,
            wire_reason: 0,
        },
    ));
    app.update();
    assert_eq!(hold(&app, unit), None, "the failed cast's hold is released");
}

/// The ranged weapon-visual merge: a spell with `Attributes & 0x2` fills every zero slot of its own
/// row from the caster's weapon visual; a non-ranged spell never looks.
#[test]
fn ranged_spells_merge_the_weapon_visual_into_their_empty_slots() {
    const THROW: u32 = 2764; // Attributes 0x410012, SpellVisual1 0: the shipped Throw row
    const FIREBALL: u32 = 133; // its own visual; the fallback must stay unused
    const MULTI_SHOT: u32 = 2643; // RANGED, own visual 567: impact + missile, no body kits
    const NO_VIS_MELEE: u32 = 772; // no visual, no RANGED attribute: stays silent
    const WEAPON_VISUAL: u32 = 98; // a shipped thrown weapon's ItemDisplayInfo col-10 visual
    const MULTI_SHOT_VISUAL: u32 = 567;

    let visuals = SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                WEAPON_VISUAL,
                VisualStages {
                    precast: 171,
                    cast: 172,
                    ..Default::default()
                },
            ),
            (
                VISUAL,
                VisualStages {
                    precast: PRECAST_KIT,
                    ..Default::default()
                },
            ),
            (
                // The shipped row: impact kit 658, missile 528, gate 1, attach 1, no body kits.
                MULTI_SHOT_VISUAL,
                VisualStages {
                    impact: 658,
                    missile_gate: 1,
                    missile_model: 528,
                    missile_attach: 1,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::new(),
    );
    let spells = crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                THROW,
                SpellDisplay {
                    visual: 0,
                    attributes: 0x410012,
                    ..Default::default()
                },
            ),
            (
                FIREBALL,
                SpellDisplay {
                    visual: VISUAL,
                    attributes: 0x2, // ranged, with its own visual: its own slot wins
                    ..Default::default()
                },
            ),
            (
                MULTI_SHOT,
                SpellDisplay {
                    visual: MULTI_SHOT_VISUAL,
                    attributes: 0x10002, // the shipped word: RANGED set, own visual present
                    ..Default::default()
                },
            ),
            (
                NO_VIS_MELEE,
                SpellDisplay {
                    visual: 0,
                    attributes: 0,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    };

    let throw = super::resolve_stages(&spells, &visuals, THROW, || Some(WEAPON_VISUAL));
    assert_eq!(
        throw.map(|s| (s.precast, s.cast)),
        Some((171, 172)),
        "Throw borrows the weapon visual's kits"
    );
    assert!(
        super::resolve_stages(&spells, &visuals, THROW, || None).is_none(),
        "no ranged weapon equipped → still silent"
    );
    assert_eq!(
        super::resolve_stages(&spells, &visuals, FIREBALL, || Some(WEAPON_VISUAL))
            .map(|s| s.precast),
        Some(PRECAST_KIT),
        "a populated slot is never displaced by the weapon's"
    );
    let multi = super::resolve_stages(&spells, &visuals, MULTI_SHOT, || Some(WEAPON_VISUAL))
        .expect("Multi-Shot has its own row");
    assert_eq!(
        (multi.precast, multi.cast),
        (171, 172),
        "the empty body-kit slots fill from the weapon — else the draw/release is missing"
    );
    assert_eq!(
        (multi.impact, multi.missile_model, multi.missile_attach),
        (658, 528, 1),
        "its own impact + missile block survives the merge untouched"
    );
    assert_eq!(
        super::resolve_stages(&spells, &visuals, MULTI_SHOT, || None).map(|s| (s.cast, s.impact)),
        Some((0, 658)),
        "no ranged weapon equipped → the own row stands alone, unfilled"
    );
    assert!(
        super::resolve_stages(&spells, &visuals, NO_VIS_MELEE, || Some(WEAPON_VISUAL)).is_none(),
        "a non-ranged spell never takes the fallback"
    );
}

/// The aura watcher arms a state kit's effects persistent under [`super::FxClass::AuraState`] as
/// the spell enters the slots and reaps them as it leaves. Food: spell 433 → visual 51 → kit 409.
#[test]
fn aura_state_kit_arms_persistent_and_reaps_on_aura_end() {
    const FOOD: u32 = 433;
    const FOOD_VISUAL: u32 = 51;
    const STATE_KIT: u32 = 409;
    const BREAD_FX: u32 = 393;

    #[derive(Resource, Default)]
    struct FxLog(Vec<SpellKitFx>);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>();
    app.add_message::<crate::net::FieldChanged>();
    // The watcher's CharProc and sound writers need their messages; kit 409 carries neither.
    app.add_message::<crate::aura_visual::AuraProc>();
    app.add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>();
    app.init_resource::<crate::net::GuidIndex>();
    app.add_message::<crate::aura_visual::BodyProcPlay>();
    app.add_message::<SheathRequest>();
    app.add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>();
    app.init_resource::<FxLog>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
        HashMap::from([(
            FOOD_VISUAL,
            VisualStages {
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            STATE_KIT,
            VisualKit {
                // Slot 4 is the spell-hand tag (KIT_SLOT_TAGS[4] = 0x16), bread's shipped slot.
                effect_slots: [
                    None,
                    None,
                    None,
                    None,
                    Some(BREAD_FX),
                    None,
                    None,
                    None,
                    None,
                ],
                ..Default::default()
            },
        )]),
        HashMap::from([(BREAD_FX, "Spells\\Item_Bread.mdx".to_string())]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            FOOD,
            SpellDisplay {
                visual: FOOD_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(
        Update,
        (
            super::arm_aura_state_fx,
            |mut r: MessageReader<SpellKitFx>, mut log: ResMut<FxLog>| {
                log.0.extend(r.read().cloned());
            },
        )
            .chain(),
    );

    let unit = app
        .world_mut()
        .spawn(crate::unit_auras::aura_unit_for_test(&[FOOD]))
        .id();
    app.update();
    {
        let log = &app.world().resource::<FxLog>().0;
        assert_eq!(log.len(), 1, "one Begin on the ADD edge");
        let SpellKitFx::Begin {
            spell_id,
            persistent,
            class,
            effects,
            ..
        } = &log[0]
        else {
            panic!("expected Begin");
        };
        assert_eq!(*spell_id, FOOD);
        assert!(*persistent, "state kit persists for the aura's life");
        assert_eq!(*class, super::FxClass::AuraState);
        assert_eq!(
            effects.as_slice(),
            [super::FxSlot {
                tag: 0x16,
                effect: BREAD_FX,
                path: "Spells\\Item_Bread.mdx".to_string(),
                scale: 1.0,
                attach: None,
                own_alpha: false,
                kit_anim: None,
            }],
            "bread at the spell hand"
        );
    }

    app.update();
    assert_eq!(
        app.world().resource::<FxLog>().0.len(),
        1,
        "a held aura re-arms nothing"
    );

    crate::unit_auras::set_auras_for_test(app.world_mut(), unit, &[]);
    app.update();
    {
        let log = &app.world().resource::<FxLog>().0;
        // The spell clear on the REMOVE edge takes every class of the spell's models (`0x743b40`).
        let reaps: Vec<_> = log[1..]
            .iter()
            .map(|e| match e {
                SpellKitFx::Reap {
                    spell_id, class, ..
                } => (*spell_id, *class),
                SpellKitFx::Begin { .. } | SpellKitFx::Hold { .. } => panic!("expected Reap"),
            })
            .collect();
        assert_eq!(
            reaps,
            [
                (FOOD, super::FxClass::AuraState),
                (FOOD, super::FxClass::Hold)
            ]
        );
    }
}

/// The spell's visual pick (`0x7fa290`): below violence 2 the second visual, when the spell has
/// one.
#[test]
fn low_violence_takes_the_second_visual_when_there_is_one() {
    let both = SpellDisplay {
        visual: 10,
        visual_low_violence: 11,
        ..Default::default()
    };
    let one = SpellDisplay {
        visual: 10,
        ..Default::default()
    };
    assert_eq!(super::pick_visual(&both, false), 10);
    assert_eq!(super::pick_visual(&both, true), 11);
    assert_eq!(super::pick_visual(&one, true), 10);
}

/// The removal core's tail (`0x71ebef`): an armed aura's end plays its state-done kit (column 5)
/// once, one-shot, even with no state kit; the add edge plays nothing of it.
#[test]
fn an_ending_aura_plays_its_state_done_kit_once() {
    const AURA: u32 = 43039;
    const VISUAL: u32 = 7001;
    const DONE_KIT: u32 = 7002;
    const DONE_FX: u32 = 7003;

    #[derive(Resource, Default)]
    struct FxLog(Vec<SpellKitFx>);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>();
    app.add_message::<crate::net::FieldChanged>();
    app.add_message::<crate::aura_visual::AuraProc>();
    app.add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>();
    app.init_resource::<crate::net::GuidIndex>();
    app.add_message::<crate::aura_visual::BodyProcPlay>();
    app.add_message::<SheathRequest>();
    app.add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>();
    app.init_resource::<FxLog>();
    app.init_resource::<super::super::PlaySeq>();
    let mut slots = [None; 9];
    slots[4] = Some(DONE_FX);
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
        HashMap::from([(
            VISUAL,
            VisualStages {
                state_done: DONE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            DONE_KIT,
            VisualKit {
                effect_slots: slots,
                ..Default::default()
            },
        )]),
        HashMap::from([(DONE_FX, "Spells\\Done.mdx".to_string())]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            AURA,
            SpellDisplay {
                visual: VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(
        Update,
        (
            super::arm_aura_state_fx,
            super::play_state_done_kits,
            |mut r: MessageReader<SpellKitFx>, mut log: ResMut<FxLog>| {
                log.0.extend(r.read().cloned());
            },
        )
            .chain(),
    );

    let unit = app
        .world_mut()
        .spawn(crate::unit_auras::aura_unit_for_test(&[AURA]))
        .id();
    app.update();
    assert!(
        app.world().resource::<FxLog>().0.is_empty(),
        "no state kit: the add edge spawns nothing"
    );

    crate::unit_auras::set_auras_for_test(app.world_mut(), unit, &[]);
    app.update();
    let begins: Vec<_> = app
        .world()
        .resource::<FxLog>()
        .0
        .iter()
        .filter_map(|f| match f {
            SpellKitFx::Begin {
                spell_id,
                persistent,
                stage,
                effects,
                ..
            } => Some((*spell_id, *persistent, *stage, effects.len())),
            _ => None,
        })
        .collect();
    assert_eq!(
        begins,
        [(AURA, false, super::FxStage::OneShot, 1)],
        "the state-done kit plays once, one-shot"
    );
    let count = app.world().resource::<FxLog>().0.len();
    app.update();
    assert_eq!(
        app.world().resource::<FxLog>().0.len(),
        count,
        "and only once"
    );
}

/// An instant harmful GO lays one severity-0 wound on each hit, a helpful one none; a state kit's
/// wound anim adds nothing (stage 2 never reaches the `[8,10]` test); a missile arrival always
/// wounds.
#[test]
fn a_harmful_go_wounds_each_hit_once_and_a_missile_arrival_always() {
    const HARMFUL: u32 = 7386; // Sunder Armor's shape: instant, enemy-targeted
    const HELPFUL: u32 = 139; // Renew's shape: instant, ally-targeted
    const VISUAL_H: u32 = 406;
    const VISUAL_F: u32 = 280;
    const IMPACT_KIT: u32 = 556; // no anim
    const STATE_KIT: u32 = 436; // names CombatWound (9), which must add nothing

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<SheathRequest>();
    let stages = VisualStages {
        impact: IMPACT_KIT,
        state: STATE_KIT,
        ..Default::default()
    };
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(VISUAL_H, stages), (VISUAL_F, stages)]),
        HashMap::from([
            (
                IMPACT_KIT,
                VisualKit {
                    anim_id: None,
                    ..Default::default()
                },
            ),
            (
                STATE_KIT,
                VisualKit {
                    anim_id: Some(9),
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                HARMFUL,
                SpellDisplay {
                    visual: VISUAL_H,
                    targets: 0x80,
                    ..Default::default()
                },
            ),
            (
                HELPFUL,
                SpellDisplay {
                    visual: VISUAL_F,
                    targets: 0x100,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);

    let caster = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn_empty().id();
    let wounds = |app: &mut App| -> Vec<Entity> {
        app.world_mut()
            .resource_mut::<Messages<WoundAnim>>()
            .drain()
            .map(|w| w.entity)
            .collect()
    };
    for (spell_id, expected) in [(HARMFUL, vec![target]), (HELPFUL, Vec::new())] {
        app.world_mut()
            .write_message(cast_event(caster, spell_id, CastEventKind::Go));
        app.world_mut().write_message(SpellGoTargets {
            list: Vec::new(),
            target_mask: 0x2,
            src: None,
            caster,
            spell_id,
            hits: vec![target],
            misses: Vec::new(),
            dest: None,
            points: Vec::new(),
            ammo_display_id: None,
            seq: 1,
            trajectory: None,
            reflected: 0,
            reflects: Vec::new(),
            caster_guid: 0,
            server_time: 0,
        });
        app.update();
        assert_eq!(wounds(&mut app), expected, "instant GO of spell {spell_id}");
    }
    app.world_mut().write_message(cast_event(
        target,
        HELPFUL,
        CastEventKind::Impact {
            on_caster: false,
            at: None,
            caster: target,
        },
    ));
    app.update();
    assert_eq!(wounds(&mut app), vec![target], "missile arrival");
}

/// The GO's release gate: a Speed>0 spell whose cast kit plays a body animation defers its
/// [`MissileSpawn`] to the animation's release keyframe; any other launches at GO. A row with no
/// missile flies only on an ammo model (`0x7fffb0`).
#[test]
fn missile_spawn_defers_iff_the_cast_kit_animates() {
    const ANIMATED: u32 = 133; // Fireball's shape: cast kit with anim 53
    const SILENT: u32 = 134; // same chain, cast kit with no anim
    const ANIMATED_VISUAL: u32 = 67;
    const SILENT_VISUAL: u32 = 68;
    const CAST_KIT: u32 = 38;
    const MUTE_KIT: u32 = 39;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                ANIMATED_VISUAL,
                VisualStages {
                    cast: CAST_KIT,
                    missile_gate: 1,
                    ..Default::default()
                },
            ),
            (
                SILENT_VISUAL,
                VisualStages {
                    cast: MUTE_KIT,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::from([
            (
                CAST_KIT,
                VisualKit {
                    anim_id: Some(53),
                    ..Default::default()
                },
            ),
            (
                MUTE_KIT,
                VisualKit {
                    anim_id: None,
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                ANIMATED,
                SpellDisplay {
                    visual: ANIMATED_VISUAL,
                    speed: 24.0,
                    ..Default::default()
                },
            ),
            (
                SILENT,
                SpellDisplay {
                    visual: SILENT_VISUAL,
                    speed: 24.0,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);

    let caster = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn_empty().id();
    for (spell_id, ammo) in [(ANIMATED, None), (SILENT, Some(5)), (SILENT, None)] {
        app.world_mut().write_message(SpellGoTargets {
            list: Vec::new(),
            target_mask: 0x2,
            src: None,
            caster,
            spell_id,
            hits: vec![target],
            misses: Vec::new(),
            dest: None,
            points: Vec::new(),
            ammo_display_id: ammo,
            seq: 1,
            trajectory: None,
            reflected: 0,
            reflects: Vec::new(),
            caster_guid: 0,
            server_time: 0,
        });
    }
    app.update();
    let spawns: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<MissileSpawn>>()
        .drain()
        .map(|m| (m.spell_id, m.awaits_release, m.path.clone()))
        .collect();
    let cube = Some(super::ERROR_CUBE.to_string());
    assert_eq!(
        spawns,
        vec![(ANIMATED, true, cube), (SILENT, false, None)],
        "deferred iff the cast kit animates; a missile row with no model flies the error cube; no missile and no ammo, no flight"
    );
}

/// A router app with one dest-targeted spell of this visual row, its kits and effect paths.
fn ground_app(
    spell_id: u32,
    speed: f32,
    stages: VisualStages,
    kits: HashMap<u32, VisualKit>,
    paths: HashMap<u32, String>,
) -> App {
    const VISUAL: u32 = 318;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
        HashMap::from([(VISUAL, stages)]),
        kits,
        paths,
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            spell_id,
            SpellDisplay {
                visual: VISUAL,
                speed,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);
    app
}

/// A Speed>0 GO with no hits but a ground point spawns one projectile at it, whose arrival rings
/// `SpellVisual` column 24's kit sound at the landing point.
#[test]
fn a_targetless_dest_go_spawns_a_ground_missile_whose_arrival_sounds_at_the_point() {
    const GROUND: u32 = 1543; // Flare's shape: speed>0, dest-targeted, empty hit list
    const AREA_KIT: u32 = 3270;
    const BOOM: u32 = 4100;
    let at = Vec3::new(11.0, 2.0, -3.0);
    let mut app = ground_app(
        GROUND,
        5.0,
        VisualStages {
            missile_gate: 1,
            impact_area_kit: AREA_KIT,
            ..Default::default()
        },
        HashMap::from([(
            AREA_KIT,
            VisualKit {
                sound: Some(BOOM),
                ..Default::default()
            },
        )]),
        HashMap::new(),
    );

    let caster = app.world_mut().spawn_empty().id();
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: 0x40,
        src: None,
        caster,
        spell_id: GROUND,
        hits: Vec::new(),
        misses: Vec::new(),
        dest: Some(at),
        points: Vec::new(),
        ammo_display_id: None,
        seq: 1,
        trajectory: None,
        reflected: 0,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();
    let spawns: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<MissileSpawn>>()
        .drain()
        .collect();
    assert_eq!(spawns.len(), 1, "one projectile at the point");
    assert_eq!(spawns[0].ground_aim, Some(at));
    assert!(spawns[0].targets.is_empty(), "no unit owns it");
    assert!(
        app.world_mut()
            .resource_mut::<Messages<crate::entities::dest_fx::GroundBurst>>()
            .drain()
            .next()
            .is_none(),
        "no instant area kit, no burst at GO: the impact area kit waits for the landing"
    );

    // The arrival the missile lane writes back.
    app.world_mut().write_message(CastEvent {
        entity: caster,
        spell_id: GROUND,
        kind: CastEventKind::GroundImpact { pos: at },
        cast_count: 0,
        seq: 2,
    });
    app.update();
    let sounds: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect();
    assert!(
        matches!(
            sounds[..],
            [SpellKitSound::PlayAt { pos, kit_sound }] if pos == at && kit_sound == BOOM
        ),
        "the area kit's sound, at the landing point: {sounds:?}"
    );
}

/// `SMSG_SPELL_GO` with a dest point plays column 23's kit there (`0x80ee85`): every effect model
/// once at the point and the sound there, missile or not.
#[test]
fn a_dest_go_plays_the_instant_area_kit_at_the_point() {
    const INSTANT_KIT: u32 = 1944;
    const SPARK: u32 = 4101;
    let at = Vec3::new(-4.0, 1.0, 9.0);
    let mut app = ground_app(
        2120,
        0.0,
        VisualStages {
            instant_area_kit: INSTANT_KIT,
            ..Default::default()
        },
        HashMap::from([(
            INSTANT_KIT,
            VisualKit {
                sound: Some(SPARK),
                // A base slot and a world effect: both land at the point.
                effect_slots: [None, None, Some(420), None, None, None, None, None, None],
                world_effect: Some(5721),
                ..Default::default()
            },
        )]),
        HashMap::from([
            (420, "Spells\\Flamestrike_Impact_Base.mdx".to_string()),
            (5721, "Spells\\Blizzard_Spawn.mdx".to_string()),
        ]),
    );
    let caster = app.world_mut().spawn_empty().id();
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: 0x40,
        src: None,
        caster,
        spell_id: 2120,
        hits: Vec::new(),
        misses: Vec::new(),
        dest: Some(at),
        points: Vec::new(),
        ammo_display_id: None,
        seq: 1,
        trajectory: None,
        reflected: 0,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();
    let bursts: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<crate::entities::dest_fx::GroundBurst>>()
        .drain()
        .map(|b| (b.path, b.pos))
        .collect();
    assert_eq!(
        bursts,
        [
            ("Spells\\Flamestrike_Impact_Base.mdx".to_string(), at),
            ("Spells\\Blizzard_Spawn.mdx".to_string(), at),
        ]
    );
    let sounds: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect();
    assert!(
        matches!(
            sounds[..],
            [SpellKitSound::PlayAt { pos, kit_sound }] if pos == at && kit_sound == SPARK
        ),
        "the kit sound at the point: {sounds:?}"
    );
}

/// A cast edge for a despawned unit neither panics the hold insert nor resurrects the unit, whether
/// it died in the wire drain applied before this chain, or `model_fade::apply_despawn_fade`,
/// unordered against it, queued the despawn first, which no queue-time check can see.
#[test]
fn a_despawned_subject_never_panics_the_router() {
    {
        let mut app = app();
        let unit = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(unit).despawn();
        app.world_mut()
            .write_message(cast_event(unit, SPELL, CastEventKind::Start));
        app.update(); // window 1: the unit is gone before the edge is read
        assert!(
            app.world().get_entity(unit).is_err(),
            "the hold write must not resurrect a dead subject"
        );
    }
    {
        // Window 2, the fade lane's shape: an ordering edge with no sync point, so both command
        // queues flush together and the despawn applies first.
        let mut app = app();
        let unit = app.world_mut().spawn_empty().id();
        app.add_systems(
            Update,
            (move |mut commands: Commands| {
                commands.entity(unit).try_despawn();
            })
            .before_ignore_deferred(route_cast_visuals),
        );
        app.world_mut()
            .write_message(cast_event(unit, SPELL, CastEventKind::Start));
        app.update();
        assert!(
            app.world().get_entity(unit).is_err(),
            "the same-frame despawn wins; the hold write is dropped"
        );
    }
}

/// The `0x400` weapon-visual hold: a ranged spell's visual play inserts [`RangedHold`] on any
/// caster, and a non-ranged play clears it.
#[test]
fn ranged_visual_play_arms_the_any_caster_hold_and_a_non_ranged_play_clears_it() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, RANGED_SPELL, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "a ranged GO's visual play sets the hold"
    );

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_none(),
        "a non-ranged visual play clears the hold"
    );

    // A ranged START re-arms it too, but this chain's precast stage is empty, so the GO re-arms it.
    app.world_mut()
        .write_message(cast_event(unit, RANGED_SPELL, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "the next ranged play re-arms"
    );
}

/// The mount poof, as the reference gives it: only the build leg allocates it, behind a test on the
/// new value, so a dismount is silent; any changed value puffs; and a unit streaming in already
/// mounted does not.
#[test]
fn the_mount_poof_puffs_on_the_build_leg_only() {
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_MOUNTDISPLAYID` (index 133).
    const FIELD_MOUNTDISPLAYID: u16 = 133;
    /// The shipped path of `SpellVisualEffectName` row 1185, the druid-morph cloud.
    const POOF: &str = "Spells\\DruidMorph_Impact_Base.mdx";
    const POOF_FX: u32 = 1185;
    /// The M2 attach the hardcoded-effect spawn stamps.
    const BASE_ATTACH: u16 = 0x13;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>();
    app.add_message::<crate::net::FieldChanged>();
    app.insert_resource(SpellVisuals(
        SpellVisualCatalog::from_tables(HashMap::new(), HashMap::new()).with_hardcoded(
            "HARDCODED Mount Poof",
            POOF_FX,
            POOF,
        ),
    ));
    app.add_systems(Update, super::arm_mount_poof_fx);

    let fields = |v: u32| ObjectFields::from_pairs(&[(FIELD_MOUNTDISPLAYID, v)]);
    // Streams in already mounted: a create block is no edge.
    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            fields(2404).into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();
    let puffs = |app: &mut App| -> Vec<super::FxSlot> {
        let mut out = Vec::new();
        let world = app.world_mut();
        let mut msgs = world.resource_mut::<Messages<SpellKitFx>>();
        for m in msgs.drain() {
            if let SpellKitFx::Begin {
                entity, effects, ..
            } = m
            {
                assert_eq!(entity, unit);
                out.extend(effects);
            }
        }
        out
    };
    assert!(
        puffs(&mut app).is_empty(),
        "a rider that streams into view did not just mount"
    );

    // Dismount: the new value is 0, so the build leg is skipped.
    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(0));
    app.update();
    assert!(puffs(&mut app).is_empty(), "no poof on the way down");

    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(2404));
    app.update();
    assert_eq!(
        puffs(&mut app),
        vec![super::FxSlot {
            tag: BASE_ATTACH,
            effect: POOF_FX,
            path: POOF.to_string(),
            scale: 1.0,
            attach: None,
            own_alpha: false,
            kit_anim: None,
        }],
        "the build leg puffs the druid-morph cloud at the base attach"
    );

    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(2404));
    app.update();
    assert!(puffs(&mut app).is_empty(), "no re-puff while just riding");

    // A swap (N → N′) is a change: the reference rebuilds and puffs again.
    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(2405));
    app.update();
    assert_eq!(
        puffs(&mut app),
        vec![super::FxSlot {
            tag: BASE_ATTACH,
            effect: POOF_FX,
            path: POOF.to_string(),
            scale: 1.0,
            attach: None,
            own_alpha: false,
            kit_anim: None,
        }]
    );
}

/// A kit with a chain `CharProc` emits a `ChainProcPlay` from both dispatcher sites: the kit
/// player's tail (the cast release) and the channel poll, the only way a channelled beam is
/// reached.
#[test]
fn a_kit_with_a_chain_char_proc_asks_for_a_beam_from_both_dispatcher_sites() {
    use benilla_formats::{char_proc_type, CharProc};

    const BEAM_SPELL: u32 = 421; // Chain Lightning
    const BEAM_VISUAL: u32 = 36;
    const BEAM_CAST_KIT: u32 = 321;
    const BEAM_CHANNEL_KIT: u32 = 402;

    let mut app = app();
    // A cast kit with a type-12 proc and a channel kit with a type-0 one, in the shipped shape:
    // chain id 1, one strand, and the flag that splits cast (0) from channel (1).
    let mut visuals = HashMap::from([(
        BEAM_VISUAL,
        VisualStages {
            cast: BEAM_CAST_KIT,
            channel: BEAM_CHANNEL_KIT,
            ..Default::default()
        },
    )]);
    let mut kits = HashMap::new();
    for (kit, ty, flag) in [
        (BEAM_CAST_KIT, char_proc_type::CHAIN_CAST, 0.0),
        (BEAM_CHANNEL_KIT, char_proc_type::CHAIN_CHANNEL, 1.0),
    ] {
        kits.insert(
            kit,
            VisualKit {
                char_proc_slots: [
                    Some(CharProc {
                        ty,
                        params: [1.0, 1.0, flag, 0.0],
                    }),
                    None,
                    None,
                    None,
                ],
                ..Default::default()
            },
        );
    }
    // A beam-less kit, the control.
    kits.insert(PRECAST_KIT, VisualKit::default());
    visuals.insert(
        VISUAL,
        VisualStages {
            cast: PRECAST_KIT,
            ..Default::default()
        },
    );
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(visuals, kits)));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                BEAM_SPELL,
                SpellDisplay {
                    visual: BEAM_VISUAL,
                    ..Default::default()
                },
            ),
            (
                SPELL,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });

    let plays = |app: &mut App| -> Vec<(Entity, u32, bool)> {
        app.world_mut()
            .resource_mut::<Messages<super::ChainProcPlay>>()
            .drain()
            .map(|p| (p.entity, p.spell_id, p.proc.flag))
            .collect()
    };

    // Site 1, the cast release.
    let unit = app.world_mut().spawn_empty().id();
    app.world_mut()
        .write_message(cast_event(unit, BEAM_SPELL, CastEventKind::Go));
    app.update();
    assert_eq!(
        plays(&mut app),
        vec![(unit, BEAM_SPELL, false)],
        "the cast kit's type-12 proc asks for a one-shot beam"
    );

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Go));
    app.update();
    assert!(plays(&mut app).is_empty(), "a beam-less kit stays silent");

    // Site 2, the channel poll, on the rising edge of `UNIT_CHANNEL_SPELL`: the only play of a
    // channel kit.
    let channeller = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(144, BEAM_SPELL)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();
    assert_eq!(
        plays(&mut app),
        vec![(channeller, BEAM_SPELL, true)],
        "the channel kit's type-0 proc asks for a persistent beam"
    );
}

// ── The shipped-table shooter ────────────────────────────────────────────────────────────────
//
// The tests above use bare units, for which `WeaponVisualSrc::caster` (a player's ranged weapon →
// its substitute visual) is always `None`. These run the shipped DBCs, a self-player and a real
// `item_template` row: one `CastEvent` in, the body's clip out.

/// Auto Shot, one `SMSG_SPELL_GO` per shot. Its shipped row: `Attributes` 0x50012 (`& 0x2` set),
/// `AttributesEx2` 0x20 (auto-repeat), Speed 40 and `SpellVisual1` 0, so every clip it plays comes
/// from the weapon's substitute visual through [`super::resolve_stages`]'s merge.
const AUTO_SHOT: u32 = 75;

/// A ranged weapon's vmangos `item_template` row; the display → `ItemDisplayInfo` col 10 →
/// `SpellVisual` → kit → anim chain is re-derived from the shipped DBCs by the test.
struct RealRanged {
    name: &'static str,
    entry: u32,
    display_id: u32,
    /// `ItemClass` 2 (weapon) and its subclass: 2 bow, 3 gun, 18 crossbow.
    class: u32,
    subclass: u32,
    /// The pull, from the weapon visual's precast kit: LoadBow 105 or LoadRifle 106.
    load_anim: u16,
    /// The release, from its cast kit: AttackBow 46 or AttackRifle 49.
    fire_anim: u16,
}

/// A bow (`item_template` 2504, display 8106 → visual 5 → kits 7/164 → 105/46).
const WORN_SHORTBOW: RealRanged = RealRanged {
    name: "Worn Shortbow",
    entry: 2504,
    display_id: 8106,
    class: 2,
    subclass: 2,
    load_anim: 105,
    fire_anim: 46,
};

/// A gun (`item_template` 2508, display 6606 → visual 224 → kits 161/167 → 106/49).
const OLD_BLUNDERBUSS: RealRanged = RealRanged {
    name: "Old Blunderbuss",
    entry: 2508,
    display_id: 6606,
    class: 2,
    subclass: 3,
    load_anim: 106,
    fire_anim: 49,
};

/// A crossbow (`item_template` 12651, display 22929 → visual 743 → kits 803/804 → 106/49):
/// crossbows share the rifle clips.
const BLACKCROW: RealRanged = RealRanged {
    name: "Blackcrow",
    entry: 12651,
    display_id: 22929,
    class: 2,
    subclass: 18,
    load_anim: 106,
    fire_anim: 49,
};

/// `PLAYER_VISIBLE_ITEM_18_0`, the item entry in equipment slot 17 (vmangos
/// `EQUIPMENT_SLOT_RANGED`): `PLAYER_VISIBLE_ITEM_1_CREATOR` (258) + 2 + 12 × 17, spelled out as
/// the base is private to `benilla-protocol`.
const VISIBLE_RANGED_ENTRY_FIELD: u16 = 258 + 2 + 12 * 17;

/// Keeps the item layer's ask-once channel alive, so an unexpected `ItemQuery` (a template not
/// landed, which starves the weapon lookup) is observable.
#[derive(Resource)]
struct AskLog(crossbeam_channel::Receiver<crate::net::ClientCommand>);

/// The cast router over the shipped tables, with a self-player wearing `weapon` in the ranged slot
/// and its template landed, as it is by the time anything shoots; `None` without an install.
fn real_shooter(weapon: &RealRanged) -> Option<(App, Entity)> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open the install's MPQ chain");
    let visuals =
        benilla_formats::load_spell_visual_catalog(&mut chain).expect("SpellVisual/SpellVisualKit");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
    let displays =
        benilla_formats::load_item_display_catalog(&mut chain).expect("ItemDisplayInfo.dbc");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(visuals));
    app.insert_resource(crate::ui_action::Spells {
        catalog: spells,
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.insert_resource(crate::entities::ItemDisplays::icons_for_tests(displays));

    let mut items = crate::items::Items::default();
    let mut template = crate::items::test_template(weapon.name);
    template.class = weapon.class;
    template.subclass = weapon.subclass;
    template.display_info_id = weapon.display_id;
    template.inventory_type = 15; // INVTYPE_RANGED, the real row's; unread by this chain
    items.insert_template(weapon.entry, Some(template));
    app.insert_resource(items);

    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(AskLog(rx));

    // Our own body, a player, with the weapon's entry in the visible-item field.
    let unit = app
        .world_mut()
        .spawn((
            crate::net::SelfPlayer,
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Player,
                display_id: Some(49), // HumanMale, which authors 46/49/105/106
                scale: 1.0,
            },
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                VISIBLE_RANGED_ENTRY_FIELD,
                weapon.entry,
            )])),
        ))
        .id();
    app.add_systems(Update, route_cast_visuals);
    Some((app, unit))
}

/// The one-shot clips this frame asked of `unit`'s body.
fn emote_anims(app: &mut App, unit: Entity) -> Vec<u16> {
    app.world_mut()
        .resource_mut::<Messages<EmoteAnim>>()
        .drain()
        .filter(|e| e.entity == unit)
        .map(|e| e.anim_id)
        .collect()
}

/// The entries the item layer asked the server for; any means the weapon lookup starved.
fn asked_entries(app: &App) -> Vec<u32> {
    app.world()
        .resource::<AskLog>()
        .0
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::ItemQuery { entry, .. } => Some(entry),
            _ => None,
        })
        .collect()
}

/// Auto Shot's GO with a bow equipped reaches the body as AttackBow (46): slot 17's template →
/// display 8106 → `ItemDisplayInfo` col 10 (visual 5) → the merge → cast kit 164 → anim 46.
#[test]
fn a_real_bow_shooters_auto_shot_go_plays_attackbow() {
    let Some((mut app, unit)) = real_shooter(&WORN_SHORTBOW) else {
        return;
    };
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
    app.update();
    assert_eq!(
        emote_anims(&mut app, unit),
        vec![WORN_SHORTBOW.fire_anim],
        "Auto Shot's GO plays the bow's release clip (asked the server for {:?})",
        asked_entries(&app)
    );
    assert!(
        asked_entries(&app).is_empty(),
        "the weapon template was already landed — no ask-once miss starved the lookup"
    );
}

/// A gun and a crossbow take the same road to AttackRifle (49), through visuals 224 and 743.
#[test]
fn a_real_gun_or_crossbow_shooters_auto_shot_go_plays_attackrifle() {
    for weapon in [&OLD_BLUNDERBUSS, &BLACKCROW] {
        let Some((mut app, unit)) = real_shooter(weapon) else {
            return;
        };
        app.world_mut()
            .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
        app.update();
        assert_eq!(
            emote_anims(&mut app, unit),
            vec![weapon.fire_anim],
            "{}'s Auto Shot GO plays its release clip",
            weapon.name
        );
    }
}

/// Auto Shot's START, once per auto-repeat activation, arms the LoadBow (105) pull as the cast
/// hold, with [`RangedHold`] and the ranged sheath snap; the GO releases it and fires.
#[test]
fn a_real_bow_shooters_auto_shot_start_arms_the_loadbow_hold() {
    let Some((mut app, unit)) = real_shooter(&WORN_SHORTBOW) else {
        return;
    };
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Start));
    app.update();
    let held = app.world().entity(unit).get::<CastHold>();
    assert_eq!(
        held.map(|h| (h.anim_id, h.spell_id, h.ranged)),
        Some((WORN_SHORTBOW.load_anim, AUTO_SHOT, true)),
        "START arms the bow's pull as the cast hold"
    );
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "…and the any-caster `0x400` weapon-visual hold"
    );
    let sheaths: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SheathRequest>>()
        .drain()
        .filter(|s| s.entity == unit)
        .map(|s| s.state)
        .collect();
    assert_eq!(sheaths, vec![2], "…and the ranged stance snaps drawn");

    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<CastHold>().is_none(),
        "the GO releases the pull"
    );
    assert_eq!(
        emote_anims(&mut app, unit),
        vec![WORN_SHORTBOW.fire_anim],
        "…and plays the release"
    );
}

/// The control: with the ranged slot empty Auto Shot resolves no clip, having no visual of its own.
#[test]
fn a_shooter_with_no_ranged_weapon_resolves_no_clip_at_all() {
    let Some((mut app, unit)) = real_shooter(&WORN_SHORTBOW) else {
        return;
    };
    // Slot 17 empty, as the wire says it.
    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        benilla_protocol::ObjectFields::from_pairs(&[(VISIBLE_RANGED_ENTRY_FIELD, 0)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Start));
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
    app.update();
    assert!(
        emote_anims(&mut app, unit).is_empty(),
        "no weapon, no substitute visual, no clip"
    );
    assert!(
        app.world().entity(unit).get::<CastHold>().is_none(),
        "and no pull to hold"
    );
}

/// An instant hit plays the impact kits alone (`0x801f10`): Charge (22911) plays Knockdown (121)
/// from impact kit 348, and state kit 349's Stun (14) is neither a second play nor a recompute; it
/// comes with the aura's own state kit.
#[test]
fn an_instant_hit_plays_its_impact_kit_and_no_state_kit() {
    const CHARGE: u32 = 22911;
    const CHARGE_VISUAL: u32 = 3783;
    const IMPACT_KIT: u32 = 348;
    const STATE_KIT: u32 = 349;
    const KNOCKDOWN: u16 = 121;
    const STUN: u16 = 14;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            CHARGE_VISUAL,
            VisualStages {
                impact: IMPACT_KIT,
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([
            (
                IMPACT_KIT,
                VisualKit {
                    anim_id: Some(KNOCKDOWN),
                    ..Default::default()
                },
            ),
            (
                STATE_KIT,
                VisualKit {
                    anim_id: Some(STUN),
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            CHARGE,
            SpellDisplay {
                visual: CHARGE_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);

    let caster = app.world_mut().spawn_empty().id();
    let victim = struck_unit(app.world_mut(), 1, &[(0, CHARGE, 1)]);
    app.world_mut()
        .write_message(cast_event(caster, CHARGE, CastEventKind::Go));
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: 0x2,
        src: None,
        caster,
        spell_id: CHARGE,
        hits: vec![victim],
        misses: Vec::new(),
        dest: None,
        points: Vec::new(),
        ammo_display_id: None,
        seq: 1,
        trajectory: None,
        reflected: 0,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();

    let plays: Vec<(Entity, u16)> = app
        .world_mut()
        .resource_mut::<Messages<EmoteAnim>>()
        .drain()
        .map(|e| (e.entity, e.anim_id))
        .collect();
    assert_eq!(
        plays,
        vec![(victim, KNOCKDOWN)],
        "the impact kit plays; the state kit must not add a second one-shot"
    );
    let recomputes: Vec<(Entity, u16)> = app
        .world_mut()
        .resource_mut::<Messages<super::BaseAnimRecompute>>()
        .drain()
        .map(|r| (r.entity, r.anim_id))
        .collect();
    assert!(
        recomputes.is_empty(),
        "no state kit plays at the instant hit, not even its Stun ({STUN}) compare"
    );
}

/// A unit at `health` holding `auras` as `(slot, spell, flags)`.
fn struck_unit(world: &mut World, health: u32, auras: &[(u8, u32, u8)]) -> Entity {
    use benilla_protocol::field::FIELD_UNIT_HEALTH;
    let store = benilla_protocol::ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, health)])
        .into_created(benilla_protocol::messages::ObjectType::Unit);
    world
        .spawn((
            crate::net::ObjectStore(store),
            crate::unit_auras::UnitAuras::of(0, auras),
        ))
        .id()
}

/// An anim-only state kit still arms, and its add edge owes a recompute: 15 shipped state kits are
/// anim-only (`benilla-extract kitanim`), kit 586's Stun (14) among them.
#[test]
fn an_anim_only_state_kit_still_arms_on_the_aura_add_edge() {
    const SPELL: u32 = 6902; // Sneezing Fit
    const VISUAL: u32 = 517;
    const STATE_KIT: u32 = 586; // anim 14, no effect slot, no sound, no CharProc
    const STUN: u16 = 14;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<crate::aura_visual::AuraProc>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .init_resource::<crate::net::GuidIndex>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<SpellKitSound>()
        .add_message::<super::BaseAnimRecompute>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            VISUAL,
            VisualStages {
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            STATE_KIT,
            VisualKit {
                anim_id: Some(STUN),
                ..Default::default()
            },
        )]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SPELL,
            SpellDisplay {
                visual: VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, super::arm_aura_state_fx);

    let unit = app
        .world_mut()
        .spawn(crate::unit_auras::aura_unit_for_test(&[SPELL]))
        .id();
    app.update();

    let recomputes: Vec<(Entity, u16)> = app
        .world_mut()
        .resource_mut::<Messages<super::BaseAnimRecompute>>()
        .drain()
        .map(|r| (r.entity, r.anim_id))
        .collect();
    assert_eq!(
        recomputes,
        vec![(unit, STUN)],
        "an anim-only state kit is not nothing — its ADD edge owes a recompute"
    );
}

/// A stealth-detection aura's state kit arms only while our own `PLAYER_FIELD_BYTES2` byte 3
/// carries the stealth vision (`0x724979`), and gaining or losing it re-arms or reaps the kit on
/// a unit whose auras did not change (`0x727a70`).
#[test]
fn a_detection_aura_s_state_kit_follows_our_own_aura_vision() {
    const SPELL: u32 = 1;
    const VISUAL: u32 = 2;
    const STATE_KIT: u32 = 3;
    const ANIM: u16 = 14;
    const MOD_STEALTH_DETECT: u32 = 17;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<crate::aura_visual::AuraProc>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .init_resource::<crate::net::GuidIndex>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<SpellKitSound>()
        .add_message::<super::BaseAnimRecompute>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            VISUAL,
            VisualStages {
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            STATE_KIT,
            VisualKit {
                anim_id: Some(ANIM),
                ..Default::default()
            },
        )]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SPELL,
            SpellDisplay {
                visual: VISUAL,
                effect_apply_aura: [MOD_STEALTH_DETECT, 0, 0],
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, super::arm_aura_state_fx);

    let vision = |byte3: u32| {
        crate::net::ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_PLAYER_FIELD_BYTES2,
            byte3 << 24,
        )]))
    };
    let me = app
        .world_mut()
        .spawn((crate::net::SelfPlayer, vision(0)))
        .id();
    let guard = app
        .world_mut()
        .spawn(crate::unit_auras::aura_unit_for_test(&[SPELL]))
        .id();
    let step = |app: &mut App| {
        app.update();
        let armed: Vec<Entity> = app
            .world_mut()
            .resource_mut::<Messages<super::BaseAnimRecompute>>()
            .drain()
            .map(|r| r.entity)
            .collect();
        let reaped: Vec<Entity> = app
            .world_mut()
            .resource_mut::<Messages<crate::aura_visual::AuraProc>>()
            .drain()
            .filter_map(|p| match p {
                crate::aura_visual::AuraProc::Reap { entity, .. } => Some(entity),
                _ => None,
            })
            .collect();
        (armed, reaped)
    };

    assert_eq!(
        step(&mut app),
        (vec![], vec![]),
        "hidden without stealth vision"
    );
    app.world_mut().entity_mut(me).insert(vision(0x20));
    assert_eq!(
        step(&mut app),
        (vec![guard], vec![]),
        "shown once we see stealth"
    );
    app.world_mut().entity_mut(me).insert(vision(0x40));
    assert_eq!(
        step(&mut app),
        (vec![], vec![guard]),
        "reaped when the vision goes"
    );
}

/// `SMSG_ACHIEVEMENT_EARNED`'s effect gates (`0x5b30c5`-`0x5b30d3`): silent, an unknown row, and a
/// row with flag `0x4` each skip it.
#[test]
fn the_earn_effect_plays_only_for_a_loud_known_row_without_flag_4() {
    use benilla_ui::script::{AchievementDb, AchievementRow};
    let row = |id, flags| AchievementRow {
        id,
        flags,
        ..Default::default()
    };
    let db = AchievementDb::new(vec![row(6, 0), row(7, 0x4)], Vec::new(), Vec::new());
    assert!(super::earn_effect_plays(Some(&db), 6, false));
    assert!(!super::earn_effect_plays(Some(&db), 6, true), "silent");
    assert!(!super::earn_effect_plays(Some(&db), 7, false), "flag 0x4");
    assert!(
        !super::earn_effect_plays(Some(&db), 99, false),
        "unknown row"
    );
    assert!(!super::earn_effect_plays(None, 6, false), "no tables");
}

/// Any player's earn arms the one-shot at attachment `0x13` on that player's entity.
#[test]
fn an_earn_arms_the_achievement_base_on_the_earning_player() {
    use benilla_protocol::messages::AchievementEarned;
    use benilla_ui::script::{AchievementDb, AchievementRow};
    const FX: u32 = 4000;
    const PATH: &str = "Spells/AchievementBase.mdx";
    let mut app = App::new();
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>();
    app.insert_resource(SpellVisuals(
        SpellVisualCatalog::from_tables(HashMap::new(), HashMap::new()).with_hardcoded(
            "HARDCODED Achievement Base",
            FX,
            PATH,
        ),
    ));
    let db = AchievementDb::new(
        vec![AchievementRow {
            id: 6,
            ..Default::default()
        }],
        Vec::new(),
        Vec::new(),
    );
    app.insert_resource(crate::ui_achievement::Achievements(std::sync::Arc::new(db)));
    let unit = app.world_mut().spawn_empty().id();
    let mut index = crate::net::GuidIndex::default();
    index.0.insert(0x55, unit);
    app.insert_resource(index);
    let earn = |silent| {
        benilla_protocol::SessionEvent::AchievementEarned(AchievementEarned {
            player: 0x55,
            achievement: 6,
            date: 0,
            silent,
        })
    };
    let world = app.world_mut();
    world
        .run_system_cached_with(super::super::net::on_achievement_earned, earn(true))
        .unwrap();
    world
        .run_system_cached_with(super::super::net::on_achievement_earned, earn(false))
        .unwrap();
    let mut msgs = world.resource_mut::<Messages<SpellKitFx>>();
    let begins: Vec<_> = msgs
        .drain()
        .filter_map(|m| match m {
            SpellKitFx::Begin {
                entity, effects, ..
            } => Some((entity, effects)),
            _ => None,
        })
        .collect();
    assert_eq!(begins.len(), 1, "the silent earn plays nothing");
    assert_eq!(begins[0].0, unit);
    assert_eq!(begins[0].1[0].tag, 0x13);
    assert_eq!(begins[0].1[0].effect, FX);
}

/// Column 24 at `SMSG_SPELL_GO` (`0x80ef23`): played at the dest point at Speed 0, deferred to the
/// would-be landing at Speed > 0 with no `HasMissile`, left to the landing with one, and never for
/// a row whose kits draw a beam.
#[test]
fn a_dest_go_plays_the_impact_area_kit_unless_a_missile_or_a_beam_carries_it() {
    const AREA_KIT: u32 = 3270;
    const BEAM_KIT: u32 = 3271;
    const BOOM: u32 = 4100;
    let at = Vec3::new(3.0, 0.0, 4.0);
    let kits = || {
        HashMap::from([
            (
                AREA_KIT,
                VisualKit {
                    sound: Some(BOOM),
                    ..Default::default()
                },
            ),
            (
                BEAM_KIT,
                VisualKit {
                    char_proc_slots: [
                        Some(benilla_formats::CharProc {
                            ty: benilla_formats::char_proc_type::CHAIN_CAST,
                            params: [743.0, 0.0, 0.0, 0.0],
                        }),
                        None,
                        None,
                        None,
                    ],
                    ..Default::default()
                },
            ),
        ])
    };
    let go = |app: &mut App| {
        let caster = app.world_mut().spawn(GlobalTransform::default()).id();
        app.world_mut().write_message(SpellGoTargets {
            list: Vec::new(),
            target_mask: 0x40,
            src: None,
            caster,
            spell_id: 1,
            hits: Vec::new(),
            misses: Vec::new(),
            dest: Some(at),
            points: Vec::new(),
            ammo_display_id: None,
            seq: 1,
            trajectory: None,
            reflected: 0,
            reflects: Vec::new(),
            caster_guid: 0,
            server_time: 0,
        });
        app.update();
        caster
    };
    let booms = |app: &mut App| -> usize {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter(|s| {
                matches!(s, SpellKitSound::PlayAt { pos, kit_sound } if *pos == at && *kit_sound == BOOM)
            })
            .count()
    };
    let delayed = |app: &mut App| -> Vec<super::DelayedAreaKit> {
        app.world_mut()
            .resource_mut::<Messages<super::DelayedAreaKit>>()
            .drain()
            .collect()
    };
    let area = VisualStages {
        impact_area_kit: AREA_KIT,
        ..Default::default()
    };

    let mut app = ground_app(1, 0.0, area, kits(), HashMap::new());
    go(&mut app);
    assert_eq!(booms(&mut app), 1, "Speed 0 plays it at GO");

    let beam_row = VisualStages {
        cast: BEAM_KIT,
        ..area
    };
    let mut app = ground_app(1, 0.0, beam_row, kits(), HashMap::new());
    go(&mut app);
    assert_eq!(booms(&mut app), 0, "a beam row skips it (0x800bf0)");

    let mut app = ground_app(1, 8.0, area, kits(), HashMap::new());
    let caster = go(&mut app);
    assert_eq!(booms(&mut app), 0, "Speed > 0 defers it");
    assert_eq!(
        delayed(&mut app),
        [super::DelayedAreaKit {
            kit_id: AREA_KIT,
            pos: at,
            wait: super::AreaKitWait::Flight {
                caster,
                speed: 8.0,
                to: at
            },
            deck: None,
        }]
    );

    let missile_row = VisualStages {
        missile_gate: 1,
        ..area
    };
    let mut app = ground_app(1, 8.0, missile_row, kits(), HashMap::new());
    go(&mut app);
    assert_eq!(booms(&mut app), 0, "HasMissile leaves it to the landing");
    assert!(delayed(&mut app).is_empty());
}

/// The deferred area kit waits distance / speed from the caster, and plays at once without one.
#[test]
fn a_delayed_area_kit_waits_the_missile_flight() {
    assert_eq!(
        super::area_kit_delay(Some(Vec3::ZERO), Vec3::new(3.0, 0.0, 4.0), 10.0),
        0.5
    );
    assert_eq!(super::area_kit_delay(None, Vec3::ONE, 10.0), 0.0);
}

/// Each extra point takes the instant area kit and, with no missile to carry it, the impact area
/// kit, as the dest does; at Speed > 0 every one waits for the last point's flight and a
/// transport's point plays on its transport (`0x80f1cc`, `0x80e978`).
#[test]
fn a_go_plays_the_area_kits_at_each_extra_point() {
    const INSTANT_KIT: u32 = 1944;
    const AREA_KIT: u32 = 3270;
    const SPARK: u32 = 4101;
    const BOOM: u32 = 4100;
    let dest = Vec3::new(3.0, 0.0, 4.0);
    let (p, local, on_deck) = (
        Vec3::new(6.0, 0.0, 8.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 30.0),
    );
    let stages = VisualStages {
        instant_area_kit: INSTANT_KIT,
        impact_area_kit: AREA_KIT,
        ..Default::default()
    };
    let kits = || {
        HashMap::from([
            (
                INSTANT_KIT,
                VisualKit {
                    sound: Some(SPARK),
                    ..Default::default()
                },
            ),
            (
                AREA_KIT,
                VisualKit {
                    sound: Some(BOOM),
                    ..Default::default()
                },
            ),
        ])
    };
    let go = |app: &mut App| {
        let caster = app.world_mut().spawn(GlobalTransform::default()).id();
        let deck = app.world_mut().spawn_empty().id();
        let point = |at, deck, world| crate::entities::DeckPoint { at, deck, world };
        app.world_mut().write_message(SpellGoTargets {
            list: Vec::new(),
            target_mask: 0x80040,
            src: None,
            caster,
            spell_id: 1,
            hits: Vec::new(),
            misses: Vec::new(),
            dest: Some(dest),
            points: vec![point(p, None, p), point(local, Some(deck), on_deck)],
            ammo_display_id: None,
            seq: 1,
            trajectory: None,
            reflected: 0,
            reflects: Vec::new(),
            caster_guid: 0,
            server_time: 0,
        });
        app.update();
        (caster, deck)
    };
    let sounds = |app: &mut App| -> Vec<(u32, Vec3)> {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter_map(|s| match s {
                SpellKitSound::PlayAt { pos, kit_sound } => Some((kit_sound, pos)),
                _ => None,
            })
            .collect()
    };

    let mut app = ground_app(1, 0.0, stages, kits(), HashMap::new());
    go(&mut app);
    assert_eq!(
        sounds(&mut app),
        vec![
            (SPARK, dest),
            (BOOM, dest),
            (SPARK, p),
            (BOOM, p),
            (SPARK, on_deck),
            (BOOM, on_deck),
        ]
    );

    let mut app = ground_app(1, 8.0, stages, kits(), HashMap::new());
    let (caster, deck) = go(&mut app);
    assert_eq!(
        sounds(&mut app),
        vec![(SPARK, dest), (SPARK, p), (SPARK, on_deck)]
    );
    let wait = super::AreaKitWait::Flight {
        caster,
        speed: 8.0,
        to: on_deck,
    };
    let delayed: Vec<super::DelayedAreaKit> = app
        .world_mut()
        .resource_mut::<Messages<super::DelayedAreaKit>>()
        .drain()
        .collect();
    let kit = |pos, deck| super::DelayedAreaKit {
        kit_id: AREA_KIT,
        pos,
        wait,
        deck,
    };
    assert_eq!(
        delayed,
        vec![
            kit(dest, None),
            kit(p, None),
            kit(on_deck, Some((deck, local)))
        ]
    );
}

/// A delayed area kit on a transport plays where the transport is when it comes due, and at its
/// place at the GO once the transport is gone.
#[test]
fn a_delayed_area_kit_on_a_transport_plays_where_it_has_moved() {
    const AREA_KIT: u32 = 3270;
    const BOOM: u32 = 4100;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_message::<super::DelayedAreaKit>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
            HashMap::new(),
            HashMap::from([(
                AREA_KIT,
                VisualKit {
                    sound: Some(BOOM),
                    ..Default::default()
                },
            )]),
            HashMap::new(),
        )))
        .add_systems(Update, super::play_delayed_area_kits);
    let deck = app
        .world_mut()
        .spawn(GlobalTransform::from_xyz(0.0, 0.0, 50.0))
        .id();
    let local = Vec3::new(2.0, 0.0, 0.0);
    let at_go = Vec3::new(2.0, 0.0, 10.0);
    for deck in [Some((deck, local)), Some((Entity::PLACEHOLDER, local))] {
        app.world_mut().write_message(super::DelayedAreaKit {
            kit_id: AREA_KIT,
            pos: at_go,
            wait: super::AreaKitWait::Secs(0.0),
            deck,
        });
    }
    app.update();
    let played: Vec<Vec3> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .filter_map(|s| match s {
            SpellKitSound::PlayAt { pos, .. } => Some(pos),
            _ => None,
        })
        .collect();
    assert_eq!(played, vec![Vec3::new(2.0, 0.0, 50.0), at_go]);
}

/// The beam test reads the cast, impact, state, channel and the two impact-kit columns, never
/// precast or state-done, and keys on a type-0 or type-12 slot whatever its params (`0x7fe470`).
#[test]
fn has_chain_effect_asks_six_stage_kits() {
    let beam = VisualKit {
        char_proc_slots: [
            None,
            Some(benilla_formats::CharProc {
                ty: benilla_formats::char_proc_type::CHAIN_CHANNEL,
                params: [0.0; 4],
            }),
            None,
            None,
        ],
        ..Default::default()
    };
    let cat = SpellVisualCatalog::from_tables(HashMap::new(), HashMap::from([(7, beam)]));
    let row = |f: fn(&mut VisualStages)| {
        let mut s = VisualStages::default();
        f(&mut s);
        cat.has_chain_effect(&s)
    };
    assert!(row(|s| s.cast = 7));
    assert!(row(|s| s.impact = 7));
    assert!(row(|s| s.state = 7));
    assert!(row(|s| s.channel = 7));
    assert!(row(|s| s.caster_impact = 7));
    assert!(row(|s| s.target_impact = 7));
    assert!(!row(|s| s.precast = 7));
    assert!(!row(|s| s.state_done = 7));
}

/// `0x70d0f1`: object scale times model height times 1.15, clamped to 1..5.
#[test]
fn the_go_loot_sparkle_scales_to_its_object() {
    assert!((super::go_loot_fx_scale(1.0, 2.0) - 2.3).abs() < 1e-6);
    assert_eq!(super::go_loot_fx_scale(1.0, 0.2), 1.0, "floor 1");
    assert_eq!(super::go_loot_fx_scale(2.0, 10.0), 5.0, "ceiling 5");
    assert_eq!(super::go_loot_fx_scale(1.0, f32::NAN), 1.0);
}

/// The GameObject sparkle arms on a flip of `GAMEOBJECT_DYNAMIC` bit `0x08` once the model has
/// loaded, re-arms at model load, ignores other bits and reaps with the bit.
#[test]
fn a_go_loot_sparkle_follows_dynamic_bit_8_and_the_model_load() {
    use benilla_protocol::field::FIELD_GAMEOBJECT_DYN_FLAGS;
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::ObjectFields;
    const LOOT_FX: u32 = 14;
    const LOOT: &str = "Particles\\LootFX.mdl";
    assert!(super::go_sparkle_edge(0, 0x8));
    assert!(!super::go_sparkle_edge(0x8, 0x9));

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>();
    app.insert_resource(SpellVisuals(
        SpellVisualCatalog::from_tables(HashMap::new(), HashMap::new()).with_hardcoded(
            "HARDCODED Loot Art",
            LOOT_FX,
            LOOT,
        ),
    ));
    app.add_systems(Update, super::arm_go_loot_fx);
    let dyn_flags = |v: u32| ObjectFields::from_pairs(&[(FIELD_GAMEOBJECT_DYN_FLAGS, v)]);
    let go = app
        .world_mut()
        .spawn((
            crate::net::ObjectStore(dyn_flags(0).into_created(ObjectType::GameObject)),
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::GameObject,
                display_id: Some(1),
                scale: 1.0,
            },
            crate::net::Guid(0xF110_0000_0000_0001),
        ))
        .id();
    app.update();
    let drain = |app: &mut App| -> Vec<SpellKitFx> {
        app.world_mut()
            .resource_mut::<Messages<SpellKitFx>>()
            .drain()
            .collect()
    };
    drain(&mut app);

    crate::net::apply_fields_for_test(app.world_mut(), go, dyn_flags(0x8));
    app.update();
    assert!(drain(&mut app).is_empty(), "no model yet (0x70d0af)");

    // The model loads: its bbox Z extent of 2 yards sizes the sparkle.
    app.world_mut()
        .entity_mut(go)
        .insert(crate::entities::OverheadFallback(2.0));
    app.update();
    let armed = drain(&mut app);
    match &armed[..] {
        [SpellKitFx::Begin {
            entity, effects, ..
        }] => {
            assert_eq!(*entity, go);
            assert_eq!(effects.len(), 1);
            assert_eq!((effects[0].tag, effects[0].effect), (0x13, LOOT_FX));
            assert!((effects[0].scale - 2.3).abs() < 1e-6);
        }
        other => panic!("one sparkle arms at model load, got {}", other.len()),
    }

    crate::net::apply_fields_for_test(app.world_mut(), go, dyn_flags(0x9));
    app.update();
    assert!(drain(&mut app).is_empty(), "bit 0x08 did not flip");

    crate::net::apply_fields_for_test(app.world_mut(), go, dyn_flags(0x1));
    app.update();
    assert!(
        matches!(drain(&mut app)[..], [SpellKitFx::Reap { entity, .. }] if entity == go),
        "the falling edge reaps"
    );
}

/// An instant hit on the caster takes the caster impact kit and one on anyone else the target
/// impact kit, each falling back on the impact kit (`0x800d00`); a missile arrival with a point
/// sounds its kit there, not on the unit (`0x702e5a`).
#[test]
fn the_impact_kit_is_the_casters_on_the_caster_and_the_targets_elsewhere() {
    const SPELL_ID: u32 = 48438; // Wild Growth's shape: an instant group heal
    const MISSILE_SPELL: u32 = 2061;
    const VISUAL_ID: u32 = 10000;
    const BARE_VISUAL: u32 = 10001;
    const IMPACT: u32 = 1;
    const CASTER_IMPACT: u32 = 2;
    const TARGET_IMPACT: u32 = 3;
    let kit = |sound| VisualKit {
        sound: Some(sound),
        ..Default::default()
    };
    let mut app = app();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                VISUAL_ID,
                VisualStages {
                    impact: IMPACT,
                    caster_impact: CASTER_IMPACT,
                    target_impact: TARGET_IMPACT,
                    ..Default::default()
                },
            ),
            (
                BARE_VISUAL,
                VisualStages {
                    impact: IMPACT,
                    caster_impact: CASTER_IMPACT,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::from([
            (IMPACT, kit(100)),
            (CASTER_IMPACT, kit(200)),
            (TARGET_IMPACT, kit(300)),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                SPELL_ID,
                SpellDisplay {
                    visual: VISUAL_ID,
                    ..Default::default()
                },
            ),
            (
                MISSILE_SPELL,
                SpellDisplay {
                    visual: BARE_VISUAL,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let caster = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn_empty().id();
    let sounds = |app: &mut App| -> Vec<SpellKitSound> {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .collect()
    };
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: 0x2,
        src: None,
        caster,
        spell_id: SPELL_ID,
        hits: vec![caster, target],
        misses: Vec::new(),
        dest: None,
        points: Vec::new(),
        ammo_display_id: None,
        seq: 1,
        trajectory: None,
        reflected: 0,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();
    let rung: Vec<(Entity, u32)> = sounds(&mut app)
        .into_iter()
        .filter_map(|s| match s {
            SpellKitSound::Play { entity, kit_sound } => Some((entity, kit_sound)),
            _ => None,
        })
        .collect();
    assert_eq!(rung, vec![(caster, 200), (target, 300)]);

    // A reflect with no outcome of its own plays the caster's kit on the caster.
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: 0x2,
        src: None,
        caster,
        spell_id: SPELL_ID,
        hits: Vec::new(),
        misses: vec![(target, 11)],
        dest: None,
        points: Vec::new(),
        ammo_display_id: None,
        seq: 1,
        trajectory: None,
        reflected: 1,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();
    assert!(matches!(
        sounds(&mut app)[..],
        [SpellKitSound::Play { entity, kit_sound: 200 }] if entity == caster
    ));

    // No target impact kit: the target falls back on column 3, and a point takes the kit.
    let at = Vec3::new(1.0, 2.0, 3.0);
    app.world_mut().write_message(cast_event(
        target,
        MISSILE_SPELL,
        CastEventKind::Impact {
            on_caster: false,
            at: Some(at),
            caster,
        },
    ));
    app.update();
    match &sounds(&mut app)[..] {
        [SpellKitSound::PlayAt {
            pos,
            kit_sound: 100,
        }] => assert_eq!(*pos, at),
        other => panic!("one point sound of the impact kit, got {other:?}"),
    }
}

/// A unit play hangs the slots, then the weapon effects at hands 2 and 1 (`0x73a6c0`), then the
/// kit's `SpellVisualKitModelAttach` rows at their own attachments (`0x745a7e`).
#[test]
fn a_kit_resolves_its_weapon_effects_and_model_attach_rows() {
    let row = |id, effect, attachment| benilla_formats::ModelAttach {
        id,
        effect,
        attachment,
        ..Default::default()
    };
    let visuals = SpellVisualCatalog::from_tables_with_paths(
        HashMap::new(),
        HashMap::new(),
        HashMap::from([
            (10, "Spells\\Slot.mdx".to_string()),
            (11, "Spells\\Pick.mdx".to_string()),
            (12, "Spells\\Cone.mdx".to_string()),
            (13, "Spells\\Ring.mdx".to_string()),
        ]),
    )
    .with_model_attach(7, &[row(1, 12, 21), row(2, 13, -1), row(3, 99, 19)]);
    let mut kit = VisualKit {
        id: 7,
        weapon_effects: [None, Some(11)],
        ..Default::default()
    };
    kit.effect_slots[0] = Some(10);
    let slots = super::resolve_kit_effects(&visuals, &kit);
    let got: Vec<(u16, u32, Option<u32>)> = slots
        .iter()
        .map(|s| (s.tag, s.effect, s.attach.map(|a| a.id)))
        .collect();
    assert_eq!(
        got,
        vec![
            (0x14, 10, None),
            (1, 11, None),
            (21, 12, Some(1)),
            (benilla_formats::WORLD_EFFECT_TAG, 13, Some(2)),
        ],
        "a row naming no model is skipped"
    );
}

/// A weapon effect stows the weapons unless the spell keeps its sheath (`Attributes & 0x40000`).
#[test]
fn a_weapon_effect_stows_unless_the_spell_keeps_the_sheath() {
    use bevy::ecs::system::RunSystemOnce;
    let kit = VisualKit {
        weapon_effects: [None, Some(11)],
        ..Default::default()
    };
    let run = |attributes: u32| {
        let mut app = App::new();
        app.add_message::<SheathRequest>();
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                SPELL,
                SpellDisplay {
                    attributes,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let unit = app.world_mut().spawn_empty().id();
        let _ = app.world_mut().run_system_once(
            move |mut w: MessageWriter<SheathRequest>, spells: Res<crate::ui_action::Spells>| {
                super::stow_for_weapon_effects(&mut w, &spells, unit, SPELL, &kit);
            },
        );
        app.world()
            .resource::<Messages<SheathRequest>>()
            .iter_current_update_messages()
            .map(|r| r.state)
            .collect::<Vec<_>>()
    };
    assert_eq!(run(0), vec![0], "stowed");
    assert_eq!(run(0x40000), Vec::<u8>::new(), "the sheath is kept");
    assert_eq!(
        run(0x2),
        vec![0, 2],
        "a ranged-slot spell draws its ranged weapon again"
    );
}

/// A precast kit's start anim rides the hold with the cast edge's stamp (`0x73b207`).
#[test]
fn a_precast_start_anim_rides_the_hold() {
    let mut app = app();
    {
        let mut visuals = app.world_mut().resource_mut::<SpellVisuals>();
        visuals.0 = SpellVisualCatalog::from_tables(
            HashMap::from([(
                VISUAL,
                VisualStages {
                    precast: PRECAST_KIT,
                    ..Default::default()
                },
            )]),
            HashMap::from([(
                PRECAST_KIT,
                VisualKit {
                    start_anim: Some(112),
                    anim_id: Some(HOLD_ANIM),
                    ..Default::default()
                },
            )]),
        );
    }
    let unit = app.world_mut().spawn_empty().id();
    app.world_mut().write_message(CastEvent {
        seq: 9,
        ..cast_event(unit, SPELL, CastEventKind::Start)
    });
    app.update();
    let h = app.world().entity(unit).get::<CastHold>().expect("held");
    assert_eq!((h.anim_id, h.start_anim, h.seq), (HOLD_ANIM, Some(112), 9));
}

/// A dest-location cast whose row flies a missile launches it from the packet's source at the
/// dest, landing on our own player, instead of timing the area kit (`0x8022d0`); with no player of
/// ours the area kit keeps the flight time.
#[test]
fn a_dest_location_missile_flies_from_the_source_to_the_dest() {
    const SPELL_ID: u32 = 52694;
    const VISUAL_ID: u32 = 10100;
    const AREA_KIT: u32 = 5;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<super::super::DestLocCast>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::DelayedAreaKit>()
        .add_message::<MissileSpawn>()
        .add_message::<super::MissileRelease>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            VISUAL_ID,
            VisualStages {
                missile_gate: 1,
                missile_model: 7,
                impact_area_kit: AREA_KIT,
                follow_ground_flags: 1,
                ..Default::default()
            },
        )]),
        HashMap::from([(AREA_KIT, VisualKit::default())]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SPELL_ID,
            SpellDisplay {
                visual: VISUAL_ID,
                speed: 20.0,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, super::play_dest_loc_casts);
    let (src, dest) = (Vec3::new(0.0, 5.0, 0.0), Vec3::new(30.0, 0.0, 0.0));
    let cast = super::super::DestLocCast {
        spell_id: SPELL_ID,
        src,
        dest,
        delay_ms: 0,
    };

    app.world_mut().write_message(cast);
    app.update();
    let waits: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<super::DelayedAreaKit>>()
        .drain()
        .map(|d| d.wait)
        .collect();
    assert_eq!(
        waits,
        vec![super::AreaKitWait::FlightFrom {
            from: src,
            speed: 20.0
        }],
        "no player of ours: the kit keeps the flight time"
    );

    let me = app.world_mut().spawn(crate::net::SelfPlayer).id();
    app.world_mut().write_message(cast);
    app.update();
    let spawns: Vec<MissileSpawn> = app
        .world_mut()
        .resource_mut::<Messages<MissileSpawn>>()
        .drain()
        .collect();
    match &spawns[..] {
        [s] => {
            assert_eq!(
                (s.caster, s.from, s.ground_aim),
                (me, Some(src), Some(dest))
            );
            assert!(s.targets.is_empty() && !s.awaits_release);
            assert!(
                s.shape.follow.is_some(),
                "the row's ground following rides along"
            );
        }
        other => panic!("one missile, got {}", other.len()),
    }
    assert!(app
        .world_mut()
        .resource_mut::<Messages<super::DelayedAreaKit>>()
        .drain()
        .next()
        .is_none());
}

/// A kit held at a point plants its head, base and world slots and its attach rows, not the rest.
#[test]
fn an_area_kit_at_a_point_plays_only_its_head_base_and_world_slots() {
    let mut slots = [None; 9];
    for (i, slot) in slots.iter_mut().enumerate() {
        *slot = Some(100 + i as u32);
    }
    let kit = VisualKit {
        effect_slots: slots,
        ..Default::default()
    };
    let paths = (0..9)
        .map(|i| (100 + i, format!("Slot{i}.m2")))
        .collect::<HashMap<_, _>>();
    let visuals = SpellVisualCatalog::from_tables_with_paths(HashMap::new(), HashMap::new(), paths);
    let mut got: Vec<&str> = super::area_kit_models(&visuals, &kit)
        .map(|(p, ..)| p)
        .collect();
    got.sort_unstable();
    let tags = benilla_formats::KIT_SLOT_TAGS;
    let want = |tag| format!("Slot{}.m2", tags.iter().position(|t| *t == tag).unwrap());
    let mut expected = [want(0x14), want(0x13)];
    expected.sort_unstable();
    assert_eq!(got, expected);
}

/// What the router sends the hold and the missile spawner for one cast edge.
#[derive(Debug, PartialEq)]
enum HoldEdge {
    /// A hold, with its slot's `(tag, effect)` when its row's own gate passes.
    Hold(u8, Option<(u16, u32)>),
    Fire,
    Clear(u8),
}

fn hold_edges(app: &mut App) -> Vec<HoldEdge> {
    let mut out: Vec<HoldEdge> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitFx>>()
        .drain()
        .filter_map(|m| match m {
            SpellKitFx::Hold {
                cast_count, slot, ..
            } => Some(HoldEdge::Hold(
                cast_count,
                slot.map(|sl| (sl.tag, sl.effect)),
            )),
            _ => None,
        })
        .collect();
    out.extend(
        app.world_mut()
            .resource_mut::<Messages<super::MissileRelease>>()
            .drain()
            .map(|r| match r.kind {
                super::ReleaseKind::Fire => HoldEdge::Fire,
                super::ReleaseKind::Clear(n) => HoldEdge::Clear(n),
            }),
    );
    out
}

/// A catalog of flag `0x20` rows for the hold tests: visual 1 a held missile at index 3, 2 with
/// column 16's -1, 3 naming no file, 4 unflagged, 5 index 13 with a cast animation; spells
/// `100 + visual`, 105 and 106 flying an aim (`SpellMissile` flag 1), 106 on visual 1.
fn hold_app() -> (App, Entity, Entity) {
    let mut app = app();
    let stages = |flags, missile_source_attach, missile_model| VisualStages {
        flags,
        missile_source_attach,
        missile_model,
        missile_gate: 1,
        ..Default::default()
    };
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
        HashMap::from([
            (1, stages(0x20, 3, 50)),
            (2, stages(0x20, u32::MAX, 50)),
            (3, stages(0x20, 3, 51)),
            (4, stages(0, 3, 50)),
            (
                5,
                VisualStages {
                    cast: 9,
                    ..stages(0x20, 13, 50)
                },
            ),
        ]),
        HashMap::from([(
            9,
            VisualKit {
                anim_id: Some(FIRE_ANIM),
                ..Default::default()
            },
        )]),
        HashMap::from([(50, "Spells\\Held.mdx".to_string())]),
    )));
    let display = |visual, flies_aim| SpellDisplay {
        visual,
        speed: 20.0,
        flies_aim,
        ..Default::default()
    };
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            (1..=5)
                .map(|v| (100 + v, display(v, v == 5)))
                .chain([(106, display(1, true))])
                .collect(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let unit = app.world_mut().spawn_empty().id();
    let me = app
        .world_mut()
        .spawn((
            crate::net::SelfPlayer,
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
        ))
        .id();
    (app, unit, me)
}

fn edge_of(
    app: &mut App,
    unit: Entity,
    spell_id: u32,
    cast_count: u8,
    kind: CastEventKind,
) -> Vec<HoldEdge> {
    app.world_mut().write_message(CastEvent {
        cast_count,
        ..cast_event(unit, spell_id, kind)
    });
    app.update();
    hold_edges(app)
}

/// A `SpellVisual` flag `0x20` row holds column 8's model at column 16 from START with the cast's
/// count; column 16's -1 and a column 8 naming no file still send the hold that drops the last,
/// and index 13 holds at the weapon hand's slot-4 attachment (`0x722950`, `0x71a7f0`).
#[test]
fn a_flagged_visual_holds_its_missile_from_start() {
    use HoldEdge::*;
    let (mut app, unit, _) = hold_app();
    let start = CastEventKind::Start;
    assert_eq!(
        edge_of(&mut app, unit, 101, 7, start),
        vec![Hold(7, Some((0x15, 50)))],
        "index 3 is the left spell hand"
    );
    assert_eq!(edge_of(&mut app, unit, 102, 0, start), vec![Hold(0, None)]);
    assert_eq!(edge_of(&mut app, unit, 103, 0, start), vec![Hold(0, None)]);
    assert!(edge_of(&mut app, unit, 104, 0, start).is_empty());
    assert_eq!(
        edge_of(&mut app, unit, 105, 0, start),
        vec![Hold(0, Some((0x16, 50)))]
    );
}

/// Our own START launches our missiles and drops any hold right after the start visuals, so it
/// holds nothing, flagged or not (`0x806a0b`).
#[test]
fn our_own_start_releases_our_missiles() {
    let (mut app, _, me) = hold_app();
    for spell_id in [101, 104] {
        assert_eq!(
            edge_of(&mut app, me, spell_id, 1, CastEventKind::Start),
            vec![HoldEdge::Fire]
        );
    }
}

/// A failure: `SMSG_SPELL_FAILURE` naming the running cast launches and drops (`0x72ed80`); the
/// end-cast drops only a numbered cast's hold of a flag-1 missile, not on wire result 187 alone
/// (`0x7feeb0`).
#[test]
fn a_failure_releases_the_hold_on_the_reference_s_paths() {
    use HoldEdge::*;
    let (mut app, unit, _) = hold_app();
    let fail = |interrupt, wire_reason| CastEventKind::Fail {
        interrupt,
        wire_reason,
    };
    assert_eq!(
        edge_of(&mut app, unit, 101, 4, fail(true, 0x28)),
        vec![Fire],
        "an interrupt; no flag 1"
    );
    assert!(
        edge_of(&mut app, unit, 101, 4, fail(false, 0x28)).is_empty(),
        "another failure leaves a hold without flag 1"
    );
    assert_eq!(
        edge_of(&mut app, unit, 106, 4, fail(false, 0x28)),
        vec![Clear(4)]
    );
    assert_eq!(
        edge_of(&mut app, unit, 106, 4, fail(true, 0x28)),
        vec![Fire, Clear(4)]
    );
    assert!(
        edge_of(&mut app, unit, 106, 0, fail(false, 0x28)).is_empty(),
        "an unnumbered cast"
    );
    assert!(
        edge_of(&mut app, unit, 106, 4, fail(false, 187)).is_empty(),
        "the launch result"
    );
    assert_eq!(
        edge_of(&mut app, unit, 106, 4, fail(false, 188)),
        vec![Clear(4)],
        "a reason past it, shared 0x91 with it"
    );
    assert!(
        edge_of(&mut app, unit, 101, 4, CastEventKind::Go).is_empty(),
        "GO keeps it"
    );
}

/// A missile row's GO with no cast animation releases the caster's missiles, the hold with them,
/// even when it made no missile; one with a cast animation waits for it (`0x80f5c1`).
#[test]
fn a_go_without_a_cast_animation_releases_even_with_no_missile() {
    let (mut app, unit, _) = hold_app();
    let go = |app: &mut App, spell_id, hits: Vec<Entity>| {
        app.world_mut().write_message(SpellGoTargets {
            list: Vec::new(),
            target_mask: 0x2,
            src: None,
            caster: unit,
            spell_id,
            hits,
            misses: Vec::new(),
            dest: None,
            points: Vec::new(),
            ammo_display_id: None,
            seq: 3,
            trajectory: None,
            reflected: 0,
            reflects: Vec::new(),
            caster_guid: 0,
            server_time: 0,
        });
        app.update();
        let spawns = app
            .world_mut()
            .resource_mut::<Messages<MissileSpawn>>()
            .drain()
            .map(|sp| sp.seq)
            .collect::<Vec<_>>();
        (spawns, hold_edges(app))
    };
    assert_eq!(
        go(&mut app, 101, Vec::new()),
        (Vec::new(), vec![HoldEdge::Fire]),
        "no missile, still a release"
    );
    assert_eq!(
        go(&mut app, 101, vec![unit]),
        (vec![3], vec![HoldEdge::Fire]),
        "the spawn, then its GO's release"
    );
    assert_eq!(
        go(&mut app, 105, Vec::new()),
        (Vec::new(), Vec::new()),
        "a cast animation releases at its keyframe"
    );
}

/// A kit whose one slot is a beam of chain row `effect_id`.
fn beam_kit(id: u32, effect_id: u32) -> VisualKit {
    use benilla_formats::{char_proc_type, CharProc};
    VisualKit {
        id,
        char_proc_slots: [
            Some(CharProc {
                ty: char_proc_type::CHAIN_CHANNEL,
                params: [effect_id as f32, 0.0, 1.0, 0.0],
            }),
            None,
            None,
            None,
        ],
        ..Default::default()
    }
}

/// Every beam play as `(owner, chain row, source)`.
fn beam_plays(app: &mut App) -> Vec<(Entity, u32, Option<Entity>)> {
    app.world_mut()
        .resource_mut::<Messages<super::ChainProcPlay>>()
        .drain()
        .map(|p| (p.entity, p.proc.effect_id, p.from))
        .collect()
}

/// A missile hit's two plays both carry the caster, but only the stage-2 state kit's sets
/// `0x1000` (`0x745409`), so only its beam names a source.
#[test]
fn only_the_state_kit_play_hands_the_caster_to_its_beam() {
    const BEAM_SPELL: u32 = 44776;
    const BEAM_VISUAL: u32 = 9118;
    const IMPACT_KIT: u32 = 910;
    const STATE_KIT: u32 = 911;
    let mut app = app();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            BEAM_VISUAL,
            VisualStages {
                impact: IMPACT_KIT,
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([
            (IMPACT_KIT, beam_kit(IMPACT_KIT, 5)),
            (STATE_KIT, beam_kit(STATE_KIT, 6)),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            BEAM_SPELL,
            SpellDisplay {
                visual: BEAM_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let caster = app.world_mut().spawn_empty().id();
    let target = struck_unit(app.world_mut(), 1, &[(0, BEAM_SPELL, 1)]);
    app.world_mut().write_message(cast_event(
        target,
        BEAM_SPELL,
        CastEventKind::Impact {
            on_caster: false,
            at: None,
            caster,
        },
    ));
    app.update();
    assert_eq!(
        beam_plays(&mut app),
        vec![(target, 5, None), (target, 6, Some(caster))]
    );

    // An instant hit plays its impact kit alone, with no guid (`0x801f10`).
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: 0x2,
        src: None,
        caster,
        spell_id: BEAM_SPELL,
        hits: vec![target],
        misses: Vec::new(),
        dest: None,
        points: Vec::new(),
        ammo_display_id: None,
        seq: 2,
        trajectory: None,
        reflected: 0,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();
    assert_eq!(beam_plays(&mut app), vec![(target, 5, None)]);
}

/// The aura beam fixture: Aura Beam Test (65444) with a state kit whose one slot is chain row 7.
const AURA_BEAM_SPELL: u32 = 65444;

/// The aura arm alone over [`AURA_BEAM_SPELL`]'s state kit, its state-done kit 913 beside it.
fn aura_arm_app() -> App {
    aura_arm_app_with(beam_kit(912, 7))
}

/// The aura arm alone over [`AURA_BEAM_SPELL`] with `state_kit` as its state kit.
fn aura_arm_app_with(state_kit: VisualKit) -> App {
    const VISUAL: u32 = 13881;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::aura_visual::AuraProc>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .add_message::<super::KitEffectClear>()
        .init_resource::<crate::net::GuidIndex>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<SpellKitSound>()
        .add_message::<super::BaseAnimRecompute>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            VISUAL,
            VisualStages {
                state: state_kit.id,
                ..Default::default()
            },
        )]),
        HashMap::from([(state_kit.id, state_kit)]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            AURA_BEAM_SPELL,
            SpellDisplay {
                visual: VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, super::arm_aura_state_fx);
    app
}

/// A unit holding `slots` as `(slot, spell, creator)`.
fn aura_holder(app: &mut App, slots: &[(u8, u32, u64)]) -> Entity {
    let (store, _) = crate::unit_auras::aura_unit_for_test(&[]);
    app.world_mut()
        .spawn((store, crate::unit_auras::UnitAuras::of_casters(slots)))
        .id()
}

/// Every spell clear and state-done kit written since the last drain, as `(entity, spell)`.
#[allow(clippy::type_complexity)]
fn clears_and_done(app: &mut App) -> (Vec<(Entity, u32)>, Vec<(Entity, u32)>) {
    let clears = app
        .world_mut()
        .resource_mut::<Messages<super::SpellEffectClear>>()
        .drain()
        .map(|c| (c.entity, c.spell_id))
        .collect();
    let done = app
        .world_mut()
        .resource_mut::<Messages<StateDoneKit>>()
        .drain()
        .map(|d| (d.entity, d.spell_id))
        .collect();
    (clears, done)
}

/// The aura arm's state-kit play carries the aura's creator (`0x724c77`); a creator not
/// streamed names an entity no store answers.
#[test]
fn an_aura_s_state_kit_beam_names_its_creator() {
    const CREATOR: u64 = 0xF130_0000_0000_0042;
    for streamed in [true, false] {
        let mut app = aura_arm_app();
        let creator = app.world_mut().spawn_empty().id();
        if streamed {
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(CREATOR, creator);
        }
        let holder = aura_holder(&mut app, &[(0, AURA_BEAM_SPELL, CREATOR)]);
        app.update();
        let want = if streamed {
            creator
        } else {
            Entity::PLACEHOLDER
        };
        assert_eq!(
            beam_plays(&mut app),
            vec![(holder, 7, Some(want))],
            "streamed={streamed}"
        );
    }
}

/// The applied core plays per slot with that slot's creator (`0x724c77`), so two casters' auras
/// of one spell draw two beams; a held slot never replays (`0x7248a8`).
#[test]
fn each_aura_slot_plays_its_state_kit_with_its_own_creator() {
    const A: u64 = 0xF130_0000_0000_00A1;
    const B: u64 = 0xF130_0000_0000_00B2;
    let mut app = aura_arm_app();
    let (a, b) = (
        app.world_mut().spawn_empty().id(),
        app.world_mut().spawn_empty().id(),
    );
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .extend([(A, a), (B, b)]);
    let holder = aura_holder(
        &mut app,
        &[(0, AURA_BEAM_SPELL, A), (3, AURA_BEAM_SPELL, B)],
    );
    app.update();
    assert_eq!(
        beam_plays(&mut app),
        vec![(holder, 7, Some(a)), (holder, 7, Some(b))]
    );
    // A third slot of the spell arriving later plays once more, with its own creator.
    app.world_mut()
        .entity_mut(holder)
        .insert(crate::unit_auras::UnitAuras::of_casters(&[
            (0, AURA_BEAM_SPELL, A),
            (3, AURA_BEAM_SPELL, B),
            (5, AURA_BEAM_SPELL, B),
        ]));
    app.update();
    assert_eq!(beam_plays(&mut app), vec![(holder, 7, Some(b))]);
}

/// Losing one of two slots holding a spell clears the spell's effects on the unit, the other
/// slot's too, and plays the state-done kit once; the slot left keeps the spell and does not
/// replay (`0x71e930`, `0x743b40`).
#[test]
fn losing_one_of_two_slots_clears_the_spell_and_replays_nothing() {
    const A: u64 = 0xF130_0000_0000_00A1;
    const B: u64 = 0xF130_0000_0000_00B2;
    let mut app = aura_arm_app();
    let holder = aura_holder(
        &mut app,
        &[(0, AURA_BEAM_SPELL, A), (1, AURA_BEAM_SPELL, B)],
    );
    app.update();
    let _ = beam_plays(&mut app);
    app.world_mut()
        .entity_mut(holder)
        .insert(crate::unit_auras::UnitAuras::of_casters(&[(
            1,
            AURA_BEAM_SPELL,
            B,
        )]));
    app.update();
    assert_eq!(
        clears_and_done(&mut app),
        (
            vec![(holder, AURA_BEAM_SPELL)],
            vec![(holder, AURA_BEAM_SPELL)]
        )
    );
    assert!(
        beam_plays(&mut app).is_empty(),
        "the slot left does not replay"
    );
}

/// A channel leaving the spell clears its effects and unstores its aura slots (`0x73eb50`): the
/// aura's later removal plays nothing, and only a new add replays the kit.
#[test]
fn a_channel_leaving_the_spell_unstores_its_aura_slots() {
    const A: u64 = 0xF130_0000_0000_00A1;
    let mut app = aura_arm_app();
    let holder = aura_holder(&mut app, &[(2, AURA_BEAM_SPELL, A)]);
    app.update();
    assert_eq!(beam_plays(&mut app).len(), 1);
    app.world_mut().write_message(super::ChannelSpellLeft {
        entity: holder,
        spell_id: AURA_BEAM_SPELL,
    });
    app.update();
    assert_eq!(
        clears_and_done(&mut app),
        (vec![(holder, AURA_BEAM_SPELL)], Vec::new()),
        "the clear, no state-done kit"
    );
    assert!(
        beam_plays(&mut app).is_empty(),
        "the held slot is not replayed"
    );
    // The aura leaving its unstored slot runs no removal core.
    app.world_mut()
        .entity_mut(holder)
        .insert(crate::unit_auras::UnitAuras::of_casters(&[]));
    app.update();
    assert_eq!(clears_and_done(&mut app), (Vec::new(), Vec::new()));
    // A fresh add plays again.
    app.world_mut()
        .entity_mut(holder)
        .insert(crate::unit_auras::UnitAuras::of_casters(&[(
            2,
            AURA_BEAM_SPELL,
            A,
        )]));
    app.update();
    assert_eq!(beam_plays(&mut app).len(), 1);
}

/// The channel poll reports the spell a unit's channel leaves, to a new channel or to none
/// (`0x73eb50`), and nothing on a rising edge.
#[test]
fn the_channel_poll_reports_the_spell_it_leaves() {
    let mut app = app();
    let left = |app: &mut App| -> Vec<(Entity, u32)> {
        app.world_mut()
            .resource_mut::<Messages<super::ChannelSpellLeft>>()
            .drain()
            .map(|l| (l.entity, l.spell_id))
            .collect()
    };
    let store = |spell: u32| {
        crate::net::ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(144, spell)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        )
    };
    let unit = app.world_mut().spawn(store(SPELL)).id();
    app.update();
    assert!(left(&mut app).is_empty(), "a rising edge leaves nothing");
    app.world_mut().entity_mut(unit).insert(store(RANGED_SPELL));
    app.update();
    assert_eq!(left(&mut app), vec![(unit, SPELL)]);
    app.world_mut().entity_mut(unit).insert(store(0));
    app.update();
    assert_eq!(left(&mut app), vec![(unit, RANGED_SPELL)]);
}

/// A missile hit plays the state kit only on a living target holding the spell's aura, an active
/// one when the kit sets `0x80` (`0x702f17`, `0x702f25`, `0x702f2e`).
#[test]
fn a_missile_hit_plays_the_state_kit_only_on_a_living_holder_of_the_aura() {
    const SPELL: u32 = 44776;
    const VISUAL: u32 = 9118;
    const STATE_KIT: u32 = 911;
    // (health, auras as (slot, spell, flags), kit flags, plays)
    type Case<'a> = (u32, &'a [(u8, u32, u8)], u32, bool);
    let cases: [Case; 7] = [
        (1, &[(0, SPELL, 1)], 0, true),
        (0, &[(0, SPELL, 1)], 0, false),
        (1, &[], 0, false),
        (1, &[(0, SPELL + 1, 1)], 0, false),
        (1, &[(0, SPELL, 0)], 0, true),
        (1, &[(0, SPELL, 0)], 0x80, false),
        (1, &[(0, SPELL, 0), (1, SPELL, 4)], 0x80, true),
    ];
    for (i, (health, auras, kit_flags, plays)) in cases.into_iter().enumerate() {
        let mut app = app();
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
            HashMap::from([(
                VISUAL,
                VisualStages {
                    state: STATE_KIT,
                    ..Default::default()
                },
            )]),
            HashMap::from([(
                STATE_KIT,
                VisualKit {
                    flags: kit_flags,
                    ..beam_kit(STATE_KIT, 6)
                },
            )]),
        )));
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                SPELL,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let caster = app.world_mut().spawn_empty().id();
        let target = struck_unit(app.world_mut(), health, auras);
        app.world_mut().write_message(cast_event(
            target,
            SPELL,
            CastEventKind::Impact {
                on_caster: false,
                at: None,
                caster,
            },
        ));
        app.update();
        let want = if plays {
            vec![(target, 6, Some(caster))]
        } else {
            Vec::new()
        };
        assert_eq!(beam_plays(&mut app), want, "case {i}");
    }
}

/// The aura reader adds only a slot with an effect bit set and removes one losing them all
/// (`0x72f8e1`): an inactive aura arms nothing, turning active plays the kit, and going inactive
/// clears the spell and plays the state-done kit.
#[test]
fn only_an_active_aura_slot_arms_its_state_kit() {
    const A: u64 = 0xF130_0000_0000_00A1;
    let mut app = aura_arm_app();
    let holder = app
        .world_mut()
        .spawn((
            crate::unit_auras::aura_unit_for_test(&[]).0,
            crate::unit_auras::UnitAuras::of(A, &[(0, AURA_BEAM_SPELL, 0)]),
        ))
        .id();
    app.update();
    assert!(beam_plays(&mut app).is_empty(), "inactive: nothing armed");
    let set = |app: &mut App, flags: u8| {
        app.world_mut()
            .entity_mut(holder)
            .insert(crate::unit_auras::UnitAuras::of(
                A,
                &[(0, AURA_BEAM_SPELL, flags)],
            ));
        app.update();
    };
    set(&mut app, 2);
    assert_eq!(
        beam_plays(&mut app).len(),
        1,
        "turning active plays the kit"
    );
    assert_eq!(clears_and_done(&mut app), (Vec::new(), Vec::new()));
    set(&mut app, 0);
    assert_eq!(
        clears_and_done(&mut app),
        (
            vec![(holder, AURA_BEAM_SPELL)],
            vec![(holder, AURA_BEAM_SPELL)]
        ),
        "losing the effect bits removes the slot"
    );
    // An inactive slot leaving runs no removal.
    app.world_mut()
        .entity_mut(holder)
        .insert(crate::unit_auras::UnitAuras::of(A, &[]));
    app.update();
    assert_eq!(clears_and_done(&mut app), (Vec::new(), Vec::new()));
}

/// The applied core stores the slot but skips the kit while a missile of the spell still waits on
/// the unit, for a row with a missile (`0x724b92`); the stored slot's removal still clears.
#[test]
fn a_queued_missile_of_the_spell_holds_back_the_aura_s_state_kit() {
    const SHOT: u32 = 75; // the spell `PendingMissiles::queue_a_shot` queues
    const VISUAL: u32 = 2000;
    const STATE_KIT: u32 = 914;
    for (gate, queued, plays) in [(1, true, false), (0, true, true), (1, false, true)] {
        let mut app = aura_arm_app();
        app.init_resource::<crate::entities::PendingMissiles>();
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
            HashMap::from([(
                VISUAL,
                VisualStages {
                    state: STATE_KIT,
                    missile_gate: gate,
                    ..Default::default()
                },
            )]),
            HashMap::from([(STATE_KIT, beam_kit(STATE_KIT, 8))]),
        )));
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                SHOT,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let holder = aura_holder(&mut app, &[(0, SHOT, 0xF130_0000_0000_00A1)]);
        if queued {
            crate::entities::PendingMissiles::queue_a_shot(&mut app, holder);
        }
        app.update();
        assert_eq!(
            beam_plays(&mut app).len(),
            usize::from(plays),
            "gate {gate}, queued {queued}"
        );
        app.world_mut()
            .entity_mut(holder)
            .insert(crate::unit_auras::UnitAuras::of_casters(&[]));
        app.update();
        assert_eq!(
            clears_and_done(&mut app).0,
            vec![(holder, SHOT)],
            "the stored slot is removed either way"
        );
    }
}

/// A display change re-adds every slot it does not store (`0x728e70`): one a channel unstored
/// replays with its creator, an inactive one is stored and plays, and a stored one is skipped.
#[test]
fn a_display_change_re_adds_the_slots_it_does_not_store() {
    const A: u64 = 0xF130_0000_0000_00A1;
    let mut app = aura_arm_app();
    let a = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(A, a);
    let holder = aura_holder(&mut app, &[(2, AURA_BEAM_SPELL, A)]);
    app.update();
    assert_eq!(beam_plays(&mut app), vec![(holder, 7, Some(a))]);
    let swap = |app: &mut App| {
        app.world_mut()
            .write_message(crate::entities::DisplaySwapped { entity: holder });
        app.update();
    };
    swap(&mut app);
    assert!(beam_plays(&mut app).is_empty(), "a stored slot is skipped");
    app.world_mut().write_message(super::ChannelSpellLeft {
        entity: holder,
        spell_id: AURA_BEAM_SPELL,
    });
    app.update();
    assert!(beam_plays(&mut app).is_empty());
    swap(&mut app);
    assert_eq!(beam_plays(&mut app), vec![(holder, 7, Some(a))]);
    swap(&mut app);
    assert!(
        beam_plays(&mut app).is_empty(),
        "stored again by the re-add"
    );

    // An inactive slot never arms on its add, but the display change stores and plays it.
    let idle = app
        .world_mut()
        .spawn((
            crate::unit_auras::aura_unit_for_test(&[]).0,
            crate::unit_auras::UnitAuras::of(A, &[(4, AURA_BEAM_SPELL, 0)]),
        ))
        .id();
    app.update();
    assert!(beam_plays(&mut app).is_empty());
    app.world_mut()
        .write_message(crate::entities::DisplaySwapped { entity: idle });
    app.update();
    assert_eq!(beam_plays(&mut app), vec![(idle, 7, Some(a))]);
}

/// A state kit with an anim-rate proc (11) waits for the unit's model (`0x724bf0`): the load plays
/// it with no guid while the unit holds the spell (`0x73eacf`), and plays nothing once it left.
#[test]
fn an_anim_rate_state_kit_waits_for_the_model() {
    use benilla_formats::{char_proc_type, CharProc};
    const A: u64 = 0xF130_0000_0000_00A1;
    let mut kit = beam_kit(915, 7);
    kit.char_proc_slots[1] = Some(CharProc {
        ty: char_proc_type::ANIM_RATE,
        params: [0.0; 4],
    });
    for holds_at_load in [true, false] {
        let mut app = aura_arm_app_with(kit);
        let a = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(A, a);
        let holder = aura_holder(&mut app, &[(0, AURA_BEAM_SPELL, A)]);
        app.update();
        app.update();
        assert!(beam_plays(&mut app).is_empty(), "no model, no play");
        assert!(app
            .world_mut()
            .resource_mut::<Messages<crate::aura_visual::AuraProc>>()
            .drain()
            .next()
            .is_none());
        if !holds_at_load {
            app.world_mut()
                .entity_mut(holder)
                .insert(crate::unit_auras::UnitAuras::of_casters(&[]));
            app.update();
            let _ = clears_and_done(&mut app);
        }
        app.world_mut()
            .entity_mut(holder)
            .insert(crate::entities::VisualAttached);
        app.update();
        let want = if holds_at_load {
            vec![(holder, 7, None)]
        } else {
            Vec::new()
        };
        assert_eq!(beam_plays(&mut app), want, "holds at load: {holds_at_load}");
        app.update();
        assert!(beam_plays(&mut app).is_empty(), "played once");
    }

    // A loaded model plays at once, with the slot's creator.
    let mut app = aura_arm_app_with(kit);
    let holder = aura_holder(&mut app, &[(0, AURA_BEAM_SPELL, 0)]);
    app.world_mut()
        .entity_mut(holder)
        .insert(crate::entities::VisualAttached);
    app.update();
    assert_eq!(beam_plays(&mut app), vec![(holder, 7, None)]);
}

/// `0x72af60`: a flag-0x1 impact kit of a beam spell waits for the beam to reach its unit, and is
/// dropped unplayed when nothing reaches it within 10 s (`0x728196`); without a beam it plays.
#[test]
fn a_beam_spells_flag_1_impact_kit_waits_for_the_beam_or_drops_at_the_timeout() {
    use benilla_formats::{char_proc_type, kit_flag, CharProc};

    const BEAM_SPELL: u32 = 57411; // Storm Cloud's shape: an instant pulsing beam
    const PLAIN_SPELL: u32 = 57412;
    const BEAM_VISUAL: u32 = 9813;
    const PLAIN_VISUAL: u32 = 9814;
    const CAST_KIT: u32 = 900;
    const IMPACT_KIT: u32 = 282;
    const IMPACT_SOUND: u32 = 77;

    let mut app = app();
    app.init_resource::<super::BeamKitWaits>();
    // One second a frame, past the virtual clock's 250 ms cap.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs(1),
    ));
    app.world_mut()
        .resource_mut::<Time<Virtual>>()
        .set_max_delta(std::time::Duration::from_secs(2));
    let impact_only = VisualStages {
        impact: IMPACT_KIT,
        ..Default::default()
    };
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                BEAM_VISUAL,
                VisualStages {
                    cast: CAST_KIT,
                    ..impact_only
                },
            ),
            (PLAIN_VISUAL, impact_only),
        ]),
        HashMap::from([
            (
                CAST_KIT,
                VisualKit {
                    char_proc_slots: [
                        Some(CharProc {
                            ty: char_proc_type::CHAIN_CAST,
                            params: [282.0, 0.0, 0.0, 0.0],
                        }),
                        None,
                        None,
                        None,
                    ],
                    ..Default::default()
                },
            ),
            (
                IMPACT_KIT,
                VisualKit {
                    id: IMPACT_KIT,
                    sound: Some(IMPACT_SOUND),
                    flags: kit_flag::BEAM_WAIT,
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                BEAM_SPELL,
                SpellDisplay {
                    visual: BEAM_VISUAL,
                    ..Default::default()
                },
            ),
            (
                PLAIN_SPELL,
                SpellDisplay {
                    visual: PLAIN_VISUAL,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    // `OBJECT_FIELD_TYPE`'s mask: object and unit, or object and game object.
    let store = |mask: u32| {
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, mask)]))
    };
    let caster = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn(store(0x9)).id();
    let object = app.world_mut().spawn(store(0x21)).id();
    let go_at = |app: &mut App, spell_id: u32, hit: Entity| {
        app.world_mut().write_message(SpellGoTargets {
            list: Vec::new(),
            target_mask: 0x2,
            src: None,
            caster,
            spell_id,
            hits: vec![hit],
            misses: Vec::new(),
            dest: None,
            points: Vec::new(),
            ammo_display_id: None,
            seq: 1,
            trajectory: None,
            reflected: 0,
            reflects: Vec::new(),
            caster_guid: 0,
            server_time: 0,
        });
    };
    let go = |app: &mut App, spell_id: u32| go_at(app, spell_id, target);
    let rings_on = |app: &mut App, on: Entity| -> usize {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter(|s| {
                matches!(s, SpellKitSound::Play { entity, kit_sound: IMPACT_SOUND } if *entity == on)
            })
            .count()
    };
    let rings = |app: &mut App| rings_on(app, target);
    let reach_key = |app: &mut App, spell_id: u32, key: i32| {
        app.world_mut()
            .resource_mut::<super::BeamKitWaits>()
            .reach(target, spell_id, key);
    };
    let reach = |app: &mut App, spell_id: u32| reach_key(app, spell_id, super::NO_BEAM_KEY);

    // The connect.
    go(&mut app, BEAM_SPELL);
    app.update();
    assert_eq!(rings(&mut app), 0, "held at the GO");
    app.update();
    assert_eq!(rings(&mut app), 0, "still waiting");
    reach(&mut app, PLAIN_SPELL);
    app.update();
    assert_eq!(rings(&mut app), 0, "another spell's beam reaches nothing");
    reach_key(&mut app, BEAM_SPELL, 2);
    app.update();
    assert_eq!(rings(&mut app), 0, "nor a ground bolt's keyed connect");
    reach(&mut app, BEAM_SPELL);
    app.update();
    assert_eq!(rings(&mut app), 1, "played once the beam reaches the unit");
    app.update();
    assert_eq!(rings(&mut app), 0, "and only once");
    assert!(app.world().resource::<super::BeamKitWaits>().is_empty());

    // The timeout.
    go(&mut app, BEAM_SPELL);
    for _ in 0..11 {
        app.update();
    }
    assert!(
        app.world().resource::<super::BeamKitWaits>().is_empty(),
        "dropped past 10 s"
    );
    reach(&mut app, BEAM_SPELL);
    app.update();
    assert_eq!(rings(&mut app), 0, "a late reach plays nothing");

    // The same kit on a spell with no beam.
    go(&mut app, PLAIN_SPELL);
    app.update();
    assert_eq!(rings(&mut app), 1, "no beam: played at the GO");

    // A game object's kit never waits (`0x743570`).
    go_at(&mut app, BEAM_SPELL, object);
    app.update();
    assert_eq!(rings_on(&mut app, object), 1, "an object plays at the GO");
    assert!(app.world().resource::<super::BeamKitWaits>().is_empty());

    // An impact at a point waits whole, its point kept (`0x7280c9`).
    let at = Vec3::new(1.0, 2.0, 3.0);
    app.world_mut().write_message(cast_event(
        target,
        BEAM_SPELL,
        CastEventKind::Impact {
            on_caster: false,
            at: Some(at),
            caster: Entity::PLACEHOLDER,
        },
    ));
    let at_point = |app: &mut App| -> usize {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter(|s| {
                matches!(s, SpellKitSound::PlayAt { pos, kit_sound: IMPACT_SOUND } if *pos == at)
            })
            .count()
    };
    app.update();
    assert_eq!(at_point(&mut app), 0, "the point's sound waits too");
    reach(&mut app, BEAM_SPELL);
    app.update();
    assert_eq!(at_point(&mut app), 1, "played at its point on the reach");
}

/// A location beam's area kit held under the beam's sequence plays at its point only on a connect
/// carrying that key (`0x722760`), the first bolt's end (`0x7fb15a`).
#[test]
fn a_held_area_kit_plays_on_its_keyed_connect() {
    const SPELL: u32 = 9;
    const AREA_KIT: u32 = 10;
    const SEQ: i32 = 2;
    let mut app = app();
    app.init_resource::<super::BeamKitWaits>();
    let caster = app.world_mut().spawn_empty().id();
    let pos = Vec3::new(4.0, 0.0, -2.0);
    app.world_mut()
        .resource_mut::<super::BeamKitWaits>()
        .hold_area(caster, SPELL, SEQ, AREA_KIT, pos, 0.0);
    let played = |app: &mut App| -> Vec<super::DelayedAreaKit> {
        app.world_mut()
            .resource_mut::<Messages<super::DelayedAreaKit>>()
            .drain()
            .collect()
    };
    app.update();
    assert!(played(&mut app).is_empty(), "held");
    app.world_mut()
        .resource_mut::<super::BeamKitWaits>()
        .reach(caster, SPELL, super::NO_BEAM_KEY);
    app.update();
    assert!(played(&mut app).is_empty(), "an unkeyed connect leaves it");
    app.world_mut()
        .resource_mut::<super::BeamKitWaits>()
        .reach(caster, SPELL, SEQ);
    app.update();
    assert_eq!(
        played(&mut app),
        vec![super::DelayedAreaKit {
            kit_id: AREA_KIT,
            pos,
            wait: super::AreaKitWait::Secs(0.0),
            deck: None,
        }]
    );
    assert!(app.world().resource::<super::BeamKitWaits>().is_empty());
}

/// A ground-aim targeting kit held on its caster plants its area-subset models at its point once
/// the beam reaches the caster, held by the caster rather than the flight (`0x702941`, `0x744ac0`).
#[test]
fn a_held_ground_targeting_kit_plants_at_its_point_on_the_casters_reach() {
    const SPELL: u32 = 9;
    const KIT: u32 = 8729;
    const EFFECT: u32 = 3807;
    let mut app = app();
    app.init_resource::<super::BeamKitWaits>();
    let mut slots = [None; 9];
    slots[2] = Some(EFFECT);
    slots[3] = Some(EFFECT + 1);
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
        HashMap::new(),
        HashMap::from([(
            KIT,
            VisualKit {
                id: KIT,
                effect_slots: slots,
                sound: Some(77),
                ..Default::default()
            },
        )]),
        HashMap::from([
            (EFFECT, "Spells\\Base.m2".to_string()),
            (EFFECT + 1, "Spells\\Hand.m2".to_string()),
        ]),
    )));
    let caster = app.world_mut().spawn_empty().id();
    let at = Vec3::new(24.0, 0.0, 3.0);
    app.world_mut()
        .resource_mut::<super::BeamKitWaits>()
        .hold_targeting(caster, SPELL, KIT, Some(at), 0.0);
    let planted = |app: &mut App| -> Vec<(String, Vec3, Option<Entity>)> {
        app.world_mut()
            .resource_mut::<Messages<crate::entities::dest_fx::GroundBurst>>()
            .drain()
            .map(|b| (b.path, b.pos, b.held_by))
            .collect()
    };
    let rang_at = |app: &mut App| -> Vec<Vec3> {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter_map(|s| match s {
                SpellKitSound::PlayAt { pos, kit_sound: 77 } => Some(pos),
                _ => None,
            })
            .collect()
    };
    app.update();
    assert!(planted(&mut app).is_empty(), "held");
    assert!(rang_at(&mut app).is_empty(), "its sound waits too");
    app.world_mut()
        .resource_mut::<super::BeamKitWaits>()
        .reach(caster, SPELL, super::NO_BEAM_KEY);
    app.update();
    assert_eq!(
        planted(&mut app),
        vec![("Spells\\Base.m2".to_string(), at, Some(caster))],
        "the base slot alone, at the point, held by the caster"
    );
    assert_eq!(rang_at(&mut app), vec![at]);
    assert!(app.world().resource::<super::BeamKitWaits>().is_empty());
}

/// An aura's flag-0x1 state kit on a beam spell waits for the beam to reach its unit before it
/// arms (`0x72af60` via `0x724c86`); on a spell with no beam it arms at once.
#[test]
fn a_beam_spells_flag_1_state_kit_arms_when_the_beam_reaches_its_unit() {
    use benilla_formats::{char_proc_type, kit_flag, CharProc};
    const BEAM_SPELL: u32 = 15407; // Mind Flay's shape: a channelled beam with a state kit
    const PLAIN_SPELL: u32 = 15408;
    const BEAM_VISUAL: u32 = 1;
    const PLAIN_VISUAL: u32 = 2;
    const CAST_KIT: u32 = 3;
    const STATE_KIT: u32 = 814;
    const ANIM: u16 = 14;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::aura_visual::AuraProc>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .init_resource::<crate::net::GuidIndex>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<SpellKitSound>()
        .add_message::<super::BaseAnimRecompute>();
    app.init_resource::<super::BeamKitWaits>();
    let state_only = VisualStages {
        state: STATE_KIT,
        ..Default::default()
    };
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                BEAM_VISUAL,
                VisualStages {
                    channel: CAST_KIT,
                    ..state_only
                },
            ),
            (PLAIN_VISUAL, state_only),
        ]),
        HashMap::from([
            (
                CAST_KIT,
                VisualKit {
                    char_proc_slots: [
                        Some(CharProc {
                            ty: char_proc_type::CHAIN_CAST,
                            params: [1.0, 0.0, 0.0, 0.0],
                        }),
                        None,
                        None,
                        None,
                    ],
                    ..Default::default()
                },
            ),
            (
                STATE_KIT,
                VisualKit {
                    id: STATE_KIT,
                    anim_id: Some(ANIM),
                    flags: kit_flag::BEAM_WAIT,
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                BEAM_SPELL,
                SpellDisplay {
                    visual: BEAM_VISUAL,
                    ..Default::default()
                },
            ),
            (
                PLAIN_SPELL,
                SpellDisplay {
                    visual: PLAIN_VISUAL,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, super::arm_aura_state_fx);
    let armed = |app: &mut App| -> Vec<Entity> {
        app.world_mut()
            .resource_mut::<Messages<super::BaseAnimRecompute>>()
            .drain()
            .map(|r| r.entity)
            .collect()
    };

    let unit = app
        .world_mut()
        .spawn(crate::unit_auras::aura_unit_for_test(&[BEAM_SPELL]))
        .id();
    app.update();
    assert!(armed(&mut app).is_empty(), "held at the aura's apply");
    app.update();
    assert!(armed(&mut app).is_empty(), "still waiting");
    app.world_mut().resource_mut::<super::BeamKitWaits>().reach(
        unit,
        BEAM_SPELL,
        super::NO_BEAM_KEY,
    );
    app.update();
    assert_eq!(armed(&mut app), vec![unit], "armed on the reach");
    app.update();
    assert!(armed(&mut app).is_empty(), "and only once");
    assert!(app.world().resource::<super::BeamKitWaits>().is_empty());

    let plain = app
        .world_mut()
        .spawn(crate::unit_auras::aura_unit_for_test(&[PLAIN_SPELL]))
        .id();
    app.update();
    assert_eq!(armed(&mut app), vec![plain], "no beam: armed at once");
}

/// A beam visual whose cast kit draws the beam, plus `kits` in its `stages`.
fn beam_tables(
    spell: u32,
    visual: u32,
    stages: VisualStages,
    kits: Vec<VisualKit>,
) -> (SpellVisuals, crate::ui_action::Spells) {
    use benilla_formats::{char_proc_type, CharProc};
    let cast_kit = VisualKit {
        id: stages.cast,
        char_proc_slots: [
            Some(CharProc {
                ty: char_proc_type::CHAIN_CAST,
                params: [1.0, 0.0, 0.0, 0.0],
            }),
            None,
            None,
            None,
        ],
        ..Default::default()
    };
    let kits = kits
        .into_iter()
        .chain([cast_kit])
        .map(|k| (k.id, k))
        .collect();
    let visuals = SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(visual, stages)]),
        kits,
    ));
    let spells = crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            spell,
            SpellDisplay {
                visual,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    };
    (visuals, spells)
}

/// A flag-0x1 kit that rings its own id as its sound.
fn waiting_kit(id: u32, anim_id: Option<u16>) -> VisualKit {
    VisualKit {
        id,
        anim_id,
        sound: Some(id),
        flags: benilla_formats::kit_flag::BEAM_WAIT,
        ..Default::default()
    }
}

fn unit_with_channel(channel: u32) -> crate::net::ObjectStore {
    crate::net::ObjectStore(
        // `OBJECT_FIELD_TYPE` object and unit, and `UNIT_CHANNEL_SPELL`.
        benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9), (144, channel)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    )
}

/// Every kit play on a unit passes the beam wait (`0x72af60`): the precast, the channel, the GO's
/// instant area kit (`0x80ee85`) and a landing's area kit (`0x700e20`) each wait on a beam spell,
/// and each plays its own way once the beam reaches the unit.
#[test]
fn a_beam_spells_precast_channel_and_area_kits_wait_for_the_beam() {
    const BEAM_SPELL: u32 = 5;
    const PRECAST: u32 = 8;
    const CHANNEL: u32 = 9;
    const INSTANT_AREA: u32 = 10;
    const IMPACT_AREA: u32 = 11;
    const ANIM: u16 = 52;

    let mut app = app();
    app.init_resource::<super::BeamKitWaits>();
    let (visuals, spells) = beam_tables(
        BEAM_SPELL,
        6,
        VisualStages {
            precast: PRECAST,
            cast: 7,
            channel: CHANNEL,
            instant_area_kit: INSTANT_AREA,
            impact_area_kit: IMPACT_AREA,
            ..Default::default()
        },
        vec![
            waiting_kit(PRECAST, Some(ANIM)),
            waiting_kit(CHANNEL, Some(ANIM)),
            waiting_kit(INSTANT_AREA, None),
            waiting_kit(IMPACT_AREA, None),
        ],
    );
    app.insert_resource(visuals).insert_resource(spells);
    let rung = |app: &mut App| -> Vec<(Entity, u32)> {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter_map(|s| match s {
                SpellKitSound::Play { entity, kit_sound } => Some((entity, kit_sound)),
                _ => None,
            })
            .collect()
    };
    let areas = |app: &mut App| -> Vec<u32> {
        app.world_mut()
            .resource_mut::<Messages<super::DelayedAreaKit>>()
            .drain()
            .map(|d| d.kit_id)
            .collect()
    };
    let reach = |app: &mut App, unit: Entity| {
        app.world_mut().resource_mut::<super::BeamKitWaits>().reach(
            unit,
            BEAM_SPELL,
            super::NO_BEAM_KEY,
        );
    };
    let hold_of = |app: &App, unit: Entity| {
        app.world()
            .entity(unit)
            .get::<CastHold>()
            .map(|h| h.spell_id)
    };

    // The precast: no hold and no sound until the reach, then both.
    let caster = app.world_mut().spawn(unit_with_channel(0)).id();
    app.world_mut()
        .write_message(cast_event(caster, BEAM_SPELL, CastEventKind::Start));
    app.update();
    assert!(rung(&mut app).is_empty(), "the precast waits");
    assert_eq!(hold_of(&app, caster), None);
    reach(&mut app, caster);
    app.update();
    assert_eq!(rung(&mut app), vec![(caster, PRECAST)]);
    assert_eq!(hold_of(&app, caster), Some(BEAM_SPELL), "held on the reach");

    // The channel kit, off the field's rising edge.
    let channeller = app.world_mut().spawn(unit_with_channel(BEAM_SPELL)).id();
    app.update();
    assert!(rung(&mut app).is_empty(), "the channel kit waits");
    assert_eq!(hold_of(&app, channeller), None);
    reach(&mut app, channeller);
    app.update();
    assert_eq!(rung(&mut app), vec![(channeller, CHANNEL)]);
    assert_eq!(hold_of(&app, channeller), Some(BEAM_SPELL));

    // The GO's instant area kit at its point, on the caster.
    app.world_mut().write_message(SpellGoTargets {
        list: Vec::new(),
        target_mask: 0x40,
        src: None,
        caster,
        spell_id: BEAM_SPELL,
        hits: Vec::new(),
        misses: Vec::new(),
        dest: Some(Vec3::ONE),
        points: Vec::new(),
        ammo_display_id: None,
        seq: 1,
        trajectory: None,
        reflected: 0,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    });
    app.update();
    let rings_at = |app: &mut App| -> usize {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter(|s| matches!(s, SpellKitSound::PlayAt { .. }))
            .count()
    };
    assert_eq!(rings_at(&mut app), 0, "the instant area kit waits");
    assert!(areas(&mut app).is_empty());
    reach(&mut app, caster);
    app.update();
    assert_eq!(areas(&mut app), vec![INSTANT_AREA], "played at its point");

    // A landing's area kit on its caster.
    app.world_mut().write_message(cast_event(
        caster,
        BEAM_SPELL,
        CastEventKind::GroundImpact { pos: Vec3::ONE },
    ));
    app.update();
    assert_eq!(rings_at(&mut app), 0, "the landing's area kit waits");
    assert!(areas(&mut app).is_empty());
    reach(&mut app, caster);
    app.update();
    assert_eq!(areas(&mut app), vec![IMPACT_AREA]);
    assert!(app.world().resource::<super::BeamKitWaits>().is_empty());
}

/// An aura's state-done kit on a beam spell waits for the beam like any unit kit (`0x72af60`).
#[test]
fn a_beam_spells_state_done_kit_waits_for_the_beam() {
    const BEAM_SPELL: u32 = 5;
    const DONE_KIT: u32 = 8;

    let mut app = app();
    app.init_resource::<super::BeamKitWaits>();
    app.init_resource::<super::super::PlaySeq>();
    let (visuals, spells) = beam_tables(
        BEAM_SPELL,
        6,
        VisualStages {
            cast: 7,
            state_done: DONE_KIT,
            ..Default::default()
        },
        vec![waiting_kit(DONE_KIT, None)],
    );
    app.insert_resource(visuals).insert_resource(spells);
    app.add_systems(
        Update,
        super::play_state_done_kits.after(route_cast_visuals),
    );
    let unit = app.world_mut().spawn(unit_with_channel(0)).id();
    let rung = |app: &mut App| -> usize {
        app.world_mut()
            .resource_mut::<Messages<SpellKitSound>>()
            .drain()
            .filter(|s| {
                matches!(
                    s,
                    SpellKitSound::Play {
                        kit_sound: DONE_KIT,
                        ..
                    }
                )
            })
            .count()
    };
    app.world_mut().write_message(StateDoneKit {
        entity: unit,
        spell_id: BEAM_SPELL,
    });
    app.update();
    assert_eq!(rung(&mut app), 0, "held at the aura's end");
    app.world_mut().resource_mut::<super::BeamKitWaits>().reach(
        unit,
        BEAM_SPELL,
        super::NO_BEAM_KEY,
    );
    app.update();
    assert_eq!(rung(&mut app), 1, "played on the reach");
}

/// Kit flag `0x80`: a missile replays the state kit only on a target whose aura has an effect
/// applied; without it, holding the spell in any slot is enough. A dead target replays nothing.
#[test]
fn an_active_aura_only_kit_needs_an_applied_aura() {
    use benilla_protocol::messages::{AuraRecord, AuraUpdate};

    const SPELL_ID: u32 = 5570;
    let store = |health| {
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_UNIT_HEALTH,
            health,
        )]))
    };
    let table = |flags| {
        let mut t = crate::unit_auras::UnitAuras::default();
        let record = AuraRecord {
            spell_id: SPELL_ID,
            flags,
            level: 1,
            stacks: 1,
            caster: None,
            duration_ms: None,
        };
        t.apply(
            &AuraUpdate {
                unit: 1,
                all: false,
                slots: vec![(0, Some(record))],
            },
            0.0,
        );
        t
    };
    let flagged = benilla_formats::kit_flag::ACTIVE_AURA_ONLY;
    let applied = super::AuraHeld::of(&store(100), Some(&table(1)), SPELL_ID);
    let inactive = super::AuraHeld::of(&store(100), Some(&table(0)), SPELL_ID);
    assert!(applied.replays(flagged) && applied.replays(0));
    assert!(
        !inactive.replays(flagged),
        "an empty-effect slot fails the 0x80 gate"
    );
    assert!(
        inactive.replays(0),
        "but holds the spell for an unflagged kit"
    );
    let dead = super::AuraHeld::of(&store(0), Some(&table(1)), SPELL_ID);
    assert!(!dead.replays(0), "a dead target replays nothing");
    let bare = super::AuraHeld::of(&store(100), None, SPELL_ID);
    assert!(!bare.replays(0), "nor one without the aura");
}

/// Kit flags `0x8` and `0x100` become play flags `0x4` (no tier resolve) and `0x20` (keep the
/// weapons out), beside `0x4` and `0x10` (`0x714ce0`).
#[test]
fn kit_flags_become_the_anim_play_flags() {
    use benilla_formats::kit_flag as k;
    let plain = super::AnimPlayFlags::from_kit(0);
    assert!(plain.tier_resolve() && plain.remap_special() && !plain.keep_weapons());
    let no_tier = super::AnimPlayFlags::from_kit(k::NO_TIER_RESOLVE);
    assert!(!no_tier.tier_resolve() && no_tier.remap_special());
    let raw = super::AnimPlayFlags::from_kit(k::RAW_ANIM);
    assert!(!raw.tier_resolve() && !raw.remap_special());
    let keeps = super::AnimPlayFlags::from_kit(k::KEEP_WEAPONS);
    assert!(keeps.keep_weapons() && keeps.tier_resolve() && keeps.remap_special());
    assert!(!super::AnimPlayFlags::from_kit(k::NO_SPECIAL_REMAP).remap_special());
}

/// Kit flag `0x200` marks every model the kit hangs as keeping its own alpha.
#[test]
fn an_own_alpha_kit_marks_its_models() {
    let visuals = SpellVisualCatalog::from_tables_with_paths(
        HashMap::new(),
        HashMap::new(),
        HashMap::from([(10, "Spells\\Slot.mdx".to_string())]),
    );
    let kit = |flags| VisualKit {
        flags,
        world_effect: Some(10),
        ..Default::default()
    };
    let own = |flags| {
        super::resolve_kit_effects(&visuals, &kit(flags))
            .iter()
            .map(|s| s.own_alpha)
            .collect::<Vec<_>>()
    };
    assert_eq!(own(benilla_formats::kit_flag::OWN_MODEL_ALPHA), vec![true]);
    assert_eq!(own(0), vec![false]);
}

/// A spell whose own first `SpellVisual` row flies missile model -1 or -2 keeps the weapons out
/// at every kit play; the ranged weapon's row filling an empty one does not count, nor does any
/// other model.
#[test]
fn a_weapon_missile_row_keeps_the_weapons_out() {
    const VISUAL: u32 = 900;
    const NO_ROW: u32 = 901;
    let kit = VisualKit {
        anim_id: Some(53),
        ..Default::default()
    };
    let play_for = |missile_model: u32, visual: u32| {
        let visuals = SpellVisualCatalog::from_tables(
            HashMap::from([(
                VISUAL,
                VisualStages {
                    missile_model,
                    ..Default::default()
                },
            )]),
            HashMap::new(),
        );
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                1,
                SpellDisplay {
                    visual,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        super::kit_anim_play(&spells, &visuals, 1, &kit).keep_weapons()
    };
    assert!(play_for(-1i32 as u32, VISUAL), "model -1");
    assert!(play_for(-2i32 as u32, VISUAL), "model -2");
    assert!(!play_for(-3i32 as u32, VISUAL), "model -3 is no such case");
    assert!(!play_for(0, VISUAL), "no model");
    assert!(!play_for(77, VISUAL), "a real model");
    assert!(
        !play_for(-1i32 as u32, NO_ROW),
        "only the spell's own row is read"
    );
}

/// A state kit's anim joins the unit's effect list for as long as the aura holds the spell, with
/// its own kit flags as play flags (`0x73b30d`, `0x73e79d`); its add edge's recompute carries the
/// kit play's flags, which a dead or dying id is played with (`0x73b33e`).
#[test]
fn a_state_kits_anim_rides_the_effect_list_while_its_aura_holds() {
    use super::super::{AnimPlayFlags, KitAnim, StateKitAnims};
    use benilla_formats::kit_flag;
    const SPELL: u32 = 71372; // Harvest Soul
    const VISUAL: u32 = 15103;
    const STATE_KIT: u32 = 13959; // anim 132, flags 0x4 | 0x8
    const DROWNED: u16 = 132;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<StateDoneKit>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<crate::aura_visual::AuraProc>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<super::ChannelSpellLeft>()
        .add_message::<crate::entities::DisplaySwapped>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::SpellEffectClear>()
        .init_resource::<crate::net::GuidIndex>()
        .add_message::<crate::aura_visual::BodyProcPlay>()
        .add_message::<SheathRequest>()
        .add_message::<SpellKitSound>()
        .add_message::<super::BaseAnimRecompute>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            VISUAL,
            VisualStages {
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            STATE_KIT,
            VisualKit {
                anim_id: Some(DROWNED),
                flags: kit_flag::RAW_ANIM | kit_flag::NO_TIER_RESOLVE,
                ..Default::default()
            },
        )]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SPELL,
            SpellDisplay {
                visual: VISUAL,
                attributes: 0x40000,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, super::arm_aura_state_fx);

    let unit = app
        .world_mut()
        .spawn(crate::unit_auras::aura_unit_for_test(&[SPELL]))
        .id();
    app.update();
    let raw = AnimPlayFlags::RAW | AnimPlayFlags::NO_TIER;
    assert_eq!(
        app.world().entity(unit).get::<StateKitAnims>().unwrap().0,
        vec![(
            SPELL,
            KitAnim {
                anim_id: DROWNED,
                play: AnimPlayFlags(raw),
            }
        )],
        "the kit's own flags, without the spell's keep-weapons"
    );
    let plays: Vec<u8> = app
        .world_mut()
        .resource_mut::<Messages<super::BaseAnimRecompute>>()
        .drain()
        .map(|r| r.play.0)
        .collect();
    assert_eq!(
        plays,
        vec![raw | AnimPlayFlags::KEEP_WEAPONS],
        "the play's flags take the spell's Attributes 0x40000"
    );

    crate::unit_auras::set_auras_for_test(app.world_mut(), unit, &[]);
    app.update();
    assert!(
        app.world().entity(unit).get::<StateKitAnims>().is_none(),
        "the aura's end takes the anim out"
    );
}

/// A GO's targets as the router reads them: `hits`, `misses` with their codes, the wire list in
/// that order with a guid per entity's index, and the mask and points.
fn listed_go(
    hits: &[Entity],
    misses: &[(Entity, u8)],
    target_mask: u32,
    dest: Option<Vec3>,
) -> SpellGoTargets {
    let guid = |e: Entity| 0xF130_0000_0000_0000 | u64::from(e.index_u32());
    SpellGoTargets {
        caster: Entity::PLACEHOLDER,
        spell_id: SPELL,
        hits: hits.to_vec(),
        misses: misses.to_vec(),
        list: hits
            .iter()
            .map(|&e| (guid(e), 0, 0))
            .chain(misses.iter().map(|&(e, c)| (guid(e), c, 0)))
            .collect(),
        target_mask,
        src: None,
        dest,
        points: Vec::new(),
        ammo_display_id: None,
        seq: 1,
        trajectory: None,
        reflected: 0,
        reflects: Vec::new(),
        caster_guid: 0,
        server_time: 0,
    }
}

/// A spawn as `(per-target count, ground aim, listed results, outcomes came, hops)`.
type SpawnShape = (usize, Option<Vec3>, Option<(Vec<u8>, bool, bool)>);

fn spawn_shapes(go: &SpellGoTargets, display: &SpellDisplay, flags: u32) -> Vec<SpawnShape> {
    let base = MissileSpawn {
        caster: Entity::PLACEHOLDER,
        spell_id: SPELL,
        path: None,
        ammo_display_id: None,
        dest_tag: None,
        speed: 20.0,
        targets: Vec::new(),
        reflects: Vec::new(),
        reflect_glances: false,
        scale: 1.0,
        ground_aim: None,
        missile_sound: None,
        awaits_release: false,
        trajectory: None,
        shape: crate::entities::MissileShape::default(),
        from: None,
        list: None,
        aim_deck: None,
        seq: 1,
    };
    super::go_missiles(go, display, flags, base)
        .into_iter()
        .map(|s| {
            let list = s.list.map(|l| {
                let results = l.units.iter().map(|u| u.result).collect();
                (results, l.results, l.bounce)
            });
            (s.targets.len(), s.ground_aim, list)
        })
        .collect()
}

/// The GO's missiles by its targets' kind (`0x7fffb0`): a unit or object cast flies one per target,
/// a located cast one at its point carrying the hits and another the misses with their outcomes,
/// visual flag `0x1` keeps a located cast per unit, flag `0x100` drops an empty located missile,
/// and a cast naming neither flies none.
#[test]
fn a_go_s_targets_pick_per_unit_missiles_or_one_at_the_point_carrying_them() {
    let mut world = World::new();
    let (a, b, c) = (
        world.spawn_empty().id(),
        world.spawn_empty().id(),
        world.spawn_empty().id(),
    );
    let at = Vec3::new(5.0, 0.0, 5.0);
    let plain = SpellDisplay::default();
    let unit = listed_go(&[a, b], &[(c, 2)], 0x2, None);
    assert_eq!(
        spawn_shapes(&unit, &plain, 0),
        vec![(2, None, None), (1, None, None)],
        "per unit, hits and misses apart"
    );
    let located = listed_go(&[a, b], &[(c, 2)], 0x42, Some(at));
    assert_eq!(
        spawn_shapes(&located, &plain, 0),
        vec![
            (0, Some(at), Some((vec![0, 0], false, false))),
            (0, Some(at), Some((vec![2], true, false))),
        ],
        "one at the point per group, only the misses' with outcomes"
    );
    assert_eq!(
        spawn_shapes(&located, &plain, 0x1),
        vec![(2, None, None), (1, None, None)],
        "visual flag 0x1 keeps a located cast per unit"
    );
    let missed_only = listed_go(&[], &[(c, 2)], 0x40, Some(at));
    assert_eq!(
        spawn_shapes(&missed_only, &plain, 0),
        vec![
            (0, Some(at), None),
            (0, Some(at), Some((vec![2], true, false)))
        ],
        "an empty hit group still flies at the point"
    );
    assert_eq!(
        spawn_shapes(&missed_only, &plain, 0x100),
        vec![(0, Some(at), Some((vec![2], true, false)))],
        "unless visual flag 0x100 drops it"
    );
    let source_only = SpellGoTargets {
        src: Some(at),
        ..listed_go(&[a], &[], 0x20, None)
    };
    assert_eq!(
        spawn_shapes(&source_only, &plain, 0),
        vec![(0, Some(at), Some((vec![0], false, false)))],
        "a source-only cast aims at its source"
    );
    let self_cast = listed_go(&[a], &[], 0, None);
    assert!(
        spawn_shapes(&self_cast, &plain, 0).is_empty(),
        "no unit, object or point bit: no missile"
    );
}

/// A multi-target spell (`AttributesEx4 & 0x40000`) flies one missile with every target and its
/// outcome: hopping them from the first, or at the point when located; with neither it flies
/// none (`0x800180`, `0x733600`, `0x7337e9`).
#[test]
fn a_multi_target_spell_flies_one_missile_with_every_target() {
    let mut world = World::new();
    let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
    let at = Vec3::new(5.0, 0.0, 5.0);
    let multi = SpellDisplay {
        attributes_ex4: 0x4_0000,
        ..Default::default()
    };
    let unit = listed_go(&[a], &[(b, 3)], 0x2, None);
    assert_eq!(
        spawn_shapes(&unit, &multi, 0x1),
        vec![(0, None, Some((vec![0, 3], true, true)))]
    );
    let located = listed_go(&[a], &[(b, 3)], 0x40, Some(at));
    assert_eq!(
        spawn_shapes(&located, &multi, 0),
        vec![(0, Some(at), Some((vec![0, 3], true, true)))]
    );
    let empty = listed_go(&[], &[], 0x2, None);
    assert!(spawn_shapes(&empty, &multi, 0).is_empty());
}

/// The extra points: a multi-target spell flies one more missile carrying them all, at the first
/// (`0x80f50e`); any other flies one per point carrying none (`0x80f5a0`).
#[test]
fn extra_points_fly_one_missile_each_or_one_carrying_them() {
    let mut world = World::new();
    let a = world.spawn_empty().id();
    let deck = world.spawn_empty().id();
    let (p, q) = (Vec3::new(5.0, 0.0, 5.0), Vec3::new(1.0, 0.0, 1.0));
    let qw = Vec3::new(1.0, 0.0, 11.0);
    let point = |at, deck, world| crate::entities::DeckPoint { at, deck, world };
    let go = SpellGoTargets {
        points: vec![point(p, None, p), point(q, Some(deck), qw)],
        ..listed_go(&[a], &[], 0x80002, None)
    };
    type Shape = (usize, Option<Vec3>, Option<(Entity, Vec3)>, usize);
    let shapes = |display: &SpellDisplay| -> Vec<Shape> {
        let base = MissileSpawn {
            caster: Entity::PLACEHOLDER,
            spell_id: SPELL,
            path: None,
            ammo_display_id: None,
            dest_tag: None,
            speed: 20.0,
            targets: Vec::new(),
            reflects: Vec::new(),
            reflect_glances: false,
            scale: 1.0,
            ground_aim: None,
            missile_sound: None,
            awaits_release: false,
            trajectory: None,
            shape: crate::entities::MissileShape::default(),
            from: None,
            list: None,
            aim_deck: None,
            seq: 1,
        };
        super::go_missiles(&go, display, 0, base)
            .into_iter()
            .map(|s| {
                let points = s.list.map_or(0, |l| l.points.len());
                (s.targets.len(), s.ground_aim, s.aim_deck, points)
            })
            .collect()
    };
    assert_eq!(
        shapes(&SpellDisplay::default()),
        vec![
            (1, None, None, 0),
            (0, Some(p), None, 0),
            (0, Some(qw), Some((deck, q)), 0)
        ]
    );
    let multi = SpellDisplay {
        attributes_ex4: 0x4_0000,
        ..Default::default()
    };
    assert_eq!(
        shapes(&multi),
        vec![(0, None, None, 0), (0, Some(p), None, 2)]
    );
}

/// An area landing's listed unit plays the impact kit its role picks and, under the hit's own aura
/// gate, the state kit, both carrying the caster, and never flinches (`0x70105d`, `0x7010df`).
#[test]
fn an_area_landing_plays_a_listed_unit_s_kits_under_the_hit_s_gate_with_no_wound() {
    const AREA_SPELL: u32 = 31250;
    const AREA_VISUAL: u32 = 8782;
    const CASTER_KIT: u32 = 920;
    const TARGET_KIT: u32 = 921;
    const STATE_KIT: u32 = 922;
    // (health, auras as (slot, spell, flags), kit flags, plays)
    type Case<'a> = (u32, &'a [(u8, u32, u8)], u32, bool);
    let cases: [Case; 4] = [
        (1, &[(0, AREA_SPELL, 1)], 0, true),
        (0, &[(0, AREA_SPELL, 1)], 0, false),
        (1, &[(0, AREA_SPELL, 0)], 0x80, false),
        (1, &[(0, AREA_SPELL, 4)], 0x80, true),
    ];
    for (i, (health, auras, kit_flags, plays)) in cases.into_iter().enumerate() {
        let mut app = app();
        app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
            HashMap::from([(
                AREA_VISUAL,
                VisualStages {
                    caster_impact: CASTER_KIT,
                    target_impact: TARGET_KIT,
                    state: STATE_KIT,
                    ..Default::default()
                },
            )]),
            HashMap::from([
                (CASTER_KIT, beam_kit(CASTER_KIT, 4)),
                (TARGET_KIT, beam_kit(TARGET_KIT, 5)),
                (
                    STATE_KIT,
                    VisualKit {
                        flags: kit_flags,
                        ..beam_kit(STATE_KIT, 6)
                    },
                ),
            ]),
        )));
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                AREA_SPELL,
                SpellDisplay {
                    visual: AREA_VISUAL,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let caster = struck_unit(app.world_mut(), 1, &[]);
        let target = struck_unit(app.world_mut(), health, auras);
        for unit in [target, caster] {
            app.world_mut().write_message(cast_event(
                unit,
                AREA_SPELL,
                CastEventKind::AreaImpact { caster },
            ));
        }
        app.update();
        let mut want = vec![(target, 5, None)];
        if plays {
            want.push((target, 6, Some(caster)));
        }
        want.push((caster, 4, None));
        assert_eq!(beam_plays(&mut app), want, "case {i}");
        let wounds = app
            .world_mut()
            .resource_mut::<Messages<WoundAnim>>()
            .drain()
            .count();
        assert_eq!(wounds, 0, "case {i}: an area landing flinches nobody");
    }
}
