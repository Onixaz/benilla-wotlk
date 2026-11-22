//! The verb-fired event gate: an event the reference fires on a verb's own call path
//! (`reference/3.3.5a-verb-events.tsv`) must be fired by a file that registers the verb, since
//! stock Lua calls such a verb for its event (the trainer filter's commit, fires `TRAINER_UPDATE`).
//! Otherwise the pair is declared in [`ELSEWHERE`] or [`GAP`], and the verb's own file must not
//! fire it. The check is textual, so it cannot see a fire off the verb's path.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// One row of `reference/3.3.5a-verb-events.tsv`.
struct Pair {
    verb: String,
    event: String,
    shape: String,
    via: String,
}

/// Pairs fired off the verb's module on the verb's own change, as `(verb, event, file)` with the
/// file matched as a path suffix: a feed reading the state the verb changed, or a helper on the
/// verb's own path. Each row's comment names the trigger.
const ELSEWHERE: &[(&str, &str, &str)] = &[
    // The sell slot's diff (`auction_sell_item` moved), from the click, the right-click arm or
    // the cancelled multi-sell's slot clear.
    (
        "ClickAuctionSellItemButton",
        "NEW_AUCTION_UPDATE",
        "benilla-app/src/ui_auction/mod.rs",
    ),
    (
        "UseContainerItem",
        "NEW_AUCTION_UPDATE",
        "benilla-app/src/ui_auction/mod.rs",
    ),
    (
        "CancelSell",
        "NEW_AUCTION_UPDATE",
        "benilla-app/src/ui_auction/mod.rs",
    ),
    // The multi-sell runs in the app: its drain starts it and its feed fires the events.
    (
        "CancelSell",
        "AUCTION_MULTISELL_FAILURE",
        "benilla-app/src/ui_auction/mod.rs",
    ),
    (
        "StartAuction",
        "AUCTION_MULTISELL_START",
        "benilla-app/src/ui_auction/mod.rs",
    ),
    // `keybinds.generation` moves; `sync_dispatch` re-derives the table and fires.
    (
        "SetBinding",
        "UPDATE_BINDINGS",
        "benilla-app/src/bindings.rs",
    ),
    (
        "SetBindingClick",
        "UPDATE_BINDINGS",
        "benilla-app/src/bindings.rs",
    ),
    (
        "SetBindingItem",
        "UPDATE_BINDINGS",
        "benilla-app/src/bindings.rs",
    ),
    (
        "SetBindingMacro",
        "UPDATE_BINDINGS",
        "benilla-app/src/bindings.rs",
    ),
    (
        "SetBindingSpell",
        "UPDATE_BINDINGS",
        "benilla-app/src/bindings.rs",
    ),
    // The trade drain applies the accept, the unaccept or the close to the session at once, and
    // the trade feed fires on the change it reads.
    (
        "AcceptTrade",
        "TRADE_ACCEPT_UPDATE",
        "benilla-app/src/ui_trade/mod.rs",
    ),
    (
        "CancelTradeAccept",
        "TRADE_ACCEPT_UPDATE",
        "benilla-app/src/ui_trade/mod.rs",
    ),
    (
        "CloseTrade",
        "TRADE_CLOSED",
        "benilla-app/src/ui_trade/mod.rs",
    ),
    // The direct RSVP (`direct_rsvp`) marks the action pending and signals it.
    (
        "CalendarContextInviteAvailable",
        "CALENDAR_ACTION_PENDING",
        "benilla-ui/src/script/calendar/engine.rs",
    ),
    (
        "CalendarContextInviteDecline",
        "CALENDAR_ACTION_PENDING",
        "benilla-ui/src/script/calendar/engine.rs",
    ),
    // `pickup_item` puts the item on the cursor and queues the cursor's transition event.
    (
        "PickupItem",
        "CURSOR_UPDATE",
        "benilla-ui/src/script/cursor.rs",
    ),
    // The reset restores the colour table, then `chat_types::fire_all` signals every entry.
    (
        "ResetChatWindows",
        "UPDATE_CHAT_COLOR",
        "benilla-ui/src/script/chat_types.rs",
    ),
    (
        "ResetChatWindows",
        "UPDATE_CHAT_COLOR_NAME_BY_CLASS",
        "benilla-ui/src/script/chat_types.rs",
    ),
    // Fired when the server's raid-target update lands, a round trip after the reference, which
    // fires locally from the verb.
    (
        "SetRaidTarget",
        "RAID_TARGET_UPDATE",
        "benilla-app/src/ui_party/feed.rs",
    ),
    // The item drain runs the use: a glyph fires the event ahead of it (`0x5d8ad3`).
    (
        "UseContainerItem",
        "USE_GLYPH",
        "benilla-app/src/ui_items/drain.rs",
    ),
];

/// Pairs nothing fires on the verb's path, each with its reason.
const GAP: &[(&str, &str, &str)] = &[
    (
        "CloseTrade",
        "PLAYER_TRADE_MONEY",
        "fired only when the trade did not complete, \
         zeroing the cancelled offer under a frame TRADE_CLOSED \
         hides; here `TradeSession::begin` resets the whole session at the next trade, so the \
         next window opens at zero either way",
    ),
    (
        "PetRename",
        "PET_FORCE_NAME_DECLENSION",
        "fired from the verb (`0x5d5670`) only when the client's locale declines names and the \
         call carries fewer than five forms; benilla's name check declines no name, and the \
         server's refusal with its forms fires it from the app's pet feed",
    ),
];

