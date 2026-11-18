use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay, SummonPropertiesCatalog};
use benilla_protocol::ObjectFields;

use super::*;

/// A slot-5 summon row and a guardian row.
const COMPANION_ROW: u32 = 41;
const GUARDIAN_ROW: u32 = 61;

fn summons() -> SummonPropertiesCatalog {
    SummonPropertiesCatalog::from_slots([(COMPANION_ROW, 5), (GUARDIAN_ROW, 1)])
}

fn critter(name: &str, creature: i32) -> SpellDisplay {
    SpellDisplay {
        name: name.into(),
        effects: [28, 0, 0],
        effect_misc_value: [creature, 0, 0],
        effect_misc_value_b: [COMPANION_ROW as i32, 0, 0],
        icon: Some(format!("Icon_{name}")),
        ..SpellDisplay::default()
    }
}

fn mount(name: &str, creature: i32) -> SpellDisplay {
    SpellDisplay {
        name: name.into(),
        effects: [6, 6, 0],
        effect_apply_aura: [SPELL_AURA_MOUNTED, 32, 0],
        effect_misc_value: [creature, 0, 0],
        icon: Some(format!("Icon_{name}")),
        ..SpellDisplay::default()
    }
}

/// Two critters, two mounts, a guardian summon, a hidden companion and a plain spell.
fn displays() -> HashMap<u32, SpellDisplay> {
    let guardian = SpellDisplay {
        effects: [28, 0, 0],
        effect_misc_value: [900, 0, 0],
        effect_misc_value_b: [GUARDIAN_ROW as i32, 0, 0],
        ..SpellDisplay::default()
    };
    let hidden = SpellDisplay {
        attributes: 0x80,
        ..critter("Hidden", 950)
    };
    HashMap::from([
        (10, critter("Zebra Cat", 700)),
        (11, critter("Ant", 701)),
        (20, mount("Yak", 800)),
        (21, mount("Bear", 801)),
        (30, guardian),
        (31, hidden),
        (133, SpellDisplay::default()),
    ])
}

fn catalog() -> SpellCatalog {
    SpellCatalog::from_displays(displays())
}

fn known() -> Vec<u32> {
    vec![10, 11, 20, 21, 30, 31, 133]
}

/// The cached records: every companion creature but `pending`.
fn names_with(pending: Option<u32>) -> NameCache {
    let mut names = NameCache::default();
    for (entry, name) in [
        (700, "Zebra Cat"),
        (701, "ant"),
        (800, "Yak"),
        (801, "Bear"),
    ] {
        if Some(entry) == pending {
            continue;
        }
        names.insert_creature(
            entry,
            Some(crate::names::CreatureRecord {
                name: name.into(),
                ..Default::default()
            }),
        );
    }
    names
}

fn commands() -> NetCommands {
    NetCommands(crossbeam_channel::unbounded().0)
}

/// The login's adds over `known()`, with `pending`'s record still asked; the events they fired.
fn init(pending: Option<u32>) -> (CompanionLists, Vec<&'static str>) {
    let (catalog, summons, names, commands) =
        (catalog(), summons(), names_with(pending), commands());
    let mut ui = UiEvents::default();
    let mut lists = CompanionLists::default();
    let mut cx = CompanionCx {
        catalog: &catalog,
        summons: &summons,
        names: &names,
        commands: &commands,
        ui: &mut ui,
    };
    add_initial(
        &mut lists,
        known().into_iter().map(|id| (id, None)),
        &mut cx,
    );
    (lists, ui.names())
}

