//! The mailbox's packet handlers: they fill [`MailOpen`] and [`MailPending`] and queue the
//! window's effects, which [`super::feed_mail`] applies.

use benilla_protocol::messages::{
    mail_action, mail_delete_reason, mail_error, mail_flags, mail_message_type, MailList,
    MailResult,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{mail_refusal, MailEffect, MailOpen, MailPending, MailRow, MAIL_SENT_KEY};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_social::SocialState;

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::MailList, on_mail_list)
        .net_handler(K::SendMailResult, on_send_mail_result)
        .net_handler(K::ReceivedMail, on_received_mail)
        .net_handler(K::NextMailTime, on_next_mail_time)
        .net_handler(K::ShowMailbox, on_show_mailbox)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_mail_list(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
    social: Res<SocialState>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::MailList(list) = ev {
        mail_list(list, &mut mail, &mut pending, &social, &commands);
    }
}

fn on_send_mail_result(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    commands: Res<NetCommands>,
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
) {
    if let SessionEvent::SendMailResult(result) = ev {
        let purse = self_q
            .iter()
            .next()
            .and_then(|s| s.0.player_money())
            .map_or(0, u64::from);
        send_mail_result(result, &mut mail, &commands, purse);
    }
}

fn on_received_mail(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<MailPending>,
    mail: Res<MailOpen>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::ReceivedMail { seconds } = ev {
        super::received_mail(
            seconds,
            "SMSG_RECEIVED_MAIL",
            &mut pending,
            &mail,
            &commands,
        );
    }
}

fn on_next_mail_time(In(ev): In<SessionEvent>, mut pending: ResMut<MailPending>) {
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
    }
}

/// `SMSG_SHOW_MAILBOX` (`0x56dbc0`): the server opens the window, as the mailbox open does.
fn on_show_mailbox(In(ev): In<SessionEvent>, mut mail: ResMut<MailOpen>) {
    if let SessionEvent::ShowMailbox { mailbox } = ev {
        mail.click(mailbox);
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
/// deleted, and leaves the counts (`0x571f4c`-`0x5720a3`). An empty inbox while the icon is lit
/// re-queries the next-mail time.
fn mail_list(
    list: MailList,
    mail: &mut MailOpen,
    pending: &mut MailPending,
    social: &SocialState,
    commands: &NetCommands,
) {
    let mailbox = mail.mailbox.unwrap_or(0);
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
            row.entry.stationery = mail.default_stationery;
        }
        rows.push(row);
    }
    mail.rows = rows;
    mail.total = total;
    mail.effects.push(MailEffect::Done { send: false });
    if mail.rows.is_empty() && pending.has_new_mail() {
        super::query_next_mail_time(pending, commands);
    }
    mail.effects.push(MailEffect::InboxUpdate);
}

/// `SMSG_SEND_MAIL_RESULT` (`0x5717b0`).
fn send_mail_result(result: MailResult, mail: &mut MailOpen, commands: &NetCommands, purse: u64) {
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
                mail.effects.push(MailEffect::Message(MAIL_SENT_KEY));
                mail.effects.push(MailEffect::Sent);
            }
            mail_action::MONEY_TAKEN => remove_money(mail, commands, mail_id),
            mail_action::ITEM_TAKEN => remove_item(mail, commands, mail_id, item_guid),
            mail_action::RETURNED | mail_action::DELETED => remove_mail(mail, mail_id),
            mail_action::MADE_PERMANENT => {
                if let Some(i) = mail.index_of(mail_id) {
                    mail.rows[i].entry.flags |= mail_flags::COPIED;
                    mail.effects.push(MailEffect::InboxUpdate);
                }
            }
            _ => {}
        },
        mail_error::EQUIP_ERROR => mail
            .effects
            .push(MailEffect::EquipError(equip_error.unwrap_or(0))),
        mail_error::ITEM_HAS_EXPIRED => {
            mail.effects
                .push(MailEffect::Message("ERR_MAIL_ATTACHMENT_EXPIRED"));
            remove_item(mail, commands, mail_id, item_guid);
        }
        other => mail
            .effects
            .extend(mail_refusal(other).map(MailEffect::Message)),
    }
    if action == mail_action::SEND {
        mail.effects.push(MailEffect::Done { send: true });
    }
    // An auto-loot takes its next attachment on each OK take (`0x57195f`).
    let continued = error == mail_error::OK
        && action == mail_action::ITEM_TAKEN
        && mail
            .auto_looting
            .take()
            .is_some_and(|id| next_auto_loot(mail, commands, id, purse));
    if !continued {
        mail.effects.push(MailEffect::Done { send: false });
    }
    mail.effects.push(MailEffect::Result {
        ok: error == mail_error::OK,
    });
}

