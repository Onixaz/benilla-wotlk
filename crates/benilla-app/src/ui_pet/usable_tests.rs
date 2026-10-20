//! The pet-usability predicate `0x5d3e80` and every verb it gates, whole client headless: each of
//! its exits alone must grey `GetPetActionsUsable`, and must stop a press, an order, both autocast
//! toggles and the drag's writes, while a pet that trips none of them takes all of it as before.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use benilla_protocol::messages::{
    PetActionEntry, PET_ACT_COMMAND, PET_ACT_ENABLED, PET_ACT_REACTION, PET_COMMAND_ATTACK,
    PET_COMMAND_FOLLOW, PET_COMMAND_STAY, PET_REACT_AGGRESSIVE, PET_STATE_VEHICLE,
};

use crate::net::{ClientCommand, Guid, GuidIndex};
use crate::target::UNIT_FLAG_POSSESSED;

use super::press_tests::{
    claw, latched, owned_pet, owned_pet_with, pet_actions, player_with, rig, Rig, CHARMEDBY, CLAW,
    FLAGS, ME, PET, SUMMONEDBY,
};
use super::PetBar;

/// `PLAYER_FARSIGHT`, low word: the view's anchor, the guid `0x5d3ec0`-`0x5d3ef3` holds against the
/// pet's.
const FARSIGHT: u16 = 712;
/// Some other guid than ours or the pet's.
const OTHER: u64 = 0x99;
/// A foe the Attack order can be aimed at.
const FOE: u64 = 0x77;

fn guid_pair(field: u16, guid: u64) -> [(u16, u32); 2] {
    [(field, guid as u32), (field + 1, (guid >> 32) as u32)]
}

/// The player, replaced: player-controlled, so the Attack order's validator needs no faction
/// table, and carrying `extra`.
fn set_player(rig: &mut Rig, extra: &[(u16, u32)]) {
    let world = rig.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    let mut fields = vec![(FLAGS, 0x8)];
    fields.extend_from_slice(extra);
    world.entity_mut(me).insert(player_with(&fields));
}

/// The pet, replaced by [`owned_pet_with`] `extra`: alive, so the Attack order's validator passes.
fn set_pet(rig: &mut Rig, extra: &[(u16, u32)]) {
    let world = rig.app.world_mut();
    let pet = world.resource::<GuidIndex>().0[&PET];
    let mut fields = vec![(22, 100), (28, 100)];
    fields.extend_from_slice(extra);
    world.entity_mut(pet).insert(owned_pet_with(&fields));
}

const ATTACK: u32 = PET_COMMAND_ATTACK | ((PET_ACT_COMMAND as u32) << 24);
const FOLLOW: u32 = PET_COMMAND_FOLLOW | ((PET_ACT_COMMAND as u32) << 24);
const STAY: u32 = PET_COMMAND_STAY | ((PET_ACT_COMMAND as u32) << 24);
const AGGRESSIVE: u32 = PET_REACT_AGGRESSIVE | ((PET_ACT_REACTION as u32) << 24);
const CLAW_WORD: u32 = CLAW | ((PET_ACT_ENABLED as u32) << 24);

/// A usable pet on a bar with Claw (autocast on) in slot 1, and in the pet's book, with the foe
/// selected for the Attack order. Fed once, so the VM holds what the bar last pushed.
fn scene() -> Rig {
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], Some(owned_pet()));
    set_player(&mut rig, &[]);
    set_pet(&mut rig, &[]);
    let world = rig.app.world_mut();
    world.resource_mut::<PetBar>().spells.spells = vec![PetActionEntry::from(CLAW_WORD)];
    let foe = world
        .spawn((
            Guid(FOE),
            Transform::default(),
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(FOE, foe);
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(foe),
        guid: Some(FOE),
        ..Default::default()
    };
    rig.frame();
    rig
}

/// One state of the world to try every verb in.
struct Case {
    name: &'static str,
    apply: fn(&mut Rig),
    /// What `0x5d3e80` answers.
    usable: bool,
    /// Whether `PickupPetAction`'s own gate lets the drag start: the pet's object must resolve
    /// (`0x4be1f7`) and not be `UNIT_FLAG_POSSESSED` (`0x4be20a`), which is no exit of the
    /// predicate, whose possession never blocks and whose unresolved pet is one of its exits.
    draggable: bool,
}

