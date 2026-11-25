//! The bar's per-slot verbs read at the call and its usable caches, through the stock verbs with
//! the world lent.

use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_SPELL};
use benilla_ui::script::UiScript;

use crate::creature_anim::Engaged;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::spell::{AutoRepeatActive, Cooldowns};
use crate::target::Selection;

use super::{PlayerActions, Spells};

/// The full usable update (`0x53cf10`) over `app`'s world, into a fresh VM.
fn full_pass(app: &mut App) {
    use bevy::ecs::system::RunSystemOnce;
    app.init_resource::<super::usable_cache::UsableCaches>();
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.world_mut()
        .run_system_once(
            |mut run: super::usable_cache::UsableRun, mut script: NonSendMut<UiScript>| {
                run.fire(&mut script, super::usable_cache::Pass::Full);
            },
        )
        .expect("the pass runs");
}

/// Our own player's guid in the range rigs.
const ME: u64 = 0x10;

/// Our player: its guid, the player type, alive with power.
fn own_player_fields() -> Vec<(u16, u32)> {
    vec![
        (0, ME as u32),
        (2, 0x19),
        (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
        (benilla_protocol::field::FIELD_UNIT_POWER1, 500),
    ]
}

/// `entity` is our player, as the live view finds it.
fn register_self(app: &mut App, entity: Entity) {
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, entity);
    app.insert_resource(crate::net::SelfGuid(Some(ME)));
}

/// `lua` evaluated with the world lent, as a verb call reads it.
fn live_eval<T: mlua::FromLuaMulti>(app: &mut App, lua: &str) -> T {
    crate::ui_script::live::with_live_world(app.world_mut(), |s| s.eval::<T>(lua).unwrap())
        .expect("the VM")
}

#[test]
fn the_full_pass_caches_a_bare_macro_as_usable() {
    use crate::ui_macro::{BoundSpell, MacroBoundSpells};
    use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = crate::ui_action::ActionSlots::default();
    for (slot, index) in [(0u8, 1u32), (1, 2), (2, 3)] {
        actions.buttons.insert(
            slot,
            ActionButton {
                slot,
                action: index,
                kind: ACTION_KIND_MACRO,
            },
        );
    }
    // Macro 1 is `.spawn 16032`, 2 an unknown `/cast Pyroblast`, and 3 does not exist.
    let mut bound = MacroBoundSpells::default();
    bound.0.insert(1, BoundSpell::None);
    bound.0.insert(2, BoundSpell::Unresolved);
    app.insert_resource(actions)
        .init_resource::<PlayerActions>()
        .insert_resource(bound)
        .insert_resource(Spells::empty_for_tests())
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<Items>()
        .insert_resource(NetCommands(tx));
    full_pass(&mut app);

    let mut usable = |action: u32| {
        live_eval::<(bool, bool)>(
            &mut app,
            &format!(
                "local u, oom = IsUsableAction({action}) return u and true or false, oom and true or false"
            ),
        )
    };
    assert!(usable(1).0, "a macro that casts nothing is a usable button");
    assert!(
        !usable(2).0,
        "a /cast of an unknown spell is grey (the reference's -1)"
    );
    assert!(!usable(3).0, "a slot whose macro no longer exists is grey");
    assert!(
        !usable(2).1,
        "grey, not the out-of-power blue: notEnoughMana stays 0 on the spell-less leg"
    );
}

