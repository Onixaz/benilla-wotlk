//! The stock target-of-target frame, `TargetFrameToT` (`TargetofTargetFrameTemplate`,
//! `TargetFrame.xml:467-621`, made by `TargetFrame_CreateTargetofTarget`), over synthetic
//! `"targettarget"` snapshots. Nothing on it answers a unit event (`UnitFrame_OnEvent`,
//! `UnitFrame.lua:101-117`): its `OnUpdate`, `TargetofTarget_Update` (`TargetFrame.xml:619`), is
//! the only driver, so most steps here are a `tick`.

use benilla_ui::script::{
    AuraState, PartyMemberInfo, PartyState, QuadContent, RaidMemberInfo, ScriptValue,
    SelectionRequest, UiScript, UnitState,
};

/// A unit snapshot; the guid is what `UnitIsUnit` compares for the "target is you" gate.
fn unit(name: &str, guid: u64, health: u32) -> UnitState {
    UnitState {
        exists: true,
        is_connected: true,
        name: Some(name.into()),
        guid,
        health,
        max_health: 100,
        level: 60,
        power_type: 0,
        power: 50,
        max_power: 100,
        ..UnitState::default()
    }
}

/// The stock unit frames over a player, a target and a target's target: everything but the switch
/// already allows the frame.
fn load_tot() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_stock_ui(&mut s);
    // The pair's defaults, the `showTargetOfTarget` and `targetOfTargetMode` rows
    // (`InterfaceOptionsFrame.lua:318-319`).
    s.run(r#"SHOW_TARGET_OF_TARGET = "0" SHOW_TARGET_OF_TARGET_STATE = "5""#)
        .unwrap();
    s.set_unit("player", Some(unit("Tri", 0x100, 100)));
    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));
    s.set_unit("targettarget", Some(unit("Tri", 0x100, 100)));
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s
}

/// Turn the frame on as the option row does: the write, then its `applyFunc`.
fn switch_on(s: &mut UiScript) {
    s.run(r#"SHOW_TARGET_OF_TARGET = "1" TargetofTarget_Update(TargetFrameToT)"#)
        .unwrap();
}

fn shown(s: &mut UiScript) -> bool {
    s.eval::<bool>("return TargetFrameToT:IsShown() and true or false")
        .unwrap()
}

/// Every texture path the UI draws this frame.
fn drawn(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p),
            _ => None,
        })
        .collect()
}

fn debuff(spell_id: u32, name: &str, debuff_type: Option<&str>) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count: 1,
        debuff_type: debuff_type.map(str::to_string),
        // Only auras on the player carry a duration on the wire (vmangos `SpellAuras.cpp:7516`).
        duration: 0.0,
        expiration_time: 0.0,
        helpful: false,
        cancelable: false,
        until_cancelled: false,
        channeled: false,
        ..Default::default()
    }
}

/// A party of `n` others; `GetNumPartyMembers` is that list's length.
fn party(n: usize) -> PartyState {
    PartyState {
        members: (0..n)
            .map(|i| PartyMemberInfo {
                name: format!("Mate{i}"),
                guid: 0x300 + i as u64,
            })
            .collect(),
        ..PartyState::default()
    }
}

/// A raid of `n`, the player included, as `GetRaidRosterInfo` counts.
fn raid(n: usize) -> PartyState {
    PartyState {
        raid: (0..n)
            .map(|i| RaidMemberInfo {
                name: format!("Raider{i}"),
                guid: 0x400 + i as u64,
                ..RaidMemberInfo::default()
            })
            .collect(),
        ..PartyState::default()
    }
}

/// `SHOW_TARGET_OF_TARGET` defaults to `"0"` (`InterfaceOptionsFrame.lua:318`).
#[test]
fn the_frame_ships_off_and_the_switch_is_what_shows_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return SHOW_TARGET_OF_TARGET").unwrap(),
        "0",
        "the shipped default is the reference's: off"
    );
    assert!(
        !shown(&mut s),
        "off means hidden with everything else ready"
    );

    switch_on(&mut s);
    assert!(shown(&mut s), "the switch alone brings it up");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The four unit gates around the switch (`TargetFrame.lua:730`).
