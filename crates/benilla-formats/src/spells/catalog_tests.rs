//! The spell catalog's column pins, checked end to end against the real 12340 `Spell.dbc`.

use super::*;

/// A learn wrapper is never in `SkillLineAbility`, so the tree groups it by the spell it teaches:
/// Apprentice Blacksmith 2020 teaches Blacksmithing 2018, Lockpicking 1809 Pick Lock 1804, and
/// the Ice Armor wrapper 1214 Ice Armor 7302.
#[test]
fn real_learn_spell_hop_resolves_the_taught_ability() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let spells = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let skills = crate::skill_lines::load_skill_line_catalog(&mut chain).expect("load skill lines");

    for (wrapper, ability, line) in [
        (2020u32, 2018u32, 164u32),
        (1809, 1804, 633),
        (1214, 7302, 6),
    ] {
        assert_eq!(
            spells.learned_spell(wrapper),
            Some(ability),
            "learn wrapper {wrapper} teaches ability {ability}"
        );
        assert_eq!(
            skills.spell_to_line(wrapper, 0, 0),
            None,
            "the wrapper {wrapper} is not itself in SkillLineAbility"
        );
        assert_eq!(
            skills.spell_to_line(ability, 0, 0),
            Some(line),
            "the taught ability {ability} groups under skill line {line}"
        );
        assert!(
            spells.get(ability).is_some_and(|d| !d.name.is_empty()),
            "the taught ability carries the display name"
        );
    }
}

#[test]
fn real_spell_catalog_reads_ranged_attributes() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Auto Shot: ranged (0x2) and auto-repeat (0x20); no visual, the wire ammo is the missile.
    let auto_shot = cat.get(75).expect("Auto Shot");
    assert_eq!(auto_shot.attributes, 0x50012);
    assert_eq!(auto_shot.attributes_ex2, 0x20);
    assert!(auto_shot.ranged_attack());
    assert_eq!(auto_shot.visual, 0, "the shot spells have no SpellVisual");
    assert_eq!(auto_shot.speed, 40.0);

    // Wand Shoot: the 0x18&0x2 side of the client gate, plus auto-repeat.
    let shoot = cat.get(5019).expect("Shoot (wand)");
    assert_eq!(shoot.attributes, 0x12);
    assert_eq!(shoot.attributes_ex2, 0x20);
    assert!(shoot.ranged_attack());

    // Throw: ranged but not auto-repeat; its `AttributesEx2` is the post-cast attack bit alone.
    let throw = cat.get(2764).expect("Throw");
    assert_eq!(throw.attributes, 0x410012);
    assert_eq!(throw.attributes_ex2, 0x10_0000);
    assert!(throw.ranged_attack() && !throw.auto_repeat());

    // Fireball: neither bit.
    let fireball = cat.get(133).expect("Fireball");
    assert_eq!(fireball.attributes, 0x10000);
    assert_eq!(fireball.attributes_ex2, 0);
    assert!(!fireball.ranged_attack());

    // Effect[0] (column 71): 6603 "Auto Attack" carries SPELL_EFFECT_ATTACK (78).
    let attack = cat.get(6603).expect("Auto Attack");
    assert_eq!(
        attack.effects[0], 78,
        "6603 Effect[0] == SPELL_EFFECT_ATTACK"
    );
    assert!(attack.is_melee_auto_attack());
    assert!(!fireball.is_melee_auto_attack());
    assert!(
        !auto_shot.is_melee_auto_attack(),
        "Auto Shot is ranged (Effect 58), not melee"
    );
}

/// `CanShowAura` (`0x53d580`), the spell's own legs: `Attributes & 0x80` and `AttributesEx &
/// 0x10000000` hide an aura whoever cast it; a passive shows only with an area-aura effect; the
/// `AttributesEx5` pair hides a self-cast or another's aura.
#[test]
fn real_spell_catalog_hides_stances_from_the_aura_bar() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let shows = |id: u32, self_cast: bool| {
        cat.get(id)
            .unwrap_or_else(|| panic!("spell {id}"))
            .shows_aura(self_cast)
    };

    // The warrior stances carry NO_AURA_ICON, Battle Stance with CAST_WHEN_LEARNED.
    for (id, ex) in [
        (2457u32, 0x9000_0400u32),
        (71, 0x1000_0400),
        (2458, 0x1000_0400),
    ] {
        assert_eq!(cat.get(id).unwrap().attributes_ex, ex, "stance {id}");
        assert!(!shows(id, true), "stance {id}");
    }
    // Defensive State (DND) 5301: DO_NOT_DISPLAY. Its 1.12 sibling 5302 has no 12340 row.
    assert_eq!(cat.get(5301).unwrap().attributes, 0x1d0);
    assert!(!shows(5301, true));
    assert!(cat.get(5302).is_none());

    assert!(shows(6673, true), "Battle Shout");
    assert!(shows(1243, false), "Power Word: Fortitude");
    // The dword sign bit (`CANT_CANCEL`) filters nothing: Echoes of Lordaeron still shows.
    assert_eq!(cat.get(1386).unwrap().attributes, 0x8800_0100);
    assert!(shows(1386, false));

    // A passive without an area aura is hidden (Diplomacy 20599), one with it listed (Heroic
    // Presence 6562, effect 65).
    assert!(cat.get(20599).unwrap().passive && !shows(20599, true));
    assert!(cat.get(6562).unwrap().passive && shows(6562, true));

    // `AttributesEx5` bit 27 hides Mind Flay's slow only when self-cast; bit 28 hides Fixate's
    // only when another cast it, and Mirror Image carries both.
    assert!(!shows(15407, true) && shows(15407, false));
    assert!(shows(12021, true) && !shows(12021, false));
    assert!(!shows(55342, true) && !shows(55342, false));
    // `AttributesEx6 & 0x100000`, the caster-only leg, ships on two rows.
    let caster_only: Vec<u32> = {
        let mut v: Vec<u32> = cat
            .iter()
            .filter(|(_, d)| d.only_visible_to_caster())
            .map(|(id, _)| id)
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(caster_only, [58371, 62218], "Recently Slain, Frenzy");
}

#[test]
fn real_spell_catalog_reads_rank_and_passive() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Fireball's first three ranks; even rank 1 carries the literal "Rank 1".
    assert_eq!(cat.get(133).unwrap().rank.as_deref(), Some("Rank 1"));
    assert_eq!(cat.get(143).unwrap().rank.as_deref(), Some("Rank 2"));
    assert_eq!(cat.get(145).unwrap().rank.as_deref(), Some("Rank 3"));
    assert_eq!(cat.get(168).unwrap().rank.as_deref(), Some("Rank 1")); // Frost Armor
    assert_eq!(cat.get(172).unwrap().rank.as_deref(), Some("Rank 1")); // Corruption
    assert_eq!(cat.get(2136).unwrap().rank.as_deref(), Some("Rank 1")); // Fire Blast

    // None of the above is passive; a weapon skill (One-Handed Swords, 201) is.
    assert!(!cat.get(133).unwrap().passive);
    assert!(
        cat.get(201).unwrap().passive,
        "One-Handed Swords is passive"
    );
}

#[test]
fn real_spell_catalog_gates_the_spellbook() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Shown: Fireball's ranks, Frostbolt, Polymorph (0x80 clear, castUI 0).
    for id in [133, 143, 145, 116, 118] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert!(
            d.in_spellbook(&crate::SummonPropertiesCatalog::default()),
            "spell {id} should show ({:#x})",
            d.attributes
        );
        assert_eq!(d.attributes & 0x80, 0, "spell {id} is not DO_NOT_DISPLAY");
        assert_eq!(d.cast_ui, 0, "an ordinary spell reads castUI 0");
    }

    // Hidden: a language, armor and weapon proficiencies, each `0xC0 = PASSIVE | DO_NOT_DISPLAY`.
    for (id, what) in [
        (668u32, "Language: Common"),
        (9078, "Cloth"),
        (9077, "Leather"),
        (196, "One-Handed Axes"),
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id} {what}"));
        assert_eq!(
            d.attributes & 0xC0,
            0xC0,
            "{what} ({id}) is PASSIVE|DO_NOT_DISPLAY"
        );
        assert!(
            !d.in_spellbook(&crate::SummonPropertiesCatalog::default()),
            "{what} ({id}) is hidden from the book"
        );
    }
}

