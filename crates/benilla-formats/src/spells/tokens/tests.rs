use super::*;
use crate::spells::SpellDisplay;

/// Deliberately unlike the shipped wording, so a wrong key cannot pass by reading the same.
fn global(key: &str) -> Option<String> {
    Some(
        match key {
            "SPELL_DURATION_UNTIL_CANCELLED" => "<forever>",
            "INT_SPELL_DURATION_SEC" => "<%dsec>",
            "INT_SPELL_DURATION_MIN" => "<%dmin>",
            "INT_SPELL_DURATION_HOURS" => "<%dhour>",
            "INT_SPELL_DURATION_DAYS" => "<%ddays>",
            "SPELL_DURATION_MIN" => "<%.2fmin>",
            "SPELL_DURATION_SEC" => "<%.2fsec>",
            "INT_SPELL_POINTS_SPREAD_TEMPLATE" => "<%d..%d>",
            "SPELL_POINTS_SPREAD_TEMPLATE" => "<%.1f to %.1f>",
            _ => return None,
        }
        .into(),
    )
}

/// Every spell-data formatter key needed by a corpus walk.
fn corpus_global(key: &str) -> Option<String> {
    global(key).or_else(|| {
        if key.starts_with("INT_SPELL_DURATION_") {
            Some("%d".into())
        } else if key.starts_with("SPELL_DURATION_") {
            Some("%.1f".into())
        } else if key == "HOME_INN" {
            Some("<inn>".into())
        } else {
            None
        }
    })
}

/// `%d`, `%.Nf` and `%%` in order; the tests keep clear of the CRT's rounding ties.
fn printf(template: &str, args: &[TokenNumber]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut spec = String::new();
        while let Some(&n) = chars.peek() {
            chars.next();
            spec.push(n);
            if n.is_ascii_alphabetic() || n == '%' {
                break;
            }
        }
        match (spec.as_str(), args.next()) {
            ("d", Some(TokenNumber::Int(n))) => out.push_str(&n.to_string()),
            (f, Some(TokenNumber::Float(x))) if f.starts_with('.') => {
                let p: usize = f[1..f.len() - 1].parse().unwrap();
                out.push_str(&format!("{x:.p$}"));
            }
            (s, a) => panic!("printf: %{s} with {a:?}"),
        }
    }
    out
}

/// A caster's table: `(op, flat, pct)` rows through the three appliers' arithmetic.
struct Mods(Vec<(u8, i32, i32)>);

impl Mods {
    fn find(&self, op: u8) -> Option<(i32, i32)> {
        self.0
            .iter()
            .find(|m| m.0 == op)
            .map(|&(_, flat, pct)| (flat, pct))
    }
}

impl SpellMods for Mods {
    fn apply_int(&self, _: &SpellDisplay, op: u8, value: i32) -> i32 {
        self.find(op)
            .map_or(value, |(flat, pct)| (value + flat) * pct / 100)
    }
    fn apply_float(&self, _: &SpellDisplay, op: u8, value: f32) -> f32 {
        self.find(op).map_or(value, |(flat, pct)| {
            ((f64::from(value) + f64::from(flat)) * f64::from(pct) * f64::from(0.01f32)) as f32
        })
    }
    fn apply_soft(&self, _: &SpellDisplay, op: u8, value: f32) -> f32 {
        self.find(op)
            .map_or(value, |(flat, pct)| crate::soft_modify(value, flat, pct))
    }
}

/// A player who knows `known`, wears `auras`, has attack power 200, spell power 100 in every
/// school, level 80 and cast speed `speed`.
#[derive(Default)]
struct Player {
    known: Vec<u32>,
    auras: Vec<u32>,
    speed: f32,
}

impl Caster for Player {
    fn knows_spell(&self, spell: u32) -> bool {
        self.known.contains(&spell)
    }
    fn has_aura(&self, spell: u32) -> bool {
        self.auras.contains(&spell)
    }
    fn value(&self, value: PlayerValue) -> f32 {
        match value {
            PlayerValue::AttackPowerTotal => 200.0,
            PlayerValue::SchoolSpellPower(_) | PlayerValue::SpellPower => 100.0,
            PlayerValue::Level => 80.0,
            _ => 0.0,
        }
    }
    fn power_cost(&self, _: &SpellDisplay) -> i32 {
        35
    }
    fn power_cost_per_second(&self, _: &SpellDisplay) -> i32 {
        7
    }
    fn cast_speed(&self) -> f32 {
        self.speed
    }
}

static NOBODY: Player = Player {
    known: Vec::new(),
    auras: Vec::new(),
    speed: 1.0,
};

fn ctx<'a>(
    durations: &'a SpellDurationCatalog,
    radii: &'a SpellRadiusCatalog,
    lookup: &'a dyn Fn(u32) -> Option<&'a SpellDisplay>,
) -> TokenContext<'a> {
    TokenContext {
        home_area: &|| None,
        durations,
        radii,
        ranges: None,
        variables: None,
        skill: &|_| 0,
        lookup,
        mods: None,
        unmodified_points: false,
        gender: &|| 0,
        caster: Some(&NOBODY),
        global: &global,
        printf: &printf,
    }
}

