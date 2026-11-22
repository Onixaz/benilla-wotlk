//! The trade's packet handlers: `SMSG_TRADE_STATUS` drives the open, accept and close state and
//! `SMSG_TRADE_STATUS_EXTENDED` replaces one side's offer, both into [`TradeSession`]; each then
//! makes the window's writes and raises its events and lines through [`UiEvents`] at 12340's step
//! (`0x7044a0` -> `0x5873e0`, `0x704680` -> `0x586dc0`).

use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::{OfferUpdate, TradeSession};
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore};
use crate::ui_action::signals::SignalSink;
use crate::ui_action::{FillArg, UiError};
use crate::ui_script::live::UiEvents;

/// Register the trade's handlers: one per kind, the name answers, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TradeStatus, on_trade_status)
        .net_handler(K::TradeStatusExtended, on_trade_status_extended)
        .net_handler(K::PlayerName, on_name_answer)
        .net_handler(K::PlayerNameMiss, on_name_answer)
        .net_handler(K::Disconnected, on_session_end);
}

/// The status handler's reads, bundled under Bevy's parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
struct StatusReads<'w, 's> {
    commands: Res<'w, NetCommands>,
    items: Option<Res<'w, Items>>,
    limits: Option<Res<'w, super::ItemLimitCategories>>,
    names: Option<Res<'w, NameCache>>,
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
}

fn on_trade_status(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    reads: StatusReads,
    ui: Option<ResMut<UiEvents>>,
) {
    let SessionEvent::TradeStatus { status } = ev else {
        return;
    };
    use crate::ui_session::NpcSession;
    // The partner setter takes the interact target on the open (`0x5874cb` -> `0x587124`) and clears it on the close (`0x5870f7`).
    let partner = match status {
        TradeStatus::OpenWindow { .. } => trade.partner,
        TradeStatus::Canceled | TradeStatus::Complete | TradeStatus::CloseWindow { .. } => {
            trade.npc()
        }
        _ => None,
    };
    let use_cursor = trade.opens_with_cursor();
    // A naming arm reads the guid cells before the tail clears them.
    let naming = trade.line_names();
    let close_line = match status {
        TradeStatus::CloseWindow {
            result,
            target,
            item,
        } => {
            // The item cache alone, never asked (`0x58757c`).
            let limit = |entry: u32| {
                let category = reads
                    .items
                    .as_deref()?
                    .template_cached(entry)?
                    .item_limit_category;
                let row = reads.limits.as_deref()?.0.get(category)?;
                Some((row.quantity, row.name.clone()))
            };
            close_window_line(result, target, item, &limit)
        }
        _ => None,
    };
    trade_status(status, &mut trade, &reads.commands);
    let Some(mut ui) = ui else {
        return;
    };

    // The packet's own arms (`0x7044a0`): our offered coins zero on the open and on each close
    // (`0x704545`, `0x70459f`, `0x7045eb`) and on the coins' refusal (`0x70457c`); codes 3 and 13
    // signal `PLAYER_MONEY` (`0x7045ab`).
    let zeroes = match status {
        TradeStatus::OpenWindow { .. }
        | TradeStatus::Canceled
        | TradeStatus::Complete
        | TradeStatus::CloseWindow { .. }
        | TradeStatus::Unknown13 => true,
        TradeStatus::OnlyConjured { slot } | TradeStatus::NotEligible { slot } => slot == 0xff,
        _ => false,
    };
    if zeroes {
        ui.write(|script| script.set_trade_offer_gold(0));
    }
    if matches!(status, TradeStatus::Canceled | TradeStatus::Unknown13) {
        ui.fire("PLAYER_MONEY", vec![]);
    }

    // The window's arms (`0x5873e0`), each at its step.
    match status {
        TradeStatus::OpenWindow { .. } => {
            if let Some(partner) = partner {
                ui.world(move |w| crate::ui_session::set_interact_target(w, partner));
                ui.write(move |script| script.open_trade_partner(partner, use_cursor));
            }
        }
        // `TRADE_REQUEST_CANCEL`, the unlock, the close, then the line (`0x587503`-`0x58751d`).
        TradeStatus::Canceled => {
            ui.fire("TRADE_REQUEST_CANCEL", vec![]);
            ui.write(UiScript::unlock_trade_items);
            close_partner(&mut ui, partner, false);
            ui.line(UiError::key("ERR_TRADE_CANCELLED"));
        }
        // The unlock, the close keeping the completion, then the line (`0x587529`-`0x58753e`).
        TradeStatus::Complete => {
            ui.write(UiScript::unlock_trade_items);
            close_partner(&mut ui, partner, true);
            ui.line(UiError::key("ERR_TRADE_COMPLETE"));
        }
        // The unlock, the line, then the close (`0x58754a`, `0x5875bd`, `0x5875cb`).
        TradeStatus::CloseWindow { .. } => {
            ui.write(UiScript::unlock_trade_items);
            if let Some(line) = close_line {
                ui.line(line);
            }
            close_partner(&mut ui, partner, false);
        }
        TradeStatus::Accept => ui.write(|script| script.set_trade_target_accepted(true)),
        TradeStatus::Rejected => ui.write(|script| script.set_trade_target_accepted(false)),
        TradeStatus::BackToTrade => ui.write(UiScript::trade_back_to_trade),
        // The line, then the refused slot's deny (`0x58771d`/`0x587738` -> `0x587727`/`0x587742`).
        TradeStatus::OnlyConjured { slot } | TradeStatus::NotEligible { slot } => {
            if let Some(Line::Plain(key)) = status_message(status) {
                ui.line(UiError::key(key));
            }
            ui.write(move |script| script.deny_trade_item(slot));
        }
        _ => match status_message(status) {
            Some(Line::Plain(key)) => ui.line(UiError::key(key)),
            // Our own initiate's partner while still an object, named by its unit (`0x58760a`-`0x58764b`).
            Some(Line::Naming(key)) => {
                if let Some(who) = naming {
                    let streamed = reads.index.as_deref().is_some_and(|index| {
                        super::streamed_player(index, &reads.stores, who).is_some()
                    });
                    if streamed {
                        ui.line(super::unit_name_line(
                            key,
                            who,
                            reads.names.as_deref(),
                            &reads.commands,
                        ));
                    }
                }
            }
            None => {}
        },
    }
}

