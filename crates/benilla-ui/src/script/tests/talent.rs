//! The talent bindings over the state lent at the call, the learn sends, and `SetTalent`: the talent builder
//! for a passive talent, the spell tooltip with the talent lines interleaved for an exceptional one.

use super::common::script;
use crate::script::*;

fn ranks(first: u32, name: &str, n: u32) -> Vec<TalentRankView> {
    (0..n)
        .map(|i| TalentRankView {
            spell: first + i,
            name: name.into(),
            icon: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
        })
        .collect()
}

/// Fire: Improved Fireball (tier 1, rank 3 of 5) and Ignite (tier 2, unlearned, needs Improved
/// Fireball at rank 5); an empty Frost tab.
fn one_tab_pages() -> TalentPages {
    TalentPages {
        tabs: vec![
            TalentTabView {
                id: 41,
                name: "Fire".into(),
                icon: None,
                background: "MageFire".into(),
            },
            TalentTabView {
                id: 61,
                name: "Frost".into(),
                icon: None,
                background: "MageFrost".into(),
            },
        ],
        talents: vec![
            vec![
                TalentView {
                    id: 26,
                    tier: 1,
                    column: 1,
                    ranks: ranks(11068, "Improved Fireball", 5),
                    ..Default::default()
                },
                TalentView {
                    id: 34,
                    tier: 2,
                    column: 2,
                    ranks: ranks(11119, "Ignite", 5),
                    prereqs: vec![TalentPrereqView {
                        talent: 26,
                        rank: 4,
                        seat: Some((1, 1)),
                        spell: 11068 + 4,
                        name: Some("Improved Fireball".into()),
                    }],
                    ..Default::default()
                },
            ],
            Vec::new(),
        ],
    }
}

/// One group, Improved Fireball at rank 3, two points unspent, laid over `pages`.
fn state_over(pages: TalentPages) -> live::TestView {
    let mut info = TalentInfo::default();
    info.update_talent_data(
        &TalentPacket {
            unspent: 2,
            active: 0,
            groups: vec![TalentPacketGroup {
                talents: vec![(26, 2)],
                glyphs: Vec::new(),
            }],
        },
        &|_, _| true,
    );
    info.enter_world(|| Some(pages));
    live::TestView {
        talent_info: Some(info),
        talent_units: (Some(ME), None),
        character_points: Some((2, 1)),
        player: Some(ME),
        ..Default::default()
    }
}

const ME: u64 = 0x10;

#[test]
fn bindings_read_the_state_at_the_call() {
    let mut s = script();
    s.set_unit_guids(&UnitGuids {
        player: ME,
        ..Default::default()
    });
    let mut v = state_over(one_tab_pages());
    s.with_live(&mut v, |s| {
        s.run(
            r#"
        assert(GetNumTalentTabs() == 2)
        local name, icon, points, file, preview = GetTalentTabInfo(1)
        assert(name == "Fire" and icon == nil and points == 3 and file == "MageFire" and preview == 0)
        assert(GetTalentTabInfo(3) == nil, "out of range is nil")
        assert(GetNumTalents(1) == 2 and GetNumTalents(2) == 0 and GetNumTalents(9) == 0)

        local n, icon, tier, col, rank, max, exc, meets, prank, pmeets = GetTalentInfo(1, 1)
        assert(n == "Improved Fireball" and tier == 1 and col == 1)
        assert(rank == 3 and max == 5 and exc == nil and meets == 1 and prank == 3 and pmeets == 1)
        assert(icon == "Interface\\Icons\\Spell_Fire_FlameBolt")
        assert(GetTalentInfo(1, 9) == nil)

        -- The prereq quadruplets, flat.
        local pt, pc, pl, ppl = GetTalentPrereqs(1, 2)
        assert(pt == 1 and pc == 1 and pl == nil and ppl == nil)
        assert(GetTalentPrereqs(1, 1) == nil, "no prereqs is empty")

        local cp1, cp2 = UnitCharacterPoints("player")
        assert(cp1 == 2 and cp2 == 1)
    "#,
        )
        .unwrap();
    });
    assert!(s.take_errors().is_empty());
}

