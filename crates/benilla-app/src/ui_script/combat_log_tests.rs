//! The stock 3.3.5a `Blizzard_CombatLog` over benilla's combat-log store, on the production load.

use benilla_ui::script::{
    CombatLogEntry, CombatLogPayload, CombatLogUnit, Damage, MissType, UiScript,
};

use crate::local_state::test_env::ENV_LOCK;

const ME: u64 = 0x3ee;
const MOB: u64 = 0xf130_0000_060c_281e;
/// Mine, friendly, player-controlled player.
const ME_FLAGS: u32 = 0x511;
/// Outsider, hostile, NPC-controlled NPC.
const MOB_FLAGS: u32 = 0xa48;

fn unit(guid: u64, name: &str, flags: u32) -> CombatLogUnit {
    CombatLogUnit {
        guid,
        name: Some(name.into()),
        flags,
    }
}

fn entry(
    time_ms: u64,
    source: CombatLogUnit,
    dest: CombatLogUnit,
    payload: CombatLogPayload,
) -> CombatLogEntry {
    CombatLogEntry {
        time_ms,
        timestamp: 1_000_000.0 + time_ms as f64 * 0.001,
        source,
        dest,
        payload,
    }
}

/// The errors the combat log raised since the last take, the load's other gaps aside.
fn combat_log_errors(s: &mut UiScript) -> Vec<String> {
    s.take_errors()
        .into_iter()
        .filter(|e| e.contains("CombatLog"))
        .collect()
}

/// The production load with `Blizzard_CombatLog` up and its tab shown, its lines in `LINES`.
fn combat_log_ui() -> UiScript {
    let (mut s, failures) =
        super::layer_tests::production_load_with("combatlog", false, "", |_| {});
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("CombatLog"))
        .collect();
    assert!(ours.is_empty(), "combat-log load failures: {ours:#?}");
    s.take_errors();
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_CombatLog");
    s.run(
        "CombatLog_LoadUI()
         LINES = {}
         local add = ChatFrame2.AddMessage
         ChatFrame2.AddMessage = function(self, text, ...)
           LINES[#LINES + 1] = text
           return add(self, text, ...)
         end
         ChatFrame2:Show()",
    )
    .unwrap();
    s.resolve();
    let errors = combat_log_errors(&mut s);
    assert!(errors.is_empty(), "addon errors: {errors:#?}");
    s
}

/// Swings reach the stock tab as one line each, and the stock refilter reads them back.
#[test]
fn a_swing_reaches_the_stock_combat_log_tab() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = combat_log_ui();
    s.push_combat_log(entry(
        6_000,
        unit(ME, "Claude", ME_FLAGS),
        unit(MOB, "Kobold Vermin", MOB_FLAGS),
        CombatLogPayload::SwingDamage(Damage {
            amount: 17,
            school: 1,
            ..Damage::default()
        }),
    ));
    s.push_combat_log(entry(
        6_400,
        unit(MOB, "Kobold Vermin", MOB_FLAGS),
        unit(ME, "Claude", ME_FLAGS),
        CombatLogPayload::SwingMissed {
            miss: MissType::Dodge,
            amount: None,
        },
    ));
    s.resolve();
    let errors = combat_log_errors(&mut s);
    assert!(errors.is_empty(), "event errors: {errors:#?}");
    let lines: Vec<String> = s.eval("return LINES").unwrap();
    assert_eq!(lines.len(), 2, "one line per swing: {lines:#?}");
    assert!(
        lines[0].contains("Kobold Vermin") && lines[0].contains("17"),
        "the hit names its victim and amount: {}",
        lines[0]
    );
    assert!(
        lines[1].contains("Kobold Vermin"),
        "the dodge names its attacker: {}",
        lines[1]
    );

    // The refilter walks a few entries a frame; each step stands for one frame's `OnUpdate`.
    s.run(
        "LINES = {}
         Blizzard_CombatLog_Refilter()
         for _ = 1, 10 do
           if not CombatLogUpdateFrame.refiltering then break end
           Blizzard_CombatLog_RefilterUpdate()
         end",
    )
    .unwrap();
    s.resolve();
    let errors = combat_log_errors(&mut s);
    assert!(errors.is_empty(), "refilter errors: {errors:#?}");
    let refiltered: Vec<String> = s.eval("return LINES").unwrap();
    assert_eq!(
        refiltered.len(),
        2,
        "the refilter reads both: {refiltered:#?}"
    );
}

