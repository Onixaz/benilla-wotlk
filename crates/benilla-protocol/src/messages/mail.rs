//! Mail wire messages, as 12340's `CGMailInfo` reads and writes them. A mailbox opens locally on a
//! GameObject or NPC use, or from `SMSG_SHOW_MAILBOX`; each mailbox CMSG leads with its guid.

use std::io;

use crate::wire::{
    capacity_hint, read_cstring, read_f32_le, read_i32_le, read_u16_le, read_u32_le, read_u64_le,
    read_u8,
};

/// [`MailListEntry::message_type`]: each value fixes the form of the row's sender field.
pub mod mail_message_type {
    /// A player: the sender field is an 8-byte guid.
    pub const NORMAL: u8 = 0;
    /// An auction-house notice: the sender field is the `AuctionHouse.dbc` row.
    pub const AUCTION: u8 = 2;
    /// A creature: the sender field is a creature entry.
    pub const CREATURE: u8 = 3;
    /// A GameObject: the sender field is a GameObject entry.
    pub const GAMEOBJECT: u8 = 4;
    /// A calendar notice: the sender field is a u32; the subject leads with the creator's guid.
    pub const CALENDAR: u8 = 5;
}

/// [`MailListEntry::flags`] bits, the server's `checked` mask.
pub mod mail_flags {
    pub const READ: u32 = 0x1;
    pub const RETURNED: u32 = 0x2;
    /// The letter was copied to an item (`textCreated`).
    pub const COPIED: u32 = 0x4;
    pub const COD_PAYMENT: u32 = 0x8;
    pub const HAS_BODY: u32 = 0x10;
    /// A calendar notice for a removed invite rather than a removed event (`0x56f520`).
    pub const CALENDAR_INVITE_REMOVED: u32 = 0x100;
    /// Shown as returned and never returnable, like RETURNED and COD_PAYMENT (`0x56f280`'s
    /// `0x20A` mask).
    pub const UNRETURNABLE: u32 = 0x200;
}

/// `SMSG_SEND_MAIL_RESULT`'s `action`.
pub mod mail_action {
    pub const SEND: u32 = 0;
    pub const MONEY_TAKEN: u32 = 1;
    pub const ITEM_TAKEN: u32 = 2;
    pub const RETURNED: u32 = 3;
    pub const DELETED: u32 = 4;
    pub const MADE_PERMANENT: u32 = 5;
}

/// `SMSG_SEND_MAIL_RESULT`'s `error`, as 12340's handler `0x5717b0` switches on it.
pub mod mail_error {
    pub const OK: u32 = 0;
    pub const EQUIP_ERROR: u32 = 1;
    pub const CANNOT_SEND_TO_SELF: u32 = 2;
    pub const NOT_ENOUGH_MONEY: u32 = 3;
    pub const RECIPIENT_NOT_FOUND: u32 = 4;
    pub const NOT_YOUR_TEAM: u32 = 5;
    pub const INTERNAL_ERROR: u32 = 6;
    pub const TRIAL_ACCOUNT: u32 = 14;
    pub const RECIPIENT_CAP_REACHED: u32 = 15;
    pub const CANT_SEND_WRAPPED_COD: u32 = 16;
    pub const MAIL_AND_CHAT_SUSPENDED: u32 = 17;
    pub const TOO_MANY_ATTACHMENTS: u32 = 18;
    pub const ATTACHMENT_INVALID: u32 = 19;
    /// Shows nothing (`0x5717b0`'s arm 20).
    pub const SILENT: u32 = 20;
    /// An expired attachment on a take; it carries the take's item tail, as `OK` does.
    pub const ITEM_HAS_EXPIRED: u32 = 21;
}

/// `CMSG_MAIL_DELETE`'s trailing reason: a player's delete, or one of the client's own purges.
pub mod mail_delete_reason {
    /// `DeleteInboxItem` (`0x56f23a`).
    pub const PLAYER: u32 = 0;
    /// An ignored sender's empty mail, dropped as the list lands (`0x571ff9`).
    pub const IGNORED: u32 = 1;
    /// Emptied by a money take (`0x56f770`).
    pub const MONEY_TAKEN: u32 = 2;
    /// Emptied by an item take (`0x56f8d0`).
    pub const ITEM_TAKEN: u32 = 3;
}