#[test]
fn learn_talent_sends_the_talent_and_its_next_rank_in_the_call() {
    let mut s = script();
    let mut v = state_over(one_tab_pages());
    s.with_live(&mut v, |s| {
        s.run("LearnTalent(1, 1); LearnTalent(1, 2)").unwrap()
    });
    assert_eq!(v.talent_learns, vec![(None, 26, 3), (None, 34, 0)]);
    assert!(s.take_errors().is_empty());
}

/// Stand-in values for the talent keys, not the shipped wording: the tests check which key each
/// line reaches.
fn seed_talent_strings(s: &mut UiScript) {
    s.run(
        r#"
        TOOLTIP_TALENT_RANK      = "[RANK %d/%d]"
        TOOLTIP_TALENT_NEXT_RANK = "[NEXT_RANK]"
        TOOLTIP_TALENT_LEARN     = "[LEARN]"
        TOOLTIP_TALENT_PREREQ_P1 = "Requires %d points in %s"
        TALENT_TOOLTIP_ADDPREVIEWPOINT    = "[ADD]"
        TALENT_TOOLTIP_REMOVEPREVIEWPOINT = "[REMOVE]"
    "#,
    )
    .unwrap();
}

/// A passive talent's full spell view, every body line filled, so a test sees which ones the talent
/// builder drops.
fn passive_body(description: &str) -> SpellTooltipView {
    SpellTooltipView {
        name: "Improved Fireball".into(),
        cost: Some("35 Mana".into()),
        range: Some("30 yd range".into()),
        cast_time: Some("Instant".into()),
        cooldown: Some("6 sec cooldown".into()),
        requires_item: Some("Requires One-Handed Axes".into()),
        requires_form: Some("Requires Battle Stance".into()),
        reagents: Some("Reagents: Light Feather".into()),
        chance: Some("2.62% chance to dodge".into()),
        description: description.into(),
        ..Default::default()
    }
}

/// `SetTalent` on a passive talent is the talent builder: name, `TOOLTIP_TALENT_RANK` (white), the
/// gold description, a `" "` spacer, `TOOLTIP_TALENT_NEXT_RANK` and the next rank's gold
/// description, green `TOOLTIP_TALENT_LEARN`; no cost, range, cast, requirement, reagent or chance
/// line.
#[test]
fn set_talent_on_a_passive_talent_is_the_talent_builder() {
    let mut s = script();
    seed_talent_strings(&mut s);
    let mut v = state_over(one_tab_pages());
    live::lending(&mut v, || {
        s.set_spell_tooltip(
            11070,
            passive_body("Reduces the casting time of your Fireball spell by 0.3 sec."),
        );
        s.set_spell_tooltip(
            11071,
            passive_body("Reduces the casting time of your Fireball spell by 0.4 sec."),
        );
        s.run(
        r#"
        local a = CreateFrame("Button", "TB1"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 1)
        assert(tt:IsShown(), "SetTalent shows")
        -- name, rank, desc, spacer, next-rank header, next desc, learn hint = 7 lines.
        assert(tt:NumLines() == 7, "got " .. tt:NumLines())
        assert(TTTextLeft1:GetText() == "Improved Fireball")
        assert(TTTextLeft2:GetText() == "[RANK 3/5]", "got " .. TTTextLeft2:GetText())
        assert(TTTextLeft3:GetText() == "Reduces the casting time of your Fireball spell by 0.3 sec.")
        assert(TTTextLeft4:GetText() == " ", "the spacer, got " .. tostring(TTTextLeft4:GetText()))
        assert(TTTextLeft5:GetText() == "[NEXT_RANK]")
        assert(TTTextLeft6:GetText() == "Reduces the casting time of your Fireball spell by 0.4 sec.")
        assert(TTTextLeft7:GetText() == "[LEARN]")
        for i = 1, 7 do
            assert(getglobal("TTTextRight" .. i):GetText() == nil, "no right column on line " .. i)
        end
    "#,
    )
    .unwrap();
        s.resolve();
        let quads = s.extract();
        let green = quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == "[LEARN]" && c[0] < 1e-6 && (c[1] - 1.0).abs() < 1e-6)
        });
        assert!(green, "the learn hint is green");
        // The spacer is the tooltip line's own gold, not the caller's colour.
        let gold_spacer = quads.iter().any(|q| {
        matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t == " " && (c[0] - 1.0).abs() < 1e-6 && (c[1] - 210.0 / 255.0).abs() < 1e-3 && c[2] < 1e-6)
    });
        assert!(gold_spacer, "the spacer is gold");
    });
    assert!(s.take_errors().is_empty());
}