fn none_lookup<'a>(_: u32) -> Option<&'a SpellDisplay> {
    None
}

/// A flat effect of `points` in slot 0 (base `points − 1`, one side).
fn points(points: i32) -> SpellDisplay {
    SpellDisplay {
        effect_base_points: [points - 1, 0, 0],
        effect_die_sides: [1, 0, 0],
        ..Default::default()
    }
}

/// An effect rolling `min..=max` in slot 0.
fn spread(min: i32, max: i32) -> SpellDisplay {
    SpellDisplay {
        effect_base_points: [min - 1, 0, 0],
        effect_die_sides: [max - min + 1, 0, 0],
        ..Default::default()
    }
}

/// `GetDurationString` asks `GetText(key, -1)`, whose plural index is 1: a `_P1` twin wins for any
/// count, and without one the bare key serves every count.
#[test]
fn the_duration_ladder_picks_its_unit_and_the_twin_wins_whatever_the_count() {
    let mut durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    for (idx, ms) in [
        (1, 30_000),
        (2, 60_000),
        (3, 3_600_000),
        (4, 7_200_000),
        (5, 172_800_000),
        (6, -1),
        (7, 0),
    ] {
        durations.insert_for_tests(idx, ms);
    }
    let twin = |key: &str| {
        (key == "INT_SPELL_DURATION_HOURS_P1")
            .then(|| "<%dhrs>".to_string())
            .or_else(|| global(key))
    };
    let c = ctx(&durations, &radii, &none_lookup);
    let with_twin = TokenContext {
        global: &twin,
        ..ctx(&durations, &radii, &none_lookup)
    };
    let d = |duration_index, c: &TokenContext| {
        substitute(
            "$d",
            &SpellDisplay {
                duration_index,
                ..Default::default()
            },
            c,
        )
    };
    assert_eq!(d(1, &c), "<30sec>");
    assert_eq!(d(2, &c), "<1min>");
    assert_eq!(d(3, &c), "<1hour>");
    assert_eq!(d(4, &c), "<2hour>");
    assert_eq!(d(3, &with_twin), "<1hrs>");
    assert_eq!(d(4, &with_twin), "<2hrs>");
    assert_eq!(d(5, &c), "<2days>");
    assert_eq!(d(6, &c), "<forever>");
    // No positive duration, and no row, which `0x800a70` reads as 0.
    assert_eq!(d(7, &c), "<forever>");
    assert_eq!(d(8, &c), "<forever>");
}

/// `GenderConditional`: the first form on gender 0, the second on anything else, the chosen
/// form trimmed of spaces; without a player the token fails.
#[test]
fn gender_takes_the_players_side_of_the_branch() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let d = SpellDisplay::default();
    let gender = std::cell::Cell::new(0u8);
    let c = TokenContext {
        gender: &|| gender.get(),
        ..ctx(&durations, &radii, &none_lookup)
    };

    assert_eq!(substitute("$ghis:her;", &d, &c), "his");
    gender.set(1);
    assert_eq!(substitute("$ghis:her;", &d, &c), "her");
    gender.set(2);
    assert_eq!(substitute("$ghis:her;", &d, &c), "her");
    assert_eq!(substitute("$GLad:LAss;", &d, &c), "LAss");
    gender.set(0);
    assert_eq!(substitute("$g his : her ;", &d, &c), "his");
    gender.set(1);
    assert_eq!(substitute("$g his : her ;", &d, &c), "her");
    // A selector ends the female form.
    assert_eq!(substitute("$gdude:lady:c; hi", &d, &c), "lady hi");
    gender.set(0);
    assert_eq!(substitute("$g:male;", &d, &c), "");
    gender.set(1);
    assert_eq!(substitute("$g:male;", &d, &c), "male");
    assert_eq!(substitute("$g female:;", &d, &c), "");
    assert_eq!(
        substitute("providing the mage and $ghis:her; allies", &d, &c),
        "providing the mage and her allies"
    );
    let nobody = TokenContext {
        caster: None,
        ..ctx(&durations, &radii, &none_lookup)
    };
    assert_eq!(substitute("$ghis:her;", &d, &nobody), "$his:her;");
}

