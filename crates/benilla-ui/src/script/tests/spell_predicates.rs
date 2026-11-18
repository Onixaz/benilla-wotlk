//! The spellbook's per-slot predicates and lookups no stock file calls (#399):
//! `FindSpellBookSlotByID`, `GetSpellCount`, `IsAttackSpell`, `IsAutoRepeatSpell`,
//! `IsConsumableSpell`, `IsHelpfulSpell`, `IsHarmfulSpell`, and `IsCurrentItem`.

use std::rc::Rc;

use crate::script::{
    ContainerSlot, ContainerState, PetBookState, SpellBookState, SpellInfo, SpellInfoSource,
    SpellSlotView, SpellStatEffect, SpellStatEffects, UiScript,
};

const ATTACK: u32 = 6603;
const SHOOT: u32 = 5019;
const FROST_NOVA: u32 = 122;
const RENEW: u32 = 139;
const PORTAL: u32 = 10059;
const ARCANE_POWDER: u32 = 17020;
const RUNE: u32 = 17031;
const HEARTHSTONE: u32 = 6948;
const BITE: u32 = 17253;

/// The rows the predicates read, Shoot as the auto-repeat and the Hearthstone as the item cast.
struct Rows;

impl SpellInfoSource for Rows {
    fn spell_info(&self, _: u32, _: bool) -> Option<SpellInfo> {
        None
    }

    fn spell_stat_effects(&self, spell_id: u32) -> Option<SpellStatEffects> {
        let mut row = SpellStatEffects::default();
        match spell_id {
            ATTACK => {
                row.effects[0] = SpellStatEffect {
                    effect: 78,
                    ..Default::default()
                };
            }
            FROST_NOVA | BITE => row.harmful = true,
            RENEW => row.helpful = true,
            // Two reagents: the uses are the scarcer one's.
            PORTAL => {
                row.has_reagents = true;
                row.reagents[0] = (ARCANE_POWDER, 2);
                row.reagents[1] = (RUNE, 1);
            }
            SHOOT => {}
            _ => return None,
        }
        Some(row)
    }

    fn is_auto_repeat_spell(&self, spell_id: u32) -> bool {
        spell_id == SHOOT
    }

    fn is_current_item(&self, item_id: u32) -> bool {
        item_id == HEARTHSTONE
    }
}

fn slot(spell_id: u32, name: &str) -> SpellSlotView {
    SpellSlotView {
        spell_id,
        name: name.into(),
        ..Default::default()
    }
}

/// Attack, Shoot, Frost Nova, Renew and Portal in the player's book, a second Renew last; Bite in
/// the pet's.
fn script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_spell_info_source(Rc::new(Rows));
    s.set_spellbook(SpellBookState {
        slots: vec![
            slot(ATTACK, "Attack"),
            slot(SHOOT, "Shoot"),
            slot(FROST_NOVA, "Frost Nova"),
            slot(RENEW, "Renew"),
            slot(PORTAL, "Portal: Stormwind"),
            slot(RENEW, "Renew"),
        ],
        ..Default::default()
    });
    s.set_pet_book(PetBookState {
        token: Some("PET".into()),
        slots: vec![slot(BITE, "Bite")],
    });
    s
}

fn flag(s: &UiScript, chunk: &str) -> bool {
    s.eval::<Option<i64>>(&format!("return {chunk}"))
        .unwrap()
        .is_some()
}

fn count(s: &UiScript, chunk: &str) -> i64 {
    s.eval::<i64>(&format!("return select('#', {chunk})"))
        .unwrap()
}

#[test]
fn each_predicate_reads_its_slots_row() {
    let s = script();
    let cases: [(&str, usize); 5] = [
        ("IsAttackSpell", 1),
        ("IsAutoRepeatSpell", 2),
        ("IsHarmfulSpell", 3),
        ("IsHelpfulSpell", 4),
        ("IsConsumableSpell", 5),
    ];
    for (verb, hit) in cases {
        for slot in 1..=5 {
            assert_eq!(
                flag(&s, &format!("{verb}({slot}, 'spell')")),
                slot == hit,
                "{verb}({slot})"
            );
        }
        // By name, as the slot parser reads one.
        let name = [
            "Attack",
            "Shoot",
            "Frost Nova",
            "Renew",
            "Portal: Stormwind",
        ][hit - 1];
        assert!(flag(&s, &format!("{verb}('{name}')")), "{verb}('{name}')");
    }
    // The pet's book is its own array.
    assert!(flag(&s, "IsHarmfulSpell(1, 'pet')"));
    assert!(!flag(&s, "IsHelpfulSpell(1, 'pet')"));
}

