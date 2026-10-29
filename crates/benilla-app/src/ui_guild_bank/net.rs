//! The guild bank's packet handlers and the guild events that touch it.

use benilla_protocol::messages::{guild_event, GuildBankList, GuildBankLog, GuildEventNotice};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{GuildBankState, LogRow, TAB_SLOTS};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_guild::GuildState;

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GuildBankList, on_list)
        .net_handler(K::GuildBankLog, on_log)
        .net_handler(K::GuildBankText, on_text)
        .net_handler(K::GuildEvent, on_event)
        .net_handler(K::GuildPermissions, on_permissions)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut state: ResMut<GuildBankState>,
    mut guild: ResMut<GuildState>,
) {
    if let SessionEvent::GuildBankList(list) = ev {
        apply_list(&mut state, &mut guild, list);
    }
}

/// `SMSG_GUILD_BANK_LIST` (`0x5a7250`).
fn apply_list(state: &mut GuildBankState, guild: &mut GuildState, list: GuildBankList) {
    let tab = list.tab;
    let Some(t) = state.tabs.get_mut(usize::from(tab)) else {
        return;
    };
    // The money, clamped to the reference's 51 bits.
    guild.set_bank_money(list.money.min(0x7_ffff_ffff_ffff));
    state.withdrawals_remaining = list.withdrawals_remaining;
    if t.slots.is_empty() {
        t.slots = vec![Default::default(); usize::from(TAB_SLOTS)];
    }
    if let Some(tabs) = list.tabs {
        // The tab list arrives with tab 0, which it also empties before the slots.
        state.num_tabs = tabs.len().min(super::GUILD_BANK_TABS) as u8;
        for (row, wire) in state.tabs.iter_mut().zip(&tabs) {
            row.name = wire.name.clone();
            row.icon = wire.icon.clone();
            state.owe_tabs += 1;
        }
        for s in &mut state.tabs[usize::from(tab)].slots {
            *s = Default::default();
        }
    }
    let mut listed = Vec::with_capacity(list.slots.len());
    let slots = &mut state.tabs[usize::from(tab)].slots;
    for s in list.slots {
        if let Some(dst) = slots.get_mut(usize::from(s.slot)) {
            listed.push(s.slot);
            *dst = s;
        }
    }
    state.tabs[usize::from(tab)].stale = false;
    state.listed.push((tab, listed));
    state.dirty = true;
    match (state.banker, state.pending) {
        // The answer to a hello: the window opens on it, or closes with no banker asked.
        (None, Some(banker)) => {
            state.banker = Some(banker);
            state.pending = None;
            state.owe_opened = true;
        }
        (None, None) => state.owe_closed = true,
        (Some(_), _) => state.owe_slots = Some((tab, 0)),
    }
}

fn on_log(In(ev): In<SessionEvent>, mut state: ResMut<GuildBankState>) {
    if let SessionEvent::GuildBankLog(log) = ev {
        apply_log(&mut state, log, super::unix_now());
    }
}

/// `MSG_GUILD_BANK_LOG_QUERY`'s answer (`0x5a4800`): each row's time is arrival less its age.
fn apply_log(state: &mut GuildBankState, log: GuildBankLog, now: u32) {
    let Some(rows) = state.logs.get_mut(usize::from(log.tab)) else {
        return;
    };
    *rows = log
        .entries
        .into_iter()
        .take(25)
        .map(|entry| LogRow {
            written_at: now.wrapping_sub(entry.age),
            entry,
        })
        .collect();
    state.owe_log = Some((log.tab, 0));
    state.dirty = true;
}

/// `MSG_GUILD_PERMISSIONS` carries the bank's tab count too (`0x5cba6a`).
fn on_permissions(In(ev): In<SessionEvent>, mut state: ResMut<GuildBankState>) {
    if let SessionEvent::GuildPermissions(p) = ev {
        let tabs = p.num_tabs.min(super::GUILD_BANK_TABS as u8);
        if state.num_tabs != tabs {
            state.num_tabs = tabs;
            state.dirty = true;
        }
    }
}

fn on_text(In(ev): In<SessionEvent>, mut state: ResMut<GuildBankState>) {
    if let SessionEvent::GuildBankText { tab, text } = ev {
        // `MSG_QUERY_GUILD_BANK_TEXT`'s answer (`0x5a4ae0`), then `GUILDBANK_UPDATE_TEXT(tab)`.
        if let Some(t) = state.tabs.get_mut(usize::from(tab)) {
            t.text = text;
            state.owe_text.push(tab);
            state.dirty = true;
        }
    }
}

fn on_event(
    In(ev): In<SessionEvent>,
    mut state: ResMut<GuildBankState>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::GuildEvent(notice) = ev {
        apply_event(&mut state, &commands, &notice);
    }
}