/// The attachment auto-loot step (`0x570270`): the next attachment's take, refused locally when the
/// purse cannot pay the COD; true when a take went out.
fn next_auto_loot(
    mail: &mut MailOpen,
    commands: &NetCommands,
    message_id: u32,
    purse: u64,
) -> bool {
    let Some(mailbox) = mail.mailbox else {
        return false;
    };
    let Some(i) = mail.index_of(message_id) else {
        return false;
    };
    let row = &mail.rows[i];
    if purse < u64::from(row.entry.cod) {
        mail.effects
            .push(MailEffect::Message("ERR_NOT_ENOUGH_MONEY"));
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
fn purge_if_empty(mail: &mut MailOpen, commands: &NetCommands, i: usize, reason: u32) {
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
    mail.effects.push(MailEffect::CloseInboxItem(i as u32 + 1));
}

/// The money take's result (`0x56f770`).
fn remove_money(mail: &mut MailOpen, commands: &NetCommands, mail_id: u32) {
    let Some(i) = mail.index_of(mail_id) else {
        return;
    };
    mail.rows[i].entry.money = 0;
    purge_if_empty(mail, commands, i, mail_delete_reason::MONEY_TAKEN);
    mail.effects.push(MailEffect::InboxUpdate);
}

/// The item take's result (`0x56f8d0`): the attachment by its guid low part, and the COD with it.
fn remove_item(mail: &mut MailOpen, commands: &NetCommands, mail_id: u32, item_guid: u32) {
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
    purge_if_empty(mail, commands, i, mail_delete_reason::ITEM_TAKEN);
    mail.effects.push(MailEffect::InboxUpdate);
}

/// The mail removal (`0x56dc70`): a returned or deleted mail leaves the rows and the total.
fn remove_mail(mail: &mut MailOpen, mail_id: u32) {
    let Some(i) = mail.index_of(mail_id) else {
        return;
    };
    mail.rows.remove(i);
    mail.total = mail.total.saturating_sub(1);
    mail.effects.push(MailEffect::InboxUpdate);
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
        mail.click(0x42);
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
        let mut mail = MailOpen::default();
        mail.click(0x42);
        let mut pending = MailPending::default();
        let list = MailList {
            total: 3,
            mails: vec![
                entry(1, 0, 100, "", &[]),
                entry(2, 0, 0, "hi", &[]),
                entry(3, 2, 0, "", &[]),
            ],
        };
        mail_list(list, &mut mail, &mut pending, &social, &cmds);
        assert_eq!(mail.rows.len(), 1, "only the auction notice stays");
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
        send_mail_result(
            result(1, mail_action::MONEY_TAKEN, 0, None),
            &mut mail,
            &cmds,
            0,
        );
        send_mail_result(
            result(2, mail_action::MONEY_TAKEN, 0, None),
            &mut mail,
            &cmds,
            0,
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
        assert!(mail
            .effects
            .iter()
            .any(|e| matches!(e, MailEffect::CloseInboxItem(1))));
        assert_eq!(mail.rows[1].entry.money, 0);
    }

    #[test]
    fn an_item_take_clears_its_slot_and_the_cod() {
        let (cmds, rx) = commands();
        let mut e = entry(1, 0, 0, "hi", &[(0, 0x31), (1, 0x32)]);
        e.cod = 900;
        let mut mail = open_with(vec![e]);
        send_mail_result(
            result(1, mail_action::ITEM_TAKEN, 0, Some((0x31, 5))),
            &mut mail,
            &cmds,
            0,
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
        send_mail_result(
            result(1, mail_action::ITEM_TAKEN, 0, Some((0x31, 5))),
            &mut mail,
            &cmds,
            0,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::MailTakeItem {
                item_guid_low: 0x32,
                ..
            })
        ));
        assert_eq!(mail.auto_looting, Some(1), "one more after this one");
        assert!(
            !mail
                .effects
                .iter()
                .any(|e| matches!(e, MailEffect::Done { send: false })),
            "the command stays in flight"
        );
        // A refused take ends it.
        mail.effects.clear();
        send_mail_result(
            result(1, mail_action::ITEM_TAKEN, mail_error::INTERNAL_ERROR, None),
            &mut mail,
            &cmds,
            0,
        );
        assert_eq!(mail.auto_looting, None);
        assert!(mail
            .effects
            .iter()
            .any(|e| matches!(e, MailEffect::Result { ok: false })));
    }

    #[test]
    fn a_sent_mail_resets_the_form_and_succeeds() {
        let (cmds, _rx) = commands();
        let mut mail = open_with(vec![]);
        send_mail_result(result(0, mail_action::SEND, 0, None), &mut mail, &cmds, 0);
        let kinds: Vec<&str> = mail
            .effects
            .iter()
            .map(|e| match e {
                MailEffect::Message(k) => *k,
                MailEffect::Sent => "sent",
                MailEffect::Done { send: true } => "done-send",
                MailEffect::Done { send: false } => "done",
                MailEffect::Result { ok: true } => "MAIL_SUCCESS",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["ERR_MAIL_SENT", "sent", "done-send", "done", "MAIL_SUCCESS"]
        );
    }

    #[test]
    fn a_delete_or_return_leaves_the_rows_and_the_total() {
        let (cmds, _rx) = commands();
        let mut mail = open_with(vec![entry(1, 0, 0, "", &[]), entry(2, 0, 0, "", &[])]);
        send_mail_result(
            result(1, mail_action::DELETED, 0, None),
            &mut mail,
            &cmds,
            0,
        );
        assert_eq!(mail.rows.len(), 1);
        assert_eq!(mail.total, 1);
        send_mail_result(
            result(2, mail_action::MADE_PERMANENT, 0, None),
            &mut mail,
            &cmds,
            0,
        );
        assert_eq!(
            mail.rows[0].entry.flags & mail_flags::COPIED,
            mail_flags::COPIED
        );
    }
}