/// One way `0x5d3e80` answers zero, and nothing else changed.
type Exit = (&'static str, fn(&mut Rig));

/// Every exit of the predicate, each alone, in the order the bytes take them.
const EXITS: [Exit; 11] = [
    ("the active player does not resolve (0x5d3ea4)", |rig| {
        rig.app
            .world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .remove(&ME);
    }),
    ("the player is charmed (0x5d3eb0)", |rig| {
        set_player(rig, &guid_pair(CHARMEDBY, OTHER));
    }),
    (
        "the player's far sight is on another unit (0x5d3ec0)",
        |rig| set_player(rig, &guid_pair(FARSIGHT, OTHER)),
    ),
    ("the pet does not resolve (0x5d3f3c)", |rig| {
        rig.app
            .world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .remove(&PET);
    }),
    ("the pet is summoned by another (0x5d3f5b)", |rig| {
        set_pet(rig, &guid_pair(SUMMONEDBY, OTHER));
    }),
    (
        "the pet has no summoner, whatever created it (0x5d3f5b)",
        |rig| set_pet(rig, &guid_pair(SUMMONEDBY, 0)),
    ),
    (
        "the pet is charmed by another, summoned by us (0x5d3f4f)",
        |rig| set_pet(rig, &guid_pair(CHARMEDBY, OTHER)),
    ),
    ("the pet is stunned (0x5d3310)", |rig| {
        set_pet(rig, &[(FLAGS, 0x0004_0000)]);
    }),
    ("the pet is confused (0x5d3310)", |rig| {
        set_pet(rig, &[(FLAGS, 0x0040_0000)]);
    }),
    ("the pet is fleeing (0x5d3310)", |rig| {
        set_pet(rig, &[(FLAGS, 0x0080_0000)]);
    }),
    ("a vehicle's bar we do not ride (0x5d3f77)", |rig| {
        rig.app.world_mut().resource_mut::<PetBar>().spells.state |= PET_STATE_VEHICLE;
    }),
];

/// Pets that trip none of the exits, however much like one: each still works.
const USABLE: [Exit; 5] = [
    ("an untouched pet", |_| {}),
    ("far sight on the pet itself, Eyes of the Beast", |rig| {
        set_player(rig, &guid_pair(FARSIGHT, PET));
    }),
    (
        "the pet charmed by us and summoned by another: the charmed-by wins",
        |rig| {
            let mut fields = guid_pair(CHARMEDBY, ME).to_vec();
            fields.extend(guid_pair(SUMMONEDBY, OTHER));
            set_pet(rig, &fields);
        },
    ),
    ("the pet possessed, which greys nothing", |rig| {
        set_pet(rig, &[(FLAGS, UNIT_FLAG_POSSESSED)]);
    }),
    ("the pet in combat", |rig| set_pet(rig, &[(FLAGS, 0x0800)])),
];

/// Every exit alone, then every pet that trips none: the cases the rest of the file runs each
/// verb over.
fn all_cases() -> impl Iterator<Item = Case> {
    let usable = USABLE.iter().map(|&(name, apply)| Case {
        name,
        apply,
        usable: true,
        draggable: !name.contains("possessed"),
    });
    let exits = EXITS.iter().map(|&(name, apply)| Case {
        name,
        apply,
        usable: false,
        // Every other exit still lifts the action; only a pet that does not resolve returns first.
        draggable: !name.starts_with("the pet does not resolve"),
    });
    usable.chain(exits)
}

/// What a press did, by name, so one assert says which effect leaked and for which case.
fn effects(rig: &Rig, sent: Vec<ClientCommand>) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = sent
        .iter()
        .map(|c| match c {
            ClientCommand::PetAction { .. } => "PetAction",
            ClientCommand::PetCancelAura { .. } => "PetCancelAura",
            _ => "another command",
        })
        .collect();
    if !rig.pet_list_untouched() {
        out.push("a GCD armed");
    }
    out
}

