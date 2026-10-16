use std::collections::HashMap;

use super::*;
use crate::script::{GlueRequest, UiScript};

fn race(id: u8, file: &str, faction: &str, expansion: u8) -> GlueCreateRace {
    GlueCreateRace {
        id,
        flags: 0,
        required_expansion: expansion,
        name: file.into(),
        name_female: String::new(),
        name_male: String::new(),
        file: file.into(),
        faction_name: faction.into(),
        faction_internal: faction.into(),
    }
}

fn class(id: u8, file: &str, expansion: u8) -> GlueCreateClass {
    GlueCreateClass {
        id,
        required_expansion: expansion,
        name: file.into(),
        name_female: String::new(),
        name_male: String::new(),
        file: file.into(),
    }
}

/// Human and Gnome for the Alliance, Orc and Blood Elf for the Horde, a flagged row the screen
/// leaves out; Warrior, Mage, Death Knight.
fn catalog() -> GlueCreateCatalog {
    let mut flagged = race(9, "Goblin", "Horde", 0);
    flagged.flags = 0x1;
    let races = vec![
        race(1, "Human", "Alliance", 0),
        race(2, "Orc", "Horde", 0),
        race(7, "Gnome", "Alliance", 0),
        flagged,
        race(10, "BloodElf", "Horde", 1),
    ];
    let classes = vec![
        class(1, "WARRIOR", 0),
        class(6, "DEATHKNIGHT", 2),
        class(8, "MAGE", 0),
    ];
    let base_info = vec![
        (1, 1),
        (1, 6),
        (1, 8),
        (2, 1),
        (2, 6),
        (7, 8),
        (10, 6),
        (10, 8),
    ];
    let mut ranges = HashMap::new();
    let mut hair = HashMap::new();
    for r in [1, 2, 7, 10] {
        for sex in [0, 1] {
            ranges.insert((r, sex), [3, 2, 4, 5, 2]);
        }
        hair.insert(
            r,
            (
                "NORMAL".to_string(),
                ["HAIR".to_string(), "NONE".to_string()],
            ),
        );
    }
    let names = ["Aldous", "Bertram", "Cedric", "Damon", "Edwin"]
        .iter()
        .flat_map(|n| [(n.to_string(), 1, 0), (n.to_string(), 7, 1)])
        .collect();
    GlueCreateCatalog {
        races,
        classes,
        base_info,
        ranges,
        hair,
        names,
    }
}

/// A glue VM with the catalog seeded, an account of `expansion`, and the create frame set.
fn vm(expansion: u8) -> UiScript {
    let mut s = UiScript::new_glue().unwrap();
    s.set_account_expansion(expansion);
    s.set_glue_create_catalog(catalog());
    s.run("CreateFrame('ModelFFX', 'CharacterCreate') SetCharCustomizeFrame('CharacterCreate')")
        .unwrap();
    s
}

fn races(s: &UiScript) -> Vec<String> {
    s.eval::<Vec<String>>(
        "local t = {GetAvailableRaces()} local out = {} \
         for i = 2, #t, 3 do out[#out + 1] = t[i] .. ':' .. t[i + 1] end return out",
    )
    .unwrap()
}

/// `0x4e1c20`'s list: Alliance's races, then Horde's, each in table order, the flagged row left
/// out; a race the account's expansion does not reach answers 0.
#[test]
fn the_races_are_alliance_then_horde_and_gated_by_the_expansion() {
    let s = vm(0);
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(races(&s), ["Human:1", "Gnome:1", "Orc:1", "BloodElf:0"]);
    let s = vm(2);
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(races(&s), ["Human:1", "Gnome:1", "Orc:1", "BloodElf:1"]);
    assert_eq!(
        s.eval::<(String, String)>("return GetFactionForRace(3)")
            .unwrap(),
        ("Horde".into(), "Horde".into())
    );
    assert_eq!(
        s.eval::<(Option<String>, Option<String>)>("return GetFactionForRace(9)")
            .unwrap(),
        (None, None)
    );
}

/// Without the frame `ResetCharCustomize` builds nothing; with it, a race the account may play,
/// a class that race may be, and every getter agreeing.
#[test]
fn a_reset_builds_a_playable_character() {
    let s = UiScript::new_glue().unwrap();
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(s.eval::<f64>("return GetSelectedRace()").unwrap(), 0.0);
    for _ in 0..20 {
        let s = vm(0);
        s.run("ResetCharCustomize()").unwrap();
        let (index, file): (usize, String) = s
            .eval("local i = GetSelectedRace() return i, select(2, GetNameForRace())")
            .unwrap();
        assert_ne!(file, "BloodElf", "an expansion race on a classic account");
        assert_eq!(races(&s)[index - 1].split(':').next(), Some(file.as_str()));
        let (_, class_file, _) = s
            .eval::<(String, String, f64)>("return GetSelectedClass()")
            .unwrap();
        assert_ne!(class_file, "DEATHKNIGHT");
        let look = s.glue_create_look().unwrap();
        assert!(look.character.dials.iter().all(|&d| d < 5));
    }
}

