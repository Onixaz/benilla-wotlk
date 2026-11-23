//! `SMSG_ITEM_PUSH_RESULT` (`0x523900`), run at its handler's step: the item's record first, then
//! our own push's `ITEM_PUSH`, chat line, tutorials and sound, or another player's chat line. A record
//! not cached is asked, and the whole push runs again from its callback (`0x52398d` -> `0x512b60`).

use benilla_protocol::messages::{ItemInfo, ItemPushResult};
use benilla_ui::script::{ScriptValue, UiScript};
use benilla_ui::strings::Arg;
use bevy::prelude::*;

use super::{push_container, LootState};
use crate::entities::ItemDisplays;
use crate::items::{Items, RecordWait, RecordWaits, RollCatalogs};
use crate::names::NameCache;
use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};
use crate::tutorial::id;
use crate::ui_script::live::UiEvents;

/// `OBJECT_FIELD_TYPE`'s player bit: the push names a player object or shows nothing (`0x5239be`).
const TYPEMASK_PLAYER: u32 = 0x10;
/// The hearthstone's entry (`0x523c0e`).
const HEARTHSTONE: u32 = 6948;
/// The item class a container carries (`0x523be0`).
const CLASS_CONTAINER: u32 = 1;
/// Consumables (class 0) of the food and drink subclasses, 4 and 5 (`0x523b81`, `0x523c39`).
const CLASS_CONSUMABLE: u32 = 0;
const FOOD_AND_DRINK: [u32; 2] = [4, 5];
/// A wand: weapon class 2, subclass 19 (`0x523d92`).
const CLASS_WEAPON: u32 = 2;
const SUBCLASS_WAND: u32 = 19;
/// The ranged inventory types, 15 and 26 (`0x523d79`), and the hunter, who skips them (`0x523d8c`).
const RANGED_TYPES: [u32; 2] = [15, 26];
const CLASS_HUNTER: u8 = 3;
/// The tutorial whose acknowledgement arms Equippable Items (`0x523bbe`).
const EQUIP_ARMS: u32 = 0x37;
/// The equip-slot bits the Equippable Items test walks (`0x523bcf`).
const EQUIP_SLOT_BITS: u32 = (1 << 23) - 1;

/// The loot line's chat type (`0x523b4f`).
const CHAT_TYPE_LOOT: u8 = 27;

/// The pushing player as the push reads it: their level and class.
struct Pusher {
    level: u32,
    class: Option<u8>,
}

/// The push at its step: the handler's run and the record callback's alike.
pub(crate) fn item_push(world: &mut World, p: ItemPushResult) {
    let (Some(items), Some(net)) = (
        world.get_resource::<Items>(),
        world.get_resource::<NetCommands>(),
    ) else {
        return;
    };
    // The record first, whoever the push names: a miss asks it, leaves the callback and stops (`0x523924`, `0x52398d`).
    let Some(info) = items.ask_template(p.item_entry, 0, net).cloned() else {
        if let Some(mut waits) = world.get_non_send_resource_mut::<RecordWaits>() {
            waits.wait(p.item_entry, RecordWait::ItemPush, false);
        }
        if let Some(mut loot) = world.get_resource_mut::<LootState>() {
            loot.receives.push(p);
        }
        return;
    };
    let Some(pusher) = pusher(world, p.player_guid) else {
        return;
    };
    let me = world.get_resource::<SelfGuid>().and_then(|s| s.0);
    let ours = me == Some(p.player_guid);
    if ours {
        own_push(world, &p, &info, &pusher);
    } else if (!p.from_npc || p.created) && p.show_in_chat {
        // Another player's loot or create, named off their object (`0x523c6a`-`0x523d5b`).
        let key = match (p.count as i32 > 1, p.created) {
            (true, true) => "CREATED_ITEM_MULTIPLE",
            (true, false) => "LOOT_ITEM_MULTIPLE",
            (false, true) => "CREATED_ITEM",
            (false, false) => "LOOT_ITEM",
        };
        let name = pusher_name(world, p.player_guid);
        let link = push_link(world, &p, &info);
        let count = i64::from(p.count);
        loot_line(world, key, &[Arg::S(&name), Arg::S(&link), Arg::D(count)]);
    }
    // Ranged Weapons: our own ranged item off the hunter, or anyone's wand (`0x523d63`-`0x523d9e`).
    let ranged =
        ours && RANGED_TYPES.contains(&info.inventory_type) && pusher.class != Some(CLASS_HUNTER);
    if ranged || (info.class == CLASS_WEAPON && info.subclass == SUBCLASS_WAND) {
        crate::tutorial::trigger_at_step(world, id::RANGED_WEAPONS);
    }
}

