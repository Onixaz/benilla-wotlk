//! The error lines through a real Lua VM: [`super::feed::feed_actions`]'s drains, a cast
//! refusal's display ([`super::show_refusal`]) and a handler's line in its turn.

use std::collections::HashMap;

use benilla_formats::{ItemDisplay, ItemDisplayCatalog};
use benilla_protocol::messages::{ActionButton, ItemSpellEntry, ACTION_KIND_ITEM};
use benilla_ui::messages::MsgKind;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::feed::feed_actions;
use super::{CastFail, PlayerActions, TextSink};
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

/// A resolved line ([`TextSink`]) end to end into the stock `UIErrorsFrame`, in the handler's turn: an `SMSG_NOTIFICATION` draws red, an `SMSG_AREA_TRIGGER_MESSAGE` yellow.
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

    // What the handlers show for one `.gm on`, plus a refused portal, fired in their turn.
    let mut ui = crate::ui_script::live::UiEvents::default();
    ui.text_line(MsgKind::Error, "GM mode is ON".to_string());
    ui.text_line(
        MsgKind::Info,
        "You must be at least level 58 to enter.".to_string(),
    );
    app.insert_resource(ui);
    crate::ui_script::live::fire_handler_events(app.world_mut());

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
        item: None,
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

/// 12340's arms read the wire's words, not our bags: 100 names the item its word names and its word `-2` shows message 0 (`0x80869a`, `0x8086b2`); 129 names its `ItemLimitCategory.dbc` row on message `0x272` (`0x8080b1`, `0x808a86`); 176 names the casting item off the record's caster (`0x808a0a`-`0x808a66`). Each logs the shown line.
#[test]
fn the_reagent_limit_and_glyph_arms_name_what_12340_names() {
    use crate::ui_action::Spells;
    use benilla_formats::{
        ItemLimitCategory, ItemLimitCategoryCatalog, SpellCatalog, SpellDisplay,
    };

    const SMELT_COPPER: u32 = 2657;
    const COPPER_ORE: u32 = 2770;
    const TIN_ORE: u32 = 2771;
    const GLYPH_SPELL: u32 = 56368;
    const GLYPH_ITEM: u32 = 42739;
    const GLYPH_GUID: u64 = 0x4000_0000_0000_0042;
    const MANA_GEMS: u32 = 4;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut reagents = [(0, 0); 8];
    reagents[0] = (COPPER_ORE, 1);
    let mut items = Items::default();
    items.insert_template(COPPER_ORE, Some(test_template("Copper Ore")));
    items.insert_template(TIN_ORE, Some(test_template("Tin Ore")));
    items.insert_template(GLYPH_ITEM, Some(test_template("Glyph of Fireball")));
    app.insert_resource(Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                SMELT_COPPER,
                SpellDisplay {
                    name: "Smelt Copper".into(),
                    reagents,
                    ..Default::default()
                },
            ),
            (
                GLYPH_SPELL,
                SpellDisplay {
                    name: "Glyph of Fire".into(),
                    ..Default::default()
                },
            ),
        ])),
        ..Spells::empty_for_tests()
    })
    .insert_resource(items)
    .insert_resource(crate::ui_trade::ItemLimitCategories(
        ItemLimitCategoryCatalog::from_rows([ItemLimitCategory {
            id: MANA_GEMS,
            name: "Mana Gem".into(),
            quantity: 3,
            flags: 0,
        }]),
    ))
    .init_resource::<crate::net::GuidIndex>()
    .init_resource::<crate::ui_chat::ChatLog>()
    .init_resource::<crate::sound::MessageSounds>()
    .add_message::<crate::combat_log::SpellCastFailedLog>()
    .insert_resource(NetCommands(tx));
    // The glyph's item object: object and item bits, its template entry.
    let glyph = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x3),
                (benilla_protocol::field::FIELD_OBJECT_ENTRY, GLYPH_ITEM),
            ]),
        ))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(GLYPH_GUID, glyph);
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"SPELL_FAILED_REAGENTS = "Missing reagent: %s"
               ERR_INV_FULL = "Inventory is full."
               SPELL_FAILED_TOO_MANY_OF_ITEM = "Too many of that item already"
               ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS = "You can only carry %d %s"
               SPELL_FAILED_UNIQUE_GLYPH = "%s: That glyph is already inscribed."
               BENILLA_TEST_LINES = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("UI_ERROR_MESSAGE")
               f:SetScript("OnEvent", function(_, _, t) table.insert(BENILLA_TEST_LINES, t) end)"#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    let wire = |spell_id, reason, arg| CastFail {
        arg,
        ..CastFail::local(spell_id, reason)
    };
    show_all(
        &mut app,
        [
            wire(SMELT_COPPER, 0x5c, Some(TIN_ORE)),
            wire(SMELT_COPPER, 0x5c, Some(0xFFFF_FFFE)),
            wire(SMELT_COPPER, 0x77, Some(MANA_GEMS)),
            wire(SMELT_COPPER, 0x77, None),
            CastFail {
                item: Some(GLYPH_GUID),
                ..CastFail::local(GLYPH_SPELL, 0xB6)
            },
        ],
    );
    let shown = app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<String>("return table.concat(BENILLA_TEST_LINES, '|')")
        .unwrap();
    assert_eq!(
        shown,
        [
            "Missing reagent: Tin Ore",
            "Inventory is full.",
            "You can only carry 3 Mana Gem",
            "Too many of that item already",
            "Glyph of Fireball: That glyph is already inscribed.",
        ]
        .join("|")
    );
    let logged: Vec<String> = app
        .world_mut()
        .resource_mut::<Messages<crate::combat_log::SpellCastFailedLog>>()
        .drain()
        .map(|f| f.reason)
        .collect();
    assert_eq!(
        logged[1..3],
        ["Inventory is full.", "You can only carry 3 Mana Gem"]
    );
}

