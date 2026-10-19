//! The auction live probe (`WOW_PROBE_AUCTION=1`): hops to a Stormwind auctioneer, greets it on
//! the wire and drives the stock 3.3.5a auction window through the live Lua VM as clicks would,
//! logging one `PROBE_AUCTION:` PASS/FAIL/SKIP line per step, then
//! `PROBE_AUCTION: DONE pass=<n> fail=<m>`, and exits. Non-combat; the switches are
//! `docs/CONTRIBUTING.md`, "Running it unattended".
//!
//! The arc: the window's open state (12 classes, no rows, the search prompt, the portrait),
//! a browse and a header click that re-sorts through the server, the filter tree, a two-stack
//! multi-sell put in by right-click on the Auctions tab, a bid and a buyout on another seller's
//! auctions, the Bids tab, then cancelling this run's listings. `WOW_PROBE_AUCTION_KEEP=1` keeps
//! the listings and skips bidding: run it on a second account first to leave auctions to bid on.

use bevy::prelude::*;

use benilla_protocol::messages::{auction_action, auction_error, auction_sort};
use benilla_protocol::EntityKind;
use benilla_ui::script::{UiScript, LIST, OWNER};

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_auction::AuctionOpen;

/// Auctioneer Fitch's spawn (map 0, Stormwind Trade District), the `.go xyz` target; the unit
/// itself is found by its npc flag.
const AUCTIONEER_AT: [f32; 3] = [-8821.53, 659.886, 97.4645];
const NPC_FLAG_AUCTIONEER: u32 = crate::target::cursor_mode::npc_flags::AUCTIONEER;
/// Wider than the interact range, so a slightly-off `.go` landing still finds one.
const SCAN_RANGE: f32 = 12.0;
const INTERACT_MAX_YD: f32 = 5.0;

/// The multi-sell fixture: Linen Cloth, stacking to 20; two auctions of 20.
const ITEM_ENTRY: u32 = 2589;
const STACK_SIZE: u32 = 20;
const NUM_STACKS: u32 = 2;
const START_PRICE: u32 = 5_000;
const BUYOUT_PRICE: u32 = 10_000;

const SETTLE_SECS: f64 = 3.0;
const TIMEOUT_SECS: f64 = 15.0;
const ITEM_TIMEOUT_SECS: f64 = 10.0;

pub(crate) struct ProbeAuctionPlugin;

impl Plugin for ProbeAuctionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuctionProbe>()
            .add_systems(Update, auction_probe);
    }
}

#[derive(Resource, Default)]
struct AuctionProbe {
    phase: Phase,
    auctioneer: Option<u64>,
    /// The auctions this run listed, from their `STARTED` results.
    listed: Vec<u32>,
    /// Command results already read, an index into the wire log.
    results_seen: usize,
    /// The purse before the multi-sell, and the client's deposit quote for it.
    money_before: u32,
    deposit: i64,
    /// The browse row picked for the bid, by `(name, owner)`.
    bid_target: Option<String>,
    /// When the last stack's answer landed: the slot and purse settle a moment after.
    settled_at: Option<f64>,
    /// The last owner-list ask, which the server answers up to 4.5 s late.
    last_ask: f64,
    /// `.modify money` went out.
    funded: bool,
    passes: u32,
    fails: u32,
    keep: bool,
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    Settling {
        since: f64,
    },
    Greet {
        since: f64,
    },
    /// The owner and bidder lists the open asked for; logs what a relog brings back.
    Lists {
        since: f64,
    },
    Browse {
        since: f64,
        sent: bool,
        results: u32,
    },
    Sort {
        since: f64,
        sent: bool,
        results: u32,
    },
    Filters {
        since: f64,
        sent: bool,
        results: u32,
    },
    EnsureItem {
        since: f64,
        sent: bool,
    },
    Attach {
        since: f64,
    },
    MultiSell {
        since: f64,
    },
    Owned {
        since: f64,
    },
    BidSearch {
        since: f64,
        sent: bool,
        results: u32,
    },
    Bid {
        since: f64,
        buyout: bool,
    },
    BidsTab {
        since: f64,
    },
    Cancel {
        since: f64,
        sent: bool,
    },
    Done,
}

fn pass(probe: &mut AuctionProbe, line: &str) {
    info!("PROBE_AUCTION: PASS {line}");
    probe.passes += 1;
}

fn fail(probe: &mut AuctionProbe, line: &str) {
    error!("PROBE_AUCTION: FAIL {line}");
    probe.fails += 1;
}

