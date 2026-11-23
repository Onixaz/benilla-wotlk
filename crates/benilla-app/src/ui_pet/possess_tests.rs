//! The possess spell `[0xc234e0]`: the aura walk (`0x5d62a0`), the two verbs' reads of its record, and the handlers that look it up.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay, SummonPropertiesCatalog};
use benilla_protocol::messages::{ObjectType, AURA_UPDATE_FLAG_HARMFUL};

use crate::unit_auras::UnitAuras;

use super::possess::{find_possessed, find_possessed_spell, possess_view, PossessSpell};

const ME: u64 = 0x19;
const MIND_CONTROL: u32 = 605;
const EYE: u32 = 126;
const VEHICLE: u32 = 900;
const DUMMY: u32 = 901;
const ROW_POSSESSED: u32 = 65;
const ROW_GUARDIAN: u32 = 61;

fn spell(name: &str, effects: [u32; 3], auras: [u32; 3], misc_b: [i32; 3]) -> SpellDisplay {
    SpellDisplay {
        name: name.into(),
        icon: Some(format!("{name}-icon")),
        effects,
        effect_apply_aura: auras,
        effect_misc_value_b: misc_b,
        ..Default::default()
    }
}

/// Mind Control's shape (an aura 2 effect), Eye of Kilrogg's (a summon through a possessed row),
/// a vehicle's control aura, a plain dummy, and a summon through a guardian row.
fn catalog() -> SpellCatalog {
    let mut icons = HashMap::new();
    icons.insert(693, "cancel-icon".to_string());
    SpellCatalog::from_displays(HashMap::from([
        (MIND_CONTROL, spell("control", [6, 6, 0], [2, 4, 0], [0; 3])),
        (
            EYE,
            spell("eye", [28, 6, 0], [0, 4, 0], [ROW_POSSESSED as i32, 0, 0]),
        ),
        (VEHICLE, spell("vehicle", [6, 0, 0], [236, 0, 0], [0; 3])),
        (DUMMY, spell("dummy", [6, 0, 0], [4, 0, 0], [0; 3])),
        (
            902,
            spell("guardian", [28, 0, 0], [0; 3], [ROW_GUARDIAN as i32, 0, 0]),
        ),
    ]))
    .with_icons(icons)
}

fn summons() -> SummonPropertiesCatalog {
    SummonPropertiesCatalog::from_records([(ROW_POSSESSED, 3, 0, 0), (ROW_GUARDIAN, 1, 0, 0)])
}

fn find(auras: &[(u8, u32, u8)], on_vehicle: bool) -> Option<u32> {
    let table = UnitAuras::of(ME, auras);
    find_possessed_spell(Some(&table), on_vehicle, &catalog(), &summons())
}

/// The walk takes the first helpful aura by slot whose spell possesses, charms or summons a
/// possessed unit; a harmful aura, a spell with no record, a guardian summon and a vehicle's aura
/// off the charm never match.
#[test]
fn the_walk_takes_the_first_helpful_possessing_aura() {
    assert_eq!(
        find(&[(0, DUMMY, 0), (3, MIND_CONTROL, 0)], false),
        Some(MIND_CONTROL)
    );
    assert_eq!(
        find(&[(1, EYE, 0)], false),
        Some(EYE),
        "a possessed summon row"
    );
    assert_eq!(
        find(&[(1, EYE, 0), (0, MIND_CONTROL, 0)], false),
        Some(MIND_CONTROL),
        "slot 0 first"
    );
    assert_eq!(
        find(&[(0, MIND_CONTROL, AURA_UPDATE_FLAG_HARMFUL)], false),
        None,
        "the victim's side of the aura"
    );
    assert_eq!(
        find(&[(0, 902, 0), (1, 7777, 0)], false),
        None,
        "guardian; no record"
    );
    assert_eq!(
        find(&[(0, VEHICLE, 0)], false),
        None,
        "a vehicle off our charm"
    );
    assert_eq!(
        find(&[(0, VEHICLE, 0)], true),
        Some(VEHICLE),
        "riding our charm"
    );
    assert_eq!(
        find_possessed_spell(None, false, &catalog(), &summons()),
        None
    );
}