/// Our own push past the record (`0x5239ec`-`0x523c5d`): `ITEM_PUSH`, the line, the tutorials in
/// the handler's order, then the item's sound.
fn own_push(world: &mut World, p: &ItemPushResult, info: &ItemInfo, pusher: &Pusher) {
    let icon =
        crate::ui_items::item_icon(world.get_resource::<ItemDisplays>(), info.display_info_id)
            .unwrap_or_default();
    let container = push_container(p.bag_slot, p.item_slot);
    debug!(
        "ui_loot: ITEM_PUSH container {container} icon {icon} (item {})",
        p.item_entry
    );
    // Fired whatever the line does (`0x523a60`).
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire(
            "ITEM_PUSH",
            vec![ScriptValue::Int(container), ScriptValue::Str(icon)],
        );
    }
    if p.show_in_chat {
        let key = match (p.count as i32 > 1, p.created, p.from_npc) {
            (true, true, _) => "LOOT_ITEM_CREATED_SELF_MULTIPLE",
            (true, false, true) => "LOOT_ITEM_PUSHED_SELF_MULTIPLE",
            (true, false, false) => "LOOT_ITEM_SELF_MULTIPLE",
            (false, true, _) => "LOOT_ITEM_CREATED_SELF",
            (false, false, true) => "LOOT_ITEM_PUSHED_SELF",
            (false, false, false) => "LOOT_ITEM_SELF",
        };
        let link = push_link(world, p, info);
        loot_line(world, key, &[Arg::S(&link), Arg::D(i64::from(p.count))]);
    }
    for tutorial in own_tutorials(world, p, info, pusher) {
        crate::tutorial::trigger_at_step(world, tutorial);
    }
    // The item's kit 1 (`0x523c5d` -> `0x4d0090`), after the tutorials' events.
    let display = info.display_info_id;
    crate::ui_script::live::in_turn(world, move |world| {
        crate::sound::play_item_sound(world, display, benilla_formats::ItemGesture::PutDown);
    });
}

/// The self leg's tutorials in their order (`0x523b5a`-`0x523c4f`), each test read off the record.
fn own_tutorials(world: &World, p: &ItemPushResult, info: &ItemInfo, pusher: &Pusher) -> Vec<u32> {
    let mut out = vec![id::BACKPACK];
    let food_or_drink = info.class == CLASS_CONSUMABLE && FOOD_AND_DRINK.contains(&info.subclass);
    // A use spell on anything but food and drink (`0x523b70`-`0x523b9b`).
    if info
        .spells
        .iter()
        .any(|s| s.spell_id != 0 && s.trigger == 0 && !food_or_drink)
    {
        out.push(id::USING_ITEMS);
    }
    let slots = crate::ui_tradeskill::INVENTORY_TYPE_SLOTS
        .get(info.inventory_type as usize)
        .copied()
        .unwrap_or(0);
    let armed = world
        .get_resource::<crate::tutorial::Tutorials>()
        .is_some_and(|t| t.is_flagged(EQUIP_ARMS));
    if slots & EQUIP_SLOT_BITS != 0 && armed {
        out.push(id::EQUIPPABLE_ITEMS);
    }
    if info.class == CLASS_CONTAINER {
        out.push(id::BAGS);
    }
    if push_container(p.bag_slot, p.item_slot) == crate::ui_items::KEYRING_CONTAINER {
        out.push(id::KEYRINGS);
    }
    if p.item_entry == HEARTHSTONE {
        out.push(id::HEARTHSTONES);
    }
    if pusher.level >= 2 && food_or_drink {
        out.extend([id::FOOD, id::DRINK]);
    }
    out
}