fn ui<R>(rig: &mut Rig, f: impl FnOnce(&mut benilla_ui::script::UiScript) -> R) -> R {
    let mut script = rig
        .app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>();
    f(&mut script)
}

/// `GetPetActionsUsable()` as the Lua reads it after the feed ran.
fn usable_answer(rig: &mut Rig) -> bool {
    rig.frame();
    ui(rig, |s| {
        s.eval::<bool>("return GetPetActionsUsable() ~= nil")
            .unwrap()
    })
}

/// `0x5d53c0`: the display answers nil for each exit alone, and 1 for every pet that trips none.
#[test]
fn each_exit_alone_makes_get_pet_actions_usable_nil() {
    for Case {
        name,
        apply,
        usable,
        ..
    } in all_cases()
    {
        let mut rig = scene();
        apply(&mut rig);
        assert_eq!(usable_answer(&mut rig), usable, "{name}");
    }
}

/// `0x5d4210`: a spell press, and the orders: every one leaves at the gate, before the aura cancel,
/// the latch, the GCD and the send.
#[test]
fn each_exit_alone_stops_a_press_and_every_order() {
    for Case {
        name,
        apply,
        usable,
        ..
    } in all_cases()
    {
        let mut rig = scene();
        apply(&mut rig);
        rig.frame();
        let before = latched(&rig);

        rig.press(1);
        let sent = rig.sent();
        let got = effects(&rig, sent);
        if usable {
            // A possessed unit's press takes the generic cast entry, which arms its own GCD.
            assert!(got.contains(&"PetAction"), "{name}: a spell press: {got:?}");
        } else {
            assert_eq!(got, Vec::<&str>::new(), "{name}: a spell press");
            assert_eq!(latched(&rig), before, "{name}: nothing latched");
        }

        for (word, order) in [
            (FOLLOW, "Follow"),
            (STAY, "Stay"),
            (AGGRESSIVE, "Aggressive"),
            (ATTACK, "Attack"),
        ] {
            let before = latched(&rig);
            rig.order(word);
            let sent = rig.sent();
            let went = sent
                .iter()
                .any(|c| matches!(c, ClientCommand::PetAction { packed, .. } if *packed == word));
            assert_eq!(went, usable, "{name}: {order} sends only on a usable pet");
            if !usable {
                assert_eq!(latched(&rig), before, "{name}: {order} latched nothing");
            }
        }
    }
}

/// `0x5d66f0`: the bar's `TogglePetAutocast` flips the word and sends only for a usable pet.
#[test]
fn each_exit_alone_stops_the_bars_autocast_toggle() {
    for Case {
        name,
        apply,
        usable,
        ..
    } in all_cases()
    {
        let mut rig = scene();
        apply(&mut rig);
        rig.frame();
        let word = rig.app.world().resource::<PetBar>().spells.bar[0];

        ui(&mut rig, |s| s.run("TogglePetAutocast(1)").unwrap());
        rig.app
            .world_mut()
            .run_system_once(super::drain::drain_pet_actions)
            .expect("the drain runs");

        let sent = rig.sent();
        let flipped = rig.app.world().resource::<PetBar>().spells.bar[0] != word;
        assert_eq!(flipped, usable, "{name}: the word flips only when usable");
        assert_eq!(
            sent.iter()
                .filter(|c| matches!(c, ClientCommand::PetSetAction { .. }))
                .count(),
            usize::from(usable),
            "{name}: the toggle is sent only when usable"
        );
    }
}

