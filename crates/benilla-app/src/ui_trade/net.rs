//! The trade's packet handlers: `SMSG_TRADE_STATUS` drives the open, accept and close state and
//! `SMSG_TRADE_STATUS_EXTENDED` replaces one side's offer, both into [`TradeSession`]. The Lua
//! events they cause are fired by [`super::feed_trade`], which owns the VM.

use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{NamedLine, OfferUpdate, TradeSession};
use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_action::{FillArg, UiError, UiErrorKeys};

/// Register the trade's handlers: one per kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TradeStatus, on_trade_status)
        .net_handler(K::TradeStatusExtended, on_trade_status_extended)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_trade_status(
    In(ev): In<SessionEvent>,
    mut trade: ResMut<TradeSession>,
    mut errors: ResMut<UiErrorKeys>,
    commands: Res<NetCommands>,
    items: Option<Res<Items>>,
    limits: Option<Res<super::ItemLimitCategories>>,
    ui: Option<ResMut<crate::ui_script::live::UiEvents>>,
) {
    if let SessionEvent::TradeStatus { status } = ev {
        use crate::ui_session::NpcSession;
        // The partner setter takes the interact target on the open (`0x5874cb` -> `0x587124`) and clears it on the close (`0x5870f7`), whose interaction close takes the window bound to it.
        let target = match status {
            TradeStatus::OpenWindow { .. } => trade.partner.map(|p| (p, true)),
            TradeStatus::Canceled | TradeStatus::Complete | TradeStatus::CloseWindow { .. } => {
                trade.npc().map(|p| (p, false))
            }
            _ => None,
        };
        if let Some(mut ui) = ui {
            // The packet's own arms run first: our offered coins zero on the open and on each close (`0x704545`, `0x70459f`, `0x7045eb`), on the coins' refusal (`0x70457c`), and codes 3 and 13 signal `PLAYER_MONEY` (`0x7045ab`).
            let zeroes = match status {
                TradeStatus::OpenWindow { .. }
                | TradeStatus::Canceled
                | TradeStatus::Complete
                | TradeStatus::CloseWindow { .. }
                | TradeStatus::Unknown13 => true,
                TradeStatus::OnlyConjured { slot } | TradeStatus::NotEligible { slot } => {
                    slot == 0xff
                }
                _ => false,
            };
            if zeroes {
                ui.write(|script| script.set_trade_offer_gold(0));
            }
            if matches!(status, TradeStatus::Canceled | TradeStatus::Unknown13) {
                ui.fire("PLAYER_MONEY", vec![]);
            }
            // The cancel signals first (`0x587503`); each close unlocks the offer's items (`0x587508`, `0x587529`, `0x58754a`) ahead of the partner setter's close.
            if matches!(status, TradeStatus::Canceled) {
                ui.fire("TRADE_REQUEST_CANCEL", vec![]);
            }
            if matches!(
                status,
                TradeStatus::Canceled | TradeStatus::Complete | TradeStatus::CloseWindow { .. }
            ) {
                ui.write(benilla_ui::script::UiScript::unlock_trade_items);
            }
            if let Some((partner, open)) = target {
                // The close releases the enchanting item and keeps the completion flag (`0x5870d5`, `0x5870f2`), then drops the interact target (`0x5870f7`).
                let completed = matches!(status, TradeStatus::Complete);
                if !open {
                    ui.write(move |script| script.close_trade_partner(completed));
                }
                ui.world(move |w| {
                    if open {
                        crate::ui_session::set_interact_target(w, partner);
                    } else {
                        crate::ui_session::clear_interact_target(w, partner);
                    }
                });
            }
            // The refused slot's item unlocks and its slot zeroes after the line (`0x587727`, `0x587742` -> `0x586410`); `0xff` is the coins.
            if let TradeStatus::OnlyConjured { slot } | TradeStatus::NotEligible { slot } = status {
                if slot != 0xff {
                    ui.write(move |script| script.deny_trade_item(slot));
                }
            }
        }
        if let TradeStatus::CloseWindow {
            result,
            target,
            item,
        } = status
        {
            // The item cache alone, never asked (`0x58757c`).
            let limit = |entry: u32| {
                let category = items
                    .as_deref()?
                    .template_cached(entry)?
                    .item_limit_category;
                let row = limits.as_deref()?.0.get(category)?;
                Some((row.quantity, row.name.clone()))
            };
            errors
                .0
                .extend(close_window_line(result, target, item, &limit));
        }
        trade_status(status, &mut trade, &mut errors, &commands);
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
    ui: Option<ResMut<crate::ui_script::live::UiEvents>>,
) {
    if let SessionEvent::TradeStatusExtended { state } = ev {
        let seventh = trade.their_seventh();
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
        // The partner's seventh slot changing releases the enchanting item (`0x586fb6`).
        if trade.their_seventh() != seventh {
            ui.write(benilla_ui::script::UiScript::release_trade_enchanting_item);
        }
    }
}

/// An open trade dies with the socket, after `net::session::on_disconnected`'s own teardown.
fn on_session_end(In(_): In<SessionEvent>, mut trade: ResMut<TradeSession>) {
    trade.clear_session();
}

/// `SMSG_TRADE_STATUS`, arm for arm the reference's 23-case dispatcher: an arm may print
/// ([`status_message`]), only `CANCELED`, `COMPLETE` and `CLOSE_WINDOW` close the window, and the
/// handler's tail then clears the network cells ([`TradeSession::clear_pending`]). `BEGIN_TRADE`
/// only records the request for [`super::answer_trade_request`].
fn trade_status(
    status: TradeStatus,
    trade: &mut TradeSession,
    errors: &mut UiErrorKeys,
    commands: &NetCommands,
) {
    bevy::log::info!(target: "trade", "SMSG_TRADE_STATUS {status:?}");

    // The line first: a naming arm reads the guid cells before the tail clears them, the
    // reference's order. A `%s` line waits for a name.
    match status_message(status) {
        Some(Line::Plain(key)) => errors.0.push(UiError::key(key)),
        Some(Line::Naming(key)) => {
            if let Some(who) = trade.line_names() {
                trade.owe_named_line(NamedLine {
                    key,
                    who,
                    // The arm reads the live unit; the feed applies that guard.
                    needs_live_object: true,
                });
            }
        }
        None => {}
    }

    match status {
        TradeStatus::BeginTrade { partner } => trade.request(partner),
        TradeStatus::OpenWindow { trade_id } => trade.open_window(trade_id),
        // Case 4 sets the partner's accept flag and case 9 clears it; neither touches ours, closes
        // anything or is in the tail's clear set.
        TradeStatus::Accept => trade.partner_accepted(),
        TradeStatus::Rejected => trade.partner_unaccepted(),
        // Case 7 drops both accepts, ours through the unaccept, which sends `CMSG_UNACCEPT_TRADE`
        // when the flag was up; vmangos then passes a `BACK_TO_TRADE` on to the partner
        // (`TradeHandler.cpp:539`).
        TradeStatus::BackToTrade => {
            if trade.we_accepted() {
                let _ = commands.0.send(ClientCommand::UnacceptTrade);
            }
            trade.back_to_trade();
        }
        // The three that close the window, each through the partner setter, whose close ends on `CMSG_CANCEL_TRADE` whether a window stood or not (`0x587116` -> `0x704040`).
        TradeStatus::Canceled | TradeStatus::Complete | TradeStatus::CloseWindow { .. } => {
            trade.close_window();
            let _ = commands.0.send(ClientCommand::CancelTrade);
        }
        // The placement is bounced, not the window.
        TradeStatus::OnlyConjured { slot } | TradeStatus::NotEligible { slot } => {
            trade.bounce_own_offer(slot)
        }
        // The initiate refusals: the tail's clear alone, each line already queued above.
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
        // Past `0x16`: the bare epilogue; no server sends one.
        TradeStatus::Unknown(_) => {}
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

    fn item() -> benilla_protocol::messages::TradeItem {
        benilla_protocol::messages::TradeItem {
            entry: 1234,
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

    fn sink() -> (
        UiErrorKeys,
        NetCommands,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (UiErrorKeys::default(), NetCommands(tx), rx)
    }

    /// A window up and both sides accepted, so a wrong close, accept reset or cell clear shows.
    fn open_trade() -> TradeSession {
        let mut trade = TradeSession::default();
        trade.initiate(0x7, false);
        trade.open_window(0);
        trade.accept();
        trade.partner_accepted();
        trade.take_named_lines_for_test();
        trade
    }

    #[test]
    fn the_table_routes_the_status_and_the_session_end_to_the_trade() {
        let (_, commands, _rx) = sink();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<TradeSession>()
            .init_resource::<UiErrorKeys>()
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
                let (mut errors, commands, _rx) = sink();
                let mut trade = open_trade();
                trade_status(*status, &mut trade, &mut errors, &commands);
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
            let (mut errors, commands, _rx) = sink();
            let mut trade = open_trade();
            trade_status(status, &mut trade, &mut errors, &commands);
            assert!(trade.is_open(), "{status:?} must not close the window");
            assert!(
                trade.line_names().is_none(),
                "{status:?} must drop the initiate latch"
            );
            assert!(
                trade.we_accepted(),
                "{status:?} must not touch the accept glow"
            );
        }
    }

    #[test]
    fn rejected_only_drops_the_partners_accept() {
        let (mut errors, commands, _rx) = sink();
        let mut trade = open_trade();
        trade_status(TradeStatus::Rejected, &mut trade, &mut errors, &commands);
        assert!(trade.is_open());
        assert!(trade.we_accepted(), "ours is untouched");
        assert!(!trade.partner_has_accepted(), "theirs is dropped");
        assert!(errors.0.is_empty(), "and it says nothing");
        assert!(trade.line_names().is_some(), "9 is not in the clear set");
    }

    #[test]
    fn back_to_trade_unaccepts_on_the_wire_but_only_if_we_had_accepted() {
        let (mut errors, commands, rx) = sink();
        let mut trade = open_trade(); // we accepted
        trade_status(TradeStatus::BackToTrade, &mut trade, &mut errors, &commands);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::UnacceptTrade)));
        assert!(!trade.we_accepted());
        assert!(errors.0.is_empty(), "the bounce is silent");

        // Idempotent: a second bounce with no accept up sends nothing.
        trade_status(TradeStatus::BackToTrade, &mut trade, &mut errors, &commands);
        assert!(rx.try_recv().is_err());
    }

    /// `0xff` names the gold; an out-of-range slot (200) is dropped.
    #[test]
    fn only_conjured_bounces_one_slot_and_leaves_the_window_up() {
        // Wire slot 1 is UI slot 2, the one `place_own_item` fills below.
        for (slot, expect_gold, expect_slot2) in
            [(1u8, 700u32, false), (0xff, 0, true), (200, 700, true)]
        {
            let (mut errors, commands, _rx) = sink();
            let mut trade = open_trade();
            trade.place_own_item(2, item());
            trade.set_own_gold(700);
            trade_status(
                TradeStatus::OnlyConjured { slot },
                &mut trade,
                &mut errors,
                &commands,
            );
            assert!(trade.is_open(), "slot {slot}: the window stays up");
            let (gold, filled) = trade.own_offer_for_test();
            assert_eq!(gold, expect_gold, "slot {slot}");
            assert_eq!(filled[1], expect_slot2, "slot {slot}");
            assert_eq!(errors.0, vec![UiError::key("ERR_TRADE_WRONG_REALM")]);
        }
    }

    /// Cases 0, 5 and 14 print only under the initiate latch, so the refuser stays silent when
    /// vmangos echoes `IGNORE_YOU` to both sides (`TradeHandler.cpp:46`).
    #[test]
    fn the_naming_lines_are_gated_on_having_initiated() {
        for status in [
            TradeStatus::Busy,
            TradeStatus::Busy2,
            TradeStatus::IgnoreYou,
        ] {
            // We asked: the line is owed, naming our target.
            let (mut errors, commands, _rx) = sink();
            let mut trade = TradeSession::default();
            trade.initiate(0x7, false);
            trade.take_named_lines_for_test();
            trade_status(status, &mut trade, &mut errors, &commands);
            assert_eq!(
                trade.take_named_lines_for_test(),
                vec![NamedLine {
                    key: match status {
                        TradeStatus::IgnoreYou => "ERR_IGNORING_YOU_S",
                        _ => "ERR_PLAYER_BUSY_S",
                    },
                    who: 0x7,
                    needs_live_object: true,
                }],
                "{status:?}: the asker is told, and told about its target"
            );
            assert!(errors.0.is_empty(), "a %s line waits for the name cache");

            // We did not ask: the refuser's own echo is silent.
            let (mut errors, commands, _rx) = sink();
            let mut trade = TradeSession::default();
            trade.request(0x7);
            trade.begin(0x7); // accepted an incoming request: no latch
            trade_status(status, &mut trade, &mut errors, &commands);
            assert!(
                trade.take_named_lines_for_test().is_empty() && errors.0.is_empty(),
                "{status:?}: the side that did not initiate hears nothing"
            );
        }
    }

    #[test]
    fn the_plain_refusals_are_raised_immediately() {
        let (mut errors, commands, _rx) = sink();
        let mut trade = TradeSession::default();
        trade.initiate(0x7, false);
        trade.take_named_lines_for_test();
        trade_status(TradeStatus::TargetDead, &mut trade, &mut errors, &commands);
        assert_eq!(errors.0, vec![UiError::key("ERR_TRADE_TARGET_DEAD")]);
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
    use benilla_ui::script::{ContainerSlot, ContainerState, TradeState, UiScript};
    use bevy::prelude::*;

    use super::super::TradeSession;
    use crate::net::{ClientCommand, NetCommands};

    /// A world with the trade's handlers, an open window on `0x7`, a pack of three items and a spy logging the trade and lock events.
    fn rig() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
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
        script.set_trade(Some(TradeState::default()));
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

    /// `0x587503`, `0x587508`, `0x587513`: the cancel signals, the offer's items unlock, then the partner setter's close sends the cancel.
    #[test]
    fn a_cancel_signals_then_unlocks_the_offer_and_sends_the_cancel() {
        let (mut app, rx) = offered();
        status(&mut app, TradeStatus::Canceled);
        assert_eq!(
            log(&mut app),
            "TRADE_REQUEST_CANCEL ITEM_UNLOCKED:1 ITEM_UNLOCKED:2"
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
            assert_eq!(log(&mut app), "ITEM_UNLOCKED:1 ITEM_UNLOCKED:2");
            assert_eq!(sent(&rx), ["CancelTrade"]);
            // The feed's push of the closed window, then the frame's `CloseTrade`.
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_trade(None);
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
        assert_eq!(log(&mut app), "ITEM_UNLOCKED:2");

        // The status first: both unlock, and the destroy then takes the item out of the slots that stand.
        let (mut app, _rx) = offered();
        status(&mut app, TradeStatus::Complete);
        assert_eq!(log(&mut app), "ITEM_UNLOCKED:1 ITEM_UNLOCKED:2");
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
        // `0xff` is the coins: no slot is touched (`0x586418`).
        let (mut app, _rx) = offered();
        status(&mut app, TradeStatus::OnlyConjured { slot: 0xff });
        assert_eq!(log(&mut app), "");
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
            snapshot(app, true, 2, &[(6, 6217)]);
            // The feed's push of the partner's column.
            let mut st = TradeState::default();
            st.target.slots[6] = Some(Default::default());
            let facts = benilla_ui::script::TradeItemFacts {
                enchants_on_use: true,
                ..Default::default()
            };
            {
                let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
                script.set_trade(Some(st));
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
        assert_eq!(log(&mut app), "TRADE_REQUEST_CANCEL ITEM_UNLOCKED:3");
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
        assert_eq!(log(&mut app), "PLAYER_MONEY TRADE_REQUEST_CANCEL");
        let (mut app, _rx) = rig();
        run(&mut app, spy);
        status(&mut app, TradeStatus::Unknown13);
        assert_eq!(log(&mut app), "PLAYER_MONEY");
        status(&mut app, TradeStatus::Complete);
        assert_eq!(log(&mut app), "", "a completion signals no purse");

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

    /// `0x5880c7`, `0x5880f4`, `0x588133`, `0x588138`: the world leave unlocks the offer and the enchanting item, closes the window and sends the cancel twice.
    #[test]
    fn the_world_leave_unlocks_the_offer_and_cancels_twice() {
        let (mut app, rx) = offered();
        super::super::leave_world(app.world_mut());
        crate::ui_script::live::fire_handler_events(app.world_mut());
        assert_eq!(log(&mut app), "ITEM_UNLOCKED:1 ITEM_UNLOCKED:2");
        assert!(!app.world().resource::<TradeSession>().is_open());
        assert_eq!(sent(&rx), ["CancelTrade", "CancelTrade"]);
    }
}
