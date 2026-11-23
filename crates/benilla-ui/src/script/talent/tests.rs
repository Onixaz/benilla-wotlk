use super::*;
use crate::script::live::TestView;
use crate::script::{TalentPacket, TalentPacketGroup, TalentPages, UiScript};

const ME: u64 = 0x10;
const PET: u64 = 0xF140_0000_0000_0021;

fn rank(spell: u32, name: &str) -> TalentRankView {
    TalentRankView {
        spell,
        name: name.into(),
        icon: Some("Interface\\Icons\\Spell_Fire_Fireball".into()),
    }
}

/// One tab: A (tier 1, three ranks), B (tier 1, one rank) and C (tier 2, needs A at rank 3).
fn pages() -> TalentPages {
    let a = TalentView {
        id: 10,
        tier: 1,
        column: 1,
        ranks: vec![rank(100, "A"), rank(101, "A"), rank(102, "A")],
        ..Default::default()
    };
    let b = TalentView {
        id: 11,
        tier: 1,
        column: 2,
        ranks: vec![rank(110, "B")],
        ..Default::default()
    };
    let c = TalentView {
        id: 12,
        tier: 2,
        column: 1,
        ranks: vec![rank(120, "C")],
        prereqs: vec![TalentPrereqView {
            talent: 10,
            rank: 2,
            seat: Some((1, 1)),
            spell: 1,
            name: Some("A".into()),
        }],
        ..Default::default()
    };
    TalentPages {
        tabs: vec![TalentTabView {
            id: 41,
            name: "Fire".into(),
            icon: Some("Interface\\Icons\\Spell_Fire_FireBolt02".into()),
            background: "MageFire".into(),
        }],
        talents: vec![vec![a, b, c]],
    }
}

fn valid(_: u32, _: i32) -> bool {
    true
}

/// Two groups; group 1 active with two points in A and three unspent.
fn info() -> TalentInfo {
    let mut info = TalentInfo::default();
    info.update_talent_data(
        &TalentPacket {
            unspent: 3,
            active: 0,
            groups: vec![
                TalentPacketGroup {
                    talents: vec![(10, 1)],
                    glyphs: Vec::new(),
                },
                TalentPacketGroup::default(),
            ],
        },
        &valid,
    );
    info.enter_world(|| Some(pages()));
    info
}

fn view() -> TestView {
    TestView {
        talent_info: Some(info()),
        talent_units: (Some(ME), None),
        player: Some(ME),
        ..Default::default()
    }
}

#[test]
fn the_tab_and_talent_tuples_read_the_state_at_the_call() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        let tab: (String, String, i64, String, i64) = s.eval("return GetTalentTabInfo(1)").unwrap();
        assert_eq!(
            tab,
            (
                "Fire".into(),
                "Interface\\Icons\\Spell_Fire_FireBolt02".into(),
                2,
                "MageFire".into(),
                0
            )
        );
        let info: (String, i64, i64, i64, i64, Option<i64>, i64, i64) = s
            .eval("local n,_,t,c,r,m,e,p,pr,pp = GetTalentInfo(1, 1) return n,t,c,r,m,e,p,pr")
            .unwrap();
        assert_eq!(info, ("A".into(), 1, 1, 2, 3, None, 1, 2));
        assert_eq!(
            s.eval::<i64>("return select(5, GetTalentInfo(1, 1, false, false, 2))")
                .unwrap(),
            0,
            "the other group holds nothing"
        );
        assert_eq!(
            s.eval::<i64>("return select(3, GetTalentTabInfo(1, false, false, 2))")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return select('#', GetTalentInfo(1, 9))")
                .unwrap(),
            10
        );
        assert_eq!(s.eval::<i64>("return GetNumTalentGroups()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetActiveTalentGroup()").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetNumTalentGroups(false, true)")
                .unwrap(),
            0,
            "no pet tree: the pet spec tab stays hidden"
        );
        assert_eq!(s.eval::<i64>("return GetUnspentTalentPoints()").unwrap(), 3);
        assert_eq!(
            s.eval::<i64>("return GetUnspentTalentPoints(false, false, 2)")
                .unwrap(),
            5,
            "the total less its own spent (`0x5c9ca0`)"
        );
    });
    assert_eq!(
        s.eval::<i64>("return GetNumTalentTabs()").unwrap(),
        0,
        "no view lent reads no state"
    );
}

