//! The mailbox sends. A verb on one mail is acked by `SMSG_SEND_MAIL_RESULT` carrying its action.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_GET_MAIL_LIST`: opens and refreshes the inbox; answered by `SMSG_MAIL_LIST_RESULT`.
    pub fn get_mail_list(&mut self, mailbox: u64) -> Result<()> {
        self.send(
            opcode::CMSG_GET_MAIL_LIST,
            &messages::get_mail_list(mailbox),
        )
    }

    /// `CMSG_SEND_MAIL`: `items` are the `(attachment slot, item guid)` pairs, empty for none.
    #[allow(clippy::too_many_arguments)]
    pub fn send_mail(
        &mut self,
        mailbox: u64,
        receiver: &str,
        subject: &str,
        body: &str,
        stationery: u32,
        package: u32,
        items: &[(u8, u64)],
        money: u32,
        cod: u32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_SEND_MAIL,
            &messages::send_mail(
                mailbox, receiver, subject, body, stationery, package, items, money, cod,
            ),
        )
    }

    /// `CMSG_MAIL_TAKE_MONEY`: acked with [`messages::mail_action::MONEY_TAKEN`].
    pub fn mail_take_money(&mut self, mailbox: u64, mail_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_MAIL_TAKE_MONEY,
            &messages::mail_take_money(mailbox, mail_id),
        )
    }

    /// `CMSG_MAIL_TAKE_ITEM`: one attachment by its item guid's low part; acked with
    /// [`messages::mail_action::ITEM_TAKEN`].
    pub fn mail_take_item(&mut self, mailbox: u64, mail_id: u32, item_guid_low: u32) -> Result<()> {
        self.send(
            opcode::CMSG_MAIL_TAKE_ITEM,
            &messages::mail_take_item(mailbox, mail_id, item_guid_low),
        )
    }

    /// `CMSG_MAIL_MARK_AS_READ`: sent when a letter opens. There is no reply.
    pub fn mail_mark_as_read(&mut self, mailbox: u64, mail_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_MAIL_MARK_AS_READ,
            &messages::mail_mark_as_read(mailbox, mail_id),
        )
    }

    /// `CMSG_MAIL_RETURN_TO_SENDER`: acked with [`messages::mail_action::RETURNED`].
    pub fn mail_return_to_sender(&mut self, mailbox: u64, mail_id: u32, sender: u64) -> Result<()> {
        self.send(
            opcode::CMSG_MAIL_RETURN_TO_SENDER,
            &messages::mail_return_to_sender(mailbox, mail_id, sender),
        )
    }

    /// `CMSG_MAIL_DELETE`: a [`messages::mail_delete_reason`] trails; acked with
    /// [`messages::mail_action::DELETED`].
    pub fn mail_delete(&mut self, mailbox: u64, mail_id: u32, reason: u32) -> Result<()> {
        self.send(
            opcode::CMSG_MAIL_DELETE,
            &messages::mail_delete(mailbox, mail_id, reason),
        )
    }

    /// `CMSG_MAIL_CREATE_TEXT_ITEM`: a permanent copy of the letter; acked with
    /// [`messages::mail_action::MADE_PERMANENT`].
    pub fn mail_create_text_item(&mut self, mailbox: u64, mail_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_MAIL_CREATE_TEXT_ITEM,
            &messages::mail_create_text_item(mailbox, mail_id),
        )
    }

    /// `CMSG_ITEM_TEXT_QUERY`: a copied letter's text, by the letter item's guid.
    pub fn item_text_query(&mut self, item_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_ITEM_TEXT_QUERY,
            &messages::item_text_query(item_guid),
        )
    }

    /// `MSG_QUERY_NEXT_MAIL_TIME`, empty: the reply is the next-mail time and up to three senders.
    pub fn query_next_mail_time(&mut self) -> Result<()> {
        self.send(opcode::MSG_QUERY_NEXT_MAIL_TIME, &[])
    }
}