#[test]
fn duration_modifier_updates_description_and_overtime_total() {
    let mut durations = SpellDurationCatalog::default();
    durations.insert_for_tests(1, 120_000);
    let radii = SpellRadiusCatalog::default();
    let mods = Mods(vec![(1, 0, 150)]);
    let c = TokenContext {
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let d = SpellDisplay {
        duration_index: 1,
        effect_amplitude: [3_000, 0, 0],
        ..points(3)
    };
    assert_eq!(
        substitute("Lasts $d; deals $o1 total.", &d, &c),
        "Lasts <3min>; deals 180 total."
    );
}

/// `TimeFractionBelow`: a duration a hundredth off its unit prints the float template.
#[test]
fn fractional_minutes_take_the_float_template() {
    let mut durations = SpellDurationCatalog::default();
    durations.insert_for_tests(1, 120_000);
    let radii = SpellRadiusCatalog::default();
    let spell = SpellDisplay {
        duration_index: 1,
        ..Default::default()
    };
    for (rank, expected) in [
        (0, "<2min>"),
        (1, "<2.20min>"),
        (2, "<2.40min>"),
        (3, "<2.60min>"),
        (4, "<2.80min>"),
        (5, "<3min>"),
    ] {
        let mods = Mods(vec![(1, 0, 100 + rank * 10)]);
        let ctx = TokenContext {
            mods: Some(&mods),
            ..ctx(&durations, &radii, &none_lookup)
        };
        assert_eq!(substitute("$d", &spell, &ctx), expected, "rank {rank}");
    }
}

/// `BasePoints + 1` to `BasePoints + DieSides`; a single value prints one number, negative
/// points print positive.
#[test]
fn s_token_bounds() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let mut d = SpellDisplay {
        effect_base_points: [13, 24, 0],
        effect_die_sides: [9, 1, 0],
        ..Default::default()
    };
    let c = ctx(&durations, &radii, &none_lookup);
    assert_eq!(
        substitute("causes $s1 Fire damage and $s2 more", &d, &c),
        "causes <14..22> Fire damage and 25 more"
    );
    d.effect_base_points = [-31, 0, 0];
    d.effect_die_sides = [1, 0, 0];
    assert_eq!(
        substitute("reduces armor by $s1", &d, &c),
        "reduces armor by 30"
    );
}

#[test]
fn scaled_effect_points_keep_fractional_seconds() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    for (n, expected) in [(100, "0.1"), (200, "0.2"), (500, "0.5")] {
        let d = points(n);
        assert_eq!(
            substitute("$/1000;S1 sec.", &d, &c),
            format!("{expected} sec.")
        );
        assert_eq!(
            substitute("$/1000;m1 sec.", &d, &c),
            format!("{expected} sec.")
        );
    }
    assert_eq!(substitute("$/1000;S1", &points(1000), &c), "1");
}

/// The scale is an integer: a decimal `$*` reads its integer part and `$*0.04;` is no scale.
#[test]
fn scaled_m_tokens_print_tenths_and_the_scale_is_an_integer() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    let d = points(326);
    assert_eq!(
        substitute("$/77;m1 to $/25;M1 additional Fire damage", &d, &c),
        "4.2 to 13.0 additional Fire damage"
    );
    assert_eq!(substitute("$*0.04;M1", &d, &c), "326");
    assert_eq!(substitute("$*2;M1", &d, &c), "652");
    assert_eq!(substitute("$/0;M1", &d, &c), "326", "a zero leaves 1.0");
    assert_eq!(
        substitute("$/2 x;M1", &d, &c),
        "163",
        "the digits stop at the space"
    );
    assert_eq!(substitute("$/2 M1", &d, &c), "$/2 M1", "no `;`, no token");
}

#[test]
fn a_fractional_single_value_truncates_under_s_and_keeps_tenths_under_capital_s() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    let d = points(3);
    assert_eq!(substitute("$/2;s1", &d, &c), "1");
    assert_eq!(
        substitute("$/2;o1", &d, &c),
        "0",
        "no duration spreads nothing"
    );
    assert_eq!(substitute("$/2;S1", &d, &c), "1.5");
    assert_eq!(substitute("$/2;m1 $/2;M1", &d, &c), "1.5 1.5");
}

#[test]
fn a_spread_picks_its_template_by_letter_and_wholeness() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    assert_eq!(substitute("$/2;s1", &spread(4, 6), &c), "<2..3>");
    assert_eq!(substitute("$/2;s1", &spread(3, 5), &c), "<1..3>");
    assert_eq!(substitute("$/2;S1", &spread(3, 5), &c), "<1.5 to 2.5>");
    assert_eq!(substitute("$/2;s1", &spread(4, 5), &c), "<2..3>");
    assert_eq!(substitute("$/2;S1", &spread(4, 5), &c), "<2.0 to 2.5>");
    assert_eq!(substitute("$/2;O1", &spread(4, 5), &c), "0");
}

/// `m` keys `$l` on its whole minimum, the others on the whole maximum, a fraction as 2.
#[test]
fn the_plural_keys_on_the_whole_bound_or_two() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    let plural = |text: &str, d: &SpellDisplay| substitute(text, d, &c);
    assert_eq!(plural("$m1 $lpoint:points;", &spread(1, 3)), "1 point");
    assert_eq!(
        plural("$s1 $lpoint:points;", &spread(1, 3)),
        "<1..3> points"
    );
    assert_eq!(plural("$M1 $lpoint:points;", &points(1)), "1 point");
    assert_eq!(plural("$/2;m1 $lpoint:points;", &points(2)), "1 point");
    assert_eq!(plural("$/4;S1 $lpoint:points;", &points(2)), "0.5 points");
    // Spaces are trimmed off the pick; without a `;` the selector consumes nothing.
    assert_eq!(plural("$M1 $l point : points ;", &points(1)), "1 point");
    assert_eq!(plural("$M1 $lpoint", &points(1)), "1 point");
}

