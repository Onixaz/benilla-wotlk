//! The mailbox's packet handlers: each moves [`MailOpen`] and [`MailPending`] and raises the
//! window's events at its step, as 12340's do.

use benilla_protocol::messages::{
    mail_action, mail_delete_reason, mail_error, mail_flags, mail_message_type, MailList,
    MailResult,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use super::{
    mail_refusal, MailOpen, MailPending, MailRow, NameCallback, NameRecord, MAIL_SENT_KEY,
};
use crate::items::{Items, RecordWait, RecordWaits};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_action::signals::SignalSink;
use crate::ui_action::UiError;
use crate::ui_script::live::{UiEvents, UniqueSignals};
use crate::ui_social::SocialState;

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::MailList, on_mail_list)
        .net_handler(K::SendMailResult, on_send_mail_result)
        .net_handler(K::ReceivedMail, on_received_mail)
        .net_handler(K::NextMailTime, on_next_mail_time)
        .net_handler(K::ShowMailbox, on_show_mailbox)
        .net_handler(K::PlayerName, on_name_record)
        .net_handler(K::PlayerNameMiss, on_name_record)
        .net_handler(K::CreatureName, on_name_record)
        .net_handler(K::GameObjectInfo, on_name_record)
        .net_handler(K::Disconnected, on_session_end);
}

/// The item records a list's auction notices wait on, each with the subject's callback.
#[derive(bevy::ecs::system::SystemParam)]
struct ListReads<'w> {
    items: Res<'w, Items>,
    waits: Option<NonSendMut<'w, RecordWaits>>,
}

fn on_mail_list(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
    social: Res<SocialState>,
    commands: Res<NetCommands>,
    mut reads: ListReads,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::MailList(list) = ev {
        let items = &reads.items;
        let mut waits = reads.waits.as_deref_mut();
        let mut ask = |entry: u32| {
            if items.template(entry, 0, &commands).is_none() {
                if let Some(waits) = waits.as_deref_mut() {
                    waits.wait(entry, RecordWait::MailInbox, false);
                }
            }
        };
        mail_list(
            list,
            &mut mail,
            &mut pending,
            &social,
            &commands,
            &mut ask,
            &mut ui,
        );
    }
}

fn on_send_mail_result(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    commands: Res<NetCommands>,
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::SendMailResult(result) = ev {
        let purse = self_q
            .iter()
            .next()
            .and_then(|s| s.0.player_money())
            .map_or(0, u64::from);
        send_mail_result(result, &mut mail, &commands, purse, &mut ui);
    }
}

/// `SMSG_RECEIVED_MAIL` (`0x571a10`): `ReceivedMail`, which signals when it lights the icon.
fn on_received_mail(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<MailPending>,
    mail: Res<MailOpen>,
    commands: Res<NetCommands>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::ReceivedMail { seconds } = ev {
        if super::received_mail(
            seconds,
            "SMSG_RECEIVED_MAIL",
            &mut pending,
            &mail,
            &commands,
        ) {
            ui.fire("UPDATE_PENDING_MAIL", Vec::new());
        }
    }
}

/// `MSG_QUERY_NEXT_MAIL_TIME`'s reply (`0x56daf0`): the time and the senders, then
/// `UPDATE_PENDING_MAIL` whatever changed (`0x56dbac`).
fn on_next_mail_time(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<MailPending>,
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::NextMailTime { seconds, senders } = ev {
        debug!(
            "ui_mail: next mail time {seconds}, {} sender(s) {:?}",
            senders.len(),
            senders
                .iter()
                .map(|s| (s.guid, s.entry, s.message_type, s.delay))
                .collect::<Vec<_>>()
        );
        pending.apply_query_reply(seconds, &senders);
        ui.fire("UPDATE_PENDING_MAIL", Vec::new());
    }
}

/// `SMSG_SHOW_MAILBOX` (`0x56dbc0`): the server opens the window, as the mailbox open does.
fn on_show_mailbox(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::ShowMailbox { mailbox } = ev {
        ui.world(move |w| super::talk_to_mailbox(w, mailbox));
    }
}