/// An exceptional talent (`Talent.dbc` Flags bit 0) is the spell builder instead, whose cost, cast
/// and required-item lines stay: Holy Shield's "Requires Shields".
#[test]
fn set_talent_on_an_exceptional_talent_keeps_the_spell_body() {
    let mut s = script();
    seed_talent_strings(&mut s);
    s.run(r#"TOOLTIP_TALENT_TIER_POINTS = "Requires %d points in %s Talents""#)
        .unwrap();
    let mut pages = one_tab_pages();
    pages.tabs[0].name = "Protection".into();
    let holy_shield = &mut pages.talents[0][1];
    holy_shield.ranks = ranks(11119, "Holy Shield", 5);
    holy_shield.tier = 7;
    holy_shield.prereqs.clear();
    holy_shield.exceptional = true;
    let mut v = state_over(pages);
    live::lending(&mut v, || {
        s.set_spell_tooltip(
            11119,
            SpellTooltipView {
                name: "Holy Shield".into(),
                cost: Some("150 Mana".into()),
                cast_time: Some("Instant cast".into()),
                requires_item: Some("Requires Shields".into()),
                description: "Increases chance to block by 30% for 10 sec.".into(),
                ..Default::default()
            },
        );
        s.run(
        r#"
        local a = CreateFrame("Button", "TB3"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT3")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 2)
        local want = {
            "Holy Shield", "[RANK 0/5]", "Requires 30 points in Protection Talents", "150 Mana",
            "Instant cast", "Requires Shields", "Increases chance to block by 30% for 10 sec.",
        }
        assert(tt:NumLines() == table.getn(want), "got " .. tt:NumLines())
        for i, text in ipairs(want) do
            local got = getglobal("TT3TextLeft" .. i):GetText()
            assert(got == text, "line " .. i .. ": " .. tostring(got))
        end
    "#,
    )
    .unwrap();
    });
    assert!(s.take_errors().is_empty());
}

/// A missing spell view renders the rank line alone.
#[test]
fn set_talent_locked_reqs_and_a_missing_view() {
    let mut s = script();
    seed_talent_strings(&mut s);
    let mut v = state_over(one_tab_pages());
    live::lending(&mut v, || {
        // No spell view pushed for Ignite (11119).
        s.run(
        r#"
        local a = CreateFrame("Button", "TB2"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT2")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 2)
        assert(tt:IsShown())
        assert(TT2TextLeft1:GetText() == "[RANK 0/5]", "fallback shows the rank head")
    "#,
    )
    .unwrap();

        s.set_spell_tooltip(
            11119,
            SpellTooltipView {
                name: "Ignite".into(),
                description: "Your critical strikes ignite the target.".into(),
                ..Default::default()
            },
        );
        s.run(
            r#"
        local tt = getglobal("TT2")
        tt:SetOwner(getglobal("TB2"), "ANCHOR_RIGHT")
        tt:SetTalent(1, 2)
        -- name, rank, req(red), desc = 4 lines; rank 0 has no next block, locked has no hint.
        assert(tt:NumLines() == 4, "got " .. tt:NumLines())
        assert(TT2TextLeft3:GetText() == "Requires 5 points in Improved Fireball",
            tostring(TT2TextLeft3:GetText()))
    "#,
        )
        .unwrap();
        s.resolve();
        let quads = s.extract();
        let red = quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(t), color: Some(c), .. }
            if t.starts_with("Requires 5") && (c[0] - 1.0).abs() < 1e-6 && c[1] < 0.2)
        });
        assert!(red, "the requirement line is red");
    });
    assert!(s.take_errors().is_empty());
}