/// The partner setter with none (`0x5870a0`): the enchanting item and the completion flag, the interact target's clear on the partner, then the partner zeroed and `TRADE_CLOSED` (`0x5870d5`-`0x58710e`); the cancel it ends on goes out from [`trade_status`].
fn close_partner(ui: &mut UiEvents, partner: Option<u64>, completed: bool) {
    ui.write(move |script| script.close_trade_partner(completed));
    if let Some(partner) = partner {
        ui.world(move |w| crate::ui_session::clear_interact_target(w, partner));
    }
    ui.write(UiScript::trade_partner_closed);
}

/// A name answer runs the trade's name callback (`0x703c60`): each line owed on it shows on a found name, the placeholder's too (`0x635894` caches it as found), and is dropped on a denial.
fn on_name_answer(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    mut ui: ResMut<UiEvents>,
) {
    use benilla_protocol::messages::NameQueryMiss;
    let (guid, name) = match ev {
        SessionEvent::PlayerName { guid, name, .. } => (guid, Some(name)),
        SessionEvent::PlayerNameMiss {
            guid,
            miss: NameQueryMiss::Placeholder,
        } => (guid, Some(crate::names::PLACEHOLDER_NAME.to_string())),
        SessionEvent::PlayerNameMiss {
            guid,
            miss: NameQueryMiss::Deny,
        } => (guid, None),
        _ => return,
    };
    for line in trade.take_named_lines_for(guid) {
        if let Some(name) = &name {
            ui.line(UiError::s(line.key, name.clone()));
        }
    }
}

/// `CLOSE_WINDOW`'s line (`0x58754f`-`0x5875bd`): result `0x54` with an item names the item's
/// limit category, quantity then name, and shows nothing when the item or its row is not cached.
fn close_window_line(
    result: u32,
    target: bool,
    item: u32,
    limit: &dyn Fn(u32) -> Option<(u32, String)>,
) -> Option<UiError> {
    let key = trade_error_key(result, target)?;
    if result == 0x54 && item != 0 {
        let (quantity, name) = limit(item)?;
        return Some(UiError::args(
            key,
            vec![FillArg::D(i64::from(quantity)), FillArg::S(name)],
        ));
    }
    Some(UiError::key(key))
}

/// The close's error id (`0x586520`): results 4, 17, 28 and 84 pick by the byte (`0x58659c`), every
/// other goes through the inventory table. The byte's reagent id, `0x2da`, is past the catalog.
fn trade_error_key(result: u32, target: bool) -> Option<&'static str> {
    Some(match (result, target) {
        (4, true) => "ERR_TRADE_BAG_FULL",
        (4, false) => "ERR_TRADE_TARGET_BAG_FULL",
        (17, false) => "ERR_TRADE_MAX_COUNT_EXCEEDED",
        (17, true) => "ERR_TRADE_TARGET_MAX_COUNT_EXCEEDED",
        (28, false) => "ERR_SPELL_FAILED_REAGENTS_GENERIC",
        (28, true) => return None,
        (84, false) => "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS",
        (84, true) => "ERR_TRADE_TARGET_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS",
        _ => return crate::ui_items::inventory_error_key(result),
    })
}

fn on_trade_status_extended(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    ui: Option<ResMut<UiEvents>>,
) {
    let SessionEvent::TradeStatusExtended { state } = ev else {
        return;
    };
    let update = trade_status_extended(&state, &mut trade, &commands);
    let Some(mut ui) = ui.filter(|_| update == OfferUpdate::Applied) else {
        return;
    };
    // An own-window snapshot carries our offered coins (`0x7046ff`) and denies each slot it no longer fills (`0x704810`, `0x704873` -> `0x586410`).
    if !state.their_window {
        let gold = state.gold;
        ui.write(move |script| script.set_trade_offer_gold(gold));
        for slot in trade.mirror_own(&state) {
            ui.write(move |script| script.deny_trade_item(slot));
        }
    }
    // The window's offer update over the partner's window, either side's snapshot (`0x7048b2`).
    let (items, cells) = (trade.their_items(), trade.cells());
    ui.write(move |script| script.trade_updated(items, cells));
}

/// An open trade dies with the socket, after `net::session::on_disconnected`'s own teardown.
fn on_session_end(In(_): In<SessionEvent>, mut trade: ResMut<TradeSession>) {
    trade.clear_session();
}

/// `SMSG_TRADE_STATUS`'s network side, arm for arm the reference's 23-case dispatcher: only
/// `CANCELED`, `COMPLETE` and `CLOSE_WINDOW` close the window, and the handler's tail then clears the
/// network cells ([`TradeSession::clear_pending`]). `BEGIN_TRADE` only records the request for
/// [`super::answer_trade_request`]. The window's side, the events and the lines, is
/// [`on_trade_status`]'s.
fn trade_status(status: TradeStatus, trade: &mut TradeSession, commands: &NetCommands) {
    bevy::log::info!(target: "trade", "SMSG_TRADE_STATUS {status:?}");
    match status {
        TradeStatus::BeginTrade { partner } => trade.request(partner),
        TradeStatus::OpenWindow { trade_id } => trade.open_window(trade_id),
        // The three that close the window, each through the partner setter, whose close ends on `CMSG_CANCEL_TRADE` whether a window stood or not (`0x587116` -> `0x704040`).
        TradeStatus::Canceled | TradeStatus::Complete | TradeStatus::CloseWindow { .. } => {
            trade.close_window();
            let _ = commands.0.send(ClientCommand::CancelTrade);
        }
        // The placement is bounced, not the window.
        TradeStatus::OnlyConjured { slot } | TradeStatus::NotEligible { slot } => {
            trade.bounce_own_offer(slot)
        }
        // The initiate refusals: the tail's clear alone.
        TradeStatus::Busy
        | TradeStatus::Busy2
        | TradeStatus::NoTarget
        | TradeStatus::TargetTooFar
        | TradeStatus::WrongFaction
        | TradeStatus::IgnoreYou
        | TradeStatus::YouStunned
        | TradeStatus::TargetStunned
        | TradeStatus::YouDead
        | TradeStatus::TargetDead
        | TradeStatus::YouLogout
        | TradeStatus::TargetLogout
        | TradeStatus::TrialAccount => trade.clear_pending(),
        // Code 13 shares the cancel's packet arm (`0x704584`): the coins zero, then the tail's clear.
        TradeStatus::Unknown13 => {
            trade.zero_gold();
            trade.clear_pending();
        }
        // The accept flags are the window's (`0x586390`, `0x586930`); past `0x16`, the bare epilogue.
        TradeStatus::Accept
        | TradeStatus::Rejected
        | TradeStatus::BackToTrade
        | TradeStatus::Unknown(_) => {}
    }
}

