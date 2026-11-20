//! The error lines through a real Lua VM: [`super::feed::feed_actions`]'s drains, a cast
//! refusal's display ([`super::show_refusal`]) and a handler's line in its turn.

use std::collections::HashMap;

use benilla_formats::{ItemDisplay, ItemDisplayCatalog};
use benilla_protocol::messages::{ActionButton, ItemSpellEntry, ACTION_KIND_ITEM};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::feed::feed_actions;
use super::{CastFail, PlayerActions, UiErrorKeys, UiErrorTexts};
use crate::entities::ItemDisplays;
use crate::items::{test_template, Items};
use crate::net::{ClientCommand, NetCommands};

/// A new human warrior's food button (vmangos `playercreateinfo_action`): Tough Jerky on wire
/// slot 83, the Battle Stance page.
const JERKY: u32 = 117;
const JERKY_SLOT: u8 = 83;
/// Tough Jerky's display id and its `ItemDisplayInfo.dbc` icon.
const JERKY_DISPLAY: u32 = 2473;
const JERKY_ICON: &str = "Interface\\Icons\\INV_Misc_Food_16";

/// The feed with the food button on the bar and a cold template cache, as at login: the error
/// drains' rig.
fn app_with_food_on_the_bar() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut actions = crate::ui_action::ActionSlots::default();
    actions.buttons.insert(
        JERKY_SLOT,
        ActionButton {
            slot: JERKY_SLOT,
            action: JERKY,
            kind: ACTION_KIND_ITEM,
        },
    );
    // As `SMSG_ACTION_BUTTONS` sets it; the feed's first pass clears it.
    actions.dirty = true;

    let displays = HashMap::from([(
        JERKY_DISPLAY,
        ItemDisplay {
            icon: Some(JERKY_ICON.to_string()),
            ..Default::default()
        },
    )]);

    app.insert_resource(actions)
        .init_resource::<PlayerActions>()
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<UiErrorKeys>()
        .init_resource::<UiErrorTexts>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .insert_resource(ItemDisplays::icons_for_tests(
            ItemDisplayCatalog::from_displays(displays),
        ))
        .insert_resource(NetCommands(tx));
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.add_systems(Update, feed_actions);
    (app, rx)
}

/// `IsConsumableAction`'s two clauses; the item class is neither.
#[test]
fn is_consumable_is_ammo_thrown_or_a_negative_charge_use_block() {
    let block = |trigger: u32, charges: i32| ItemSpellEntry {
        index: 0,
        spell_id: 439,
        trigger,
        charges,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    };

    // A mount: class 15, InventoryType 0, one ON_USE block with charges 0 (not used up).
    let mut mount = test_template("Red Skeletal Horse");
    mount.class = 15;
    mount.spells = vec![block(0, 0)];
    assert!(!mount.is_consumable(), "a mount has no stack to show");

    // A potion: its ON_USE block's -1 charges decide, not its class 0.
    let mut potion = test_template("Minor Healing Potion");
    potion.spells = vec![block(0, -1)];
    assert!(potion.is_consumable());

    // Class 0 with no ON_USE block is not consumable.
    let classless = test_template("Trade Good");
    assert!(!classless.is_consumable());

    // Ammo (24) and thrown (25) always count, charges or not.
    for inv in [24u32, 25] {
        let mut ammo = test_template("Rough Arrow");
        ammo.inventory_type = inv;
        assert!(ammo.is_consumable(), "InventoryType {inv} is consumable");
    }
    let mut trinket = test_template("Trinket");
    trinket.inventory_type = 12;
    assert!(!trinket.is_consumable());

    // An ON_EQUIP proc with negative charges is not an ON_USE block: the trigger must be 0.
    let mut proc_item = test_template("Proc Weapon");
    proc_item.spells = vec![block(1, -1)];
    assert!(!proc_item.is_consumable());
}