/// The requirement lines in 12340's order (`0x6224f0`): the prerequisite slots (`0x622660`), then
/// the tier line (`0x622739`). The tier compares the tab's spent plus preview points whatever
/// isPreview (`0x6226d9`), shows only with a group (`0x62268b`), and a held rank shows no line
/// (`0x627085`).
#[test]
fn set_talent_lists_the_prerequisites_before_the_tier_line() {
    let mut s = script();
    seed_talent_strings(&mut s);
    s.run(r#"TOOLTIP_TALENT_TIER_POINTS = "Requires %d points in %s Talents""#)
        .unwrap();
    let mut v = state_over(one_tab_pages());
    live::lending(&mut v, || {
        s.set_spell_tooltip(11119, passive_body("Ignite."));
        s.run(
        r#"
        local a = CreateFrame("Button", "TB6"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT6")
        local function lines(...)
            tt:SetOwner(a, "ANCHOR_RIGHT")
            tt:SetTalent(...)
            local out = {}
            for i = 1, tt:NumLines() do
                local t = getglobal("TT6TextLeft" .. i):GetText()
                if t and string.find(t, "^Requires") then table.insert(out, t) end
            end
            return table.concat(out, "|")
        end
        local both = "Requires 5 points in Improved Fireball|Requires 5 points in Fire Talents"
        local got = lines(1, 2)
        assert(got == both, "prerequisite first: " .. got)
        -- No group 3: the prerequisite shows, the tier line does not.
        got = lines(1, 2, false, false, 3)
        assert(got == "Requires 5 points in Improved Fireball", "no group: " .. got)
        -- Two preview points make the tab's 5 for the tier line with isPreview off.
        AddPreviewTalentPoints(1, 1, 2)
        got = lines(1, 2)
        assert(got == "Requires 5 points in Improved Fireball", "preview counts: " .. got)
    "#,
    )
    .unwrap();
    });
    assert!(s.take_errors().is_empty());
}

/// A talent that holds a rank shows no requirement line (`0x627085`), even with its prerequisite
/// short.
#[test]
fn set_talent_on_a_held_rank_shows_no_requirement() {
    let mut s = script();
    seed_talent_strings(&mut s);
    s.run(r#"TOOLTIP_TALENT_TIER_POINTS = "Requires %d points in %s Talents""#)
        .unwrap();
    let mut info = TalentInfo::default();
    info.update_talent_data(
        &TalentPacket {
            unspent: 2,
            active: 0,
            groups: vec![TalentPacketGroup {
                talents: vec![(26, 2), (34, 0)],
                glyphs: Vec::new(),
            }],
        },
        &|_, _| true,
    );
    info.enter_world(|| Some(one_tab_pages()));
    let mut v = live::TestView {
        talent_info: Some(info),
        talent_units: (Some(ME), None),
        player: Some(ME),
        ..Default::default()
    };
    live::lending(&mut v, || {
        s.set_spell_tooltip(11119, passive_body("Ignite."));
        s.set_spell_tooltip(11120, passive_body("Ignite more."));
        s.run(
        r#"
        local a = CreateFrame("Button", "TB7"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT7")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 2)
        assert(TT7TextLeft2:GetText() == "[RANK 1/5]", tostring(TT7TextLeft2:GetText()))
        for i = 1, tt:NumLines() do
            local t = getglobal("TT7TextLeft" .. i):GetText() or ""
            assert(not string.find(t, "^Requires %d"), "line " .. i .. ": " .. t)
        end
    "#,
    )
    .unwrap();
    });
    assert!(s.take_errors().is_empty());
}

/// With `isPreview` the closing hints are the preview moves that would pass (`0x622800`): a point
/// can go on Improved Fireball and, once one is on, come off.
#[test]
fn set_talent_in_preview_offers_the_moves_that_would_pass() {
    let mut s = script();
    seed_talent_strings(&mut s);
    let mut v = state_over(one_tab_pages());
    live::lending(&mut v, || {
        s.set_spell_tooltip(
            11070,
            passive_body("Reduces the casting time of your Fireball spell by 0.3 sec."),
        );
        s.set_spell_tooltip(
            11071,
            passive_body("Reduces the casting time of your Fireball spell by 0.4 sec."),
        );
        s.run(
        r#"
        local a = CreateFrame("Button", "TB4"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT4")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 1, false, false, 1, true)
        local last = getglobal("TT4TextLeft" .. tt:NumLines()):GetText()
        assert(last == "[ADD]", "nothing previewed: add only, got " .. tostring(last))
        AddPreviewTalentPoints(1, 1, 1)
        tt:SetOwner(a, "ANCHOR_RIGHT")
        tt:SetTalent(1, 1, false, false, 1, true)
        local n = tt:NumLines()
        assert(getglobal("TT4TextLeft" .. (n - 1)):GetText() == "[ADD]")
        assert(getglobal("TT4TextLeft" .. n):GetText() == "[REMOVE]")
    "#,
    )
    .unwrap();
    });
    assert!(s.take_errors().is_empty());
}

/// A pet talent's required spell is asked of the pet at the call (`0x5c60c0`, `0x5c6128`,
/// `0x5c6134`): the requirement line shows while the pet lacks it, and goes once it knows it.
#[test]
fn set_talent_asks_the_pet_for_a_pet_talents_required_spell() {
    const PET: u64 = 0xF140_0000_0000_0021;
    let mut s = script();
    seed_talent_strings(&mut s);
    s.run(r#"ITEM_REQ_SKILL = "Requires %s""#).unwrap();
    s.set_spell_tooltip(
        200,
        SpellTooltipView {
            name: "Cobra Reflexes".into(),
            description: "Faster attacks.".into(),
            ..Default::default()
        },
    );
    let pages = TalentPages {
        tabs: vec![TalentTabView {
            id: 409,
            name: "Ferocity".into(),
            icon: None,
            background: "HunterPetFerocity".into(),
        }],
        talents: vec![vec![TalentView {
            id: 20,
            tier: 1,
            column: 1,
            ranks: ranks(200, "Cobra Reflexes", 2),
            required_spell: 777,
            required_spell_name: Some("Dash".into()),
            ..Default::default()
        }]],
    };
    let mut v = state_over(one_tab_pages());
    let info = v.talent_info.as_mut().unwrap();
    info.update_pet_tabs(1, || Some(pages));
    info.update_pet_talent_data(1, &[], &|_, _| true);
    v.talent_units = (Some(ME), Some(PET));
    v.pet_category = Some(0);
    let shows_req = |s: &mut UiScript, v: &mut live::TestView| -> bool {
        live::lending(v, || {
            s.eval::<bool>(
                r#"
            local a = CreateFrame("Button"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
            local tt = TTP or CreateFrame("GameTooltip", "TTP")
            tt:SetOwner(a, "ANCHOR_RIGHT")
            tt:SetTalent(1, 1, false, true, 1)
            for i = 1, tt:NumLines() do
                if getglobal("TTPTextLeft" .. i):GetText() == "Requires Dash" then return true end
            end
            return false
        "#,
            )
            .unwrap()
        })
    };
    assert!(shows_req(&mut s, &mut v), "the pet lacks Dash");
    v.pet_known = vec![777];
    assert!(!shows_req(&mut s, &mut v), "the pet knows Dash");
}