/// A status arm's line: a plain key, or one whose `%s` waits for a player's name.
enum Line {
    Plain(&'static str),
    Naming(&'static str),
}

/// The `GlobalStrings` key each status arm raises through the error display, or `None`. The catalog
/// row's `+0x04`, not the key, decides chat, yellow or red: the two successful outcomes are the
/// yellow ones, and `ERR_TRADE_WRONG_REALM`'s text is the conjured-items sentence.
/// `CLOSE_WINDOW`'s line is [`close_window_line`], from the handler.
fn status_message(status: TradeStatus) -> Option<Line> {
    Some(match status {
        TradeStatus::Busy | TradeStatus::Busy2 => Line::Naming("ERR_PLAYER_BUSY_S"),
        TradeStatus::IgnoreYou => Line::Naming("ERR_IGNORING_YOU_S"),
        TradeStatus::Canceled => Line::Plain("ERR_TRADE_CANCELLED"),
        TradeStatus::NoTarget => Line::Plain("ERR_GENERIC_NO_TARGET"),
        TradeStatus::Complete => Line::Plain("ERR_TRADE_COMPLETE"),
        TradeStatus::TargetTooFar => Line::Plain("ERR_TRADE_TOO_FAR"),
        TradeStatus::WrongFaction => Line::Plain("ERR_PLAYER_WRONG_FACTION"),
        TradeStatus::YouStunned => Line::Plain("ERR_GENERIC_STUNNED"),
        TradeStatus::TargetStunned => Line::Plain("ERR_TARGET_STUNNED"),
        TradeStatus::YouDead => Line::Plain("ERR_PLAYER_DEAD"),
        TradeStatus::TargetDead => Line::Plain("ERR_TRADE_TARGET_DEAD"),
        TradeStatus::YouLogout => Line::Plain("ERR_LOGGING_OUT"),
        TradeStatus::TargetLogout => Line::Plain("ERR_TARGET_LOGGING_OUT"),
        TradeStatus::TrialAccount => Line::Plain("ERR_RESTRICTED_ACCOUNT"),
        TradeStatus::OnlyConjured { .. } => Line::Plain("ERR_TRADE_WRONG_REALM"),
        TradeStatus::NotEligible { .. } => Line::Plain("ERR_TRADE_NOT_ON_TAPLIST"),
        // Silent, plus `BEGIN_TRADE` (the ladder's line) and `CLOSE_WINDOW` (its own line).
        TradeStatus::BeginTrade { .. }
        | TradeStatus::OpenWindow { .. }
        | TradeStatus::Accept
        | TradeStatus::BackToTrade
        | TradeStatus::Rejected
        | TradeStatus::CloseWindow { .. }
        | TradeStatus::Unknown13
        | TradeStatus::Unknown(_) => return None,
    })
}

/// `SMSG_TRADE_STATUS_EXTENDED`: replace one side's offer; the feed repaints on the change.
fn trade_status_extended(
    ext: &TradeStatusExtended,
    trade: &mut TradeSession,
    commands: &NetCommands,
) -> OfferUpdate {
    let update = trade.set_offer(ext);
    bevy::log::info!(
        target: "trade",
        "SMSG_TRADE_STATUS_EXTENDED their_window={} trade_id={} state={} gold={} items={:?} -> {update:?}",
        ext.their_window,
        ext.trade_id,
        ext.state_index,
        ext.gold,
        ext.slots.map(|s| s.map(|i| i.entry)),
    );
    if update == OfferUpdate::OtherTrade {
        let _ = commands.0.send(ClientCommand::CancelTrade);
    }
    update
}

#[cfg(test)]
mod tests {
    use benilla_ui::messages::{kind_of, MsgKind};

    use super::*;

    fn every_status() -> Vec<TradeStatus> {
        vec![
            TradeStatus::Busy,
            TradeStatus::BeginTrade { partner: 0x7 },
            TradeStatus::OpenWindow { trade_id: 0 },
            TradeStatus::Canceled,
            TradeStatus::Accept,
            TradeStatus::Busy2,
            TradeStatus::NoTarget,
            TradeStatus::BackToTrade,
            TradeStatus::Complete,
            TradeStatus::Rejected,
            TradeStatus::TargetTooFar,
            TradeStatus::WrongFaction,
            TradeStatus::CloseWindow {
                result: 0,
                target: false,
                item: 0,
            },
            TradeStatus::Unknown13,
            TradeStatus::IgnoreYou,
            TradeStatus::YouStunned,
            TradeStatus::TargetStunned,
            TradeStatus::YouDead,
            TradeStatus::TargetDead,
            TradeStatus::YouLogout,
            TradeStatus::TargetLogout,
            TradeStatus::TrialAccount,
            TradeStatus::OnlyConjured { slot: 2 },
            TradeStatus::NotEligible { slot: 2 },
        ]
    }

    fn sink() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// A window up under our own initiate, so a wrong close or cell clear shows.
    fn open_trade() -> TradeSession {
        let mut trade = TradeSession::default();
        trade.initiate(0x7, false);
        trade.open_window(0);
        trade
    }

    #[test]
    fn the_table_routes_the_status_and_the_session_end_to_the_trade() {
        let (commands, _rx) = sink();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<TradeSession>()
            .insert_resource(commands);
        register(&mut app);

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::TradeStatus {
                status: TradeStatus::OpenWindow { trade_id: 0 },
            }],
        );
        assert!(app.world().resource::<TradeSession>().is_open());

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );
        assert!(!app.world().resource::<TradeSession>().is_open());
    }

    #[test]
    fn seventeen_arms_print_here_and_the_seven_silent_ones_are_pinned() {
        let silent: Vec<u32> = every_status()
            .into_iter()
            .filter(|s| status_message(*s).is_none())
            .map(TradeStatus::code)
            .collect();
        assert_eq!(
            silent,
            vec![1, 2, 4, 7, 9, 12, 13],
            "silent: BEGIN_TRADE (the ladder's line is its own), OPEN_WINDOW, TRADE_ACCEPT, \
             BACK_TO_TRADE, REJECTED, CLOSE_WINDOW (a computed id we deliberately do not build) \
             and UNKNOWN_13"
        );
    }

    #[test]
    fn the_surfaces_are_the_catalogs_and_not_the_key_names() {
        for status in every_status() {
            let Some(line) = status_message(status) else {
                continue;
            };
            let key = match line {
                Line::Plain(k) | Line::Naming(k) => k,
            };
            assert!(
                benilla_ui::messages::by_key(key).is_some(),
                "{status:?} names {key}, which is not a catalog row"
            );
        }
        assert_eq!(kind_of("ERR_PLAYER_BUSY_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_IGNORING_YOU_S"), MsgKind::Chat);
        assert_eq!(kind_of("ERR_TRADE_CANCELLED"), MsgKind::Info);
        assert_eq!(kind_of("ERR_TRADE_COMPLETE"), MsgKind::Info);
        assert_eq!(kind_of("ERR_TRADE_TARGET_DEAD"), MsgKind::Error);
    }

    /// The trade-partner set has four call sites in the dispatcher: one opens the window, three
    /// close it.
    #[test]
    fn only_canceled_complete_and_close_window_tear_the_window_down() {
        let closes: Vec<u32> = every_status()
            .into_iter()
            .filter(|status| {
                let (commands, _rx) = sink();
                let mut trade = open_trade();
                trade_status(*status, &mut trade, &commands);
                !trade.is_open()
            })
            .map(TradeStatus::code)
            .collect();
        assert_eq!(closes, vec![3, 8, 12]);
    }

    /// The tail's clear set minus the three closing codes.
    #[test]
    fn the_refusals_clear_the_latch_and_leave_the_window_standing() {
        for status in every_status() {
            let code = status.code();
            if !matches!(code, 0 | 5 | 6 | 10 | 11 | 13 | 14..=21) {
                continue;
            }
            let (commands, _rx) = sink();
            let mut trade = open_trade();
            trade_status(status, &mut trade, &commands);
            assert!(trade.is_open(), "{status:?} must not close the window");
            assert!(
                trade.line_names().is_none(),
                "{status:?} must drop the initiate latch"
            );
        }
    }

    /// Cases 4, 7 and 9 leave the network cells alone: the accept flags are the window's (`0x586390`, `0x586930`), and none is in the tail's clear set.
    #[test]
    fn the_accept_codes_leave_the_cells_alone() {
        for status in [
            TradeStatus::Accept,
            TradeStatus::BackToTrade,
            TradeStatus::Rejected,
        ] {
            let (commands, rx) = sink();
            let mut trade = open_trade();
            trade_status(status, &mut trade, &commands);
            assert!(
                trade.is_open() && trade.line_names().is_some(),
                "{status:?}"
            );
            assert!(rx.try_recv().is_err(), "{status:?} sends nothing itself");
        }
    }

    /// `0xff` names the gold; an out-of-range slot (200) is dropped.
    #[test]
    fn only_conjured_bounces_one_slot_and_leaves_the_window_up() {
        // Wire slot 1 is UI slot 2, the one `offer_from` fills below.
        for (slot, expect_gold, expect_slot2) in
            [(1u8, 700u32, false), (0xff, 0, true), (200, 700, true)]
        {
            let (commands, _rx) = sink();
            let mut trade = open_trade();
            trade.offer_from(2, 0, 5);
            trade.set_own_gold(700);
            trade_status(TradeStatus::OnlyConjured { slot }, &mut trade, &commands);
            assert!(trade.is_open(), "slot {slot}: the window stays up");
            let (gold, filled) = trade.own_offer_for_test();
            assert_eq!(gold, expect_gold, "slot {slot}");
            assert_eq!(filled[1], expect_slot2, "slot {slot}");
        }
    }

    /// `0x586520`: the byte picks within each of the four pairs, every other result takes the
    /// inventory table (0 shows nothing), and `0x54` with an item names its cached category.
    #[test]
    fn a_close_window_shows_its_results_line() {
        let limit = |entry: u32| (entry == 7).then(|| (2, "Stones".to_string()));
        let line = |result, target| close_window_line(result, target, 0, &limit);
        assert_eq!(line(0, false), None);
        assert_eq!(line(4, true), Some(UiError::key("ERR_TRADE_BAG_FULL")));
        assert_eq!(
            line(4, false),
            Some(UiError::key("ERR_TRADE_TARGET_BAG_FULL"))
        );
        assert_eq!(
            line(17, false),
            Some(UiError::key("ERR_TRADE_MAX_COUNT_EXCEEDED"))
        );
        assert_eq!(
            line(17, true),
            Some(UiError::key("ERR_TRADE_TARGET_MAX_COUNT_EXCEEDED"))
        );
        assert_eq!(
            line(28, false),
            Some(UiError::key("ERR_SPELL_FAILED_REAGENTS_GENERIC"))
        );
        assert_eq!(line(28, true), None);
        assert_eq!(line(50, true), Some(UiError::key("ERR_INV_FULL")));
        assert_eq!(
            line(84, true),
            Some(UiError::key(
                "ERR_TRADE_TARGET_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS"
            ))
        );
        assert_eq!(
            close_window_line(84, false, 7, &limit),
            Some(UiError::args(
                "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS",
                vec![FillArg::D(2), FillArg::S("Stones".into())]
            ))
        );
        assert_eq!(
            close_window_line(84, false, 8, &limit),
            None,
            "an uncached item shows nothing"
        );
    }
}