/// The OPEN_LOCK effect (column 71 == 0x21) and its `LockType` (`EffectMiscValue`, column 110): a
/// Copper Vein's `Lock.dbc` skill slot names LockType 3, which Mining (2575) opens.
#[test]
fn real_spell_catalog_reads_open_lock_types() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    assert_eq!(
        cat.get(2575).unwrap().open_lock_type(),
        Some(3),
        "Mining opens LockType 3"
    );
    assert_eq!(
        cat.get(2366).unwrap().open_lock_type(),
        Some(2),
        "Herb Gathering opens LockType 2"
    );
    assert_eq!(
        cat.get(1804).unwrap().open_lock_type(),
        Some(1),
        "Pick Lock opens LockType 1"
    );
    assert_eq!(
        cat.get(133).unwrap().open_lock_type(),
        None,
        "Fireball opens no lock"
    );

    // LockType 13 has two openers every character knows; the resolver takes the first sufficient
    // one in known-spell order, and the cast bar prints it.
    assert_eq!(cat.get(6478).unwrap().open_lock_type(), Some(13));
    assert_eq!(cat.get(6478).unwrap().name, "Opening");
    assert_eq!(cat.get(22810).unwrap().open_lock_type(), Some(13));
    assert_eq!(
        cat.get(22810).unwrap().name,
        "Opening - No Text",
        "the placeholder is Blizzard's own Spell.dbc string, not a formatting bug of ours"
    );

    // The cast bar shows no name for `AttributesEx3 & 0x4` (`UnitCastingInfo` `0x611df0`), the
    // one column separating 22810 from 6478; 28 shipped rows carry it.
    assert!(cat.get(22810).unwrap().no_casting_bar_text());
    assert!(!cat.get(6478).unwrap().no_casting_bar_text());
    let silent: Vec<u32> = {
        let mut v: Vec<u32> = cat
            .iter()
            .filter(|(_, d)| d.no_casting_bar_text())
            .map(|(id, _)| id)
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(silent.len(), 28);
    assert_eq!(silent[..3], [6477, 21651, 22810]);

    // The tools: 12340 gathering names a `TotemCategory` (columns 222-223), not an item, so the
    // Mining Pick and Skinning Knife columns are empty; reagents are columns 52-59, counts 60-67.
    let mining = cat.get(2575).unwrap();
    assert_eq!((mining.totems, mining.totem_categories), ([0, 0], [165, 0]));
    let skinning = cat.get(8613).unwrap();
    assert_eq!(
        (skinning.totems, skinning.totem_categories),
        ([0, 0], [166, 0])
    );
    let slow_fall = cat.get(130).unwrap();
    assert_eq!(
        slow_fall.reagents[0],
        (17056, 1),
        "Slow Fall consumes one Light Feather (17056)"
    );
    assert_eq!(cat.get(133).unwrap().totems, [0, 0]);
}

/// The two `Effect[0]` values the client latches at learn time (`0x4b25e0` into `[0xb700e4]` and
/// `[0xb700e8]`); the cursor's skin leg shows the knife only when one of them is in the book.
#[test]
fn real_spell_catalog_pins_the_skin_latch_effects() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Skinning (8613): `0x4b2623: cmp [esi+0xf4], 0x5f`.
    assert_eq!(
        cat.get(8613).unwrap().effects[0],
        crate::SPELL_EFFECT_SKINNING,
        "Skinning carries SPELL_EFFECT_SKINNING (95 == 0x5f)"
    );
    // Remove Insignia (22027), the `[0xb700e8]` half: `0x4b2632: cmp [esi+0xf4], 0x74`.
    assert_eq!(
        cat.get(22027).unwrap().effects[0],
        0x74,
        "Remove Insignia carries SPELL_EFFECT_SKIN_PLAYER_CORPSE (116 == 0x74)"
    );
    // Nothing an ordinary caster starts with does, so a non-skinner's latch stays empty.
    assert_ne!(
        cat.get(133).unwrap().effects[0],
        crate::SPELL_EFFECT_SKINNING
    );
    assert_ne!(
        cat.get(6247).unwrap().effects[0],
        crate::SPELL_EFFECT_SKINNING
    );
}

/// The left side of the client's lock test (`0x5f850f`), with the player's skill as the level
/// term (`0x5ea690`). Its columns (27, 28, 64, 67, 70, 73, 76) are pinned by result, against
/// values known from the game; the below-cap rows are the ones a caster-level term would miss.
#[test]
fn real_spell_catalog_computes_the_lock_skill_an_opener_provides() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Pick Lock (1804): `4 + 1 + 5.0×(skill/5 − 1)`, the skill itself.
    assert_eq!(cat.get(1804).unwrap().open_lock_skill(300), Some(300));
    assert_eq!(cat.get(1804).unwrap().open_lock_skill(225), Some(225));
    assert_eq!(cat.get(1804).unwrap().open_lock_skill(150), Some(150));
    // Mining (2575), Herb Gathering (2366): `−1 + 1 + 5.0×(skill/5)`; 1 Mining provides 0.
    assert_eq!(cat.get(2575).unwrap().open_lock_skill(300), Some(300));
    assert_eq!(cat.get(2575).unwrap().open_lock_skill(100), Some(100));
    assert_eq!(cat.get(2575).unwrap().open_lock_skill(1), Some(0));
    assert_eq!(cat.get(2366).unwrap().open_lock_skill(300), Some(300));
    // Seaforium Charges (4056, 4075): a flat 150 and 250; lock 92 asks for Blasting 150.
    assert_eq!(cat.get(4056).unwrap().open_lock_skill(0), Some(150));
    assert_eq!(cat.get(4056).unwrap().open_lock_skill(300), Some(150));
    assert_eq!(cat.get(4075).unwrap().open_lock_skill(0), Some(250));
    // The "Opening"/"Closing" family every character knows is a flat 100, so the Action gate,
    // not the value test, keeps them off a padlocked door.
    for id in [3365, 6233, 6246, 6247, 6477, 6478, 21651, 21652] {
        assert_eq!(
            cat.get(id).unwrap().open_lock_skill(0),
            Some(100),
            "spell {id} is a flat-100 opener"
        );
        assert_eq!(cat.get(id).unwrap().open_lock_skill(300), Some(100));
    }
    assert_eq!(cat.get(133).unwrap().open_lock_skill(300), None);
}

#[test]
fn real_spell_catalog_reads_cooldown_cost_and_range_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Fireball r1: no cooldown, the ordinary GCD (category 133, 1500 ms), 8% of base mana,
    // range row 35.
    let fireball = cat.get(133).unwrap();
    assert_eq!(
        (
            fireball.category,
            fireball.recovery_ms,
            fireball.category_recovery_ms
        ),
        (0, 0, 0)
    );
    assert_eq!(
        (fireball.start_recovery_category, fireball.start_recovery_ms),
        (133, 1500)
    );
    assert_eq!(
        (
            fireball.power_type,
            fireball.mana_cost,
            fireball.mana_cost_pct
        ),
        (0, 0, 8)
    );
    assert_eq!(fireball.range_index, 35);
    assert!(!fireball.cooldown_on_event());

    // Charge: category 1219 with a 15 s category cooldown, rage (1), its own GCD category 1178
    // with no time, range row 95.
    let charge = cat.get(100).unwrap();
    assert_eq!(
        (
            charge.category,
            charge.recovery_ms,
            charge.category_recovery_ms
        ),
        (1219, 0, 15000)
    );
    assert_eq!(
        (charge.start_recovery_category, charge.start_recovery_ms),
        (1178, 0)
    );
    assert_eq!(charge.power_type, 1, "Charge costs rage");
    assert_eq!(charge.range_index, 95);

    // Feign Death: a 30 s RecoveryTime and the cooldown-on-event bit of `0x2150100`.
    let feign = cat.get(5384).unwrap();
    assert_eq!(feign.recovery_ms, 30_000);
    assert!(
        feign.cooldown_on_event(),
        "Feign Death is cooldown-on-event"
    );

    // Lay on Hands: the 20-minute category cooldown (56 / 1_200_000).
    let loh = cat.get(633).unwrap();
    assert_eq!((loh.category, loh.category_recovery_ms), (56, 1_200_000));

    // ManaCostPercentage: Purge r1 (370) is 8, Dispel Magic r1 (527) 14.
    assert_eq!(cat.get(370).unwrap().mana_cost_pct, 8);
    assert_eq!(cat.get(527).unwrap().mana_cost_pct, 14);
}