/// A name record answered: each callback a mail read left on it runs. Found, the sender name's
/// queues `MAIL_INBOX_UPDATE` with the frame's unique signals and signals `UPDATE_PENDING_MAIL`
/// (`0x56d46e`, `0x56d47a`); the inbox's queues the unique signal alone (`0x56d4be`). A
/// placeholder answer counts as found (`0x635894`).
fn on_name_record(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut ui: ResMut<UiEvents>,
    mut signals: Option<NonSendMut<UniqueSignals>>,
) {
    use benilla_protocol::messages::NameQueryMiss;
    let (record, found) = match ev {
        SessionEvent::PlayerName { guid, .. } => (NameRecord::Player(guid), true),
        SessionEvent::PlayerNameMiss { guid, miss } => {
            (NameRecord::Player(guid), miss == NameQueryMiss::Placeholder)
        }
        SessionEvent::CreatureName { entry, name, .. } => {
            (NameRecord::Creature(entry), name.is_some())
        }
        SessionEvent::GameObjectInfo { entry, .. } => (NameRecord::GameObject(entry), true),
        _ => return,
    };
    for callback in mail.take_name_waits(record) {
        if !found {
            continue;
        }
        if let Some(signals) = signals.as_deref_mut() {
            signals.unique_signal("MAIL_INBOX_UPDATE");
        }
        if callback == NameCallback::UpdateAndPending {
            ui.fire("UPDATE_PENDING_MAIL", Vec::new());
        }
    }
}

/// The window and the arrival state end with the session; the next world enter re-queries.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
) {
    mail.clear_session();
    *pending = MailPending::default();
}

/// `SMSG_MAIL_LIST_RESULT` (`0x571c50`): the rows replace the old. A player mail from an ignored
/// sender that is not returnable-exempt goes back unread, returned if it holds anything, else
/// deleted, and leaves the counts (`0x571f4c`-`0x5720a3`). An auction notice's subject reads its
/// item's record, `ask` leaving the subject's callback on a miss (`0x56f672`). Then the command in
/// flight ends, an empty inbox while the icon is lit re-queries the next-mail time, and
/// `MAIL_INBOX_UPDATE` fires (`0x5721f3`).
fn mail_list(
    list: MailList,
    mail: &mut MailOpen,
    pending: &mut MailPending,
    social: &SocialState,
    commands: &NetCommands,
    ask: &mut dyn FnMut(u32),
    ui: &mut UiEvents,
) {
    let mailbox = mail.mailbox.unwrap_or(0);
    let default_stationery = mail.default_stationery();
    let mut total = list.total;
    let mut rows = Vec::with_capacity(list.mails.len());
    for entry in list.mails {
        let mut row = MailRow::new(entry);
        let e = &row.entry;
        if e.message_type == mail_message_type::NORMAL
            && social.is_ignored(e.sender_guid)
            && e.flags & 0x20A == 0
        {
            let cmd = if row.num_attachments() != 0 || e.money != 0 {
                ClientCommand::MailReturnToSender {
                    mailbox,
                    mail_id: e.message_id,
                    sender: e.sender_guid,
                }
            } else {
                ClientCommand::MailDelete {
                    mailbox,
                    mail_id: e.message_id,
                    reason: mail_delete_reason::IGNORED,
                }
            };
            let _ = commands.0.send(cmd);
            total = total.saturating_sub(1);
            continue;
        }
        if row.entry.stationery == 0 {
            row.entry.stationery = default_stationery;
        }
        if row.entry.message_type == mail_message_type::AUCTION {
            ask(super::invoice::parse_subject(&row.entry.subject).entry);
        }
        rows.push(row);
    }
    mail.rows = rows;
    mail.total = total;
    ui.write(|s| s.mail_command_done(false));
    if mail.rows.is_empty() && pending.has_new_mail() {
        super::query_next_mail_time(pending, commands);
    }
    ui.fire("MAIL_INBOX_UPDATE", Vec::new());
}

