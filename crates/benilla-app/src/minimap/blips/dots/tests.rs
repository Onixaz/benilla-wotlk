use super::*;

use benilla_formats::{LockSlot, LOCK_KEY_ITEM, MAX_LOCK_SLOTS};
use benilla_protocol::field::{
    FIELD_UNIT_BYTES_1, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_NPC_FLAGS,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::TOWN_TYPES;

const FIELD_SUMMONEDBY: u16 = 12;
const FIELD_GAMEOBJECT_TYPE_ID: u16 = 21;
const ME: u64 = 0x7;
const NPC: u64 = 0xF130_0000_0000_0042;

fn store(pairs: &[(u16, u32)]) -> ObjectStore {
    ObjectStore(ObjectFields::from_pairs(pairs))
}

fn town(token: &str) -> &'static TownType {
    TOWN_TYPES.iter().find(|t| t.token == token).unwrap()
}

/// The world a test classifies in: no factions (every reaction neutral), nothing tracked.
struct Fixture {
    statuses: HashMap<u64, u32>,
    names: NameCache,
    templates: GameObjectTemplates,
    reputations: Reputations,
    group: crate::ui_party::GroupState,
    me: ObjectStore,
}

impl Fixture {
    fn new() -> Self {
        Self {
            statuses: HashMap::new(),
            names: NameCache::default(),
            templates: GameObjectTemplates::default(),
            reputations: Reputations::default(),
            group: Default::default(),
            me: store(&[(FIELD_UNIT_HEALTH, 100)]),
        }
    }