/// `IsActionInRange` hands the spell range lookup the selected unit, so a ranged row pads by
/// that unit's reach and the player's alone: a Fireball (0-35) button reads in range out to 38
/// yards from a 1.5-reach target, whoever the player swings at, and `ActionHasRange` (a null
/// target) says the button has a range.
#[test]
fn a_ranged_button_pads_its_range_by_the_selected_target_alone() {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;

    const FIREBALL: u32 = 133;
    const SWUNG_AT: u64 = 0xA1;
    const SELECTED: u64 = 0xB2;
    /// `UNIT_FIELD_COMBATREACH`.
    const REACH: u16 = benilla_protocol::field::FIELD_UNIT_COMBATREACH;

    let verdict = |distance: f32| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = crate::ui_action::ActionSlots::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: FIREBALL,
                kind: ACTION_KIND_SPELL,
            },
        );
        // Any attackable unit (`0x2`), so the check needs no faction tables.
        let fireball = SpellDisplay {
            range_index: 4,
            targets: 0x2,
            ..Default::default()
        };
        let row = benilla_formats::SpellRange {
            min: 0.0,
            max: 35.0,
            friendly_min: 0.0,
            friendly_max: 35.0,
            flags: 0,
        };
        app.insert_resource(actions)
            .init_resource::<PlayerActions>()
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(FIREBALL, fireball)].into_iter().collect(),
                ),
                forms: Default::default(),
                ranges: benilla_formats::SpellRangeCatalog::from_rows(
                    [(4, row)].into_iter().collect(),
                ),
                cast_times: Default::default(),
                power_displays: Default::default(),
                difficulties: Default::default(),
                instance_difficulty: Default::default(),
                durations: Default::default(),
                radii: Default::default(),
            })
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));
        // The player swings at a 4.0-reach unit, close by.
        let swung_at = app
            .world_mut()
            .spawn((
                Transform::from_xyz(2.0, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(&[
                    (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                    (REACH, 4.0f32.to_bits()),
                ])),
            ))
            .id();
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Engaged(SWUNG_AT),
                Transform::default(),
                ObjectStore(ObjectFields::from_pairs(&own_player_fields())),
            ))
            .id();
        register_self(&mut app, me);
        // The selected unit keeps the descriptor's default 1.5 reach.
        let selected = app
            .world_mut()
            .spawn((
                Transform::from_xyz(distance, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_UNIT_HEALTH,
                    100,
                )])),
            ))
            .id();
        let mut index = app.world_mut().resource_mut::<crate::net::GuidIndex>();
        index.0.insert(SWUNG_AT, swung_at);
        index.0.insert(SELECTED, selected);
        app.insert_resource(Selection {
            target: Some(selected),
            guid: Some(SELECTED),
            last: None,
        });
        app.insert_non_send_resource(UiScript::new().unwrap());
        assert!(live_eval::<bool>(&mut app, "return ActionHasRange(1) == 1"));
        live_eval::<i64>(&mut app, "return IsActionInRange(1) or -1")
    };

    // 35 + the caster's 1.5 + the selected unit's 1.5 = 38.
    assert_eq!(verdict(37.5), 1, "inside the padded max");
    assert_eq!(
        verdict(38.5),
        0,
        "past it: the swing's 4.0 reach pads nothing"
    );
}

/// `IsActionInRange` hands the spell range lookup the selected unit, so a melee button reads in
/// range out to 7.667 yards while the player and the selected mob both run, and to the 5.0
/// floor otherwise.
#[test]
fn a_melee_button_reaches_the_moving_bonus_while_both_units_run() {
    use benilla_protocol::messages::ActionButton;
    use benilla_protocol::ObjectFields;

    const HAMSTRING: u32 = 1715;
    const SELECTED: u64 = 0xB2;

    let verdict = |caster_runs: bool, mob_runs: bool, distance: f32| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = crate::ui_action::ActionSlots::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: HAMSTRING,
                kind: ACTION_KIND_SPELL,
            },
        );
        let melee = benilla_formats::SpellRange {
            min: 0.0,
            max: 5.0,
            friendly_min: 0.0,
            friendly_max: 5.0,
            flags: 1,
        };
        app.insert_resource(actions)
            .init_resource::<PlayerActions>()
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        HAMSTRING,
                        SpellDisplay {
                            range_index: 2,
                            targets: 0x2,
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                forms: Default::default(),
                ranges: benilla_formats::SpellRangeCatalog::from_rows(
                    [(2, melee)].into_iter().collect(),
                ),
                cast_times: Default::default(),
                power_displays: Default::default(),
                difficulties: Default::default(),
                instance_difficulty: Default::default(),
                durations: Default::default(),
                radii: Default::default(),
            })
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(crate::player::Player::with_move_flags(if caster_runs {
                crate::creature_anim::move_flags::FORWARD
            } else {
                0
            }))
            .insert_resource(NetCommands(tx));
        let speeds = crate::net::UnitSpeeds(benilla_protocol::MoveSpeeds {
            walk: 2.5,
            run: 7.0,
            run_back: 4.5,
            swim: 4.7,
            swim_back: 2.5,
            turn_rate: 3.1,
            ..Default::default()
        });
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                crate::net::Embodied,
                speeds,
                Transform::default(),
                ObjectStore(ObjectFields::from_pairs(&own_player_fields())),
            ))
            .id();
        register_self(&mut app, me);
        // A mob at the descriptor's default 1.5 reach; a live spline at 8 yd/s is its run.
        let mob = app
            .world_mut()
            .spawn((
                speeds,
                Transform::from_xyz(distance, 0.0, 0.0),
                ObjectStore(ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_UNIT_HEALTH,
                    100,
                )])),
            ))
            .id();
        if mob_runs {
            app.world_mut().entity_mut(mob).insert(crate::net::Spline {
                points: vec![[distance, 0.0, 0.0], [distance + 16.0, 0.0, 0.0]],
                start: Instant::now(),
                duration: std::time::Duration::from_secs(2),
                id: 1,
                grounded: true,
                catmull_rom: false,
                end_facing: benilla_protocol::MonsterMoveFacing::None,
                run_mode: true,
                deck: None,
                arc: None,
                facing_fixed: false,
                cycle: None,
            });
        }
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(SELECTED, mob);
        app.insert_resource(Selection {
            target: Some(mob),
            guid: Some(SELECTED),
            last: None,
        });
        app.insert_non_send_resource(UiScript::new().unwrap());
        live_eval::<i64>(&mut app, "return IsActionInRange(1) or -1")
    };

    // 1.5 + 1.5 + 1.3333 is under the 5.0 floor; the bonus goes on the floor: 7.667.
    assert_eq!(verdict(true, true, 7.5), 1, "both running: inside 7.667");
    assert_eq!(verdict(true, true, 8.0), 0, "past it");
    assert_eq!(verdict(false, true, 7.5), 0, "the player stands");
    assert_eq!(verdict(true, false, 7.5), 0, "the mob stands");
    assert_eq!(verdict(false, false, 4.9), 1, "the floor");
}