/// One Lua-side event counter, or 0 on an eval error.
fn event_count(script: &UiScript, event: &str) -> i64 {
    script
        .eval::<i64>(&format!(
            "return (ProbeAuctionEvents or {{}})['{event}'] or 0"
        ))
        .unwrap_or(0)
}

/// The arguments of the event's latest fire, comma-joined.
fn event_args(script: &UiScript, event: &str) -> String {
    script
        .eval::<String>(&format!(
            "return (ProbeAuctionArgs or {{}})['{event}'] or ''"
        ))
        .unwrap_or_default()
}

/// Lua errors raised by the auction addon since the hook went in, from the error handler's log.
fn auction_errors(script: &UiScript) -> String {
    script
        .eval::<String>("return table.concat(ProbeAuctionLuaErrors or {}, ' | ')")
        .unwrap_or_default()
}

/// Whether every error is the reference's own: the multi-sell branches of
/// `AuctionFrameAuctions_OnEvent` read the global `arg1`, which 12340 never sets.
fn only_reference_errors(errors: &str) -> bool {
    errors
        .split(" | ")
        .filter(|e| !e.is_empty())
        .all(|e| e.contains("AuctionFrameAuctions_OnEvent"))
}

/// `CanSendAuctionQuery()` for the Browse list.
fn can_query(script: &UiScript) -> bool {
    script
        .eval::<bool>("return CanSendAuctionQuery() ~= nil")
        .unwrap_or(false)
}

/// The red `UI_ERROR_MESSAGE` lines raised so far.
fn red_errors(script: &UiScript) -> String {
    script
        .eval::<String>("return table.concat(ProbeAuctionErrors or {}, ' | ')")
        .unwrap_or_default()
}

/// The command results that landed since the last call.
fn new_results(probe: &mut AuctionProbe, auction: &AuctionOpen) -> Vec<(u32, u32, u32)> {
    let fresh = auction.wire.results[probe.results_seen.min(auction.wire.results.len())..].to_vec();
    probe.results_seen = auction.wire.results.len();
    fresh
}

/// Evaluate a chunk, a failure logged and read as the type's default.
macro_rules! eval {
    ($script:expr, $chunk:expr $(,)?) => {
        $script.eval($chunk).unwrap_or_else(|e| {
            warn!("PROBE_AUCTION: eval failed: {e}");
            Default::default()
        })
    };
}