fn table() -> Vec<Pair> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-verb-events.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/3.3.5a-verb-events.tsv");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("verb\t"))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 4).then(|| Pair {
                verb: f[0].to_string(),
                event: f[1].to_string(),
                shape: f[2].to_string(),
                via: f[3].to_string(),
            })
        })
        .collect()
}

/// Every non-test `.rs` under `crates/`: comment lines dropped, the trailing test module cut.
fn sources() -> Vec<(PathBuf, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("crates");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            // Skip tests and `*_gate.rs`, test-only modules whose names do not say so.
            let name = p.to_string_lossy();
            if p.extension().is_none_or(|x| x != "rs")
                || name.contains("test")
                || name.ends_with("_gate.rs")
            {
                continue;
            }
            // A CRLF checkout (`core.autocrlf` on Windows) would hide the cut below.
            let whole = std::fs::read_to_string(&p)
                .unwrap_or_default()
                .replace("\r\n", "\n");
            let head = whole
                .rfind("#[cfg(test)]\nmod ")
                .map_or(whole.as_str(), |i| &whole[..i]);
            let text: String = head
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            out.push((p, text));
        }
    }
    out.sort();
    out
}

fn quoted(text: &str, name: &str) -> bool {
    text.contains(&format!("\"{name}\""))
}

fn short(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.split_once("/crates/")
        .map_or(s.to_string(), |(_, t)| t.to_string())
}

/// The files that register `verb`, and whether any of them fires `event`.
fn verb_module_fires(src: &[(PathBuf, String)], verb: &str, event: &str) -> (Vec<String>, bool) {
    let registrars: Vec<&(PathBuf, String)> = src.iter().filter(|(_, t)| quoted(t, verb)).collect();
    let fires = registrars.iter().any(|(_, t)| quoted(t, event));
    (registrars.iter().map(|(p, _)| short(p)).collect(), fires)
}

#[test]
fn every_event_the_reference_fires_from_a_verb_is_fired_by_the_verb_s_module() {
    let src = sources();
    assert!(src.len() > 100, "source walk found {} files", src.len());
    let declared: BTreeSet<(&str, &str)> = ELSEWHERE
        .iter()
        .chain(GAP.iter())
        .map(|(v, e, _)| (*v, *e))
        .collect();

    let mut surprises = Vec::new();
    let mut seen = 0;
    for pair in table() {
        let (registrars, fires) = verb_module_fires(&src, &pair.verb, &pair.event);
        if registrars.is_empty() {
            continue; // an unbuilt verb: `scripts/api-coverage.sh`'s queue
        }
        seen += 1;
        if fires || declared.contains(&(pair.verb.as_str(), pair.event.as_str())) {
            continue;
        }
        let fired_by: Vec<String> = src
            .iter()
            .filter(|(_, t)| quoted(t, &pair.event))
            .map(|(p, _)| short(p))
            .collect();
        surprises.push(format!(
            "{} -> {}  [{}{}]  registered in {}; {}",
            pair.verb,
            pair.event,
            pair.shape,
            if pair.via == "-" {
                String::new()
            } else {
                format!(" via {}", pair.via)
            },
            registrars.join(", "),
            if fired_by.is_empty() {
                "fired NOWHERE in benilla".to_string()
            } else {
                format!("fired only by {}", fired_by.join(", "))
            },
        ));
    }
    assert!(
        seen > 20,
        "only {seen} pairs had a registered verb — the walk or the table is off"
    );
    assert!(
        surprises.is_empty(),
        "the reference fires these events from inside the verb, and the module that registers \
         the verb here fires nothing — the stock Lua that calls the verb for its side effect \
         repaints nothing. Fire it from the verb, or declare the pair: ELSEWHERE \
         when a feed fires it on THIS verb's change and you read that at the feed, GAP with the \
         reason otherwise:\n  {}",
        surprises.join("\n  ")
    );
}

#[test]
fn the_declared_rows_are_still_true() {
    let src = sources();
    let table = table();
    let in_table =
        |verb: &str, event: &str| table.iter().any(|p| p.verb == verb && p.event == event);
    let mut wrong = Vec::new();
    for (verb, event, file) in ELSEWHERE {
        if !in_table(verb, event) {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: not a pair in the table any more"
            ));
        }
        let named: Vec<&(PathBuf, String)> = src
            .iter()
            .filter(|(p, _)| short(p).ends_with(file))
            .collect();
        if named.is_empty() {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: no source file ends with {file}"
            ));
        } else if !named.iter().any(|(_, t)| quoted(t, event)) {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: {file} no longer fires it"
            ));
        }
        if verb_module_fires(&src, verb, event).1 {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: the verb's own module fires it now — take the row out"
            ));
        }
    }
    for (verb, event, _) in GAP {
        if !in_table(verb, event) {
            wrong.push(format!(
                "GAP {verb} -> {event}: not a pair in the table any more"
            ));
        }
        if verb_module_fires(&src, verb, event).1 {
            wrong.push(format!(
                "GAP {verb} -> {event}: the verb's own module fires it now — take the row out"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "stale declarations:\n  {}",
        wrong.join("\n  ")
    );
}