/// One ON_USE block casting 433 "Food", whose shipped `Attributes` are `0x18000100`.
const FOOD_ITEM: u32 = 1487;
const FOOD_SPELL: u32 = 433;

/// A player with ten muffins in backpack slot 1 and the muffins on bar slot 1, in combat or not.
fn food_app(in_combat: bool) -> App {
    use benilla_protocol::messages::{ActionButton, ItemUseSpell};
    use benilla_protocol::ObjectFields;

    // Raw indices: `ITEM_FIELD_STACK_COUNT`, and `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1.
    const STACK: u16 = benilla_protocol::field::FIELD_ITEM_STACK_COUNT;
    const PACK_SLOT_1: u16 = benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = crate::ui_action::ActionSlots::default();
    actions.buttons.insert(
        0,
        ActionButton {
            slot: 0,
            action: FOOD_ITEM,
            kind: ACTION_KIND_ITEM,
        },
    );
    let mut items = Items::default();
    items.insert_template(
        FOOD_ITEM,
        Some(benilla_protocol::messages::ItemInfo {
            use_spell: Some(ItemUseSpell {
                spell_id: FOOD_SPELL,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }),
            ..crate::items::test_template("Conjured Muffin")
        }),
    );
    let food = SpellDisplay {
        attributes: 0x1800_0100,
        ..Default::default()
    };
    app.insert_resource(actions)
        .init_resource::<PlayerActions>()
        .insert_resource(crate::ui_macro::MacroBoundSpells::default())
        .insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(FOOD_SPELL, food)].into_iter().collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        })
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<crate::names::NameCache>()
        .insert_resource(items)
        .insert_resource(NetCommands(tx));
    crate::items::test_spawn_item(
        app.world_mut(),
        0xF0,
        ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_ENTRY, FOOD_ITEM),
            (STACK, 10),
        ]),
        false,
    );
    // The player: alive, with the ten muffins in backpack slot 1.
    let flags = (1u32 << 3)
        | if in_combat {
            crate::player::UNIT_FLAG_IN_COMBAT
        } else {
            0
        };
    let me = app
        .world_mut()
        .spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[
                (0, ME as u32),
                (2, 0x19),
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_POWER1, 500),
                (benilla_protocol::field::FIELD_UNIT_FLAGS, flags),
                (PACK_SLOT_1, 0xF0),
            ])),
        ))
        .id();
    register_self(&mut app, me);
    app
}

/// An item slot's on-use spell takes the usable walk, and every shipped Food/Drink spell has
/// `Attributes` bit 28, `ATTR_NOT_IN_COMBAT` (the walk's leg 8).
#[test]
fn food_on_the_bar_greys_while_the_player_is_in_combat() {
    let lit = |in_combat: bool| {
        let mut app = food_app(in_combat);
        full_pass(&mut app);
        live_eval::<bool>(&mut app, "return (IsUsableAction(1)) and true or false")
    };

    assert!(lit(false), "out of combat the muffins are full-colour");
    assert!(
        !lit(true),
        "in combat the food's on-use spell fails leg 8 and the button greys"
    );
}