#[test]
fn the_four_unit_gates_each_take_the_frame_down() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    assert!(shown(&mut s));

    s.set_unit("targettarget", None);
    s.run("TargetofTarget_Update(TargetFrameToT)").unwrap();
    assert!(!shown(&mut s), "nothing to show");
    s.set_unit("targettarget", Some(unit("Tri", 0x100, 100)));

    s.set_unit("target", None);
    s.run("TargetofTarget_Update(TargetFrameToT)").unwrap();
    assert!(!shown(&mut s), "no target, no target's target");
    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));

    // The target is the player (same guid).
    s.set_unit("target", Some(unit("Tri", 0x100, 100)));
    s.run("TargetofTarget_Update(TargetFrameToT)").unwrap();
    assert!(!shown(&mut s), "self-target: nothing to add");
    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));

    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 0)));
    s.run("TargetofTarget_Update(TargetFrameToT)").unwrap();
    assert!(!shown(&mut s), "a corpse is fighting nobody");

    s.set_unit("target", Some(unit("Kobold Miner", 0x200, 80)));
    s.run("TargetofTarget_Update(TargetFrameToT)").unwrap();
    assert!(shown(&mut s), "and back");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Each mode against solo, party and raid (`TargetFrame.lua:731-735`).
#[test]
fn the_five_modes_answer_solo_party_and_raid() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);

    let states = [
        ("solo", PartyState::default()),
        ("party", party(2)),
        ("raid", raid(10)),
    ];
    // mode -> (solo, party, raid)
    let expected = [
        ("1", [false, false, true]), // raid only
        ("2", [false, true, false]), // party, and not while raiding
        ("3", [true, false, false]), // solo only
        ("4", [false, true, true]),  // grouped at all
        ("5", [true, true, true]),   // always
    ];
    for (mode, want) in expected {
        for (i, (label, state)) in states.iter().enumerate() {
            s.set_party(state.clone());
            s.run(&format!(
                r#"SHOW_TARGET_OF_TARGET_STATE = "{mode}" TargetofTarget_Update(TargetFrameToT)"#
            ))
            .unwrap();
            assert_eq!(
                shown(&mut s),
                want[i],
                "mode {mode} while {label}: expected shown={}",
                want[i]
            );
        }
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Mode 3 is solo only: the frame's `OnUpdate`, `TargetofTarget_Update`, hides it once a
/// raid forms (`TargetFrame.lua:727-760`).
#[test]
fn the_solo_mode_hides_the_frame_when_a_raid_forms() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.run(r#"SHOW_TARGET_OF_TARGET_STATE = "3" TargetofTarget_Update(TargetFrameToT)"#)
        .unwrap();
    assert!(shown(&mut s), "solo, in solo mode");

    s.set_party(raid(10));
    s.tick(0.016);
    assert!(
        !shown(&mut s),
        "the raid forms and the solo mode lets the frame go"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The frame repaints in its `OnUpdate`'s `UnitFrame_Update` (`TargetFrame.lua:748`), so it waits
/// for a tick. A powerless unit keeps a 0/0 rail: the template declares no numerals, and a bar
/// without a `TextString` never hides (`TextStatusBar.lua:31-33`).
#[test]
fn the_frame_paints_its_unit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.tick(0.016);

    assert_eq!(
        s.eval::<String>("return TargetFrameToTTextureFrameName:GetText()")
            .unwrap(),
        "Tri"
    );
    let (value, max) = s
        .eval::<(f64, f64)>(
            "local _, m = TargetFrameToTHealthBar:GetMinMaxValues() \
             return TargetFrameToTHealthBar:GetValue(), m",
        )
        .unwrap();
    assert_eq!((value, max), (100.0, 100.0));
    assert!(
        s.eval::<bool>("return TargetFrameToTManaBar:IsShown() and true or false")
            .unwrap(),
        "a unit with mana shows its rail"
    );

    let mut powerless = unit("Skeleton", 0x100, 100);
    powerless.max_power = 0;
    s.set_unit("targettarget", Some(powerless));
    s.tick(0.016);
    assert!(
        s.eval::<bool>("return TargetFrameToTManaBar:IsShown() and true or false")
            .unwrap(),
        "no power, and the rail is STILL drawn — the reference has no hide leg"
    );
    assert_eq!(
        s.eval::<(f64, f64)>(
            "local _, m = TargetFrameToTManaBar:GetMinMaxValues() \
             return TargetFrameToTManaBar:GetValue(), m",
        )
        .unwrap(),
        (0.0, 0.0),
        "it reads 0/0"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Zero health shows the dead text only while connected, which tells a corpse from a linkdead
/// player (`TargetofTarget_CheckDead`, `TargetFrame.lua:763-771`).
#[test]
fn the_dead_word_needs_a_connected_corpse() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.tick(0.016);
    assert!(
        !s.eval::<bool>("return TargetFrameToTTextureFrameDeadText:IsShown() and true or false")
            .unwrap(),
        "alive: no word"
    );

    let mut corpse = unit("Tri", 0x100, 0);
    corpse.dead = true;
    s.set_unit("targettarget", Some(corpse.clone()));
    s.tick(0.016);
    assert!(
        s.eval::<bool>("return TargetFrameToTTextureFrameDeadText:IsShown() and true or false")
            .unwrap(),
        "dead: the word"
    );

    let mut linkdead = corpse;
    linkdead.is_connected = false;
    s.set_unit("targettarget", Some(linkdead));
    s.tick(0.016);
    assert!(
        !s.eval::<bool>("return TargetFrameToTTextureFrameDeadText:IsShown() and true or false")
            .unwrap(),
        "disconnected, not dead"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The tint (`TargetofTargetHealthCheck`, `TargetFrame.lua:773-789`) runs off the health bar's
/// `OnValueChanged` (`TargetFrame.xml:556`) and only for players.
#[test]
fn the_portrait_tints_with_a_players_state_and_never_a_creatures() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.tick(0.016);

    let mut hurt = unit("Tri", 0x100, 15);
    hurt.is_player = true;
    s.set_unit("targettarget", Some(hurt));
    s.tick(0.016);
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return TargetFrameToTPortrait:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 0.0, 0.0), "a player under a fifth: red");

    let mut healthy = unit("Tri", 0x100, 90);
    healthy.is_player = true;
    s.set_unit("targettarget", Some(healthy));
    s.tick(0.016);
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return TargetFrameToTPortrait:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 1.0, 1.0), "and white again");

    // A creature at 15%: the check returns before touching the tint.
    s.set_unit("targettarget", Some(unit("Kobold", 0x500, 15)));
    s.tick(0.016);
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>("return TargetFrameToTPortrait:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 1.0, 1.0), "creatures carry no reading");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_left_click_targets_the_unit() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.run(r#"TargetFrameToT:Click("LeftButton")"#).unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::Unit("targettarget".into())]
    );

    // A right click opens no menu: `SecureUnitButton_OnLoad` gets no menu function here
    // (`TargetFrame.lua:720`).
    s.run(r#"TargetFrameToT:Click("RightButton")"#).unwrap();
    assert!(s.take_selection_requests().is_empty());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The row is `RefreshDebuffs` over `MAX_PARTY_DEBUFFS` (`UIParent.lua:3293-3346`), called from
/// `TargetofTarget_Update` (`TargetFrame.lua:751`). The `UNIT_AURA` fire is what the feed does,
/// but it is inert: `UnitFrame_OnEvent` ignores it, so the `tick` draws the row.
#[test]
fn the_debuff_row_draws_what_the_unit_carries() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    s.set_unit_guids(&unit_guids());
    s.set_unit_auras(
        TOT,
        Some(vec![
            debuff(1000, "Rend", None),
            debuff(1001, "Curse of Agony", Some("Curse")),
        ]),
    );
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("targettarget".into())]);
    s.tick(0.016);

    let paths = drawn(&mut s);
    assert!(
        paths.iter().any(|p| p == "Interface\\Icons\\Spell_1000"),
        "the first debuff's icon draws: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "Interface\\Icons\\Spell_1001"),
        "and the second"
    );
    assert!(
        !s.eval::<bool>("return TargetFrameToTDebuff3:IsShown() and true or false")
            .unwrap(),
        "the empty slots stay down"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// While this frame shows, the target's first two aura rows hold 101 px instead of 122
/// (`TargetFrame_UpdateAuras`, `TOT_AURA_ROW_WIDTH`), so five small debuffs fill a row and the
/// sixth wraps. The rows re-lay on the target's next aura update, and on this frame's `OnHide`
/// (`TargetofTarget_OnHide`).
#[test]
fn the_target_rows_wrap_short_while_the_frame_stands_beside_them() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    s.set_unit_guids(&unit_guids());
    let seven: Vec<AuraState> = (0..7)
        .map(|i| debuff(2000 + i, &format!("D{i}"), None))
        .collect();
    s.set_unit_auras(TARGET, Some(seven));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("target".into())]);

    let anchor = |s: &mut UiScript, frame: &str| -> String {
        s.eval::<String>(&format!(
            "local _, rel = {frame}:GetPoint(1) return rel:GetName()"
        ))
        .unwrap()
    };
    assert_eq!(
        anchor(&mut s, "TargetFrameDebuff6"),
        "TargetFrameDebuff5",
        "hidden: six fit in 122 px"
    );
    assert_eq!(anchor(&mut s, "TargetFrameDebuff7"), "TargetFrameDebuff1");

    switch_on(&mut s);
    assert!(shown(&mut s));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("target".into())]);
    assert_eq!(
        anchor(&mut s, "TargetFrameDebuff6"),
        "TargetFrameDebuff1",
        "shown: the sixth starts the second row"
    );

    // Back off: the frame's `OnHide` re-lays the rows full width.
    s.run(r#"SHOW_TARGET_OF_TARGET = "0" TargetofTarget_Update(TargetFrameToT)"#)
        .unwrap();
    assert!(!shown(&mut s));
    assert_eq!(anchor(&mut s, "TargetFrameDebuff6"), "TargetFrameDebuff5");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A token going away fires no event, so the frame's `OnUpdate`, `TargetofTarget_Update`,
/// reconciles it (`TargetFrame.lua:727-760`); it runs only while the target frame shows.
#[test]
fn the_reconcile_takes_the_frame_down_when_the_token_goes_silent() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_tot();
    switch_on(&mut s);
    assert!(shown(&mut s));

    // As the feed does: clear the token, fire nothing.
    s.set_unit("targettarget", None);
    assert!(shown(&mut s), "no event, so nothing has told the frame yet");

    s.tick(0.016);
    assert!(!shown(&mut s), "the reconcile notices within a frame");

    // A token appearing does fire, and the reconcile covers it too.
    s.set_unit("targettarget", Some(unit("Tri", 0x100, 100)));
    s.tick(0.016);
    assert!(shown(&mut s), "and brings it back");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

const TARGET: u64 = 0xF130_0000_0000_0001;
const TOT: u64 = 0xF130_0000_0000_0002;

/// The resolver's inputs: `"target"` is `TARGET`, held and targeting `TOT`, so `"targettarget"`
/// is `TOT`.
fn unit_guids() -> benilla_ui::script::UnitGuids {
    benilla_ui::script::UnitGuids {
        player: 1,
        target: TARGET,
        held: [(TARGET, TOT)].into_iter().collect(),
        ..Default::default()
    }
}
