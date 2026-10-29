//! `Spell.dbc` columns pinned on known spells, in the 12340 layout: SpellIconID 133, the enUS
//! SpellName 136, SpellVisual 131 and Speed 47.

use benilla_formats::{load_spell_catalog, open_chain};

#[test]
fn spell_catalog_resolves_known_spells() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open patch chain");
    let catalog = load_spell_catalog(&mut chain).expect("load spell catalog");
    assert_eq!(catalog.len(), 49_839, "12340 ships 49839 spells");

    let hs = catalog.get(78).expect("Heroic Strike");
    assert_eq!(hs.name, "Heroic Strike");
    assert_eq!(
        hs.icon.as_deref(),
        Some("Interface\\Icons\\Ability_Rogue_Ambush")
    );

    let bs = catalog.get(6673).expect("Battle Shout");
    assert_eq!(bs.name, "Battle Shout");
    assert_eq!(
        bs.icon.as_deref(),
        Some("Interface\\Icons\\Ability_Warrior_BattleShout")
    );

    let attack = catalog.get(6603).expect("Attack");
    assert_eq!(attack.name, "Auto Attack", "3.3.5a renamed it");
    assert!(attack.icon.is_some(), "auto-attack has an icon");

    // `PreventionType`, column 214: 1 silence, 2 pacify, 0 neither. Column 213, `DmgClass`, also
    // runs 0 to 2 on these rows, except Auto Shot's 3.
    assert_eq!(
        catalog.get(133).expect("Fireball").prevention_type,
        1,
        "Fireball is silence-preventable"
    );
    assert_eq!(
        catalog.get(78).expect("Heroic Strike").prevention_type,
        2,
        "Heroic Strike is pacify-preventable"
    );
    assert_eq!(
        catalog.get(6603).expect("Attack").prevention_type,
        0,
        "the auto-attack is neither"
    );
    assert_eq!(
        catalog.get(75).expect("Auto Shot").prevention_type,
        2,
        "Auto Shot is pacify-preventable — and its DmgClass 3 is what makes column 213 \
         distinguishable from 214 at all"
    );

    // The crowd-control exemption's columns: `SchoolMask` 225, `Mechanic` 3, `EffectMechanic`
    // 83-85.
    assert_eq!(catalog.get(133).expect("Fireball").school_mask, 0x4, "fire");
    assert_eq!(
        catalog.get(116).expect("Frostbolt").school_mask,
        0x10,
        "frost"
    );
    assert_eq!(catalog.get(585).expect("Smite").school_mask, 0x2, "holy");
    assert_eq!(
        catalog.get(78).expect("Heroic Strike").school_mask,
        0x1,
        "physical"
    );

    assert_eq!(
        catalog.get(118).expect("Polymorph").mechanic,
        17,
        "MECHANIC_POLYMORPH"
    );
    assert_eq!(
        catalog.get(5782).expect("Fear").mechanic,
        5,
        "MECHANIC_FEAR"
    );
    assert_eq!(
        catalog.get(133).expect("Fireball").mechanic,
        0,
        "a plain nuke carries no mechanic"
    );

    assert_eq!(
        catalog.get(116).expect("Frostbolt").effect_mechanic[0],
        11,
        "Frostbolt's slow is MECHANIC_SNARE, on effect 0"
    );
    assert_eq!(
        catalog.get(122).expect("Frost Nova").effect_mechanic[1],
        7,
        "Frost Nova's root is MECHANIC_ROOT, on effect 1 — not effect 0"
    );

    // Visual and speed as the file has them.
    let fireball = catalog.get(133).expect("Fireball");
    assert_eq!(fireball.visual, 67, "Fireball's SpellVisual id");
    assert_eq!(fireball.speed, 24.0, "Fireball's projectile speed");

    let frostbolt = catalog.get(116).expect("Frostbolt");
    assert_eq!(frostbolt.visual, 13);
    assert_eq!(frostbolt.speed, 28.0);

    let corruption = catalog.get(172).expect("Corruption");
    assert_eq!(corruption.visual, 8629);
    assert_eq!(corruption.speed, 0.0, "a DoT tick has no projectile");
}