/// `IsUsableItem` (`0x51c450`) and `IsUsableSpell` (`0x541680`) walk at the call, with no cache:
/// the muffins by id, by link and their Food spell by name and book slot are usable out of combat
/// and grey in it; an item not held, a name the book lacks and a slot past the book are grey.
#[test]
fn is_usable_item_and_spell_walk_at_the_call() {
    use benilla_ui::script::{SpellBookState, SpellSlotView, SpellTabView};

    let answers = |in_combat: bool| {
        let mut app = food_app(in_combat);
        app.insert_resource(crate::ui_spellbook::SpellBook::of(SpellBookState {
            tabs: vec![SpellTabView {
                name: "General".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: FOOD_SPELL,
                name: "Food".into(),
                ..Default::default()
            }],
            ..Default::default()
        }));
        app.insert_non_send_resource(UiScript::new().unwrap());
        live_eval::<Vec<String>>(
            &mut app,
            "local out = {} \
             local function pair(u, oom) out[#out + 1] = tostring(u) .. ',' .. tostring(oom) end \
             pair(IsUsableItem(1487)) \
             pair(IsUsableItem('|cffffffff|Hitem:1487:0:0:0:0:0:0:0:0:0|h[Conjured Muffin]|h|r')) \
             pair(IsUsableItem(9999)) \
             pair(IsUsableSpell('Food')) \
             pair(IsUsableSpell(1, 'spell')) \
             pair(IsUsableSpell(2, 'spell')) \
             out[#out + 1] = tostring(select('#', IsUsableSpell('Pyroblast'))) \
             return out",
        )
    };

    assert_eq!(
        answers(false),
        ["1,nil", "1,nil", "nil,nil", "1,nil", "1,nil", "nil,nil", "0"],
        "out of combat"
    );
    assert_eq!(
        answers(true),
        ["nil,nil", "nil,nil", "nil,nil", "nil,nil", "nil,nil", "nil,nil", "0"],
        "in combat leg 8 refuses the Food spell, alone or through the muffins"
    );
}

/// A GCD armed mid-frame, after the clock pair's anchor, reaches `GetActionCooldown` at or
/// before `GetTime()`, so the `Cooldown` frame `CooldownFrame_SetTimer` arms with it sweeps the
/// full 1.5 s and flashes, instead of finishing on its first update (`0x5ec91b`).
#[test]
fn a_mid_frame_gcd_sweeps_its_whole_duration() {
    use benilla_protocol::messages::ActionButton;

    const SPELL: u32 = 20154;
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = crate::ui_action::ActionSlots::default();
    actions.buttons.insert(
        0,
        ActionButton {
            slot: 0,
            action: SPELL,
            kind: ACTION_KIND_SPELL,
        },
    );
    let seal = SpellDisplay {
        start_recovery_category: 133,
        start_recovery_ms: 1500,
        ..Default::default()
    };
    // The frame's pair, with a fraction that rounds up: 12.0006 s reads 12_000 in the widget.
    let anchor = Instant::now();
    let ui_now = 12.0006;
    let mut cooldowns = Cooldowns::default();
    cooldowns.start_gcd(
        SPELL,
        &seal,
        anchor + std::time::Duration::from_millis(4),
        &crate::spell::SpellModifiers::default(),
        1.0,
    );
    app.insert_resource(actions)
        .init_resource::<PlayerActions>()
        .insert_resource(crate::ui_macro::MacroBoundSpells::default())
        .insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(SPELL, seal)].into_iter().collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        })
        .insert_resource(cooldowns)
        .init_resource::<crate::spell::SpellModifiers>()
        .insert_resource(crate::ui_script::UiClock { ui_now })
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<Items>()
        .insert_resource(NetCommands(tx));
    let mut script = UiScript::new().unwrap();
    script.set_now(ui_now);
    // Stock `CooldownFrame_SetTimer` (`Cooldown.lua`) on the bar's event.
    script
        .run(
            "Sweep = CreateFrame('Cooldown', 'Sweep') Sweep:Hide()
             local f = CreateFrame('Frame')
             f:RegisterEvent('ACTIONBAR_UPDATE_COOLDOWN')
             f:SetScript('OnEvent', function()
               local start, duration, enable = GetActionCooldown(1)
               SeenStart, SeenNow = start, GetTime()
               if start > 0 and duration > 0 and enable > 0 then
                 Sweep:SetCooldown(start, duration)
               end
             end)",
        )
        .unwrap();
    app.insert_non_send_resource(script);
    // The bar's cooldown signal, the verb read at the call inside it.
    crate::ui_script::live::with_live_world(app.world_mut(), |s| {
        s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", Vec::new());
    })
    .expect("the VM");

    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    // The event may queue for the next tick's head; a zero step fires it without moving time.
    script.tick(0.0);
    let (start, now): (f64, f64) = script.eval("return SeenStart, SeenNow").unwrap();
    assert!(
        start > 0.0 && start <= now,
        "GetActionCooldown's start {start} must not pass GetTime() {now}"
    );
    let shown = |s: &UiScript| s.eval::<bool>("return Sweep:IsShown() ~= nil").unwrap();
    let mut at = 0.0;
    for _ in 0..14 {
        script.tick(0.1);
        at += 0.1;
        assert!(shown(&script), "the sweep hid {at:.1} s into a 1.5 s GCD");
    }
    // Past the sweep and its one-second flash, it hides itself.
    for _ in 0..12 {
        script.tick(0.1);
    }
    assert!(!shown(&script), "the sweep and its flash end on their own");
}