/// The most attachments one inbox row holds (`MailboxMessageInfo`'s 16 slots).
pub const MAIL_ATTACHMENTS_MAX: usize = 16;
/// The most `MSG_QUERY_NEXT_MAIL_TIME` sender records the client keeps (`0x56db26`).
pub const MAIL_RECENT_SENDERS: usize = 3;

/// One enchant slot of an attachment: id, duration and charges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MailEnchant {
    pub id: u32,
    pub duration: u32,
    pub charges: u32,
}

/// One attachment of a list row (`0x571e29`-`0x571f29`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailAttachment {
    /// The attachment slot the server names, `0..16`.
    pub slot: u8,
    /// The item's guid low part, the key `CMSG_MAIL_TAKE_ITEM` names it by.
    pub item_guid_low: u32,
    pub entry: u32,
    /// The item's seven enchant slots, permanent first.
    pub enchants: [MailEnchant; 7],
    pub random_property_id: i32,
    pub suffix_factor: u32,
    pub count: u32,
    pub spell_charges: i32,
    pub max_durability: u32,
    pub durability: u32,
}

/// One `SMSG_MAIL_LIST_RESULT` row (`0x571c50`). `sender_guid` and `sender_entry` are one wire
/// field whose form `message_type` picks; the other reads 0.
#[derive(Debug, Clone, PartialEq)]
pub struct MailListEntry {
    pub message_id: u32,
    pub message_type: u8,
    pub sender_guid: u64,
    pub sender_entry: u32,
    pub cod: u32,
    /// The `Package.dbc` row, 0 for none.
    pub package: u32,
    pub stationery: u32,
    pub money: u32,
    /// [`mail_flags`].
    pub flags: u32,
    pub expire_days: f32,
    pub mail_template_id: u32,
    pub subject: String,
    /// The letter, inline; empty when there is none.
    pub body: String,
    pub attachments: Vec<MailAttachment>,
}

/// The whole `SMSG_MAIL_LIST_RESULT`: the shown rows and how many the server holds in all.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MailList {
    pub total: u32,
    pub mails: Vec<MailListEntry>,
}

/// One `MSG_QUERY_NEXT_MAIL_TIME` sender record (`0x56db2c`-`0x56db5b`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MailSender {
    /// A player sender's guid, else 0.
    pub guid: u64,
    /// A non-player sender's entry, else 0.
    pub entry: u32,
    pub message_type: u32,
    pub stationery: u32,
    /// Seconds until the mail is delivered; `<= 0` is delivered.
    pub delay: f32,
}

/// `SMSG_SEND_MAIL_RESULT`'s fields (`0x5717b0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailResult {
    pub mail_id: u32,
    pub action: u32,
    pub error: u32,
    /// The inventory error, on [`mail_error::EQUIP_ERROR`].
    pub equip_error: Option<u32>,
    /// `(item guid low, count)`, on an [`mail_action::ITEM_TAKEN`] that is `OK` or
    /// `ITEM_HAS_EXPIRED`.
    pub item: Option<(u32, u32)>,
}

/// `CMSG_GET_MAIL_LIST` (`0x56e440`): the mailbox guid.
pub fn get_mail_list(mailbox: u64) -> Vec<u8> {
    mailbox.to_le_bytes().to_vec()
}