/// The guild events' bank arms (`SMSG_GUILD_EVENT`'s handler); the money event is the guild
/// mirror's.
fn apply_event(state: &mut GuildBankState, commands: &NetCommands, notice: &GuildEventNotice) {
    match notice.event {
        guild_event::DISBANDED => {
            state.close();
            for t in &mut state.tabs {
                t.name.clear();
                t.icon.clear();
                t.text.clear();
            }
        }
        // The current tab again, and the slots repaint.
        guild_event::BANK_BAGSLOTS_CHANGED => {
            state.requery = true;
            state.owe_bare_slots = true;
        }
        guild_event::BANK_TAB_PURCHASED => {
            state.num_tabs = (state.num_tabs + 1).min(super::GUILD_BANK_TABS as u8);
            state.owe_tabs += 1;
            state.dirty = true;
        }
        // The tab info update (`0x5a4220`): tab, name, icon.
        guild_event::BANK_TAB_UPDATED => {
            let tab = notice.params.first().and_then(|p| p.parse::<usize>().ok());
            if let Some(t) = tab.and_then(|i| state.tabs.get_mut(i)) {
                t.name = notice.params.get(1).cloned().unwrap_or_default();
                t.icon = notice.params.get(2).cloned().unwrap_or_default();
                state.owe_tabs += 1;
                state.dirty = true;
            }
        }
        guild_event::BANK_TAB_AND_MONEY_UPDATED => {
            state.requery = true;
            let _ = commands
                .0
                .send(ClientCommand::GuildBankMoneyWithdrawnRequest);
        }
        guild_event::BANK_TEXT_CHANGED => {
            let tab = notice.params.first().and_then(|p| p.parse::<u8>().ok());
            if let Some(tab) = tab.filter(|&t| usize::from(t) < super::GUILD_BANK_TABS) {
                state.tabs[usize::from(tab)].text_stale = true;
                state.owe_text_changed.push(tab);
            }
        }
        _ => {}
    }
}

/// Reset at session end, as world entry resets the reference's (`0x5a56f0`).
fn on_session_end(In(_): In<SessionEvent>, mut state: ResMut<GuildBankState>) {
    *state = GuildBankState::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{GuildBankLogEntry, GuildBankSlot, GuildBankTab};

    fn list(tab: u8, tabs: Option<Vec<GuildBankTab>>, slots: Vec<GuildBankSlot>) -> GuildBankList {
        GuildBankList {
            money: 1234,
            tab,
            withdrawals_remaining: 5,
            tabs,
            slots,
        }
    }

    fn item(slot: u8, item_id: u32, count: u32) -> GuildBankSlot {
        GuildBankSlot {
            slot,
            item_id,
            count,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_list_opens_the_asked_banker() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut state = GuildBankState::default();
        let mut guild = GuildState::default();
        state.talk(0x77, &net);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::GuildBank(
                benilla_protocol::messages::GuildBankSend::Activate {
                    banker: 0x77,
                    full_update: true
                }
            ))
        ));
        assert!(!state.is_open());
        let tabs = vec![GuildBankTab {
            name: "Mats".into(),
            icon: "INV_Ore_Copper_01".into(),
        }];
        apply_list(
            &mut state,
            &mut guild,
            list(0, Some(tabs), vec![item(3, 2589, 20)]),
        );
        assert!(state.is_open());
        assert!(state.owe_opened);
        assert_eq!((state.num_tabs, state.owe_tabs), (1, 1));
        assert_eq!(state.tabs[0].slots.len(), 98);
        assert_eq!(state.slot(0, 3).map(|s| s.count), Some(20));
        assert!(
            !state.tabs[0].stale,
            "listed: the next query is not a full update"
        );
        assert_eq!(state.listed, vec![(0, vec![3])]);
        // A second talk to the same banker sends nothing.
        state.talk(0x77, &net);
        assert!(rx.try_recv().is_err());
        // The next list of an open bank owes the slot event.
        apply_list(&mut state, &mut guild, list(0, None, vec![item(3, 0, 0)]));
        assert_eq!(state.slot(0, 3), None);
        assert_eq!(state.owe_slots, Some((0, 0)));
    }

    #[test]
    fn a_list_with_no_banker_asked_closes() {
        let mut state = GuildBankState::default();
        apply_list(
            &mut state,
            &mut GuildState::default(),
            list(1, None, vec![]),
        );
        assert!(state.owe_closed);
        assert!(!state.is_open());
    }

    #[test]
    fn log_rows_keep_their_write_time() {
        let mut state = GuildBankState::default();
        apply_log(
            &mut state,
            GuildBankLog {
                tab: benilla_protocol::messages::GUILD_BANK_MONEY_LOG_TAB,
                entries: vec![GuildBankLogEntry {
                    kind: 4,
                    money: 100,
                    age: 60,
                    ..Default::default()
                }],
            },
            1_000,
        );
        assert_eq!(state.logs[6][0].written_at, 940);
        assert_eq!(state.owe_log, Some((6, 0)));
    }

    #[test]
    fn bank_events_requery_retab_and_stale_the_text() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut state = GuildBankState::default();
        let ev = |event, params: &[&str]| GuildEventNotice {
            event,
            params: params.iter().map(|s| s.to_string()).collect(),
            guid: None,
        };
        apply_event(&mut state, &net, &ev(guild_event::BANK_TAB_PURCHASED, &[]));
        assert_eq!(state.num_tabs, 1);
        apply_event(
            &mut state,
            &net,
            &ev(
                guild_event::BANK_TAB_UPDATED,
                &["0", "Ore", "INV_Ore_Iron_01"],
            ),
        );
        assert_eq!(state.tabs[0].name, "Ore");
        state.tabs[0].text_stale = false;
        apply_event(
            &mut state,
            &net,
            &ev(guild_event::BANK_TEXT_CHANGED, &["0"]),
        );
        assert!(state.tabs[0].text_stale);
        assert_eq!(state.owe_text_changed, vec![0]);
        apply_event(
            &mut state,
            &net,
            &ev(guild_event::BANK_TAB_AND_MONEY_UPDATED, &[]),
        );
        assert!(state.requery);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::GuildBankMoneyWithdrawnRequest)
        ));
    }
}
