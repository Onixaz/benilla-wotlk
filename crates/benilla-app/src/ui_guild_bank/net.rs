//! The guild bank's packet handlers and the guild events that touch it: each applies its change and fires its events in turn, as 12340's handlers do (`0x5a7250`, `0x5a4800`, `0x5a4ab0`, `0x5a4ae0`).

use benilla_protocol::messages::{
    guild_command, guild_command_error, guild_event, GuildBankList, GuildBankLog, GuildBankSlot,
    GuildCommandResult, GuildEventNotice, GUILD_BANK_TAB_SLOTS,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{set_guild_banker_none, BankSlot, Callback, GuildBankState, LogRow};
use crate::items::Items;
use crate::items::{RecordWait, RecordWaits};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfPlayer};
use crate::ui_guild::GuildState;
use crate::ui_script::live::UiEvents;
use benilla_ui::script::GUILD_BANK_TABS;

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GuildBankList, on_list)
        .net_handler(K::GuildBankLog, on_log)
        .net_handler(K::GuildBankText, on_text)
        .net_handler(K::GuildBankWithdrawMoney, on_withdraw_money)
        .net_handler(K::GuildEvent, on_event)
        .net_handler(K::GuildCommandResult, on_command_result)
        .net_handler(K::GuildPermissions, on_permissions)
        .net_handler(K::Disconnected, on_session_end);
}

fn bank_mut(world: &mut World) -> Option<Mut<'_, GuildBankState>> {
    world.get_non_send_resource_mut::<GuildBankState>()
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut state: NonSendMut<GuildBankState>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::GuildBankList(list) = ev {
        apply_list(&mut state, &mut ui, list);
    }
}

/// `SMSG_GUILD_BANK_LIST` (`0x5a7250`): the header, each tab row then its `GUILDBANK_UPDATE_TABS`, the slots, then the open, the close or the slots' event.
fn apply_list(state: &mut GuildBankState, ui: &mut UiEvents, list: GuildBankList) {
    let tab = list.tab;
    if usize::from(tab) >= GUILD_BANK_TABS {
        return;
    }
    // A count still waiting is zeroed; its callbacks stay registered, the cancel asking the name cache (`0x5a72a5`, `0x5a72c8`).
    state.slot_waits = 0;
    // The money, clamped to the reference's 51 bits (`0x5a72e5`).
    state.money = list.money.min(0x7_ffff_ffff_ffff);
    state.withdrawals_remaining = list.withdrawals_remaining;
    let t = &mut state.tabs[usize::from(tab)];
    if t.slots.is_empty() {
        t.slots = vec![BankSlot::default(); usize::from(GUILD_BANK_TAB_SLOTS)];
    }
    if let Some(rows) = list.tabs.filter(|_| tab == 0) {
        state.num_tabs = rows.len().min(GUILD_BANK_TABS) as u8;
        for (i, row) in rows.into_iter().take(GUILD_BANK_TABS).enumerate() {
            ui.world(move |world| {
                if let Some(mut bank) = bank_mut(world) {
                    bank.tabs[i].name = row.name;
                    bank.tabs[i].icon = row.icon;
                }
            });
            // Each row's name, then its event (`0x5a73f6`).
            ui.fire("GUILDBANK_UPDATE_TABS", Vec::new());
        }
        // The rows read, the listed tab empties (`0x5a7411`).
        ui.world(move |world| {
            if let Some(mut bank) = bank_mut(world) {
                for s in &mut bank.tabs[usize::from(tab)].slots {
                    *s = BankSlot::default();
                }
            }
        });
    }
    let slots = list.slots;
    ui.world(move |world| list_slots(world, tab, slots));
}

/// The list's slots: the cursor's bank item read once (`0x5a743d`), no active player ends the handler (`0x5a7462`).
fn list_slots(world: &mut World, tab: u8, slots: Vec<GuildBankSlot>) {
    let held = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .and_then(|s| s.guild_bank_cursor_item());
    let mut player = world.query_filtered::<(), With<SelfPlayer>>();
    if player.iter(world).next().is_none() {
        return;
    }
    list_slots_from(world, tab, slots, 0, held);
}

