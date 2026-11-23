//! The loot window's packet handlers: the [`LootState`] window and the [`LootLatch`], each moved and
//! signalled at its step, the fishing verdicts, and the item push behind "You receive …".

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{LootLatch, LootState};
use crate::items::RecordWaits;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::pending_item_ops::PendingItemOps;
use crate::ui_action::signals::SignalSink;
use crate::ui_action::UiError;
use crate::ui_script::live::UiEvents;

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::LootResponse, on_response)
        .net_handler(K::LootError, on_error)
        .net_handler(K::LootRemoved, on_removed)
        .net_handler(K::LootMoneyNotify, on_money_notify)
        .net_handler(K::LootClearMoney, on_clear_money)
        .net_handler(K::LootReleaseResponse, on_release_response)
        .net_handler(K::LootMasterList, on_master_list)
        .net_handler(K::LootSlotChanged, on_slot_changed)
        .net_handler(K::FishNotHooked, on_fish_verdict)
        .net_handler(K::FishEscaped, on_fish_verdict)
        .net_handler(K::ItemPushResult, on_item_push_result)
        .net_handler(K::PlayerName, on_name_answer)
        .net_handler(K::PlayerNameMiss, on_name_answer)
        .net_handler(K::Disconnected, on_session_end);
}

/// The rows 12340 reads off a response: the count clamps to 18 (`0x6d55e7`).
const MAX_LOOT_ROWS: usize = 18;

/// 12340's moving test after the loot open (`0x6d57a6`, `0xc000ff`), in benilla's flag layout.
const MOVING_CLOSES_LOOT: u32 = {
    use crate::creature_anim::move_flags as f;
    f::STEERING | f::ASCENDING | f::DESCENDING
};

/// The loot response handler's item shape (`0x6d53b0`): an object we do not hold is released
/// (`0x6d560f`), an item we hold locked (`0x6d56af`), one we hold latched (`0x6d56c8`),
/// targeted, and the window let go and set (`0x5264f0`), the set in turn after the outgoing
/// window's `LOOT_CLOSED`; a window that opens while we move closes as a movement start does
/// (`0x6d57b5`).
fn on_response(
    In(ev): In<SessionEvent>,
    mut select: crate::target::SelectCommit,
    player: Option<Res<crate::player::Player>>,
    index: Res<crate::net::GuidIndex>,
    mut pending: ResMut<PendingItemOps>,
    mut ui: ResMut<UiEvents>,
) {
    let SessionEvent::LootResponse {
        guid,
        loot_type,
        gold,
        mut items,
    } = ev
    else {
        return;
    };
    let seam = &mut select.seam;
    if !admit_response(guid, loot_type, &mut seam.loot_latch, &seam.net) {
        let mut unlock = ItemUnlock {
            pending: &mut pending,
            ui: &mut ui,
        };
        unlock.unlock(guid);
        return;
    }
    if !index.0.contains_key(&guid) {
        debug!("net: loot response {guid:#x} names no object we hold — released");
        ui.line(UiError::key("ERR_LOOT_DIDNT_KILL"));
        let _ = seam.net.0.send(ClientCommand::LootRelease { guid });
        return;
    }
    if benilla_protocol::guid::is_item(guid) {
        ui.world(move |world| super::lock_item_now(world, guid));
    }
    seam.loot_latch.0 = Some(guid);
    items.truncate(MAX_LOOT_ROWS);
    debug!(
        "net: loot response {guid:#x} type {loot_type} gold {gold} {} item(s)",
        items.len()
    );
    select.select_unit(guid, &mut *ui);
    select.seam.loot.let_go(&mut *ui);
    let moving = player
        .is_some_and(|p| p.foreign_mover.is_none() && p.move_flags() & MOVING_CLOSES_LOOT != 0);
    ui.world(move |world| super::set_window(world, guid, loot_type, gold, items, moving));
}

fn on_error(
    In(ev): In<SessionEvent>,
    mut latch: ResMut<LootLatch>,
    mut pending: ResMut<PendingItemOps>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::LootError { guid, error } = ev {
        let refused = loot_error(guid, error, &mut latch);
        if let Some(key) = refused.line {
            ui.line(UiError::key(key));
        }
        if refused.unlock {
            ItemUnlock {
                pending: &mut pending,
                ui: &mut ui,
            }
            .unlock(guid);
        }
    }
}

/// `SMSG_LOOT_REMOVED` (`0x6d5860`): a row was taken, by anyone. Only while the latched object is
/// held (`0x6d588c`) does the wire slot zero (`0x6d58a1`-`0x6d58ba`) and the window clear it
/// (`0x6d58bd` -> `0x5124e0` -> `0x589920`); a window it leaves with no loot closes after the
/// signal.
fn on_removed(
    In(ev): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    latch: Res<LootLatch>,
    index: Res<crate::net::GuidIndex>,
    mut waits: NonSendMut<RecordWaits>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::LootRemoved { slot } = ev {
        debug!("net: loot slot {slot} removed");
        if !latch.0.is_some_and(|g| index.0.contains_key(&g)) {
            return;
        }
        let held = loot.source().is_some_and(|g| index.0.contains_key(&g));
        if loot.remove_slot(slot, held, &mut waits, &mut *ui) {
            ui.world(super::close_emptied);
        }
    }
}

/// `SMSG_LOOT_SLOT_CHANGED` (`0x6d58d0` -> `0x589a30`), in the handler's turn.
fn on_slot_changed(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::LootSlotChanged(p) = ev {
        debug!(
            "net: loot slot {} changed to item {} x{}",
            p.slot, p.item_id, p.count
        );
        ui.world(move |world| {
            super::with_window(world, |loot, step| {
                loot.slot_changed(&p, step);
                None
            });
        });
    }
}

fn on_money_notify(
    In(ev): In<SessionEvent>,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
    mut chat: ResMut<crate::ui_chat::ChatLog>,
) {
    if let SessionEvent::LootMoneyNotify { amount, solo } = ev {
        let get = |key: &str| {
            script
                .as_deref()
                .and_then(|s| crate::ui_chat::combat::global_string(s, key))
        };
        if let Some(line) = loot_money_line(amount, solo, &get) {
            chat.push_event(line);
        }
    }
}

/// `SMSG_LOOT_CLEAR_MONEY` (`0x589100`): the coin row goes, for every looter; an emptied window's
/// close follows its signal.
fn on_clear_money(
    In(ev): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    index: Res<crate::net::GuidIndex>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::LootClearMoney = ev {
        debug!("net: loot coin line cleared");
        let held = loot.source().is_some_and(|g| index.0.contains_key(&g));
        if loot.clear_money(held, &mut *ui) {
            ui.world(super::close_emptied);
        }
    }
}

fn on_release_response(
    In(ev): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
    mut ui: ResMut<UiEvents>,
    mut dead: super::DeadUnitDeselect,
    net: Res<NetCommands>,
) {
    if let SessionEvent::LootReleaseResponse { guid, result } = ev {
        let closed = loot_release_response(guid, result, &mut loot, &mut latch, &net, &mut *ui);
        dead.after_close(closed);
    }
}

/// The item unlock on the loot guid, as the loot closes call it: only an item we locked at its
/// `CMSG_OPEN_ITEM` send unlocks, firing `ITEM_LOCK_CHANGED` in the handler's turn (`0x707020`).
struct ItemUnlock<'a> {
    pending: &'a mut PendingItemOps,
    ui: &'a mut UiEvents,
}

impl ItemUnlock<'_> {
    fn unlock(&mut self, guid: u64) {
        if self.pending.unlock(guid) {
            self.ui
                .world(move |world| crate::ui_items::watch::signal_unlocked(world, guid));
        }
        // The bar's usable update follows the unlock, an item or none (`0x51379b`).
        self.ui.world(crate::ui_action::usable_cache::step(
            crate::ui_action::usable_cache::Pass::Bar,
        ));
    }
}