/// A possessed unit's bar (the pet bar paged onto the main bar): a pet spell slot walks with
/// the pet as caster and reads the pet's power, an order is usable, an empty word is grey, and
/// with the pet not streamed its spell is grey (`0x5a9e20`).
#[test]
fn a_pet_action_slot_walks_with_the_pet_as_caster() {
    use benilla_protocol::messages::{ActionButton, PetActionEntry};
    use benilla_protocol::ObjectFields;

    const CHEAP: u32 = 9001;
    const DEAR: u32 = 9002;
    const PET: u64 = 0xF130_0000_0000_0001;
    let spell_word = |id: u32| PetActionEntry::from(id | (1 << 24));

    let run = |pet_streamed: bool| {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = crate::ui_action::ActionSlots::default();
        for i in 0..4u8 {
            actions.buttons.insert(
                i,
                ActionButton {
                    slot: i,
                    action: u32::from(i),
                    kind: crate::ui_action::ACTION_KIND_PET,
                },
            );
        }
        let mut bar = crate::ui_pet::PetBar::default();
        bar.spells.pet_guid = PET;
        bar.spells.bar[0] = spell_word(CHEAP);
        bar.spells.bar[1] = PetActionEntry::from(0x0700_0002); // Attack
        bar.spells.bar[2] = spell_word(DEAR);
        let dear = SpellDisplay {
            mana_cost: 50,
            ..Default::default()
        };
        app.insert_resource(actions)
            .init_resource::<PlayerActions>()
            .insert_resource(bar)
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(CHEAP, SpellDisplay::default()), (DEAR, dear)]
                        .into_iter()
                        .collect(),
                ),
                forms: Default::default(),
                ranges: Default::default(),
                cast_times: Default::default(),
                power_displays: Default::default(),
                difficulties: Default::default(),
                instance_difficulty: Default::default(),
                durations: Default::default(),
                radii: Default::default(),
            })
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Selection>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));
        // The player has no mana at all: only the pet's pool may pay.
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_POWER1, 0),
            ])),
        ));
        if pet_streamed {
            let pet = app
                .world_mut()
                .spawn((
                    Transform::default(),
                    ObjectStore(ObjectFields::from_pairs(&[
                        (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                        (benilla_protocol::field::FIELD_UNIT_POWER1, 10),
                    ])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(PET, pet);
        }
        full_pass(&mut app);
        (1..=4)
            .map(|a| {
                live_eval::<(bool, bool)>(
                    &mut app,
                    &format!(
                        "local u, oom = IsUsableAction({a}) return u and true or false, oom and true or false"
                    ),
                )
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        run(true),
        [(true, false), (true, false), (false, true), (false, false)],
        "the pet's spell, an order, the pet's spell past its mana, an empty word"
    );
    assert_eq!(
        run(false)[0],
        (false, false),
        "no pet object, no caster: grey"
    );
}

/// A spell that takes two muffins per cast, for the reagent legs.
const MUFFIN_SPELL: u32 = 9001;

/// The food rig with the muffins' display row, a VM, and a reagent spell on bar slot 2.
fn bar_app() -> App {
    use benilla_protocol::messages::ActionButton;
    let mut app = food_app(false);
    let display = app
        .world()
        .resource::<Items>()
        .template_cached(FOOD_ITEM)
        .map_or(0, |t| t.display_info_id);
    let displays = std::collections::HashMap::from([(
        display,
        benilla_formats::ItemDisplay {
            icon: Some("Interface\\Icons\\INV_Misc_Food_11".into()),
            ..Default::default()
        },
    )]);
    app.insert_resource(crate::entities::ItemDisplays::icons_for_tests(
        benilla_formats::ItemDisplayCatalog::from_displays(displays),
    ));
    let reagent = SpellDisplay {
        reagents: [
            (FOOD_ITEM, 2),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
        ],
        ..Default::default()
    };
    let food = SpellDisplay {
        attributes: 0x1800_0100,
        ..Default::default()
    };
    app.world_mut().resource_mut::<Spells>().catalog = benilla_formats::SpellCatalog::from_displays(
        [(FOOD_SPELL, food), (MUFFIN_SPELL, reagent)]
            .into_iter()
            .collect(),
    );
    app.world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            1,
            ActionButton {
                slot: 1,
                action: MUFFIN_SPELL,
                kind: ACTION_KIND_SPELL,
            },
        );
    app.init_resource::<super::usable_cache::UsableCaches>()
        .init_resource::<crate::ui_script::live::UiEvents>();
    app.insert_non_send_resource(UiScript::new().unwrap());
    app
}

/// Each slot's icon, count and gates read at the call (`0x5a97f0`, `0x5a7d10`, `0x5a9360`): the
/// item's art and carried count, a reagent spell's uses and its consumable gate.
#[test]
fn the_bar_slots_read_their_icon_and_count_at_the_call() {
    let mut app = bar_app();
    let read = live_eval::<Vec<String>>(
        &mut app,
        "local out = {} \
         for a = 1, 3 do \
           out[#out + 1] = tostring(GetActionTexture(a)) .. ',' .. GetActionCount(a) .. ',' \
             .. tostring(IsConsumableAction(a)) .. ',' .. tostring(HasAction(a)) \
         end \
         return out",
    );
    assert_eq!(
        read,
        [
            "Interface\\Icons\\INV_Misc_Food_11,10,nil,1",
            "nil,5,1,1",
            "nil,0,nil,nil",
        ],
        "the muffins, the spell their pairs feed, then an empty slot"
    );
}

/// A pickup writes the bar inside the call (`0x5abe70` -> `0x5aaa90`): the slot empties, the clear
/// goes out, and `ACTIONBAR_SLOT_CHANGED` fires before the call returns; the temporary bar's slots
/// refuse a pickup (`0x5abe9d`).
#[test]
fn a_pickup_writes_the_bar_and_signals_inside_the_call() {
    let mut app = bar_app();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let log = live_eval::<Vec<String>>(
        &mut app,
        "Seen = {} \
         local f = CreateFrame('Frame') f:RegisterEvent('ACTIONBAR_SLOT_CHANGED') \
         f:SetScript('OnEvent', function(_, _, a) Seen[#Seen + 1] = 'slot ' .. a end) \
         PickupAction(1) Seen[#Seen + 1] = 'returned ' .. tostring(HasAction(1)) \
         PickupAction(121) Seen[#Seen + 1] = 'temp' \
         return Seen",
    );
    assert_eq!(log, ["slot 1", "returned nil", "temp"]);
    assert!(!app
        .world()
        .resource::<crate::ui_action::ActionSlots>()
        .buttons
        .contains_key(&0));
    let sent: Vec<_> = rx
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::SetActionButton { button, packed } => Some((button, packed)),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [(0, 0)]);
}

/// The item update (`0x5aa5c0`) signals, in slot order, the item's slot and a spell slot that takes
/// it as a reagent, each slot's usable bytes first.
#[test]
fn an_item_update_signals_its_slots_and_its_reagent_spells() {
    let mut app = bar_app();
    live_eval::<()>(
        &mut app,
        "Seen = {} \
         local f = CreateFrame('Frame') f:RegisterEvent('ACTIONBAR_SLOT_CHANGED') \
         f:SetScript('OnEvent', function(_, _, a) Seen[#Seen + 1] = a end)",
    );
    super::slots::update_item(app.world_mut(), FOOD_ITEM);
    crate::ui_script::live::fire_handler_events(app.world_mut());
    let seen = live_eval::<Vec<i64>>(&mut app, "return Seen");
    assert_eq!(seen, [1, 2]);
}

/// The bar's pages read at the call: `GetBonusBarOffset` the stored offset (`0x5a7f20`), and
/// `GetActionBarPage` pinned to 1 while the temporary bar has an owner (`0x5a7fd0`).
#[test]
fn the_bar_pages_read_the_stored_offset_and_owners_at_the_call() {
    let mut app = bar_app();
    let read = |app: &mut App| {
        live_eval::<Vec<i64>>(app, "return { GetBonusBarOffset(), GetActionBarPage() }")
    };
    assert_eq!(read(&mut app), [0, 1]);
    app.insert_resource(crate::ui_action::TempActionBar {
        flags: 0,
        bonus_offset: 3,
    });
    assert_eq!(read(&mut app), [3, 1]);
    live_eval::<()>(&mut app, "ChangeActionBarPage(2)");
    assert_eq!(read(&mut app), [3, 2]);
    app.insert_resource(crate::ui_action::TempActionBar {
        flags: crate::ui_action::TEMP_BAR_PET,
        bonus_offset: 5,
    });
    assert_eq!(read(&mut app), [5, 1], "an owner pins the page");
}

/// A set slot is usable while sets can be used and neither an item of the set nor a slot it fills is locked (`0x5a9fe3`, `0x5aa00d`, `0x5aa01e`); the bar's update after a lock and an unlock (`0x51376b`, `0x51379b`) moves its bytes and signals `ACTIONBAR_UPDATE_USABLE` inside the verb that locked.
#[test]
fn a_lock_greys_a_set_slot_and_signals_the_bar_inside_the_verb() {
    use benilla_protocol::field::{
        FIELD_ITEM_FLAGS, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_PACK_SLOT_1,
    };
    use benilla_protocol::messages::{ActionButton, EquipmentSet, ACTION_KIND_EQUIPMENT_SET};
    use benilla_protocol::ObjectFields;

    /// A helmet in the pack, which the set names for the head, and the one worn there.
    const PACKED: u64 = 0x4000_0000_0000_0A01;
    const WORN: u64 = 0x4000_0000_0000_0A02;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut slots = crate::ui_action::ActionSlots::default();
    slots.buttons.insert(
        0,
        ActionButton {
            slot: 0,
            action: 3,
            kind: ACTION_KIND_EQUIPMENT_SET,
        },
    );
    let mut items = [1u64; benilla_protocol::messages::EQUIPMENT_SET_SLOTS];
    items[0] = PACKED;
    let actions = PlayerActions {
        equipment_sets: vec![EquipmentSet {
            guid: 0x77,
            id: 3,
            name: "Tank".into(),
            icon: "INV_Helmet_01".into(),
            items,
        }],
        ..Default::default()
    };
    app.insert_resource(slots)
        .insert_resource(actions)
        .init_resource::<crate::ui_macro::MacroBoundSpells>()
        .insert_resource(Spells::empty_for_tests())
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<Selection>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .init_resource::<crate::names::NameCache>()
        .init_resource::<crate::pending_item_ops::PendingItemOps>()
        .init_resource::<Items>()
        .insert_resource(NetCommands(tx));
    let mut fields = own_player_fields();
    fields.extend([
        (FIELD_PLAYER_PACK_SLOT_1, PACKED as u32),
        (FIELD_PLAYER_PACK_SLOT_1 + 1, (PACKED >> 32) as u32),
        (FIELD_PLAYER_INV_SLOT_HEAD, WORN as u32),
        (FIELD_PLAYER_INV_SLOT_HEAD + 1, (WORN >> 32) as u32),
    ]);
    let me = app
        .world_mut()
        .spawn((SelfPlayer, ObjectStore(ObjectFields::from_pairs(&fields))))
        .id();
    register_self(&mut app, me);
    for (guid, entry) in [(PACKED, 1001), (WORN, 1002)] {
        crate::items::test_spawn_item(
            app.world_mut(),
            guid,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, entry), (FIELD_ITEM_FLAGS, 0)]),
            false,
        );
        app.world_mut().resource_mut::<Items>().insert_template(
            entry,
            Some(benilla_protocol::messages::ItemInfo {
                inventory_type: 1,
                ..crate::items::test_template("Helm")
            }),
        );
    }
    full_pass(&mut app);
    let usable =
        |app: &mut App| live_eval::<bool>(app, "return IsUsableAction(1) and true or false");
    assert!(
        !usable(&mut app),
        "sets are off until `equipmentManager` is on"
    );

    // The cvar on: the next walk reads it (`0x5a9fe3`), here the one `CanUseEquipmentSets` runs on its edge (`0x5ad8b9`).
    {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.register_cvars([("equipmentManager", "1")]);
        script
            .run(
                "LOG = {} local f = CreateFrame('Frame') \
                 f:RegisterEvent('ACTIONBAR_UPDATE_USABLE') f:RegisterEvent('ITEM_LOCKED') f:RegisterEvent('ITEM_UNLOCKED') \
                 f:SetScript('OnEvent', function(_, e) LOG[#LOG + 1] = e .. ':' .. tostring(IsUsableAction(1) and 1 or 0) end)",
            )
            .unwrap();
    }
    let log = |app: &mut App, lua: &str| {
        live_eval::<String>(
            app,
            &format!("LOG = {{}} {lua} LOG[#LOG + 1] = 'after' return table.concat(LOG, ' ')"),
        )
    };
    assert_eq!(
        log(&mut app, "CanUseEquipmentSets()"),
        "ACTIONBAR_UPDATE_USABLE:1 after"
    );
    assert_eq!(log(&mut app, "CanUseEquipmentSets()"), "after", "no edge");

    // Picking the set's item up locks it: the slot greys, the bar signalling after the lock's own events and inside the call.
    assert_eq!(
        log(&mut app, "PickupContainerItem(0, 1)"),
        "ITEM_LOCKED:1 ACTIONBAR_UPDATE_USABLE:0 after"
    );
    assert_eq!(
        log(&mut app, "ClearCursor()"),
        "ITEM_UNLOCKED:0 ACTIONBAR_UPDATE_USABLE:1 after"
    );
    // A locked item in a slot the set fills greys it too (`0x5aa01e`).
    {
        let mut worn: benilla_ui::script::InventorySlots = Default::default();
        worn[1] = Some(benilla_ui::script::InvSlotView {
            guid: WORN,
            item_id: 1002,
            count: 1,
            equip_slots: vec![1],
            ..Default::default()
        });
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_inventory_slots(worn);
    }
    assert_eq!(
        log(&mut app, "PickupInventoryItem(1)"),
        "ITEM_LOCKED:1 ACTIONBAR_UPDATE_USABLE:0 after"
    );
    assert_eq!(
        log(&mut app, "ClearCursor()"),
        "ITEM_UNLOCKED:0 ACTIONBAR_UPDATE_USABLE:1 after"
    );

    // An app site's unlock runs the same update in its turn (`0x51379b`).
    app.init_resource::<crate::ui_script::live::UiEvents>();
    log(&mut app, "PickupContainerItem(0, 1)");
    live_eval::<()>(&mut app, "LOG = {}");
    crate::ui_items::watch::unlock_item(app.world_mut(), PACKED);
    crate::ui_script::live::fire_handler_events(app.world_mut());
    assert_eq!(
        live_eval::<String>(&mut app, "return table.concat(LOG, ' ')"),
        "ITEM_UNLOCKED:0 ACTIONBAR_UPDATE_USABLE:1"
    );

    // A swap out greys every set's slot after its own signal (`0x5addef`, `0x5addf7`), and its result restores them after its signals (`0x5af75d`, `0x5af7be`).
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_equipment_set_list(vec![benilla_ui::script::EquipmentSetView {
            guid: 0x77,
            id: 3,
            name: "Tank".into(),
            icon: "INV_Helmet_01".into(),
            items,
        }]);
        script
            .run(
                "local f = CreateFrame('Frame') f:RegisterEvent('EQUIPMENT_SWAP_PENDING') f:RegisterEvent('EQUIPMENT_SWAP_FINISHED')                  f:SetScript('OnEvent', function(_, e) LOG[#LOG + 1] = e .. ':' .. tostring(IsUsableAction(1) and 1 or 0) end)",
            )
            .unwrap();
    }
    log(&mut app, "ClearCursor()");
    assert_eq!(
        log(&mut app, "UseEquipmentSet('Tank')"),
        "EQUIPMENT_SWAP_PENDING:1 ACTIONBAR_UPDATE_USABLE:0 after"
    );
    live_eval::<()>(&mut app, "LOG = {}");
    crate::ui_script::live::with_live_world(app.world_mut(), |s| s.equipment_set_use_result(0));
    assert_eq!(
        live_eval::<String>(&mut app, "return table.concat(LOG, ' ')"),
        "EQUIPMENT_SWAP_FINISHED:0 ACTIONBAR_UPDATE_USABLE:1"
    );
}