/// The slots from `from`, each written unlocked with its record asked (`0x5a7648`, `0x5a7673`); the held item's slot drops the cursor, then unlocks with its event (`0x5a74be`, `0x5a74cc`).
fn list_slots_from(
    world: &mut World,
    tab: u8,
    slots: Vec<GuildBankSlot>,
    from: usize,
    held: Option<(u8, u8, u32)>,
) {
    for i in from..slots.len() {
        let s = &slots[i];
        if let Some((_, _, item)) = held.filter(|&(t, slot, _)| t == tab && slot == s.slot) {
            let slot = s.slot;
            let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
                return;
            };
            ui.clear_cursor();
            ui.world(move |world| {
                if let Some(mut bank) = bank_mut(world) {
                    bank.set_lock(tab, slot, item, false);
                }
            });
            ui.fire("GUILDBANK_ITEM_LOCK_CHANGED", Vec::new());
            ui.world(move |world| list_slots_from(world, tab, slots, i, None));
            return;
        }
        let entry = s.item_id;
        let index = usize::from(s.slot);
        let asked = entry != 0 && {
            let items = world.get_resource::<Items>();
            let net = world.get_resource::<NetCommands>();
            match (items, net) {
                (Some(items), Some(net)) => items.ask_template(entry, 0, net).is_none(),
                _ => false,
            }
        };
        let Some(mut bank) = bank_mut(world) else {
            return;
        };
        let bank = &mut *bank;
        if let Some(dst) = bank.tabs[usize::from(tab)].slots.get_mut(index) {
            *dst = BankSlot {
                wire: s.clone(),
                locked: false,
            };
        }
        if asked {
            bank.slot_waits += 1;
            if let Some(mut waits) = world.get_non_send_resource_mut::<RecordWaits>() {
                waits.wait(entry, RecordWait::GuildBank(Callback::Slots), false);
            }
        }
    }
    list_tail(world, tab);
}

/// The list's end (`0x5a7698`): the tab is fresh; no banker and none asked closes (`0x5a76ef`); one asked takes the interact target and opens (`0x5a772b`, `0x5a774c`); an open bank fires the slots' event once nothing waits (`0x5a76c7`).
fn list_tail(world: &mut World, tab: u8) {
    let Some(mut bank) = bank_mut(world) else {
        return;
    };
    bank.tabs[usize::from(tab)].stale = false;
    let (banker, pending, waits) = (bank.banker, bank.pending, bank.slot_waits);
    match (banker, pending) {
        (None, None) => {
            if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
                set_guild_banker_none(&mut ui);
            }
        }
        (None, Some(asked)) => {
            crate::ui_bank::close_by_interaction(world);
            let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
                return;
            };
            ui.world(move |world| {
                if let Some(mut bank) = bank_mut(world) {
                    bank.banker = Some(asked);
                }
            });
            ui.fire("GUILDBANKFRAME_OPENED", Vec::new());
        }
        (Some(_), _) => {
            if waits == 0 {
                if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
                    ui.fire("GUILDBANKBAGSLOTS_CHANGED", Vec::new());
                }
            }
        }
    }
}

/// A name answered: the log's callbacks on it run in ask order (`0x5a47d0`).
pub(crate) fn name_landed(world: &mut World, guid: u64) {
    let Some(mut bank) = bank_mut(world) else {
        return;
    };
    if !bank.name_callbacks.iter().any(|&(g, _)| g == guid) {
        return;
    }
    let mut taken = Vec::new();
    bank.name_callbacks.retain(|&(g, callback)| {
        if g == guid {
            taken.push(callback);
        }
        g != guid
    });
    for callback in taken {
        record_callback(world, callback);
    }
}