/// `UiErrorTexts` end to end into the stock `UIErrorsFrame`: an `SMSG_NOTIFICATION` draws red,
/// an `SMSG_AREA_TRIGGER_MESSAGE` yellow.
#[test]
fn pre_resolved_lines_land_on_the_errors_frame_in_the_arms_colour() {
    benilla_formats::wow_data_or_skip!();
    let (mut app, _rx) = app_with_food_on_the_bar();
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_screen_size(1024.0, 768.0);
        // Stock files, read from the install.
        for file in [
            "Interface\\FrameXML\\Fonts.xml",
            "Interface\\FrameXML\\UIErrorsFrame.xml",
        ] {
            crate::ui_script::load_ui_for_test(&script, file);
        }
    }

    // What the net drain queues for one `.gm on`, plus a refused portal.
    let mut texts = app.world_mut().resource_mut::<UiErrorTexts>();
    texts.error("GM mode is ON".to_string());
    texts.info("You must be at least level 58 to enter.".to_string());
    app.update();

    assert!(
        app.world().resource::<UiErrorTexts>().0.is_empty(),
        "the feed drains the queue"
    );

    let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
    assert!(
        script.errors().is_empty(),
        "VM errors: {:?}",
        script.errors()
    );
    script.resolve();
    let mut drawn: Vec<(String, [f32; 4])> = script
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            benilla_ui::script::QuadContent::Text {
                text: Some(t),
                color: Some(c),
                ..
            } if !t.is_empty() => Some((t.clone(), *c)),
            _ => None,
        })
        .collect();
    drawn.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        drawn,
        [
            // `AddMessage` quantizes each channel to a byte (`ftol(v*255 + 0.5)`): 0.1 is 26/255.
            (
                "GM mode is ON".to_string(),
                [1.0, 26.0 / 255.0, 26.0 / 255.0, 1.0]
            ),
            (
                "You must be at least level 58 to enter.".to_string(),
                [1.0, 1.0, 0.0, 1.0],
            ),
        ],
        "red UI_ERROR_MESSAGE for the notice, yellow UI_INFO_MESSAGE for the area trigger"
    );
}

/// The player's refused cast logs (`0x751ad0`) and the pet's does not. Both entries name the same
/// spell, so only the caster differs.
#[test]
fn a_pets_refused_cast_writes_no_combat_log_line() {
    use crate::ui_action::{Caster, Spells};
    use benilla_formats::{SpellCatalog, SpellDisplay};

    const GROWL: u32 = 2649;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    // 0x5f ROOTED has a string on both tables, so neither entry drops for want of one.
    let errors = [
        CastFail::local(GROWL, 0x5F),
        CastFail {
            caster: Caster::Pet,
            ..CastFail::local(GROWL, 0x5F)
        },
    ];

    app.insert_resource(PlayerActions::default())
        .insert_resource(Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                GROWL,
                SpellDisplay {
                    name: "Growl".into(),
                    ..Default::default()
                },
            )])),
            ..Spells::empty_for_tests()
        })
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<UiErrorKeys>()
        .init_resource::<UiErrorTexts>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .add_message::<crate::combat_log::SpellCastFailedLog>()
        .insert_resource(NetCommands(tx));
    let script = UiScript::new().unwrap();
    // Only what the log entry needs; the red line's text has its own tests.
    script.run(r#"SPELL_FAILED_ROOTED = "A reason";"#).unwrap();
    app.insert_non_send_resource(script);
    show_all(&mut app, errors);

    let logged: Vec<(u32, String)> = app
        .world_mut()
        .resource_mut::<Messages<crate::combat_log::SpellCastFailedLog>>()
        .drain()
        .map(|f| (f.spell_id, f.reason))
        .collect();
    assert_eq!(
        logged,
        [(GROWL, "A reason".to_string())],
        "the player's failure logs, with the shown text, and the pet's does not"
    );
}

/// A custom refusal (wire 172) shows the row its word names and, unlike a plain refusal, writes
/// no combat log line: the player's handler sets its shown flag (`0x808537`).
#[test]
fn a_custom_refusal_shows_its_row_and_is_not_logged() {
    use crate::ui_action::cast_fail::CUSTOM_ERROR;
    use crate::ui_action::Spells;

    const FIRE_NOVA: u32 = 61657;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let errors = [
        CastFail {
            arg: Some(83),
            ..CastFail::local(FIRE_NOVA, CUSTOM_ERROR)
        },
        CastFail::local(FIRE_NOVA, 0x5F),
    ];

    app.insert_resource(PlayerActions::default())
        .insert_resource(Spells::empty_for_tests())
        .init_resource::<Items>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<UiErrorKeys>()
        .init_resource::<UiErrorTexts>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .add_message::<crate::combat_log::SpellCastFailedLog>()
        .insert_resource(NetCommands(tx));
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"SPELL_FAILED_CUSTOM_ERROR_83 = "A fire totem line"
               SPELL_FAILED_ROOTED = "A reason"
               BENILLA_TEST_LINES = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("UI_ERROR_MESSAGE")
               f:SetScript("OnEvent", function(self, event, arg1)
                   table.insert(BENILLA_TEST_LINES, arg1)
               end)"#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    show_all(&mut app, errors);

    let shown = app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<String>("return table.concat(BENILLA_TEST_LINES, '|')")
        .unwrap();
    assert_eq!(shown, "A fire totem line|A reason", "each shows once");
    let logged: Vec<String> = app
        .world_mut()
        .resource_mut::<Messages<crate::combat_log::SpellCastFailedLog>>()
        .drain()
        .map(|f| f.reason)
        .collect();
    assert_eq!(logged, ["A reason"], "only the plain refusal logs");
}