/// No player held: `meetsPrereq` reads nil (`0x5c7800`), and nothing is learned (`0x5c81df`).
#[test]
fn without_the_unit_nothing_meets_and_nothing_learns() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    v.talent_units = (None, None);
    s.with_live(&mut v, |s| {
        let meets: Option<i64> = s.eval("return select(8, GetTalentInfo(1, 1))").unwrap();
        assert_eq!(meets, None);
        s.run("LearnTalent(1, 1)").unwrap();
    });
    assert!(v.talent_learns.is_empty());
}

/// A required spell meets once known or a higher rank of it is (`0x5c7b67`, `0x5c7b76`).
#[test]
fn a_required_spell_reads_the_known_spells_at_the_call() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    if let Some(info) = v.talent_info.as_mut() {
        let mut p = pages();
        p.talents[0][1].required_spell = 999;
        *info = TalentInfo::default();
        info.enter_world(|| Some(p));
    }
    s.with_live(&mut v, |s| {
        let meets: Option<i64> = s.eval("return select(8, GetTalentInfo(1, 2))").unwrap();
        assert_eq!(meets, None, "999 unknown");
    });
}

#[test]
fn prereqs_are_quadruplets_against_the_groups_ranks() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        let q: (i64, i64, Option<i64>, Option<i64>) =
            s.eval("return GetTalentPrereqs(1, 3)").unwrap();
        assert_eq!(q, (1, 1, None, None), "A is at rank 2 of the 3 C needs");
        assert_eq!(
            s.eval::<i64>("return select('#', GetTalentPrereqs(1, 1))")
                .unwrap(),
            0
        );
    });
}

#[test]
fn learn_talent_sends_the_next_zero_based_rank_in_the_call() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        s.run("LearnTalent(1, 1)").unwrap();
        assert_eq!(
            crate::script::live::test_view(|v| v.talent_learns.clone()),
            vec![(None, 10, 2)],
            "sent before the call returns"
        );
        s.run("LearnTalent(1, 2)").unwrap();
    });
    assert_eq!(v.talent_learns, vec![(None, 10, 2), (None, 11, 0)]);
    // Maxed: nothing.
    let mut info = TalentInfo::default();
    info.update_talent_data(
        &TalentPacket {
            unspent: 1,
            active: 0,
            groups: vec![TalentPacketGroup {
                talents: vec![(11, 0)],
                glyphs: Vec::new(),
            }],
        },
        &valid,
    );
    info.enter_world(|| Some(pages()));
    v.talent_info = Some(info);
    v.talent_learns.clear();
    s.with_live(&mut v, |s| s.run("LearnTalent(1, 2)").unwrap());
    assert!(v.talent_learns.is_empty());
}