/// One callback on a landed record: its count at zero, or counted down to it, fires its event (`0x5a47a0`, `0x5a47d0`).
pub(crate) fn record_callback(world: &mut World, callback: Callback) {
    let Some(mut bank) = bank_mut(world) else {
        return;
    };
    let waits = match callback {
        Callback::Slots => &mut bank.slot_waits,
        Callback::Log => &mut bank.log_waits,
    };
    let fire = *waits == 0 || {
        *waits -= 1;
        *waits == 0
    };
    if !fire {
        return;
    }
    let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    match callback {
        Callback::Slots => ui.fire("GUILDBANKBAGSLOTS_CHANGED", Vec::new()),
        Callback::Log => ui.fire("GUILDBANKLOG_UPDATE", Vec::new()),
    }
}

fn on_log(
    In(ev): In<SessionEvent>,
    mut state: NonSendMut<GuildBankState>,
    items: Res<Items>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut ui: ResMut<UiEvents>,
    mut waits: NonSendMut<RecordWaits>,
) {
    if let SessionEvent::GuildBankLog(log) = ev {
        let mut asks = |row: &benilla_protocol::messages::GuildBankLogEntry| {
            let item = row.item_id != 0 && items.ask_template(row.item_id, 0, &commands).is_none();
            let name = names.resolve(row.player, &commands).is_none();
            (item, name)
        };
        apply_log(&mut state, &mut waits, log, super::unix_now(), &mut asks);
        if state.banker.is_some() && state.log_waits == 0 {
            ui.fire("GUILDBANKLOG_UPDATE", Vec::new());
        }
    }
}

/// `MSG_GUILD_BANK_LOG_QUERY`'s answer (`0x5a4800`): a count still waiting is zeroed and its callbacks cancelled (`0x5a4840`); each row's time is arrival less its age, an item row's record and every row's name asked (`0x5a49d9`, `0x5a4a2b`).
fn apply_log(
    state: &mut GuildBankState,
    waits: &mut RecordWaits,
    log: GuildBankLog,
    now: u32,
    asks: &mut dyn FnMut(&benilla_protocol::messages::GuildBankLogEntry) -> (bool, bool),
) {
    if usize::from(log.tab) > GUILD_BANK_TABS {
        return;
    }
    if state.log_waits != 0 && state.log_tab.is_some() {
        state.log_waits = 0;
        state.name_callbacks.retain(|&(_, c)| c != Callback::Log);
        waits.cancel(|w| w == RecordWait::GuildBank(Callback::Log));
    }
    state.log_tab = Some(log.tab);
    let mut rows = Vec::with_capacity(log.entries.len().min(25));
    for entry in log.entries.into_iter().take(25) {
        let money_row = matches!(entry.kind, 4 | 5 | 6 | 8 | 9);
        let (item, name) = asks(&entry);
        if !money_row && item {
            state.log_waits += 1;
            waits.wait(entry.item_id, RecordWait::GuildBank(Callback::Log), false);
        }
        if name {
            state.log_waits += 1;
            state.name_callbacks.push((entry.player, Callback::Log));
        }
        rows.push(LogRow {
            written_at: now.wrapping_sub(entry.age),
            entry,
        });
    }
    state.logs[usize::from(log.tab)] = rows;
}

/// `MSG_GUILD_PERMISSIONS` carries the bank's tab count too (`0x5cba6a`).
fn on_permissions(In(ev): In<SessionEvent>, mut state: NonSendMut<GuildBankState>) {
    if let SessionEvent::GuildPermissions(p) = ev {
        state.num_tabs = p.num_tabs.min(GUILD_BANK_TABS as u8);
    }
}

/// `MSG_QUERY_GUILD_BANK_TEXT`'s answer (`0x5a4ae0`): the text, then `GUILDBANK_UPDATE_TEXT(tab)`.
fn on_text(
    In(ev): In<SessionEvent>,
    mut state: NonSendMut<GuildBankState>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::GuildBankText { tab, text } = ev {
        if let Some(t) = state.tabs.get_mut(usize::from(tab)) {
            t.text = text;
            ui.fire(
                "GUILDBANK_UPDATE_TEXT",
                vec![benilla_ui::script::ScriptValue::Int(i64::from(tab) + 1)],
            );
        }
    }
}