/// `CMSG_SEND_MAIL` (`0x570910`): mailbox guid, receiver, subject and body cstrings, `u32`
/// stationery and package, `u8` attachment count, `(u8 slot, u64 item guid)` per attachment,
/// `u32` money and COD, then a zero `u64` and a zero `u8`.
#[allow(clippy::too_many_arguments)]
pub fn send_mail(
    mailbox: u64,
    receiver: &str,
    subject: &str,
    body: &str,
    stationery: u32,
    package: u32,
    items: &[(u8, u64)],
    money: u32,
    cod: u32,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        8 + receiver.len() + subject.len() + body.len() + 3 + 9 + items.len() * 9 + 8 + 9,
    );
    out.extend_from_slice(&mailbox.to_le_bytes());
    for s in [receiver, subject, body] {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(&stationery.to_le_bytes());
    out.extend_from_slice(&package.to_le_bytes());
    out.push(items.len() as u8);
    for &(slot, guid) in items {
        out.push(slot);
        out.extend_from_slice(&guid.to_le_bytes());
    }
    out.extend_from_slice(&money.to_le_bytes());
    out.extend_from_slice(&cod.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.push(0);
    out
}

fn mailbox_and_mail_id(mailbox: u64, mail_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(20);
    body.extend_from_slice(&mailbox.to_le_bytes());
    body.extend_from_slice(&mail_id.to_le_bytes());
    body
}

/// `CMSG_MAIL_TAKE_MONEY` (`0x56edf0`): answered by a [`mail_action::MONEY_TAKEN`] result.
pub fn mail_take_money(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_MAIL_TAKE_ITEM` (`0x5711c0`): the attachment by its item guid's low part; answered by an
/// [`mail_action::ITEM_TAKEN`] result naming it.
pub fn mail_take_item(mailbox: u64, mail_id: u32, item_guid_low: u32) -> Vec<u8> {
    let mut body = mailbox_and_mail_id(mailbox, mail_id);
    body.extend_from_slice(&item_guid_low.to_le_bytes());
    body
}

/// `CMSG_MAIL_MARK_AS_READ` (`0x56fe30`): there is no reply.
pub fn mail_mark_as_read(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_MAIL_RETURN_TO_SENDER` (`0x56f000`): the sender's guid trails; answered by a
/// [`mail_action::RETURNED`] result.
pub fn mail_return_to_sender(mailbox: u64, mail_id: u32, sender: u64) -> Vec<u8> {
    let mut body = mailbox_and_mail_id(mailbox, mail_id);
    body.extend_from_slice(&sender.to_le_bytes());
    body
}

/// `CMSG_MAIL_DELETE` (`0x56f140`): a [`mail_delete_reason`] trails; answered by a
/// [`mail_action::DELETED`] result.
pub fn mail_delete(mailbox: u64, mail_id: u32, reason: u32) -> Vec<u8> {
    let mut body = mailbox_and_mail_id(mailbox, mail_id);
    body.extend_from_slice(&reason.to_le_bytes());
    body
}

/// `CMSG_MAIL_CREATE_TEXT_ITEM` (`0x56eef0`): answered by a [`mail_action::MADE_PERMANENT`]
/// result, on which the client sets the mail's COPIED bit.
pub fn mail_create_text_item(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_ITEM_TEXT_QUERY`: the item guid alone, as the item-text cache sends no requester
/// (`0x669110`).
pub fn item_text_query(item_guid: u64) -> Vec<u8> {
    item_guid.to_le_bytes().to_vec()
}

fn read_attachment(r: &mut &[u8]) -> io::Result<MailAttachment> {
    let slot = read_u8(r)?;
    let item_guid_low = read_u32_le(r)?;
    let entry = read_u32_le(r)?;
    let mut enchants = [MailEnchant::default(); 7];
    for e in &mut enchants {
        *e = MailEnchant {
            id: read_u32_le(r)?,
            duration: read_u32_le(r)?,
            charges: read_u32_le(r)?,
        };
    }
    let attachment = MailAttachment {
        slot,
        item_guid_low,
        entry,
        enchants,
        random_property_id: read_i32_le(r)?,
        suffix_factor: read_u32_le(r)?,
        count: read_u32_le(r)?,
        spell_charges: read_i32_le(r)?,
        max_durability: read_u32_le(r)?,
        durability: read_u32_le(r)?,
    };
    let _unk = read_u8(r)?; // stored at `+0x78`, never read
    Ok(attachment)
}

/// `SMSG_MAIL_LIST_RESULT` (`0x571c50`): `u32` total, `u8` shown, then the rows. Each row leads
/// with a `u16` size the client skips; a type past 5 has no sender field.
pub(super) fn read_mail_list_result(r: &mut &[u8]) -> io::Result<MailList> {
    let total = read_u32_le(r)?;
    let count = read_u8(r)?;
    // The client grows its inbox to whatever count comes; a u8 bounds it.
    let mut mails = Vec::with_capacity(capacity_hint(count, usize::from(u8::MAX)));
    for _ in 0..count {
        let _size = read_u16_le(r)?;
        let message_id = read_u32_le(r)?;
        let message_type = read_u8(r)?;
        let (sender_guid, sender_entry) = match message_type {
            mail_message_type::NORMAL => (read_u64_le(r)?, 0),
            1..=5 => (0, read_u32_le(r)?),
            _ => (0, 0),
        };
        let cod = read_u32_le(r)?;
        let package = read_u32_le(r)?;
        let stationery = read_u32_le(r)?;
        let money = read_u32_le(r)?;
        let flags = read_u32_le(r)?;
        let expire_days = read_f32_le(r)?;
        let mail_template_id = read_u32_le(r)?;
        let subject = read_cstring(r)?;
        let body = read_cstring(r)?;
        let n = read_u8(r)?;
        let mut attachments = Vec::with_capacity(capacity_hint(n, MAIL_ATTACHMENTS_MAX));
        for _ in 0..n {
            attachments.push(read_attachment(r)?);
        }
        mails.push(MailListEntry {
            message_id,
            message_type,
            sender_guid,
            sender_entry,
            cod,
            package,
            stationery,
            money,
            flags,
            expire_days,
            mail_template_id,
            subject,
            body,
            attachments,
        });
    }
    Ok(MailList { total, mails })
}

/// `SMSG_SEND_MAIL_RESULT` (`0x5717b0`): `u32 mailId, action, error`, then `u32 equipError` on
/// `EQUIP_ERROR`, or `u32 itemGuidLow, itemCount` on an `ITEM_TAKEN` that is `OK` or
/// `ITEM_HAS_EXPIRED`.
pub(super) fn read_send_mail_result(r: &mut &[u8]) -> io::Result<MailResult> {
    let mail_id = read_u32_le(r)?;
    let action = read_u32_le(r)?;
    let error = read_u32_le(r)?;
    let mut equip_error = None;
    let mut item = None;
    if error == mail_error::EQUIP_ERROR {
        equip_error = Some(read_u32_le(r)?);
    } else if action == mail_action::ITEM_TAKEN
        && matches!(error, mail_error::OK | mail_error::ITEM_HAS_EXPIRED)
    {
        item = Some((read_u32_le(r)?, read_u32_le(r)?));
    }
    Ok(MailResult {
        mail_id,
        action,
        error,
        equip_error,
        item,
    })
}

/// `SMSG_ITEM_TEXT_QUERY_RESPONSE` (`0x635a40`): `u8` result, then on 0 the item guid and its
/// text; any other result carries nothing.
pub(super) fn read_item_text_query_response(r: &mut &[u8]) -> io::Result<Option<(u64, String)>> {
    if read_u8(r)? != 0 {
        return Ok(None);
    }
    Ok(Some((read_u64_le(r)?, read_cstring(r)?)))
}

/// `SMSG_RECEIVED_MAIL` (`0x571a10`): an `f32` delay in seconds until the mail is delivered.
pub(super) fn read_received_mail(r: &mut &[u8]) -> io::Result<f32> {
    read_f32_le(r)
}

/// The `MSG_QUERY_NEXT_MAIL_TIME` reply (`0x56daf0`; the request is an empty body on the same
/// opcode): `f32` next-mail time (`0.0` unread mail waiting, `-86400.0` none), `u32` count, then
/// sender records, of which the client reads the first three.
pub(super) fn read_query_next_mail_time(r: &mut &[u8]) -> io::Result<(f32, Vec<MailSender>)> {
    let seconds = read_f32_le(r)?;
    let count = read_u32_le(r)?;
    let mut senders = Vec::with_capacity(MAIL_RECENT_SENDERS);
    for _ in 0..(count as usize).min(MAIL_RECENT_SENDERS) {
        senders.push(MailSender {
            guid: read_u64_le(r)?,
            entry: read_u32_le(r)?,
            message_type: read_u32_le(r)?,
            stationery: read_u32_le(r)?,
            delay: read_f32_le(r)?,
        });
    }
    Ok((seconds, senders))
}

/// `SMSG_SHOW_MAILBOX` (`0x56dbc0`): the mailbox guid the server opens the window on.
pub(super) fn read_show_mailbox(r: &mut &[u8]) -> io::Result<u64> {
    read_u64_le(r)
}