/// `0x5d5e10`: the pet book's `ToggleSpellAutocast` flips the word, repaints and sends only for a
/// usable pet.
#[test]
fn each_exit_alone_stops_the_pet_books_autocast_toggle() {
    for Case {
        name,
        apply,
        usable,
        ..
    } in all_cases()
    {
        let mut rig = scene();
        apply(&mut rig);
        rig.frame();
        let (word, signals) = {
            let bar = rig.app.world().resource::<PetBar>();
            (bar.spells.spells[0], bar.bar_signals)
        };

        ui(&mut rig, |s| {
            s.run("ToggleSpellAutocast(1, BOOKTYPE_PET)").unwrap()
        });
        rig.app
            .world_mut()
            .run_system_once(crate::ui_pet_book::drain_pet_book)
            .expect("the drain runs");

        let sent = rig.sent();
        let bar = rig.app.world().resource::<PetBar>();
        assert_eq!(
            bar.spells.spells[0] != word,
            usable,
            "{name}: the book's word flips only when usable"
        );
        assert_eq!(
            bar.bar_signals != signals,
            usable,
            "{name}: the bar repaints only when usable"
        );
        assert_eq!(
            sent.iter()
                .filter(|c| matches!(c, ClientCommand::PetSpellAutocast { .. }))
                .count(),
            usize::from(usable),
            "{name}: the toggle is sent only when usable"
        );
    }
}

/// `0x5d46f0`: the pet menu's Dismiss hands its command word to the dispatcher, so it is refused
/// at `0x5d4210` like any other order.
#[test]
fn each_exit_alone_stops_the_menus_dismiss() {
    const DISMISS: u32 = 0x0700_0003;
    for Case {
        name,
        apply,
        usable,
        ..
    } in all_cases()
    {
        let mut rig = scene();
        apply(&mut rig);
        rig.frame();

        ui(&mut rig, |s| s.run("PetDismiss()").unwrap());
        rig.app
            .world_mut()
            .run_system_once(super::menu::drain_pet_menu)
            .expect("the drain runs");

        let went = rig
            .sent()
            .iter()
            .any(|c| matches!(c, ClientCommand::PetAction { packed, .. } if *packed == DISMISS));
        assert_eq!(went, usable, "{name}: Dismiss sends only on a usable pet");
    }
}

/// `0x4bc9d0`, from the drop `0x4bce33` and the pickup's blank `0x4be27f`: lifting Claw and
/// dropping it on the next slot writes and sends nothing on an unusable pet, and two writes on a
/// usable one.
#[test]
fn each_exit_alone_stops_the_drags_writes() {
    for Case {
        name,
        apply,
        usable,
        draggable,
    } in all_cases()
    {
        let writes = usable && draggable;
        let mut rig = scene();
        apply(&mut rig);
        rig.frame();
        let bar_before = rig.app.world().resource::<PetBar>().spells.bar;

        ui(&mut rig, |s| {
            s.run("PickupPetAction(1) PickupPetAction(2)").unwrap();
        });
        rig.app
            .world_mut()
            .run_system_once(super::drain::drain_pet_actions)
            .expect("the drain runs");

        let sent = rig.sent();
        let bar = rig.app.world().resource::<PetBar>().spells.bar;
        assert_eq!(
            sent.iter()
                .filter(|c| matches!(c, ClientCommand::PetSetAction { .. }))
                .count(),
            if writes { 2 } else { 0 },
            "{name}: the pickup's blank and the drop send only when usable"
        );
        assert_eq!(
            bar != bar_before,
            writes,
            "{name}: the bar's words move only when usable"
        );
    }
}

/// `0x4be1f7`-`0x4be20a`: `PickupPetAction` looks the pet up and returns at once with no object,
/// then again on `UNIT_FLAG_POSSESSED`, so a drag from an unstreamed or a possessed pet's bar lifts
/// nothing. Every other state lifts the action, an unusable pet included: `0x4bc9d0` refuses only
/// the pickup's blank, never the lift (`0x4be25d`).
#[test]
fn the_pickup_lifts_only_with_the_pets_object_held_and_not_possessed() {
    for Case {
        name,
        apply,
        draggable,
        ..
    } in all_cases()
    {
        let mut rig = scene();
        apply(&mut rig);
        rig.frame();

        ui(&mut rig, |s| s.run("PickupPetAction(1)").unwrap());
        let held = ui(&mut rig, |s| {
            matches!(
                s.cursor_payload(),
                Some(benilla_ui::script::CursorPayload::PetAction(c)) if c.src_slot == 1
            )
        });
        assert_eq!(
            held, draggable,
            "{name}: the action is lifted onto the cursor"
        );
    }
}