/// `GetEffectModifierType` `0x7fd440`: School Damage rounds and takes op 0; a dummy effect does
/// neither; a zero maximum ceils to 1.
#[test]
fn a_rounding_effect_floors_and_ceils_its_modified_bounds() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let mods = Mods(vec![(0, 0, 110)]);
    let c = TokenContext {
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let school = SpellDisplay {
        effects: [2, 0, 0],
        ..spread(14, 22)
    };
    assert_eq!(substitute("$s1", &school, &c), "<15..25>");
    assert_eq!(substitute("$m1 $M1", &school, &c), "15 25");
    let zero = SpellDisplay {
        effects: [2, 0, 0],
        ..points(0)
    };
    assert_eq!(
        substitute("$s1", &zero, &ctx(&durations, &radii, &none_lookup)),
        "<0..1>"
    );
    let dummy = SpellDisplay {
        effects: [3, 0, 0],
        ..spread(14, 22)
    };
    assert_eq!(substitute("$s1", &dummy, &c), "<14..22>");
}

/// The jump tables `0x7fd498` and `0x7fd53c`, decoded.
#[test]
fn the_modifier_type_table_matches_the_reference() {
    for effect in 0..=160u32 {
        for aura in [0, 3, 8, 15, 20, 21, 24, 43, 53, 62, 63, 64, 89, 162, 200] {
            let expected = match effect {
                2 | 9 | 10 | 17 | 31 | 58 | 67 | 75 | 121 => (true, false, true),
                6 | 27 | 35 | 65 | 119 | 128 | 129 | 143 => match aura {
                    3 | 8 | 20 | 53 | 62 | 89 => (true, true, true),
                    15 | 43 => (true, false, true),
                    21 | 24 | 63 | 64 | 162 => (true, false, false),
                    _ => (false, false, false),
                },
                8 | 30 | 62 | 146 => (true, false, false),
                _ => (false, false, false),
            };
            assert_eq!(
                classify(effect, aura),
                expected,
                "effect {effect} aura {aura}"
            );
        }
    }
}

/// A periodic damage or heal aura takes op 22, a direct one op 0.
#[test]
fn a_periodic_aura_takes_op_22() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let mods = Mods(vec![(22, 0, 200)]);
    let c = TokenContext {
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let aura = |aura| SpellDisplay {
        effects: [6, 0, 0],
        effect_apply_aura: [aura, 0, 0],
        ..points(10)
    };
    assert_eq!(substitute("$s1", &aura(3), &c), "20");
    assert_eq!(substitute("$s1", &aura(8), &c), "20");
    assert_eq!(substitute("$s1", &aura(15), &c), "10");
}

/// Each slot takes its own op: 3, 12 and 23 (`0x7ff8a2`).
#[test]
fn each_slot_takes_its_own_op() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let mods = Mods(vec![(3, 0, 200), (12, 0, 300), (23, 0, 400)]);
    let c = TokenContext {
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let d = SpellDisplay {
        effect_base_points: [9, 9, 9],
        effect_die_sides: [1, 1, 1],
        ..Default::default()
    };
    assert_eq!(substitute("$s1 $s2 $s3", &d, &c), "20 30 40");
}

/// The soft applier's truncating steps land 269 × 1.01 × 1.33 on 361.34375 after the 1/128
/// quantizer, where the FPU applier would reach 361.3515625.
#[test]
fn effect_points_take_the_software_float_applier() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let mods = Mods(vec![(8, 0, 101), (2, 0, 133)]);
    let c = TokenContext {
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let d = SpellDisplay {
        effects: [6, 0, 0],
        effect_apply_aura: [10, 0, 0],
        ..points(269)
    };
    assert_eq!(substitute("$M1", &d, &c), "361.3");
}

#[test]
fn the_aura_tooltip_context_leaves_points_and_duration_unmodified() {
    let mut durations = SpellDurationCatalog::default();
    durations.insert_for_tests(1, 60_000);
    let radii = SpellRadiusCatalog::default();
    let mods = Mods(vec![(8, 0, 125), (1, 0, 200), (18, 10, 100)]);
    let modified = TokenContext {
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let aura = TokenContext {
        unmodified_points: true,
        ..modified
    };
    let d = SpellDisplay {
        duration_index: 1,
        proc_chance: 5,
        ..points(40)
    };
    assert_eq!(substitute("$s1 $d $h", &d, &modified), "50 <2min> 15");
    assert_eq!(substitute("$s1 $d $h", &d, &aura), "40 <1min> 15");
}

#[test]
fn a_base_level_at_or_below_zero_is_not_subtracted() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let d = SpellDisplay {
        base_level: -1i32 as u32,
        effect_real_points_per_level: [1.0, 0.0, 0.0],
        ..points(10)
    };
    let c = TokenContext {
        skill: &|_| 50,
        ..ctx(&durations, &radii, &none_lookup)
    };
    assert_eq!(substitute("$s1", &d, &c), "20");
}