/// `0x5426a5` files a summon into a slot-5 row as a critter and a `MOUNTED` spell as a mount; a guardian summon, a `0x80` spell and a plain spell join neither. With every record cached the login's update sorts each list by name, case-insensitively (`0x53c7c0`), and fires `COMPANION_UPDATE` once (`0x53cec0`).
#[test]
fn the_lists_take_companions_alone_and_sort_by_creature_name() {
    let (lists, fired) = init(None);
    assert_eq!(
        lists.list(CompanionType::Critter),
        [11, 10],
        "ant before Zebra Cat"
    );
    assert_eq!(
        lists.list(CompanionType::Mount),
        [21, 20],
        "Bear before Yak"
    );
    assert_eq!(fired, ["COMPANION_UPDATE"]);
}

/// The update does not sort while a record is pending: the lists keep add order and nothing fires; the answer's callback then sorts and fires `COMPANION_UPDATE` (`0x53ebc0`).
#[test]
fn a_pending_record_holds_the_sort_until_its_answer() {
    let (mut lists, fired) = init(Some(801));
    assert_eq!(lists.list(CompanionType::Critter), [10, 11]);
    assert_eq!(lists.list(CompanionType::Mount), [20, 21]);
    assert!(fired.is_empty(), "{fired:?}");
    let (catalog, summons, names, commands) =
        (catalog(), summons(), names_with(Some(801)), commands());
    let mut ui = UiEvents::default();
    let mut cx = CompanionCx {
        catalog: &catalog,
        summons: &summons,
        names: &names,
        commands: &commands,
        ui: &mut ui,
    };
    record_answered(&mut lists, 801, Some("Bear"), &mut cx);
    assert_eq!(
        lists.list(CompanionType::Mount),
        [21, 20],
        "the answer's name sorts"
    );
    assert_eq!(ui.names(), ["COMPANION_UPDATE"]);
}

/// A live learn with its record cached shows the learn line, then the update's `COMPANION_UPDATE`, then `COMPANION_LEARNED` (`0x5427be`); with the record asked it waits for the answer, which runs the same three (`0x53ebc0`). An unlearn updates and fires `COMPANION_UNLEARNED` (`0x53fad0`).
#[test]
fn a_live_learn_and_unlearn_fire_from_their_handlers() {
    let (catalog, summons, commands) = (catalog(), summons(), commands());
    let names = names_with(Some(700));
    let mut lists = CompanionLists::default();
    let mut ui = UiEvents::default();
    let mut cx = CompanionCx {
        catalog: &catalog,
        summons: &summons,
        names: &names,
        commands: &commands,
        ui: &mut ui,
    };
    assert_eq!(
        add_known(&mut lists, 11, true, None, &mut cx),
        Some(crate::tutorial::id::COMPANION_CRITTER)
    );
    assert_eq!(
        cx.ui.names(),
        ["<world>", "COMPANION_UPDATE", "COMPANION_LEARNED"]
    );
    let mut ui = UiEvents::default();
    cx.ui = &mut ui;
    let _ = add_known(&mut lists, 10, true, None, &mut cx);
    assert!(cx.ui.is_empty(), "the record is asked first");
    record_answered(&mut lists, 700, Some("Zebra Cat"), &mut cx);
    assert_eq!(
        cx.ui.names(),
        ["<world>", "COMPANION_UPDATE", "COMPANION_LEARNED"]
    );
    assert_eq!(lists.list(CompanionType::Critter), [11, 10]);
    let mut ui = UiEvents::default();
    cx.ui = &mut ui;
    remove_known(&mut lists, 11, &mut cx);
    remove_known(&mut lists, 133, &mut cx);
    assert_eq!(cx.ui.names(), ["COMPANION_UPDATE", "COMPANION_UNLEARNED"]);
    assert_eq!(lists.list(CompanionType::Critter), [10]);
}

fn critter_guid(entry: u32) -> u64 {
    0xF130_0000_0000_0000 | (u64::from(entry) << 24) | 0x42
}

