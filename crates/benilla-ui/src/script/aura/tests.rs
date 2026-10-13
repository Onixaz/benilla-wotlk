use crate::script::{AuraState, SpellTooltipView, TrackingState, UiScript, UnitGuids};

const ME: u64 = 0x10;
const TARGET: u64 = 0xF130_0000_0000_0001;

/// A VM whose resolver names us as `player` and `TARGET` as `target`.
fn script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_unit_guids(&UnitGuids {
        player: ME,
        target: TARGET,
        ..Default::default()
    });
    s
}

fn aura(spell_id: u32, name: &str, helpful: bool, cancelable: bool) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count: 1,
        helpful,
        cancelable,
        ..Default::default()
    }
}

/// The icon a read answers, the third return, the list's identity.
fn icon_of(s: &UiScript, call: &str) -> Option<String> {
    s.eval::<Option<String>>(&format!("return select(3, {call})"))
        .unwrap()
}

/// `0x6147c0` pushes eleven values, in the order stock `BuffFrame.lua:125` and
/// `TargetFrame.lua:352` read them.
#[test]
fn unit_aura_returns_the_eleven_value_tuple() {
    let mut s = script();
    let mut fort = aura(1243, "Power Word: Fortitude", true, true);
    fort.rank = Some("Rank 1".into());
    fort.count = 1;
    fort.duration = 1800.0;
    fort.expiration_time = 1900.0;
    fort.caster = ME;
    fort.consolidate = true;
    s.set_player_auras(vec![fort]);

    assert_eq!(s.arity(r#"UnitAura("player", 1)"#).unwrap(), 11);
    let (name, rank, icon, count, dtype, duration, expiration): (
        String,
        String,
        String,
        i64,
        Option<String>,
        f64,
        f64,
    ) = s
        .eval(
            r#"local n, r, i, c, d, du, e = UnitAura("player", 1)
               return n, r, i, c, d, du, e"#,
        )
        .unwrap();
    assert_eq!(
        (name.as_str(), rank.as_str(), icon.as_str()),
        (
            "Power Word: Fortitude",
            "Rank 1",
            "Interface\\Icons\\Spell_1243"
        )
    );
    assert_eq!(
        (count, dtype, duration, expiration),
        (1, None, 1800.0, 1900.0)
    );
    let (caster, steal, consolidate, id): (Option<String>, Option<i64>, Option<i64>, i64) = s
        .eval(r#"return select(8, UnitAura("player", 1))"#)
        .unwrap();
    assert_eq!(caster.as_deref(), Some("player"), "the caster as a token");
    assert_eq!(steal, None, "a flag is 1 or nil");
    assert_eq!(consolidate, Some(1));
    assert_eq!(id, 1243);

    // The stock target frame's read, verbatim: the icon is the third value.
    assert!(s
        .eval::<bool>(
            r#"local name, rank, icon, count, debuffType, duration, expirationTime, caster,
                   isStealable = UnitBuff("player", 1)
               return icon == "Interface\\Icons\\Spell_1243" and count == 1"#
        )
        .unwrap());
}

/// An untimed aura answers 0 for both times, never nil, as `BuffFrame.lua`'s `duration > 0`
/// compares unguarded; a missing rank is the empty string.
#[test]
fn an_untimed_aura_answers_zero_times_and_an_empty_rank() {
    let mut s = script();
    let mut stance = aura(2457, "Battle Stance", true, false);
    stance.expiration_time = 50.0; // stale, with no duration: not shown
    s.set_player_auras(vec![stance]);
    let (rank, duration, expiration): (String, f64, f64) = s
        .eval(r#"local _, r, _, _, _, d, e = UnitAura("player", 1) return r, d, e"#)
        .unwrap();
    assert_eq!((rank.as_str(), duration, expiration), ("", 0.0, 0.0));
}

/// No filter means `HELPFUL` (`0x608419`); `HELPFUL` and `HARMFUL` each clear the other, so the
/// later wins; `UnitBuff` and `UnitDebuff` force their sign over the string.
#[test]
fn the_filter_defaults_to_helpful_and_the_verb_forces_the_sign() {
    let mut s = script();
    s.set_player_auras(vec![
        aura(2457, "Battle Stance", true, true),
        aura(589, "Shadow Word: Pain", false, false),
        aura(1126, "Mark of the Wild", true, true),
    ]);
    let id = |s: &UiScript, call: &str| -> Option<i64> {
        s.eval(&format!("return select(11, {call})")).unwrap()
    };
    assert_eq!(id(&s, r#"UnitAura("player", 1)"#), Some(2457));
    assert_eq!(id(&s, r#"UnitAura("player", 2)"#), Some(1126));
    assert_eq!(id(&s, r#"UnitAura("player", 3)"#), None);
    assert_eq!(id(&s, r#"UnitAura("player", 1, "HARMFUL")"#), Some(589));
    assert_eq!(id(&s, r#"UnitAura("player", 1, "harmful")"#), Some(589));
    assert_eq!(
        id(&s, r#"UnitAura("player", 1, "HELPFUL|HARMFUL")"#),
        Some(589)
    );
    assert_eq!(
        id(&s, r#"UnitAura("player", 1, "HARMFUL HELPFUL")"#),
        Some(2457)
    );
    assert_eq!(id(&s, r#"UnitBuff("player", 1, "HARMFUL")"#), Some(2457));
    assert_eq!(id(&s, r#"UnitDebuff("player", 1, "HELPFUL")"#), Some(589));
    assert_eq!(id(&s, r#"UnitDebuff("player", 2)"#), None);
    // An unknown token sets nothing.
    assert_eq!(id(&s, r#"UnitAura("player", 1, "BOGUS")"#), Some(2457));
    // Index 0 is `-1` with no name, which finds nothing (`0x72c9c2`).
    assert_eq!(s.arity(r#"UnitAura("player", 0)"#).unwrap(), 0);
    // A numeric string is an index, truncated.
    assert_eq!(id(&s, r#"UnitAura("player", "2.9")"#), Some(1126));
}

/// `CANCELABLE`/`NOT_CANCELABLE` test the record's flag, and `PLAYER` keeps the player's own.
#[test]
fn cancelable_and_player_filters_narrow_the_list() {
    let mut s = script();
    let mut mine = aura(1243, "Power Word: Fortitude", true, true);
    mine.caster = ME;
    let mut theirs = aura(1126, "Mark of the Wild", true, true);
    theirs.caster = TARGET;
    let fixed = aura(2457, "Battle Stance", true, false);
    s.set_player_auras(vec![theirs, fixed, mine]);
    let id = |s: &UiScript, call: &str| -> Option<i64> {
        s.eval(&format!("return select(11, {call})")).unwrap()
    };
    assert_eq!(id(&s, r#"UnitAura("player", 1, "PLAYER")"#), Some(1243));
    assert_eq!(id(&s, r#"UnitAura("player", 2, "PLAYER")"#), None);
    assert_eq!(
        id(&s, r#"UnitAura("player", 1, "NOT_CANCELABLE")"#),
        Some(2457)
    );
    assert_eq!(id(&s, r#"UnitAura("player", 2, "CANCELABLE")"#), Some(1243));
    assert_eq!(
        id(&s, r#"UnitAura("player", 1, "CANCELABLE|NOT_CANCELABLE")"#),
        None
    );
    // The caster token is the first naming the guid: our target here.
    assert_eq!(
        s.eval::<Option<String>>(r#"return select(8, UnitAura("player", 1))"#)
            .unwrap()
            .as_deref(),
        Some("target")
    );
    assert_eq!(
        s.eval::<Option<String>>(r#"return select(8, UnitAura("player", 2))"#)
            .unwrap(),
        None,
        "no caster known: nil"
    );
}

/// A string second argument is a spell name, matched case-insensitively, with an optional rank.
#[test]
fn a_name_and_rank_find_the_aura() {
    let mut s = script();
    let mut r1 = aura(1243, "Power Word: Fortitude", true, true);
    r1.rank = Some("Rank 1".into());
    let mut r2 = aura(1244, "Power Word: Fortitude", true, true);
    r2.rank = Some("Rank 2".into());
    s.set_unit_auras(TARGET, Some(vec![r1, r2]));
    let id = |s: &UiScript, call: &str| -> Option<i64> {
        s.eval(&format!("return select(11, {call})")).unwrap()
    };
    assert_eq!(
        id(&s, r#"UnitAura("target", "power word: fortitude")"#),
        Some(1243)
    );
    assert_eq!(
        id(
            &s,
            r#"UnitAura("target", "Power Word: Fortitude", "Rank 2")"#
        ),
        Some(1244)
    );
    assert_eq!(
        id(
            &s,
            r#"UnitAura("target", "Power Word: Fortitude", "Rank 3")"#
        ),
        None
    );
    assert_eq!(
        id(
            &s,
            r#"UnitAura("target", "Power Word: Fortitude", nil, "HARMFUL")"#
        ),
        None,
        "the filter follows the rank"
    );
    assert_eq!(
        id(&s, r#"UnitDebuff("target", "Power Word: Fortitude")"#),
        None
    );
}

/// A unit and an index or name are required: anything else raises the verb's usage line.
#[test]
fn a_bad_argument_list_raises_the_usage_line() {
    let s = script();
    for (call, verb) in [
        (r#"UnitAura("player")"#, "UnitAura"),
        (r#"UnitBuff(nil, 1)"#, "UnitBuff"),
        (r#"UnitDebuff("player", {})"#, "UnitDebuff"),
        (r#"CancelUnitBuff("player")"#, "CancelUnitBuff"),
    ] {
        let err = s.run(call).unwrap_err().to_string();
        assert!(
            err.contains(&format!(
                r#"Usage: {verb}("unit", [index] or ["name", "rank"][, "filter"])"#
            )),
            "{call}: {err}"
        );
    }
}

/// `CancelUnitBuff` cancels the player's own cancelable or channeled aura by spell id; another
/// unit's, or one neither flag allows, is a no-op.
#[test]
fn cancel_unit_buff_queues_the_players_cancelable_aura() {
    let mut s = script();
    let mut channel = aura(5143, "Arcane Missiles", false, false);
    channel.channeled = true;
    s.set_player_auras(vec![
        aura(2457, "Battle Stance", true, false),
        aura(1126, "Mark of the Wild", true, true),
        channel,
    ]);
    s.set_unit_auras(
        TARGET,
        Some(vec![aura(1126, "Mark of the Wild", true, true)]),
    );

    s.run(r#"CancelUnitBuff("player", 1)"#).unwrap();
    assert!(s.take_cancel_aura_requests().is_empty(), "not cancelable");
    s.run(r#"CancelUnitBuff("player", 2, "HELPFUL")"#).unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1126]);
    s.run(r#"CancelUnitBuff("player", 1, "HARMFUL")"#).unwrap();
    assert_eq!(
        s.take_cancel_aura_requests(),
        vec![5143],
        "a channel breaks"
    );
    s.run(r#"CancelUnitBuff("player", "Mark of the Wild")"#)
        .unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1126]);
    s.run(r#"CancelUnitBuff("target", 1)"#).unwrap();
    s.run(r#"CancelUnitBuff("player", 9)"#).unwrap();
    assert!(s.take_cancel_aura_requests().is_empty());
    assert_eq!(s.arity(r#"CancelUnitBuff("player", 2)"#).unwrap(), 0);
}

#[test]
fn tracking_bindings_read_the_pushed_state_and_cancel_by_spell_id() {
    let mut s = script();
    assert!(s
        .eval::<bool>("return GetTrackingTexture() == nil")
        .unwrap());
    s.eval::<()>("CancelTrackingBuff()").unwrap();
    assert!(s.take_cancel_aura_requests().is_empty());

    s.set_tracking(Some(TrackingState {
        spell_id: 2580,
        name: Some("Find Minerals".into()),
        icon: Some("Interface\\Icons\\Trade_Mining".into()),
        cancelable: true,
    }));
    assert_eq!(
        s.eval::<String>("return GetTrackingTexture()").unwrap(),
        "Interface\\Icons\\Trade_Mining"
    );
    s.eval::<()>("CancelTrackingBuff()").unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![2580]);

    s.set_tracking(Some(TrackingState {
        spell_id: 2580,
        cancelable: false,
        ..Default::default()
    }));
    s.eval::<()>("CancelTrackingBuff()").unwrap();
    assert!(s.take_cancel_aura_requests().is_empty());

    s.set_tracking(None);
    assert!(s
        .eval::<bool>("return GetTrackingTexture() == nil")
        .unwrap());
}

#[test]
fn an_emptied_player_list_answers_nil() {
    let mut s = script();
    s.set_player_auras(vec![aura(2457, "Stance", true, true)]);
    assert!(s
        .eval::<bool>(r#"return UnitBuff("player", 1) ~= nil"#)
        .unwrap());
    s.set_player_auras(Vec::new());
    assert!(s
        .eval::<bool>(r#"return UnitBuff("player", 1) == nil"#)
        .unwrap());
}

const MOB: u64 = 0xF130_0000_0000_0002;
const P1: u64 = 0x21;
const P1_TARGET: u64 = 0xF130_0000_0000_0003;

/// We target `TARGET`, which targets party1, who targets `P1_TARGET`, which targets us; the mouse
/// is over `MOB`. Each unit's list holds a buff and a debuff whose spell ids, and so icons, are
/// its own: `base + 1` and `base + 2`.
fn group_script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_unit_guids(&UnitGuids {
        player: ME,
        target: TARGET,
        mouseover: MOB,
        party: [P1, 0, 0, 0],
        held: [
            (ME, TARGET),
            (TARGET, P1),
            (P1, P1_TARGET),
            (MOB, 0),
            (P1_TARGET, ME),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    s.set_player_auras(vec![
        aura(2457, "Battle Stance", true, true),
        aura(1126, "Mark of the Wild", true, true),
        aura(11976, "Strike", false, false),
    ]);
    for (guid, base, name) in [
        (TARGET, 100, "on the target"),
        (MOB, 200, "on the mouseover"),
        (P1, 300, "on party1"),
        (P1_TARGET, 400, "on party1's target"),
    ] {
        s.set_unit_auras(
            guid,
            Some(vec![
                aura(base + 1, name, true, true),
                aura(base + 2, name, false, false),
            ]),
        );
    }
    s
}

/// Every token reads the list of the unit the resolver names: the mouseover, a group member's
/// target and any depth of `target` chain.
#[test]
fn a_token_reads_the_list_of_the_unit_it_names() {
    let s = group_script();
    for (token, base) in [
        ("mouseover", 200),
        ("party1target", 400),
        ("targettarget", 300),
        ("TargetTarget", 300),
        ("targettargettarget", 400),
        ("party1", 300),
        ("PARTY1", 300),
    ] {
        assert_eq!(
            icon_of(&s, &format!(r#"UnitBuff("{token}", 1)"#)),
            Some(format!("Interface\\Icons\\Spell_{}", base + 1)),
            "{token}"
        );
        assert_eq!(
            icon_of(&s, &format!(r#"UnitDebuff("{token}", 1)"#)),
            Some(format!("Interface\\Icons\\Spell_{}", base + 2)),
            "{token}"
        );
    }
    assert!(s
        .eval::<bool>(r#"return UnitBuff("party1target", 2) == nil"#)
        .unwrap());
}

/// A token naming the player, in any case or down a chain, reads the player's list.
#[test]
fn a_token_naming_the_player_reads_the_player_list() {
    let s = group_script();
    for token in [
        "player",
        "PLAYER",
        "Player",
        "targettargettargettarget",
        "party1TARGETtarget",
    ] {
        assert_eq!(
            icon_of(&s, &format!(r#"UnitBuff("{token}", 2)"#)).as_deref(),
            Some("Interface\\Icons\\Spell_1126"),
            "{token}"
        );
    }
}

/// A token naming nobody, recognised or not, answers nothing: `0x60abf0` raises for none.
#[test]
fn an_unknown_or_empty_token_is_nil() {
    let s = group_script();
    for call in [
        r#"UnitBuff("party2", 1)"#,
        r#"UnitDebuff("raid1target", 1)"#,
        r#"UnitBuff("mouseovertargettarget", 1)"#,
        r#"UnitBuff("playerfoo", 1)"#,
        r#"UnitBuff("pet", 1)"#,
        r#"UnitBuff("", 1)"#,
        r#"UnitBuff("bogus", 1)"#,
        r#"UnitDebuff("focus", 1)"#,
        r#"UnitDebuff("npctarget", 1)"#,
    ] {
        assert_eq!(s.arity(call).unwrap(), 0, "{call}");
    }
}

/// `GameTooltip:SetUnitAura` and its siblings read through the same finder (`0x625f00`), and
/// a timed aura appends the time-remaining line on any unit (`0x62600a`).
#[test]
fn set_unit_aura_resolves_the_token_and_the_filter() {
    let mut s = group_script();
    s.run(
        r#"local a = CreateFrame("Button", "TF1")
           a:SetPoint("CENTER", 0, 0) a:SetWidth(10) a:SetHeight(10)
           TT = CreateFrame("GameTooltip", "TT")
           TT:SetOwner(a, "ANCHOR_BOTTOMRIGHT")
           TT:SetUnitDebuff("MouseOver", 1)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return TTTextLeft1:GetText()").unwrap(),
        "on the mouseover"
    );
    s.run(r#"TT:SetUnitAura("player", 1, "HARMFUL")"#).unwrap();
    assert_eq!(
        s.eval::<String>("return TTTextLeft1:GetText()").unwrap(),
        "Strike"
    );
    // A miss leaves the plate as it was (`0x625fbe`).
    s.run(r#"TT:SetUnitBuff("bogus", 1)"#).unwrap();
    assert_eq!(
        s.eval::<String>("return TTTextLeft1:GetText()").unwrap(),
        "Strike"
    );
    let err = s
        .run(r#"TT:SetUnitAura("player")"#)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(r#"Usage: TT:SetUnitAura("unit", [index] or ["name", "rank"][, "filter"])"#),
        "{err}"
    );

    // A timed aura: 90 s left at `GetTime` 10 ceils to 2 minutes, which takes the `_P1` twin.
    s.set_spell_tooltip(
        1243,
        SpellTooltipView {
            name: "Power Word: Fortitude".into(),
            aura_description: "Stamina increased by 3.".into(),
            ..Default::default()
        },
    );
    s.tick(10.0);
    let mut timed = aura(1243, "Power Word: Fortitude", true, true);
    timed.duration = 1800.0;
    timed.expiration_time = 100.0;
    s.set_player_auras(vec![timed]);
    s.run(
        r#"SPELL_TIME_REMAINING_MIN = "<%d m>"
           SPELL_TIME_REMAINING_MIN_P1 = "<%d mm>"
           TT:SetUnitAura("player", 1)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(i64, String, String)>(
            "return TT:NumLines(), TTTextLeft2:GetText(), TTTextLeft3:GetText()"
        )
        .unwrap(),
        (3, "Stamina increased by 3.".into(), "<2 mm>".into())
    );
}
