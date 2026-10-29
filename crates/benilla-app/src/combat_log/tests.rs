//! The producers against hand-built units and spells: each packet's entry, centre text,
//! `UNIT_COMBAT` and floating text.

use benilla_formats::{SpellCastTime, SpellCastTimeCatalog, SpellCatalog, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL};
use benilla_protocol::messages::{
    hit_info, AttackerState, ObjectType, PeriodicAuraLog, PeriodicTick, SpellDamageLog,
    SpellLogMiss, SpellRemoveKind, SpellRemoveLog,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{CombatLogEntry, CombatLogEvent, MissType, ScriptValue};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use super::entries::{CombatLogClock, PendingEntries};
use super::Log;
use crate::combat_text::{CombatTextSpawn, DamageTextGates};
use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, Reputations, SelfGuid};
use crate::ui_unit::{CombatTextEvent, UnitCombatFeedback};

const ME: u64 = 0x3ee;
const MOB: u64 = 0xF130_0000_4500_002A;
/// A cast-time fire spell.
const FIREBALL: u32 = 133;
/// An instant frost spell with a harmful aura.
const NOVA: u32 = 122;
/// A heal.
const HEAL: u32 = 2050;
/// A dispel.
const PURGE: u32 = 527;
/// A ranged basic shot.
const SHOT: u32 = 75;
/// A crowd-control spell: `AttributesExF & 0x200000` and a stun effect mechanic (12).
const STUN: u32 = 853;

fn spell(name: &str, school: u32, cast_row: u32) -> SpellDisplay {
    SpellDisplay {
        name: name.into(),
        school_mask: school,
        casting_time_index: cast_row,
        ..Default::default()
    }
}

fn spells() -> crate::ui_action::Spells {
    let mut s = crate::ui_action::Spells::empty_for_tests();
    s.catalog = SpellCatalog::from_displays(
        [
            (FIREBALL, spell("Fireball", 4, 5)),
            (NOVA, spell("Frost Nova", 16, 1)),
            (HEAL, spell("Lesser Heal", 2, 5)),
            (PURGE, spell("Dispel Magic", 2, 1)),
            (
                SHOT,
                SpellDisplay {
                    attributes_ex3: 0x8000,
                    ..spell("Auto Shot", 1, 1)
                },
            ),
            (
                STUN,
                SpellDisplay {
                    attributes_ex6: 0x20_0000,
                    effect_mechanic: [12, 12, 0],
                    ..spell("Hammer of Justice", 2, 1)
                },
            ),
        ]
        .into_iter()
        .collect(),
    );
    let row = |base_ms| SpellCastTime {
        base_ms,
        per_level_ms: 0,
        minimum_ms: base_ms,
    };
    s.cast_times = SpellCastTimeCatalog::from_rows([(1, row(0)), (5, row(1500))]);
    s
}

fn unit(kind: ObjectType, health: u32) -> ObjectStore {
    // The type mask: object and unit, and player for a player.
    let mask = if kind == ObjectType::Player {
        0x19
    } else {
        0x9
    };
    ObjectStore(
        ObjectFields::from_pairs(&[
            (2, mask), // OBJECT_FIELD_TYPE
            (FIELD_UNIT_HEALTH, health),
            (FIELD_UNIT_LEVEL, 80),
        ])
        .into_created(kind),
    )
}

/// The player and a mob, both streamed, and every resource the producers read.
fn rig() -> App {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(SelfGuid(Some(ME)))
        .init_resource::<GuidIndex>()
        .init_resource::<crate::ui_party::GroupState>()
        .init_resource::<Reputations>()
        .init_resource::<crate::names::NameCache>()
        .insert_resource(NetCommands(tx))
        .insert_resource(spells())
        .init_resource::<PendingEntries>()
        .init_resource::<CombatLogClock>()
        .init_resource::<DamageTextGates>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .add_message::<CombatTextSpawn>()
        .add_message::<UnitCombatFeedback>()
        .add_message::<CombatTextEvent>();
    for (guid, store) in [
        (ME, unit(ObjectType::Player, 100)),
        (MOB, unit(ObjectType::Unit, 100)),
    ] {
        let e = app
            .world_mut()
            .spawn((Guid(guid), store, Transform::default()))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
    }
    app
}