#[test]
fn preview_points_move_within_the_rules_fire_in_the_call_and_learn_in_one_send() {
    let mut s = UiScript::new().unwrap();
    s.run(
        "PREV = {} local f = CreateFrame('Frame') f:RegisterEvent('PREVIEW_TALENT_POINTS_CHANGED') \
         f:SetScript('OnEvent', function(_, e, i, t, g, d) \
           table.insert(PREV, i .. ',' .. t .. ',' .. g .. ',' .. d .. ':' .. GetGroupPreviewTalentPointsSpent()) end)",
    )
    .unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        // C is locked: tier 2 needs five points in the tab, the tab has two.
        s.run("AddPreviewTalentPoints(1, 3, 1)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent()")
                .unwrap(),
            0
        );
        s.run("AddPreviewTalentPoints(1, 1, 5); AddPreviewTalentPoints(1, 2, 1)")
            .unwrap();
        let pr: (i64, i64) = s
            .eval("return select(9, GetTalentInfo(1, 1)), select(9, GetTalentInfo(1, 2))")
            .unwrap();
        assert_eq!(pr, (3, 1), "A clamps at its max rank");
        assert_eq!(
            s.eval::<i64>("return select(5, GetTalentTabInfo(1))")
                .unwrap(),
            2
        );
        s.run("AddPreviewTalentPoints(1, 1, -5)").unwrap();
        assert_eq!(
            s.eval::<i64>("return select(9, GetTalentInfo(1, 1))")
                .unwrap(),
            2,
            "never under the learned rank"
        );
        s.run("AddPreviewTalentPoints(1, 1, 1); LearnPreviewTalents()")
            .unwrap();
        assert_eq!(
            crate::script::live::test_view(|v| v.talent_preview_learns.clone()),
            vec![(None, vec![(10, 2), (11, 0)])]
        );
        s.run("ResetGroupPreviewTalentPoints()").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent()")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return GetPreviewTalentPointsSpent(1)")
                .unwrap(),
            0,
            "12340's stub"
        );
        let fired: Vec<String> = s.eval("return PREV").unwrap();
        assert_eq!(
            fired,
            vec![
                "1,1,1,1:1",
                "2,1,1,1:2",
                "1,1,1,-1:1",
                "1,1,1,1:2",
                "0,0,1,-2:0"
            ],
            "each move's event inside its call, reading the total it left"
        );
    });
}

#[test]
fn a_link_names_the_talent_and_its_zero_based_rank() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        assert_eq!(
            s.eval::<String>("return GetTalentLink(1, 1)").unwrap(),
            "|cff4e96f7|Htalent:10:1|h[A]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return GetTalentLink(1, 2)").unwrap(),
            "|cff4e96f7|Htalent:11:-1|h[B]|h|r"
        );
    });
}

/// The preview-over-rank flag is argument 5, the group index read again as a boolean
/// (`0x5c7e13`); a sixth argument is never read, whatever the usage text says.
#[test]
fn a_link_takes_the_preview_rank_when_the_group_index_reads_true() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        s.run("AddPreviewTalentPoints(1, 1, 1)").unwrap();
        let links: (String, String, String, String) = s
            .eval(
                "return GetTalentLink(1, 1, false, false, 1), \
                 GetTalentLink(1, 1, false, false, nil, true), \
                 GetTalentLink(1, 1), GetTalentLink(1, 1, false, false, '1')",
            )
            .unwrap();
        assert_eq!(
            links,
            (
                "|cff4e96f7|Htalent:10:2|h[A]|h|r".into(),
                "|cff4e96f7|Htalent:10:1|h[A]|h|r".into(),
                "|cff4e96f7|Htalent:10:1|h[A]|h|r".into(),
                "|cff4e96f7|Htalent:10:2|h[A]|h|r".into(),
            )
        );
    });
}

#[test]
fn switching_groups_casts_the_activation_spell() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        s.run("SetActiveTalentGroup(1); SetActiveTalentGroup(2); SetActiveTalentGroup(3)")
            .unwrap();
    });
    let casts = s.take_spell_casts();
    assert_eq!(
        casts.len(),
        1,
        "only another existing group casts: {casts:?}"
    );
    assert_eq!(casts[0].0, 63644);
}