/// The same lookup sits above the cursor fork, so a held action is neither dropped nor cleared
/// while the pet's object is gone: the drop's `ClearCursor` (`0x4be220`) is past it.
#[test]
fn a_held_action_stays_held_while_the_pet_is_out_of_view() {
    let mut rig = scene();
    ui(&mut rig, |s| s.run("PickupPetAction(1)").unwrap());
    rig.app
        .world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .remove(&PET);
    rig.frame();
    rig.app
        .world_mut()
        .run_system_once(super::drain::drain_pet_actions)
        .expect("the drain runs");
    let _ = rig.sent();

    ui(&mut rig, |s| s.run("PickupPetAction(2)").unwrap());
    rig.app
        .world_mut()
        .run_system_once(super::drain::drain_pet_actions)
        .expect("the drain runs");

    assert!(
        matches!(
            ui(&mut rig, |s| s.cursor_payload()),
            Some(benilla_ui::script::CursorPayload::PetAction(c)) if c.src_slot == 1
        ),
        "the payload is still on the cursor"
    );
    assert!(
        !rig.sent()
            .iter()
            .any(|c| matches!(c, ClientCommand::PetSetAction { .. })),
        "and nothing was written"
    );
}

/// The whole default UI on the rig's world in place of its listening VM: the stock pet bar, and
/// the fed pet under it.
fn with_stock_ui(rig: &mut Rig) {
    let mut script = benilla_ui::script::UiScript::new().expect("a VM");
    script.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, so a player exists.
    script.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = crate::ui_script::load_default_ui(&script);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    rig.app.world_mut().insert_non_send_resource(script);
}

/// The feeds, then the VM's clock and layout, so the bar's slide and repaint have happened.
fn settle(rig: &mut Rig) {
    rig.feed.run(rig.app.world_mut());
    ui(rig, |s| {
        for _ in 0..3 {
            s.tick(0.05);
        }
        s.resolve();
    });
}

/// Whether the button's icon quad draws greyscale: `PetActionBar_Update`'s `SetDesaturation` on
/// `PetActionButton1Icon` (`PetActionBarFrame.lua:129-134`).
fn icon_is_grey(rig: &mut Rig, icon: &str) -> bool {
    ui(rig, |s| {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                benilla_ui::script::QuadContent::Texture {
                    path: Some(p),
                    desaturated,
                    ..
                } if p == icon => Some(*desaturated),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the icon {icon} is on screen"))
    })
}

/// A left click on `PetActionButton1`, at its own centre, and the frame's script calls applied.
fn click_first_button(rig: &mut Rig) {
    let (x, y): (f32, f32) = ui(rig, |s| {
        s.eval("return PetActionButton1:GetCenter()").unwrap()
    });
    ui(rig, |s| {
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_button(x, y, "LeftButton", false);
    });
    rig.app
        .world_mut()
        .run_system_once(crate::script_calls::apply_script_calls)
        .expect("the frame's script calls apply");
}

/// With the default UI loaded, the stock bar reads `GetPetActionsUsable` for its icons and clicks
/// `CastPetAction` whatever it answered, so the engine's gate is the only one: a stunned pet's bar
/// greys, and clicking it sends nothing (`0x5d4210`), where the same click on the pet before it was
/// stunned went out.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn the_stock_bar_greys_a_stunned_pet_and_its_click_sends_nothing() {
    benilla_formats::wow_data_or_skip!();
    const ICON: &str = "Interface\\Icons\\Ability_Druid_Rake";
    let display = benilla_formats::SpellDisplay {
        icon: Some(ICON.into()),
        ..claw()
    };
    let mut rig = rig(vec![(CLAW, display)], &[CLAW], Some(owned_pet()));
    with_stock_ui(&mut rig);
    settle(&mut rig);

    assert!(
        !icon_is_grey(&mut rig, ICON),
        "a usable pet's icon is in colour"
    );
    click_first_button(&mut rig);
    assert_eq!(
        pet_actions(&rig.sent()),
        1,
        "and its button presses, as before"
    );

    set_pet(&mut rig, &[(FLAGS, 0x0004_0000)]);
    settle(&mut rig);

    assert!(icon_is_grey(&mut rig, ICON), "a stunned pet's bar greys");
    let before = latched(&rig);
    click_first_button(&mut rig);
    assert!(
        rig.sent().is_empty(),
        "the click on the greyed button sends nothing"
    );
    assert_eq!(latched(&rig), before);

    // The same click again once the stun ends.
    set_pet(&mut rig, &[]);
    settle(&mut rig);
    assert!(!icon_is_grey(&mut rig, ICON));
    click_first_button(&mut rig);
    assert_eq!(pet_actions(&rig.sent()), 1);
}

