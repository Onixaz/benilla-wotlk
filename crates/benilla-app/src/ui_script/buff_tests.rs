//! The stock buff bar (`BuffFrame.xml`) under a stubbed aura feed. `BuffFrame_Update` walks
//! `UnitAura(PlayerFrame.unit, i, filter)` per filter and makes `BuffButton<i>` / `DebuffButton<i>`
//! on demand, so a button's id is its index within its filter.

use benilla_ui::script::{AuraState, ExtractedQuad, QuadContent, UiScript};

/// Our player's guid: `CancelUnitBuff` cancels only the player's own auras.
const PLAYER: u64 = 0x42;

fn harness() -> UiScript {
    let mut s = super::test_ui::stock_ui(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            guid: PLAYER,
            name: Some("Probesix".into()),
            level: 60,
            race: Some("Human".into()),
            race_file: Some("Human".into()),
            class: Some("Warrior".into()),
            class_file: Some("WARRIOR".into()),
            faction_group: Some("Alliance".into()),
            faction_group_localized: Some("Alliance".into()),
            ..Default::default()
        }),
    );
    s.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: PLAYER,
        ..Default::default()
    });
    // Timers on, as the Buffs option's `setFunc` applies them.
    s.run("SHOW_BUFF_DURATIONS = \"1\" BuffFrame_UpdatePositions()")
        .unwrap();
    s
}

/// An [`AuraState`]: `expiration_time` 0 is permanent, and the harness's `GetTime()` starts at 0.
#[allow(clippy::too_many_arguments)]
fn aura(
    spell_id: u32,
    name: &str,
    icon: &str,
    helpful: bool,
    count: u8,
    debuff_type: Option<&str>,
    expiration_time: f64,
    cancelable: bool,
) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(icon.into()),
        count,
        debuff_type: debuff_type.map(Into::into),
        duration: expiration_time,
        expiration_time,
        helpful,
        cancelable,
        until_cancelled: expiration_time == 0.0,
        channeled: false,
        ..Default::default()
    }
}

/// Push the list and fire `UNIT_AURA("player")`, as the app's feed does.
fn push(s: &mut UiScript, auras: Vec<AuraState>) {
    s.set_player_auras(auras);
    s.fire_event(
        "UNIT_AURA",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
}

/// One tick of the app's real order (OnUpdate → resolve → draw list), returning the quads.
fn frame(s: &mut UiScript, dt: f32) -> Vec<ExtractedQuad> {
    s.tick(dt);
    s.resolve();
    s.extract()
}

/// Shown, and false for a button never made.
fn shown(s: &UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!("return {name} ~= nil and {name}:IsShown() == 1"))
        .unwrap()
}

fn text(s: &UiScript, name: &str) -> String {
    s.eval::<String>(&format!("return tostring(({name}:GetText()) or \"\")"))
        .unwrap()
}

fn alpha(s: &UiScript, name: &str) -> f64 {
    s.eval::<f64>(&format!("return {name}:GetAlpha()")).unwrap()
}

/// The first texture quad whose path ends in `leaf`.
fn tex_quad<'a>(quads: &'a [ExtractedQuad], leaf: &str) -> Option<&'a ExtractedQuad> {
    quads.iter().find(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } => p.ends_with(leaf),
        _ => false,
    })
}

fn mark(expiry: f64) -> AuraState {
    aura(
        1126,
        "Mark of the Wild",
        "Interface\\Icons\\Spell_Nature_Regeneration",
        true,
        1,
        None,
        expiry,
        true,
    )
}