fn on_master_list(In(ev): In<SessionEvent>, mut loot: ResMut<LootState>) {
    if let SessionEvent::LootMasterList { candidates } = ev {
        loot_master_list(candidates, &mut loot);
    }
}

fn on_fish_verdict(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    match ev {
        SessionEvent::FishNotHooked => fish_verdict(false, &mut *ui),
        SessionEvent::FishEscaped => fish_verdict(true, &mut *ui),
        _ => {}
    }
}

/// What the item push reads: the record and its callbacks, the icons and suffixes, the chat log.
/// `SMSG_ITEM_PUSH_RESULT` (`0x523900`): the push runs at the handler's step, its events and line
/// in turn with the handler's.
pub(super) fn on_item_push_result(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::ItemPushResult(p) = ev {
        debug!(
            "net: item push {} x{} for {:#x} → bag {} slot {:#x}",
            p.item_entry, p.count, p.player_guid, p.bag_slot, p.item_slot
        );
        ui.world(move |world| super::push::item_push(world, p));
    }
}

/// A player name answered: each `GetMasterLootCandidate` that missed it signals
/// `UPDATE_MASTER_LOOT_LIST` on a found name (`0x588150`); a placeholder counts as found
/// (`0x635894`).
fn on_name_answer(In(ev): In<SessionEvent>, mut loot: ResMut<LootState>, mut ui: ResMut<UiEvents>) {
    use benilla_protocol::messages::NameQueryMiss;
    let (guid, found) = match ev {
        SessionEvent::PlayerName { guid, .. } => (guid, true),
        SessionEvent::PlayerNameMiss { guid, miss } => (guid, miss == NameQueryMiss::Placeholder),
        _ => return,
    };
    for _ in 0..loot.take_name_waits(guid) {
        if found {
            ui.fire("UPDATE_MASTER_LOOT_LIST", Vec::new());
        }
    }
}

/// The open window and the latch die with the session, unconditionally.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
) {
    loot.clear_session();
    latch.0 = None;
}

/// The server-started `loot_type`s a cold latch admits (`0x6d53fb`-`0x6d541f`).
const SERVER_STARTED_LOOT: [u8; 6] = [2, 3, 4, 6, 7, 8];

/// `SMSG_LOOT_RESPONSE`'s gate (`0x6d53d4`); a refusal releases (`0x6d546c`) and drops the latch.
fn admit_response(guid: u64, loot_type: u8, latch: &mut LootLatch, net: &NetCommands) -> bool {
    let accept = match latch.0 {
        Some(latched) => latched == guid,
        None => SERVER_STARTED_LOOT.contains(&loot_type),
    };
    if !accept {
        debug!(
            "net: loot response {guid:#x} type {loot_type} REFUSED (latch {:?})",
            latch.0
        );
        if loot_type != 0 {
            let _ = net.0.send(ClientCommand::LootRelease { guid });
        }
        latch.0 = None;
    }
    accept
}

/// `SMSG_FISH_ESCAPED` (the skill roll failed) or `SMSG_FISH_NOT_HOOKED` (clicked before the
/// splash, or expired): a yellow toast, as the handlers push ids `0x13e`/`0x13f`, type-1 entries
/// that the error display fires as `UI_INFO_MESSAGE`.
fn fish_verdict(escaped: bool, sink: &mut dyn SignalSink) {
    let key = if escaped {
        "ERR_FISH_ESCAPED"
    } else {
        "ERR_FISH_NOT_HOOKED"
    };
    debug!("net: fishing verdict {key}");
    sink.line(UiError::key(key));
}

/// The default arm is code 7's arm, so it releases; the compare is unsigned, so codes 0..3 and
/// 15..255 all land there.
const DEFAULT_ARM_RELEASES: bool = true;

/// One arm of the loot-refusal jump table: the message it pushes, and whether it falls into the
/// release tail.
struct LootRefusal {
    /// The `GlobalStrings` key of the id the arm hands the UI's error display.
    key: &'static str,
    /// The tail zeroes the loot guid (our [`LootLatch`]), runs the item unlock and the
    /// base-animation recompute with `-1`: it ends the kneel.
    releases: bool,
}

/// The reference's refusal switch (12340 `0x6d54b8`, table at `0x6d5828`). The arms that answer
/// an already-open window (10, 12, 13, 14) and 12340's `NOT_WHILE_SHAPESHIFTED` (16, `0x6d556d`)
/// skip the release tail. Every other code takes the default, `ERR_LOOT_DIDNT_KILL`, 15
/// included: its own line is shown only for a refused response ([`loot_error`]).
fn loot_refusal(reason: u8) -> LootRefusal {
    use benilla_protocol::messages::loot_error as e;
    // A gap in the vmangos enum, but a real arm of the client's table.
    const UNNAMED_DIDNT_KILL: u8 = 7;
    let (key, releases) = match reason {
        e::TOO_FAR => ("ERR_LOOT_TOO_FAR", true),
        e::BAD_FACING => ("ERR_LOOT_BAD_FACING", true),
        e::LOCKED => ("ERR_LOOT_LOCKED", true),
        UNNAMED_DIDNT_KILL => ("ERR_LOOT_DIDNT_KILL", true),
        e::NOTSTANDING => ("ERR_LOOT_NOTSTANDING", true),
        e::STUNNED => ("ERR_LOOT_STUNNED", true),
        e::PLAYER_NOT_FOUND => ("ERR_LOOT_PLAYER_NOT_FOUND", false),
        e::PLAY_TIME_EXCEEDED => ("ERR_PLAY_TIME_EXCEEDED", true),
        e::MASTER_INV_FULL => ("ERR_LOOT_MASTER_INV_FULL", false),
        e::MASTER_UNIQUE_ITEM => ("ERR_LOOT_MASTER_UNIQUE_ITEM", false),
        e::MASTER_OTHER => ("ERR_LOOT_MASTER_OTHER", false),
        e::NOT_WHILE_SHAPESHIFTED => ("ERR_NOT_WHILE_SHAPESHIFTED", false),
        _ => ("ERR_LOOT_DIDNT_KILL", DEFAULT_ARM_RELEASES),
    };
    LootRefusal { key, releases }
}

/// What a loot error does: the line it shows, then whether its release tail unlocks the item.
#[derive(Debug, PartialEq, Eq)]
struct Refused {
    line: Option<&'static str>,
    unlock: bool,
}

/// `SMSG_LOOT_RESPONSE`'s error shape (`0x6d54b8`); one off the latch is refused (`0x6d57cb`).
fn loot_error(guid: u64, error: u8, latch: &mut LootLatch) -> Refused {
    use benilla_protocol::messages::loot_error as e;
    if latch.0 != Some(guid) {
        debug!(
            "net: loot error {error} on {guid:#x} refused (latch {:?})",
            latch.0
        );
        latch.0 = None;
        return Refused {
            line: (error == e::ALREADY_PICKPOCKETED).then_some("ERR_ALREADY_PICKPOCKETED"),
            unlock: true,
        };
    }
    let LootRefusal { key, releases } = loot_refusal(error);
    debug!("net: loot error {error} on {guid:#x} → {key} (releases: {releases})");
    if releases {
        // Guid-matched: the tail's store is unconditional, but only a matching latch reaches it.
        latch.clear_for(guid);
    }
    Refused {
        line: Some(key),
        unlock: releases,
    }
}