/// `0x5d3310` reads three flags and never the pet's health: a dead pet's bar is usable,
/// its press goes out, and only the Attack order's own validator refuses it (`0x612e10`).
#[test]
fn a_dead_pet_is_still_usable() {
    let mut rig = scene();
    set_pet(&mut rig, &[(22, 0)]);

    assert!(usable_answer(&mut rig));
    rig.press(1);
    assert_eq!(pet_actions(&rig.sent()), 1, "the spell press goes out");
    rig.order(FOLLOW);
    assert_eq!(pet_actions(&rig.sent()), 1, "and so does Follow");
    rig.order(ATTACK);
    assert_eq!(
        pet_actions(&rig.sent()),
        0,
        "the attack validator refuses the dead actor, past the gate"
    );
}

/// The predicate on its own, over every field it reads, so the exit table above is what the
/// function says and not what the rig happens to build.
#[test]
fn the_predicate_reads_exactly_the_reference_fields() {
    use super::usable::actions_usable;
    let store = |fields: &[(u16, u32)]| {
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(fields))
    };
    let bar = PetBar {
        spells: benilla_protocol::messages::PetSpells {
            pet_guid: PET,
            ..Default::default()
        },
        ..Default::default()
    };
    let me = Some(ME);
    let player = store(&[(2, 0x19)]);
    let pet = owned_pet();

    assert!(actions_usable(&bar, me, Some(&player), Some(&pet), None));
    assert!(!actions_usable(&bar, None, Some(&player), Some(&pet), None));
    assert!(!actions_usable(&bar, me, None, Some(&pet), None));
    assert!(!actions_usable(&bar, me, Some(&player), None, None));

    // The owner is `CHARMEDBY` when set, else `SUMMONEDBY`, and never `CREATEDBY`.
    let created_only = store(&[(14, ME as u32), (15, 0)]);
    assert!(!actions_usable(
        &bar,
        me,
        Some(&player),
        Some(&created_only),
        None
    ));
    let unowned = store(&[]);
    assert!(!actions_usable(
        &bar,
        me,
        Some(&player),
        Some(&unowned),
        None
    ));

    // A far-sight guid of zero is no view; the pet's own guid is the one that stays usable.
    let zero_view = store(&[(FARSIGHT, 0), (FARSIGHT + 1, 0)]);
    assert!(actions_usable(&bar, me, Some(&zero_view), Some(&pet), None));
    let on_pet = store(&[(FARSIGHT, PET as u32), (FARSIGHT + 1, 0)]);
    assert!(actions_usable(&bar, me, Some(&on_pet), Some(&pet), None));
    let elsewhere = store(&[(FARSIGHT, OTHER as u32), (FARSIGHT + 1, 0)]);
    assert!(!actions_usable(
        &bar,
        me,
        Some(&elsewhere),
        Some(&pet),
        None
    ));
    // The high word counts: a view whose low word is the pet's and whose high word is not.
    let high_only = store(&[(FARSIGHT, PET as u32), (FARSIGHT + 1, 1)]);
    assert!(!actions_usable(
        &bar,
        me,
        Some(&high_only),
        Some(&pet),
        None
    ));

    // Each of the three flags alone, and no other.
    for flag in [0x0004_0000, 0x0040_0000, 0x0080_0000] {
        let hurt = owned_pet_with(&[(FLAGS, flag)]);
        assert!(
            !actions_usable(&bar, me, Some(&player), Some(&hurt), None),
            "flag {flag:#x} disables"
        );
    }
    let possessed = owned_pet_with(&[(FLAGS, UNIT_FLAG_POSSESSED)]);
    assert!(actions_usable(
        &bar,
        me,
        Some(&player),
        Some(&possessed),
        None
    ));

    let mut disabled = PetBar {
        spells: bar.spells.clone(),
        ..Default::default()
    };
    disabled.spells.state |= PET_STATE_VEHICLE;
    assert!(!actions_usable(
        &disabled,
        me,
        Some(&player),
        Some(&pet),
        None
    ));
    // A vehicle's bar works only while we ride that very pet (`0x5d3f77`).
    assert!(actions_usable(
        &disabled,
        me,
        Some(&player),
        Some(&pet),
        Some(PET)
    ));
    assert!(!actions_usable(
        &disabled,
        me,
        Some(&player),
        Some(&pet),
        Some(OTHER)
    ));
    assert!(actions_usable(
        &bar,
        me,
        Some(&player),
        Some(&pet),
        Some(OTHER)
    ));
}