#[allow(clippy::too_many_lines)]
fn auction_probe(
    time: ProbeClock,
    mut probe: ResMut<AuctionProbe>,
    auction: Res<AuctionOpen>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    units: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    interact: Res<crate::ui_session::InteractNpc>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(store) = self_player.single() else {
        return;
    };
    let Some(script) = script else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let say = |text: String| {
        let _ = net.0.send(ClientCommand::Chat {
            kind: ChatKind::Say,
            target: None,
            text,
            language: None,
        });
    };

    match probe.phase {
        Phase::Wait => {
            probe.keep = std::env::var("WOW_PROBE_AUCTION_KEEP").is_ok_and(|v| v == "1");
            if let Err(e) = script.run(
                r#"
                if not ProbeAuctionHooked then
                    ProbeAuctionHooked = true
                    ProbeAuctionEvents, ProbeAuctionArgs, ProbeAuctionChat = {}, {}, {}
                    ProbeAuctionErrors, ProbeAuctionLuaErrors = {}, {}
                    local f = CreateFrame("Frame")
                    for _, e in ipairs({"AUCTION_HOUSE_SHOW", "AUCTION_HOUSE_CLOSED",
                        "AUCTION_HOUSE_DISABLED", "AUCTION_ITEM_LIST_UPDATE",
                        "AUCTION_OWNED_LIST_UPDATE", "AUCTION_BIDDER_LIST_UPDATE",
                        "NEW_AUCTION_UPDATE", "AUCTION_MULTISELL_START", "AUCTION_MULTISELL_UPDATE",
                        "AUCTION_MULTISELL_FAILURE", "UI_ERROR_MESSAGE", "CHAT_MSG_SYSTEM"}) do
                        f:RegisterEvent(e)
                    end
                    f:SetScript("OnEvent", function(self, event, a1, a2)
                        ProbeAuctionEvents[event] = (ProbeAuctionEvents[event] or 0) + 1
                        ProbeAuctionArgs[event] = tostring(a1 or "") .. "," .. tostring(a2 or "")
                        if event == "UI_ERROR_MESSAGE" then
                            table.insert(ProbeAuctionErrors, a1 or "")
                        elseif event == "CHAT_MSG_SYSTEM" then
                            table.insert(ProbeAuctionChat, a1 or "")
                        end
                    end)
                    local prev = geterrorhandler()
                    seterrorhandler(function(msg)
                        if string.find(tostring(msg), "Blizzard_Auction") then
                            table.insert(ProbeAuctionLuaErrors, tostring(msg))
                        end
                        return prev(msg)
                    end)
                end
                "#,
            ) {
                fail(&mut probe, &format!("(0 setup) — installing the hook: {e}"));
                probe.phase = Phase::Done;
                return;
            }
            let [x, y, z] = AUCTIONEER_AT;
            say(format!(".go xyz {x} {y} {z} 0"));
            probe.phase = Phase::Settling { since: now };
        }
        Phase::Settling { since } => {
            if now - since < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let nearest = units
                .iter()
                .filter(|(_, net_e, store, tf)| {
                    net_e.kind == EntityKind::Unit
                        && store.0.unit_npc_flags() & NPC_FLAG_AUCTIONEER != 0
                        && tf.translation.distance(me) < SCAN_RANGE
                })
                .map(|(guid, _, _, tf)| (guid.0, tf.translation.distance(me)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((guid, dist)) = nearest.filter(|n| n.1 <= INTERACT_MAX_YD) {
                pass(
                    &mut probe,
                    &format!("(1 reach) — auctioneer {guid:#x} {dist:.1} yd away"),
                );
                probe.auctioneer = Some(guid);
                let _ = net.0.send(ClientCommand::AuctionHello { auctioneer: guid });
                probe.phase = Phase::Greet { since: now };
            } else if now - since > TIMEOUT_SECS {
                fail(
                    &mut probe,
                    &format!("(1 reach) — no auctioneer within {INTERACT_MAX_YD} yd: {nearest:?}"),
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Greet { since } => {
            let shown: bool = eval!(
                script,
                "return AuctionFrame ~= nil and AuctionFrame:IsShown() ~= nil"
            );
            if auction.auctioneer.is_some() && shown {
                // The open state the issue describes: 12 classes, no rows before a search, the
                // search prompt, the default sort, and the "npc" portrait bound to the auctioneer
                // (a unit binding has no texture path for Lua to read).
                let (classes, row, prompt, sort): (i64, bool, bool, String) = eval!(
                    &script,
                    "return select('#', GetAuctionItemClasses()), BrowseButton1:IsShown() ~= nil, \
                     BrowseNoResultsText:GetText() == BROWSE_SEARCH_TEXT, GetAuctionSort('list', 1) or ''",
                );
                let portrait = interact.0.is_some() && interact.1 == probe.auctioneer;
                let line = format!(
                    "classes={classes} row1_shown={row} search_prompt={prompt} portrait={portrait} \
                     sort1={sort:?} house={}",
                    auction.house_id
                );
                if classes == 12 && !row && prompt && portrait && sort == "quality" {
                    pass(&mut probe, &format!("(2 open) — {line}"));
                } else {
                    fail(&mut probe, &format!("(2 open) — {line}"));
                }
                probe.phase = Phase::Lists { since: now };
            } else if now - since > TIMEOUT_SECS {
                fail(
                    &mut probe,
                    &format!(
                        "(2 open) — session {:?}, AuctionFrame shown {shown}",
                        auction.auctioneer
                    ),
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Lists { since } => {
            // AuctionFrame_Show asks the owner and bidder lists once per opening.
            let landed = auction.wire.list_results[OWNER] > 0 && auction.wire.list_results[1] > 0;
            if landed || now - since > TIMEOUT_SECS {
                let (owned, bids): (i64, i64) = eval!(
                    &script,
                    "return GetNumAuctionItems('owner'), GetNumAuctionItems('bidder')",
                );
                let line = format!(
                    "owner results={} rows={owned}, bidder results={} rows={bids}",
                    auction.wire.list_results[OWNER], auction.wire.list_results[1]
                );
                if landed {
                    pass(&mut probe, &format!("(3 lists) — {line}"));
                } else {
                    fail(&mut probe, &format!("(3 lists) — {line}"));
                }
                probe.phase = Phase::Browse {
                    since: now,
                    sent: false,
                    results: auction.wire.list_results[LIST],
                };
            }
        }
        Phase::Browse {
            since,
            sent,
            results,
        } => {
            if !sent {
                if can_query(&script) {
                    crate::ui_script::run_or_warn(&script, "AuctionFrameBrowse_Search()");
                    probe.phase = Phase::Browse {
                        since: now,
                        sent: true,
                        results,
                    };
                } else if now - since > TIMEOUT_SECS {
                    fail(&mut probe, "(4 browse) — CanSendAuctionQuery never opened");
                    probe.phase = Phase::Done;
                }
                return;
            }
            if auction.wire.list_results[LIST] > results
                && event_count(&script, "AUCTION_ITEM_LIST_UPDATE") > 0
            {
                let (batch, total, shown, name, prompt): (i64, i64, bool, String, bool) = eval!(
                    &script,
                    "local b, t = GetNumAuctionItems('list') \
                     return b, t, BrowseButton1:IsShown() ~= nil, BrowseButton1Name:GetText() or '', \
                     BrowseNoResultsText:IsShown() ~= nil",
                );
                let line = format!(
                    "rows={batch} total={total} row1_shown={shown} row1={name:?} no_results_shown={prompt}"
                );
                let ok = if batch > 0 {
                    shown && !name.is_empty() && !prompt
                } else {
                    !shown && prompt
                };
                if ok {
                    pass(&mut probe, &format!("(4 browse) — {line}"));
                } else {
                    fail(&mut probe, &format!("(4 browse) — {line}"));
                }
                probe.phase = Phase::Sort {
                    since: now,
                    sent: false,
                    results: auction.wire.list_results[LIST],
                };
            } else if now - since > TIMEOUT_SECS {
                fail(&mut probe, "(4 browse) — no list result");
                probe.phase = Phase::Done;
            }
        }
        Phase::Sort {
            since,
            sent,
            results,
        } => {
            if !sent {
                if can_query(&script) {
                    // The Level header: a new column opens reversed, and the search re-asks.
                    crate::ui_script::run_or_warn(
                        &script,
                        "AuctionFrame_OnClickSortColumn('list', 'level')",
                    );
                    probe.phase = Phase::Sort {
                        since: now,
                        sent: true,
                        results,
                    };
                } else if now - since > TIMEOUT_SECS {
                    fail(&mut probe, "(5 sort) — CanSendAuctionQuery never reopened");
                    probe.phase = Phase::Done;
                }
                return;
            }
            if auction.wire.list_results[LIST] > results {
                let front = auction.wire.last_sort.first().copied();
                let (col, rev): (String, bool) = eval!(
                    script,
                    "local c, r = GetAuctionSort('list', 1) return c or '', r ~= nil"
                );
                let line = format!(
                    "wire criteria {:?}, GetAuctionSort('list',1)=({col:?}, {rev})",
                    auction.wire.last_sort
                );
                if front == Some((auction_sort::LEVEL, true)) && col == "level" && rev {
                    pass(&mut probe, &format!("(5 sort) — {line}"));
                } else {
                    fail(&mut probe, &format!("(5 sort) — {line}"));
                }
                probe.phase = Phase::Filters {
                    since: now,
                    sent: false,
                    results: auction.wire.list_results[LIST],
                };
            } else if now - since > TIMEOUT_SECS {
                fail(&mut probe, "(5 sort) — the header click sent no query");
                probe.phase = Phase::Done;
            }
        }
        Phase::Filters {
            since,
            sent,
            results,
        } => {
            if !sent {
                if !can_query(&script) {
                    if now - since > TIMEOUT_SECS {
                        fail(
                            &mut probe,
                            "(6 filters) — CanSendAuctionQuery never reopened",
                        );
                        probe.phase = Phase::Done;
                    }
                    return;
                }
                // Weapon, its first subclass, then Armor and its second (Cloth), as clicks.
                let (subs, weapon_inv, armor_inv, cloak, class, sub): (i64, i64, i64, bool, i64, i64) =
                    eval!(
                        &script,
                        "AuctionFrameBrowse_Reset(BrowseResetButton) \
                         local function count(kind, shown) local n = 0 \
                           for _, f in ipairs(OPEN_FILTER_LIST) do \
                             if f[2] == kind and (not shown or f[6]) then n = n + 1 end end return n end \
                         AuctionFilterButton1:Click() local subs = count('subclass') \
                         AuctionFilterButton2:Click() local winv = count('invtype', true) \
                         AuctionFilterButton1:Click() AuctionFilterButton2:Click() AuctionFilterButton4:Click() \
                         local ainv, cloak = 0, false \
                         for _, f in ipairs(OPEN_FILTER_LIST) do if f[2] == 'invtype' and f[6] then \
                           ainv = ainv + 1 if string.find(f[1], INVTYPE_CLOAK, 1, true) then cloak = true end end end \
                         AuctionFrameBrowse_Search() \
                         return subs, winv, ainv, cloak, AuctionFrameBrowse.selectedClassIndex or -1, \
                           AuctionFrameBrowse.selectedSubclassIndex or -1",
                    );
                info!(
                    "PROBE_AUCTION: (6 filters) weapon subclasses={subs} weapon slots={weapon_inv} \
                     cloth slots={armor_inv} cloak={cloak} selected=({class}, {sub})"
                );
                if subs > 0 && weapon_inv == 0 && armor_inv > 0 && cloak && (class, sub) == (2, 2) {
                    pass(&mut probe, "(6 filters) — Weapon lists its subclasses without slots; Cloth lists its slots, cloak included");
                } else {
                    fail(
                        &mut probe,
                        "(6 filters) — the tree did not open as the reference's",
                    );
                }
                probe.phase = Phase::Filters {
                    since: now,
                    sent: true,
                    results,
                };
                return;
            }
            if auction.wire.list_results[LIST] > results {
                let filter = auction.wire.last_filter;
                if matches!(filter, Some((_, 4, 1))) {
                    pass(&mut probe, &format!("(6b filter-query) — Armor/Cloth went out as class 4 subclass 1: {filter:?}"));
                } else {
                    fail(
                        &mut probe,
                        &format!("(6b filter-query) — wire filters {filter:?}"),
                    );
                }
                crate::ui_script::run_or_warn(
                    &script,
                    "AuctionFrameBrowse_Reset(BrowseResetButton)",
                );
                probe.phase = Phase::EnsureItem {
                    since: now,
                    sent: false,
                };
            } else if now - since > TIMEOUT_SECS {
                fail(&mut probe, "(6b filter-query) — no list result");
                probe.phase = Phase::EnsureItem {
                    since: now,
                    sent: false,
                };
            }
        }
        Phase::EnsureItem { since, sent } => {
            let total: i64 = eval!(
                &script,
                &format!(
                    "local t = 0 for bag = 0, 4 do for slot = 1, GetContainerNumSlots(bag) or 0 do \
                     local link = GetContainerItemLink(bag, slot) \
                     if link and string.find(link, 'item:{ITEM_ENTRY}:', 1, true) then \
                       local _, c = GetContainerItemInfo(bag, slot) t = t + (c or 0) end end end return t"
                ),
            );
            let wanted = i64::from(STACK_SIZE * NUM_STACKS);
            // A purse for the deposits, a bid and a buyout; the probe accounts are GMs.
            let money = store.0.player_money().unwrap_or(0);
            if money < 100_000 {
                if !probe.funded {
                    probe.funded = true;
                    info!("PROBE_AUCTION: (7 sell) purse {money}c — `.modify money 1000000`");
                    say(".modify money 1000000".into());
                }
                if now - since < ITEM_TIMEOUT_SECS {
                    return;
                }
            }
            if total >= wanted {
                info!("PROBE_AUCTION: (7 sell) {total} of item {ITEM_ENTRY} in the bags");
                probe.phase = Phase::Attach { since: now };
            } else if !sent {
                say(format!(".additem {ITEM_ENTRY} {}", wanted - total));
                probe.phase = Phase::EnsureItem {
                    since: now,
                    sent: true,
                };
            } else if now - since > ITEM_TIMEOUT_SECS {
                fail(
                    &mut probe,
                    &format!("(7 sell) — only {total} of item {ITEM_ENTRY} after .additem"),
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::Attach { since } => {
            // The Auctions tab, then a right-click on a stack: `UseContainerItem`'s auction arm.
            let (name, stack, total, button): (String, i64, i64, bool) = eval!(
                &script,
                &format!(
                    "if not ProbeAuctionAttached then ProbeAuctionAttached = true \
                       AuctionFrameTab_OnClick(AuctionFrameTab3) \
                       for bag = 0, 4 do for slot = 1, GetContainerNumSlots(bag) or 0 do \
                         local link = GetContainerItemLink(bag, slot) \
                         if link and string.find(link, 'item:{ITEM_ENTRY}:', 1, true) then \
                           UseContainerItem(bag, slot) return '', 0, 0, false end end end end \
                     local n, _, _, _, _, _, _, s, t = GetAuctionSellItemInfo() \
                     return n or '', s or 0, t or 0, AuctionsStackSizeEntry:IsShown() ~= nil"
                ),
            );
            if !name.is_empty() {
                let line = format!("sell slot {name:?} stackCount={stack} totalCount={total} stack_entry_shown={button}");
                if stack == i64::from(STACK_SIZE)
                    && total >= i64::from(STACK_SIZE * NUM_STACKS)
                    && button
                {
                    pass(
                        &mut probe,
                        &format!("(7 attach) — right-click filled it: {line}"),
                    );
                } else {
                    fail(&mut probe, &format!("(7 attach) — {line}"));
                }
                probe.money_before = store.0.player_money().unwrap_or(0);
                probe.deposit = eval!(
                    &script,
                    &format!("return CalculateAuctionDeposit(1, {STACK_SIZE}) * {NUM_STACKS}"),
                );
                let enabled: bool = eval!(
                    &script,
                    &format!(
                        "AuctionsStackSizeEntry:SetNumber({STACK_SIZE}) AuctionsNumStacksEntry:SetNumber({NUM_STACKS}) \
                         AuctionFrameAuctions.priceType = 2 UIDropDownMenu_SetSelectedValue(PriceDropDown, 2) \
                         AuctionFrameAuctions.duration = 1 \
                         MoneyInputFrame_SetCopper(StartPrice, {START_PRICE}) \
                         MoneyInputFrame_SetCopper(BuyoutPrice, {BUYOUT_PRICE}) \
                         AuctionsFrameAuctions_ValidateAuction() \
                         local on = AuctionsCreateAuctionButton:IsEnabled() \
                         AuctionsCreateAuctionButton_OnClick() return on ~= nil and on ~= 0"
                    ),
                );
                info!(
                    "PROBE_AUCTION: (8 multisell) Create Auction enabled={enabled}; deposit quote {}c for {NUM_STACKS}x{STACK_SIZE}",
                    probe.deposit
                );
                probe.phase = Phase::MultiSell { since: now };
            } else if now - since > ITEM_TIMEOUT_SECS {
                fail(
                    &mut probe,
                    "(7 attach) — the right-click left the sell slot empty",
                );
                probe.phase = Phase::Done;
            }
        }
        Phase::MultiSell { since } => {
            for (id, action, error) in new_results(&mut probe, &auction) {
                if action == auction_action::STARTED && error == auction_error::OK {
                    probe.listed.push(id);
                } else {
                    fail(
                        &mut probe,
                        &format!("(8 multisell) — result id {id} action {action} error {error}"),
                    );
                }
            }
            let done = probe.listed.len() as u32 >= NUM_STACKS && auction.multisell.is_none();
            if done && now - *probe.settled_at.get_or_insert(now) < 2.0 {
                return;
            }
            if done {
                let start = event_args(&script, "AUCTION_MULTISELL_START");
                let update = event_args(&script, "AUCTION_MULTISELL_UPDATE");
                let (slot_empty, blocked): (bool, bool) = eval!(
                    &script,
                    "return GetAuctionSellItemInfo() == nil, AuctionsBlockFrame:IsShown() ~= nil",
                );
                let spent =
                    i64::from(probe.money_before) - i64::from(store.0.player_money().unwrap_or(0));
                let line = format!(
                    "listed {:?}, sends {:?}, START({start}) UPDATE({update}), slot empty {slot_empty}, \
                     block frame {blocked}, purse down {spent}c for a {}c quote",
                    probe.listed, auction.wire.sells, probe.deposit
                );
                let sends_ok = auction.wire.sells.len() as u32 == NUM_STACKS
                    && auction
                        .wire
                        .sells
                        .iter()
                        .all(|s| s.iter().sum::<u32>() == STACK_SIZE);
                if start.starts_with(&NUM_STACKS.to_string())
                    && update == format!("{NUM_STACKS},{NUM_STACKS}")
                    && sends_ok
                    && slot_empty
                    && spent == probe.deposit
                {
                    pass(&mut probe, &format!("(8 multisell) — {line}"));
                } else {
                    fail(&mut probe, &format!("(8 multisell) — {line}"));
                }
                probe.phase = Phase::Owned { since: now };
            } else if now - since > TIMEOUT_SECS * 2.0 {
                let line = format!(
                    "(8 multisell) — listed {:?} of {NUM_STACKS}; FAILURE fired {} time(s); errors {:?}",
                    probe.listed,
                    event_count(&script, "AUCTION_MULTISELL_FAILURE"),
                    red_errors(&script)
                );
                fail(&mut probe, &line);
                probe.phase = Phase::Owned { since: now };
            }
        }
        Phase::Owned { since } => {
            let ours: Vec<u32> = auction.lists[OWNER]
                .entries
                .iter()
                .map(|e| e.wire.auction_id)
                .filter(|id| probe.listed.contains(id))
                .collect();
            if probe.listed.is_empty() {
                warn!("PROBE_AUCTION: SKIP (9 owned) — nothing was listed");
                probe.phase = Phase::Done;
            } else if ours.len() == probe.listed.len() {
                let (rows, painted): (i64, String) = eval!(
                    &script,
                    "return GetNumAuctionItems('owner'), AuctionsButton1:IsShown() and (AuctionsButton1Name:GetText() or '') or ''",
                );
                pass(
                    &mut probe,
                    &format!("(9 owned) — the Auctions tab holds {rows} row(s), this run's {ours:?}; row 1 paints {painted:?}"),
                );
                probe.phase = if probe.keep {
                    Phase::Done
                } else {
                    Phase::BidSearch {
                        since: now,
                        sent: false,
                        results: auction.wire.list_results[LIST],
                    }
                };
            } else if now - since > TIMEOUT_SECS * 2.0 {
                let line = format!("(9 owned) — owner rows hold {ours:?} of {:?}", probe.listed);
                fail(&mut probe, &line);
                probe.phase = if probe.keep {
                    Phase::Done
                } else {
                    Phase::Cancel {
                        since: now,
                        sent: false,
                    }
                };
            } else if now - probe.last_ask > 5.0 {
                // A revisit of the tab: the stock pane asks once per opening, the gate decides.
                probe.last_ask = now;
                crate::ui_script::run_or_warn(&script, "GetOwnerAuctionItems()");
            }
        }
        Phase::BidSearch {
            since,
            sent,
            results,
        } => {
            if !sent {
                if can_query(&script) {
                    crate::ui_script::run_or_warn(
                        &script,
                        "AuctionFrameTab_OnClick(AuctionFrameTab1) AuctionFrameBrowse_Search()",
                    );
                    probe.phase = Phase::BidSearch {
                        since: now,
                        sent: true,
                        results,
                    };
                } else if now - since > TIMEOUT_SECS {
                    fail(&mut probe, "(10 bid) — CanSendAuctionQuery never reopened");
                    probe.phase = Phase::Cancel {
                        since: now,
                        sent: false,
                    };
                }
                return;
            }
            if auction.wire.list_results[LIST] > results {
                // Another seller's auction with no bid of ours: bid its minimum.
                let picked: String = eval!(
                    &script,
                    "local me = UnitName('player') local n = GetNumAuctionItems('list') \
                     for i = 1, n do local name, _, _, _, _, _, minBid, inc, buyout, bid, high, owner = GetAuctionItemInfo('list', i) \
                       if name and owner and owner ~= me and not high and (buyout == 0 or buyout > (bid > 0 and bid + inc or minBid)) then \
                         local need = bid > 0 and bid + inc or minBid \
                         SetSelectedAuctionItem('list', i) PlaceAuctionBid('list', i, need) \
                         return name .. ' from ' .. owner .. ' for ' .. need end end return ''",
                );
                if picked.is_empty() {
                    warn!("PROBE_AUCTION: SKIP (10 bid) — no other seller's auction to bid on; run WOW_PROBE_AUCTION_KEEP=1 on another account first");
                    probe.phase = Phase::Cancel {
                        since: now,
                        sent: false,
                    };
                } else {
                    info!("PROBE_AUCTION: (10 bid) bidding on {picked}");
                    probe.bid_target = Some(picked);
                    probe.results_seen = auction.wire.results.len();
                    probe.phase = Phase::Bid {
                        since: now,
                        buyout: false,
                    };
                }
            } else if now - since > TIMEOUT_SECS {
                fail(&mut probe, "(10 bid) — no list result");
                probe.phase = Phase::Cancel {
                    since: now,
                    sent: false,
                };
            }
        }
        Phase::Bid { since, buyout } => {
            let step = if buyout { "11 buyout" } else { "10 bid" };
            let results = new_results(&mut probe, &auction);
            if let Some(&(id, _, error)) =
                results.iter().find(|r| r.1 == auction_action::BID_PLACED)
            {
                if error == auction_error::OK {
                    let line = format!(
                        "({step}) — BID_PLACED OK on auction {id}: {:?}",
                        probe.bid_target
                    );
                    pass(&mut probe, &line);
                } else {
                    fail(
                        &mut probe,
                        &format!("({step}) — BID_PLACED error {error} on auction {id}"),
                    );
                }
                if buyout {
                    probe.phase = Phase::BidsTab { since: now };
                    return;
                }
                // Then a buyout of another seller's auction.
                let picked: String = eval!(
                    &script,
                    "local me = UnitName('player') local n = GetNumAuctionItems('list') \
                     for i = 1, n do local name, _, _, _, _, _, _, _, buyout, _, high, owner = GetAuctionItemInfo('list', i) \
                       if name and owner and owner ~= me and not high and buyout > 0 and buyout <= GetMoney() then \
                         SetSelectedAuctionItem('list', i) PlaceAuctionBid('list', i, buyout) \
                         return name .. ' from ' .. owner .. ' for ' .. buyout end end return ''",
                );
                if picked.is_empty() {
                    warn!("PROBE_AUCTION: SKIP (11 buyout) — no other seller's auction with a buyout left");
                    probe.phase = Phase::BidsTab { since: now };
                } else {
                    info!("PROBE_AUCTION: (11 buyout) buying out {picked}");
                    probe.bid_target = Some(picked);
                    probe.phase = Phase::Bid {
                        since: now,
                        buyout: true,
                    };
                }
            } else if now - since > TIMEOUT_SECS {
                fail(
                    &mut probe,
                    &format!(
                        "({step}) — no BID_PLACED result; errors {:?}",
                        red_errors(&script)
                    ),
                );
                probe.phase = Phase::BidsTab { since: now };
            }
        }
        Phase::BidsTab { since } => {
            let (rows, status): (i64, String) = eval!(
                &script,
                "AuctionFrameTab_OnClick(AuctionFrameTab2) \
                 return GetNumAuctionItems('bidder'), BidButton1:IsShown() and (BidButton1BidStatus:GetText() or '') or ''",
            );
            let won: bool = eval!(
                &script,
                "for _, l in ipairs(ProbeAuctionChat) do if string.find(l, string.gsub(ERR_AUCTION_WON_S, '%%s.*', ''), 1, true) == 1 then return true end end return false",
            );
            if rows > 0 && !status.is_empty() {
                pass(&mut probe, &format!("(12 bids tab) — {rows} bid row(s), row 1 status {status:?}; won line in chat {won}"));
                probe.phase = Phase::Cancel {
                    since: now,
                    sent: false,
                };
            } else if now - since > TIMEOUT_SECS {
                fail(
                    &mut probe,
                    &format!(
                        "(12 bids tab) — {rows} bid row(s), status {status:?}; won line {won}"
                    ),
                );
                probe.phase = Phase::Cancel {
                    since: now,
                    sent: false,
                };
            }
        }
        Phase::Cancel { since, sent } => {
            if probe.listed.is_empty() {
                probe.phase = Phase::Done;
                return;
            }
            if !sent {
                crate::ui_script::run_or_warn(&script, "AuctionFrameTab_OnClick(AuctionFrameTab3)");
                // The Lua indexes the VM's sorted rows.
                let indices: Vec<usize> = script
                    .auction_ids(OWNER)
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| probe.listed.contains(id))
                    .map(|(i, _)| i + 1)
                    .collect();
                for i in indices.iter().rev() {
                    crate::ui_script::run_or_warn(&script, &format!("CancelAuction({i})"));
                }
                probe.results_seen = auction.wire.results.len();
                probe.phase = Phase::Cancel {
                    since: now,
                    sent: true,
                };
                return;
            }
            let left: Vec<u32> = auction.lists[OWNER]
                .entries
                .iter()
                .map(|e| e.wire.auction_id)
                .filter(|id| probe.listed.contains(id))
                .collect();
            if left.is_empty() {
                let line = format!(
                    "(13 cancel) — this run's auctions {:?} are gone from the Auctions tab",
                    probe.listed
                );
                pass(&mut probe, &line);
                probe.phase = Phase::Done;
            } else if now - since > TIMEOUT_SECS {
                let results = new_results(&mut probe, &auction);
                let line = format!("(13 cancel) — still listed {left:?}; results {results:?}");
                fail(&mut probe, &line);
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            let errors = auction_errors(&script);
            if errors.is_empty() {
                pass(
                    &mut probe,
                    "(14 lua) — Blizzard_AuctionUI raised no Lua error",
                );
            } else if only_reference_errors(&errors) {
                pass(
                    &mut probe,
                    &format!(
                        "(14 lua) — only the reference's own multi-sell `arg1` errors: {errors}"
                    ),
                );
            } else {
                fail(&mut probe, &format!("(14 lua) — {errors}"));
            }
            info!(
                "PROBE_AUCTION: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_AUCTION: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