fn run(app: &mut App, f: impl FnOnce(&mut Log) + Send + Sync + 'static) {
    let mut f = Some(f);
    app.world_mut()
        .run_system_once(move |mut log: Log| {
            if let Some(f) = f.take() {
                f(&mut log);
            }
        })
        .unwrap();
}

fn entries(app: &App) -> Vec<CombatLogEntry> {
    app.world()
        .resource::<PendingEntries>()
        .entries()
        .into_iter()
        .cloned()
        .collect()
}

fn events(app: &App) -> Vec<CombatLogEvent> {
    entries(app).iter().map(|e| e.event).collect()
}

fn centre(app: &mut App) -> Vec<(&'static str, Vec<ScriptValue>)> {
    app.world_mut()
        .resource_mut::<Messages<CombatTextEvent>>()
        .drain()
        .map(|e| (e.message_type, e.args))
        .collect()
}

fn floated(app: &mut App) -> Vec<(String, u8, Option<u32>)> {
    app.world_mut()
        .resource_mut::<Messages<CombatTextSpawn>>()
        .drain()
        .map(|s| (s.text, s.category, s.color))
        .collect()
}

fn feedback(app: &mut App) -> Vec<(u64, &'static str, &'static str, i32)> {
    app.world_mut()
        .resource_mut::<Messages<UnitCombatFeedback>>()
        .drain()
        .map(|f| (f.guid, f.action, f.flags, f.amount))
        .collect()
}

fn fireball_hit(hit_info: u32, damage: u32) -> SpellDamageLog {
    SpellDamageLog {
        target: MOB,
        caster: ME,
        spell_id: FIREBALL,
        damage,
        overkill: 0,
        school: 4,
        absorbed: 0,
        resisted: 0,
        periodic: false,
        blocked: 0,
        hit_info,
    }
}

/// #429: a crit's hit-info bit reaches the entry and the number, which floats as category 2.
#[test]
fn a_spell_crit_logs_and_floats_as_a_crit() {
    let mut app = rig();
    run(&mut app, |log| {
        log.spell_damage_log(&fireball_hit(0x2, 300))
    });
    let e = &entries(&app)[0];
    assert_eq!(e.event, CombatLogEvent::SpellDamage);
    assert_eq!((e.source.guid, e.dest.guid), (ME, MOB));
    assert_eq!(e.spell.as_ref().map(|s| s.id), Some(FIREBALL));
    let d = e.info.damage.expect("a damage block");
    assert_eq!((d.amount, d.school, d.critical), (300, 4, true));
    assert_eq!(
        floated(&mut app),
        [(
            "300".to_string(),
            2,
            Some(crate::combat_text::COLOR_SPELL_GOLD)
        )]
    );
    assert_eq!(feedback(&mut app), [(MOB, "WOUND", "CRITICAL", 300)]);
}

/// A split hit logs `DAMAGE_SPLIT`; a hit that lands nothing logs its absorb as a miss.
#[test]
fn split_damage_and_a_full_absorb() {
    let mut app = rig();
    run(&mut app, |log| {
        log.spell_damage_log(&fireball_hit(0x8, 40));
        log.spell_damage_log(&SpellDamageLog {
            absorbed: 50,
            ..fireball_hit(0, 0)
        });
    });
    let all = entries(&app);
    assert_eq!(all[0].event, CombatLogEvent::DamageSplit);
    assert_eq!(all[1].event, CombatLogEvent::SpellMissed);
    let miss = all[1].info.miss.expect("a miss");
    assert_eq!((miss.kind, miss.amount), (MissType::Absorb, Some(50)));
}