/// `isInspect` reads the inspected player's set: its own groups and active one whatever the
/// player's, a talent meeting its prerequisite once it holds a rank, and nothing for an inspected
/// pet (`0x5c5c60`, `0x5c6080`, `0x5c7b93`).
#[test]
fn the_inspect_set_answers_the_inspected_players_groups() {
    let mut s = UiScript::new().unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        assert_eq!(s.eval::<i64>("return GetNumTalentTabs(true)").unwrap(), 0);
    });
    let mut inspected = pages();
    inspected.tabs[0].name = "Arcane".into();
    if let Some(info) = v.talent_info.as_mut() {
        info.set_inspect_target(0x77, Some(inspected));
        info.update_inspect_talent_data(
            &TalentPacket {
                unspent: 7,
                active: 0,
                groups: vec![TalentPacketGroup {
                    talents: vec![(10, 2), (12, 0)],
                    glyphs: Vec::new(),
                }],
            },
            &valid,
        );
    }
    s.with_live(&mut v, |s| {
        assert_eq!(s.eval::<i64>("return GetNumTalentTabs(true)").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetNumTalentTabs(true, true)")
                .unwrap(),
            0
        );
        assert_eq!(s.eval::<i64>("return GetNumTalents(1, true)").unwrap(), 3);
        assert_eq!(
            s.eval::<i64>("return GetNumTalents(1, true, true)")
                .unwrap(),
            0
        );
        assert_eq!(s.eval::<i64>("return GetNumTalentGroups(true)").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return GetActiveTalentGroup(true)").unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>("return GetUnspentTalentPoints(true)")
                .unwrap(),
            7
        );
        assert_eq!(
            s.eval::<i64>("return GetUnspentTalentPoints(true, true)")
                .unwrap(),
            0
        );
        let tab: (String, i64) = s
            .eval("local n, _, spent = GetTalentTabInfo(1, true) return n, spent")
            .unwrap();
        assert_eq!(tab, ("Arcane".into(), 4));
        let a: (i64, Option<i64>, i64, Option<i64>) = s
            .eval("local _,_,_,_,r,_,_,p,pr,pp = GetTalentInfo(1, 1, true) return r, p, pr, pp")
            .unwrap();
        assert_eq!(a, (3, Some(1), 3, Some(1)));
        let b: (i64, Option<i64>) = s
            .eval("local _,_,_,_,r,_,_,p = GetTalentInfo(1, 2, true) return r, p")
            .unwrap();
        assert_eq!(b, (0, None), "no rank: the prerequisite reads unmet");
        assert!(s
            .eval::<bool>("return GetTalentInfo(1, 1, true, true) == nil")
            .unwrap());
        let q: (i64, i64, Option<i64>) = s.eval("return GetTalentPrereqs(1, 3, true)").unwrap();
        assert_eq!(q, (1, 1, Some(1)), "A holds rank 3 here");
        assert_eq!(
            s.eval::<String>("return GetTalentLink(1, 1, true)")
                .unwrap(),
            "|cff4e96f7|Htalent:10:2|h[A]|h|r"
        );
        assert_eq!(
            s.eval::<String>("return (GetTalentTabInfo(1))").unwrap(),
            "Fire",
            "the player's own set is untouched"
        );
    });
}

/// A pet tree: A (tier 1, three ranks), B (tier 2, one rank, not for this pet's category), C
/// (tier 2, one rank). One group with 2 points in A and 4 unspent.
fn pet_pages() -> TalentPages {
    let talent = |id, tier, column, ranks: Vec<TalentRankView>, mask| TalentView {
        id,
        tier,
        column,
        ranks,
        category_mask: [mask, 0],
        ..Default::default()
    };
    TalentPages {
        tabs: vec![TalentTabView {
            id: 409,
            name: "Ferocity".into(),
            icon: None,
            background: "HunterPetFerocity".into(),
        }],
        talents: vec![vec![
            talent(
                20,
                1,
                1,
                vec![rank(200, "A"), rank(201, "A"), rank(202, "A")],
                0,
            ),
            talent(21, 2, 1, vec![rank(210, "B")], 1 << 5),
            talent(22, 2, 2, vec![rank(220, "C")], 0),
        ]],
    }
}

fn pet_view() -> TestView {
    let mut info = info();
    info.update_pet_tabs(1, || Some(pet_pages()));
    info.update_pet_talent_data(4, &[(20, 1)], &valid);
    TestView {
        talent_info: Some(info),
        talent_units: (Some(ME), Some(PET)),
        pet_category: Some(1),
        player: Some(ME),
        ..Default::default()
    }
}