/// [`COL_TARGETS`] (16) and [`COL_IMPLICIT_TARGET_A1`] (86).
#[test]
fn real_spell_catalog_reads_cast_targeting_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Implicit targets: 6 single enemy (hostile bit), 1 self, 21 single friend (assist bit), 56
    // the caster's raid around it (no arm, so the mask stays 0).
    assert_eq!(cat.get(133).unwrap().implicit_target_a1, 6, "Fireball");
    assert_eq!(cat.get(7302).unwrap().implicit_target_a1, 1, "Ice Armor");
    assert_eq!(cat.get(5384).unwrap().implicit_target_a1, 1, "Feign Death");
    assert_eq!(
        cat.get(1459).unwrap().implicit_target_a1,
        21,
        "Arcane Intellect"
    );
    assert_eq!(
        cat.get(6673).unwrap().implicit_target_a1,
        56,
        "Battle Shout"
    );

    // The `Targets` seed mask: 0 for ordinary casts; Resurrection carries the corpse-ally bit 15,
    // Skinning unit bit 1 and the requires-explicit-selection bit 10.
    assert_eq!(cat.get(133).unwrap().targets, 0);
    assert_eq!(cat.get(6673).unwrap().targets, 0);
    assert_eq!(cat.get(2006).unwrap().targets, 0x8000, "Resurrection");
    assert_eq!(cat.get(8613).unwrap().targets, 0x402, "Skinning");
}

/// The usable walk's columns (`0x80b5d0`), one row per gate family, and the real form flags.
#[test]
fn real_spell_catalog_reads_usable_walk_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let forms = load_shapeshift_forms(&mut chain).expect("load SpellShapeshiftForm");
    let flags = |form: u8| forms.get(&u32::from(form)).map_or(0, |f| f.flags);

    // Claw: cat form (1). Ambush: stealth (form 30), a dagger and the only-stealthed attribute.
    // Execute: battle or berserker stance and the target's below-20% aura state. Revenge: the
    // caster's defense state.
    let claw = cat.get(1082).unwrap();
    assert_eq!(claw.stances, 0x1);
    let ambush = cat.get(8676).unwrap();
    assert_eq!(ambush.stances, 0x2000_0000);
    assert_ne!(ambush.attributes & ATTR_ONLY_STEALTHED, 0);
    assert_eq!(
        (
            ambush.equipped_item_class,
            ambush.equipped_item_subclass_mask
        ),
        (2, 0x8000)
    );
    let execute = cat.get(5308).unwrap();
    assert_eq!((execute.stances, execute.target_aura_state), (0x50000, 2));
    assert_eq!(cat.get(6572).unwrap().caster_aura_state, 1, "Revenge");

    // The combo-point gate: Overpower has no aura state; its window rides `AttributesEx` bit 20
    // like the finishers.
    for rank in [7384, 7887, 11584, 11585] {
        let op = cat.get(rank).unwrap();
        assert!(op.needs_combo_points(), "Overpower {rank}");
        assert_eq!(
            (op.caster_aura_state, op.target_aura_state),
            (0, 0),
            "Overpower {rank} has no aura-state gate — the combo-point gate is all that holds it"
        );
    }
    for finisher in [
        2098, /* Eviscerate */
        1943, /* Rupture */
        5171, /* Slice and Dice */
    ] {
        assert!(
            cat.get(finisher).unwrap().needs_combo_points(),
            "{finisher}"
        );
    }
    for plain in [
        78,   /* Heroic Strike */
        5308, /* Execute */
        6572, /* Revenge */
    ] {
        assert!(!cat.get(plain).unwrap().needs_combo_points(), "{plain}");
    }

    // Auto Shot: bows/guns/crossbows. Slow Fall: one Light Feather.
    let auto_shot = cat.get(75).unwrap();
    assert_eq!(
        (
            auto_shot.equipped_item_class,
            auto_shot.equipped_item_subclass_mask
        ),
        (2, 0x4000c)
    );
    assert_eq!(cat.get(130).unwrap().reagents[0], (17056, 1), "Slow Fall");

    // Battle Stance (17) is a stance (flag 0x1), Cat Form (1) a true shapeshift.
    assert!(forms.get(&17).unwrap().is_stance());
    assert!(!forms.get(&1).unwrap().is_stance());
    assert_eq!(forms.get(&1).unwrap().bonus_bar, 1);

    // Claw in cat only; Fireball unshifted and in a stance, not in Cat Form; Execute in Battle
    // (17), not in Defensive (18).
    assert!(claw.usable_in_form(1, flags(1)));
    assert!(!claw.usable_in_form(0, 0));
    let fireball = cat.get(133).unwrap();
    assert!(fireball.usable_in_form(0, 0));
    assert!(fireball.usable_in_form(17, flags(17)));
    assert!(!fireball.usable_in_form(1, flags(1)));
    assert!(execute.usable_in_form(17, flags(17)));
    assert!(!execute.usable_in_form(18, flags(18)));

    // Ghost Wolf: form 16 is a cancelable true shapeshift; the spell carries NOT_SHAPESHIFT (bit
    // 16) and the stance-bar exclusion (`AttributesEx2 & 0x2`: a shaman gets no stance bar). In
    // the form an ordinary spell refuses NOT_SHAPESHIFT; a form spell out of its form refuses
    // ONLY_SHAPESHIFT.
    let ghost_wolf = cat.get(2645).unwrap();
    assert_eq!(ghost_wolf.shapeshift_form, Some(16));
    assert_ne!(ghost_wolf.attributes & 0x1_0000, 0);
    assert_ne!(ghost_wolf.attributes_ex2 & 0x2, 0);
    let wolf_row = forms.get(&16).unwrap();
    assert!(!wolf_row.is_stance());
    assert!(wolf_row.cancelable());
    use crate::FormRefusal;
    let bolt = cat.get(403).unwrap();
    assert_eq!(
        bolt.form_refusal(16, flags(16)),
        Some(FormRefusal::NotShapeshift)
    );
    assert_eq!(bolt.form_refusal(0, 0), None);
    assert_eq!(claw.form_refusal(0, 0), Some(FormRefusal::OnlyShapeshift));
    assert_eq!(
        ghost_wolf.form_refusal(16, flags(16)),
        Some(FormRefusal::NotShapeshift),
        "re-pressing Ghost Wolf in the form draws the gate too (the cancel is a separate branch)"
    );
    // Moonkin (31) carries the stance flag, so an ordinary spell casts in it; the Zombie form
    // (21) carries `0x400`, which refuses every spell its mask does not name.
    assert!(forms.get(&31).unwrap().is_stance());
    assert_eq!(bolt.form_refusal(31, flags(31)), None);
    assert_ne!(flags(21) & 0x400, 0);
    assert_eq!(
        claw.form_refusal(21, flags(21)),
        Some(FormRefusal::NotShapeshift)
    );

    // The icons: Ghost Wolf and Battle Stance both carry an `ActiveIconID`.
    assert_ne!(ghost_wolf.active_icon_id, 0);
    assert_eq!(cat.get(2457).unwrap().active_icon_id, 279, "Battle Stance");

    // AttackIconID (column 21): Cat Form has its own, Ghost Wolf's 0 means the weapon.
    assert_eq!(
        forms.get(&1).unwrap().attack_icon.as_deref(),
        Some("Interface\\Icons\\Ability_Druid_CatFormAttack")
    );
    assert_eq!(wolf_row.attack_icon, None);
}

