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
