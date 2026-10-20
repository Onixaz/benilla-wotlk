//! The possess spell `[0xc234e0]`: `FindPossessedSpell`'s aura walk (`0x5d62a0`), the two verbs'
//! reads of its record, and the feed's triggers, whole client headless.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SpellDisplay, SummonPropertiesCatalog};
use benilla_protocol::messages::{ObjectType, AURA_UPDATE_FLAG_HARMFUL};

use crate::net::{FieldChanged, Guid, GuidIndex, ObjectStore, SelfGuid};
use crate::unit_auras::UnitAuras;

use super::possess::{feed_possess_spell, find_possessed_spell, possess_view};

const ME: u64 = 0x19;
const VICTIM: u64 = 0x2A;
const MIND_CONTROL: u32 = 605;
const EYE: u32 = 126;
const VEHICLE: u32 = 900;
const DUMMY: u32 = 901;
const ROW_POSSESSED: u32 = 65;
const ROW_GUARDIAN: u32 = 61;
const CHARM: u16 = 6;
const FARSIGHT: u16 = 712;

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

struct Rig {
    app: App,
    me: Entity,
}

impl Rig {
    fn new() -> Self {
        let mut app = App::new();
        app.add_message::<FieldChanged>()
            .init_resource::<GuidIndex>()
            .insert_resource(SelfGuid(Some(ME)))
            .insert_resource(crate::ui_action::Spells {
                catalog: catalog(),
                ..crate::ui_action::Spells::empty_for_tests()
            })
            .insert_resource(crate::spell::SummonSlots(summons()))
            .add_systems(Update, feed_possess_spell);
        let script = benilla_ui::script::UiScript::new().unwrap();
        script
            .run(
                "Bonus = 0 local f = CreateFrame('Frame') \
                 f:RegisterEvent('UPDATE_BONUS_ACTIONBAR') \
                 f:SetScript('OnEvent', function() Bonus = Bonus + 1 end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let me = app.world_mut().spawn(Guid(ME)).id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
        let victim = app
            .world_mut()
            .spawn((
                Guid(VICTIM),
                ObjectStore(benilla_protocol::ObjectFields::default()),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(VICTIM, victim);
        Self { app, me }
    }

    /// Our object streams in, with `auras`.
    fn enter(&mut self, auras: &[(u8, u32, u8)]) {
        self.app.world_mut().entity_mut(self.me).insert((
            ObjectStore(benilla_protocol::ObjectFields::default().into_created(ObjectType::Player)),
            UnitAuras::of(ME, auras),
        ));
    }

    fn auras(&mut self, auras: &[(u8, u32, u8)]) {
        self.app
            .world_mut()
            .entity_mut(self.me)
            .insert(UnitAuras::of(ME, auras));
    }

    /// One of our guid fields moves to `guid`, as a values update would.
    fn field(&mut self, index: u16, guid: u64) {
        let me = self.me;
        let delta = benilla_protocol::ObjectFields::from_pairs(&[
            (index, guid as u32),
            (index + 1, (guid >> 32) as u32),
        ]);
        crate::net::apply_fields_for_test(self.app.world_mut(), me, delta);
    }

    /// One frame: `(IsPossessBarVisible, the first slot's name, events so far)`.
    fn frame(&mut self) -> (bool, Option<String>, i64) {
        self.app.update();
        let s = self
            .app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>();
        s.eval("return IsPossessBarVisible(), select(2, GetPossessInfo(1)), Bonus")
            .unwrap()
    }
}

/// World entry walks the auras; a charm or far-sight move walks them again; the walk never
/// clears, a far sight with no subject does, and leaving the world does. Each change fires
/// `UPDATE_BONUS_ACTIONBAR` once.
#[test]
fn the_feed_follows_entry_the_charm_far_sight_and_leaving() {
    let mut rig = Rig::new();
    let control = || Some("control".to_string());
    assert_eq!(rig.frame(), (false, None, 0), "no player, no spell");

    rig.enter(&[(0, MIND_CONTROL, 0)]);
    assert_eq!(rig.frame(), (true, control(), 1), "entry finds it");
    assert_eq!(
        rig.frame(),
        (true, control(), 1),
        "a quiet frame fires nothing"
    );

    // An aura edge alone is no trigger, and a walk that finds nothing keeps the spell.
    rig.auras(&[]);
    assert_eq!(rig.frame(), (true, control(), 1));
    rig.field(CHARM, VICTIM);
    assert_eq!(rig.frame(), (true, control(), 1), "the walk never clears");

    // A far sight on a streamed subject walks; one cleared drops the spell.
    rig.field(FARSIGHT, VICTIM);
    assert_eq!(rig.frame(), (true, control(), 1));
    rig.field(FARSIGHT, 0);
    assert_eq!(rig.frame(), (false, None, 2), "far sight gone");

    // Eye of Kilrogg's summon row, found off the charm move; then a far sight on a subject not
    // streamed clears it.
    rig.auras(&[(2, EYE, 0)]);
    rig.field(CHARM, 0);
    assert_eq!(rig.frame(), (true, Some("eye".into()), 3));
    rig.field(FARSIGHT, 0x7777);
    assert_eq!(rig.frame(), (false, None, 4), "an unstreamed subject");

    // Leaving the world clears it.
    rig.auras(&[(0, MIND_CONTROL, 0)]);
    rig.field(CHARM, VICTIM);
    assert_eq!(rig.frame(), (true, control(), 5));
    rig.app
        .world_mut()
        .entity_mut(rig.me)
        .remove::<ObjectStore>();
    assert_eq!(rig.frame(), (false, None, 6), "left the world");
}