/// A cross-spell token takes the expanded spell's level capped at its row's `maxLevel`; only a
/// level of 0 falls through to the referenced spell's own.
#[test]
fn a_cross_spell_token_caps_the_outer_level() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let per_level = |id: u32, max_level: u32| SpellDisplay {
        id,
        base_level: 1,
        max_level,
        effect_real_points_per_level: [1.0, 0.0, 0.0],
        ..points(10)
    };
    let (capped, uncapped) = (per_level(2, 10), per_level(3, 0));
    let player_level = SpellDisplay {
        attributes_ex2: 0x100_0000,
        ..per_level(4, 0)
    };
    let lookup = |id: u32| match id {
        2 => Some(&capped),
        3 => Some(&uncapped),
        4 => Some(&player_level),
        _ => None,
    };
    let outer = SpellDisplay {
        id: 1,
        ..Default::default()
    };
    let c = TokenContext {
        skill: &|id| if id == 1 { 150 } else { 0 },
        ..ctx(&durations, &radii, &lookup)
    };
    assert_eq!(substitute("$2s1 $3s1", &outer, &c), "19 39");
    // `AttributesExB` `0x1000000`: the player's level, 80.
    assert_eq!(substitute("$4s1", &outer, &c), "89");
    let c = TokenContext {
        skill: &|id| if id == 1 { 0 } else { 100 },
        ..ctx(&durations, &radii, &lookup)
    };
    assert_eq!(substitute("$2s1 $3s1", &outer, &c), "19 29");
}

/// Below `baseLevel` the duration's per-level term goes negative: no floor.
#[test]
fn the_duration_level_term_is_not_floored() {
    let mut durations = SpellDurationCatalog::default();
    durations.insert_row_for_tests(
        1,
        crate::SpellDuration {
            base_ms: 20_000,
            per_level_ms: 1_000,
            max_ms: 30_000,
        },
    );
    let radii = SpellRadiusCatalog::default();
    let d = SpellDisplay {
        base_level: 10,
        duration_index: 1,
        ..Default::default()
    };
    let skill = std::cell::Cell::new(0);
    let skill_of = |_| skill.get();
    let c = TokenContext {
        skill: &skill_of,
        ..ctx(&durations, &radii, &none_lookup)
    };
    skill.set(25);
    assert_eq!(substitute("$d", &d, &c), "<15sec>");
    skill.set(100);
    assert_eq!(substitute("$d", &d, &c), "<30sec>");
}

/// `AttributesExE` `0x200` counts one more tick in `$o`.
#[test]
fn overtime_duration_period_scale_plural() {
    let mut durations = SpellDurationCatalog::default();
    durations.insert_for_tests(1, 18_000);
    let radii = SpellRadiusCatalog::default();
    let d = SpellDisplay {
        duration_index: 1,
        effect_amplitude: [3000, 0, 0],
        ..points(3)
    };
    let c = ctx(&durations, &radii, &none_lookup);
    assert_eq!(
        substitute("Deals $o1 damage over $d, every $t1 sec.", &d, &c),
        "Deals 18 damage over <18sec>, every 3 sec."
    );
    let at_apply = SpellDisplay {
        attributes_ex5: 0x200,
        duration_index: 1,
        effect_amplitude: [3000, 0, 0],
        ..points(3)
    };
    assert_eq!(substitute("$o1", &at_apply, &c), "21");
    assert_eq!(
        substitute("Restores $/2;S1 health: $l point:points;.", &d, &c),
        "Restores 1.5 health: points."
    );
}

/// `AttributesExE` `0x2000` scales a periodic spell's `$d` and `$t` by cast speed, rounded to
/// nearest; `AttributesExC` `0x20000000` exempts it; `$o` stays unhasted.
#[test]
fn haste_scales_a_periodic_spells_duration_and_tick() {
    let mut durations = SpellDurationCatalog::default();
    durations.insert_for_tests(1, 18_000);
    let radii = SpellRadiusCatalog::default();
    let quick = Player {
        speed: 0.8,
        ..Default::default()
    };
    let c = TokenContext {
        caster: Some(&quick),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let periodic = |attributes_ex3| SpellDisplay {
        duration_index: 1,
        effect_amplitude: [3000, 0, 0],
        attributes_ex5: 0x2000,
        attributes_ex3,
        ..points(3)
    };
    let d = periodic(0);
    assert_eq!(substitute("$d $t1 $o1", &d, &c), "<14.40sec> 2.40 18");
    let exempt = periodic(0x2000_0000);
    assert_eq!(substitute("$d $t1", &exempt, &c), "<18sec> 3");
}

/// `$t`: ProcFlags bit 0 is five seconds; else the amplitude through op 19, whole seconds or
/// two decimals.
#[test]
fn t_is_the_modified_amplitude_in_seconds() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let mods = Mods(vec![(19, -1000, 100)]);
    let c = TokenContext {
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let tick = |amplitude, proc_flags| SpellDisplay {
        effect_amplitude: [amplitude, 0, 0],
        proc_flags,
        ..Default::default()
    };
    let bare = ctx(&durations, &radii, &none_lookup);
    assert_eq!(substitute("$t1", &tick(1500, 0), &bare), "1.50");
    assert_eq!(substitute("$t1", &tick(0, 0), &bare), "0");
    assert_eq!(substitute("$t1", &tick(3000, 0), &c), "2");
    assert_eq!(substitute("$t1", &tick(3000, 1), &c), "5");
}

/// The `$l` deviation in [`substitute`]: the plural keys on the `$t` just before it.
#[test]
fn the_plural_keys_on_the_nearest_token_a_deviation() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    let blizzard = SpellDisplay {
        effect_base_points: [24, 0, 0],
        effect_die_sides: [1, 0, 0],
        effect_amplitude: [1000, 0, 0],
        ..Default::default()
    };
    assert_eq!(
        substitute(
            "$s1 Frost damage every $t1 $lsecond:seconds;",
            &blizzard,
            &c
        ),
        "25 Frost damage every 1 second"
    );
    let multiplier = SpellDisplay {
        effect_base_points: [24, 0, 0],
        effect_die_sides: [1, 1, 0],
        damage_multiplier: [1.0, 1.5, 0.0],
        ..Default::default()
    };
    assert_eq!(
        substitute("$s1 then $F1 $lpoint:points;", &multiplier, &c),
        "25 then 1 point"
    );
    assert_eq!(
        substitute("$s2 then $f2 $lpoint:points;", &multiplier, &c),
        "1 then 1.5 points"
    );
}