#[test]
fn buffs_and_debuffs_fill_their_own_buttons_with_counts_and_the_dispel_tint() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![
            mark(120.0),
            aura(
                589,
                "Shadow Word: Pain",
                "Interface\\Icons\\Spell_Shadow_ShadowWordPain",
                false,
                3,
                Some("Magic"),
                18.0,
                false,
            ),
        ],
    );
    let quads = frame(&mut s, 0.1);

    assert!(shown(&s, "BuffButton1"), "the first buff makes BuffButton1");
    assert!(!shown(&s, "BuffButton2"), "no second buff");
    assert!(
        shown(&s, "DebuffButton1"),
        "the first debuff makes DebuffButton1"
    );
    assert!(!shown(&s, "DebuffButton2"), "no second debuff");
    assert!(tex_quad(&quads, "Spell_Nature_Regeneration").is_some());
    assert!(tex_quad(&quads, "Spell_Shadow_ShadowWordPain").is_some());

    // `DebuffTypeColor["Magic"]` is 0.20, 0.60, 1.00 (`BuffFrame.lua`).
    let (r, g, b): (f64, f64, f64) = s
        .eval("return DebuffButton1Border:GetVertexColor()")
        .unwrap();
    assert!(
        (r - 0.20).abs() < 1e-3 && (g - 0.60).abs() < 1e-3 && (b - 1.00).abs() < 1e-3,
        "Magic tint, got {r} {g} {b}"
    );

    // The count shows only above 1.
    assert!(shown(&s, "DebuffButton1Count"));
    assert_eq!(text(&s, "DebuffButton1Count"), "3");
    assert!(!shown(&s, "BuffButton1Count"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn a_timed_aura_counts_down_and_a_permanent_one_shows_no_timer() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![
            aura(
                2457,
                "Battle Stance",
                "Interface\\Icons\\Ability_Warrior_OffensiveStance",
                true,
                1,
                None,
                0.0,
                false,
            ),
            // Timed, 120 s out: 119.9 s left after the tick, which rounds up to "2 m".
            mark(120.0),
        ],
    );
    frame(&mut s, 0.1);

    assert!(
        !shown(&s, "BuffButton1Duration"),
        "a permanent aura shows no timer"
    );
    assert!(shown(&s, "BuffButton2Duration"));
    assert_eq!(
        text(&s, "BuffButton2Duration"),
        "2 m",
        "120s remaining abbreviates to minutes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Under `BUFF_WARNING_TIME` (31 s) a button takes `BuffFrame.BuffAlphaValue`, the shared pulse;
/// a long aura stays at full alpha (`AuraButton_OnUpdate`).
#[test]
fn the_warning_flash_pulses_a_low_aura_but_leaves_a_long_one_solid() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![
            aura(
                1,
                "Long",
                "Interface\\Icons\\INV_Misc_QuestionMark",
                true,
                1,
                None,
                300.0,
                true,
            ),
            aura(
                2,
                "Short",
                "Interface\\Icons\\INV_Misc_QuestionMark",
                true,
                1,
                None,
                20.0,
                true,
            ),
        ],
    );

    let (mut short_min, mut long_min) = (2.0_f64, 2.0_f64);
    for _ in 0..20 {
        s.tick(0.1);
        short_min = short_min.min(alpha(&s, "BuffButton2"));
        long_min = long_min.min(alpha(&s, "BuffButton1"));
    }
    assert!(
        short_min < 0.9,
        "the 20s aura pulses toward BUFF_MIN_ALPHA (min seen {short_min})"
    );
    assert!(
        (long_min - 1.0).abs() < 1e-9,
        "the 5min aura stays solid (min seen {long_min})"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `BuffButton_OnClick` is `CancelUnitBuff(self.unit, self:GetID(), self.filter)`: the button's
/// index within its filter, so the second buff cancels the second helpful aura whatever sits
/// between; a stance (not cancelable) queues nothing.
#[test]
fn right_clicking_cancels_the_aura_under_the_cursor() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(
        &mut s,
        vec![
            mark(120.0),
            aura(
                589,
                "Shadow Word: Pain",
                "Interface\\Icons\\B",
                false,
                3,
                Some("Magic"),
                18.0,
                false,
            ),
            aura(
                1459,
                "Arcane Intellect",
                "Interface\\Icons\\C",
                true,
                1,
                None,
                600.0,
                true,
            ),
            aura(
                2457,
                "Battle Stance",
                "Interface\\Icons\\E",
                true,
                1,
                None,
                0.0,
                false,
            ),
        ],
    );
    frame(&mut s, 0.1);
    assert!(s.take_cancel_aura_requests().is_empty());

    s.run("BuffButton_OnClick(BuffButton2)").unwrap();
    assert_eq!(
        s.take_cancel_aura_requests(),
        vec![1459],
        "the second helpful aura, past the debuff between"
    );
    s.run("BuffButton_OnClick(BuffButton3)").unwrap();
    assert!(
        s.take_cancel_aura_requests().is_empty(),
        "a stance is not cancelable"
    );
    s.run("BuffButton_OnClick(BuffButton1)").unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1126]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn an_emptied_bar_hides_every_button() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, vec![mark(120.0)]);
    frame(&mut s, 0.1);
    assert!(shown(&s, "BuffButton1"), "shown while the aura is up");

    push(&mut s, vec![]);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "BuffButton1"), "hidden once the aura is gone");
    assert!(!shown(&s, "BuffButton1Duration"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The countdown runs on the button's own `timeLeft`, which only `UNIT_AURA` resets
/// (`AuraButton_Update`): a refresh reaches the bar with its event.
#[test]
fn a_refreshed_duration_reaches_the_bar_with_its_aura_event() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // `AuraButton_OnUpdate` writes the duration before it takes the frame's elapsed off, so the
    // first tick still reads the full 20 s.
    push(&mut s, vec![mark(20.0)]);
    frame(&mut s, 0.1);
    assert_eq!(text(&s, "BuffButton1Duration"), "20 s");

    push(&mut s, vec![mark(300.0)]);
    frame(&mut s, 0.1);
    assert_eq!(text(&s, "BuffButton1Duration"), "5 m");
    assert!(
        (alpha(&s, "BuffButton1") - 1.0).abs() < 1e-6,
        "no longer inside the 31s warning window, so full alpha"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `SHOW_BUFF_DURATIONS` "0" hides the timers and closes each row's 15px gutter to 5px
/// (`BuffFrame_UpdatePositions`); the warning pulse runs regardless of it.
#[test]
fn the_duration_switch_hides_the_timers_and_closes_their_gutter() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // Nine buffs, so the ninth opens a second row.
    let nine: Vec<AuraState> = (0..9)
        .map(|i| {
            aura(
                100 + i,
                &format!("B{i}"),
                "Interface\\Icons\\INV_Misc_QuestionMark",
                true,
                1,
                None,
                20.0,
                true,
            )
        })
        .collect();
    push(&mut s, nine.clone());
    frame(&mut s, 0.1);
    assert!(shown(&s, "BuffButton1Duration"));
    assert_eq!(text(&s, "BuffButton1Duration"), "20 s");
    let gap = |s: &mut UiScript| {
        s.resolve();
        s.eval::<f64>("return ConsolidatedBuffs:GetBottom() - BuffButton9:GetTop()")
            .unwrap()
    };
    assert!((gap(&mut s) - 15.0).abs() < 1e-3, "gutter: {}", gap(&mut s));

    s.run("SHOW_BUFF_DURATIONS = \"0\" BuffFrame_UpdatePositions()")
        .unwrap();
    frame(&mut s, 0.1);
    assert!(!shown(&s, "BuffButton1Duration"), "no timer drawn");
    assert!(shown(&s, "BuffButton1"), "the icon stays");
    assert!((gap(&mut s) - 5.0).abs() < 1e-3, "gutter: {}", gap(&mut s));
    assert!(
        alpha(&s, "BuffButton1") < 1.0,
        "the last-31s pulse is not gated on the timers"
    );

    s.run("SHOW_BUFF_DURATIONS = \"1\" BuffFrame_UpdatePositions()")
        .unwrap();
    frame(&mut s, 0.1);
    assert!(shown(&s, "BuffButton1Duration"));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The temporary-enchant row (`TemporaryEnchantFrame_OnUpdate`): idle, it hides both slots; an
/// enchanted weapon takes slot 1 with its inventory slot id, counts down from milliseconds and
/// moves the first buff left of the row.
#[test]
fn the_temporary_enchant_row_shows_a_weapon_enchant_and_moves_the_buffs_aside() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, vec![mark(120.0)]);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "TempEnchant1"));
    assert!(!shown(&s, "TempEnchant2"));
    s.resolve();
    let resting = s.eval::<f64>("return BuffButton1:GetRight()").unwrap();

    s.set_weapon_enchants(
        Some(benilla_ui::script::WeaponEnchant {
            remaining_ms: Some(480_000),
            charges: 0,
        }),
        None,
    );
    frame(&mut s, 0.1);
    assert!(
        shown(&s, "TempEnchant1"),
        "the enchanted weapon takes slot 1"
    );
    assert!(!shown(&s, "TempEnchant2"));
    assert_eq!(
        s.eval::<i64>("return TempEnchant1:GetID()").unwrap(),
        16,
        "the MainHandSlot id, which its item tooltip hover needs"
    );
    assert_eq!(
        text(&s, "TempEnchant1Duration"),
        "8 m",
        "480000 ms / 1000 -> 480 s -> SecondsToTimeAbbrev"
    );
    s.resolve();
    let shifted = s.eval::<f64>("return BuffButton1:GetRight()").unwrap();
    assert!(
        (resting - shifted - 37.0).abs() < 1e-3,
        "one 32px enchant column and the 5px gutter: {resting} -> {shifted}"
    );

    // Both hands: off hand in slot 1, main hand in slot 2.
    s.set_weapon_enchants(
        Some(benilla_ui::script::WeaponEnchant {
            remaining_ms: Some(480_000),
            charges: 0,
        }),
        Some(benilla_ui::script::WeaponEnchant {
            remaining_ms: Some(120_000),
            charges: 0,
        }),
    );
    frame(&mut s, 0.1);
    assert_eq!(s.eval::<i64>("return TempEnchant1:GetID()").unwrap(), 17);
    assert_eq!(s.eval::<i64>("return TempEnchant2:GetID()").unwrap(), 16);
    assert_eq!(text(&s, "TempEnchant1Duration"), "2 m");
    assert_eq!(text(&s, "TempEnchant2Duration"), "8 m");
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return TemporaryEnchantFrame:GetWidth()")
            .unwrap(),
        64.0,
        "two 32px columns"
    );

    s.set_weapon_enchants(None, None);
    frame(&mut s, 0.1);
    assert!(!shown(&s, "TempEnchant1") && !shown(&s, "TempEnchant2"));
    s.resolve();
    assert!((s.eval::<f64>("return BuffButton1:GetRight()").unwrap() - resting).abs() < 1e-3);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `AuraButton_OnUpdate` writes the alpha and the duration every tick, so sentinels are