#[test]
fn real_spell_catalog_reads_tooltip_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let cast_times = load_spell_cast_times(&mut chain).expect("load SpellCastTimes");
    let durations = load_spell_durations(&mut chain).expect("load SpellDuration");

    // Fireball r1 has a real 4 s damage-over-time tail ("over $d"), a 2 s tick in effect slot 1.
    let fireball = cat.get(133).unwrap();
    assert!(
        fireball
            .description
            .as_deref()
            .unwrap()
            .starts_with("Hurls a fiery ball that causes"),
        "Fireball description: {:?}",
        fireball.description
    );
    assert_eq!(fireball.casting_time_index, 16);
    assert_eq!(
        cast_times.get(16).unwrap().base_ms,
        1500,
        "Fireball's real cast time"
    );
    assert_eq!(fireball.duration_index, 35);
    assert_eq!(
        durations.get(35).unwrap().base_ms,
        4000,
        "Fireball's DoT-tail duration, not zero — it genuinely has one"
    );
    assert_eq!(
        fireball.proc_chance, 101,
        "vmangos's always-triggers sentinel"
    );
    assert_eq!(
        (fireball.effect_base_points[0], fireball.effect_die_sides[0]),
        (13, 9),
        "Fireball r1's direct-damage roll: 14-22"
    );
    assert_eq!(
        (fireball.effect_apply_aura[1], fireball.effect_amplitude[1]),
        (3, 2000),
        "Fireball's periodic-damage tail: SPELL_AURA_PERIODIC_DAMAGE ticking every 2s"
    );

    // Frost Armor: 30 minutes, instant, its description naming the chill proc 6136.
    let frost_armor = cat.get(168).unwrap();
    assert!(frost_armor
        .aura_description
        .as_deref()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(frost_armor.duration_index, 30);
    assert_eq!(durations.get(30).unwrap().base_ms, 1_800_000, "30 minutes");
    assert_eq!(frost_armor.casting_time_index, 1);
    assert_eq!(cast_times.get(1).unwrap().base_ms, 0, "instant");
    assert_eq!(
        cast_times.get(18).unwrap().resolved_ms(60, 1),
        -1_000_000,
        "the hunter shots' row reads signed and stays negative"
    );
    assert_eq!(
        frost_armor.effect_apply_aura[0], 22,
        "SPELL_AURA_MOD_RESISTANCE"
    );
    assert_eq!(
        frost_armor.effect_trigger_spell[1], 6136,
        "the chill proc the description text itself names"
    );

    // Fire Blast has no aura: no aura text and no duration row.
    let fire_blast = cat.get(2136).unwrap();
    assert_eq!(fire_blast.aura_description, None);
    assert_eq!(fire_blast.duration_index, 0);
    assert!(durations.get(0).is_none(), "no row 0 in SpellDuration.dbc");

    // Auto Shot and Feign Death: the signed EffectBasePoints sentinel -1, no fixed roll.
    assert_eq!(cat.get(75).unwrap().effect_base_points[0], -1, "Auto Shot");
    assert_eq!(
        cat.get(5384).unwrap().effect_base_points[0],
        -1,
        "Feign Death"
    );
}

/// The WDBC schema must decode the new float arrays before `f32_at` can pass them to token
/// expansion. An integer schema silently turns Chain Heal's 0.5 into the loader's 0.0 fallback.
#[test]
fn spell_schema_decodes_token_float_columns() {
    let mut raw = Vec::new();
    raw.extend_from_slice(b"WDBC");
    for n in [1u32, SPELL_FIELDS as u32, (SPELL_FIELDS * 4) as u32, 1] {
        raw.extend_from_slice(&n.to_le_bytes());
    }
    let mut row = vec![0u8; SPELL_FIELDS * 4];
    row[COL_DAMAGE_MULTIPLIER_1 * 4..COL_DAMAGE_MULTIPLIER_1 * 4 + 4]
        .copy_from_slice(&0.5f32.to_le_bytes());
    row[COL_EFFECT_POINTS_PER_COMBO_POINT_1 * 4..COL_EFFECT_POINTS_PER_COMBO_POINT_1 * 4 + 4]
        .copy_from_slice(&20.0f32.to_le_bytes());
    raw.extend_from_slice(&row);
    raw.push(0);
    let set = parse(&raw, spell_schema(), "Spell.dbc").expect("synthetic Spell.dbc");
    let spell = set.records().first().unwrap();
    assert_eq!(f32_at(spell, COL_DAMAGE_MULTIPLIER_1), Some(0.5));
    assert_eq!(
        f32_at(spell, COL_EFFECT_POINTS_PER_COMBO_POINT_1),
        Some(20.0)
    );
}

/// The attack-start masks: [`SpellDisplay::on_next_swing`] (`0x404`),
/// [`SpellDisplay::initiates_auto_attack`] (adding `AttributesEx & 0x200`) and
/// [`SpellDisplay::initiates_auto_attack_at_go`] (`AttributesEx2 & 0x100000`), whose union is
/// [`SpellDisplay::initiates_combat`] (`0x7fcc70`).
#[test]
fn real_spell_catalog_classifies_combat_initiation() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // (spell, on_next_swing, initiates_auto_attack, initiates_auto_attack_at_go)
    for (id, name, next_swing, initiates, at_go) in [
        (78u32, "Heroic Strike", true, true, false),
        (845, "Cleave", true, true, false),
        (2973, "Raptor Strike", true, true, false),
        (772, "Rend", false, true, false),
        (7386, "Sunder Armor", false, true, false),
        (1464, "Slam", false, true, false),
        (1752, "Sinister Strike", false, true, false),
        (100, "Charge", false, false, false),
        (6673, "Battle Shout", false, false, false),
        (6603, "Auto Attack", false, false, false),
        (133, "Fireball", false, false, false),
        // The GO-deferred class: bit 20 set, so the attack starts at `SMSG_SPELL_GO` (`0x80e1b0`).
        (53, "Backstab", false, false, true),
        (703, "Garrote", false, false, true),
        (8676, "Ambush", false, false, true),
        (1833, "Cheap Shot", false, false, true),
        (5221, "Shred", false, false, true),
        (6785, "Ravage", false, false, true),
        (9005, "Pounce", false, false, true),
        (20271, "Judgement of Light", false, false, true),
        (49576, "Death Grip", false, false, true),
        (2764, "Throw", false, false, true),
        // The hunter's instant shots start no attack of either kind.
        (1978, "Serpent Sting", false, false, false),
        (3044, "Arcane Shot", false, false, false),
        (2643, "Multi-Shot", false, false, false),
        (75, "Auto Shot", false, false, false),
    ] {
        let d = cat
            .get(id)
            .unwrap_or_else(|| panic!("{name} ({id}) in the catalog"));
        assert_eq!(d.name, name, "{id}");
        assert_eq!(
            d.on_next_swing(),
            next_swing,
            "{name} ({id}) on_next_swing (Attributes {:#x})",
            d.attributes
        );
        assert_eq!(
            d.initiates_auto_attack(),
            initiates,
            "{name} ({id}) initiates_auto_attack (Attributes {:#x}, Ex {:#x}, Ex2 {:#x})",
            d.attributes,
            d.attributes_ex,
            d.attributes_ex2
        );
        assert_eq!(
            d.initiates_auto_attack_at_go(),
            at_go,
            "{name} ({id}) initiates_auto_attack_at_go (Ex2 {:#x})",
            d.attributes_ex2
        );
        assert_eq!(
            d.initiates_combat(),
            next_swing || initiates || at_go,
            "{name} ({id}) initiates_combat"
        );
        // Bit 20 suppresses the send-time start, so no spell starts the attack twice.
        assert!(
            !(d.initiates_auto_attack() && d.initiates_auto_attack_at_go()),
            "{name} ({id}) may not start the auto-attack at BOTH the send and the GO"
        );
    }

    // 77 shipped rows carry bit 20: the openers, the positional strikes, the Judgements, Death
    // Grip, Throw and a few creature and test rows.
    let deferred: Vec<&str> = cat
        .iter()
        .filter(|(_, d)| d.initiates_auto_attack_at_go())
        .map(|(_, d)| d.name.as_str())
        .collect();
    assert_eq!(deferred.len(), 77);
    let mut names = deferred;
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names,
        [
            "Ambush",
            "Backstab",
            "Caught!",
            "Cheap Shot",
            "Death Grip",
            "Garrote",
            "Gnaw",
            "Judgement",
            "Judgement of Justice",
            "Judgement of Light",
            "Judgement of Wisdom",
            "Mutilate",
            "Pounce",
            "Ravage",
            "Shiv",
            "Shred",
            "Test Backstab",
            "Test Stab R50",
            "Throw",
        ]
    );
}