/// A race switch keeps a class the race may be, takes a random one otherwise, and each (race,
/// sex) gets back the dials it last held.
#[test]
fn a_race_switch_keeps_or_replaces_the_class_and_remembers_the_dials() {
    let s = vm(2);
    s.run("ResetCharCustomize() SetSelectedRace(1) SetSelectedSex(2) SetSelectedClass(3)")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return select(2, GetSelectedClass())")
            .unwrap(),
        "MAGE"
    );
    s.run("CycleCharCustomization(4, 1)").unwrap();
    let human = s.glue_create_look().unwrap().character;
    // Gnome may be a mage: the class stays.
    s.run("SetSelectedRace(2)").unwrap();
    assert_eq!(s.glue_create_look().unwrap().character.race, 7);
    assert_eq!(
        s.eval::<String>("return select(2, GetSelectedClass())")
            .unwrap(),
        "MAGE"
    );
    // Orc may not: a random class of the orc's.
    s.run("SetSelectedRace(3)").unwrap();
    let file = s
        .eval::<String>("return select(2, GetSelectedClass())")
        .unwrap();
    assert!(file == "WARRIOR" || file == "DEATHKNIGHT", "{file}");
    // Back to Human: its dials as it left them.
    s.run("SetSelectedRace(1)").unwrap();
    assert_eq!(s.glue_create_look().unwrap().character, human);
}

/// `SetSelectedSex` takes Lua's sex numbers, 2 male and 3 female, and `GetSelectedSex` answers
/// them; any other number changes nothing.
#[test]
fn the_sex_is_lua_numbered() {
    let s = vm(0);
    s.run("ResetCharCustomize() SetSelectedSex(2)").unwrap();
    assert_eq!(s.eval::<f64>("return GetSelectedSex()").unwrap(), 2.0);
    s.run("SetSelectedSex(3)").unwrap();
    assert_eq!(s.eval::<f64>("return GetSelectedSex()").unwrap(), 3.0);
    assert_eq!(s.glue_create_look().unwrap().character.sex, 1);
    s.run("SetSelectedSex(0) SetSelectedSex(7)").unwrap();
    assert_eq!(s.eval::<f64>("return GetSelectedSex()").unwrap(), 3.0);
    assert_eq!(
        s.eval::<String>("return GetFacialHairCustomization()")
            .unwrap(),
        "NONE"
    );
}

/// A dial steps either way and wraps; delta 0 and an index off the five change nothing.
#[test]
fn a_dial_cycles_and_wraps() {
    let s = vm(0);
    s.run("ResetCharCustomize()").unwrap();
    let start = s.glue_create_look().unwrap().character.dials;
    s.run("CycleCharCustomization(1, 1) CycleCharCustomization(1, 1) CycleCharCustomization(1, 1)")
        .unwrap();
    assert_eq!(
        s.glue_create_look().unwrap().character.dials,
        start,
        "skin has three"
    );
    s.run(
        "CycleCharCustomization(2, -1) CycleCharCustomization(2, 0) CycleCharCustomization(9, 1)",
    )
    .unwrap();
    let after = s.glue_create_look().unwrap().character.dials;
    assert_eq!(after[1], (start[1] + 1) % 2);
    assert_eq!(
        (after[0], after[2], after[3], after[4]),
        (start[0], start[2], start[3], start[4])
    );
    let err = s.run("CycleCharCustomization(1)").unwrap_err().to_string();
    assert!(
        err.contains("Usage: CycleCharCustomization(index, delta)"),
        "{err}"
    );
}

/// `IsRaceClassValid` takes a race index and a `ChrClasses` row index: 1, nil, or nothing for a
/// row off the table.
#[test]
fn race_class_validity_reads_char_base_info() {
    let s = vm(0);
    s.run("ResetCharCustomize()").unwrap();
    assert_eq!(
        s.eval::<Option<f64>>("return IsRaceClassValid(1, 2)")
            .unwrap(),
        Some(1.0)
    );
    assert_eq!(
        s.eval::<Option<f64>>("return IsRaceClassValid(2, 1)")
            .unwrap(),
        None
    );
    assert_eq!(
        s.eval::<f64>("return select('#', IsRaceClassValid(1, 9))")
            .unwrap(),
        0.0
    );
}

