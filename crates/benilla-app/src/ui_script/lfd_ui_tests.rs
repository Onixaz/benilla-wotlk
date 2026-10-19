//! The stock 3.3.5a dungeon finder (`LFDFrame.lua`, `LFGFrame.lua`, `Minimap.xml`'s eye) on the
//! production load, fed the install's dungeon tables and a lock info shaped like the server's
//! answer to a level 80 warrior: the reported window's Type menu, reward panel and Find Group.

use benilla_ui::script::{
    LfgDungeonRewardView, LfgProposalMemberView, LfgProposalView, LfgRequest, LfgRewardItemView,
    LfgUpdateView, UiScript, UnitState,
};

/// Errors from the dungeon finder's own code; the production load carries unrelated 3.3.5a gaps.
fn lfg_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("LFD") || e.contains("LFG") || e.contains("LFR"))
        .collect()
}

/// The production VM with the tables pushed and the game initialized for a level 80 warrior.
fn finder() -> Option<(UiScript, benilla_ui::script::LfgCatalogView)> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalog = crate::ui_lfg::read_catalog(&mut chain).expect("the dungeon finder tables");
    let (mut s, _) = super::layer_tests::production_load_seeded(
        "lfd",
        false,
        "",
        |_| {},
        |s| {
            s.set_account_expansion(2);
        },
    );
    let _ = s.take_errors();
    s.set_lfg_catalog(catalog.clone());
    s.lfg_initialize_game();
    s.set_lfg_class(1);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            level: 80,
            ..Default::default()
        }),
    );
    Some((s, catalog))
}

/// The random rows a level 80 Wrath account is offered, as the server lists them.
fn level_80_randoms(catalog: &benilla_ui::script::LfgCatalogView) -> Vec<u32> {
    catalog
        .dungeons
        .iter()
        .filter(|d| d.type_id == 6 && d.expansion_level == 2 && d.max_level >= 80)
        .map(|d| d.id as u32 | 6 << 24)
        .collect()
}

fn text(s: &UiScript, expr: &str) -> String {
    s.eval::<Option<String>>(&format!("return {expr}"))
        .unwrap()
        .unwrap_or_else(|| "<nil>".into())
}