fn tick(crit: bool) -> PeriodicAuraLog {
    PeriodicAuraLog {
        target: MOB,
        caster: ME,
        spell_id: NOVA,
        ticks: vec![PeriodicTick::Damage {
            amount: 80,
            overkill: 0,
            school: 16,
            absorbed: 0,
            resisted: 0,
            crit,
        }],
    }
}

/// #429: a periodic tick's crit byte floats it as a crit, and `CombatLogPeriodicSpells` gates the
/// number but never the entry.
#[test]
fn a_periodic_crit_floats_as_a_crit_under_its_cvar() {
    let mut app = rig();
    run(&mut app, |log| log.periodic_aura_log(&tick(true)));
    assert_eq!(events(&app), [CombatLogEvent::SpellPeriodicDamage]);
    assert!(entries(&app)[0].info.damage.unwrap().critical);
    assert_eq!(floated(&mut app)[0].1, 2);

    app.world_mut().resource_mut::<DamageTextGates>().periodic = false;
    run(&mut app, |log| log.periodic_aura_log(&tick(false)));
    assert_eq!(entries(&app).len(), 2, "the entry still logs");
    assert!(floated(&mut app).is_empty(), "the number does not float");
}

/// The mechanic word (`0x74da70`): an aura from a flagged spell floats its upper-cased state
/// name in the mechanic gold, the player's own bare under `fctSpellMechanics`, another caster's
/// with its name only under `fctSpellMechanicsOther`; `fctAllSpellMechanics` shows every one, and
/// an unflagged spell none.
#[test]
fn a_flagged_auras_mechanic_floats_by_its_three_cvars() {
    let mut app = rig();
    app.insert_resource(crate::ui_action::SpellMechanics {
        catalog: benilla_formats::SpellMechanicCatalog::from_names([
            (12, "stunned".to_string()),
            (26, "interrupted".to_string()),
        ]),
    });
    let gold = Some(crate::combat_text::COLOR_MECHANIC);
    let applied = |app: &mut App, caster| {
        run(app, move |log| {
            log.aura_added_or_removed(MOB, caster, STUN, true, true);
        });
        floated(app)
    };
    assert!(applied(&mut app, ME).is_empty(), "the rows boot off");
    app.world_mut().resource_mut::<DamageTextGates>().mechanics = true;
    assert_eq!(
        applied(&mut app, ME),
        [("Stunned".to_string(), 0, gold)],
        "one word for the two equal effect mechanics"
    );
    assert!(applied(&mut app, MOB).is_empty(), "another's needs its row");
    app.world_mut()
        .resource_mut::<DamageTextGates>()
        .mechanics_other = true;
    let other = applied(&mut app, MOB);
    assert!(
        other.len() == 1 && other[0].0.starts_with("Stunned ("),
        "{other:?}"
    );
    *app.world_mut().resource_mut::<DamageTextGates>() = DamageTextGates {
        all_mechanics: true,
        ..DamageTextGates::default()
    };
    assert_eq!(
        applied(&mut app, MOB).len(),
        1,
        "the all row alone shows it"
    );
    run(&mut app, |log| {
        log.aura_added_or_removed(MOB, ME, NOVA, true, true)
    });
    assert!(
        floated(&mut app).is_empty(),
        "an unflagged spell floats none"
    );
}

/// A heal over someone else floats green `+N`; over the player it is centre text with the healer.
#[test]
fn heals_float_over_others_and_centre_over_you() {
    let mut app = rig();
    run(&mut app, |log| {
        log.heal(ME, MOB, HEAL, 120, 30, 0, true, false);
    });
    let h = entries(&app)[0].info.heal.expect("a heal block");
    assert_eq!((h.amount, h.overheal, h.critical), (120, 30, true));
    assert_eq!(floated(&mut app), [("+120".to_string(), 7, None)]);

    run(&mut app, |log| {
        log.heal(MOB, ME, HEAL, 50, 0, 0, false, false)
    });
    assert!(floated(&mut app).is_empty());
    let c = centre(&mut app);
    assert_eq!(c[0].0, "HEAL");
    assert_eq!(c[0].1[1], ScriptValue::Int(50));
}