/// The field arms read the referenced row at the slot digit and print `%d`.
#[test]
fn field_tokens_read_their_columns_and_plural_counts() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let rows = [
        SpellDisplay {
            id: 42,
            effect_points_per_combo_point: [0.0, -2.9, 0.0],
            damage_multiplier: [0.5, 1.3, -0.25],
            effect_misc_value: [135, -1, 0],
            max_affected_targets: 4,
            stack_amount: 5,
            max_target_level: 40,
            ..Default::default()
        },
        SpellDisplay {
            id: 43,
            effect_points_per_combo_point: [1.9, 0.0, 0.0],
            effect_misc_value: [0, 0, 1],
            max_affected_targets: 1,
            stack_amount: 1,
            max_target_level: 1,
            ..Default::default()
        },
        SpellDisplay {
            id: 44,
            max_affected_targets: 0x8000_0000,
            stack_amount: u32::MAX,
            max_target_level: u32::MAX - 1,
            ..Default::default()
        },
    ];
    let lookup = |id| rows.iter().find(|r| r.id == id);
    let c = ctx(&durations, &radii, &lookup);
    let empty = SpellDisplay::default();
    assert_eq!(substitute("$42b2", &empty, &c), "-2");
    assert_eq!(substitute("$42q1 $42Q2", &empty, &c), "135 -1");
    assert_eq!(substitute("$42i $42u $42v", &empty, &c), "4 5 40");
    assert_eq!(substitute("$42I $42U $42V", &empty, &c), "4 5 40");
    assert_eq!(substitute("$*100;42F1", &empty, &c), "50");
    assert_eq!(substitute("$42f2", &empty, &c), "1.3");
    assert_eq!(substitute("$*10;42F3", &empty, &c), "-3");
    assert_eq!(
        substitute("$44i $44u $44v", &empty, &c),
        "-2147483648 -1 -2"
    );
    for token in ["$43b1", "$43q3", "$43i", "$43u", "$43v"] {
        assert_eq!(
            substitute(&format!("{token} $lone:many;"), &empty, &c),
            "1 one",
            "{token}"
        );
    }
    // A slot digit past 3 reads slot 1.
    assert_eq!(substitute("$42q5", &empty, &c), "135");
}

#[test]
fn the_multiplier_tokens_multiply_in_double() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    let d = SpellDisplay {
        damage_multiplier: [0.23, 0.833_333_3, 0.0],
        ..Default::default()
    };
    assert_eq!(substitute("$*5;f1", &d, &c), "1.2");
    assert_eq!(substitute("$*3;F2", &d, &c), "2");
}

/// A token the parser cannot read leaves a `$` and the text after what it consumed: an unknown
/// letter keeps its letter, a missing cross-spell row drops its digits.
#[test]
fn unreadable_tokens_leave_a_dollar() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let other = points(100);
    let lookup = |id: u32| -> Option<&SpellDisplay> { (id == 1234).then_some(&other) };
    let d = SpellDisplay::default();
    let c = TokenContext {
        home_area: &|| Some("Goldshire"),
        ..ctx(&durations, &radii, &lookup)
    };
    assert_eq!(
        substitute("as strong as $1234s1 hits", &d, &c),
        "as strong as 100 hits"
    );
    assert_eq!(substitute("unknown $j", &d, &c), "unknown $j");
    assert_eq!(substitute("gone $99s1 ok", &d, &c), "gone $s1 ok");
    assert_eq!(substitute("tail $", &d, &c), "tail $");
    assert_eq!(substitute("coûte $é or $…!", &d, &c), "coûte $é or $…!");
    assert_eq!(
        substitute("Returns you to $z.", &d, &c),
        "Returns you to Goldshire."
    );
    let unbound = ctx(&durations, &radii, &lookup);
    assert_eq!(
        substitute("Returns you to $z.", &d, &unbound),
        "Returns you to $z."
    );
    let inn = TokenContext {
        global: &corpus_global,
        ..ctx(&durations, &radii, &lookup)
    };
    assert_eq!(
        substitute("Returns you to $z.", &d, &inn),
        "Returns you to <inn>."
    );
    // `$s0` has no slot: the token is `$s`, and the `0` stays.
    assert_eq!(substitute("$s0", &points(7), &c), "70");
}