/// `0x5d3fd0` per slot: an order follows the bar's predicate; a spell asks it without the control
/// test, then refuses a stun, fear or confusion its `AttributesEx5` does not exempt, then asks the
/// usable walk.
#[test]
fn a_slot_is_usable_as_its_kind_and_the_spells_exemptions_say() {
    use super::usable::slot_usable;
    let order = PetActionEntry::from(PET_COMMAND_FOLLOW | (u32::from(PET_ACT_COMMAND) << 24));
    let spell = PetActionEntry::from(CLAW | (u32::from(PET_ACT_ENABLED) << 24));
    let mut d = benilla_formats::SpellDisplay::default();
    let pet = owned_pet();
    let walk_ok = |_: &benilla_formats::SpellDisplay, _: &crate::net::ObjectStore| true;

    assert!(slot_usable(order, None, true, None, walk_ok));
    assert!(!slot_usable(order, None, false, Some(&pet), walk_ok));
    assert!(slot_usable(spell, Some(&d), false, Some(&pet), walk_ok));
    assert!(
        !slot_usable(spell, Some(&d), true, None, walk_ok),
        "no pet past the gate"
    );
    assert!(
        !slot_usable(spell, None, true, Some(&pet), walk_ok),
        "no record"
    );
    assert!(!slot_usable(spell, Some(&d), true, Some(&pet), |_, _| {
        false
    }));

    // Each state alone greys a spell, unless its own exemption bit is set.
    for (flag, exempt) in [
        (0x4_0000, 0x8),
        (0x80_0000, 0x2_0000),
        (0x40_0000, 0x4_0000),
    ] {
        let hurt = owned_pet_with(&[(FLAGS, flag)]);
        d.attributes_ex5 = 0;
        assert!(
            !slot_usable(spell, Some(&d), true, Some(&hurt), walk_ok),
            "{flag:#x}"
        );
        d.attributes_ex5 = exempt;
        assert!(
            slot_usable(spell, Some(&d), true, Some(&hurt), walk_ok),
            "{flag:#x} exempt"
        );
    }
}

/// `PetAbandon` (`0x5d4900`) sends `CMSG_PET_ABANDON`, then lists the stable again while a stable
/// master is held, and only then.
#[test]
fn an_abandon_at_a_stable_master_lists_the_stable_again() {
    const MASTER: u64 = 0x5_0000;
    for at_stable in [false, true] {
        let mut rig = scene();
        if at_stable {
            rig.app
                .world_mut()
                .resource_mut::<crate::ui_stable::StableOpen>()
                .open(MASTER, 0, Vec::new());
        }
        rig.frame();
        ui(&mut rig, |s| s.run("PetAbandon()").unwrap());
        rig.app
            .world_mut()
            .run_system_once(super::menu::drain_pet_menu)
            .expect("the drain runs");
        let sent = rig.sent();
        assert!(sent
            .iter()
            .any(|c| matches!(c, ClientCommand::PetAbandon { pet_guid } if *pet_guid == PET)));
        let relisted = sent
            .iter()
            .any(|c| matches!(c, ClientCommand::ListStabledPets { npc } if *npc == MASTER));
        assert_eq!(relisted, at_stable, "at a stable master: {at_stable}");
    }
}