/// `SMSG_SEND_MAIL_RESULT` (`0x5717b0`), each step raised in turn: the message, then the action's
/// change to the rows with its events, the pending flags, an auto-loot's next take, then
/// `MAIL_SUCCESS` or `MAIL_FAILED` (`0x571997`). An expired attachment's removal runs after its
/// line (`0x57191f`, then `0x571930`), so the line's listeners read the row still held.
fn send_mail_result(
    result: MailResult,
    mail: &mut MailOpen,
    commands: &NetCommands,
    purse: u64,
    ui: &mut UiEvents,
) {
    let MailResult {
        mail_id,
        action,
        error,
        equip_error,
        item,
    } = result;
    debug!("ui_mail: result mail {mail_id} action {action} error {error} {equip_error:?} {item:?}");
    let item_guid = item.map_or(0, |(guid, _)| guid);
    if action == mail_action::ITEM_TAKEN && error != mail_error::OK {
        mail.auto_looting = None;
    }
    match error {
        mail_error::OK => match action {
            mail_action::SEND => {
                ui.line(UiError::key(MAIL_SENT_KEY));
                ui.write(UiScript::mail_sent);
            }
            mail_action::MONEY_TAKEN => remove_money(mail, commands, mail_id, ui),
            mail_action::ITEM_TAKEN => remove_item(mail, commands, mail_id, item_guid, ui),
            mail_action::RETURNED | mail_action::DELETED => remove_mail(mail, mail_id, ui),
            mail_action::MADE_PERMANENT => {
                if let Some(i) = mail.index_of(mail_id) {
                    mail.rows[i].entry.flags |= mail_flags::COPIED;
                    ui.fire("MAIL_INBOX_UPDATE", Vec::new());
                }
            }
            _ => {}
        },
        // The result names no bag, so the plain line.
        mail_error::EQUIP_ERROR => ui.line(UiError::key(crate::ui_items::equip_error_key(
            equip_error.unwrap_or(0),
        ))),
        mail_error::ITEM_HAS_EXPIRED => {
            ui.line(UiError::key("ERR_MAIL_ATTACHMENT_EXPIRED"));
            ui.world(move |w| remove_item_in_turn(w, mail_id, item_guid));
        }
        other => {
            if let Some(key) = mail_refusal(other) {
                ui.line(UiError::key(key));
            }
        }
    }
    if action == mail_action::SEND {
        ui.write(|s| s.mail_command_done(true));
    }
    // An auto-loot takes its next attachment on each OK take (`0x57195f`).
    let continued = error == mail_error::OK
        && action == mail_action::ITEM_TAKEN
        && mail
            .auto_looting
            .take()
            .is_some_and(|id| next_auto_loot(mail, commands, id, purse, ui));
    if !continued {
        ui.write(|s| s.mail_command_done(false));
    }
    let event = if error == mail_error::OK {
        "MAIL_SUCCESS"
    } else {
        "MAIL_FAILED"
    };
    ui.fire(event, Vec::new());
}

/// The expired attachment's removal in its turn after the line, its events fired there.
fn remove_item_in_turn(world: &mut World, mail_id: u32, item_guid: u32) {
    let Some(commands) = world
        .get_resource::<NetCommands>()
        .map(|c| NetCommands(c.0.clone()))
    else {
        return;
    };
    world.resource_scope(|w, mut mail: Mut<MailOpen>| {
        if let Some(mut ui) = w.get_resource_mut::<UiEvents>() {
            remove_item(&mut mail, &commands, mail_id, item_guid, &mut ui);
        }
    });
}