/// overwritten; under a minute the number turns `HIGHLIGHT_FONT_COLOR` white.
#[test]
fn a_settled_buff_button_rewrites_alpha_and_duration_every_tick() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    push(&mut s, vec![mark(300.0)]);
    frame(&mut s, 0.1);
    frame(&mut s, 0.1);
    assert_eq!(text(&s, "BuffButton1Duration"), "5 m");

    s.run(r#"BuffButton1:SetAlpha(0.42) BuffButton1Duration:SetText("X")"#)
        .unwrap();
    for _ in 0..10 {
        frame(&mut s, 0.016);
    }
    assert!((alpha(&s, "BuffButton1") - 1.0).abs() < 1e-6);
    assert_eq!(text(&s, "BuffButton1Duration"), "5 m");

    // The duration trails `timeLeft` by a frame: the minute shows on the tick after.
    s.tick(60.0);
    s.tick(0.016);
    assert_eq!(text(&s, "BuffButton1Duration"), "4 m");

    s.tick(210.0); // ~29.5 s left
    let a1 = alpha(&s, "BuffButton1");
    s.tick(0.2);
    let a2 = alpha(&s, "BuffButton1");
    assert!(a1 != a2, "the warning pulse flows per tick: {a1} {a2}");
    s.tick(0.016);
    let seconds_text = text(&s, "BuffButton1Duration");
    assert!(seconds_text.ends_with(" s"), "{seconds_text}");
    let (r, g, b): (f64, f64, f64) = s.eval("return BuffButton1Duration:GetTextColor()").unwrap();
    assert!(
        (r - 1.0).abs() < 1e-3 && (g - 1.0).abs() < 1e-3 && (b - 1.0).abs() < 1e-3,
        "the sub-minute band is HIGHLIGHT white: {r} {g} {b}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The aura tooltip's duration line over the install's own `GlobalStrings.lua`, formatted by
/// `tooltip::duration_text` on the reference's ladder.
#[test]
fn the_duration_line_reads_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    s.set_spell_tooltip(
        1459,
        benilla_ui::script::SpellTooltipView {
            name: "Arcane Intellect".into(),
            aura_description: "Intellect increased by 2.".into(),
            ..Default::default()
        },
    );
    s.tick(10.0); // GetTime = 10
    s.run(
        r#"
        BENILLA_ANCHOR = CreateFrame("Button", "BF9")
        BENILLA_ANCHOR:SetPoint("CENTER", 0, 0); BENILLA_ANCHOR:SetWidth(10); BENILLA_ANCHOR:SetHeight(10)
        BENILLA_TIP = CreateFrame("GameTooltip", "TT9")
    "#,
    )
    .unwrap();

    // `secs_left` seconds remaining at GetTime = 10, read back as the tooltip's last line.
    let mut line = |secs_left: f64| -> String {
        s.set_player_auras(vec![AuraState {
            spell_id: 1459,
            name: Some("Arcane Intellect".into()),
            duration: 86_400.0,
            expiration_time: 10.0 + secs_left,
            helpful: true,
            ..Default::default()
        }]);
        s.run(
            r#"
            BENILLA_TIP:SetOwner(BENILLA_ANCHOR, "ANCHOR_RIGHT")
            BENILLA_TIP:SetPlayerBuff(0)
            BENILLA_LAST = TT9TextLeft3:GetText()
        "#,
        )
        .unwrap();
        s.eval::<String>("return BENILLA_LAST").unwrap()
    };

    assert_eq!(line(7_200.0), "2 hours remaining", "was '120 minutes'");
    assert_eq!(line(3_600.0), "1 hour remaining", "the hour edge, singular");
    assert_eq!(
        line(3_599.999),
        "60 minutes remaining",
        "one ms under the hour stays in minutes — no '1 hour' until it is whole"
    );
    assert_eq!(line(60.0), "1 minute remaining", "was '1 minutes'");
    assert_eq!(line(61.0), "2 minutes remaining", "the minute arm ceils");
    // The seconds arm truncates where every arm above it ceils.
    assert_eq!(line(5.4), "5 seconds remaining", "was '6 seconds'");
    assert_eq!(line(1.0), "1 second remaining", "singular at exactly one");
    assert_eq!(
        line(-0.4),
        "0 seconds remaining",
        "the lapsing read, plural at zero"
    );
    assert_eq!(line(129_600.0), "2 days remaining", "was '2160 minutes'");

    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}