/// The reported window: the Type menu lists "Specific Dungeons" and the offered randoms, the best
/// one selected and named; Find Group sends the join for it once a role is chosen; the queue
/// update shows the minimap eye.
#[test]
fn the_stock_dungeon_finder_lists_its_types_and_joins() {
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((mut s, catalog)) = finder() else {
        return;
    };
    let randoms = level_80_randoms(&catalog);
    assert!(
        !randoms.is_empty(),
        "the install offers a random level 80 dungeon"
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.lfg_player_info(
        randoms
            .iter()
            .map(|&slot| LfgDungeonRewardView {
                slot,
                base_money: 50_000,
                base_xp: 0,
                items: vec![LfgRewardItemView {
                    item: 49426,
                    display: 0,
                    count: 2,
                }],
                ..Default::default()
            })
            .collect(),
        Vec::new(),
    );
    s.run("ToggleLFDParentFrame()").unwrap();
    s.tick(0.0);
    let errors = lfg_errors(s.take_errors());
    assert!(errors.is_empty(), "window errors: {errors:#?}");
    assert!(s
        .eval::<bool>("return LFDParentFrame:IsShown() == 1")
        .unwrap());

    let best: i64 = s.eval("return GetRandomDungeonBestChoice()").unwrap();
    assert!(randoms.contains(&(best as u32 | 6 << 24)));
    assert_eq!(s.eval::<i64>("return LFDQueueFrame.type").unwrap(), best);
    let best_name = catalog
        .dungeons
        .iter()
        .find(|d| i64::from(d.id) == best)
        .unwrap()
        .name
        .clone();
    assert_eq!(
        text(&s, "UIDropDownMenu_GetText(LFDQueueFrameTypeDropDown)"),
        best_name
    );

    s.run("ToggleDropDownMenu(1, nil, LFDQueueFrameTypeDropDown)")
        .unwrap();
    s.tick(0.0);
    let entries: i64 = s.eval("return DropDownList1.numButtons").unwrap();
    assert_eq!(
        entries as usize,
        1 + randoms.len(),
        "Specific Dungeons and each offered random"
    );
    assert_eq!(
        text(&s, "DropDownList1Button1:GetText()"),
        text(&s, "SPECIFIC_DUNGEONS")
    );
    s.run("CloseDropDownMenus()").unwrap();
    assert!(lfg_errors(s.take_errors()).is_empty());

    s.run("SetLFGRoles(false, false, false, true) LFG_UpdateRoleCheckboxes()")
        .unwrap();
    s.run("LFDQueueFrameFindGroupButton:Click()").unwrap();
    s.tick(0.0);
    let joins: Vec<LfgRequest> = s
        .take_lfg_requests()
        .into_iter()
        .filter(|r| matches!(r, LfgRequest::Join { .. }))
        .collect();
    assert_eq!(
        joins,
        [LfgRequest::Join {
            roles: 0x8,
            no_partial_clear: false,
            achievements: false,
            slots: vec![best as u32 | 6 << 24],
            needs: [0; 3],
            comment: String::new(),
        }]
    );

    s.lfg_update_player(
        LfgUpdateView {
            update_type: 5,
            joined: true,
            lfg_joined: true,
            queued: true,
            slots: vec![best as u32 | 6 << 24],
            ..Default::default()
        },
        false,
    );
    s.tick(0.0);
    assert!(s
        .eval::<bool>("return MiniMapLFGFrame:IsShown() == 1")
        .unwrap());
    assert_eq!(
        text(&s, "LFDQueueFrameFindGroupButton:GetText()"),
        text(&s, "LEAVE_QUEUE")
    );
    let errors = lfg_errors(s.take_errors());
    assert!(errors.is_empty(), "queued errors: {errors:#?}");
}

/// Specific Dungeons lists the Wrath dungeons under their headers, a locked one marked with its
/// reason, with no script error.
#[test]
fn the_specific_list_shows_headers_and_locks() {
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((mut s, catalog)) = finder() else {
        return;
    };
    let locked = catalog
        .dungeons
        .iter()
        .find(|d| {
            d.type_id == 1
                && d.expansion_level == 2
                && d.flags & 0x2 != 0
                && (d.min_level..=d.max_level).contains(&80)
        })
        .map(|d| d.id as u32 | 1 << 24)
        .unwrap();
    s.lfg_player_info(Vec::new(), vec![(locked, 2)]);
    s.run("ToggleLFDParentFrame() LFDQueueFrame_SetType('specific')")
        .unwrap();
    s.tick(0.0);
    let errors = lfg_errors(s.take_errors());
    assert!(errors.is_empty(), "specific list errors: {errors:#?}");
    assert!(s
        .eval::<bool>("return LFDQueueFrameSpecific:IsShown() == 1")
        .unwrap());
    let rows: i64 = s.eval("return #LFDDungeonList").unwrap();
    assert!(rows > 0, "the list has rows");
    let header = text(
        &s,
        "LFDQueueFrameSpecificListButton1.instanceName:GetText()",
    );
    assert!(
        catalog.groups.iter().any(|g| g.name == header),
        "the first row is a header: {header}"
    );
    let reason: i64 = s
        .eval(&format!("return LFGLockList[{}]", locked & 0xff_ffff))
        .unwrap();
    assert_eq!(reason, 2, "the stock lock list holds the reason");
}

/// A group found: the ready popup shows the dungeon, Enter Dungeon answers the proposal and the
/// popup turns to the members' status; the group's formation hides it.
#[test]
fn the_ready_popup_answers_a_proposal() {
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((mut s, catalog)) = finder() else {
        return;
    };
    let dungeon = catalog
        .dungeons
        .iter()
        .find(|d| d.type_id == 1 && d.expansion_level == 2 && d.flags & 0x2 != 0)
        .unwrap();
    let slot = dungeon.id as u32 | 1 << 24;
    let member = |is_local: bool, roles: u32| LfgProposalMemberView {
        roles,
        is_local,
        ..Default::default()
    };
    let proposal = LfgProposalView {
        slot,
        state: 0,
        id: 42,
        completed_mask: 0,
        silent: false,
        members: vec![
            member(true, 0x8),
            member(false, 0x2),
            member(false, 0x4),
            member(false, 0x8),
            member(false, 0x8),
        ],
    };
    s.lfg_update_player(
        LfgUpdateView {
            update_type: 5,
            joined: true,
            lfg_joined: true,
            queued: true,
            slots: vec![slot],
            ..Default::default()
        },
        false,
    );
    s.lfg_proposal(proposal);
    s.tick(0.0);
    assert!(lfg_errors(s.take_errors()).is_empty());
    assert!(s
        .eval::<bool>("return LFDDungeonReadyPopup:IsShown() == 1")
        .unwrap());
    assert!(s
        .eval::<bool>("return LFDDungeonReadyDialog:IsShown() == 1")
        .unwrap());
    s.run("LFDDungeonReadyDialogEnterDungeonButton:Click()")
        .unwrap();
    s.tick(0.0);
    let answers: Vec<LfgRequest> = s
        .take_lfg_requests()
        .into_iter()
        .filter(|r| matches!(r, LfgRequest::ProposalResult { .. }))
        .collect();
    assert_eq!(
        answers,
        [LfgRequest::ProposalResult {
            id: 42,
            accept: true
        }]
    );
    assert!(s
        .eval::<bool>("return LFDDungeonReadyStatus:IsShown() == 1")
        .unwrap());
    s.lfg_update_player(
        LfgUpdateView {
            update_type: 10,
            joined: false,
            ..Default::default()
        },
        false,
    );
    s.tick(0.0);
    assert!(!s
        .eval::<bool>("return LFDDungeonReadyPopup:IsShown() == 1")
        .unwrap());
    let errors = lfg_errors(s.take_errors());
    assert!(errors.is_empty(), "popup errors: {errors:#?}");
}