    fn world(&self, tracking: SelfTracking, town: Option<&'static TownType>) -> BlipWorld<'_> {
        BlipWorld {
            me: Some(&self.me),
            self_guid: Some(ME),
            tracking,
            town,
            statuses: &self.statuses,
            names: &self.names,
            templates: &self.templates,
            locks: None,
            forms: None,
            factions: None,
            reputations: &self.reputations,
            group: &self.group,
        }
    }
}

fn blip(c: Option<Classified>) -> Option<Blip> {
    c.map(|c| c.blip)
}

#[test]
fn each_blip_is_its_own_cell_of_the_eight_by_two_atlas() {
    assert_eq!(Blip::AttackablePlayer.cell(), [0.25, 0.375, 0.0, 0.5]);
    assert_eq!(Blip::FriendlyNpc.cell(), [0.875, 1.0, 0.0, 0.5]);
    assert_eq!(Blip::Tracked.cell(), [0.0, 0.125, 0.5, 1.0], "the gold dot");
    assert_eq!(Blip::QuestAvailable.cell(), [0.125, 0.25, 0.5, 1.0]);
    assert_eq!(Blip::Interact.cell(), [0.625, 0.75, 0.5, 1.0]);
}

#[test]
fn quest_status_picks_the_icon_and_trivial_tracking_admits_the_low_level_ones() {
    let npc = store(&[(FIELD_UNIT_HEALTH, 10)]);
    let at = |status, town| {
        let mut f = Fixture::new();
        f.statuses.insert(NPC, status);
        blip(f.world(SelfTracking::default(), town).classify(
            NPC,
            EntityKind::Unit,
            Some(&npc),
            false,
        ))
    };
    assert_eq!(at(10, None), Some(Blip::QuestReward));
    assert_eq!(at(8, None), Some(Blip::QuestAvailable));
    assert_eq!(at(7, None), Some(Blip::QuestRepeatable));
    for s in [0, 1, 2, 3, 4, 5, 6, 9] {
        assert_eq!(at(s, None), None, "status {s} draws nothing untracked");
    }
    let trivial = Some(town("MINIMAP_TRACKING_TRIVIAL_QUESTS"));
    assert_eq!(at(2, trivial), Some(Blip::QuestAvailable));
    assert_eq!(at(4, trivial), Some(Blip::QuestRepeatable));
}

#[test]
fn a_town_type_tracks_its_npcs_and_a_profession_trainer_carries_its_title() {
    let fx = Fixture::new();
    let repairer = store(&[
        (FIELD_UNIT_HEALTH, 10),
        (FIELD_UNIT_NPC_FLAGS, 0x1000 | 0x80),
    ]);
    let classify = |town| {
        fx.world(SelfTracking::default(), town).classify(
            NPC,
            EntityKind::Unit,
            Some(&repairer),
            false,
        )
    };
    assert_eq!(classify(None), None, "an untracked vendor has no blip");
    assert_eq!(
        classify(Some(town("MINIMAP_TRACKING_REPAIR"))),
        Some(Blip::Tracked.into())
    );
    assert_eq!(classify(Some(town("MINIMAP_TRACKING_BANKER"))), None);

    let trainer = store(&[(FIELD_UNIT_HEALTH, 10), (FIELD_UNIT_NPC_FLAGS, 0x10 | 0x40)]);
    let c = fx
        .world(
            SelfTracking::default(),
            Some(town("MINIMAP_TRACKING_TRAINER_PROFESSION")),
        )
        .classify(NPC, EntityKind::Unit, Some(&trainer), false);
    assert_eq!(
        c,
        Some(Classified {
            blip: Blip::Tracked,
            titled: true
        })
    );
}

#[test]
fn the_dead_draw_nothing_and_our_own_minion_takes_the_blank_cell() {
    let fx = Fixture::new();
    let w = fx.world(SelfTracking::default(), None);
    let dead = store(&[(FIELD_UNIT_HEALTH, 0)]);
    let mut f = Fixture::new();
    f.statuses.insert(NPC, 8);
    assert_eq!(
        blip(f.world(SelfTracking::default(), None).classify(
            NPC,
            EntityKind::Unit,
            Some(&dead),
            false
        )),
        None,
        "a dead quest giver"
    );
    let pet = store(&[
        (FIELD_UNIT_HEALTH, 0),
        (FIELD_SUMMONEDBY, ME as u32),
        (FIELD_SUMMONEDBY + 1, 0),
    ]);
    assert_eq!(
        blip(w.classify(NPC, EntityKind::Unit, Some(&pet), false)),
        Some(Blip::OwnMinion),
        "ahead of the health test"
    );
    assert_eq!(blip(w.classify(NPC, EntityKind::Unit, None, false)), None);
}

#[test]
fn an_unlearned_flight_master_shows_the_green_mark() {
    let fx = Fixture::new();
    let fm = store(&[(FIELD_UNIT_HEALTH, 10), (FIELD_UNIT_NPC_FLAGS, 0x2000)]);
    let w = fx.world(SelfTracking::default(), None);
    assert_eq!(
        blip(w.classify(NPC, EntityKind::Unit, Some(&fm), true)),
        Some(Blip::Interact)
    );
    assert_eq!(
        blip(w.classify(NPC, EntityKind::Unit, Some(&fm), false)),
        None
    );
}

#[test]
fn creature_tracking_colours_by_reaction_and_the_untrackable_never_show() {
    let fx = Fixture::new();
    let marked = store(&[
        (FIELD_UNIT_HEALTH, 10),
        (FIELD_UNIT_DYNAMIC_FLAGS, UNIT_DYNFLAG_TRACK_UNIT),
    ]);
    let w = fx.world(SelfTracking::default(), None);
    assert_eq!(
        blip(w.classify(NPC, EntityKind::Unit, Some(&marked), false)),
        Some(Blip::NeutralNpc),
        "with no faction data every reaction reads neutral"
    );
    let hidden = store(&[
        (FIELD_UNIT_HEALTH, 10),
        (FIELD_UNIT_DYNAMIC_FLAGS, UNIT_DYNFLAG_TRACK_UNIT),
        (FIELD_UNIT_BYTES_1, 0x4 << 24),
    ]);
    assert_eq!(
        blip(w.classify(NPC, EntityKind::Unit, Some(&hidden), false)),
        None
    );

    let mut ghost = Fixture::new();
    ghost.me = store(&[(FIELD_UNIT_HEALTH, 0)]);
    assert_eq!(
        blip(ghost.world(SelfTracking::default(), None).classify(
            NPC,
            EntityKind::Unit,
            Some(&marked),
            false
        )),
        None,
        "a dead tracker tracks nothing"
    );
}

#[test]
fn the_mailbox_type_tracks_mailboxes() {
    let fx = Fixture::new();
    let mailbox = store(&[(FIELD_GAMEOBJECT_TYPE_ID, 19)]);
    let chair = store(&[(FIELD_GAMEOBJECT_TYPE_ID, 7)]);
    let w = fx.world(
        SelfTracking::default(),
        Some(town("MINIMAP_TRACKING_MAILBOX")),
    );
    assert_eq!(
        blip(w.classify(1, EntityKind::GameObject, Some(&mailbox), false)),
        Some(Blip::Tracked)
    );
    assert_eq!(
        blip(w.classify(1, EntityKind::GameObject, Some(&chair), false)),
        None
    );
}

/// Bit `1 << (n − 1)`, `n` the lock's skill-slot `LockType` or the creature type.
#[test]
fn tracking_predicates_follow_the_mask_bit_law() {
    let mut vein = [LockSlot::default(); MAX_LOCK_SLOTS];
    vein[0] = LockSlot {
        key_type: LOCK_KEY_SKILL,
        index: 3,
        skill: 0,
        action: 0,
    };
    let mut keyed = [LockSlot::default(); MAX_LOCK_SLOTS];
    keyed[0] = LockSlot {
        key_type: LOCK_KEY_ITEM,
        index: 3,
        skill: 0,
        action: 0,
    };
    let locks = LockCatalog::from_rows([(38, vein), (40, keyed)]);
    assert!(tracked_resource(1 << 2, 38, &locks), "mining bit lights it");
    assert!(
        !tracked_resource(1 << 1, 38, &locks),
        "herbalism bit doesn't"
    );
    assert!(!tracked_resource(0, 38, &locks));
    assert!(!tracked_resource(1 << 2, 0, &locks), "lock 0 never tracks");
    assert!(
        !tracked_resource(1 << 2, 40, &locks),
        "a key-item slot's index is an item entry"
    );

    let beasts = SelfTracking {
        creatures: 1,
        ..default()
    };
    assert!(tracked_creature(beasts, || 1, 0, false));
    assert!(!tracked_creature(beasts, || 7, 0, false));
    assert!(!tracked_creature(beasts, || 0, 0, false), "no type cached");
    assert!(tracked_creature(
        SelfTracking::default(),
        || unreachable!(),
        UNIT_DYNFLAG_TRACK_UNIT,
        false
    ));
    let stealth = SelfTracking {
        stealthed: true,
        ..default()
    };
    assert!(tracked_creature(stealth, || 0, 0, true));
    assert!(!tracked_creature(stealth, || 0, 0, false), "not sneaking");
}

#[test]
fn a_party_dot_is_its_class_cell() {
    assert_eq!(party_cell(1), Some([0.0, 0.125, 0.0, 0.25]), "warrior");
    assert_eq!(party_cell(11), Some([0.25, 0.375, 0.25, 0.5]), "druid");
    assert_eq!(party_cell(0), None);
}

/// Find Minerals' aura sets the Mining bit, which lights a Copper Vein's lock and not an herb's.
#[test]
fn real_find_minerals_lights_a_copper_vein_not_an_herb() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
    let locks = benilla_formats::load_lock_catalog(&mut chain).expect("Lock.dbc");

    // Bit `1 << (MiscValue − 1)` of the aura-`kind` effect (44 creatures, 45 resources).
    let mask_of = |spell_id: u32, kind: u32| -> u32 {
        let s = spells.get(spell_id).expect("spell row");
        assert!(s.tracking_aura());
        (0..3)
            .find_map(|i| {
                (s.effect_apply_aura[i] == kind).then(|| 1u32 << (s.effect_misc_value[i] - 1))
            })
            .expect("tracking effect present")
    };
    let minerals = mask_of(2580, 45);
    let herbs = mask_of(2383, 45);
    // Copper Vein (`gameobject_template` 1731) has lock 38, Peacebloom (1618) lock 29.
    assert!(tracked_resource(minerals, 38, &locks));
    assert!(!tracked_resource(herbs, 38, &locks));
    assert!(tracked_resource(herbs, 29, &locks));
    assert!(!tracked_resource(minerals, 29, &locks));
}