/// 29-31 name the class and mask the wire's two words carry, not the spell's (`0x8085dd`-`0x808613`); 54 the first class-6 subclass under `1 << word` (`0x80865d` -> `0x7fea30`).
#[test]
fn the_equipped_item_arms_name_the_wire_words() {
    use crate::ui_action::Spells;
    use benilla_formats::{SpellCatalog, SpellDisplay};

    const SHOOT: u32 = 5019;
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let subs = benilla_formats::load_item_sub_classes(&mut chain).expect("ItemSubClass.dbc");
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SHOOT,
            SpellDisplay {
                equipped_item_class: 2,
                equipped_item_subclass_mask: 1 << 19,
                ..Default::default()
            },
        )])),
        ..Spells::empty_for_tests()
    })
    .insert_resource(crate::ui_items::ItemSubClasses(subs))
    .init_resource::<Items>()
    .init_resource::<crate::net::GuidIndex>()
    .init_resource::<crate::ui_chat::ChatLog>()
    .init_resource::<crate::sound::MessageSounds>()
    .add_message::<crate::combat_log::SpellCastFailedLog>()
    .insert_resource(NetCommands(tx));
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"SPELL_FAILED_EQUIPPED_ITEM_CLASS = "Must have a %s equipped"
               SPELL_FAILED_NEED_EXOTIC_AMMO = "Requires exotic ammo: %s"
               BENILLA_TEST_LINES = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("UI_ERROR_MESSAGE")
               f:SetScript("OnEvent", function(_, _, t) table.insert(BENILLA_TEST_LINES, t) end)"#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    show_all(
        &mut app,
        [
            CastFail::local(SHOOT, 0x19),
            CastFail {
                arg: Some(2),
                arg2: Some(1 << 15),
                ..CastFail::local(SHOOT, 0x19)
            },
            CastFail {
                arg: Some(2),
                ..CastFail::local(SHOOT, 0x31)
            },
        ],
    );
    let shown = app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<String>("return table.concat(BENILLA_TEST_LINES, '|')")
        .unwrap();
    assert_eq!(
        shown,
        "Must have a Wand equipped|Must have a Dagger equipped|Requires exotic ammo: Arrow"
    );
}

/// The silent exit (`0x8086ff`): arm 4 with no player and a reagent not yet cached show no line and end the cast on `DONT_REPORT` (`0x808705` -> `0x8055c0`), `UNIT_SPELLCAST_FAILED_QUIET`; a shown line ends it on the wire's reason (`0x808b6c`).
#[test]
fn a_silent_exit_ends_the_cast_quietly() {
    use crate::ui_action::Spells;
    use crate::ui_cast::CastBarEdge;
    use benilla_formats::{SpellCatalog, SpellDisplay};

    const SMELT_COPPER: u32 = 2657;
    const COPPER_ORE: u32 = 2770;
    const ME: u64 = 0x11;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut reagents = [(0, 0); 8];
    reagents[0] = (COPPER_ORE, 1);
    app.insert_resource(Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SMELT_COPPER,
            SpellDisplay {
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
    .init_resource::<crate::ui_script::live::UiEvents>()
    .add_message::<crate::combat_log::SpellCastFailedLog>()
    .insert_resource(NetCommands(tx));
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"ERR_SPELL_FAILED_ALREADY_AT_FULL_POWER_S = "You are already at full %s."
               SPELL_FAILED_REAGENTS = "Missing reagent: %s"
               SPELL_FAILED_ROOTED = "You are unable to move""#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_non_send_resource(crate::items::RecordWaits::default());
    let reasons: Vec<u8> = [0x02, 0x5c, 0x5F]
        .into_iter()
        .map(|reason| {
            let fail = CastFail {
                arg: (reason == 0x5c).then_some(COPPER_ORE),
                ..CastFail::local(SMELT_COPPER, reason)
            };
            super::refuse_then_end(app.world_mut(), fail, ME, 1);
            let edges = app
                .world()
                .resource::<crate::ui_script::live::UiEvents>()
                .cast_edges();
            app.world_mut()
                .resource_mut::<crate::ui_script::live::UiEvents>()
                .set_aside();
            match edges.as_slice() {
                [CastBarEdge::Failed { reason, .. }] => *reason,
                other => panic!("one end, got {other:?}"),
            }
        })
        .collect();
    assert_eq!(reasons, [0x17, 0x17, 0x5F]);
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