/// A cast-time spell logs its start, never its success; an instant logs its success.
#[test]
fn cast_start_and_success_split_on_the_cast_time() {
    let mut app = rig();
    run(&mut app, |log| {
        log.cast_start(ME, FIREBALL);
        log.spell_go(ME, None, FIREBALL, 0, MOB, &[], &[]);
        log.cast_start(ME, NOVA);
        log.spell_go(ME, None, NOVA, 0, 0, &[], &[]);
        // A hidden go logs nothing.
        log.spell_go(ME, None, NOVA, 0x1, 0, &[], &[]);
    });
    assert_eq!(
        events(&app),
        [
            CombatLogEvent::SpellCastStart,
            CombatLogEvent::SpellCastSuccess
        ]
    );
}

/// A go's miss logs `SPELL_MISSED`, a ranged shot's `RANGE_MISSED`, and a reflect logs the
/// reflected outcome back on the caster.
#[test]
fn go_misses_and_their_reflect() {
    let mut app = rig();
    run(&mut app, |log| {
        log.spell_go(ME, None, NOVA, 0, MOB, &[(MOB, 11)], &[(MOB, 3)]);
        log.spell_go(ME, None, SHOT, 0, MOB, &[(MOB, 3)], &[]);
    });
    let all = entries(&app);
    let misses: Vec<(CombatLogEvent, u64, MissType)> = all
        .iter()
        .filter_map(|e| Some((e.event, e.dest.guid, e.info.miss?.kind)))
        .collect();
    assert_eq!(
        misses,
        [
            (CombatLogEvent::SpellMissed, MOB, MissType::Reflect),
            (CombatLogEvent::SpellMissed, ME, MissType::Dodge),
            (CombatLogEvent::RangeMissed, MOB, MissType::Dodge),
        ]
    );
    let words: Vec<String> = floated(&mut app).into_iter().map(|w| w.0).collect();
    assert_eq!(words, ["Reflect", "Dodge"], "never a word over the player");
    assert!(feedback(&mut app).contains(&(ME, "DODGE", "", 0)));
}

/// #155: aura edges log applied and removed with their type, a dead unit's removal logs nothing,
/// and the player's own auras raise the centre text.
#[test]
fn aura_edges_log_with_their_type() {
    let mut app = rig();
    run(&mut app, |log| {
        log.aura_added_or_removed(MOB, ME, NOVA, true, true);
        log.aura_added_or_removed(ME, ME, HEAL, false, true);
        log.aura_dose(MOB, ME, NOVA, 1, 2, true);
        log.aura_refreshed(MOB, ME, NOVA, true);
        log.aura_added_or_removed(ME, ME, HEAL, false, false);
    });
    assert_eq!(
        events(&app),
        [
            CombatLogEvent::SpellAuraApplied,
            CombatLogEvent::SpellAuraApplied,
            CombatLogEvent::SpellAuraRefresh,
            CombatLogEvent::SpellAuraRemoved,
        ],
        "a dose needs a stacking spell"
    );
    assert_eq!(entries(&app)[0].info.harmful, Some(true));
    let c: Vec<&str> = centre(&mut app).iter().map(|c| c.0).collect();
    assert_eq!(c, ["SPELL_AURA_START", "SPELL_AURA_END"]);

    let dead = app.world_mut().resource::<GuidIndex>().0[&MOB];
    app.world_mut()
        .entity_mut(dead)
        .insert(unit(ObjectType::Unit, 0));
    run(&mut app, |log| {
        log.aura_added_or_removed(MOB, ME, NOVA, true, false)
    });
    assert_eq!(entries(&app).len(), 4, "a dead unit's removal does not log");
}