#[test]
fn the_pet_tree_is_its_own_set_with_one_group() {
    let mut s = UiScript::new().unwrap();
    let mut v = pet_view();
    s.with_live(&mut v, |s| {
        let counts: (i64, i64, i64, i64, i64) = s
            .eval(
                "return GetNumTalentGroups(false, true), GetNumTalentTabs(false, true), \
                 GetNumTalents(1, false, true), GetUnspentTalentPoints(false, true, 1), \
                 GetActiveTalentGroup(false, true)",
            )
            .unwrap();
        assert_eq!(counts, (1, 1, 3, 4, 1));
        assert_eq!(s.eval::<i64>("return GetNumTalentGroups()").unwrap(), 2);
        assert_eq!(
            s.eval::<i64>("return select(3, GetTalentTabInfo(1, false, true, 1))")
                .unwrap(),
            2
        );
        let a: (String, i64, i64) = s
            .eval("local n, _, _, _, r, m = GetTalentInfo(1, 1, false, true, 1) return n, r, m")
            .unwrap();
        assert_eq!(a, ("A".into(), 2, 3));
        assert!(s
            .eval::<bool>("return GetTalentInfo(1, 2, false, true, 1) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetTalentLink(1, 2, false, true, 1) == nil")
            .unwrap());
        assert_eq!(
            s.eval::<i64>("return select(5, GetTalentInfo(1, 1, false, true, 2))")
                .unwrap(),
            0,
            "the pet's only group is group 1"
        );
    });
    // The pet gone: every pet talent is the placeholder (`0x5c5a20` with no unit).
    v.pet_category = None;
    s.with_live(&mut v, |s| {
        assert!(s
            .eval::<bool>("return GetTalentInfo(1, 1, false, true, 1) == nil")
            .unwrap());
    });
}

/// The side flags go through the bool-or-default reader (`0x815500`; `0x5c61ab`, `0x5c5dfe`,
/// `0x5c8163`): `0`, `"0"`, `"false"` and `"off"` pick the player's side, where Lua truthiness
/// would pick the pet's.
#[test]
fn a_zero_or_off_flag_reads_the_players_side() {
    let mut s = UiScript::new().unwrap();
    let mut v = pet_view();
    s.with_live(&mut v, |s| {
        for off in ["0", "'0'", "'false'", "'off'"] {
            let tab: String = s
                .eval(&format!("return GetTalentTabInfo(1, {off}, {off})"))
                .unwrap();
            assert_eq!(tab, "Fire", "{off} is false");
            let groups: i64 = s
                .eval(&format!("return GetNumTalentGroups(false, {off})"))
                .unwrap();
            assert_eq!(groups, 2, "{off}: the player's two groups");
        }
        assert_eq!(
            s.eval::<String>("return GetTalentTabInfo(1, 0, 1)")
                .unwrap(),
            "Ferocity",
            "1 is true"
        );
        s.run("LearnTalent(1, 1, 0)").unwrap();
    });
    assert_eq!(
        v.talent_learns,
        vec![(None, 10, 2)],
        "LearnTalent(1, 1, 0) is the player's"
    );
}

/// Tier 2 opens at 3 points for a pet (`0x5c73d0`), where the player's needs 5; the learns carry
/// the pet's guid.
#[test]
fn a_pet_tier_takes_three_points_and_previews_fire_the_pet_event() {
    let mut s = UiScript::new().unwrap();
    s.run(
        "PETPREV = 0 local f = CreateFrame('Frame') \
         f:RegisterEvent('PREVIEW_PET_TALENT_POINTS_CHANGED') f:RegisterEvent('PREVIEW_TALENT_POINTS_CHANGED') \
         f:SetScript('OnEvent', function(_, e) if e == 'PREVIEW_PET_TALENT_POINTS_CHANGED' then PETPREV = PETPREV + 1 else PETPREV = -100 end end)",
    )
    .unwrap();
    let mut v = pet_view();
    s.with_live(&mut v, |s| {
        s.run("AddPreviewTalentPoints(1, 3, 1, true, 1)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent(true, 1)")
                .unwrap(),
            0,
            "two points open no tier 2"
        );
        s.run("AddPreviewTalentPoints(1, 1, 1, true, 1) AddPreviewTalentPoints(1, 3, 1, true, 1)")
            .unwrap();
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent(true, 1)")
                .unwrap(),
            2,
            "the third point in A opens C"
        );
        assert_eq!(s.eval::<i64>("return PETPREV").unwrap(), 2);
        assert_eq!(
            s.eval::<i64>("return GetGroupPreviewTalentPointsSpent()")
                .unwrap(),
            0,
            "the player's preview is untouched"
        );
        s.run("LearnPreviewTalents(true) LearnTalent(1, 1, true) LearnTalent(1, 2, true) LearnTalent(1, 3, true)")
            .unwrap();
    });
    assert_eq!(
        v.talent_preview_learns,
        vec![(Some(PET), vec![(20, 2), (22, 0)])]
    );
    assert_eq!(
        v.talent_learns,
        vec![(Some(PET), 20, 2), (Some(PET), 22, 0)],
        "B is outside the pet's category"
    );
}