/// A slot past the book's spells reads spell 0, one nil; a name the book lacks answers nothing;
/// a slot outside the array raises.
#[test]
fn the_slot_parser_decides_nil_nothing_or_an_error() {
    let s = script();
    for verb in [
        "IsAttackSpell",
        "IsAutoRepeatSpell",
        "IsConsumableSpell",
        "IsHelpfulSpell",
        "IsHarmfulSpell",
    ] {
        assert_eq!(count(&s, &format!("{verb}(40, 'spell')")), 1, "{verb} past");
        assert!(!flag(&s, &format!("{verb}(40, 'spell')")), "{verb} past");
        assert_eq!(count(&s, &format!("{verb}('Fireball')")), 0, "{verb} miss");
        assert!(s
            .run(&format!("{verb}(2000, 'spell')"))
            .unwrap_err()
            .to_string()
            .contains(&format!("{verb}(): Invalid spell slot")));
    }
    assert_eq!(count(&s, "GetSpellCount('Fireball')"), 0);
    assert_eq!(
        s.eval::<f64>("return GetSpellCount(40, 'spell')").unwrap(),
        0.0
    );
}

/// The pet's slot is never the auto-repeat, even holding the same spell.
#[test]
fn auto_repeat_is_the_players_book_alone() {
    let mut s = script();
    s.set_pet_book(PetBookState {
        token: Some("PET".into()),
        slots: vec![slot(SHOOT, "Shoot")],
    });
    assert!(flag(&s, "IsAutoRepeatSpell(2, 'spell')"));
    assert!(!flag(&s, "IsAutoRepeatSpell(1, 'pet')"));
}

/// The scarcer reagent sets the count, by whole casts; no reagent is 0.
#[test]
fn spell_count_is_the_casts_the_reagents_pay_for() {
    let mut s = script();
    let stack = |item_id: u32, count: u32| ContainerSlot {
        item_id,
        count,
        ..Default::default()
    };
    assert_eq!(
        s.eval::<f64>("return GetSpellCount(5, 'spell')").unwrap(),
        0.0
    );
    s.set_container(
        0,
        Some(ContainerState {
            num_slots: 16,
            slots: [
                (1, stack(ARCANE_POWDER, 7)),
                (2, stack(RUNE, 5)),
                (3, stack(ARCANE_POWDER, 2)),
            ]
            .into(),
            ..Default::default()
        }),
    );
    // Powder 9 / 2 = 4, rune 5 / 1 = 5.
    assert_eq!(
        s.eval::<f64>("return GetSpellCount(5, 'spell')").unwrap(),
        4.0
    );
    assert_eq!(
        s.eval::<f64>("return GetSpellCount('Portal: Stormwind')")
            .unwrap(),
        4.0
    );
    assert_eq!(
        s.eval::<f64>("return GetSpellCount(4, 'spell')").unwrap(),
        0.0
    );
}

/// The 0-based index of the last slot holding the id, the pet's array on a true flag.
#[test]
fn find_spell_book_slot_by_id_is_the_last_match_zero_based() {
    let s = script();
    let find = |chunk: &str| {
        s.eval::<Option<i64>>(&format!("return FindSpellBookSlotByID({chunk})"))
            .unwrap()
    };
    assert_eq!(find(&ATTACK.to_string()), Some(0));
    assert_eq!(find(&RENEW.to_string()), Some(5));
    assert_eq!(find(&format!("'{FROST_NOVA}'")), Some(2));
    assert_eq!(find(&BITE.to_string()), None);
    assert_eq!(find(&format!("{BITE}, true")), Some(0));
    assert_eq!(find(&format!("{ATTACK}, 1")), None);
    assert_eq!(find("999999"), None);
    for bad in ["", "0", "-5", "'Renew'", "{}"] {
        assert!(
            s.run(&format!("FindSpellBookSlotByID({bad})"))
                .unwrap_err()
                .to_string()
                .contains("Usage: FindSpellBookSlotByID(spellID[, isPet])"),
            "FindSpellBookSlotByID({bad})"
        );
    }
}

#[test]
fn the_current_item_is_the_pending_casts_item_by_id_or_link() {
    let s = script();
    assert!(flag(&s, &format!("IsCurrentItem({HEARTHSTONE})")));
    assert!(flag(
        &s,
        &format!(
            "IsCurrentItem('|cffffffff|Hitem:{HEARTHSTONE}:0:0:0:0:0:0:0:80|h[Hearthstone]|h|r')"
        )
    ));
    assert!(!flag(&s, "IsCurrentItem(4540)"));
    assert!(!flag(&s, "IsCurrentItem(0)"));
    assert!(!flag(&s, "IsCurrentItem({})"));
    assert!(!flag(&s, "IsCurrentItem()"));
}
