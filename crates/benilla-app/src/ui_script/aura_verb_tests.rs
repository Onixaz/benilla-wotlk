//! The stock 3.3.5a buff frame (`BuffFrame.lua:125` `AuraButton_Update`) and target frame
//! (`TargetFrame.lua:343` `TargetFrame_UpdateAuras`) over a pushed aura feed: the 12340 verbs
//! `UnitAura`, `UnitBuff` and `UnitDebuff` they call, and `CancelUnitBuff` behind a buff's
//! right-click.

use benilla_ui::script::{AuraState, ScriptValue, UiScript, UnitGuids, UnitState};

const ME: u64 = 0x10;
const MOB: u64 = 0xF130_0000_0000_0001;

/// The unit frames and the buff frame, in production order. Load errors are collected, not
/// asserted per file: the files around these frames carry unrelated 3.3.5a gaps, and only the
/// aura path's own errors are under test.
fn harness() -> (UiScript, Vec<String>) {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let mut errors = Vec::new();
    s.register_cvars(crate::cvars::registered_pairs());
    for entry in super::test_ui::production_order(&[&[
        r"Interface\FrameXML\GlobalStrings.lua",
        r"Interface\FrameXML\Fonts.xml",
        r"Interface\FrameXML\BasicControls.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\SecureTemplates.lua",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\GameTooltip.xml",
        r"Interface\FrameXML\UIDropDownMenu.xml",
        r"Interface\FrameXML\TextStatusBar.lua",
        r"Interface\FrameXML\TextStatusBar.xml",
        r"Interface\FrameXML\Cooldown.xml",
        r"Interface\FrameXML\BuffFrame.xml",
        r"Interface\FrameXML\CombatFeedback.xml",
        r"Interface\FrameXML\CastingBarFrame.xml",
        r"Interface\FrameXML\UnitPopup.xml",
        r"Interface\FrameXML\UnitFrame.xml",
        r"Interface\FrameXML\PlayerFrame.xml",
        r"Interface\FrameXML\TargetFrame.xml",
        r"Interface\FrameXML\ReputationFrame.lua",
    ]]) {
        let path = entry.replace('\\', "/");
        let bytes = super::test_ui::read(&path).unwrap_or_else(|| panic!("{entry}: not found"));
        if path.to_ascii_lowercase().ends_with(".lua") {
            if let Err(e) = s.run_chunk_named(&bytes, &format!("@{entry}")) {
                errors.push(e.to_string());
            }
            continue;
        }
        let doc = benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes))
            .unwrap_or_else(|e| panic!("{entry}: {e}"));
        let provider = |req: &str| super::test_ui::read(req);
        errors.extend(benilla_ui::loader::load_in(&s, &doc, &path, &provider).errors);
    }
    errors.extend(s.take_errors());
    s.set_unit_guids(&UnitGuids {
        player: ME,
        ..Default::default()
    });
    (s, errors)
}

fn aura(spell_id: u32, name: &str, icon: &str, helpful: bool) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        rank: Some("Rank 1".into()),
        icon: Some(format!("Interface\\Icons\\{icon}")),
        count: 1,
        helpful,
        cancelable: helpful,
        ..Default::default()
    }
}

/// Errors naming the aura verbs or the two frames' aura code.
fn aura_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| {
            [
                "UnitAura",
                "UnitBuff",
                "UnitDebuff",
                "BuffFrame",
                "TargetFrame",
                "AuraButton",
            ]
            .iter()
            .any(|k| e.contains(k))
        })
        .collect()
}

fn texture(s: &UiScript, region: &str) -> String {
    s.eval::<Option<String>>(&format!("return {region}:GetTexture()"))
        .unwrap()
        .unwrap_or_default()
}

/// The player's buff and debuff rows fill from `UnitAura` on `UNIT_AURA`, the debuff border
/// tints by its dispel type, and a right-click cancels the buff under it by spell id.
#[test]
fn the_stock_buff_frame_draws_the_players_auras_and_cancels_on_right_click() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, errors) = harness();
    let load = aura_errors(errors);
    assert!(load.is_empty(), "load errors: {load:#?}");

    let mut fort = aura(
        1243,
        "Power Word: Fortitude",
        "Spell_Holy_WordFortitude",
        true,
    );
    fort.caster = ME;
    fort.duration = 1800.0;
    fort.expiration_time = s.now() + 1800.0;
    let mut poison = aura(744, "Poison", "Spell_Nature_CorrosiveBreath", false);
    poison.debuff_type = Some("Poison".into());
    poison.count = 3;
    s.set_player_auras(vec![fort, poison]);
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("player".into())]);
    s.tick(0.1);

    assert!(s.eval::<bool>("return BuffButton1:IsShown()").unwrap());
    assert!(
        texture(&s, "BuffButton1Icon").contains("Spell_Holy_WordFortitude"),
        "{}",
        texture(&s, "BuffButton1Icon")
    );
    assert!(s
        .eval::<bool>("return BuffButton2 == nil or not BuffButton2:IsShown()")
        .unwrap());
    assert!(s.eval::<bool>("return DebuffButton1:IsShown()").unwrap());
    assert!(texture(&s, "DebuffButton1Icon").contains("Spell_Nature_CorrosiveBreath"));
    assert_eq!(
        s.eval::<String>("return DebuffButton1Count:GetText()")
            .unwrap(),
        "3"
    );
    // `DebuffTypeColor.Poison` is (0, 0.6, 0).
    let (r, g, b): (f64, f64, f64) = s
        .eval("return DebuffButton1Border:GetVertexColor()")
        .unwrap();
    assert!(
        r < 0.01 && (g - 0.6).abs() < 0.01 && b < 0.01,
        "{r} {g} {b}"
    );

    s.run(r#"BuffButton_OnClick(BuffButton1)"#).unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1243]);
    s.run(r#"BuffButton_OnClick(DebuffButton1)"#).unwrap();
    assert!(s.take_cancel_aura_requests().is_empty(), "a debuff stays");

    // The list empties: every button hides.
    s.set_player_auras(Vec::new());
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("player".into())]);
    assert!(!s.eval::<bool>("return BuffButton1:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return DebuffButton1:IsShown()").unwrap());

    let run = aura_errors(s.take_errors());
    assert!(run.is_empty(), "runtime errors: {run:#?}");
}