fn self_store(critter: Option<u64>, mount_display: u32, charmed_by: u64) -> ObjectStore {
    let critter = critter.unwrap_or(0);
    ObjectStore(ObjectFields::from_pairs(&[
        (
            benilla_protocol::field::FIELD_UNIT_CHARMEDBY,
            charmed_by as u32,
        ),
        (
            benilla_protocol::field::FIELD_UNIT_CHARMEDBY + 1,
            (charmed_by >> 32) as u32,
        ),
        (benilla_protocol::field::FIELD_UNIT_CRITTER, critter as u32),
        (
            benilla_protocol::field::FIELD_UNIT_CRITTER + 1,
            (critter >> 32) as u32,
        ),
        (
            benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID,
            mount_display,
        ),
    ]))
}

/// The critter out is the one whose guid's entry bits name the spell's creature (`0x802dcc`); a
/// mount is up by its own aura or another `MOUNTED` aura riding the same creature.
#[test]
fn the_active_flag_reads_the_critter_guid_and_the_mount_auras() {
    let store = self_store(Some(critter_guid(700)), 0, 0);
    // Spell 22: another spell riding the Yak's creature, as a mount item's spell does.
    let mut displays = displays();
    displays.insert(22, mount("Yak (item)", 800));
    let catalog = SpellCatalog::from_displays(displays);
    let auras = UnitAuras::of(1, &[(0, 22, 0)]);
    let name_of = |_: u32| None;
    let active = |spell_id: u32| {
        companion_view(
            spell_id,
            &catalog,
            &summons(),
            &name_of,
            Some((&store.0, Some(&auras))),
        )
        .is_some_and(|c| c.active)
    };
    assert_eq!([10, 11, 20, 21].map(active), [true, false, true, false]);
    let view = companion_view(10, &catalog, &summons(), &name_of, None).unwrap();
    assert_eq!(
        (
            view.creature_id,
            view.name,
            view.icon.as_deref(),
            view.active
        ),
        (700, None, Some("Icon_Zebra Cat"), false)
    );
}

/// The aura cancel (`0x802f80`): the critter out goes home by `CMSG_DISMISS_CRITTER` with its guid,
/// a mount by `CMSG_CANCEL_AURA`, and a charmed player sends nothing.
#[test]
fn a_dismissal_sends_the_critter_home_or_cancels_the_mount() {
    let cat = catalog();
    let out = critter_guid(700);
    let store = self_store(Some(out), 0, 0);
    let sends = dismiss(10, &cat, &summons(), &store, None);
    assert!(
        matches!(sends.as_slice(), [ClientCommand::DismissCritter { guid }] if *guid == out),
        "{sends:?}"
    );
    assert!(
        dismiss(11, &cat, &summons(), &store, None).is_empty(),
        "another critter is not out"
    );

    let mounted = self_store(None, 1234, 0);
    let auras = UnitAuras::of(1, &[(0, 20, 0)]);
    let sends = dismiss(20, &cat, &summons(), &mounted, Some(&auras));
    assert!(
        matches!(
            sends.as_slice(),
            [ClientCommand::CancelAura { spell_id: 20 }]
        ),
        "{sends:?}"
    );

    let charmed = self_store(Some(out), 0, 0xF130_0000_0000_0099);
    assert!(dismiss(10, &cat, &summons(), &charmed, None).is_empty());
}

/// A mount ridden through another spell of the same creature cancels that spell's aura
/// (`0x8030bc`).
#[test]
fn a_mount_ridden_through_another_spell_cancels_that_one() {
    let mut displays: HashMap<u32, SpellDisplay> = HashMap::new();
    displays.insert(20, mount("Yak", 800));
    displays.insert(22, mount("Yak (item)", 800));
    let cat = SpellCatalog::from_displays(displays);
    let mounted = self_store(None, 1234, 0);
    let auras = UnitAuras::of(1, &[(0, 22, 0)]);
    let sends = dismiss(20, &cat, &summons(), &mounted, Some(&auras));
    assert!(
        matches!(
            sends.as_slice(),
            [ClientCommand::CancelAura { spell_id: 22 }]
        ),
        "{sends:?}"
    );
}