/// TryCast's attack pick (`0x6e4edf`) is built for a spell press alone, so no form, recipe
/// (`SPELL_EFFECT_CREATE_ITEM` 24, `SPELL_EFFECT_ENCHANT_ITEM` 53) or `modalNextSpell` chain may
/// carry predicate `0x6e5200`. No item's use spell does either (vmangos `item_template`).
#[test]
fn real_spell_catalog_keeps_the_attack_pick_to_spell_presses() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let (mut checked, mut chained) = (0, 0);
    for (id, d) in cat.iter() {
        let recipe = d.effects.iter().any(|&e| e == 24 || e == 53);
        if d.shapeshift_form.is_some() || recipe {
            checked += 1;
            assert!(
                !d.initiates_combat(),
                "{} ({id}) is a form or a recipe of predicate 0x6e5200",
                d.name
            );
        }
        if d.modal_next_spell != 0 {
            chained += 1;
            let next = cat.get(d.modal_next_spell);
            assert!(
                !next.is_some_and(|n| n.initiates_combat()),
                "{} ({id}) chains {} of predicate 0x6e5200",
                d.name,
                d.modal_next_spell
            );
        }
    }
    assert!(
        checked > 0 && chained > 0,
        "the forms, recipes and Auto Shot's chain are read"
    );
}

/// `modalNextSpell`, column 48, which the cast's finish (`0x8054f0`) reads to start Auto Shot
/// after a hunter shot: nonzero on 111 rows, 106 of them naming spell 75.
#[test]
fn real_spell_catalog_reads_modal_next_spell() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // Rank 1 of each hunter shot.
    for (id, name, next) in [
        (1978u32, "Serpent Sting", 75u32),
        (3044, "Arcane Shot", 75),
        (2643, "Multi-Shot", 75),
        (5116, "Concussive Shot", 75),
        (19434, "Aimed Shot", 75),
        (3034, "Viper Sting", 75),
        (3043, "Scorpid Sting", 75),
        (3674, "Black Arrow", 75),
        (20736, "Distracting Shot", 75),
        (53351, "Kill Shot", 75),
        (56641, "Steady Shot", 75),
        // Auto Shot's own column is 0: the chain is one hop.
        (75, "Auto Shot", 0),
        // A melee strike and an ordinary cast carry nothing here.
        (78, "Heroic Strike", 0),
        (133, "Fireball", 0),
    ] {
        let d = cat
            .get(id)
            .unwrap_or_else(|| panic!("{name} ({id}) in the catalog"));
        assert_eq!(d.name, name, "{id}");
        assert_eq!(d.modal_next_spell, next, "{name} ({id}) modalNextSpell");
    }

    let chained: Vec<(u32, u32)> = cat
        .iter()
        .filter(|(_, d)| d.modal_next_spell != 0)
        .map(|(id, d)| (id, d.modal_next_spell))
        .collect();
    assert_eq!(chained.len(), 111);
    assert_eq!(
        chained.iter().filter(|&&(_, next)| next == 75).count(),
        106,
        "the rest: three (TEST) bow shot rows → 59, two Minigun → 23675"
    );
    // Nothing points at a spell that chains onward, but for the two that name themselves.
    for &(id, next) in &chained {
        let onward = cat.get(next).map(|d| d.modal_next_spell).unwrap_or(0);
        assert!(
            onward == 0 || onward == next,
            "spell {id} chains {next}, which itself chains {onward} — the chain must not walk"
        );
    }
}

/// `EffectItemType` (columns 103-105) and `RequiresSpellFocus` (15); values from vmangos.
#[test]
fn real_crafting_columns_read_created_item_and_focus() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // (recipe spell, created item, focus): Bolt of Linen Cloth, Minor Healing Potion,
    // Copper Axe, Crafted Light Shot, Charred Wolf Meat.
    for (spell, item, focus) in [
        (2963u32, 2996u32, 0u32),
        (2330, 118, 0),
        (2738, 2845, 1), // Blacksmithing needs the Anvil (focus 1)
        (3920, 8067, 0),
        (2538, 2679, 4), // Cooking needs a Cooking Fire (focus 4)
    ] {
        let d = cat.get(spell).expect("recipe in the catalog");
        assert_eq!(
            d.effects[0], SPELL_EFFECT_CREATE_ITEM,
            "spell {spell} creates"
        );
        assert_eq!(d.effect_item_type[0], item, "spell {spell} created item");
        assert_eq!(d.requires_spell_focus, focus, "spell {spell} focus");
    }

    // Crafted Light Shot makes 200 a craft: BasePoints 199 plus DieSides 1.
    let shots = cat.get(3920).expect("Crafted Light Shot");
    assert_eq!(shots.effect_base_points[0], 199);
    assert_eq!(shots.effect_die_sides[0], 1);

    // The openers carry effect 47 and no product: Tailoring 3908, Enchanting 7411.
    for opener in [3908u32, 7411] {
        let d = cat.get(opener).expect("opener in the catalog");
        assert_eq!(d.effects[0], SPELL_EFFECT_TRADE_SKILL, "opener {opener}");
        assert_eq!(
            d.effect_item_type, [0; 3],
            "opener {opener} creates nothing"
        );
    }
}

#[test]
fn the_tooltip_gates_read_effect_and_mask() {
    // `0x52eb15`: the cast and cooldown line is omitted for the passive bit or Effect[0] 47 or 78.
    let plain = SpellDisplay::default();
    assert!(!plain.tooltip_omits_cast_line());
    let attribute_passive = SpellDisplay {
        passive: true,
        ..Default::default()
    };
    assert!(attribute_passive.tooltip_omits_cast_line());
    // 6603 "Attack"'s shape: Effect[0] 78, attributes 0x10, not the passive bit.
    let auto_attack = SpellDisplay {
        effects: [78, 0, 0],
        attributes: 0x10,
        ..Default::default()
    };
    assert!(!auto_attack.passive, "the attribute bit is clear");
    assert!(auto_attack.tooltip_omits_cast_line(), "the Effect[0] leg");
    let trade_skill = SpellDisplay {
        effects: [47, 0, 0],
        ..Default::default()
    };
    assert!(trade_skill.tooltip_omits_cast_line());
}

/// The weapon auto-pick family (`Attributes & 0x200`): for a player caster the cast binds an
/// equipped weapon instead of arming the item cursor (`BeginCast 0x80c790`). On 12340 it is the
/// weapon imbues and the fishing lures, each with `Targets == 0x10` and a temporary enchant (54),
/// most gated on a weapon or a fishing pole, but for Windfury Weapon 32911, a 54-less creature
/// row. Rockbiter's four ranks carry their enchant in effect slot 1.
#[test]
fn real_main_hand_autopick_family() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    let mut names = std::collections::BTreeSet::<String>::new();
    let mut rows = 0usize;
    for (id, d) in cat.iter().filter(|(_, d)| d.targets_main_hand_item()) {
        rows += 1;
        names.insert(d.name.clone());
        if id == 32911 {
            assert_eq!((d.targets, d.effects), (0, [6, 0, 0]));
            continue;
        }
        assert_eq!(d.targets, 0x10, "spell {id} ({})", d.name);
        let slot = d
            .effects
            .iter()
            .position(|&e| e == crate::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY);
        let expected = if [8017, 8018, 8019, 10399].contains(&id) {
            1
        } else {
            0
        };
        assert_eq!(slot, Some(expected), "spell {id} ({})", d.name);
    }
    assert_eq!(rows, 70, "the whole auto-pick family");
    assert_eq!(
        names.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "Aquadynamic Fish Attractor",
            "Aquadynamic Fish Lens",
            "Bright Baubles",
            "Consecrated Weapon",
            "Earthliving Weapon",
            "Fishing Hat Lure",
            "Flametongue Weapon",
            "Flesh Eating Worm",
            "Frostbrand Weapon",
            "Frozen Rune Weapon",
            "Glow Worm",
            "Nightcrawlers",
            "Rockbiter Weapon",
            "Sharpened Fish Hook",
            "Shiny Bauble",
            "Windfury Weapon",
        ]
    );
    // A lure gates on the fishing pole (weapon subclass 20).
    let bauble = cat
        .iter()
        .find(|(_, d)| d.name == "Shiny Bauble")
        .map(|(_, d)| d)
        .expect("Shiny Bauble");
    assert_eq!(
        (
            bauble.equipped_item_class,
            bauble.equipped_item_subclass_mask
        ),
        (2, 1 << 20)
    );
    // Windfury Totem is not in it: a totem summon with no target word at all.
    let wf_totem = cat.get(8512).expect("Windfury Totem");
    assert!(!wf_totem.targets_main_hand_item());
    assert_eq!(wf_totem.targets, 0, "a totem summon binds nothing");
}