/// The pushing player's object (`0x5239be`, typemask `0x10`): `None` when it is not a player in view.
fn pusher(world: &World, guid: u64) -> Option<Pusher> {
    let entity = *world.get_resource::<GuidIndex>()?.0.get(&guid)?;
    let store = world
        .get::<ObjectStore>(entity)
        .filter(|s| s.0.type_mask() & TYPEMASK_PLAYER != 0)?;
    Some(Pusher {
        level: store.0.unit_level().unwrap_or(0),
        class: store.0.unit_class(),
    })
}

/// The pushing player's name as their object reads it (`0x72a000`): the name cache's, asked on a
/// miss, and `UNKNOWNOBJECT` meanwhile.
fn pusher_name(world: &World, guid: u64) -> String {
    let store = world
        .get_resource::<GuidIndex>()
        .and_then(|i| i.0.get(&guid))
        .and_then(|&e| world.get::<ObjectStore>(e));
    world
        .get_resource::<NameCache>()
        .zip(world.get_resource::<NetCommands>())
        .and_then(|(names, net)| names.resolve_unit(guid, store, net))
        .map(str::to_string)
        .unwrap_or_else(|| global_text(world, "UNKNOWNOBJECT"))
}

/// The push's item link (`0x61e360`): the record's rolled name and quality, the wire's random
/// property and suffix factor.
fn push_link(world: &World, p: &ItemPushResult, info: &ItemInfo) -> String {
    let rolls = RollCatalogs {
        props: world.get_resource::<crate::items::RandomProperties>(),
        enchants: world.get_resource::<crate::items::Enchants>(),
    };
    crate::ui_items::item_link_full(
        p.item_entry,
        &benilla_ui::script::ItemLinkFields::roll(
            p.random_property_id as i32,
            p.suffix_factor as i32,
        ),
        &rolls.name(&info.name, p.random_property_id),
        info.quality,
    )
}

/// The global string `key` as the engine looks it up (`0x819d40`); "" with no VM.
fn global_text(world: &World, key: &str) -> String {
    world
        .get_non_send_resource::<UiScript>()
        .map(|s| s.engine_text(key))
        .unwrap_or_default()
}

/// `key`'s format filled with `args`, added as a loot chat line in the push's turn, its
/// `CHAT_MSG_LOOT` fired before the steps after it (`0x523b52`, `0x523d5b` -> `0x509dd0`, type 27).
fn loot_line(world: &mut World, key: &str, args: &[Arg]) {
    let text = benilla_ui::strings::fill(&global_text(world, key), args);
    crate::ui_script::live::in_turn(world, move |world| {
        crate::ui_chat::add_chat_line_now(world, CHAT_TYPE_LOOT, text);
    });
}