/// `MSG_GUILD_BANK_MONEY_WITHDRAWN`'s answer (`0x5a4ab0`): the value, then `GUILDBANK_UPDATE_WITHDRAWMONEY`.
fn on_withdraw_money(
    In(ev): In<SessionEvent>,
    mut state: NonSendMut<GuildBankState>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::GuildBankWithdrawMoney { remaining } = ev {
        state.withdraw_money = remaining;
        ui.fire("GUILDBANK_UPDATE_WITHDRAWMONEY", Vec::new());
    }
}

fn on_event(
    In(ev): In<SessionEvent>,
    mut state: NonSendMut<GuildBankState>,
    commands: Res<NetCommands>,
    guild: Res<GuildState>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::GuildEvent(notice) = ev {
        apply_event(&mut state, &commands, &notice, guild.rank_index(), &mut ui);
    }
}

fn on_command_result(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::GuildCommandResult(result) = ev {
        apply_command_result(&result, &mut ui);
    }
}

/// Leaving the guild shuts the banker: the quit command's success arm sets it to 0 (`0x6cc630`).
fn apply_command_result(result: &GuildCommandResult, ui: &mut UiEvents) {
    if result.command == guild_command::QUIT
        && result.result == guild_command_error::PLAYER_NO_MORE_IN_GUILD
    {
        set_guild_banker_none(ui);
    }
}

/// `MSG_GUILD_BANK_MONEY_WITHDRAWN`'s ask.
fn ask_withdraw_money(commands: &NetCommands) {
    let _ = commands
        .0
        .send(ClientCommand::GuildBankMoneyWithdrawnRequest);
}

/// The guild events' bank arms (`SMSG_GUILD_EVENT`'s handler), each firing in the handler.
fn apply_event(
    state: &mut GuildBankState,
    commands: &NetCommands,
    notice: &GuildEventNotice,
    own_rank: u32,
    ui: &mut UiEvents,
) {
    let param = |i: usize| notice.params.get(i).map(String::as_str).unwrap_or("");
    match notice.event {
        // Our rank renamed: the open banker's current tab again (`0x6d9543`).
        guild_event::UPDATE_RANK_NAME => {
            if param(0).parse::<u32>().ok() == Some(own_rank) {
                state.query_tab(state.current_tab, commands);
            }
        }
        // The banker setter with no banker, then every tab record zeroed (`0x6d9416`, `0x6d9426`).
        guild_event::DISBANDED => {
            set_guild_banker_none(ui);
            ui.world(|world| {
                if let Some(mut bank) = bank_mut(world) {
                    for t in &mut bank.tabs {
                        t.name.clear();
                        t.icon.clear();
                        t.text.clear();
                        t.stale = false;
                    }
                }
            });
        }
        // The current tab again, then the slots' event (`0x6d96d7`, `0x6d96e3`).
        guild_event::BANK_BAGSLOTS_CHANGED => {
            state.query_tab(state.current_tab, commands);
            ui.fire("GUILDBANKBAGSLOTS_CHANGED", Vec::new());
        }
        // One more tab, then `GUILDBANK_UPDATE_TABS` (`0x6d96f7`, `0x6d9705`).
        guild_event::BANK_TAB_PURCHASED => {
            state.num_tabs = state.num_tabs.wrapping_add(1);
            ui.fire("GUILDBANK_UPDATE_TABS", Vec::new());
        }
        // The tab info update (`0x5a4220`): tab, name, icon, then `GUILDBANK_UPDATE_TABS`.
        guild_event::BANK_TAB_UPDATED => {
            let tab = param(0).parse::<usize>().ok();
            if let Some(t) = tab.and_then(|i| state.tabs.get_mut(i)) {
                t.name = param(1).to_string();
                t.icon = param(2).to_string();
                ui.fire("GUILDBANK_UPDATE_TABS", Vec::new());
            }
        }
        // The money stored, the withdrawal asked, then `GUILDBANK_UPDATE_MONEY` (`0x6d9768`, `0x6d97a2`, `0x6d97ad`).
        guild_event::BANK_MONEY_SET => {
            if let Some(copper) = notice
                .params
                .first()
                .and_then(|p| benilla_protocol::messages::guild_bank_money_param(p))
            {
                state.money = copper;
            }
            ask_withdraw_money(commands);
            ui.fire("GUILDBANK_UPDATE_MONEY", Vec::new());
        }
        // The current tab and the withdrawal asked, with no event (`0x6d97e8`, `0x6d9820`).
        guild_event::BANK_TAB_AND_MONEY_UPDATED => {
            state.query_tab(state.current_tab, commands);
            ask_withdraw_money(commands);
        }
        // The text stale, then `GUILDBANK_TEXT_CHANGED(tab)` (`0x6d9864`, `0x6d9877`).
        guild_event::BANK_TEXT_CHANGED => {
            let tab = param(0).parse::<u8>().ok();
            if let Some(tab) = tab.filter(|&t| usize::from(t) < GUILD_BANK_TABS) {
                state.tabs[usize::from(tab)].text_stale = true;
                ui.fire(
                    "GUILDBANK_TEXT_CHANGED",
                    vec![benilla_ui::script::ScriptValue::Int(i64::from(tab) + 1)],
                );
            }
        }
        _ => {}
    }
}