/// The attachment auto-loot step (`0x570270`): the next attachment's take, refused locally when the
/// purse cannot pay the COD; true when a take went out.
fn next_auto_loot(
    mail: &mut MailOpen,
    commands: &NetCommands,
    message_id: u32,
    purse: u64,
    ui: &mut UiEvents,
) -> bool {
    let Some(mailbox) = mail.mailbox else {
        return false;
    };
    let Some(i) = mail.index_of(message_id) else {
        return false;
    };
    let row = &mail.rows[i];
    if purse < u64::from(row.entry.cod) {
        ui.line(UiError::key("ERR_NOT_ENOUGH_MONEY"));
        return false;
    }
    let mut filled = row.slots.iter().flatten().filter(|a| a.entry != 0);
    let Some(first) = filled.next() else {
        return false;
    };
    let item_guid_low = first.item_guid_low;
    let more = filled.next().is_some();
    let _ = commands.0.send(ClientCommand::MailTakeItem {
        mailbox,
        mail_id: message_id,
        item_guid_low,
    });
    if more {
        mail.auto_looting = Some(message_id);
    }
    true
}

/// The purge after a take: a mail left empty is deleted with the take's reason, then
/// `CLOSE_INBOX_ITEM` names its row (`0x56f890`, `0x56fa2f`).
fn purge_if_empty(
    mail: &mut MailOpen,
    commands: &NetCommands,
    i: usize,
    reason: u32,
    ui: &mut UiEvents,
) {
    if !mail.rows[i].empty_after_take() {
        return;
    }
    if let Some(mailbox) = mail.mailbox {
        let _ = commands.0.send(ClientCommand::MailDelete {
            mailbox,
            mail_id: mail.rows[i].entry.message_id,
            reason,
        });
    }
    mail.rows[i].delete_pending = true;
    ui.fire("CLOSE_INBOX_ITEM", vec![ScriptValue::Int(i as i64 + 1)]);
}

/// The money take's result (`0x56f770`).
fn remove_money(mail: &mut MailOpen, commands: &NetCommands, mail_id: u32, ui: &mut UiEvents) {
    let Some(i) = mail.index_of(mail_id) else {
        return;
    };
    mail.rows[i].entry.money = 0;
    purge_if_empty(mail, commands, i, mail_delete_reason::MONEY_TAKEN, ui);
    ui.fire("MAIL_INBOX_UPDATE", Vec::new());
}

/// The item take's result (`0x56f8d0`): the attachment by its guid low part, and the COD with it.
fn remove_item(
    mail: &mut MailOpen,
    commands: &NetCommands,
    mail_id: u32,
    item_guid: u32,
    ui: &mut UiEvents,
) {
    let Some(i) = mail.index_of(mail_id) else {
        return;
    };
    let Some(slot) = mail.rows[i].slots.iter_mut().find(|s| {
        s.as_ref()
            .is_some_and(|a| a.entry != 0 && a.item_guid_low == item_guid)
    }) else {
        return;
    };
    *slot = None;
    mail.rows[i].entry.cod = 0;
    purge_if_empty(mail, commands, i, mail_delete_reason::ITEM_TAKEN, ui);
    ui.fire("MAIL_INBOX_UPDATE", Vec::new());
}