/// `$c` and `$p` read the caster's power costs.
#[test]
fn power_cost_tokens_read_the_caster() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    let d = SpellDisplay::default();
    assert_eq!(substitute("$c then $p", &d, &c), "35 then 7");
}

/// `$r` prints whole yards through op 5 and keys `$l` on their ceiling; `$e` one decimal
/// through op 27; `$a` truncates the radius, applies op 6, and prints an integer.
#[test]
fn r_e_and_a_follow_their_arms() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::from_rows(
        [(
            7,
            crate::SpellRadius {
                radius: 8.5,
                per_level: 0.0,
                max: 8.5,
            },
        )]
        .into(),
    );
    let range = |max| crate::SpellRange {
        min: 0.0,
        max,
        flags: 0,
    };
    let ranges = SpellRangeCatalog::from_rows([(1, range(5.0)), (4, range(30.0))].into());
    let mods = Mods(vec![(5, 0, 110), (27, 0, 150), (6, 2, 100)]);
    let c = TokenContext {
        ranges: Some(&ranges),
        mods: Some(&mods),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let d = |range_index, radius| SpellDisplay {
        range_index,
        effect_radius_index: [radius, 0, 0],
        effect_multiple_value: [2.0, 0.0, 0.0],
        ..Default::default()
    };
    assert_eq!(substitute("$r", &d(4, 0), &c), "33");
    assert_eq!(substitute("$r", &d(0, 0), &c), "5", "index 0 reads row 1");
    assert_eq!(substitute("$r $lyard:yards;", &d(0, 0), &c), "5 yards");
    assert_eq!(substitute("$r", &d(9, 0), &c), "0", "no row");
    assert_eq!(substitute("$e1", &d(4, 0), &c), "3.0");
    assert_eq!(substitute("$a1", &d(4, 7), &c), "10");
    assert_eq!(substitute("$a1", &d(4, 3), &c), "0", "no row");
}

#[test]
fn a_bound_is_whole_within_a_thousandth() {
    assert_eq!(whole(3.0), (3, true));
    assert_eq!(whole(2.9995), (3, true));
    assert_eq!(whole(3.0005), (3, true));
    assert_eq!(whole(2.5), (2, false));
    assert_eq!(whole(0.0), (0, true));
}

/// `${…}` prints `%.0f`, or `%.Nf` after a `.N`, and keys `$l` on its rounded value; a failed
/// expression drops its text for a `$`.
#[test]
fn an_expression_prints_its_value() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let c = ctx(&durations, &radii, &none_lookup);
    // Refreshing Spring Water's shape: 42 mana a tick, five-second ticks, eighteen seconds.
    let drink = SpellDisplay {
        effect_base_points: [0, 41, 0],
        effect_die_sides: [1, 1, 0],
        ..Default::default()
    };
    assert_eq!(
        substitute("Restores ${$m2/5*18} mana.", &drink, &c),
        "Restores 151 mana."
    );
    assert_eq!(
        substitute("${$m2/5}.2 a second", &drink, &c),
        "8.40 a second"
    );
    assert_eq!(
        substitute("${$m2/5}.0", &drink, &c),
        "8.0",
        "`.0` is not a precision"
    );
    assert_eq!(substitute("${$AP*0.1+$m1}", &drink, &c), "21");
    assert_eq!(substitute("${$m1} $lpoint:points;", &drink, &c), "1 point");
    assert_eq!(
        substitute("${$m1*2} $lpoint:points;", &drink, &c),
        "2 points"
    );
    assert_eq!(substitute("a ${$s1*2} b", &drink, &c), "a $ b");
    // No `}`: a `$` for the `$`, and the text after it parsed as text.
    assert_eq!(substitute("a ${$m1*2 b", &drink, &c), "a ${1*2 b");
    let nobody = TokenContext {
        caster: None,
        ..ctx(&durations, &radii, &none_lookup)
    };
    assert_eq!(
        substitute("${$m2+3}", &drink, &nobody),
        "3",
        "no player reads 0"
    );
}

/// A cross-spell operand inside an expression reads that spell's row; a missing row fails it.
#[test]
fn an_expression_reads_a_referenced_spell() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let well = points(267);
    let lookup = |id: u32| (id == 7001).then_some(&well);
    let c = ctx(&durations, &radii, &lookup);
    let d = SpellDisplay::default();
    assert_eq!(substitute("${$7001m1*3}", &d, &c), "801");
    assert_eq!(substitute("${$7002m1*3}", &d, &c), "$");
}