/// Our `PLAYER_GUILDID` changed: the all-tabs clear (`0x6e1c11` -> `0x5a7760`).
pub(crate) fn guild_id_changed(ui: &mut UiEvents) {
    ui.world(|world| {
        if let Some(mut bank) = bank_mut(world) {
            bank.clear_all_tabs();
        }
    });
}

/// Reset at session end, as world entry resets the reference's (`0x5a56f0`).
fn on_session_end(In(_): In<SessionEvent>, mut state: NonSendMut<GuildBankState>) {
    *state = GuildBankState::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::GuildBankLogEntry;

    #[test]
    fn log_rows_keep_their_write_time_and_count_their_asks() {
        let mut state = GuildBankState::default();
        let mut waits = RecordWaits::default();
        let entry = |kind, item_id, player| GuildBankLogEntry {
            kind,
            item_id,
            player,
            money: 100,
            age: 60,
            ..Default::default()
        };
        let log = GuildBankLog {
            tab: 0,
            entries: vec![entry(4, 0, 7), entry(1, 2589, 8)],
        };
        // Every record a miss: the money row asks its name, the item row its item and name.
        apply_log(&mut state, &mut waits, log, 1_000, &mut |_| (true, true));
        assert_eq!(state.logs[0][0].written_at, 940);
        assert_eq!(state.log_waits, 3);
        assert_eq!(
            state.name_callbacks,
            [(7, Callback::Log), (8, Callback::Log)]
        );
        assert_eq!(
            waits.waiting(),
            [(2589, RecordWait::GuildBank(Callback::Log))]
        );
        // A new log while they wait cancels them (`0x5a4840`).
        apply_log(
            &mut state,
            &mut waits,
            GuildBankLog {
                tab: 6,
                entries: Vec::new(),
            },
            1_000,
            &mut |_| (false, false),
        );
        assert_eq!((state.log_waits, state.name_callbacks.len()), (0, 0));
        assert!(waits.waiting().is_empty());
    }

    #[test]
    fn a_lock_holds_only_while_the_slot_holds_its_item() {
        let mut state = GuildBankState::default();
        state.tabs[0].slots = vec![BankSlot::default(); 98];
        state.tabs[0].slots[3].wire.item_id = 2589;
        state.set_lock(0, 3, 4306, true);
        assert!(!state.is_locked(0, 3, 4306), "another item locks nothing");
        state.set_lock(0, 3, 2589, true);
        assert!(state.is_locked(0, 3, 2589));
        state.tabs[0].slots[3].wire.item_id = 4306;
        assert!(!state.is_locked(0, 3, 2589), "the lock tests the item too");
    }
}