/// `SMSG_LOOT_MONEY_NOTIFY` (`0x6ceca0`): no coin, no line (`0x6cecc9`); else the coin text
/// (`0x6cece1`) fills `YOU_LOOT_MONEY` looted alone or `LOOT_MONEY_SPLIT` shared, a
/// `CHAT_MSG_MONEY` line (`0x6ced63`). The coin row itself drops on `SMSG_LOOT_CLEAR_MONEY`.
fn loot_money_line(
    amount: u32,
    solo: bool,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<crate::ui_chat::ChatEvent> {
    debug!("net: loot money {amount} (solo {solo})");
    if amount == 0 {
        return None;
    }
    let coins = crate::ui_quest::coin_text(amount, &|k| get(k).unwrap_or_default());
    let key = if solo {
        "YOU_LOOT_MONEY"
    } else {
        "LOOT_MONEY_SPLIT"
    };
    let text = benilla_ui::strings::fill(&get(key)?, &[benilla_ui::strings::Arg::S(&coins)]);
    Some(crate::ui_chat::ChatEvent::text_only(
        crate::ui_chat::ChatEventKind::Money,
        text,
    ))
}

/// `SMSG_LOOT_RELEASE_RESPONSE` (`0x6d59e0`): a nonzero result on the latched guid closes.
fn loot_release_response(
    guid: u64,
    result: u8,
    loot: &mut LootState,
    latch: &mut LootLatch,
    net: &NetCommands,
    sink: &mut dyn SignalSink,
) -> Option<u64> {
    if result == 0 || latch.0 != Some(guid) {
        debug!(
            "net: loot release {guid:#x} (result {result}) ignored, latch {:?}",
            latch.0
        );
        return None;
    }
    debug!("net: loot released {guid:#x}");
    latch.0 = None;
    super::close_interaction(loot, latch, net, false, sink)
}

/// `SMSG_LOOT_MASTER_LIST` (`0x6fa690`): the list replaced, open window or none.
fn loot_master_list(candidates: Vec<u64>, loot: &mut LootState) {
    debug!("net: master-loot candidates: {} eligible", candidates.len());
    loot.set_master_candidates(candidates);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// `0x6ceca0`: no coin, no line; the solo byte picks `YOU_LOOT_MONEY` over `LOOT_MONEY_SPLIT`,
    /// filled with the coin text, on the money lane.
    #[test]
    fn a_money_notify_writes_its_loot_money_line() {
        let get = |key: &str| {
            Some(
                match key {
                    "YOU_LOOT_MONEY" => "You loot %s",
                    "LOOT_MONEY_SPLIT" => "Your share of the loot is %s.",
                    "GOLD_AMOUNT" => "%d Gold",
                    "SILVER_AMOUNT" => "%d Silver",
                    "COPPER_AMOUNT" => "%d Copper",
                    _ => return None,
                }
                .to_string(),
            )
        };
        assert!(loot_money_line(0, true, &get).is_none());
        let solo = loot_money_line(10_203, true, &get).expect("a line");
        assert_eq!(solo.kind, Some(crate::ui_chat::ChatEventKind::Money));
        assert_eq!(solo.text, "You loot 1 Gold, 2 Silver, 3 Copper");
        assert_eq!(
            loot_money_line(5, false, &get).expect("a line").text,
            "Your share of the loot is 5 Copper."
        );
    }

    use crate::ui_script::live::UiEvents;

    #[test]
    fn fish_verdict_keys_resolve_in_the_real_global_strings() {
        let mut lines: Vec<&'static str> = Vec::new();
        fish_verdict(false, &mut lines);
        fish_verdict(true, &mut lines);
        assert_eq!(lines, ["ERR_FISH_NOT_HOOKED", "ERR_FISH_ESCAPED"]);

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).expect(key);
        assert_eq!(g("ERR_FISH_NOT_HOOKED"), "No fish are hooked.");
        assert_eq!(g("ERR_FISH_ESCAPED"), "Your fish got away!");
    }

    /// The ids 12340's switch (`0x6d54b8`) pushes: a plausible wrong key fails.
    #[test]
    fn every_refusal_names_the_message_key_the_reference_pushes() {
        const TABLE: &[(u8, &str)] = &[
            (4, "ERR_LOOT_TOO_FAR"),
            (5, "ERR_LOOT_BAD_FACING"),
            (6, "ERR_LOOT_LOCKED"),
            (7, "ERR_LOOT_DIDNT_KILL"),
            (8, "ERR_LOOT_NOTSTANDING"),
            (9, "ERR_LOOT_STUNNED"),
            (10, "ERR_LOOT_PLAYER_NOT_FOUND"),
            (11, "ERR_PLAY_TIME_EXCEEDED"),
            (12, "ERR_LOOT_MASTER_INV_FULL"),
            (13, "ERR_LOOT_MASTER_UNIQUE_ITEM"),
            (14, "ERR_LOOT_MASTER_OTHER"),
            (16, "ERR_NOT_WHILE_SHAPESHIFTED"),
            // The default arm: 0 is `DIDNT_KILL`, 1 is an enum gap, and 15 shows its own line
            // only on the refused leg.
            (0, "ERR_LOOT_DIDNT_KILL"),
            (1, "ERR_LOOT_DIDNT_KILL"),
            (15, "ERR_LOOT_DIDNT_KILL"),
            (200, "ERR_LOOT_DIDNT_KILL"),
        ];
        for &(code, want) in TABLE {
            let key = loot_refusal(code).key;
            let record = benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("code {code} named {key}, which is not a catalog row"));
            assert_eq!(key, want, "code {code}");
            // A red `UIErrorsFrame` line: the record's `+0x04`.
            assert_eq!(
                record.kind,
                benilla_ui::messages::MsgKind::Error,
                "{key} is not a red line"
            );
        }

        // Four refusals are voiced in the player's race and gender: their `type_tag` (`+0x0c`) is
        // a `VocalUIEnum` line id, not the `0x44` cue sentinel.
        let voiced = |key: &str| {
            usize::from(benilla_ui::messages::by_key(key).expect(key).type_tag)
                < benilla_formats::VOCAL_UI_LINES
        };
        for key in [
            "ERR_LOOT_DIDNT_KILL",
            "ERR_LOOT_BAD_FACING",
            "ERR_LOOT_LOCKED",
            "ERR_LOOT_TOO_FAR",
        ] {
            assert!(voiced(key), "{key} lost its voice line");
        }
        // The rest are cue-tagged.
        for key in ["ERR_LOOT_NOTSTANDING", "ERR_LOOT_MASTER_OTHER"] {
            assert!(!voiced(key), "{key} unexpectedly carries a voice line");
        }
    }

    #[test]
    fn refusal_keys_resolve_to_the_real_1_12_sentences() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).expect(key);

        for (code, want) in [
            (4u8, "You are too far away to loot that corpse."),
            (5, "You must be facing the corpse to loot it."),
            (6, "Someone is already looting that corpse."),
            (7, "You don't have permission to loot that corpse."),
            (8, "You need to be standing up to loot something!"),
            (9, "You can't loot anything while stunned!"),
            (10, "Player not found"),
            (11, "Maximum play time exceeded"),
            (12, "That player's inventory is full"),
            (13, "Player has too many of that item already"),
            (14, "Can't assign item to that player"),
            // The default arm.
            (0, "You don't have permission to loot that corpse."),
            (15, "You don't have permission to loot that corpse."),
        ] {
            assert_eq!(g(loot_refusal(code).key), want, "wire code {code}");
        }
    }

    /// Codes 10 and 12..=14 answer an open window and jump past the release tail.
    #[test]
    fn only_the_window_less_refusals_drop_the_kneel() {
        // The releasing arms plus the default, which is code 7's arm.
        for code in [4u8, 5, 6, 7, 8, 9, 11, 0, 1, 2, 3, 15, 200, 255] {
            let mut latch = LootLatch(Some(CORPSE));
            let refused = loot_error(CORPSE, code, &mut latch);
            assert_eq!(latch.0, None, "code {code} should release");
            assert!(refused.line.is_some(), "code {code} shows exactly one line");
            assert!(refused.unlock, "code {code} runs the unlock");
        }
        for code in [10u8, 12, 13, 14, 16] {
            let mut latch = LootLatch(Some(CORPSE));
            assert!(!loot_error(CORPSE, code, &mut latch).unlock);
            assert_eq!(
                latch.0,
                Some(CORPSE),
                "code {code} answers an OPEN window — the kneel stays"
            );
        }
    }

    /// An error the latch does not hold takes the refused leg (`0x6d57cb`): no line but
    /// `ALREADY_PICKPOCKETED`'s, and the latch drops whatever it held (`0x6d57f8`).
    #[test]
    fn an_error_the_latch_does_not_hold_is_refused_silently() {
        for held in [Some(CHEST), None] {
            let mut latch = LootLatch(held);
            let refused = loot_error(CORPSE, 8, &mut latch);
            assert_eq!(latch.0, None, "{held:?}");
            assert_eq!(refused.line, None, "{held:?}");
        }
    }

    /// A pickpocket's `ALREADY_PICKPOCKETED` arrives on a cold latch: 12340's refused leg shows
    /// its own line and drops the latch (`0x6d57de`). On a held latch it is the default arm.
    #[test]
    fn already_pickpocketed_shows_on_the_refused_leg_only() {
        use benilla_protocol::messages::loot_error::ALREADY_PICKPOCKETED as PICKED;
        let run = |held| {
            let mut latch = LootLatch(held);
            let refused = loot_error(CORPSE, PICKED, &mut latch);
            (refused.line.expect("a line"), latch.0)
        };
        assert_eq!(run(None), ("ERR_ALREADY_PICKPOCKETED", None));
        assert_eq!(run(Some(CHEST)), ("ERR_ALREADY_PICKPOCKETED", None));
        assert_eq!(run(Some(CORPSE)), ("ERR_LOOT_DIDNT_KILL", None));
    }

    /// A GameObject guid: a chest.
    const CHEST: u64 = 0xF110_0000_0000_1234;
    /// A creature guid: a corpse.
    const CORPSE: u64 = 0xF130_0000_0000_00AB;

    /// A `NetCommands` and its receiver, to read what a handler sent.
    fn net() -> (
        crate::net::NetCommands,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (crate::net::NetCommands(tx), rx)
    }

    /// No `CMSG_LOOT` armed the latch: pickpocketing, fishing, disenchanting, skinning,
    /// prospecting and milling pass on it (`0x6d53fb`-`0x6d541f`).
    #[test]
    fn a_cold_latch_admits_a_server_started_loot_and_arms_on_it() {
        for loot_type in [2, 3, 4, 6, 7, 8] {
            let (mut world, rx) = tabbed_world();
            assert_eq!(
                world.resource::<LootLatch>().0,
                None,
                "no CMSG_LOOT was sent, so nothing armed it"
            );

            respond(&mut world, CHEST, loot_type);
            assert_eq!(
                world.resource::<LootLatch>().0,
                Some(CHEST),
                "type {loot_type}: the open window is the loot session"
            );
            assert_eq!(
                world.resource::<LootState>().source(),
                Some(CHEST),
                "type {loot_type}: …and the window opened"
            );
            assert!(
                rx.try_recv().is_err(),
                "type {loot_type}: an accepted response bounces nothing"
            );

            release(&mut world, CHEST, 1);
            assert_eq!(
                world.resource::<LootLatch>().0,
                None,
                "type {loot_type}: the release ends the session"
            );
            assert_eq!(world.resource::<LootState>().source(), None);
        }
        // Types 5 and 9 are no server-started loot: refused and bounced.
        for loot_type in [5, 9] {
            let (mut world, rx) = tabbed_world();
            respond(&mut world, CHEST, loot_type);
            assert_eq!(world.resource::<LootState>().source(), None, "{loot_type}");
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CHEST),
                "{loot_type}"
            );
        }
    }

    /// `SMSG_LOOT_RELEASE_RESPONSE` after a response, as AzerothCore's release-first sends a
    /// reopened guid (#216): the release drops the latch, so the response that follows meets a
    /// cold latch and is refused, as 12340's (`0x6d59e0`, then `0x6d53d4`).
    #[test]
    fn a_release_before_the_response_on_the_same_guid_refuses_the_response() {
        let (mut world, rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE); // the `CMSG_LOOT` send
        release(&mut world, CORPSE, 1);
        assert_eq!(world.resource::<LootLatch>().0, None);
        respond(&mut world, CORPSE, 1);
        assert_eq!(world.resource::<LootState>().source(), None);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the refused response is released back"
        );
    }

    /// Only a nonzero result on the latched guid acts (`0x6d5a03`-`0x6d5a1d`): another guid's
    /// release, or a zero result, leaves the open window and its latch.
    #[test]
    fn a_release_acts_only_on_the_latched_guid_with_a_nonzero_result() {
        for (guid, result) in [(CHEST, 1), (CORPSE, 0)] {
            let (mut world, _rx) = tabbed_world();
            world.resource_mut::<LootLatch>().0 = Some(CORPSE);
            respond(&mut world, CORPSE, 1);
            release(&mut world, guid, result);
            assert_eq!(
                world.resource::<LootLatch>().0,
                Some(CORPSE),
                "{guid:#x}/{result}"
            );
            assert_eq!(world.resource::<LootState>().source(), Some(CORPSE));
        }
    }

    /// An admitted response for an object we do not hold (`0x6d560f`-`0x6d5665`):
    /// `ERR_LOOT_DIDNT_KILL`, a release, no window, and the latch as it was.
    #[test]
    fn a_response_for_an_object_we_do_not_hold_is_released_with_didnt_kill() {
        const GONE: u64 = 0xF130_0000_0900_0001;
        let (mut world, rx) = tabbed_world();
        world.init_resource::<crate::ui_chat::ChatLog>();
        world.init_resource::<crate::sound::MessageSounds>();
        let script = benilla_ui::script::UiScript::new().expect("VM");
        script
            .run(
                "ERR_LOOT_DIDNT_KILL = 'No permission.' SHOWN = {} local f = CreateFrame('Frame') \
                 f:RegisterEvent('UI_ERROR_MESSAGE') \
                 f:SetScript('OnEvent', function(_, _, text) SHOWN[#SHOWN + 1] = text end)",
            )
            .unwrap();
        world.insert_non_send_resource(script);
        world.resource_mut::<LootLatch>().0 = Some(GONE);
        respond(&mut world, GONE, 1);
        assert_eq!(world.resource::<LootState>().source(), None);
        assert_eq!(world.resource::<LootLatch>().0, Some(GONE));
        let shown: String = world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval("return table.concat(SHOWN, ',')")
            .unwrap();
        assert_eq!(shown, "No permission.", "shown in the handler's turn");
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == GONE));
    }

    /// The count clamps to 18 rows (`0x6d55e7`).
    #[test]
    fn a_response_keeps_at_most_eighteen_rows() {
        let (mut world, _rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        let items = (0..20)
            .map(|slot| benilla_protocol::messages::LootItem {
                slot,
                item_id: 117,
                count: 1,
                display_info_id: 1117,
                random_property_id: 0,
                random_suffix: 0,
                slot_type: 0,
            })
            .collect();
        world
            .run_system_once_with(
                on_response,
                SessionEvent::LootResponse {
                    guid: CORPSE,
                    loot_type: 1,
                    gold: 0,
                    items,
                },
            )
            .expect("the handler runs as a one-shot system");
        crate::ui_script::live::fire_handler_events(&mut world);
        assert_eq!(world.resource::<LootState>().items.len(), 18);
    }

    /// A release response through the registered handler.
    fn release(world: &mut World, guid: u64, result: u8) {
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse { guid, result },
            )
            .expect("the handler runs as a one-shot system");
    }

    /// The `CMSG_LOOT` send armed the same guid: the match branch.
    #[test]
    fn a_matching_latch_admits_any_loot_type() {
        let (mut world, _rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE); // the `CMSG_LOOT` send
        respond(&mut world, CORPSE, 1);
        assert_eq!(world.resource::<LootLatch>().0, Some(CORPSE));
        assert_eq!(world.resource::<LootState>().source(), Some(CORPSE));
    }

    /// The refusal arm: a type-1 answer on a cold latch answers nothing we asked.
    #[test]
    fn a_cold_latch_refuses_a_corpse_typed_response_and_bounces_it() {
        let (mut world, rx) = tabbed_world();

        respond(&mut world, CORPSE, 1);
        assert_eq!(
            world.resource::<LootState>().source(),
            None,
            "no window for an unasked-for corpse answer"
        );
        assert_eq!(
            world.resource::<LootLatch>().0,
            None,
            "…and nothing latched"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the refusal releases the object the PACKET named"
        );
    }

    /// The refusal's clear is not guid-matched: a response for B drops a latch on A.
    #[test]
    fn a_refusal_drops_whatever_latch_was_live_not_just_a_matching_one() {
        let (mut world, rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);

        respond(&mut world, CHEST, 1);
        assert_eq!(
            world.resource::<LootLatch>().0,
            None,
            "A's latch is dropped by B's refusal"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CHEST),
            "…and it is B that gets released"
        );
    }

    /// An item guid (`HIGHGUID_ITEM` 0x4000 in the high word): a lockbox.
    const LOCKBOX: u64 = 0x4000_0000_0000_0007;

    /// A world with the lockbox at bag 0 slot 3 locked, as its `CMSG_OPEN_ITEM` send leaves it.
    fn opened_lockbox_world() -> World {
        let mut world = World::new();
        world.init_resource::<LootState>();
        world.insert_resource(net().0);
        world.insert_resource(LootLatch(Some(LOCKBOX)));
        world.init_resource::<UiEvents>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>();
        let mut pending = PendingItemOps::default();
        pending.lock(LOCKBOX);
        world.insert_resource(pending);
        world
    }

    /// With loot left, no field update ever resolves the lock: the release's `CloseLoot` on the
    /// item's open window is the clear (`0x5236fc`).
    #[test]
    fn a_release_for_the_opened_item_unlocks_its_slot() {
        let mut world = opened_lockbox_world();
        world
            .resource_mut::<LootState>()
            .open_for_test(LOCKBOX, 1, 0, Vec::new());
        release(&mut world, LOCKBOX, 1);
        assert_eq!(
            world.resource::<UiEvents>().names(),
            ["<world>", "LOOT_CLOSED"],
            "the close's unlock in the handler's turn ahead of the window's let-go (`0x5236fc`, `0x52374c`)"
        );
        crate::ui_script::live::fire_handler_events(&mut world);
        assert!(
            !world.resource::<PendingItemOps>().is_locked(LOCKBOX),
            "the lockbox is no longer grey and locked"
        );
    }

    #[test]
    fn a_release_for_another_guid_leaves_the_lock() {
        let mut world = opened_lockbox_world();
        world
            .resource_mut::<LootState>()
            .open_for_test(LOCKBOX, 1, 0, Vec::new());
        release(&mut world, CORPSE, 1);
        assert!(world.resource::<PendingItemOps>().is_locked(LOCKBOX));
        assert!(world.resource::<UiEvents>().is_empty());
    }

    /// The server's release closes the window through the shared close, sending nothing back, and
    /// its last step deselects the dead unit it closed on.
    #[test]
    fn a_release_response_closes_the_window_and_deselects_a_dead_unit() {
        let (net, rx) = net();
        let mut world = opened_lockbox_world();
        world.insert_resource(net);
        let corpse = world
            .spawn((
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                // `UNIT_FIELD_HEALTH` (22) at 0.
                crate::net::ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_UNIT_HEALTH,
                    0,
                )])),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(CORPSE, corpse);
        world
            .resource_mut::<LootState>()
            .open_for_test(CORPSE, 1, 0, Vec::new());
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);

        release(&mut world, CORPSE, 1);
        assert_eq!(world.resource::<LootState>().source(), None);
        assert_eq!(world.resource::<LootLatch>().0, None);
        assert!(rx.try_recv().is_err(), "the server's release is not echoed");
        let asked: Vec<u64> = world
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect();
        assert_eq!(asked, [CORPSE]);

        // A second release finds nothing open and asks nothing.
        release(&mut world, CORPSE, 1);
        assert!(world
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .next()
            .is_none());
    }

    /// A live creature: `TYPEMASK_OBJECT | TYPEMASK_UNIT`, so it is the Tab target.
    const KOBOLD: u64 = 0xF130_0000_0600_00CD;
    /// A creature flagged `UNIT_FLAG_NOT_SELECTABLE`.
    const UNSELECTABLE: u64 = 0xF130_0000_0700_00EF;

    const OBJECT_FIELD_TYPE: u16 = benilla_protocol::field::FIELD_OBJECT_TYPE;
    const OBJECT_FIELD_ENTRY: u16 = benilla_protocol::field::FIELD_OBJECT_ENTRY;
    /// `OBJECT_FIELD_TYPE` masks: `TYPEMASK_OBJECT` with `UNIT`, `PLAYER` or `GAMEOBJECT`.
    const TYPE_UNIT: u32 = 0x09;
    const TYPE_PLAYER: u32 = 0x19;
    const TYPE_GAMEOBJECT: u32 = 0x21;
    /// `UNIT_FLAG_NOT_SELECTABLE`.
    const NOT_SELECTABLE: u32 = 1 << 25;
    const KOBOLD_ENTRY: u32 = 6;
    const CORPSE_ENTRY: u32 = 257;

    /// Us, the Tab target selected, and a corpse, a chest and an unselectable unit beside it, all
    /// streamed: the issue's spot, with what [`on_response`] reads and the selection commit writes.
    fn tabbed_world() -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let mut world = World::new();
        let rx = seat_tabbed(&mut world);
        (world, rx)
    }

    /// [`tabbed_world`]'s seating, into any world.
    fn seat_tabbed(world: &mut World) -> crossbeam_channel::Receiver<ClientCommand> {
        use crate::net::{Guid, GuidIndex, NetEntity, ObjectStore, SelfPlayer};
        use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH};
        use benilla_protocol::messages::{ObjectFields, ObjectType};
        use benilla_protocol::EntityKind::{GameObject, Unit};
        let unit = |kind, type_mask, pairs: &[(u16, u32)]| {
            let mut fields = vec![(OBJECT_FIELD_TYPE, type_mask)];
            fields.extend_from_slice(pairs);
            (
                NetEntity {
                    kind,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore(ObjectFields::from_pairs(&fields).into_created(match kind {
                    GameObject => ObjectType::GameObject,
                    _ => ObjectType::Unit,
                })),
            )
        };
        let (net, rx) = net();
        world.insert_resource(net);
        world.init_resource::<LootState>();
        world.init_resource::<LootLatch>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<bevy::ecs::message::Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<GuidIndex>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::target::AssistAttack>();
        world.init_resource::<crate::target::Selection>();
        world.init_resource::<PendingItemOps>();
        world.init_resource::<UiEvents>();
        world.init_resource::<crate::items::Items>();
        world.insert_non_send_resource(crate::items::RecordWaits::default());
        world.init_resource::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>();
        world.init_resource::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>();
        world.init_resource::<crate::player::Player>();
        world.init_resource::<super::super::LootMoveStart>();
        world.spawn((
            SelfPlayer,
            Guid(1),
            unit(Unit, TYPE_PLAYER, &[(FIELD_UNIT_HEALTH, 100)]),
        ));
        for (guid, bundle) in [
            (
                KOBOLD,
                unit(
                    Unit,
                    TYPE_UNIT,
                    &[(OBJECT_FIELD_ENTRY, KOBOLD_ENTRY), (FIELD_UNIT_HEALTH, 100)],
                ),
            ),
            (
                CORPSE,
                unit(
                    Unit,
                    TYPE_UNIT,
                    &[(OBJECT_FIELD_ENTRY, CORPSE_ENTRY), (FIELD_UNIT_HEALTH, 0)],
                ),
            ),
            (CHEST, unit(GameObject, TYPE_GAMEOBJECT, &[])),
            (
                UNSELECTABLE,
                unit(
                    Unit,
                    TYPE_UNIT,
                    &[(FIELD_UNIT_HEALTH, 0), (FIELD_UNIT_FLAGS, NOT_SELECTABLE)],
                ),
            ),
        ] {
            let e = world.spawn((Guid(guid), bundle)).id();
            world.resource_mut::<GuidIndex>().0.insert(guid, e);
        }
        let kobold = world.resource::<GuidIndex>().0[&KOBOLD];
        let mut selection = world.resource_mut::<crate::target::Selection>();
        selection.target = Some(kobold);
        selection.guid = Some(KOBOLD);
        rx
    }

    /// A response for `guid` of `loot_type` with no rows, through the registered handler.
    fn respond(world: &mut World, guid: u64, loot_type: u8) {
        world
            .run_system_once_with(
                on_response,
                SessionEvent::LootResponse {
                    guid,
                    loot_type,
                    gold: 0,
                    items: Vec::new(),
                },
            )
            .expect("the handler runs as a one-shot system");
        crate::ui_script::live::fire_handler_events(world);
    }

    /// The selection set on the looted unit's guid.
    #[test]
    fn a_loot_window_opening_on_a_unit_selects_it() {
        let (mut world, rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE); // the `CMSG_LOOT` send
        respond(&mut world, CORPSE, 1);
        assert_eq!(world.resource::<LootState>().source(), Some(CORPSE));
        let selection = world.resource::<crate::target::Selection>();
        assert_eq!(
            selection.guid,
            Some(CORPSE),
            "the looted body is the target"
        );
        assert_eq!(
            selection.last,
            Some(KOBOLD),
            "…through the selection set's stamp"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [ClientCommand::SetSelection { guid }] if guid == CORPSE),
            "{sent:?}"
        );

        // The setter's dedup: a second window on the selected body sends nothing.
        respond(&mut world, CORPSE, 1);
        assert!(rx.try_recv().is_err());
    }

    /// The reference tests `TYPEMASK_UNIT`; the selection set's selectable test refuses the flagged
    /// unit; a refused response never reaches the select.
    #[test]
    fn a_chest_an_unselectable_unit_and_a_refusal_leave_the_target() {
        for (guid, latched, loot_type) in [
            (CHEST, true, 1),
            (UNSELECTABLE, true, 1),
            (CORPSE, false, 1),
        ] {
            let (mut world, rx) = tabbed_world();
            world.resource_mut::<LootLatch>().0 = latched.then_some(guid);
            respond(&mut world, guid, loot_type);
            assert_eq!(
                world.resource::<crate::target::Selection>().guid,
                Some(KOBOLD),
                "{guid:#x}"
            );
            assert!(
                !rx.try_iter()
                    .any(|c| matches!(c, ClientCommand::SetSelection { .. })),
                "{guid:#x}"
            );
        }
    }

    /// The selection comes before the window: the outgoing target's teardown closes the window
    /// still open on it, its release ahead of the new selection.
    #[test]
    fn the_selection_tears_down_the_old_targets_window_before_the_new_one_opens() {
        let (mut world, rx) = tabbed_world();
        world
            .resource_mut::<LootState>()
            .open_for_test(KOBOLD, 2, 0, Vec::new());
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::LootRelease { guid: a }, ClientCommand::SetSelection { guid: b }]
                    if a == KOBOLD && b == CORPSE
            ),
            "{sent:?}"
        );
        assert_eq!(world.resource::<LootState>().source(), Some(CORPSE));
        // [`LootLatch`]'s deviation: the reference's teardown zeroes it here.
        assert_eq!(world.resource::<LootLatch>().0, Some(CORPSE));
    }

    /// The selection set's switch while swinging: the swing at the Tab target stops, and none opens
    /// on the body.
    #[test]
    fn looting_while_swinging_at_the_tab_target_stops_the_swing() {
        use crate::net::SelfPlayer;
        let (mut world, rx) = tabbed_world();
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .expect("us");
        world
            .entity_mut(me)
            .insert(crate::creature_anim::Engaged(KOBOLD));
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AttackStop, ClientCommand::SetSelection { guid }] if guid == CORPSE
            ),
            "{sent:?}"
        );
    }

    /// `StartAttack`'s target gate: a body is no swing target even when hostile, so looting one
    /// mid-swing stops and selects, and swings at nothing.
    #[test]
    fn looting_a_hostile_body_while_swinging_never_swings_at_it() {
        use crate::net::{ObjectStore, SelfPlayer};
        use benilla_protocol::field::{FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_HEALTH};
        use benilla_protocol::messages::{ObjectFields, ObjectType};
        const UNIT_FIELD_BYTES_0: u16 = benilla_protocol::field::FIELD_UNIT_BYTES_0;
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let (factions, stormwind, reps) = crate::target::stormwind_fixture(&mut chain, 0);
        let me_store = ObjectStore(
            ObjectFields::from_pairs(&[
                (OBJECT_FIELD_TYPE, TYPE_PLAYER),
                (FIELD_UNIT_HEALTH, 100),
                (FIELD_UNIT_FACTIONTEMPLATE, stormwind),
                (UNIT_FIELD_BYTES_0, crate::target::HUMAN_WARRIOR),
            ])
            .into_created(ObjectType::Player),
        );
        let creature = |template, health| {
            ObjectStore(
                ObjectFields::from_pairs(&[
                    (OBJECT_FIELD_TYPE, TYPE_UNIT),
                    (FIELD_UNIT_HEALTH, health),
                    (FIELD_UNIT_FACTIONTEMPLATE, template),
                ])
                .into_created(ObjectType::Unit),
            )
        };
        let hostile = (1u32..4096)
            .find(|&t| {
                crate::target::can_attack(
                    Some(&creature(t, 100)),
                    Some(&factions),
                    &reps,
                    &crate::target::NoGroup,
                    Some(&me_store),
                )
            })
            .expect("a template a Stormwind human can attack");

        let (mut world, rx) = tabbed_world();
        world.insert_resource(factions);
        world.insert_resource(reps);
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .expect("us");
        world
            .entity_mut(me)
            .insert((me_store, crate::creature_anim::Engaged(KOBOLD)));
        let corpse = world.resource::<crate::net::GuidIndex>().0[&CORPSE];
        world.entity_mut(corpse).insert(creature(hostile, 0));
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AttackStop, ClientCommand::SetSelection { guid }] if guid == CORPSE
            ),
            "{sent:?}"
        );
    }

    /// [`tabbed_world`] under the stock UI, with the unit feed and both creatures named.
    fn stock_app() -> App {
        let s = crate::ui_script::test_ui::stock_ui(1024.0, 768.0);

        let mut app = App::new();
        seat_tabbed(app.world_mut());
        seat_a_real_player(app.world_mut());
        let mut names = crate::names::NameCache::default();
        for (entry, name) in [
            (KOBOLD_ENTRY, "Kobold Vermin"),
            (CORPSE_ENTRY, "Kobold Worker"),
        ] {
            names.insert_creature(
                entry,
                Some(crate::names::CreatureRecord {
                    cursor_name: None,
                    name: name.into(),
                    subname: None,
                    creature_type: 7,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                    ..Default::default()
                }),
            );
        }
        app.insert_resource(names)
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>()
            .init_resource::<crate::ui_guild::GuildCache>()
            .insert_non_send_resource(s);
        crate::ui_unit::add_unit_feed(&mut app);
        app
    }

    /// Our body as a server sends it: a race, whose side `UnitFactionGroup` reads off the faction
    /// catalog, and rest state 2, Normal; stock `PLAYER_ENTERING_WORLD` handlers read both
    /// unguarded (`GetRestState` is nil on 0).
    fn seat_a_real_player(world: &mut World) {
        let data = benilla_formats::wow_data().expect("the callers gate on the install");
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("Faction.dbc");
        world.insert_resource(crate::target::Factions::from_catalog(catalog));
        use crate::net::{ObjectStore, SelfPlayer};
        use benilla_protocol::field::FIELD_UNIT_HEALTH;
        use benilla_protocol::messages::{ObjectFields, ObjectType};
        const UNIT_FIELD_BYTES_0: u16 = benilla_protocol::field::FIELD_UNIT_BYTES_0;
        const PLAYER_BYTES_2: u16 = benilla_protocol::field::FIELD_PLAYER_BYTES_2;
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(world)
            .expect("our body");
        world.entity_mut(me).insert(ObjectStore(
            ObjectFields::from_pairs(&[
                (OBJECT_FIELD_TYPE, TYPE_PLAYER),
                (FIELD_UNIT_HEALTH, 100),
                (UNIT_FIELD_BYTES_0, crate::target::HUMAN_WARRIOR),
                (PLAYER_BYTES_2, 2 << 24),
            ])
            .into_created(ObjectType::Player),
        ));
    }

    /// The name on the stock `TargetFrame`, empty while it is hidden.
    fn target_frame_name(app: &App) -> String {
        let s = app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>();
        assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
        s.eval("return TargetFrame:IsShown() and TargetFrame.name:GetText() or ''")
            .expect("the target frame's name")
    }

    /// The issue's symptom on the stock frames: the Tab target is on `TargetFrame` when the body's
    /// window opens, and the unit feed then names the body there (`TargetFrame.lua:63`,
    /// `UnitFrame.lua:25`).
    #[test]
    fn the_stock_target_frame_names_the_looted_body() {
        benilla_formats::wow_data_or_skip!();
        let mut app = stock_app();

        app.update();
        assert_eq!(target_frame_name(&app), "Kobold Vermin", "the Tab target");

        app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(app.world_mut(), CORPSE, 1);
        app.update();
        assert_eq!(target_frame_name(&app), "Kobold Worker", "the looted body");
    }

    /// What [`super::super::drain_loot`] reads beside [`seat_tabbed`]'s, with our body's movement
    /// word at `player`'s.
    fn seat_drain(world: &mut World, player: crate::player::Player) {
        world.insert_resource(player);
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<crate::items::Items>();
        world.init_resource::<bevy::ecs::message::Messages<crate::sound::LootPickupSound>>();
    }

    /// The deselects asked for since the last call.
    fn deselects(world: &mut World) -> Vec<u64> {
        world
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect()
    }

    /// A response on the corpse with `player` seated, then one drain.
    fn open_while(
        player: crate::player::Player,
    ) -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut world, rx) = tabbed_world();
        seat_drain(&mut world, player);
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        world
            .run_system_once(super::super::drain_loot)
            .expect("the drain runs as a one-shot system");
        (world, rx)
    }

    /// The response's tail (`0x6d57a6`-`0x6d57b5`): a bit of our movement word's low byte, forward,
    /// back, strafe, turn or pitch, takes the move-start close on the window it just opened, the
    /// release after the selection and the dead body's deselect last, inside the close after its
    /// release (`0x523754`, `0x5237ae`), its `CMSG_SET_SELECTION` 0 on the wire behind the release.
    #[test]
    fn a_window_that_opens_while_moving_closes_as_a_movement_start_does() {
        for bit in (0..8).map(|b| 1u32 << b) {
            let (mut world, rx) = open_while(crate::player::Player::with_move_flags(bit));
            assert_eq!(world.resource::<LootState>().source(), None, "{bit:#x}");
            assert_eq!(world.resource::<LootLatch>().0, None, "{bit:#x}");
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(
                    sent[..],
                    [
                        ClientCommand::SetSelection { guid: a },
                        ClientCommand::LootRelease { guid: b },
                        ClientCommand::SetSelection { guid: 0 },
                    ] if a == CORPSE && b == CORPSE
                ),
                "{bit:#x}: {sent:?}"
            );
            assert!(
                deselects(&mut world).is_empty(),
                "{bit:#x}: applied, not asked"
            );
        }
    }

    /// Only that byte, and only our body's: walking, falling or swimming alone leaves the window
    /// open, as does a step of the creature we possess.
    #[test]
    fn a_window_opened_falling_swimming_or_possessing_stays_open() {
        use crate::creature_anim::move_flags as f;
        use crate::player::Player;
        let mut possessing = Player::with_move_flags(f::FORWARD);
        possessing.foreign_mover = Some(KOBOLD);
        for (what, player) in [
            ("still", Player::with_move_flags(0)),
            ("walk mode", Player::with_move_flags(f::WALK_MODE)),
            ("falling", Player::with_move_flags(f::FALLING)),
            ("swimming", Player::with_move_flags(f::SWIMMING)),
            ("possessing", possessing),
        ] {
            let (mut world, rx) = open_while(player);
            assert_eq!(
                world.resource::<LootState>().source(),
                Some(CORPSE),
                "{what}"
            );
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(sent[..], [ClientCommand::SetSelection { guid }] if guid == CORPSE),
                "{what}: {sent:?}"
            );
            assert!(deselects(&mut world).is_empty(), "{what}");
        }
    }

    /// The window's object set (`0x5896b0`) sweeps an armed auto-loot window (`0x588f90`) before
    /// the response's movement close (`0x6d57b5`), so a body auto-looted on the run is taken, then
    /// released.
    #[test]
    fn auto_loot_on_the_run_takes_the_rows_before_the_release() {
        const TOUGH_JERKY: u32 = 117;
        let mut app = App::new();
        let rx = seat_tabbed(app.world_mut());
        seat_drain(
            app.world_mut(),
            crate::player::Player::with_move_flags(crate::creature_anim::move_flags::FORWARD),
        );
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(
                TOUGH_JERKY,
                Some(crate::items::test_template("Tough Jerky")),
            );
        app.world_mut().resource_mut::<LootState>().auto_loot = true; // armed at the right-click
        app.init_resource::<super::super::LootConfig>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_non_send_resource(benilla_ui::script::UiScript::new().expect("VM"))
            .add_systems(Update, super::super::drain_loot);

        app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        app.world_mut()
            .run_system_once_with(
                on_response,
                SessionEvent::LootResponse {
                    guid: CORPSE,
                    loot_type: 1,
                    gold: 0,
                    items: vec![benilla_protocol::messages::LootItem {
                        slot: 0,
                        item_id: TOUGH_JERKY,
                        count: 1,
                        display_info_id: 1117,
                        random_property_id: 0,
                        random_suffix: 0,
                        slot_type: 0,
                    }],
                },
            )
            .expect("the handler runs as a one-shot system");
        crate::ui_script::live::fire_handler_events(app.world_mut());
        app.update();
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::SetSelection { .. },
                    ClientCommand::AutostoreLootItem { slot: 0 },
                    ClientCommand::LootRelease { guid },
                    // The dead body's deselect inside the close (`0x5237ae`).
                    ClientCommand::SetSelection { guid: 0 },
                ] if guid == CORPSE
            ),
            "{sent:?}"
        );
    }

    /// The issue's spot on the stock frames: a body's window that opens on the run is shown by
    /// `LOOT_OPENED` and hidden by `LOOT_CLOSED` in the same frame (`LootFrame.lua:13-16`,
    /// `:53-55`), its one release
    /// sent, and `TargetFrame` lets the body go; standing, the window and the target stay.
    #[test]
    fn the_stock_loot_frame_closes_on_a_body_looted_on_the_run() {
        use crate::creature_anim::move_flags as f;
        use benilla_ui::script::UiScript;
        benilla_formats::wow_data_or_skip!();
        for (flags, stays) in [(f::FORWARD, false), (0, true)] {
            let mut app = stock_app();
            let (tx, rx) = crossbeam_channel::unbounded();
            app.insert_resource(crate::net::NetCommands(tx));
            seat_drain(
                app.world_mut(),
                crate::player::Player::with_move_flags(flags),
            );
            app.add_message::<crate::target::click::ArrivalAttack>()
                .init_resource::<super::super::LootConfig>()
                .init_resource::<ButtonInput<KeyCode>>()
                .add_systems(
                    Update,
                    (
                        super::super::drain_loot,
                        crate::target::click::clear_target_requests,
                    )
                        .chain(),
                );
            app.world()
                .non_send_resource::<UiScript>()
                .run(
                    "LOOT_EVENTS = {} local f = CreateFrame('Frame') \
                     f:RegisterEvent('LOOT_OPENED') f:RegisterEvent('LOOT_CLOSED') \
                     f:SetScript('OnEvent', function() tinsert(LOOT_EVENTS, event) end)",
                )
                .unwrap();
            app.update();

            app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
            respond(app.world_mut(), CORPSE, 1);
            // One frame: the reference opens and closes in the response's handler.
            app.update();
            let (events, shown): (String, bool) = app
                .world()
                .non_send_resource::<UiScript>()
                .eval("return table.concat(LOOT_EVENTS, ','), LootFrame:IsShown() == 1")
                .unwrap();
            for _ in 0..2 {
                app.update();
            }
            let releases = rx
                .try_iter()
                .filter(|c| matches!(c, ClientCommand::LootRelease { guid } if *guid == CORPSE))
                .count();
            if stays {
                assert_eq!(events, "LOOT_OPENED");
                assert!(shown, "standing, the window stays");
                assert_eq!(releases, 0);
                assert_eq!(target_frame_name(&app), "Kobold Worker");
            } else {
                assert_eq!(events, "LOOT_OPENED,LOOT_CLOSED");
                assert!(
                    !shown,
                    "on the run, the window closes in the frame it opens"
                );
                assert_eq!(releases, 1, "released once");
                assert_eq!(target_frame_name(&app), "", "and the body is let go");
            }
        }
    }

    /// A response on an item we hold locks it (`0x6d56af`) ahead of the set, so a server-started
    /// window on a bag item shows its slot grey.
    #[test]
    fn a_response_on_an_item_locks_it() {
        let (mut world, _rx) = tabbed_world();
        let e = world.spawn_empty().id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(LOCKBOX, e);
        world.resource_mut::<LootLatch>().0 = Some(LOCKBOX);
        respond(&mut world, LOCKBOX, 4);
        assert!(world.resource::<PendingItemOps>().is_locked(LOCKBOX));
        assert_eq!(world.resource::<LootState>().source(), Some(LOCKBOX));
    }

    /// A removal acts only while the latched object is held (`0x6d588c`).
    #[test]
    fn a_removal_with_no_latched_object_held_clears_nothing() {
        let (mut world, _rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        world
            .resource_mut::<LootState>()
            .slot_changed_for_test(CORPSE, 0, 117);
        world.resource_mut::<LootLatch>().0 = None;
        world
            .run_system_once_with(on_removed, SessionEvent::LootRemoved { slot: 0 })
            .expect("the handler runs as a one-shot system");
        assert!(world.resource::<UiEvents>().is_empty(), "no clear raised");
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        world
            .run_system_once_with(on_removed, SessionEvent::LootRemoved { slot: 0 })
            .expect("the handler runs as a one-shot system");
        assert_eq!(world.resource::<UiEvents>().names()[0], "LOOT_SLOT_CLEARED");
    }

    /// Only a releasing arm reaches the item unlock, through the tail.
    #[test]
    fn a_releasing_loot_error_on_the_opened_item_unlocks_it() {
        let mut world = opened_lockbox_world();
        // PLAYER_NOT_FOUND (10) does not reach the tail: nothing unlocks.
        world
            .run_system_once_with(
                on_error,
                SessionEvent::LootError {
                    guid: LOCKBOX,
                    error: 10,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert!(world.resource::<PendingItemOps>().is_locked(LOCKBOX));
        // TOO_FAR (4) does.
        world
            .run_system_once_with(
                on_error,
                SessionEvent::LootError {
                    guid: LOCKBOX,
                    error: 4,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert!(!world.resource::<PendingItemOps>().is_locked(LOCKBOX));
        // Each error's line in the handler's turn, then the unlock's signals and the bar's usable
        // update (`0x51379b`).
        assert_eq!(
            world.resource::<UiEvents>().names(),
            ["<world>", "<world>", "<world>", "<world>"]
        );
    }

    /// `GetMasterLootCandidate` on a name not cached asks it and leaves the list's callback, one
    /// per miss (`0x58899d`); a found answer signals `UPDATE_MASTER_LOOT_LIST` for each
    /// (`0x588150`), and a denied one nothing.
    #[test]
    fn a_master_candidates_name_signals_the_list_when_it_lands() {
        const BOB: u64 = 0x0000_0000_0000_0B0B;
        let mut world = World::new();
        world.init_resource::<LootState>();
        world.init_resource::<UiEvents>();
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<crate::names::NameCache>();
        world.init_resource::<crate::items::Items>();
        let (net, rx) = net();
        world.insert_resource(net);
        {
            let mut loot = world.resource_mut::<LootState>();
            loot.set_master_candidates(vec![BOB]);
            loot.open_for_test(CORPSE, 1, 0, Vec::new());
        }
        let ask = |world: &mut World| {
            world
                .run_system_once(|mut live: super::super::LootLive| live.master_candidate(0))
                .expect("the read runs as a one-shot system")
        };
        assert_eq!(ask(&mut world), None, "nil while the name is asked");
        assert_eq!(ask(&mut world), None);
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, ClientCommand::NameQuery { guid } if guid == BOB)),
            "the name is asked"
        );
        let answer = |world: &mut World, ev: SessionEvent| {
            world
                .run_system_once_with(on_name_answer, ev)
                .expect("the handler runs as a one-shot system");
        };
        answer(
            &mut world,
            SessionEvent::PlayerNameMiss {
                guid: 0xDEAD,
                miss: benilla_protocol::messages::NameQueryMiss::Deny,
            },
        );
        assert!(world.resource::<UiEvents>().is_empty(), "another name");
        answer(
            &mut world,
            SessionEvent::PlayerName {
                guid: BOB,
                name: "Bob".into(),
                realm: String::new(),
                race: 1,
                class: 1,
                gender: 0,
                declensions: None,
            },
        );
        assert_eq!(
            world.resource::<UiEvents>().names(),
            ["UPDATE_MASTER_LOOT_LIST", "UPDATE_MASTER_LOOT_LIST"],
            "one per miss"
        );
    }
}