/// `UnitCharacterPoints` reads our fields at the call; any other unit answers 0, 0.
#[test]
fn character_points_read_our_fields_at_the_call() {
    let mut s = UiScript::new().unwrap();
    s.set_unit_guids(&crate::script::UnitGuids {
        player: ME,
        target: 0x99,
        ..Default::default()
    });
    let mut v = view();
    v.character_points = Some((2, 1));
    s.with_live(&mut v, |s| {
        let me: (i64, i64) = s.eval("return UnitCharacterPoints('player')").unwrap();
        assert_eq!(me, (2, 1));
        let other: (i64, i64) = s.eval("return UnitCharacterPoints('target')").unwrap();
        assert_eq!(other, (0, 0));
    });
    assert!(s.run("UnitCharacterPoints()").is_err());
}

/// `SetCVar("previewTalents", ...)` runs the CVar's callback inside the call, ahead of the store and
/// changed or not (`0x7668c0` -> `0x5127d0`): every player group's preview back to its ranks, then
/// `PREVIEW_TALENT_POINTS_CHANGED(0, 0, 0, -spent)`, then the pet's event; an unknown name runs none.
#[test]
fn the_preview_cvar_resets_the_preview_inside_the_call() {
    let mut s = UiScript::new().unwrap();
    s.register_cvars([("previewTalents", "0")]);
    s.run(
        "PREV = {} local f = CreateFrame('Frame') \
         f:RegisterEvent('PREVIEW_TALENT_POINTS_CHANGED') f:RegisterEvent('PREVIEW_PET_TALENT_POINTS_CHANGED') \
         f:SetScript('OnEvent', function(_, e, i, t, g, d) table.insert(PREV, e .. ' ' .. i .. ',' .. t .. ',' .. g .. ',' .. d \
           .. ':' .. GetGroupPreviewTalentPointsSpent() .. ':' .. GetCVar('previewTalents')) end)",
    )
    .unwrap();
    let mut v = view();
    s.with_live(&mut v, |s| {
        s.run("AddPreviewTalentPoints(1, 1, 1) AddPreviewTalentPoints(1, 2, 1) PREV = {}")
            .unwrap();
        s.run("SetCVar('previewTalents', '1')").unwrap();
        let fired: Vec<String> = s.eval("return PREV").unwrap();
        assert_eq!(
            fired,
            vec![
                "PREVIEW_TALENT_POINTS_CHANGED 0,0,0,-2:0:0",
                "PREVIEW_PET_TALENT_POINTS_CHANGED 0,0,0,0:0:0",
            ],
            "the callback runs ahead of the store"
        );
        s.run("PREV = {} SetCVar('previewTalents', '1') SetCVar('noSuchCVar', '1')")
            .unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return PREV").unwrap().len(),
            2,
            "an unchanged value runs it again; an unknown name runs none"
        );
        s.run("AddPreviewTalentPoints(1, 1, 1) PREV = {} ConsoleExec('previewTalents 0')")
            .unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return PREV").unwrap(),
            vec![
                "PREVIEW_TALENT_POINTS_CHANGED 0,0,0,-1:0:1",
                "PREVIEW_PET_TALENT_POINTS_CHANGED 0,0,0,0:0:1",
            ],
            "a console set runs the same callback ahead of its store"
        );
        assert_eq!(
            s.eval::<String>("return GetCVar('previewTalents')")
                .unwrap(),
            "0"
        );
    });
}