/// Slot 1's icon is the active icon when the spell names one; a vehicle's control aura hides the
/// bar; the sign bit of `Attributes` disables the cancel slot; an id with no record reads nameless.
#[test]
fn the_view_reads_the_spell_record_as_the_verbs_do() {
    let mut spells = catalog();
    let view = possess_view(MIND_CONTROL, &spells);
    assert_eq!(view.name.as_deref(), Some("control"));
    assert_eq!(view.icon.as_deref(), Some("control-icon"));
    assert_eq!(view.cancel_icon.as_deref(), Some("cancel-icon"));
    assert!(view.bar_visible && view.cancel_enabled);
    assert!(!possess_view(VEHICLE, &spells).bar_visible);

    let none = possess_view(7777, &spells);
    assert_eq!((none.name, none.icon), (None, None));
    assert!(none.bar_visible && none.cancel_enabled);

    let active = |texture: Option<&str>| {
        let mut s = spell("active", [6, 0, 0], [2, 0, 0], [0; 3]);
        s.active_icon_id = 7;
        s.active_icon = texture.map(str::to_string);
        s.attributes = 0x8000_0000;
        SpellCatalog::from_displays(HashMap::from([(1, s)]))
    };
    spells = active(Some("active-icon"));
    let view = possess_view(1, &spells);
    assert_eq!(view.icon.as_deref(), Some("active-icon"));
    assert!(!view.cancel_enabled);
    spells = active(None);
    assert_eq!(
        possess_view(1, &spells).icon,
        None,
        "an active id with no row"
    );
}

/// The lookup holds what the walk finds and keeps the held spell on no find; each change fires `UPDATE_BONUS_ACTIONBAR` once (`0x5d30e0`).
#[test]
fn the_lookup_holds_a_find_and_fires_on_a_change() {
    let me = benilla_protocol::ObjectFields::default().into_created(ObjectType::Player);
    let (spells, summons) = (catalog(), summons());
    let mut held = PossessSpell::default();
    let mut ui = crate::ui_script::live::UiEvents::default();
    let mut look = |auras: &[(u8, u32, u8)], held: &mut PossessSpell| {
        let table = UnitAuras::of(ME, auras);
        find_possessed(
            &me,
            Some(&table),
            None,
            Some((&spells, &summons)),
            held,
            &mut ui,
        );
    };
    look(&[(0, MIND_CONTROL, 0)], &mut held);
    assert_eq!(held.0, MIND_CONTROL);
    look(&[], &mut held);
    assert_eq!(held.0, MIND_CONTROL, "the walk never clears");
    look(&[(0, MIND_CONTROL, 0)], &mut held);
    look(&[(2, EYE, 0)], &mut held);
    assert_eq!(held.0, EYE);
    assert_eq!(ui.names(), vec!["UPDATE_BONUS_ACTIONBAR"; 2]);
}

/// The world entry's setter with no pet (`0x5d6e6d`) empties the cached guid, the list and the expiry, and drops the attacking global on that change (`0x5d6a42`); the bar's words and the pet's cooldowns stay until the next `SMSG_PET_SPELLS` zeroes them (`0x5d6ba0`).
#[test]
fn the_world_entry_clear_keeps_the_words() {
    use benilla_protocol::messages::{PetActionEntry, PetSpells};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    let mut world = World::new();
    let mut spells = PetSpells {
        pet_guid: 0x2A,
        state: 0x0101,
        ..Default::default()
    };
    spells.bar[0] = PetActionEntry::from(0x0700_0002);
    world.insert_resource(super::PetBar {
        spells,
        cached: 0x2A,
        attacking: true,
        expires: Some(std::time::Instant::now()),
        ..Default::default()
    });
    world.insert_resource(super::PetList(vec![0x2A]));
    world.init_resource::<PossessSpell>();
    world.init_resource::<crate::ui_script::live::UiEvents>();
    world
        .run_system_once(super::possess::world_entry_clear)
        .unwrap();
    let bar = world.resource::<super::PetBar>();
    assert_eq!((bar.cached, bar.spells.pet_guid), (0, 0));
    assert!(!bar.has_bar() && !bar.attacking && bar.expires.is_none());
    assert_eq!(bar.spells.bar[0].packed, 0x0700_0002, "the words stay");
    assert_eq!(bar.spells.state, 0x0101);
    assert!(world.resource::<super::PetList>().0.is_empty());
    assert_eq!(
        world.resource::<crate::ui_script::live::UiEvents>().names(),
        // The empty arm's talent tab reset after the combo zero (`0x5d6ab6`).
        ["<world>", "<world>", "<world>", "UNIT_PET", "<world>"]
    );
}