/// `0x808200`'s two-word arms: `NEED_MORE_ITEMS` (wire 55) names the item and the count, and
/// `MIN_SKILL` (wire 150) the skill line and the value; the pet's table fills neither.
#[test]
fn need_more_items_and_min_skill_fill_both_argument_words() {
    use crate::ui_action::{Caster, Spells};
    use benilla_formats::{SkillLineCatalog, SkillLineInfo};

    const PROSPECTING: u32 = 31252;
    const COPPER_ORE: u32 = 2770;
    const JEWELCRAFTING: u32 = 755;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let fail = |reason, arg, arg2, caster| CastFail {
        spell_id: PROSPECTING,
        reason,
        arg: Some(arg),
        arg2: Some(arg2),
        caster,
        redisplay: false,
    };
    let errors = [
        fail(0x97, COPPER_ORE, 5, Caster::Player),
        fail(0x90, JEWELCRAFTING, 50, Caster::Player),
        fail(0x90, JEWELCRAFTING, 50, Caster::Pet),
    ];
    let mut items = Items::default();
    items.insert_template(COPPER_ORE, Some(test_template("Copper Ore")));
    let lines = SkillLineCatalog::from_lines([(
        JEWELCRAFTING,
        SkillLineInfo {
            name: "Jewelcrafting".into(),
            category_id: 11,
            icon: None,
            description: String::new(),
            alternate_verb: String::new(),
            can_link: false,
            costs_id: 0,
        },
    )]);

    app.insert_resource(PlayerActions::default())
        .insert_resource(Spells::empty_for_tests())
        .insert_resource(items)
        .insert_resource(crate::ui_spellbook::SkillLines {
            catalog: std::sync::Arc::new(lines),
        })
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<UiErrorKeys>()
        .init_resource::<UiErrorTexts>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .add_message::<crate::combat_log::SpellCastFailedLog>()
        .insert_resource(NetCommands(tx));
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"SPELL_FAILED_NEED_MORE_ITEMS = "Requires %d %s."
               SPELL_FAILED_MIN_SKILL = "Your skill is not high enough.  Requires %s (%d).""#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    show_all(&mut app, errors);

    let logged: Vec<String> = app
        .world_mut()
        .resource_mut::<Messages<crate::combat_log::SpellCastFailedLog>>()
        .drain()
        .map(|f| f.reason)
        .collect();
    assert_eq!(
        logged,
        [
            "Requires 5 Copper Ore.",
            "Your skill is not high enough.  Requires Jewelcrafting (50).",
        ],
        "the player's two lines; the pet's is never logged"
    );
}