/// The offer's item locks through the handlers, each release at its own site.
#[cfg(test)]
mod lock_tests {
    use benilla_protocol::messages::{TradeItem, TradeStatus, TradeStatusExtended};
    use benilla_protocol::SessionEvent;
    use benilla_ui::script::{ContainerSlot, ContainerState, UiScript};
    use bevy::prelude::*;

    use super::super::TradeSession;
    use crate::net::{ClientCommand, NetCommands};

    /// A world with the trade's handlers, an open window on `0x7`, a pack of three items and a spy logging the trade and lock events.
    fn rig() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .insert_resource(NetCommands(tx));
        super::register(&mut app);
        let mut script = UiScript::new().unwrap();
        let mut slots = std::collections::HashMap::new();
        for (slot, item_id) in [(1, 2589), (2, 2770), (3, 118)] {
            slots.insert(
                slot,
                ContainerSlot {
                    guid: 0x4000_0000_0000_0000 + u64::from(slot),
                    item_id,
                    count: 1,
                    ..Default::default()
                },
            );
        }
        script.set_container(
            0,
            Some(ContainerState {
                bag_family: 0,
                name: None,
                num_slots: 16,
                slots,
            }),
        );
        script.open_trade_partner(0x7, false);
        script
            .run(
                "LOG = {} local f = CreateFrame('Frame') \
                 for _, e in ipairs({'TRADE_REQUEST_CANCEL', 'TRADE_PLAYER_ITEM_CHANGED', 'ITEM_UNLOCKED', 'PLAYER_TRADE_MONEY', 'TRADE_CLOSED'}) do f:RegisterEvent(e) end \
                 f:SetScript('OnEvent', function(_, e, a, b) LOG[#LOG + 1] = e .. (b and (':' .. b) or a and (':' .. a) or '') end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.initiate(0x7, false);
            trade.open_window(9);
        }
        (app, rx)
    }

    fn run(app: &mut App, lua: &str) {
        crate::ui_script::live::with_live_world(app.world_mut(), |s| s.run(lua).unwrap());
    }

    fn log(app: &mut App) -> String {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<String>("local l = table.concat(LOG, ' ') LOG = {} return l")
            .unwrap()
    }

    fn locked(app: &mut App, slot: u32) -> bool {
        crate::ui_script::live::with_live_world(app.world_mut(), |s| {
            s.eval::<bool>(&format!(
                "return select(3, GetContainerItemInfo(0, {slot})) == 1"
            ))
            .unwrap()
        })
        .unwrap()
    }

    fn status(app: &mut App, status: TradeStatus) {
        crate::net::handlers::dispatch(app.world_mut(), vec![SessionEvent::TradeStatus { status }]);
    }

    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<String> {
        rx.try_iter().map(|c| format!("{c:?}")).collect()
    }

    /// Two items offered in slots 1 and 3.
    fn offered() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = rig();
        run(
            &mut app,
            "PickupContainerItem(0, 1) ClickTradeButton(1) PickupContainerItem(0, 2) ClickTradeButton(3)",
        );
        assert!(locked(&mut app, 1) && locked(&mut app, 2));
        log(&mut app);
        (app, rx)
    }

    /// `0x587503`, `0x587508`, `0x587513`: the cancel signals, the offer's items unlock, then the partner setter's close signals `TRADE_CLOSED` (`0x58710e`) and sends the cancel.
    #[test]
    fn a_cancel_signals_then_unlocks_the_offer_and_sends_the_cancel() {
        let (mut app, rx) = offered();
        status(&mut app, TradeStatus::Canceled);
        assert_eq!(
            log(&mut app),
            "TRADE_REQUEST_CANCEL ITEM_UNLOCKED:1 ITEM_UNLOCKED:2 TRADE_CLOSED"
        );
        assert!(!locked(&mut app, 1) && !locked(&mut app, 2));
        assert!(!app.world().resource::<TradeSession>().is_open());
        assert_eq!(sent(&rx), ["CancelTrade"]);
    }

    /// `0x587529`, `0x58754a`: the completion and the close each unlock the offer, and only the completion silences `CloseTrade`'s coins (`0x587534` passes 1, `0x587949`).
    #[test]
    fn a_completion_and_a_close_unlock_the_offer() {
        for (closing, coins) in [
            (TradeStatus::Complete, ""),
            (
                TradeStatus::CloseWindow {
                    result: 0,
                    target: false,
                    item: 0,
                },
                "PLAYER_TRADE_MONEY",
            ),
        ] {
            let (mut app, rx) = offered();
            status(&mut app, closing);
            assert_eq!(
                log(&mut app),
                "ITEM_UNLOCKED:1 ITEM_UNLOCKED:2 TRADE_CLOSED"
            );
            assert_eq!(sent(&rx), ["CancelTrade"]);
            // The frame's `CloseTrade` on the hide, the partner already zeroed.
            run(&mut app, "CloseTrade()");
            assert_eq!(log(&mut app), coins, "{closing:?}");
        }
    }

    /// The completed trade's items leave before or after its status: an item gone first is out of the offer and unlocks nothing (`0x70b0df` -> `0x586aa0`), one still held unlocks at the status (`0x5864b3`).
    #[test]
    fn a_completion_unlocks_whichever_side_of_the_items_leaving_it_lands() {
        // The destroy first: the slot's clear goes out and signals, and the status has nothing of it to unlock.
        let (mut app, _rx) = offered();
        crate::ui_script::live::with_live_world(app.world_mut(), |s| {
            s.trade_item_destroyed(0x4000_0000_0000_0001)
        });
        assert_eq!(log(&mut app), "TRADE_PLAYER_ITEM_CHANGED:1");
        status(&mut app, TradeStatus::Complete);
        assert_eq!(log(&mut app), "ITEM_UNLOCKED:2 TRADE_CLOSED");

        // The status first: both unlock, and the destroy then takes the item out of the slots that stand.
        let (mut app, _rx) = offered();
        status(&mut app, TradeStatus::Complete);
        assert_eq!(
            log(&mut app),
            "ITEM_UNLOCKED:1 ITEM_UNLOCKED:2 TRADE_CLOSED"
        );
        crate::ui_script::live::with_live_world(app.world_mut(), |s| {
            s.trade_item_destroyed(0x4000_0000_0000_0001)
        });
        assert_eq!(log(&mut app), "TRADE_PLAYER_ITEM_CHANGED:1");
    }

    /// `0x587727`, `0x587742` -> `0x586455`: a refused slot's item unlocks and its slot zeroes; the window stands.
    #[test]
    fn a_refused_slot_unlocks_its_item_and_the_window_stands() {
        for refusal in [
            TradeStatus::OnlyConjured { slot: 2 },
            TradeStatus::NotEligible { slot: 2 },
        ] {
            let (mut app, _rx) = offered();
            status(&mut app, refusal);
            assert_eq!(log(&mut app), "ITEM_UNLOCKED:2 TRADE_PLAYER_ITEM_CHANGED:3");
            assert!(locked(&mut app, 1) && !locked(&mut app, 2));
            assert!(app.world().resource::<TradeSession>().is_open());
        }
        // `0xff` is the coins: no slot is touched, the coins signal (`0x586418`, `0x58642e`).
        let (mut app, _rx) = offered();
        status(&mut app, TradeStatus::OnlyConjured { slot: 0xff });
        assert_eq!(log(&mut app), "PLAYER_TRADE_MONEY");
        assert!(locked(&mut app, 1) && locked(&mut app, 2));
    }

    fn wire_item(entry: u32) -> TradeItem {
        TradeItem {
            entry,
            display_id: 1,
            count: 1,
            wrapped: false,
            gift_creator: 0,
            perm_enchant: 0,
            socket_enchants: [0; 3],
            creator: 0,
            charges: 0,
            suffix_factor: 0,
            random_prop_id: 0,
            lock_id: 0,
            max_durability: 0,
            durability: 0,
        }
    }

    fn snapshot(app: &mut App, their_window: bool, state_index: u32, filled: &[(usize, u32)]) {
        let mut slots = [None; 7];
        for &(i, entry) in filled {
            slots[i] = Some(wire_item(entry));
        }
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::TradeStatusExtended {
                state: Box::new(TradeStatusExtended {
                    their_window,
                    trade_id: 9,
                    state_index,
                    side_state: state_index,
                    gold: 0,
                    enchant_spell_id: 0,
                    slots,
                }),
            }],
        );
    }

    /// `0x704810`, `0x704873`: an own-window snapshot denies a slot the one before it filled and it leaves empty, in either order with the slot's own status.
    #[test]
    fn an_own_snapshot_denies_the_slot_it_no_longer_fills() {
        let (mut app, _rx) = offered();
        snapshot(&mut app, false, 5, &[(0, 2589), (2, 2770)]);
        assert_eq!(log(&mut app), "", "both slots stand in it");
        snapshot(&mut app, false, 6, &[(0, 2589)]);
        assert_eq!(log(&mut app), "ITEM_UNLOCKED:2 TRADE_PLAYER_ITEM_CHANGED:3");
        assert!(locked(&mut app, 1) && !locked(&mut app, 2));
        // The status after it finds the slot empty and signals it alone (`0x586410` has no gate).
        status(&mut app, TradeStatus::NotEligible { slot: 2 });
        assert_eq!(log(&mut app), "TRADE_PLAYER_ITEM_CHANGED:3");

        // The status first: the snapshot after it still denies, the mirror being its own.
        let (mut app, _rx) = offered();
        snapshot(&mut app, false, 5, &[(0, 2589), (2, 2770)]);
        status(&mut app, TradeStatus::NotEligible { slot: 2 });
        assert_eq!(log(&mut app), "ITEM_UNLOCKED:2 TRADE_PLAYER_ITEM_CHANGED:3");
        snapshot(&mut app, false, 6, &[(0, 2589)]);
        assert_eq!(log(&mut app), "TRADE_PLAYER_ITEM_CHANGED:3");

        // A slot only we filled, which no snapshot showed, is not denied by one that lacks it.
        let (mut app, _rx) = offered();
        snapshot(&mut app, false, 5, &[(0, 2589)]);
        assert_eq!(log(&mut app), "");
        assert!(locked(&mut app, 2));
    }

    /// `0x586fb6`, `0x5870d5`: the enchanting item unlocks when the partner's seventh slot changes and when a status closes the window.
    #[test]
    fn the_enchanting_item_unlocks_on_the_partners_slot_change_and_on_the_close() {
        let aim = |app: &mut App| {
            // The partner's seventh slot reaches the window's copy in the offer update.
            snapshot(app, true, 2, &[(6, 6217)]);
            let facts = benilla_ui::script::TradeItemFacts {
                enchants_on_use: true,
                ..Default::default()
            };
            {
                let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
                script.set_trade_item_facts(0, 3, facts);
            }
            run(app, "UseContainerItem(0, 3)");
            assert!(locked(app, 3));
            log(app);
        };
        let (mut app, _rx) = rig();
        aim(&mut app);
        snapshot(&mut app, true, 3, &[(0, 118), (6, 6217)]);
        assert!(locked(&mut app, 3), "another slot's change leaves it");
        snapshot(&mut app, true, 4, &[(0, 118)]);
        assert_eq!(log(&mut app), "ITEM_UNLOCKED:3");

        let (mut app, _rx) = rig();
        aim(&mut app);
        status(&mut app, TradeStatus::Canceled);
        assert_eq!(
            log(&mut app),
            "TRADE_REQUEST_CANCEL ITEM_UNLOCKED:3 TRADE_CLOSED"
        );
    }

    /// The status packet's own arms run ahead of the trade's (`0x7044dd`): our offered coins zero on the open, each close and the coins' refusal (`0x704545`, `0x70459f`, `0x7045eb`, `0x70457c`), codes 3 and 13 signal `PLAYER_MONEY` first (`0x7045ab`), and our own snapshot carries the coins (`0x7046ff`).
    #[test]
    fn the_status_arms_zero_the_offered_coins_and_signal_the_purse() {
        let coins = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .trade_offer_gold()
        };
        let close = TradeStatus::CloseWindow {
            result: 0,
            target: false,
            item: 0,
        };
        for (code, zeroed) in [
            (TradeStatus::OpenWindow { trade_id: 9 }, true),
            (TradeStatus::Canceled, true),
            (TradeStatus::Complete, true),
            (close, true),
            (TradeStatus::Unknown13, true),
            (TradeStatus::OnlyConjured { slot: 0xff }, true),
            (TradeStatus::NotEligible { slot: 0xff }, true),
            (TradeStatus::OnlyConjured { slot: 2 }, false),
            (TradeStatus::Accept, false),
            (TradeStatus::BackToTrade, false),
            (TradeStatus::TargetTooFar, false),
        ] {
            let (mut app, _rx) = rig();
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_trade_offer_gold(500);
            status(&mut app, code);
            assert_eq!(coins(&app), if zeroed { 0 } else { 500 }, "{code:?}");
        }

        let spy = "local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_MONEY') \
                   f:SetScript('OnEvent', function(_, e) LOG[#LOG + 1] = e end)";
        let (mut app, _rx) = rig();
        run(&mut app, spy);
        status(&mut app, TradeStatus::Canceled);
        assert_eq!(
            log(&mut app),
            "PLAYER_MONEY TRADE_REQUEST_CANCEL TRADE_CLOSED"
        );
        let (mut app, _rx) = rig();
        run(&mut app, spy);
        status(&mut app, TradeStatus::Unknown13);
        assert_eq!(log(&mut app), "PLAYER_MONEY");
        status(&mut app, TradeStatus::Complete);
        assert_eq!(
            log(&mut app),
            "TRADE_CLOSED",
            "a completion signals no purse"
        );

        let (mut app, _rx) = rig();
        let gold_snapshot = |app: &mut App, their_window: bool, gold: u32| {
            crate::net::handlers::dispatch(
                app.world_mut(),
                vec![SessionEvent::TradeStatusExtended {
                    state: Box::new(TradeStatusExtended {
                        their_window,
                        trade_id: 9,
                        state_index: 5,
                        side_state: 5,
                        gold,
                        enchant_spell_id: 0,
                        slots: [None; 7],
                    }),
                }],
            );
        };
        gold_snapshot(&mut app, false, 250);
        assert_eq!(coins(&app), 250);
        gold_snapshot(&mut app, true, 99);
        assert_eq!(coins(&app), 250, "the partner's coins are not ours");
    }

    /// A spy on `events` appending `EVENT:args` to `LOG`.
    fn spy(app: &mut App, events: &[&str]) {
        let list = events
            .iter()
            .map(|e| format!("'{e}'"))
            .collect::<Vec<_>>()
            .join(", ");
        run(
            app,
            &format!(
                "local f = CreateFrame('Frame') for _, e in ipairs({{{list}}}) do f:RegisterEvent(e) end \
                 f:SetScript('OnEvent', function(_, e, ...) local a = {{}} for i = 1, select('#', ...) do a[#a + 1] = tostring((select(i, ...))) end \
                 LOG[#LOG + 1] = e .. ':' .. table.concat(a, ',') end)"
            ),
        );
    }

    /// `TRADE_ACCEPT` and `REJECTED` write the partner's flag and signal it when it moves (`0x5874d9`, `0x5874e7` -> `0x586390`); `BACK_TO_TRADE` drops ours with the unaccept send, then theirs, each signalled (`0x5874f3` -> `0x586959`-`0x586996`).
    #[test]
    fn the_accept_codes_write_the_windows_flags_at_their_step() {
        let (mut app, _rx) = rig();
        spy(&mut app, &["TRADE_ACCEPT_UPDATE"]);
        status(&mut app, TradeStatus::Accept);
        status(&mut app, TradeStatus::Accept);
        assert_eq!(log(&mut app), "TRADE_ACCEPT_UPDATE:0,1");
        run(&mut app, "AcceptTrade()");
        assert_eq!(log(&mut app), "TRADE_ACCEPT_UPDATE:1,1");
        status(&mut app, TradeStatus::BackToTrade);
        assert_eq!(
            log(&mut app),
            "TRADE_ACCEPT_UPDATE:0,1 TRADE_ACCEPT_UPDATE:0,0"
        );
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_trade_accept_sends(),
            [true, false]
        );
        status(&mut app, TradeStatus::Rejected);
        assert_eq!(log(&mut app), "", "the partner's flag is already down");
    }

    /// The partner's offer update signals each slot that moved, then the coins, after our accept's drop (`0x586dc7`, `0x586f96`, `0x587092`); an own snapshot runs it too (`0x7048b2`).
    #[test]
    fn an_offer_update_signals_in_its_handler() {
        let (mut app, _rx) = rig();
        spy(
            &mut app,
            &[
                "TRADE_ACCEPT_UPDATE",
                "TRADE_TARGET_ITEM_CHANGED",
                "TRADE_MONEY_CHANGED",
            ],
        );
        status(&mut app, TradeStatus::Accept);
        log(&mut app);
        snapshot(&mut app, true, 2, &[(0, 118), (3, 2589)]);
        assert_eq!(
            log(&mut app),
            "TRADE_ACCEPT_UPDATE:0,0 TRADE_TARGET_ITEM_CHANGED:1 TRADE_TARGET_ITEM_CHANGED:4"
        );
        snapshot(&mut app, false, 2, &[]);
        assert_eq!(log(&mut app), "", "nothing of the partner's moved");
    }

    /// A naming status shows its line at its step while our initiate's partner is an object, named by its unit or `UNKNOWNOBJECT` (`0x58760a`-`0x58764b`, `0x72a000`); a partner gone from view says nothing, and so does the side that did not initiate (`0x58760a`, `0x587657`).
    #[test]
    fn the_naming_lines_show_by_name_while_the_partner_is_an_object() {
        let named = |streamed: bool, initiated: bool, cached: bool| {
            let (mut app, _rx) = rig();
            let mut names = crate::names::NameCache::default();
            if cached {
                names.insert_player(0x7, "Grubbis".to_string(), None);
            }
            let mut index = crate::net::GuidIndex::default();
            if streamed {
                let e = app
                    .world_mut()
                    .spawn(crate::net::ObjectStore::default())
                    .id();
                index.0.insert(0x7, e);
            }
            app.insert_resource(names).insert_resource(index);
            if !initiated {
                let mut trade = app.world_mut().resource_mut::<TradeSession>();
                trade.request(0x7);
                trade.begin(0x7);
            }
            run(
                &mut app,
                "ERR_PLAYER_BUSY_S = '%s is busy.' ERR_IGNORING_YOU_S = '%s is ignoring you.' \
                 UNKNOWNOBJECT = 'Unknown'",
            );
            status(&mut app, TradeStatus::Busy);
            status(&mut app, TradeStatus::IgnoreYou);
            app.world()
                .resource::<crate::ui_chat::ChatLog>()
                .pending_lines()
        };
        assert_eq!(
            named(true, true, true),
            ["Grubbis is busy."],
            "the tail clears the latch"
        );
        // An uncached name shows at the step as `UNKNOWNOBJECT` (`0x72a271`), nothing held back.
        assert_eq!(named(true, true, false), ["Unknown is busy."]);
        assert!(named(false, true, true).is_empty());
        assert!(named(true, false, true).is_empty());
    }

    /// Each closing status shows its line at its step: the cancel's and the completion's after the close's `TRADE_CLOSED` (`0x587513` before `0x58751d`, `0x587534` before `0x58753e`), the close window's ahead of it (`0x5875bd` before `0x5875cb`).
    #[test]
    fn a_closing_statuss_line_shows_at_its_step() {
        for (closing, order) in [
            (
                TradeStatus::Canceled,
                "TRADE_CLOSED UI_INFO_MESSAGE:Cancelled",
            ),
            (
                TradeStatus::Complete,
                "TRADE_CLOSED UI_INFO_MESSAGE:Complete",
            ),
            (
                TradeStatus::CloseWindow {
                    result: 4,
                    target: true,
                    item: 0,
                },
                "UI_ERROR_MESSAGE:BagFull TRADE_CLOSED",
            ),
        ] {
            let (mut app, _rx) = rig();
            run(
                &mut app,
                "ERR_TRADE_CANCELLED = 'Cancelled' ERR_TRADE_COMPLETE = 'Complete' \
                 ERR_TRADE_BAG_FULL = 'BagFull'",
            );
            spy(&mut app, &["UI_INFO_MESSAGE", "UI_ERROR_MESSAGE"]);
            log(&mut app);
            status(&mut app, closing);
            let seen = log(&mut app);
            let kept: Vec<&str> = seen
                .split(' ')
                .filter(|e| *e == "TRADE_CLOSED" || e.starts_with("UI_"))
                .collect();
            assert_eq!(kept.join(" "), order, "{closing:?}");
        }
    }

    /// `OPEN_WINDOW` runs the partner setter's open in its handler: an initiate of ours that asked for the cursor folds the held coins, signalled ahead of `TRADE_SHOW` (`0x5872bf`, `0x5872ca`, `0x5872d8`).
    #[test]
    fn the_open_folds_the_cursors_coins_for_an_initiate_that_asked() {
        let (mut app, _rx) = rig();
        status(&mut app, TradeStatus::Canceled);
        // The purse the pickup and the fold read off our player (`0x704283`).
        let purse = benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE, 1_000),
        ]);
        let me = app.world_mut().spawn(crate::net::ObjectStore(purse)).id();
        let mut index = crate::net::GuidIndex::default();
        index.0.insert(0x42, me);
        app.insert_resource(index)
            .insert_resource(crate::net::SelfGuid(Some(0x42)));
        run(&mut app, "PickupPlayerMoney(300)");
        spy(&mut app, &["PLAYER_MONEY", "TRADE_SHOW"]);
        log(&mut app);
        app.world_mut()
            .resource_mut::<TradeSession>()
            .initiate(0x7, true);
        status(&mut app, TradeStatus::OpenWindow { trade_id: 10 });
        assert_eq!(
            log(&mut app),
            "PLAYER_TRADE_MONEY PLAYER_MONEY: TRADE_SHOW:"
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(script.trade_offer_gold(), 300);
        assert_eq!(script.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
    }

    /// `CloseTrade` clears the interact target on the partner inside its call, ahead of `TRADE_CLOSED` (`0x5879a5`, `0x5879bc`), with no drain after it.
    #[test]
    fn close_trade_clears_the_interact_target_in_its_call() {
        let (mut app, _rx) = rig();
        app.init_resource::<crate::ui_session::InteractTarget>();
        crate::ui_session::set_interact_target(app.world_mut(), 0x7);
        crate::ui_script::live::fire_handler_events(app.world_mut());
        let held = |app: &App| {
            app.world()
                .resource::<crate::ui_session::InteractTarget>()
                .0
        };
        assert_eq!(held(&app), Some(0x7));
        log(&mut app);
        run(&mut app, "CloseTrade()");
        assert_eq!(held(&app), None);
        assert_eq!(log(&mut app), "PLAYER_TRADE_MONEY TRADE_CLOSED");
    }

    /// `0x5880c7`, `0x5880f4`, `0x588133`, `0x588138`: the world leave unlocks the offer and the enchanting item, closes the window with `TRADE_CLOSED` and sends the cancel twice.
    #[test]
    fn the_world_leave_unlocks_the_offer_and_cancels_twice() {
        let (mut app, rx) = offered();
        super::super::leave_world(app.world_mut());
        crate::ui_script::live::fire_handler_events(app.world_mut());
        assert_eq!(
            log(&mut app),
            "ITEM_UNLOCKED:1 ITEM_UNLOCKED:2 TRADE_CLOSED"
        );
        assert!(!app.world().resource::<TradeSession>().is_open());
        assert_eq!(sent(&rx), ["CancelTrade", "CancelTrade"]);
    }
}