#[test]
fn shapeshift_bonus_bars_match_the_verified_table() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open patch chain");
    let forms = benilla_formats::load_shapeshift_forms(&mut chain).expect("load shapeshift rows");
    // Every non-zero BonusActionBar, field 1: 3.3.5a adds Tree of Life, Shadow Dance, Shadowform
    // and Moonkin to the cat, bear, stance and stealth bars.
    let expect = [
        (1, 1),
        (2, 2),
        (5, 3),
        (8, 3),
        (13, 2),
        (17, 1),
        (18, 2),
        (19, 3),
        (28, 1),
        (30, 1),
        (31, 4),
    ];
    let nonzero = forms.values().filter(|f| f.bonus_bar != 0).count();
    assert_eq!(nonzero, expect.len(), "exactly the eleven non-zero rows");
    for (form, bar) in expect {
        assert_eq!(
            forms.get(&form).map(|f| f.bonus_bar),
            Some(bar),
            "form {form}"
        );
    }
    // flags1's stance bit: warrior stances and stealth are stances, cat is a shapeshift.
    for stance in [17u32, 18, 19, 30] {
        assert!(forms.get(&stance).unwrap().is_stance(), "form {stance}");
    }
    assert!(!forms.get(&1).unwrap().is_stance(), "cat is a shapeshift");
    // flags1 bit 0x2 blocks the stance bar's toggle-cancel: the warrior stances (0x7) carry it, so
    // clicking the active one does nothing.
    for stance in [17u32, 18, 19] {
        assert!(
            !forms.get(&stance).unwrap().cancelable(),
            "warrior stance {stance} blocks the toggle-cancel"
        );
    }
    for form in [1u32, 5, 8, 16, 28, 30, 31] {
        assert!(
            forms.get(&form).unwrap().cancelable(),
            "form {form} is cancelable"
        );
    }
}

/// The stance bar's Spell.dbc columns: the MOD_SHAPESHIFT form id, the signed StanceBarOrder that
/// the reference sorts by, and the ActiveIconID reads.
#[test]
fn stance_bar_spell_columns_match_the_probed_values() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open patch chain");
    let catalog = load_spell_catalog(&mut chain).expect("load spell catalog");
    // (spell, form id, order): Battle/Defensive/Berserker Stance, Bear, Cat, Stealth, Moonkin.
    for (spell, form, order) in [
        (2457u32, 17u32, 0i32),
        (71, 18, 1),
        (2458, 19, 2),
        (5487, 5, 0),
        (768, 1, 2),
        (1784, 30, 0),
        (24858, 31, 4),
    ] {
        let d = catalog.get(spell).expect("form spell");
        assert_eq!(d.shapeshift_form, Some(form), "spell {spell} form");
        assert_eq!(d.stance_bar_order, order, "spell {spell} order");
    }
    // Druid forms carry the dismiss-paw ActiveIconID, 122; 3.3.5a's warrior stances their own.
    assert_eq!(catalog.get(5487).unwrap().active_icon_id, 122, "bear");
    assert_eq!(catalog.get(2457).unwrap().active_icon_id, 279, "battle");
    assert_eq!(catalog.get(133).unwrap().active_icon_id, 0, "fireball");
    let fireball = catalog.get(133).unwrap();
    assert_eq!(fireball.shapeshift_form, None);
}

/// AttributesEx3 (column 7) bit 15, `SPELL_ATTR3_NORMAL_RANGED_ATTACK`, turns combat text
/// melee-white: it is set on the ranged basic shots only.
#[test]
fn melee_white_damage_marks_the_ranged_basic_shots() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open patch chain");
    let catalog = load_spell_catalog(&mut chain).expect("load spell catalog");
    // Auto Shot, Shoot (bows and guns), Throw, wand Shoot: the white-damage ranged shots.
    for id in [75u32, 3018, 2764, 5019] {
        assert!(
            catalog.get(id).unwrap().melee_white_damage(),
            "spell {id} should carry AttributesEx3 & 0x8000"
        );
    }
    // Heroic Strike, Fireball, Auto Attack. A melee ability's yellow rides the spell id, not this
    // bit.
    for id in [78u32, 133, 6603] {
        assert!(
            !catalog.get(id).unwrap().melee_white_damage(),
            "spell {id} should NOT carry AttributesEx3 & 0x8000"
        );
    }
}