/// The 12340 boot record docks `ChatFrame2` at 2 behind the primary, so its OnLoad joins the dock
/// and lays it over `ChatFrame1`, and with General selected the combat log's quick-button bar
/// hides on `ADDON_LOADED`; a dock position of 1 asserted in `FCFDock_AddChatFrame` and left the
/// bar floating over the world (#289).
#[test]
fn the_combat_log_docks_behind_general_and_its_bar_hides() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) = super::layer_tests::production_load_with("chatdock", false, "", |_| {});
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("FloatingChatFrame.lua") || f.contains("ChatFrame2"))
        .collect();
    assert!(ours.is_empty(), "chat frame load failures: {ours:#?}");
    s.resolve();
    assert!(
        s.eval::<bool>(
            "local i for n, f in ipairs(GENERAL_CHAT_DOCK.DOCKED_CHAT_FRAMES) do \
               if f == ChatFrame2 then i = n end end \
             return GENERAL_CHAT_DOCK.primary == ChatFrame1 and i == 2 and ChatFrame2.isDocked == 1"
        )
        .unwrap(),
        "ChatFrame2 is the dock's second frame"
    );
    assert!(
        s.eval::<bool>(
            "return ChatFrame2:GetLeft() == ChatFrame1:GetLeft() \
               and ChatFrame2:GetTop() == ChatFrame1:GetTop()"
        )
        .unwrap(),
        "docked, it lies over the primary"
    );
    s.take_errors();
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_CombatLog");
    s.run("CombatLog_LoadUI()").unwrap();
    s.resolve();
    assert!(
        s.eval::<bool>(
            "return SELECTED_CHAT_FRAME == ChatFrame1 \
               and not CombatLogQuickButtonFrame_Custom:IsShown()"
        )
        .unwrap(),
        "General selected: the bar is hidden"
    );
    let errors = combat_log_errors(&mut s);
    assert!(errors.is_empty(), "addon errors: {errors:#?}");
}

/// A frame's centre, after a layout pass.
fn centre(s: &mut UiScript, frame: &str) -> (f32, f32) {
    s.resolve();
    let (x, y): (f64, f64) = s
        .eval(&format!("return {frame}:GetCenter()"))
        .unwrap_or_else(|e| panic!("{frame}:GetCenter(): {e}"));
    (x as f32, y as f32)
}

/// Some frames of `OnUpdate`, the tab drag's stop poll among them.
fn frames(s: &mut UiScript, n: usize) {
    for _ in 0..n {
        s.tick(0.016);
        s.resolve();
    }
}

/// A left drag of `tab` from its centre to `to` through the pointer path, released there.
fn drag_tab(s: &mut UiScript, tab: &str, to: (f32, f32)) {
    let (x, y) = centre(s, tab);
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_move(x + 6.0, y); // past the drag threshold: the tab's OnDragStart
    frames(s, 2);
    s.mouse_move(to.0, to.1);
    frames(s, 2);
    s.mouse_button(to.0, to.1, "LeftButton", false);
    frames(s, 2);
}

/// The errors the chat frames raised since the last take.
fn chat_errors(s: &mut UiScript) -> Vec<String> {
    s.take_errors()
        .into_iter()
        .filter(|e| e.contains("FloatingChatFrame") || e.contains("IsMouseButtonDown"))
        .collect()
}