/// 12340 ships the prospecting (127) and milling (158) effects, whose casts the server refuses
/// with `NEED_MORE_ITEMS` and `MIN_SKILL`, both carrying two argument words.
#[test]
fn real_prospecting_and_milling_effects_ship_in_12340() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let raw = chain.read_file(SPELL).expect("Spell.dbc");
    let set = parse(&raw, spell_schema(), "Spell.dbc").expect("parse Spell.dbc");

    let mut slots = 0usize;
    let mut found = Vec::new();
    for r in set.records() {
        for i in 0..3 {
            slots += 1;
            let effect = u32_at(r, COL_EFFECT_1 + i).unwrap_or(0);
            if effect == crate::SPELL_EFFECT_PROSPECTING || effect == 158 {
                found.push((u32_at(r, 0).unwrap_or(0), effect, i));
            }
        }
    }
    found.sort_unstable();
    assert_eq!(
        found,
        [
            (31032, 127, 0),
            (31252, 127, 0),
            (51005, 158, 0),
            (58510, 158, 1)
        ],
        "TestProspect, Prospecting, Milling, Magic Impedence"
    );
    // A scan that read nothing would pass too, so pin the row count.
    assert_eq!(
        slots,
        49839 * 3,
        "every effect slot of every 12340 row was read"
    );
}

/// `Targets & 0x4010` (the item and lock bits) never mixes with a unit bit on shipped data:
/// `Targets` is exactly `0x10` (enchants, poisons, stones, scopes) or `0x4000` (OPEN_LOCK), so the
/// resolver forks on the bare word.
#[test]
fn real_item_target_family_and_its_gate_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // The unit-shaped bits of the flag_word (`cast_target`'s `UNIT_BITS`).
    const UNIT_BITS: u32 = 0x0002 | 0x0004 | 0x0008 | 0x0080 | 0x0100 | 0x0200 | 0x0400 | 0x8000;
    let mut item_only = 0usize;
    let mut locked_only = 0usize;
    for (id, d) in cat.iter() {
        if d.targets & 0x4010 == 0 {
            continue;
        }
        assert_eq!(
            d.targets & UNIT_BITS,
            0,
            "spell {id} mixes an item bit with a unit bit — the resolver's fork would be wrong"
        );
        match d.targets {
            0x0010 => item_only += 1,
            0x4000 => locked_only += 1,
            other => panic!("spell {id}: unexpected item-family word {other:#x}"),
        }
    }
    assert_eq!(
        item_only, 768,
        "Targets == 0x10 — the enchant/poison family"
    );
    assert_eq!(locked_only, 203, "Targets == 0x4000 — the OPEN_LOCK family");

    // The OPEN_LOCK implicit arm: `cast_target_mask` ORs `0x800` for arm 23 and `TF_UNIT` for arm
    // 25, so one armed cursor answers both the bag click and the world click.
    let mut arms = std::collections::BTreeMap::<u32, usize>::new();
    for (_, d) in cat.iter().filter(|(_, d)| d.targets == 0x4000) {
        *arms.entry(d.implicit_target_a1).or_default() += 1;
    }
    assert_eq!(
        arms,
        [(0, 2), (23, 200), (25, 1)].into_iter().collect(),
        "the family is overwhelmingly arm 23, whose overlay is TARGET_FLAG_GAMEOBJECT"
    );
    // Bit 11 in the `Targets` column itself: two creature rows, no player spell.
    let mut go_word: Vec<u32> = cat
        .iter()
        .filter(|(_, d)| d.targets & 0x800 != 0)
        .map(|(id, _)| id)
        .collect();
    go_word.sort_unstable();
    assert_eq!(go_word, [44678, 55088], "TrajTemp Damage, Barrel o' Fun");

    // Bracer enchant: armor (4), InventoryType WRIST(9) only.
    let bracer = cat.get(7418).unwrap();
    assert_eq!(bracer.targets, 0x10);
    assert_eq!(bracer.equipped_item_class, 4);
    assert_eq!(bracer.equipped_item_inventory_type_mask, 1 << 9);
    // Chest enchant: CHEST(5) or ROBE(20), so a cloth robe is legal.
    let chest = cat.get(7443).unwrap();
    assert_eq!(
        chest.equipped_item_inventory_type_mask,
        (1 << 5) | (1 << 20)
    );
    // A weapon-side row gates on class+subclass and leaves the type mask alone.
    let poison = cat.get(8679).unwrap();
    assert_eq!(
        (
            poison.equipped_item_class,
            poison.equipped_item_subclass_mask,
            poison.equipped_item_inventory_type_mask
        ),
        (2, 0x2a5f3, 0)
    );

    // `SpellDisplay` keeps an enchant effect in slot 0 for the item cursor's verdict: the only
    // item-target rows with it elsewhere are Rockbiter's four ranks, which the weapon auto-pick
    // binds instead (`0x80c790` walks all three slots). Read raw.
    let raw = chain.read_file(SPELL).expect("Spell.dbc");
    let set = parse(&raw, spell_schema(), "Spell.dbc").expect("parse Spell.dbc");
    let is_enchant = |e: u32| {
        e == crate::SPELL_EFFECT_ENCHANT_ITEM || e == crate::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY
    };
    let mut elsewhere: Vec<u32> = set
        .records()
        .iter()
        .filter(|r| u32_at(r, COL_TARGETS).unwrap_or(0) == 0x10)
        .filter(|r| {
            let e = |i| u32_at(r, COL_EFFECT_1 + i).unwrap_or(0);
            !is_enchant(e(0)) && (is_enchant(e(1)) || is_enchant(e(2)))
        })
        .map(|r| u32_at(r, 0).unwrap_or(0))
        .collect();
    elsewhere.sort_unstable();
    assert_eq!(elsewhere, [8017, 8018, 8019, 10399]);
    assert!(elsewhere.iter().all(|&id| cat
        .get(id)
        .is_some_and(SpellDisplay::targets_main_hand_item)));
}

/// The cooldown history's wildcard (`GetSpellCooldown 0x807980`, `SpellCategory` flags `0x2`): on
/// 12340 only category 351 carries it, and no shipped spell names that category, so no row reads
/// wildcard.
#[test]
fn gcd_wildcard_and_shape_corners_hold_on_the_real_data() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("catalog");

    assert_eq!(cat.get(5019).expect("wand Shoot").category, 0);
    assert_eq!(cat.iter().filter(|(_, d)| d.category == 351).count(), 0);
    assert_eq!(cat.iter().filter(|(_, d)| d.category_wildcard).count(), 0);

    // Armor's scroll spell: the {cat≠0, time=0} shape, which a GCD locks, since the pressed
    // spell's own time is never consulted.
    let scroll = cat.get(8091).expect("Scroll of Armor's spell");
    assert_eq!(
        (scroll.start_recovery_category, scroll.start_recovery_ms),
        (133, 0)
    );
}

