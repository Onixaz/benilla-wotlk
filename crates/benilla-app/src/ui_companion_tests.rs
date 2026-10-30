use std::collections::{BTreeSet, HashMap};

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

fn known() -> BTreeSet<u32> {
    [10, 11, 20, 21, 30, 31, 133].into_iter().collect()
}

fn names(entry: u32) -> Option<Option<String>> {
    let name = match entry {
        700 => "Zebra Cat",
        701 => "ant",
        800 => "Yak",
        801 => "Bear",
        _ => return Some(None),
    };
    Some(Some(name.into()))
}

fn no_cooldown(_: u32) -> Option<(i64, u32, bool)> {
    None
}

fn spell_ids(list: &[CompanionView]) -> Vec<u32> {
    list.iter().map(|c| c.spell_id).collect()
}

/// `0x5426a5` files a summon into a slot-5 row as a critter and a `MOUNTED` spell as a mount;
/// a guardian summon, a `0x80` spell and a plain spell join neither. With every record cached each
/// list sorts by name, case-insensitively (`0x53c7c0`).
#[test]
fn the_lists_take_companions_alone_and_sort_by_creature_name() {
    let (state, pending) = build_lists(
        &known(),
        &catalog(),
        &summons(),
        &names,
        SelfFacts::default(),
        &no_cooldown,
    );
    assert!(!pending);
    assert_eq!(spell_ids(&state.critters), [11, 10], "ant before Zebra Cat");
    assert_eq!(spell_ids(&state.mounts), [21, 20], "Bear before Yak");
    let ant = &state.critters[0];
    assert_eq!(
        (ant.creature_id, ant.name.as_deref(), ant.icon.as_deref()),
        (701, Some("ant"), Some("Icon_Ant"))
    );
}

/// The companion list update (`0x53cec0`) does not sort while a record is pending: the lists keep
/// spell order and the name reads nil.
#[test]
fn a_pending_record_holds_the_sort() {
    let pending_bear = |entry: u32| if entry == 801 { None } else { names(entry) };
    let (state, pending) = build_lists(
        &known(),
        &catalog(),
        &summons(),
        &pending_bear,
        SelfFacts::default(),
        &no_cooldown,
    );
    assert!(pending);
    assert_eq!(spell_ids(&state.critters), [10, 11]);
    assert_eq!(spell_ids(&state.mounts), [20, 21]);
    assert_eq!(state.mounts[1].name, None);
}

fn critter_guid(entry: u32) -> u64 {
    0xF130_0000_0000_0000 | (u64::from(entry) << 24) | 0x42
}

fn self_store(critter: Option<u64>, mount_display: u32, charmed_by: u64) -> ObjectStore {
    let critter = critter.unwrap_or(0);
    ObjectStore(ObjectFields::from_pairs(&[
        (10, charmed_by as u32),
        (11, (charmed_by >> 32) as u32),
        (18, critter as u32),
        (19, (critter >> 32) as u32),
        (133, mount_display),
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
    let facts = SelfFacts {
        critter: store.0.unit_critter(),
        auras: Some(&auras),
    };
    let (state, _) = build_lists(&known(), &catalog, &summons(), &names, facts, &no_cooldown);
    let active = |list: &[CompanionView]| -> Vec<u32> {
        list.iter()
            .filter(|c| c.active)
            .map(|c| c.spell_id)
            .collect()
    };
    assert_eq!(active(&state.critters), [10]);
    assert_eq!(active(&state.mounts), [20]);
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