/// An undocked combat log re-docks at 2 by Unlock Window and a tab drag, and undocks again (#319, #320).
#[test]
fn an_undocked_combat_log_redocks_at_two_by_its_tab_and_undocks_again() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) = super::layer_tests::production_load_seeded(
        "chattabdrag",
        false,
        "",
        |_| {},
        |s| {
            // The display list the app pushes before the load; `UpdateMenuBarTop` indexes it.
            let r = benilla_ui::script::ScreenResolution {
                width: 1600,
                height: 900,
            };
            s.set_screen_resolutions(vec![r], Some(r));
            let mut log = benilla_ui::script::ChatWindowLook::stock(1);
            log.docked = None;
            s.set_chat_window_looks([(1, log)]);
        },
    );
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("FloatingChatFrame.lua") || f.contains("ChatFrame2"))
        .collect();
    assert!(ours.is_empty(), "chat frame load failures: {ours:#?}");
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_CombatLog");
    // Login order: the cache's windows, `PLAYER_LOGIN`, then the world-entry layout (`FCF_DockUpdate`).
    super::fire_chat_login(&mut s);
    s.fire_event("PLAYER_LOGIN", vec![]);
    s.run("UIParent_ManageFramePositions()").unwrap();
    s.resolve();
    let errors: Vec<_> = s
        .take_errors()
        .into_iter()
        .filter(|e| e.contains("UIParent.lua") || e.contains("ChatFrame") || e.contains("CombatLog"))
        .collect();
    assert!(errors.is_empty(), "login errors: {errors:#?}");
    assert!(
        s.eval::<bool>(
            "return ChatFrame2.isDocked == nil and ChatFrame2.isLocked ~= nil \
               and ChatFrame2:IsShown() ~= nil and ChatFrame2Tab:IsShown() ~= nil"
        )
        .unwrap(),
        "seeded undocked, locked and shown, its tab up"
    );
    // Undocked, the background rises over the bar and the tab rides it: tab, bar, then text.
    assert!(
        s.eval::<bool>(
            "return ChatFrame2Tab:GetBottom() >= CombatLogQuickButtonFrame_Custom:GetTop() \
               and CombatLogQuickButtonFrame_Custom:GetBottom() == ChatFrame2:GetTop()"
        )
        .unwrap(),
        "the tab sits above the quick-button bar"
    );

    // Right-click the tab, then the menu's first row, `FCF_ToggleLock`'s Unlock Window.
    let (x, y) = centre(&mut s, "ChatFrame2Tab");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    frames(&mut s, 1);
    assert!(
        s.eval::<bool>("return DropDownList1:IsShown() ~= nil").unwrap(),
        "the tab menu opens"
    );
    let (x, y) = centre(&mut s, "DropDownList1Button1");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    frames(&mut s, 1);
    assert!(
        s.eval::<bool>("return ChatFrame2.isLocked == nil").unwrap(),
        "unlocked"
    );

    // Drop it on the dock, right of General's tab: the insert index past the primary is 2.
    let to = s
        .eval::<(f64, f64)>(
            "return ChatFrame1Tab:GetRight() + 20, select(2, ChatFrame1Tab:GetCenter())",
        )
        .unwrap();
    drag_tab(&mut s, "ChatFrame2Tab", (to.0 as f32, to.1 as f32));
    let errors = chat_errors(&mut s);
    assert!(errors.is_empty(), "drag errors: {errors:#?}");
    assert!(
        s.eval::<bool>(
            "return ChatFrame2Tab:GetScript('OnUpdate') == nil and MOVING_CHATFRAME == nil"
        )
        .unwrap(),
        "the release ended the drag"
    );
    assert!(
        s.eval::<bool>(
            "local i for n, f in ipairs(GENERAL_CHAT_DOCK.DOCKED_CHAT_FRAMES) do \
               if f == ChatFrame2 then i = n end end \
             local docked = select(9, GetChatWindowInfo(2)) \
             return ChatFrame2.isDocked == 1 and i == 2 and docked == 2"
        )
        .unwrap(),
        "docked at 2, and the record says DOCKED 2"
    );
    // Docked: the primary's rect less the bar at the top, and the drop selects it (`FCF_StopDragging`).
    assert!(
        s.eval::<bool>(
            "return ChatFrame2:GetLeft() == ChatFrame1:GetLeft() \
               and ChatFrame2:GetBottom() == ChatFrame1:GetBottom() \
               and math.abs(ChatFrame1:GetTop() - ChatFrame2:GetTop() \
                 - CombatLogQuickButtonFrame_Custom:GetHeight()) < 0.01 \
               and FCFDock_GetSelectedWindow(GENERAL_CHAT_DOCK) == ChatFrame2"
        )
        .unwrap(),
        "docked over the primary, and selected"
    );
    // General's tab selects General: the combat log and its bar hide behind it.
    let (x, y) = centre(&mut s, "ChatFrame1Tab");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    frames(&mut s, 2);
    assert!(
        s.eval::<bool>(
            "return FCFDock_GetSelectedWindow(GENERAL_CHAT_DOCK) == ChatFrame1 \
               and ChatFrame2:IsShown() == nil \
               and CombatLogQuickButtonFrame_Custom:IsVisible() == nil"
        )
        .unwrap(),
        "behind General, the bar hidden"
    );

    // Adjacent: dragged out of the dock it undocks, stays where it was dropped, saves its spot.
    let (x, y) = centre(&mut s, "ChatFrame2Tab");
    drag_tab(&mut s, "ChatFrame2Tab", (x + 300.0, y + 200.0));
    let errors = chat_errors(&mut s);
    assert!(errors.is_empty(), "undock errors: {errors:#?}");
    let left_after: f64 = s.eval("return ChatFrame2:GetLeft()").unwrap();
    s.mouse_move(x + 500.0, y + 300.0);
    frames(&mut s, 10);
    assert!(
        s.eval::<bool>(
            "return ChatFrame2.isDocked == nil and ChatFrame2Tab:GetScript('OnUpdate') == nil \
               and GetChatWindowSavedPosition(2) ~= nil"
        )
        .unwrap(),
        "undocked, the drag over, its position saved"
    );
    let left_later: f64 = s.eval("return ChatFrame2:GetLeft()").unwrap();
    assert_eq!(left_after, left_later, "the frame stopped following the cursor");
}