/// A reagent refusal naming an item not yet cached shows nothing at first and links its callback
/// on the item's record (`0x8086d4` -> `0x7fea80`); the answer shows it, with no log line.
#[test]
fn a_refusal_naming_an_uncached_item_shows_when_the_answer_lands() {
    use crate::ui_action::Spells;
    use benilla_formats::{SpellCatalog, SpellDisplay};

    const SMELT_COPPER: u32 = 2657;
    const COPPER_ORE: u32 = 2770;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut reagents = [(0, 0); 8];
    reagents[0] = (COPPER_ORE, 1);
    app.insert_resource(Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SMELT_COPPER,
            SpellDisplay {
                name: "Smelt Copper".into(),
                reagents,
                ..Default::default()
            },
        )])),
        ..Spells::empty_for_tests()
    })
    .init_resource::<Items>()
    .init_resource::<crate::net::GuidIndex>()
    .init_resource::<crate::ui_chat::ChatLog>()
    .init_resource::<crate::sound::MessageSounds>()
    .add_message::<crate::combat_log::SpellCastFailedLog>()
    .insert_resource(NetCommands(tx));
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"SPELL_FAILED_REAGENTS = "Missing reagent: %s"
               BENILLA_TEST_LINES = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("UI_ERROR_MESSAGE")
               f:SetScript("OnEvent", function(_, _, t) table.insert(BENILLA_TEST_LINES, t) end)"#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_non_send_resource(crate::items::RecordWaits::default());
    let shown = |app: &App| {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<String>("return table.concat(BENILLA_TEST_LINES, '|')")
            .unwrap()
    };

    show_all(&mut app, [CastFail::local(SMELT_COPPER, 0x5c)]);
    assert_eq!(shown(&app), "", "the first pass waits on the record");
    let waits = app
        .world_mut()
        .non_send_resource_mut::<crate::items::RecordWaits>()
        .take(COPPER_ORE);
    let redisplay = CastFail::local(SMELT_COPPER, 0x5c).requeued();
    assert_eq!(
        waits,
        [crate::items::RecordWait::CastFailure(redisplay)],
        "the line waits on the ore's record"
    );
    // The answer lands and runs the record's callbacks, as the item template handler does.
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(COPPER_ORE, Some(test_template("Copper Ore")));
    super::show_refusal(app.world_mut(), redisplay);
    assert_eq!(shown(&app), "Missing reagent: Copper Ore");
    let logged = app
        .world_mut()
        .resource_mut::<Messages<crate::combat_log::SpellCastFailedLog>>()
        .drain()
        .count();
    assert_eq!(logged, 0, "a redisplay is not logged");
}

/// The reference resolves `PETTAME_<reason>` first and passes the string to the error display as
/// error id `0xee`, so `ERR_TAME_FAILED` ("%s.") prints the sentence, not the key; the line shows
/// in the handler's turn.
#[test]
fn a_tame_failure_composes_the_reason_string_into_err_tame_failed() {
    let (mut app, _rx) = app_with_food_on_the_bar();
    {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        // The strings verbatim from 1.12's `GlobalStrings.lua`, and a recorder for the event.
        script
            .run(
                r#"
                ERR_TAME_FAILED = "%s.";
                PETTAME_TOOHIGHLEVEL = "Creature is too high level for you to tame";
                PETTAME_UNKNOWNERROR = "Unknown taming error";
                BENILLA_TEST_LINES = {};
                local f = CreateFrame("Frame")
                f:RegisterEvent("UI_ERROR_MESSAGE")
                f:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9)
                    table.insert(BENILLA_TEST_LINES, arg1)
                end)
                "#,
            )
            .unwrap();
    }
    let line = |app: &mut App, i: usize| {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<Option<String>>(&format!("return BENILLA_TEST_LINES[{i}]"))
            .unwrap()
    };

    // 9 is `PETTAME_TOOHIGHLEVEL`; 13, past the reference's switch (`0x800390`), takes the
    // default arm.
    show_tame(&mut app, 9);
    show_tame(&mut app, 13);
    assert_eq!(
        line(&mut app, 1).as_deref(),
        Some("Creature is too high level for you to tame."),
        "the reason string fills ERR_TAME_FAILED's %s — not the key, and not the reason alone"
    );
    assert_eq!(
        line(&mut app, 2).as_deref(),
        Some("Unknown taming error."),
        "out of range takes the default arm rather than showing nothing"
    );

    // A reason whose `PETTAME_` string is not loaded fills the key, as the VM's string lookup
    // returns it (`0x819d40`).
    show_tame(&mut app, 1);
    assert_eq!(
        line(&mut app, 3).as_deref(),
        Some("PETTAME_INVALIDCREATURE."),
        "an unloaded reason string shows its key"
    );
}

/// Show each refusal as a press or a handler does, in order.
fn show_all(app: &mut App, fails: impl IntoIterator<Item = CastFail>) {
    for fail in fails {
        super::show_refusal(app.world_mut(), fail);
    }
}

/// `SMSG_PET_TAME_FAILURE`'s line in a handler's turn.
fn show_tame(app: &mut App, reason: u8) {
    let world = app.world_mut();
    world.init_resource::<crate::ui_script::live::UiEvents>();
    crate::ui_action::show_in_turn(
        &mut world.resource_mut::<crate::ui_script::live::UiEvents>(),
        "test",
        crate::ui_pet::tame_failure_line(reason),
    );
    crate::ui_script::live::fire_handler_events(world);
}