/// `SpellDescriptionVariables.dbc`'s definitions, read before the text: Lightwell's shape, a
/// glyph multiplier inside the expression that uses it.
#[test]
fn description_variables_resolve_inside_and_outside_expressions() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let vars = SpellDescriptionVariablesCatalog::from_rows(
        [
            (5, "$mult=$?s900[${1.2}][${1}]".to_string()),
            (6, "$a=${2}.1\r\n$b=${$<a>*3}\r\n$junk=1".to_string()),
        ]
        .into(),
    );
    let well = points(267);
    let lookup = |id: u32| (id == 7001).then_some(&well);
    let glyphed = Player {
        known: vec![900],
        speed: 1.0,
        ..Default::default()
    };
    let d = SpellDisplay {
        description_variables_id: 5,
        ..Default::default()
    };
    let text = "restore ${$7001m1*3*$<mult>} health";
    let plain = TokenContext {
        variables: Some(&vars),
        ..ctx(&durations, &radii, &lookup)
    };
    assert_eq!(substitute(text, &d, &plain), "restore 801 health");
    let glyph = TokenContext {
        caster: Some(&glyphed),
        ..plain
    };
    assert_eq!(substitute(text, &d, &glyph), "restore 961 health");
    assert_eq!(substitute("x$<mult>x", &d, &glyph), "x1x");
    // A variable's text keeps its own precision; a later one reads an earlier one's value.
    let chained = SpellDisplay {
        description_variables_id: 6,
        ..Default::default()
    };
    assert_eq!(
        substitute("$<a> $<b> $<junk>", &chained, &plain),
        "2.0 6 $<junk>"
    );
    // Without the row, an expression reads the name as 0 and the token form stays.
    assert_eq!(
        substitute(text, &d, &ctx(&durations, &radii, &lookup)),
        "restore 0 health"
    );
    assert_eq!(
        substitute("$<mult>", &d, &ctx(&durations, &radii, &lookup)),
        "$<mult>"
    );
}

/// `$?` picks its arm by known spells and auras, with `!`, `&`, `|`, brackets and `]?` chains;
/// the chosen arm is parsed as text.
#[test]
fn a_conditional_picks_its_arm() {
    let durations = SpellDurationCatalog::default();
    let radii = SpellRadiusCatalog::default();
    let player = Player {
        known: vec![10, 11],
        auras: vec![20],
        speed: 1.0,
    };
    let c = TokenContext {
        caster: Some(&player),
        ..ctx(&durations, &radii, &none_lookup)
    };
    let d = points(5);
    let pick = |text: &str| substitute(text, &d, &c);
    assert_eq!(pick("$?s10[yes][no]"), "yes");
    assert_eq!(pick("$?s99[yes][no]"), "no");
    assert_eq!(pick("$?a20[on][off]."), "on.");
    assert_eq!(
        pick("$?a10[on][off]"),
        "off",
        "a known spell is not an aura"
    );
    assert_eq!(pick("$?!s99[not][is]"), "not");
    assert_eq!(pick("$?s10&s11[both][no]"), "both");
    assert_eq!(pick("$?s10&s99[both][no]"), "no");
    assert_eq!(pick("$?s99|a20[either][no]"), "either");
    assert_eq!(pick("$?(s99|s11)&a20[nested][no]"), "nested");
    assert_eq!(pick("$?!(s99|s98)[neither][no]"), "neither");
    assert_eq!(pick("$?s99[a]?s10[b][c]"), "b");
    assert_eq!(pick("$?s99[a]?s98[b][c] end"), "c end");
    assert_eq!(pick("$?s10[a]?s98[b][c] end"), "a end");
    assert_eq!(pick("$?s10[deals $s1][none]"), "deals 5");
    assert_eq!(pick("x$?s10 no brackets"), "xs10 no brackets");
    let nobody = TokenContext {
        caster: None,
        ..ctx(&durations, &radii, &none_lookup)
    };
    assert_eq!(substitute("$?s10[yes][no]", &d, &nobody), "no");
}

/// Every 3.3.5a `Spell.dbc` description and aura text expands with no `${`, `$<` or `$?` left.
#[test]
fn every_shipped_description_expands_its_3_3_5a_forms() {
    let data = crate::wow_data_or_skip!();
    let mut chain = crate::open_chain(&data).expect("open chain");
    let spells = crate::load_spell_catalog(&mut chain).expect("Spell.dbc");
    let durations = crate::load_spell_durations(&mut chain).expect("SpellDuration.dbc");
    let radii = crate::load_spell_radii(&mut chain).expect("SpellRadius.dbc");
    let ranges = crate::load_spell_ranges(&mut chain).expect("SpellRange.dbc");
    let vars =
        crate::load_spell_description_variables(&mut chain).expect("SpellDescriptionVariables");
    let lookup = |id| spells.get(id);
    let c = TokenContext {
        ranges: Some(&ranges),
        variables: Some(&vars),
        global: &corpus_global,
        ..ctx(&durations, &radii, &lookup)
    };
    let mut texts = 0;
    let mut forms = 0;
    let mut raw = Vec::new();
    for (id, spell) in spells.iter() {
        for text in [
            spell.description.as_deref(),
            spell.aura_description.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            texts += 1;
            forms += ["${", "$<", "$?"]
                .iter()
                .map(|f| text.matches(f).count())
                .sum::<usize>();
            let out = substitute(text, spell, &c);
            if ["${", "$<", "$?"].iter().any(|f| out.contains(f)) {
                raw.push((id, out));
            }
        }
    }
    assert!(
        texts > 10_000 && forms > 2_000,
        "{texts} texts, {forms} forms"
    );
    assert!(
        raw.is_empty(),
        "{} raw: {:?}",
        raw.len(),
        &raw[..raw.len().min(5)]
    );
}