/// The mail removal (`0x56dc70`): a returned or deleted mail leaves the rows and the total.
fn remove_mail(mail: &mut MailOpen, mail_id: u32, ui: &mut UiEvents) {
    let Some(i) = mail.index_of(mail_id) else {
        return;
    };
    mail.rows.remove(i);
    mail.total = mail.total.saturating_sub(1);
    ui.fire("MAIL_INBOX_UPDATE", Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{MailAttachment, MailEnchant, MailListEntry};

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    fn attachment(slot: u8, guid: u32) -> MailAttachment {
        MailAttachment {
            slot,
            item_guid_low: guid,
            entry: 2589,
            enchants: [MailEnchant::default(); 7],
            random_property_id: 0,
            suffix_factor: 0,
            count: 5,
            spell_charges: 0,
            max_durability: 0,
            durability: 0,
        }
    }

    fn entry(id: u32, kind: u8, money: u32, body: &str, slots: &[(u8, u32)]) -> MailListEntry {
        MailListEntry {
            message_id: id,
            message_type: kind,
            sender_guid: if kind == 0 { 0xAA } else { 0 },
            sender_entry: if kind == 0 { 0 } else { 7 },
            cod: 0,
            package: 0,
            stationery: 41,
            money,
            flags: 0,
            expire_days: 29.0,
            mail_template_id: 0,
            subject: "s".into(),
            body: body.into(),
            attachments: slots.iter().map(|&(s, g)| attachment(s, g)).collect(),
        }
    }

    fn open_with(rows: Vec<MailListEntry>) -> MailOpen {
        let mut mail = MailOpen::default();
        mail.mailbox = Some(0x42);
        mail.rows = rows.into_iter().map(MailRow::new).collect();
        mail.total = mail.rows.len() as u32;
        mail
    }

    fn result(mail_id: u32, action: u32, error: u32, item: Option<(u32, u32)>) -> MailResult {
        MailResult {
            mail_id,
            action,
            error,
            equip_error: None,
            item,
        }
    }

    #[test]
    fn attachments_file_by_slot_and_a_taken_slot_moves_to_the_first_free() {
        let row = MailRow::new(entry(1, 0, 0, "", &[(3, 1), (3, 2), (20, 3)]));
        let guids: Vec<Option<u32>> = row
            .slots
            .iter()
            .take(4)
            .map(|s| s.as_ref().map(|a| a.item_guid_low))
            .collect();
        // Slot 3 again moves to the first free (0); slot 20 wants 0, now taken, so takes 1.
        assert_eq!(guids, vec![Some(2), Some(3), None, Some(1)]);
        assert_eq!(row.num_attachments(), 3);
    }

    #[test]
    fn an_ignored_senders_mail_goes_back_unread() {
        let (cmds, rx) = commands();
        let mut social = SocialState::default();
        social.set_ignores_for_test(vec![0xAA]);
        let mut mail = MailOpen {
            mailbox: Some(0x42),
            ..MailOpen::default()
        };
        let mut pending = MailPending::default();
        let list = MailList {
            total: 3,
            mails: vec![
                entry(1, 0, 100, "", &[]),
                entry(2, 0, 0, "hi", &[]),
                entry(3, 2, 0, "", &[]),
            ],
        };
        let mut asked = Vec::new();
        let mut ui = UiEvents::default();
        mail_list(
            list,
            &mut mail,
            &mut pending,
            &social,
            &cmds,
            &mut |entry| asked.push(entry),
            &mut ui,
        );
        assert_eq!(mail.rows.len(), 1, "only the auction notice stays");
        assert_eq!(asked, [0], "the notice's subject reads its item's record");
        assert_eq!(ui.names(), ["<write>", "MAIL_INBOX_UPDATE"]);
        assert_eq!(mail.total, 1);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(matches!(
            sent.as_slice(),
            [
                ClientCommand::MailReturnToSender {
                    mail_id: 1,
                    sender: 0xAA,
                    ..
                },
                ClientCommand::MailDelete {
                    mail_id: 2,
                    reason: mail_delete_reason::IGNORED,
                    ..
                },
            ]
        ));
    }

    #[test]
    fn an_empty_list_while_lit_requeries_the_next_mail_time() {
        let (cmds, rx) = commands();
        let mut mail = MailOpen::default();
        let mut pending = MailPending::default();
        pending.apply_query_reply(0.0, &[]);
        mail_list(
            MailList::default(),
            &mut mail,
            &mut pending,
            &SocialState::default(),
            &cmds,
            &mut |_| {},
            &mut UiEvents::default(),
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::QueryNextMailTime)
        ));
        assert!(!pending.has_new_mail(), "the query stamps no mail");
    }

    #[test]
    fn a_money_take_purges_an_emptied_letterless_mail() {
        let (cmds, rx) = commands();
        let mut mail = open_with(vec![entry(1, 0, 500, "", &[]), entry(2, 0, 500, "hi", &[])]);
        let mut ui = UiEvents::default();
        send_mail_result(
            result(1, mail_action::MONEY_TAKEN, 0, None),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        send_mail_result(
            result(2, mail_action::MONEY_TAKEN, 0, None),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::MailDelete {
                    mail_id: 1,
                    reason: mail_delete_reason::MONEY_TAKEN,
                    ..
                }]
            ),
            "a letter keeps its row: {sent:?}"
        );
        assert!(mail.rows[0].delete_pending);
        assert_eq!(
            ui.names(),
            [
                "CLOSE_INBOX_ITEM",
                "MAIL_INBOX_UPDATE",
                "<write>",
                "MAIL_SUCCESS",
                "MAIL_INBOX_UPDATE",
                "<write>",
                "MAIL_SUCCESS",
            ],
            "the purge names its row ahead of the inbox update"
        );
        assert_eq!(mail.rows[1].entry.money, 0);
    }

    #[test]
    fn an_item_take_clears_its_slot_and_the_cod() {
        let (cmds, rx) = commands();
        let mut e = entry(1, 0, 0, "hi", &[(0, 0x31), (1, 0x32)]);
        e.cod = 900;
        let mut mail = open_with(vec![e]);
        let mut ui = UiEvents::default();
        send_mail_result(
            result(1, mail_action::ITEM_TAKEN, 0, Some((0x31, 5))),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        assert!(mail.rows[0].slots[0].is_none());
        assert!(mail.rows[0].slots[1].is_some());
        assert_eq!(mail.rows[0].entry.cod, 0);
        assert!(rx.try_recv().is_err(), "a letter with an item left is kept");
    }

    #[test]
    fn auto_loot_takes_the_next_attachment_on_each_ok_take() {
        let (cmds, rx) = commands();
        let mut mail = open_with(vec![entry(1, 2, 0, "", &[(0, 0x31), (4, 0x32), (9, 0x33)])]);
        mail.auto_looting = Some(1);
        let mut ui = UiEvents::default();
        send_mail_result(
            result(1, mail_action::ITEM_TAKEN, 0, Some((0x31, 5))),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::MailTakeItem {
                item_guid_low: 0x32,
                ..
            })
        ));
        assert_eq!(mail.auto_looting, Some(1), "one more after this one");
        assert_eq!(
            ui.names(),
            ["MAIL_INBOX_UPDATE", "MAIL_SUCCESS"],
            "the command stays in flight: no pending flag cleared"
        );
        // A refused take ends it.
        let mut ui = UiEvents::default();
        send_mail_result(
            result(1, mail_action::ITEM_TAKEN, mail_error::INTERNAL_ERROR, None),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        assert_eq!(mail.auto_looting, None);
        assert_eq!(ui.names(), ["<world>", "<write>", "MAIL_FAILED"]);
    }

    #[test]
    fn a_sent_mail_resets_the_form_and_succeeds() {
        let (cmds, _rx) = commands();
        let mut mail = open_with(vec![]);
        let mut ui = UiEvents::default();
        send_mail_result(
            result(0, mail_action::SEND, 0, None),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        // The line, the form reset, both pending flags, then the success.
        assert_eq!(
            ui.names(),
            ["<world>", "<write>", "<write>", "<write>", "MAIL_SUCCESS"]
        );
    }

    #[test]
    fn a_delete_or_return_leaves_the_rows_and_the_total() {
        let (cmds, _rx) = commands();
        let mut mail = open_with(vec![entry(1, 0, 0, "", &[]), entry(2, 0, 0, "", &[])]);
        let mut ui = UiEvents::default();
        send_mail_result(
            result(1, mail_action::DELETED, 0, None),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        assert_eq!(mail.rows.len(), 1);
        assert_eq!(mail.total, 1);
        send_mail_result(
            result(2, mail_action::MADE_PERMANENT, 0, None),
            &mut mail,
            &cmds,
            0,
            &mut ui,
        );
        assert_eq!(
            mail.rows[0].entry.flags & mail_flags::COPIED,
            mail_flags::COPIED
        );
    }
}