/// The push's record callback (`0x512b60`): a found record runs the first push waiting on it again,
/// a denied one drops it.
pub(crate) fn push_callback(world: &mut World, entry: u32, landed: bool) {
    let Some(mut loot) = world.get_resource_mut::<LootState>() else {
        return;
    };
    let Some(at) = loot.receives.iter().position(|r| r.item_entry == entry) else {
        return;
    };
    let p = loot.receives.remove(at);
    if landed {
        item_push(world, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::{FIELD_OBJECT_TYPE, FIELD_UNIT_BYTES_0, FIELD_UNIT_LEVEL};
    use benilla_protocol::messages::{ItemSpellEntry, ObjectFields, ObjectType};
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    use crate::net::ClientCommand;

    const ME: u64 = 0x2A;
    const THEM: u64 = 0xFF;
    const LINEN: u32 = 2589;
    /// `TYPEMASK_OBJECT | UNIT | PLAYER`.
    const TYPE_PLAYER: u32 = 0x19;
    const WARRIOR: u8 = 1;
    /// The enUS strings the lines read; [`the_push_strings_are_the_installs`] pins them.
    const STRINGS: &str = r#"
        LOOT_ITEM = "%s receives loot: %s."
        LOOT_ITEM_MULTIPLE = "%s receives loot: %sx%d."
        CREATED_ITEM = "%s creates: %s."
        CREATED_ITEM_MULTIPLE = "%s creates: %sx%d."
        LOOT_ITEM_SELF = "You receive loot: %s."
        LOOT_ITEM_SELF_MULTIPLE = "You receive loot: %sx%d."
        LOOT_ITEM_PUSHED_SELF = "You receive item: %s."
        LOOT_ITEM_PUSHED_SELF_MULTIPLE = "You receive item: %sx%d."
        LOOT_ITEM_CREATED_SELF = "You create: %s."
        LOOT_ITEM_CREATED_SELF_MULTIPLE = "You create: %sx%d."
        UNKNOWNOBJECT = "Unknown"
    "#;
    const KEYS: [(&str, &str); 14] = [
        ("LOOT_ITEM", "%s receives loot: %s."),
        ("LOOT_ITEM_MULTIPLE", "%s receives loot: %sx%d."),
        ("CREATED_ITEM", "%s creates: %s."),
        ("CREATED_ITEM_MULTIPLE", "%s creates: %sx%d."),
        ("LOOT_ITEM_SELF", "You receive loot: %s."),
        ("LOOT_ITEM_SELF_MULTIPLE", "You receive loot: %sx%d."),
        ("LOOT_ITEM_PUSHED_SELF", "You receive item: %s."),
        ("LOOT_ITEM_PUSHED_SELF_MULTIPLE", "You receive item: %sx%d."),
        ("LOOT_ITEM_CREATED_SELF", "You create: %s."),
        ("LOOT_ITEM_CREATED_SELF_MULTIPLE", "You create: %sx%d."),
        ("UNKNOWNOBJECT", "Unknown"),
        ("GOLD_AMOUNT", "%d Gold"),
        ("SILVER_AMOUNT", "%d Silver"),
        ("COPPER_AMOUNT", "%d Copper"),
    ];

    fn push(player_guid: u64) -> ItemPushResult {
        ItemPushResult {
            player_guid,
            from_npc: false,
            created: false,
            show_in_chat: true,
            bag_slot: 0xFF,
            item_slot: 23,
            item_entry: LINEN,
            suffix_factor: 0,
            random_property_id: 0,
            count: 1,
            total_count: 1,
        }
    }

    /// Us and another player in view, a tutorial bank that has seen nothing, the strings loaded.
    fn bench() -> (World, Receiver<ClientCommand>) {
        let mut world = World::new();
        world.insert_resource(SelfGuid(Some(ME)));
        world.init_resource::<LootState>();
        world.init_resource::<Items>();
        world.insert_non_send_resource(RecordWaits::default());
        let (tx, rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::ui_chat::ChatLog>();
        world.init_resource::<UiEvents>();
        world.init_resource::<NameCache>();
        world.init_resource::<GuidIndex>();
        let mut tutorials = crate::tutorial::Tutorials::default();
        tutorials.apply_flags(&[0; 32]);
        world.insert_resource(tutorials);
        let script = UiScript::new().expect("VM");
        script.run(STRINGS).expect("strings");
        world.insert_non_send_resource(script);
        seat(&mut world, ME, 10, WARRIOR);
        seat(&mut world, THEM, 10, WARRIOR);
        (world, rx)
    }

    fn seat(world: &mut World, guid: u64, level: u32, class: u8) {
        let fields = ObjectFields::from_pairs(&[
            (FIELD_OBJECT_TYPE, TYPE_PLAYER),
            (FIELD_UNIT_LEVEL, level),
            (FIELD_UNIT_BYTES_0, u32::from(class) << 8),
        ])
        .into_created(ObjectType::Player);
        let e = world.spawn(ObjectStore(fields)).id();
        world.resource_mut::<GuidIndex>().0.insert(guid, e);
    }

    fn template(world: &mut World, entry: u32, name: &str, edit: impl FnOnce(&mut ItemInfo)) {
        let mut info = crate::items::test_template(name);
        edit(&mut info);
        world
            .resource_mut::<Items>()
            .insert_template(entry, Some(info));
    }

    fn use_spell() -> ItemSpellEntry {
        ItemSpellEntry {
            index: 0,
            spell_id: 433,
            trigger: 0,
            charges: 0,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        }
    }

    /// The events and steps the push queued, taken; the steps run.
    fn fired(world: &mut World) -> Vec<String> {
        let lines = world.resource::<UiEvents>().lines();
        UiEvents::run_steps(world);
        world.resource_mut::<UiEvents>().set_aside();
        lines
    }

    /// The chat lines added, the queued steps run first.
    fn lines(world: &mut World) -> Vec<String> {
        UiEvents::run_steps(world);
        world.resource::<crate::ui_chat::ChatLog>().pending_lines()
    }

    fn linen_link(quality_color: &str) -> String {
        format!("|c{quality_color}|Hitem:2589:0:0:0:0:0:0:0:0|h[Linen Cloth]|h|r")
    }

    /// Our own push (`0x5239ec`-`0x523c5d`): `ITEM_PUSH`, the line, the tutorials in the handler's
    /// order, then the sound's step.
    #[test]
    fn our_push_fires_item_push_then_its_line_tutorials_and_sound() {
        let (mut world, _rx) = bench();
        template(&mut world, LINEN, "Linen Cloth", |_| {});
        item_push(&mut world, push(ME));
        // The line's step, then Backpack (published 8) for every push of ours (`0x523b5c`), then
        // the sound's step.
        assert_eq!(
            fired(&mut world),
            ["ITEM_PUSH 0 ", "<world>", "TUTORIAL_TRIGGER 8", "<world>"]
        );
        assert_eq!(
            lines(&mut world),
            [format!("You receive loot: {}.", linen_link("ffffffff"))]
        );
    }

    /// The six self formats (`0x523a7a`-`0x523af4`): created wins over received, a count past 1
    /// takes the `_MULTIPLE` form; `showInChat` off shows no line but still fires `ITEM_PUSH`.
    #[test]
    fn our_push_line_reads_its_format_from_the_global_strings() {
        let (mut world, _rx) = bench();
        template(&mut world, LINEN, "Linen Cloth", |_| {});
        let link = linen_link("ffffffff");
        let cases = [
            (false, false, 1, format!("You receive loot: {link}.")),
            (false, false, 3, format!("You receive loot: {link}x3.")),
            (true, false, 1, format!("You receive item: {link}.")),
            (true, false, 2, format!("You receive item: {link}x2.")),
            (false, true, 1, format!("You create: {link}.")),
            (true, true, 4, format!("You create: {link}x4.")),
        ];
        for (from_npc, created, count, want) in cases {
            item_push(
                &mut world,
                ItemPushResult {
                    from_npc,
                    created,
                    count,
                    ..push(ME)
                },
            );
            assert_eq!(lines(&mut world).last(), Some(&want));
        }
        let shown = lines(&mut world).len();
        fired(&mut world);
        item_push(
            &mut world,
            ItemPushResult {
                show_in_chat: false,
                ..push(ME)
            },
        );
        assert_eq!(lines(&mut world).len(), shown, "no line");
        assert_eq!(fired(&mut world)[0], "ITEM_PUSH 0 ");
    }

    /// Each tutorial's test read off the record (`0x523b70`-`0x523c4f`, `0x523d63`-`0x523d9e`).
    #[test]
    fn our_push_triggers_the_tutorials_its_record_names() {
        let tutorials =
            |level: u32, class: u8, armed: bool, p: ItemPushResult, edit: fn(&mut ItemInfo)| {
                let (mut world, _rx) = bench();
                if armed {
                    // Bit 55 of the acknowledged bank arms Equippable Items (`0x523bbe`).
                    let mut bank = [0u8; 32];
                    bank[6] = 0x80;
                    world
                        .resource_mut::<crate::tutorial::Tutorials>()
                        .apply_flags(&bank);
                }
                seat(&mut world, ME, level, class);
                template(&mut world, p.item_entry, "Thing", edit);
                item_push(&mut world, p);
                fired(&mut world)
                    .into_iter()
                    .filter_map(|e| e.strip_prefix("TUTORIAL_TRIGGER ").map(str::to_string))
                    .collect::<Vec<_>>()
            };
        let mine = push(ME);
        // A use spell: Using Items, but not on food or drink.
        assert_eq!(
            tutorials(10, WARRIOR, false, mine, |t| t.spells = vec![use_spell()]),
            ["8", "9"]
        );
        let food = |t: &mut ItemInfo| {
            t.subclass = 5;
            t.spells = vec![use_spell()];
        };
        assert_eq!(tutorials(10, WARRIOR, false, mine, food), ["8", "11", "12"]);
        assert_eq!(
            tutorials(1, WARRIOR, false, mine, food),
            ["8"],
            "level 2 on"
        );
        // An equippable type only once tutorial 55 is acknowledged.
        let chest = |t: &mut ItemInfo| {
            t.class = 4;
            t.inventory_type = 5;
        };
        assert_eq!(tutorials(10, WARRIOR, false, mine, chest), ["8"]);
        assert_eq!(tutorials(10, WARRIOR, true, mine, chest), ["8", "24"]);
        // A bag; a keyring slot; the hearthstone.
        assert_eq!(
            tutorials(10, WARRIOR, false, mine, |t| t.class = 1),
            ["8", "10"]
        );
        let key = ItemPushResult {
            item_slot: 86,
            ..mine
        };
        assert_eq!(tutorials(10, WARRIOR, false, key, |_| {}), ["8", "50"]);
        let hearth = ItemPushResult {
            item_entry: HEARTHSTONE,
            ..mine
        };
        assert_eq!(tutorials(10, WARRIOR, false, hearth, |_| {}), ["8", "31"]);
        // A ranged weapon off the hunter, after the sound's step; anyone's wand.
        let bow = |t: &mut ItemInfo| {
            t.class = 2;
            t.inventory_type = 15;
        };
        assert_eq!(tutorials(10, WARRIOR, false, mine, bow), ["8", "44"]);
        assert_eq!(tutorials(10, CLASS_HUNTER, false, mine, bow), ["8"]);
        let wand = |t: &mut ItemInfo| {
            t.class = 2;
            t.subclass = 19;
        };
        assert_eq!(tutorials(10, WARRIOR, false, push(THEM), wand), ["44"]);
        assert_eq!(
            tutorials(10, WARRIOR, false, push(THEM), bow),
            Vec::<String>::new()
        );
    }

    /// Another player's push (`0x523c6a`-`0x523d5b`): a loot or a create shows their line, named
    /// off their object; a push received from an NPC shows nothing, nor does one not shown in chat.
    /// No `ITEM_PUSH`, no tutorial, no sound.
    #[test]
    fn another_players_push_shows_their_line() {
        let (mut world, rx) = bench();
        world
            .resource_mut::<NameCache>()
            .insert_player(THEM, "Thrall".into(), None);
        template(&mut world, LINEN, "Linen Cloth", |_| {});
        let link = linen_link("ffffffff");
        let cases = [
            (
                false,
                false,
                true,
                1,
                Some(format!("Thrall receives loot: {link}.")),
            ),
            (
                false,
                false,
                true,
                5,
                Some(format!("Thrall receives loot: {link}x5.")),
            ),
            (
                false,
                true,
                true,
                1,
                Some(format!("Thrall creates: {link}.")),
            ),
            (
                true,
                true,
                true,
                2,
                Some(format!("Thrall creates: {link}x2.")),
            ),
            (true, false, true, 1, None),
            (false, false, false, 1, None),
        ];
        for (from_npc, created, show_in_chat, count, want) in cases {
            let before = lines(&mut world).len();
            item_push(
                &mut world,
                ItemPushResult {
                    from_npc,
                    created,
                    show_in_chat,
                    count,
                    ..push(THEM)
                },
            );
            let after = lines(&mut world);
            assert_eq!(after.get(before).cloned(), want);
            assert_eq!(after.len(), before + usize::from(want.is_some()));
            assert!(fired(&mut world).is_empty());
        }
        assert!(
            rx.try_iter().next().is_none(),
            "the cached name asks nothing"
        );
    }

    /// A name not cached is asked and reads `UNKNOWNOBJECT` meanwhile (`0x72a000`); a player not in
    /// view shows nothing (`0x5239cb`).
    #[test]
    fn another_players_name_and_object_are_read_at_the_push() {
        let (mut world, rx) = bench();
        template(&mut world, LINEN, "Linen Cloth", |_| {});
        item_push(&mut world, push(THEM));
        assert_eq!(
            lines(&mut world),
            [format!(
                "Unknown receives loot: {}.",
                linen_link("ffffffff")
            )]
        );
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::NameQuery { guid } if guid == THEM)));
        item_push(&mut world, push(0x77));
        item_push(
            &mut world,
            ItemPushResult {
                player_guid: 0x77,
                ..push(ME)
            },
        );
        assert_eq!(lines(&mut world).len(), 1, "no object, no line");
        assert!(fired(&mut world).is_empty());
    }

    /// A record not cached is asked whoever the push names; the whole push runs from the record's
    /// callback (`0x52398d` -> `0x512b60`), and a denied record drops it. The template landing ahead
    /// of the push gives the same result at the push.
    #[test]
    fn a_push_waits_on_its_record_and_runs_from_its_callback() {
        for who in [ME, THEM] {
            let (mut world, rx) = bench();
            world
                .resource_mut::<NameCache>()
                .insert_player(THEM, "Thrall".into(), None);
            item_push(&mut world, push(who));
            assert!(rx
                .try_iter()
                .any(|c| matches!(c, ClientCommand::ItemQuery { entry, .. } if entry == LINEN)));
            assert!(fired(&mut world).is_empty());
            assert!(lines(&mut world).is_empty());
            assert_eq!(
                world.non_send_resource::<RecordWaits>().waiting(),
                [(LINEN, RecordWait::ItemPush)]
            );
            template(&mut world, LINEN, "Linen Cloth", |_| {});
            push_callback(&mut world, LINEN, true);
            assert_eq!(world.resource::<LootState>().pending_receive_count(), 0);
            let late = (fired(&mut world), lines(&mut world));

            let (mut early, _rx) = bench();
            early
                .resource_mut::<NameCache>()
                .insert_player(THEM, "Thrall".into(), None);
            template(&mut early, LINEN, "Linen Cloth", |_| {});
            item_push(&mut early, push(who));
            assert_eq!(late, (fired(&mut early), lines(&mut early)), "{who:#x}");
            assert_eq!(late.1.len(), 1);

            // A record the server cannot describe drops the push.
            item_push(
                &mut world,
                ItemPushResult {
                    item_entry: 77_777,
                    ..push(who)
                },
            );
            world.resource_mut::<Items>().insert_template(77_777, None);
            push_callback(&mut world, 77_777, false);
            assert_eq!(world.resource::<LootState>().pending_receive_count(), 0);
            assert!(fired(&mut world).is_empty());
            assert_eq!(lines(&mut world).len(), 1, "nothing more shown");
        }
    }

    /// The handler runs the push at its step, in turn with the events around it.
    #[test]
    fn the_handler_runs_the_push_at_its_step() {
        let (mut world, _rx) = bench();
        template(&mut world, LINEN, "Linen Cloth", |_| {});
        world
            .run_system_once_with(
                super::super::net::on_item_push_result,
                benilla_protocol::SessionEvent::ItemPushResult(push(ME)),
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(fired(&mut world), ["<world>"]);
    }

    /// The strings the push and the coin row read, as the install's `GlobalStrings.lua` has them.
    #[test]
    fn the_push_strings_are_the_installs() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        for (key, want) in KEYS {
            assert_eq!(s.engine_text(key), want, "{key}");
        }
    }
}