/// The health power type is -2, read as `0xFFFFFFFE`.
#[test]
fn real_spell_catalog_cost_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");

    // Life Tap, every rank: health type and no cost at all, so the cost cell stays empty.
    for id in [1454u32, 1455, 1456, 11687, 11688, 11689, 27222, 57946] {
        let d = cat.get(id).unwrap();
        assert_eq!(
            (
                d.power_type,
                d.mana_cost,
                d.mana_cost_pct,
                d.mana_per_second
            ),
            (0xFFFF_FFFE, 0, 0, 0),
            "Life Tap {id}"
        );
    }
    // Bloodrage: a percentage-only health cost.
    let bloodrage = cat.get(2687).unwrap();
    assert_eq!(
        (
            bloodrage.power_type,
            bloodrage.mana_cost,
            bloodrage.mana_cost_pct
        ),
        (0xFFFF_FFFE, 0, 16)
    );
    // Health Funnel: flat health plus per second (the `_PER_TIME` line), and channeled.
    let funnel = cat.get(755).unwrap();
    assert_eq!(
        (funnel.power_type, funnel.mana_cost, funnel.mana_per_second),
        (0xFFFF_FFFE, 11, 5)
    );
    assert!(funnel.tooltip_channeled());
    // The cast cell's attribute arms: Heroic Strike (next melee; 150 on the wire is "15 Rage"),
    // Auto Shot and Throw (the ranged bit alone, so Throw reads "Attack speed"), Mind Flay
    // (channeled), Judgement of Light (percentage-only mana).
    let hs = cat.get(78).unwrap();
    assert_eq!((hs.power_type, hs.mana_cost), (1, 150));
    assert!(hs.on_next_swing());
    assert!(cat.get(75).unwrap().tooltip_on_next_ranged(), "Auto Shot");
    assert!(cat.get(2764).unwrap().tooltip_on_next_ranged(), "Throw");
    let mf = cat.get(15407).unwrap();
    assert!(mf.tooltip_channeled());
    assert_eq!((mf.power_type, mf.mana_cost, mf.mana_cost_pct), (0, 0, 9));
    let judgement = cat.get(20271).unwrap();
    assert_eq!(
        (
            judgement.power_type,
            judgement.mana_cost,
            judgement.mana_cost_pct
        ),
        (0, 0, 5)
    );

    // `manaPerSecondPerLevel` (column 45) is zero on every row, so it stays unparsed;
    // `manaCostPerlevel` (43) is nonzero on 138 creature spells, so `power_cost` applies it though
    // no player tooltip shows it.
    let bytes = chain.read_file(super::SPELL).expect("reading Spell.dbc");
    let rs = super::parse(&bytes, super::spell_schema(), "Spell.dbc").expect("Spell.dbc");
    let mut per_level_rows = 0u32;
    for r in rs.records() {
        let id = super::u32_at(r, 0).unwrap_or(0);
        if super::u32_at(r, COL_MANA_COST_PER_LEVEL).unwrap_or(0) > 0 {
            per_level_rows += 1;
        }
        assert_eq!(
            super::u32_at(r, COL_MANA_PER_SECOND_PER_LEVEL).unwrap_or(0),
            0,
            "manaPerSecondPerLevel on {id}"
        );
    }
    assert_eq!(per_level_rows, 138, "the manaCostPerlevel population");
    let dark_offering = cat.get(7154).unwrap();
    assert_eq!(
        (
            dark_offering.power_type,
            dark_offering.mana_cost,
            dark_offering.mana_cost_per_level,
            dark_offering.spell_level
        ),
        (0xFFFF_FFFE, 180, 9, 24),
        "Dark Offering: the per-level term's health-lane row"
    );
}

/// A chat language reaches its skill only through the spell whose `Effect_1` is
/// `SPELL_EFFECT_LANGUAGE`. On 12340, Demon Tongue (815) and Old Tongue (817) still declare Common
/// (7), so Demonic and Kalimag garble for everyone; Thalassian, Draconic and Titan declare their own.
#[test]
fn the_language_declaring_spells_cover_twelve_of_seventeen_languages() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let spells = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let skills = crate::skill_lines::load_skill_line_catalog(&mut chain).expect("load skill lines");

    // (spell, declared language, SkillLine.dbc id): every SPELL_EFFECT_LANGUAGE row.
    const DECLARED: &[(u32, u32, u32)] = &[
        (668, 7, 98),     // Common
        (669, 1, 109),    // Orcish
        (670, 3, 115),    // Taurahe
        (671, 2, 113),    // Darnassian
        (672, 6, 111),    // Dwarvish
        (813, 10, 137),   // Thalassian
        (814, 11, 138),   // Draconic
        (815, 7, 139),    // Demon Tongue, declaring Common
        (816, 9, 140),    // Titan
        (817, 7, 141),    // "Old Tongue (NYI)", declaring Common
        (7340, 13, 313),  // Gnomish
        (7341, 14, 315),  // Troll
        (17737, 33, 673), // Gutterspeak
        (29932, 35, 759), // Draenei
    ];

    assert_eq!(
        spells.declared_languages().count(),
        DECLARED.len(),
        "every SPELL_EFFECT_LANGUAGE row is accounted for"
    );
    for &(spell, language, line) in DECLARED {
        assert_eq!(
            spells.declared_language(spell),
            Some(language),
            "spell {spell} -> language"
        );
        // The second hop the garble gate walks: spell -> SkillLineAbility -> SkillLine.dbc id.
        assert_eq!(
            skills.spell_to_line(spell, 0, 0),
            Some(line),
            "spell {spell} -> skill line"
        );
    }

    // Both hops walked, these are the languages a character can ever understand.
    let understandable: std::collections::BTreeSet<u32> = spells
        .declared_languages()
        .filter(|(spell, _)| skills.spell_to_line(*spell, 0, 0).is_some())
        .map(|(_, l)| l)
        .collect();
    assert_eq!(
        understandable,
        [1, 2, 3, 6, 7, 9, 10, 11, 13, 14, 33, 35]
            .into_iter()
            .collect(),
        "twelve languages have both a declaring spell and a skill line"
    );

    // An ordinary ability declares nothing.
    assert_eq!(spells.declared_language(133), None); // Fireball
}

/// [`SpellDisplay::is_harmful`], the client's `0x6ea280 == 2`: an enemy implicit target in slot A
/// (6), or only in slot B as Frost Nova's (A 22, the caster's spot; B 15, a source-area enemy).
#[test]
fn real_is_harmful_pins() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let spells = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");
    let row = |id: u32| {
        spells
            .get(id)
            .unwrap_or_else(|| panic!("spell {id} in the catalog"))
    };
    for (id, name) in [
        (133u32, "Fireball"),
        (100, "Charge"),
        (7386, "Sunder Armor"),
        (122, "Frost Nova"),
    ] {
        assert!(row(id).is_harmful(), "{name} ({id}) targets enemies");
    }
    assert_eq!(
        (
            row(122).effect_implicit_target_a[0],
            row(122).effect_implicit_target_b[0]
        ),
        (22, 15),
        "Frost Nova is harmful through its B slot, the A slot being the caster's own spot"
    );
    for (id, name) in [
        (139u32, "Renew"),
        (5185, "Healing Touch"),
        (1459, "Arcane Intellect"),
        (6673, "Battle Shout"),
    ] {
        assert!(
            !row(id).is_harmful(),
            "{name} ({id}) does not target enemies"
        );
    }
}

/// `UnitChannelInfo` (`0x612090`): unlike the cast bar, the `"player"` token's channel text says
/// "Channeling" unless `AttributesEx` bit 29 lets it name the spell, as 74 of the 1961 channeled
/// rows do.
#[test]
fn real_channel_bar_name_law() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // The generic leg: every rank of Blizzard and other everyday channels.
    for id in [
        10u32, 6141, 8427, 10185, 10186, 10187, // Blizzard, ranks 1-6
        5143,  // Arcane Missiles
        15407, // Mind Flay
        689,   // Drain Life
        5740,  // Rain of Fire
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert!(d.tooltip_channeled(), "{id} {:?} channels", d.name);
        assert!(
            !d.channel_bar_own_name(),
            "{id} {:?} reads \"Channeling\", not its own name (AttributesEx {:#010x})",
            d.name,
            d.attributes_ex
        );
    }

    // The named leg.
    for (id, name) in [
        (7620u32, "Fishing"),
        (18248, "Fishing"),
        (20578, "Cannibalize"),
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert!(d.channel_bar_own_name(), "{id} names itself on the bar");
        assert_eq!(d.name, name);
    }

    let channeled: Vec<_> = cat.iter().filter(|(_, d)| d.tooltip_channeled()).collect();
    assert_eq!(channeled.len(), 1961, "channeled rows in the shipped file");
    assert_eq!(
        channeled
            .iter()
            .filter(|(_, d)| d.channel_bar_own_name())
            .count(),
        74,
        "the channeled rows that print their own name"
    );
}

