//! The stock 3.3.5a gossip window (`GossipFrame.lua`) and its popups (`UIParent.lua`'s
//! `GOSSIP_CONFIRM`, `StaticPopup.lua`) on the production load, over the menu the app pushes.

use benilla_ui::script::{
    GossipMenu, GossipOptionView, GossipQuestRow, PlayerReqState, ScriptValue, UiScript,
};

/// Errors from the gossip window's own code; the production load carries unrelated 3.3.5a gaps.
fn gossip_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Gossip", "StaticPopup"].iter().any(|k| e.contains(k)))
        .collect()
}

fn loaded() -> UiScript {
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("gossip", false, "", |_| {});
    let _ = s.take_errors();
    s.set_player_req_state(PlayerReqState {
        level: 10,
        ..Default::default()
    });
    s
}

fn option(label: &str, icon: &str) -> GossipOptionView {
    GossipOptionView {
        label: label.into(),
        icon_type: icon.into(),
        ..Default::default()
    }
}

/// `GOSSIP_SHOW` paints the 3.3.5a rows: available quests five values wide with the daily icon,
/// a trivial quest greyed, active quests four wide with the complete icon, then the options.
#[test]
fn the_stock_gossip_window_paints_both_quest_lists_and_the_options() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = loaded();
    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: vec![
            GossipQuestRow {
                quest_id: 61,
                title: "Daily Chores".into(),
                level: 10,
                daily: true,
                ..Default::default()
            },
            GossipQuestRow {
                quest_id: 62,
                title: "Old News".into(),
                level: 2,
                ..Default::default()
            },
            GossipQuestRow {
                quest_id: 60,
                title: "Report Back".into(),
                level: 9,
                active: true,
                ..Default::default()
            },
        ],
        options: vec![
            option("Train me.", "trainer"),
            option("Let me browse your goods.", "vendor"),
        ],
        force_gossip: false,
    }));
    s.set_gossip_complete_quests(vec![60]);
    s.fire_event("GOSSIP_SHOW", Vec::<ScriptValue>::new());
    let errors = gossip_errors(s.take_errors());
    assert!(errors.is_empty(), "gossip window errors: {errors:#?}");
    s.run(
        r#"
        assert(GossipFrame:IsShown(), "the window is open")
        assert(GossipGreetingText:GetText() == "Well met.", "the greeting")
        local function row(i) return _G["GossipTitleButton" .. i] end
        local function icon(i) return (_G["GossipTitleButton" .. i .. "GossipIcon"]:GetTexture() or ""):lower() end
        assert(row(1):GetText():find("Daily Chores"), "available row 1: " .. tostring(row(1):GetText()))
        assert(icon(1):find("dailyquesticon"), "the daily icon, got " .. icon(1))
        assert(row(1).type == "Available" and row(1):GetID() == 1)
        assert(row(2):GetText():find("Old News"), "available row 2")
        assert(select(1, _G["GossipTitleButton2GossipIcon"]:GetVertexColor()) < 0.6, "a trivial quest greys")
        assert(icon(2):find("availablequesticon"), "the plain available icon, got " .. icon(2))
        assert(not row(3):IsShown(), "the spacer after the available list")
        assert(row(4):GetText():find("Report Back") and row(4).type == "Active", "the active row")
        assert(icon(4):find("activequesticon"), "a complete quest's icon, got " .. icon(4))
        assert(not row(5):IsShown(), "the spacer after the active list")
        assert(row(6):GetText() == "Train me." and row(6).type == "Gossip", "option 1")
        assert(icon(6):find("trainergossipicon"), "the trainer icon, got " .. icon(6))
        assert(row(7):GetText() == "Let me browse your goods.", "option 2")
        "#,
    )
    .unwrap();

    // The second option's click queues its select, and the binding spends the menu.
    s.run("GossipTitleButton7:Click()").unwrap();
    assert_eq!(s.take_gossip_selects(), vec![(2, None)]);
    assert!(s.eval::<i64>("return GetNumGossipOptions()").unwrap() == 0);
    assert!(gossip_errors(s.take_errors()).is_empty());
}

/// A lone service option skips the window and selects itself (`GossipFrame.lua`'s first branch),
/// unless the NPC forces gossip.
#[test]
fn a_lone_service_option_selects_itself_unless_forced() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = loaded();
    let menu = GossipMenu {
        greeting: "Yes?".into(),
        options: vec![option("Train me.", "trainer")],
        ..Default::default()
    };
    s.set_gossip(Some(menu.clone()));
    s.fire_event("GOSSIP_SHOW", Vec::<ScriptValue>::new());
    assert!(gossip_errors(s.take_errors()).is_empty());
    assert_eq!(s.take_gossip_selects(), vec![(1, None)]);
    assert!(!s.eval::<bool>("return GossipFrame:IsShown()").unwrap());

    s.set_gossip(Some(GossipMenu {
        force_gossip: true,
        ..menu
    }));
    s.fire_event("GOSSIP_SHOW", Vec::<ScriptValue>::new());
    assert!(gossip_errors(s.take_errors()).is_empty());
    assert!(s.take_gossip_selects().is_empty());
    assert!(s.eval::<bool>("return GossipFrame:IsShown()").unwrap());
}

/// An option with box text asks through the stock `GOSSIP_CONFIRM` popup, with its money, and
/// the popup's accept sends the select.
#[test]
fn a_box_text_option_confirms_through_the_stock_popup() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = loaded();
    s.set_money(10_000);
    s.set_gossip(Some(GossipMenu {
        greeting: "Hm?".into(),
        options: vec![
            GossipOptionView {
                label: "Reset my talents.".into(),
                icon_type: "trainer".into(),
                box_text: Some("Are you sure?".into()),
                box_money: 500,
                ..Default::default()
            },
            option("Goodbye.", "gossip"),
        ],
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", Vec::<ScriptValue>::new());
    s.run("GossipTitleButton1:Click()").unwrap();
    assert!(s.take_gossip_selects().is_empty(), "asked, not sent");
    s.tick(0.0);
    let errors = gossip_errors(s.take_errors());
    assert!(errors.is_empty(), "popup errors: {errors:#?}");
    s.run(
        r#"
        local dialog = StaticPopup_Visible("GOSSIP_CONFIRM")
        assert(dialog, "the confirm popup is up")
        assert(_G[dialog .. "Text"]:GetText() == "Are you sure?", "its text")
        assert(_G[dialog].data == 1, "it carries the option's index")
        assert(_G[dialog .. "MoneyFrame"]:IsShown(), "and the cost")
        _G[dialog .. "Button1"]:Click()
        "#,
    )
    .unwrap();
    assert_eq!(s.take_gossip_selects(), vec![(1, None)]);
    // The sent select's `GOSSIP_CONFIRM_CANCEL` hides the popup.
    s.tick(0.0);
    assert!(s
        .eval::<bool>("return StaticPopup_Visible('GOSSIP_CONFIRM') == nil")
        .unwrap());
    assert!(gossip_errors(s.take_errors()).is_empty());
}