/// The target frame reads the icon as `UnitBuff`'s third return and lays out a row of each sign.
#[test]
fn the_stock_target_frame_draws_the_targets_buffs_and_debuffs() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, errors) = harness();
    let load = aura_errors(errors);
    assert!(load.is_empty(), "load errors: {load:#?}");

    s.set_unit_guids(&UnitGuids {
        player: ME,
        target: MOB,
        ..Default::default()
    });
    let mut sunder = aura(7386, "Sunder Armor", "Ability_Warrior_Sunder", false);
    sunder.caster = ME;
    sunder.count = 2;
    s.set_unit_auras(
        MOB,
        Some(vec![
            aura(1126, "Mark of the Wild", "Spell_Nature_Regeneration", true),
            sunder,
        ]),
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            has_object: true,
            name: Some("Young Wolf".into()),
            health: 50,
            max_health: 50,
            level: 2,
            is_connected: true,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);

    assert!(s.eval::<bool>("return TargetFrameBuff1:IsShown()").unwrap());
    assert!(texture(&s, "TargetFrameBuff1Icon").contains("Spell_Nature_Regeneration"));
    assert!(s
        .eval::<bool>("return TargetFrameDebuff1:IsShown()")
        .unwrap());
    assert!(texture(&s, "TargetFrameDebuff1Icon").contains("Ability_Warrior_Sunder"));

    let run = aura_errors(s.take_errors());
    assert!(run.is_empty(), "runtime errors: {run:#?}");
}

/// 3.3.5a's `SPELL_TIME_REMAINING_*` carry their plural inline as `|4`, which the tooltip line's
/// text store resolves (`0x483910`): each reads clean at one and at many, the two forms apart, and
/// a timed buff's tooltip line too.
#[test]
fn the_time_remaining_templates_resolve_their_plural_on_the_tooltip() {
    benilla_formats::wow_data_or_skip!();
    let (mut s, _) = harness();
    let lines: Vec<String> = s
        .eval::<String>(
            r#"local out = {}
               for _, k in ipairs({"SPELL_TIME_REMAINING_DAYS", "SPELL_TIME_REMAINING_HOURS",
                                   "SPELL_TIME_REMAINING_MIN", "SPELL_TIME_REMAINING_SEC"}) do
                 for _, n in ipairs({1, 12}) do
                   GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
                   GameTooltip:AddLine(format(_G[k], n))
                   out[#out + 1] = GameTooltipTextLeft1:GetText()
                 end
               end
               return table.concat(out, "\n")"#,
        )
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines.len(), 8);
    for pair in lines.chunks(2) {
        assert!(
            !pair[0].contains("|4") && !pair[1].contains("|4"),
            "{pair:?}"
        );
        assert_ne!(
            pair[0].replacen('1', "", 1),
            pair[1].replacen("12", "", 1),
            "{pair:?}"
        );
    }

    s.set_spell_tooltip(
        1243,
        benilla_ui::script::SpellTooltipView {
            name: "Fortitude".into(),
            aura_description: "Up.".into(),
            ..Default::default()
        },
    );
    s.tick(10.0);
    let mut timed = aura(1243, "Fortitude", "Spell_Holy_WordFortitude", true);
    timed.duration = 1800.0;
    timed.expiration_time = 730.0;
    s.set_player_auras(vec![timed]);
    let shown = s
        .eval::<String>(
            r#"GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
               GameTooltip:SetUnitAura("player", 1)
               local out = {}
               for i = 1, GameTooltip:NumLines() do
                 out[#out + 1] = _G["GameTooltipTextLeft" .. i]:GetText() or ""
               end
               return table.concat(out, "\n")"#,
        )
        .unwrap();
    assert!(shown.contains("12 "), "{shown}");
    assert!(!shown.contains("|4"), "{shown}");
}