/// A player's death logs `UNIT_DIED` with no source; a creature with no template logs nothing.
#[test]
fn deaths_log_for_known_units() {
    let mut app = rig();
    run(&mut app, |log| {
        log.unit_dead(ME);
        log.unit_dead(MOB);
    });
    let all = entries(&app);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].event, CombatLogEvent::UnitDied);
    assert_eq!((all[0].source.guid, all[0].dest.guid), (0, ME));
}

/// A dispel names the removed aura second; a break by damage names no breaking spell.
#[test]
fn dispels_and_breaks() {
    let mut app = rig();
    let removed = |kind, spell_id| SpellRemoveLog {
        kind,
        target: MOB,
        caster: ME,
        spell_id,
        auras: vec![(NOVA, true)],
    };
    run(&mut app, move |log| {
        log.spell_remove_log(&removed(SpellRemoveKind::Dispel, PURGE));
        log.spell_remove_log(&removed(SpellRemoveKind::Steal, PURGE));
        log.spell_remove_log(&removed(SpellRemoveKind::Break, 0));
    });
    let all = entries(&app);
    assert_eq!(
        all.iter().map(|e| e.event).collect::<Vec<_>>(),
        [
            CombatLogEvent::SpellDispel,
            CombatLogEvent::SpellStolen,
            CombatLogEvent::SpellAuraBroken
        ]
    );
    assert_eq!(all[0].spell.as_ref().map(|s| s.id), Some(PURGE));
    assert_eq!(all[0].info.extra_spell.as_ref().map(|s| s.id), Some(NOVA));
    assert_eq!(all[2].spell.as_ref().map(|s| s.id), Some(NOVA));
    assert!(all[2].info.extra_spell.is_none());
}

/// A damage shield's miss and a periodic immunity take their own events.
#[test]
fn shield_misses_and_periodic_immunity() {
    let mut app = rig();
    run(&mut app, |log| {
        log.spell_log_miss(&SpellLogMiss {
            spell_id: NOVA,
            caster: ME,
            debug: false,
            misses: vec![(MOB, 7)],
        });
        log.spell_missed(ME, MOB, NOVA, 7, false, true);
        log.spell_missed(ME, MOB, NOVA, 0, false, false);
    });
    assert_eq!(
        events(&app),
        [
            CombatLogEvent::DamageShieldMissed,
            CombatLogEvent::SpellPeriodicMissed
        ]
    );
}

/// Runic power shows a tenth of its raw amount, in the entry and the centre text.
#[test]
fn an_energize_shows_the_displayed_amount() {
    let mut app = rig();
    run(&mut app, |log| log.energize(ME, ME, NOVA, 6, 150, false));
    assert_eq!(entries(&app)[0].info.energize, Some((15, 6)));
    assert_eq!(
        centre(&mut app),
        [(
            "ENERGIZE",
            vec![ScriptValue::Int(15), ScriptValue::Str("RUNIC_POWER".into())]
        )]
    );
}

/// A blocked hit on the player: the damage entry, and `BLOCK` with damage and blocked amount.
#[test]
fn a_blocked_swing_on_you_centres_its_amounts() {
    let mut app = rig();
    let swing = AttackerState {
        attacker: MOB,
        victim: ME,
        hit_info: hit_info::AFFECTS_VICTIM | hit_info::BLOCK,
        damage: 20,
        overkill: 0,
        victim_state: 5,
        absorb: 0,
        resist: 0,
        blocked: 7,
        melee_spell_id: 0,
        school_mask: 1,
    };
    run(&mut app, move |log| log.swing(&swing));
    assert_eq!(events(&app), [CombatLogEvent::SwingDamage]);
    assert_eq!(
        centre(&mut app),
        [("BLOCK", vec![ScriptValue::Int(20), ScriptValue::Int(7)])]
    );
}