/// `SpellFamilyName` (208) and the three `SpellFamilyFlags` dwords (209-211): 827 rows set several
/// bits and the top bit is 95, so the third dword is live; Fireball (bits 0 and 67) pins the
/// order of the low and the third.
#[test]
fn real_spell_family_columns_carry_the_modifier_gate() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    let mut families: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    let mut multi = 0usize;
    let mut max_bit = 0u32;
    for (_, d) in cat.iter() {
        *families.entry(d.spell_family).or_default() += 1;
        if d.spell_family_flags.count_ones() > 1 {
            multi += 1;
        }
        if d.spell_family_flags != 0 {
            max_bit = max_bit.max(127 - d.spell_family_flags.leading_zeros());
        }
    }
    assert_eq!(
        families,
        [
            (0, 40256),
            (1, 137),
            (3, 1200),
            (4, 736),
            (5, 1004),
            (6, 978),
            (7, 979),
            (8, 628),
            (9, 1151),
            (10, 740),
            (11, 1049),
            (12, 2),
            (13, 375),
            (15, 576),
            (17, 28),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<u32, usize>>(),
        "SpellFamilyName histogram"
    );
    assert_eq!(multi, 827, "rows setting more than one family bit");
    assert_eq!(max_bit, 95, "the highest family bit index in the file");

    for (id, family, bits) in [
        (116u32, 3u32, &[5u32][..]),
        (4987, 10, &[12]),
        (526, 11, &[35]),
        (133, 3, &[0, 67]),
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("spell {id}"));
        assert_eq!(d.spell_family, family, "{id} {:?} family", d.name);
        let set: Vec<u32> = (0..128)
            .filter(|b| d.spell_family_flags >> b & 1 == 1)
            .collect();
        assert_eq!(set, bits, "{id} {:?} family bits", d.name);
    }
}

/// The pet bar's GCD starts from the pressed spell's `StartRecoveryCategory`/`StartRecoveryTime`
/// pair, which every learnable pet ability carries as category 133, and its press route branches
/// on `AttributesEx4 & 0x20` (`SendPetAction 0x5d4210`), column 8.
#[test]
fn real_pet_spells_carry_the_gcd_pair_and_the_client_targeting_bit() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    // (id, name, startRecoveryCategory, startRecoveryTime): Claw's first rank and every Firebolt
    // rank on 1 s, the rest on 1.5 s.
    for (id, name, gcd_category, gcd_ms) in [
        (1082u32, "Claw", 133u32, 1000u32),
        (3010, "Claw", 133, 1500),
        (17258, "Bite", 133, 1500),
        (3110, "Firebolt", 133, 1000),
        (11763, "Firebolt", 133, 1000),
        (7814, "Lash of Pain", 133, 1500),
        (3716, "Torment", 133, 1500),
        (17735, "Suffering", 133, 1500),
    ] {
        let d = cat.get(id).unwrap_or_else(|| panic!("{name} {id}"));
        assert_eq!(d.name, name);
        assert_eq!(
            (d.start_recovery_category, d.start_recovery_ms),
            (gcd_category, gcd_ms),
            "{name} {id}"
        );
        assert!(
            !d.allows_client_targeting(),
            "{name} {id} is an ordinary press"
        );
    }
    // Lash of Pain's own timer is its category's, beside the GCD.
    let lash = cat.get(7814).expect("Lash of Pain");
    assert_eq!((lash.category, lash.category_recovery_ms), (73, 12_000));

    // The rows with the bit include Flamestrike and Rain of Fire, the ground-targeted casts.
    for id in [11829u32, 19474] {
        let d = cat.get(id).expect("a ground-targeted row");
        assert_eq!(d.attributes_ex4, 0x20, "{} {id}", d.name);
        assert!(d.allows_client_targeting());
    }
}

/// `TargetCreatureType` (column 17), read as the bind gate does: bit `type - 1` of the mask.
/// Beast is type 1, Dragonkin 2, Demon 3, Elemental 4, Undead 6, Humanoid 7.
#[test]
fn real_spell_catalog_reads_the_bind_gate_columns() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let cat = load_spell_catalog(&mut chain).expect("load Spell/SpellIcon");

    for (id, mask, admits, refuses) in [
        // Hibernate: Beast and Dragonkin.
        (2637u32, 0x3u32, [1u32, 2], [7u32, 6]),
        // Banish: Demon and Elemental.
        (710, 0xc, [3, 4], [1, 7]),
        // Shackle Undead: Undead alone.
        (9484, 0x20, [6, 6], [7, 1]),
        // Mind Control: Humanoid alone, so a player (Humanoid) is a candidate.
        (605, 0x40, [7, 7], [1, 6]),
    ] {
        let d = cat.get(id).expect("spell row");
        assert_eq!(d.target_creature_type, mask, "{} ({id})", d.name);
        for t in admits {
            assert!(d.admits_creature_type(t), "{} admits type {t}", d.name);
        }
        for t in refuses {
            assert!(!d.admits_creature_type(t), "{} refuses type {t}", d.name);
        }
    }
    // Fireball has no mask: any type, and a unit with none too.
    let fireball = cat.get(133).expect("Fireball");
    assert_eq!(fireball.target_creature_type, 0);
    assert!(fireball.admits_creature_type(0));

    // Bloodlust (24185, a frenzy that never takes its own caster) and Revive (24341, which
    // `AttributesEx2 & 1` lets reach a corpse): `AttributesEx` bit 19 and `AttributesEx2` bit 0.
    assert!(cat.get(24185).expect("Bloodlust").excludes_caster());
    assert!(!fireball.excludes_caster());
    assert!(cat.get(24341).expect("Revive").allows_dead_target());
    assert!(!fireball.allows_dead_target());
}

/// The mask rule alone: no mask admits everything, a masked spell refuses a unit of type 0, and
/// the bit is `type - 1`, wrapping at 32 like the hardware's shift.
#[test]
fn creature_type_mask_rule() {
    let masked = |mask| SpellDisplay {
        target_creature_type: mask,
        ..Default::default()
    };
    let open = masked(0);
    for t in [0u32, 1, 7, 32, 33, u32::MAX] {
        assert!(open.admits_creature_type(t), "no mask admits type {t}");
    }
    let beast = masked(1);
    assert!(beast.admits_creature_type(1));
    assert!(
        !beast.admits_creature_type(0),
        "type 0 is refused under a mask"
    );
    assert!(!beast.admits_creature_type(2));
    let top = masked(1 << 31);
    assert!(top.admits_creature_type(32), "type 32 is bit 31");
    assert!(
        !top.admits_creature_type(1),
        "type 1 is bit 0, not the wrapped bit 32"
    );
    assert!(masked(1).admits_creature_type(33), "bit 32 wraps to bit 0");
}

/// `0x7fe3e0`: the vision bit from `RequiredAuraVision`, stealth detection's `0x20` from any slot,
/// invisibility detection's `0x40` only for type 0 or 10, and nothing for a plain aura.
#[test]
fn aura_vision_mask_folds_the_column_and_the_detection_auras() {
    let with = |aura: u32, misc: i32| {
        let mut d = SpellDisplay::default();
        d.effect_apply_aura[2] = aura;
        d.effect_misc_value[2] = misc;
        d
    };
    assert_eq!(SpellDisplay::default().aura_vision_mask(), 0);
    assert_eq!(
        with(SPELL_AURA_MOD_STEALTH_DETECT, 0).aura_vision_mask(),
        0x20
    );
    assert_eq!(
        with(SPELL_AURA_MOD_INVISIBILITY_DETECT, 0).aura_vision_mask(),
        0x40
    );
    assert_eq!(
        with(SPELL_AURA_MOD_INVISIBILITY_DETECT, 10).aura_vision_mask(),
        0x40
    );
    assert_eq!(
        with(SPELL_AURA_MOD_INVISIBILITY_DETECT, 3).aura_vision_mask(),
        0
    );
    let mut d = with(SPELL_AURA_MOD_STEALTH_DETECT, 0);
    d.required_aura_vision = 1;
    assert_eq!(d.aura_vision_mask(), 0x21);
}