/// `GetSelectedClass` answers the row's 1-based index and the class's roles off `s_classRoles`.
#[test]
fn the_selected_class_carries_its_roles() {
    let s = vm(2);
    s.run("ResetCharCustomize() SetSelectedRace(1) SetSelectedClass(2)")
        .unwrap();
    assert_eq!(
        s.eval::<(String, String, f64, bool, bool, bool)>("return GetSelectedClass()")
            .unwrap(),
        (
            "DEATHKNIGHT".into(),
            "DEATHKNIGHT".into(),
            2.0,
            true,
            false,
            true
        )
    );
    assert_eq!(
        s.eval::<String>("return GetCreateBackgroundModel()")
            .unwrap(),
        "DEATHKNIGHT"
    );
    // A gnome reads as the dwarf (race 3), which this catalog has no row for.
    s.run("SetSelectedRace(2)").unwrap();
    assert_eq!(
        s.eval::<String>("return GetCreateBackgroundModel()")
            .unwrap(),
        ""
    );
    s.run("SetSelectedRace(1)").unwrap();
    assert_eq!(
        s.eval::<String>("return GetCreateBackgroundModel()")
            .unwrap(),
        "Human"
    );
}

/// A name the client refuses opens the status dialog with OKAY and the token's text and sends
/// nothing; a good one is the request, with the character as built.
#[test]
fn create_checks_the_name_then_asks_the_host() {
    let mut s = vm(0);
    s.run(
        "CHAR_NAME_TOO_SHORT = 'Too short' \
         probe = CreateFrame('Frame') probe:RegisterEvent('OPEN_STATUS_DIALOG') \
         probe:SetScript('OnEvent', function(self, event, which, text) SHOWN = which .. ':' .. text end) \
         ResetCharCustomize() CreateCharacter('A')",
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return SHOWN").unwrap(), "OKAY:Too short");
    assert!(s.take_glue_requests().is_empty());
    s.run("CreateCharacter('Aldous')").unwrap();
    let look = s.glue_create_look().unwrap();
    let requests = s.take_glue_requests();
    match requests.as_slice() {
        [GlueRequest::CreateCharacter(c)] => {
            assert_eq!(c.name, "Aldous");
            assert_eq!(
                (c.race, c.class, c.sex),
                (look.character.race, look.class, look.character.sex)
            );
            assert_eq!(
                [c.skin, c.face, c.hair_style, c.hair_color, c.facial_hair],
                look.character.dials
            );
            assert_eq!(c.existing, None);
        }
        other => panic!("{other:?}"),
    }
}

/// The client's name check: letters only, 2 to 12 of them, no letter three times running.
#[test]
fn the_name_check() {
    assert_eq!(validate_character_name(""), Some("CHAR_NAME_NO_NAME"));
    assert_eq!(validate_character_name("A"), Some("CHAR_NAME_TOO_SHORT"));
    assert_eq!(
        validate_character_name("Abcdefghijklm"),
        Some("CHAR_NAME_TOO_LONG")
    );
    assert_eq!(
        validate_character_name("Ab1"),
        Some("CHAR_NAME_INVALID_CHARACTER")
    );
    assert_eq!(
        validate_character_name("Aaab"),
        Some("CHAR_NAME_THREE_CONSECUTIVE")
    );
    assert_eq!(validate_character_name("Aldous"), None);
}

/// `GetRandomName` builds from the race and sex's names a 4-to-10-letter capitalized name.
#[test]
fn a_random_name_comes_from_the_race_and_sex() {
    let s = vm(0);
    s.run("ResetCharCustomize() SetSelectedRace(1) SetSelectedSex(2)")
        .unwrap();
    for _ in 0..20 {
        let name = s.eval::<String>("return GetRandomName()").unwrap();
        assert!((4..=10).contains(&name.len()), "{name}");
        assert!(name.chars().next().unwrap().is_ascii_uppercase(), "{name}");
        assert!(validate_character_name(&name).is_none(), "{name}");
    }
}

/// The background goes to the frame's own `SetModel`, the file it holds skipped.
#[test]
fn the_background_skips_the_held_file() {
    let s = vm(0);
    s.run("SetCharCustomizeBackground('UI_Human.m2')").unwrap();
    let first = s.glue_scene().unwrap();
    assert!(first.create && first.path == "UI_Human.m2");
    s.run("SetCharCustomizeBackground('ui_human.M2')").unwrap();
    assert_eq!(s.glue_scene().unwrap().instance, first.instance);
    s.run("SetCharCustomizeBackground('UI_Orc.m2')").unwrap();
    assert!(s.glue_scene().unwrap().instance > first.instance);
}
