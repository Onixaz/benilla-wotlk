//! The mail bindings, 12340's mail verbs. The app pushes the inbox, resolved from the wire
//! (`UiScript::set_mail`), and whether a mailbox is open; the verbs gate on the reference's
//! command-pending flag and queue [`MailCommand`]s the app sends. Indices are 1-based, as
//! `MailFrame.lua` uses them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, number_arg};
use super::cursor::{self, CursorItem, CursorPayload};
use super::{Model, ScriptValue};

/// The send slots the reference fills (`0x56fff0` refuses slot 12 and past); the array holds 16.
pub const MAIL_SEND_SLOTS: usize = 12;
/// `MailboxMessageInfo`'s attachment slots.
pub const MAIL_ATTACHMENT_SLOTS: usize = 16;
/// The send array's size, 16 slots in the reference's mail state, which the readers bound by.
const SEND_ARRAY: usize = 16;

/// `CheckInbox`'s rate limit: a list query at most once a minute (`0x56e499`).
const CHECK_INBOX_THROTTLE_SECS: f64 = 60.0;

/// The postage per attached item, and the floor with none (`0x571b00`).
const POSTAGE_PER_ITEM: u32 = 30;

/// `MAIL_STATIONERY_GM`, the row a GM's mail carries (`0x56e75b`).
pub const STATIONERY_GM: u32 = 61;

/// [`MailInboxRow::flags`] bits the verbs read.
mod flags {
    pub const READ: u32 = 0x1;
    pub const COPIED: u32 = 0x4;
    /// RETURNED, COD_PAYMENT and `0x200`: never returned, always deletable (`0x56f280`).
    pub const UNRETURNABLE: u32 = 0x20A;
    pub const SHOWN_RETURNED: u32 = 0x202;
}

/// One attachment of an inbox row, resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailAttachmentView {
    pub item_id: u32,
    /// The guid low part `CMSG_MAIL_TAKE_ITEM` names the attachment by.
    pub item_guid_low: u32,
    /// The name with its random suffix; `None` while the template is in flight, when
    /// `GetInboxItem` answers `UNKNOWNOBJECT` (`0x57112e`).
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// The quality, -1 for an item whose `InventoryType` is 0 (`0x5710c1`).
    pub quality: i32,
    pub link: Option<String>,
    pub random_property_id: i32,
}

/// One auction invoice, `GetInboxInvoiceInfo`'s answer (`0x56e9b0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailInvoice {
    /// `"seller"`, `"seller_temp_invoice"` or `"buyer"`, literal tokens.
    pub kind: &'static str,
    pub item_name: String,
    /// The counterparty's name; `""` while unresolved (`0x56eb82`).
    pub player_name: String,
    pub bid: u32,
    pub buyout: u32,
    pub deposit: u32,
    pub consignment: u32,
    /// A pending sale's delay before the money is mailed, in seconds.
    pub money_delay: u32,
    /// The pending sale's mailing time of day, -1 for a field the packed time leaves open.
    pub eta_hour: i32,
    pub eta_minute: i32,
}

/// One inbox row, resolved by the app from a wire row; its 1-based index is its position in
/// [`MailState::inbox`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailInboxRow {
    pub message_id: u32,
    /// 0 a player, 2 the auction house, 3 a creature, 4 a GameObject, 5 the calendar.
    pub message_type: u8,
    /// The wire's `checked` flags.
    pub flags: u32,
    pub money: u32,
    pub cod: u32,
    pub days_left: f32,
    /// `stationery == 61`: a GM's mail.
    pub is_gm: bool,
    /// The row has a letter to copy: a body or a mail template (`0x56ef88`).
    pub has_letter: bool,
    /// A return or delete is in flight (`+0x2940`).
    pub delete_pending: bool,
    /// `packageIcon`: the package's, a multi-item crate's, or the one item's icon.
    pub package_icon: Option<String>,
    /// `stationeryIcon`, the stationery item's icon.
    pub stationery_icon: Option<String>,
    /// `sender`, `None` while its name is in flight.
    pub sender: Option<String>,
    /// The displayed subject, with the auction, calendar, template and COD forms applied.
    pub subject: String,
    /// The attachment slots, `None` for an empty slot.
    pub attachments: Vec<Option<MailAttachmentView>>,
    /// `GetInboxText`'s body: `None` for no letter or an auction notice (`0x570c6d`).
    pub body: Option<String>,
    /// `Stationery.dbc`'s texture basename.
    pub stationery_texture: Option<String>,
    /// `GetInboxText`'s `isTakeable` (`0x570e5a`).
    pub takeable: bool,
    /// The auction notice is an invoice (won, sold, or a pending sale), `isInvoice`.
    pub is_invoice: bool,
    /// `None` also while the body is missing (`0x56ea32`).
    pub invoice: Option<MailInvoice>,
    /// `CanComplainInboxItem` (`0x56fa70`).
    pub can_complain: bool,
}

impl MailInboxRow {
    fn num_attachments(&self) -> usize {
        self.attachments.iter().flatten().count()
    }

    /// The first filled slot (`0x56d420`).
    fn first_attachment(&self) -> Option<&MailAttachmentView> {
        self.attachments.iter().flatten().next()
    }

    /// The attachment read (`0x56d920`): `None` (the reference's -1) is the first filled slot.
    fn attachment(&self, slot: Option<u32>) -> Option<&MailAttachmentView> {
        match slot {
            None => self.first_attachment(),
            Some(s) => self.attachments.get(s as usize).and_then(Option::as_ref),
        }
    }

    /// `InboxItemCanDelete` (`0x56f280`): any unreturnable mark, a non-player sender, or nothing
    /// left in it.
    fn can_delete(&self) -> bool {
        self.flags & flags::UNRETURNABLE != 0
            || self.message_type != 0
            || (self.num_attachments() == 0 && self.money == 0)
    }
}

/// The inbox, pushed whole by the app. It outlives the window, as the reference's does until a
/// world change (`0x571730`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailState {
    pub inbox: Vec<MailInboxRow>,
    /// How many mails the server holds, past the shown page (`GetInboxNumItems`' second value).
    pub total: u32,
}

/// One usable stationery in the send tab's picker; the app builds the list from `Stationery.dbc`
/// and the bags, sorted as `0x56dd00` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationeryView {
    /// The `Stationery.dbc` row id, which `SelectStationery` stores and `CMSG_SEND_MAIL` carries.
    pub id: u32,
    pub name: String,
    /// The item's icon as a full `Interface\Icons\` path.
    pub icon: String,
    /// The item's BuyPrice in copper; `None` (nil) when the player carries one.
    pub cost: Option<u32>,
    /// The bare texture basename that `GetSelectedStationeryTexture` answers.
    pub texture: String,
}

/// One `Package.dbc` row, as `GetPackageInfo` answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageView {
    /// The row id, which `SelectPackage` stores and `CMSG_SEND_MAIL` carries.
    pub id: u32,
    pub name: String,
    /// The icon as a full path: `StringLookups.dbc` row 3's folder, `\` and the basename.
    pub icon: String,
    pub cost: i32,
}

/// A queued `SendMail`, which the app turns into `CMSG_SEND_MAIL`.
#[derive(Clone, Debug, PartialEq)]
pub struct MailSendRequest {
    pub target: String,
    pub subject: String,
    pub body: String,
    pub stationery: u32,
    /// 0 with no attachment (`0x570a5a`).
    pub package: u32,
    pub money: u32,
    pub cod: u32,
    /// `(send slot, bag, 1-based slot)` per attachment; the app resolves each guid.
    pub items: Vec<(u8, i64, u32)>,
}

/// A mail verb's packet, drained by the app. Rows are named by message id.
#[derive(Clone, Debug, PartialEq)]
pub enum MailCommand {
    /// `CMSG_GET_MAIL_LIST`.
    GetList,
    /// `CMSG_MAIL_MARK_AS_READ`, from `GetInboxText` on an unread mail (`0x56fe30`).
    MarkRead { message_id: u32 },
    /// `CMSG_MAIL_TAKE_MONEY`.
    TakeMoney { message_id: u32 },
    /// `CMSG_MAIL_TAKE_ITEM`; `auto_loot` when `AutoLootMailItem` has more slots to take
    /// (`0x5703c5`).
    TakeItem {
        message_id: u32,
        item_guid_low: u32,
        auto_loot: bool,
    },
    /// `CMSG_MAIL_CREATE_TEXT_ITEM`.
    TextItem { message_id: u32 },
    /// `CMSG_MAIL_RETURN_TO_SENDER`.
    Return { message_id: u32 },
    /// `CMSG_MAIL_DELETE` with the player's reason.
    Delete { message_id: u32 },
    /// `CMSG_SEND_MAIL`.
    Send(MailSendRequest),
    /// `CloseMail`: the window closed; the app ends its session.
    Close,
}

/// `ClearSendMail` (`0x56d4d0`): each attachment unlocks, firing `ITEM_LOCK_CHANGED`, then money,
/// COD, the lock, the pending send, stationery and package clear; `signal` adds the three form
/// events.
fn clear_send_mail(model: &mut Model, signal: bool) {
    let attached: Vec<CursorItem> = model
        .mail_send_items
        .iter_mut()
        .filter_map(Option::take)
        .collect();
    for item in attached {
        cursor::queue_lock_changed(model, item.bag, item.slot);
    }
    model.mail_send_money = 0;
    model.mail_lock_send_items = false;
    model.mail_send_cod = 0;
    model.mail_send_pending = false;
    model.mail_stationery = 0;
    model.mail_package = 0;
    if signal {
        for event in [
            "SEND_MAIL_MONEY_CHANGED",
            "SEND_MAIL_COD_CHANGED",
            "MAIL_SEND_SUCCESS",
        ] {
            queue(model, event, vec![]);
        }
    }
}

fn queue(model: &mut Model, event: &str, args: Vec<ScriptValue>) {
    model.pending_events.push((event.to_string(), args));
}

fn num_send_items(model: &Model) -> usize {
    model.mail_send_items.iter().flatten().count()
}

/// The reason the outgoing-mail add (`0x56fff0`) refuses an item, by its message key. Its holiday
/// and zone- or instance-bound arms read template fields the VM does not carry; the server refuses
/// those.
fn attach_refusal(model: &Model, item: &CursorItem) -> Option<&'static str> {
    if model.mail_lock_send_items {
        return Some("ERR_CLIENT_LOCKED_OUT");
    }
    let Some(t) = model.item_templates.get(&item.item_id) else {
        return Some("ERR_ITEM_NOT_FOUND");
    };
    let s = model
        .containers
        .get(&item.bag)
        .and_then(|c| c.slots.get(&item.slot));
    if s.is_some_and(|s| s.already_bound) {
        if t.bonding == 4 {
            return Some("ERR_MAIL_QUEST_ITEM");
        }
        // Static flag `0x8000000` lets a bound item be mailed (account-bound heirlooms).
        if t.flags & 0x800_0000 == 0 {
            return Some("ERR_MAIL_BOUND_ITEM");
        }
    }
    if t.flags & 0x2 != 0 {
        return Some("ERR_MAIL_CONJURED_ITEM");
    }
    if t.flags & 0x1_0000 != 0 || s.is_some_and(|s| s.duration_ms.is_some()) {
        return Some("ERR_MAIL_LIMITED_DURATION_ITEM");
    }
    // An equipped bag that still holds items.
    if t.container_slots > 0 && item.bag == cursor::EQUIPMENT_BAG {
        let equipped_bag = i64::from(item.slot) - 19;
        if model
            .containers
            .get(&equipped_bag)
            .is_some_and(|c| c.slots.values().any(|s| s.item_id != 0))
        {
            return Some("ERR_MAIL_BAG");
        }
    }
    None
}

/// The outgoing-mail add (`0x56fff0`): `index` `None` takes the first free send slot. On success
/// the item locks and `MAIL_SEND_INFO_UPDATE` fires.
fn add_item_to_outgoing(model: &mut Model, index: Option<usize>, mut item: CursorItem) -> bool {
    if let Some(key) = attach_refusal(model, &item) {
        model.ui_errors.push(key);
        return false;
    }
    let slot = match index {
        Some(i) => Some(i),
        None => (0..MAIL_SEND_SLOTS).find(|&i| model.mail_send_items[i].is_none()),
    };
    let Some(slot) = slot.filter(|&s| s < MAIL_SEND_SLOTS) else {
        model.ui_errors.push("ERR_MAIL_INVALID_ATTACHMENT_SLOT");
        return false;
    };
    // The send-slot setter (`0x56d290`) drops the same item from any other slot.
    for (i, s) in model.mail_send_items.iter_mut().enumerate() {
        if i != slot
            && s.as_ref()
                .is_some_and(|o| o.bag == item.bag && o.slot == item.slot)
        {
            *s = None;
        }
    }
    // The whole stack goes: the slot names the item, never a split count.
    item.count = None;
    let (bag, bag_slot) = (item.bag, item.slot);
    model.mail_send_items[slot] = Some(item);
    cursor::queue_lock_changed(model, bag, bag_slot);
    queue(model, "MAIL_SEND_INFO_UPDATE", vec![]);
    true
}

/// `UseContainerItem`'s mail arm (`0x5d88cf`): with the Send tab showing at an open mailbox, the
/// clicked item attaches to the first free slot. True when the arm took the click, refused or not.
pub(super) fn use_container_item(model: &mut Model, bag: i64, slot: u32) -> bool {
    if !model.mail_send_showing || !model.mail_mailbox {
        return false;
    }
    let Some(s) = model
        .containers
        .get(&bag)
        .and_then(|c| c.slots.get(&slot))
        .filter(|s| s.item_id != 0)
        .cloned()
    else {
        return false;
    };
    add_item_to_outgoing(
        model,
        None,
        CursorItem {
            bag,
            slot,
            item_id: s.item_id,
            texture: s.texture,
            link: s.link,
            count: None,
            quality: s.quality,
            equip_slots: s.equip_slots,
            bar_placeable: s.bar_placeable,
        },
    );
    true
}

/// A bag slot attached to the outgoing mail reads locked.
pub(super) fn holds_slot(model: &Model, bag: i64, slot: u32) -> bool {
    model
        .mail_send_items
        .iter()
        .flatten()
        .any(|it| it.bag == bag && it.slot == slot)
}

/// The attachment in send slot `index`, for the tooltip.
pub(super) fn send_item(model: &Model, index: usize) -> Option<&CursorItem> {
    model.mail_send_items.get(index).and_then(Option::as_ref)
}

/// An inbox attachment for the tooltip: `slot` `None` is the first filled one.
pub(super) fn inbox_attachment(
    model: &Model,
    index: usize,
    slot: Option<u32>,
) -> Option<MailAttachmentView> {
    row(model, index).and_then(|r| r.attachment(slot)).cloned()
}

impl super::UiScript {
    /// Push the inbox.
    pub fn set_mail(&mut self, state: MailState) {
        set_field!(self, mail, state);
    }

    /// Push `GetLatestThreeSenders`' answer: one entry per delivered sender, `None` while its name
    /// is in flight, which the reference pushes as nil (`0x56f3e7`).
    pub fn set_mail_latest_senders(&mut self, senders: Vec<Option<String>>) {
        set_field!(self, mail_latest_senders, senders);
    }

    /// Drain the queued mail packets, in call order.
    pub fn take_mail_commands(&mut self) -> Vec<MailCommand> {
        drain_field!(self, mail_commands)
    }

    /// The mailbox opener (`0x56da60`) and `SMSG_SHOW_MAILBOX` (`0x56dbc0`): the mailbox opens, the
    /// form resets with its events, then `MAIL_SHOW`.
    pub fn open_mailbox(&mut self) {
        {
            let mut model = self.model_mut();
            model.mail_mailbox = true;
            clear_send_mail(&mut model, true);
        }
        self.fire_pending_now();
        self.fire_event("MAIL_SHOW", vec![]);
    }

    /// `CloseMail(false)` (`0x571730`) from the engine, as the interaction range does: the form
    /// clears silently and an open mailbox fires `MAIL_CLOSED`, queueing [`MailCommand::Close`].
    pub fn close_mailbox(&mut self) {
        close_mail(&mut self.model_mut());
    }

    /// The mail state's world entry (`0x571530`): no mailbox, an empty form, nothing pending, and
    /// the inbox throttle reset.
    pub fn mail_enter_world(&mut self) {
        let mut model = self.model_mut();
        for item in model.mail_send_items.iter_mut() {
            *item = None;
        }
        model.mail_mailbox = false;
        model.mail_send_money = 0;
        model.mail_send_cod = 0;
        model.mail_lock_send_items = false;
        model.mail_send_pending = false;
        model.mail_command_pending = false;
        model.mail_stationery = 0;
        model.mail_package = 0;
        model.mail_next_update = None;
        model.mail_send_showing = false;
    }

    /// A mail result ended the command in flight; `send` also ends a pending send (`0x571954`).
    pub fn mail_command_done(&mut self, send: bool) {
        let mut model = self.model_mut();
        model.mail_command_pending = false;
        if send {
            model.mail_send_pending = false;
        }
    }

    /// A successful send's form reset (`0x571856`, `ClearSendMail(true)`).
    pub fn mail_sent(&mut self) {
        clear_send_mail(&mut self.model_mut(), true);
    }

    /// `SendMail`'s abort when an attachment is gone at send time (`0x570a7c`): the slot clears
    /// and `MAIL_SEND_INFO_UPDATE` fires.
    pub fn drop_mail_send_item(&mut self, send_slot: u8) {
        let mut model = self.model_mut();
        if let Some(s) = model.mail_send_items.get_mut(usize::from(send_slot)) {
            if s.take().is_some() {
                queue(&mut model, "MAIL_SEND_INFO_UPDATE", vec![]);
            }
        }
    }

    /// Push the usable stationery list, in the picker's order.
    pub fn set_mail_stationeries(&mut self, list: Vec<StationeryView>) {
        set_field!(self, mail_stationeries, list);
    }

    /// Push `Package.dbc`'s rows, in file order.
    pub fn set_mail_packages(&mut self, packages: Vec<PackageView>) {
        set_field!(self, mail_packages, packages);
    }

    /// Push `HasNewMail()`'s answer. The app sets it before each `UPDATE_PENDING_MAIL`, on which
    /// the minimap icon re-reads it.
    pub fn set_has_new_mail(&mut self, has: bool) {
        set_field!(self, has_new_mail, has);
    }

    /// Fire the events queued so far, so a following direct fire keeps the reference's order.
    fn fire_pending_now(&mut self) {
        let events = std::mem::take(&mut self.model_mut().pending_events);
        for (event, args) in events {
            self.fire_event(&event, args);
        }
    }
}

/// `CloseMail(false)`: `ClearSendMail(false)`, then with a mailbox open the close event and the
/// app's session end.
fn close_mail(model: &mut Model) {
    clear_send_mail(model, false);
    if model.mail_mailbox {
        model.mail_mailbox = false;
        queue(model, "MAIL_CLOSED", vec![]);
        model.mail_commands.push(MailCommand::Close);
    }
}

/// The inbox row at a 1-based index, as the reference indexes `index - 1` unsigned.
fn row(model: &Model, index: usize) -> Option<&MailInboxRow> {
    index.checked_sub(1).and_then(|i| model.mail.inbox.get(i))
}

fn row_mut(model: &mut Model, index: usize) -> Option<&mut MailInboxRow> {
    index
        .checked_sub(1)
        .and_then(|i| model.mail.inbox.get_mut(i))
}

/// A 1-based index argument, raising `usage` unless it is a number (`lua_isnumber`).
fn index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<usize> {
    let n = number_arg(lua, v, usage)?;
    Ok(usize::try_from(n).unwrap_or(0))
}

/// An optional 1-based attachment index: a number names slot `n - 1`, anything else the first
/// filled slot (`0x56ed32`).
fn attach_arg(lua: &Lua, v: Value) -> mlua::Result<Option<u32>> {
    Ok(lua
        .coerce_number(v)?
        .map(|n| super::binding_abi::ftol(n - 1.0) as u32))
}

fn opt_str(lua: &Lua, s: &Option<String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(v) => Value::String(lua.create_string(v)?),
        None => Value::Nil,
    })
}

fn values(v: Vec<Value>) -> MultiValue {
    MultiValue::from_vec(v)
}

/// Register the mail globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetInboxNumItems() (`0x56d6d0`): the shown rows and the server's total.
    g.set(
        "GetInboxNumItems",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok((model.mail.inbox.len() as i64, i64::from(model.mail.total)))
        })?,
    )?;

    // GetInboxHeaderInfo(index) (`0x56e520`): fourteen values on every path.
    g.set(
        "GetInboxHeaderInfo",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetInboxHeaderInfo(index)")?;
            let r = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                row(&model, index).cloned()
            };
            let Some(r) = r else {
                let mut miss = vec![Value::Nil; 4];
                miss.extend([Value::Integer(0), Value::Integer(0), Value::Integer(0)]);
                miss.extend(std::iter::repeat_n(Value::Nil, 7));
                return Ok(values(miss));
            };
            let n = r.num_attachments();
            let first_quantity = match (n, r.first_attachment()) {
                (1, Some(a)) => Value::Integer(i64::from(a.count)),
                _ => Value::Nil,
            };
            let player = r.message_type == 0 && !r.is_gm;
            Ok(values(vec![
                opt_str(lua, &r.package_icon)?,
                opt_str(lua, &r.stationery_icon)?,
                opt_str(lua, &r.sender)?,
                Value::String(lua.create_string(&r.subject)?),
                Value::Integer(i64::from(r.money)),
                Value::Integer(i64::from(r.cod)),
                Value::Number(f64::from(r.days_left)),
                if n > 0 {
                    Value::Integer(n as i64)
                } else {
                    Value::Nil
                },
                flag(r.flags & flags::READ != 0),
                flag(r.flags & flags::SHOWN_RETURNED != 0),
                flag(r.flags & flags::COPIED != 0),
                flag(player),
                flag(r.is_gm),
                first_quantity,
            ]))
        })?,
    )?;

    // GetInboxText(index) (`0x570bd0`): marks the mail read, then the body, the stationery
    // texture, isTakeable and isInvoice; four nils past the inbox.
    g.set(
        "GetInboxText",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetInboxText(index)")?;
            let r = {
                let mut model = crate::script::model_mut_of(lua);
                mark_as_read(&mut model, index);
                row(&model, index).cloned()
            };
            let Some(r) = r else {
                return Ok(values(vec![Value::Nil; 4]));
            };
            Ok(values(vec![
                opt_str(lua, &r.body)?,
                opt_str(lua, &r.stationery_texture)?,
                flag(r.takeable),
                flag(r.is_invoice),
            ]))
        })?,
    )?;

    // GetInboxInvoiceInfo(index) (`0x56e9b0`): ten values, or a seven-value miss of three nils
    // and four zeros.
    g.set(
        "GetInboxInvoiceInfo",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetInboxInvoiceInfo(index)")?;
            let inv = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                row(&model, index).and_then(|r| r.invoice.clone())
            };
            let Some(inv) = inv else {
                let mut miss = vec![Value::Nil; 3];
                miss.extend(std::iter::repeat_n(Value::Integer(0), 4));
                return Ok(values(miss));
            };
            Ok(values(vec![
                Value::String(lua.create_string(inv.kind)?),
                Value::String(lua.create_string(&inv.item_name)?),
                Value::String(lua.create_string(&inv.player_name)?),
                Value::Integer(i64::from(inv.bid)),
                Value::Integer(i64::from(inv.buyout)),
                Value::Integer(i64::from(inv.deposit)),
                Value::Integer(i64::from(inv.consignment)),
                Value::Integer(i64::from(inv.money_delay)),
                Value::Integer(i64::from(inv.eta_hour)),
                Value::Integer(i64::from(inv.eta_minute)),
            ]))
        })?,
    )?;

    // GetInboxItem(messageIndex, attachIndex) (`0x570f10`): name, texture, count, quality, canUse.
    g.set(
        "GetInboxItem",
        lua.create_function(|lua, (index, attach): (Value, Value)| {
            let index = index_arg(lua, index, "Usage: GetInboxItem(messageIndex, attachIndex)")?;
            let slot = attach_arg(lua, attach)?;
            // `canUse` is the shared item-usable gate (`0x5710ea`).
            let a = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                row(&model, index)
                    .and_then(|r| r.attachment(slot))
                    .cloned()
                    .map(|a| {
                        let usable = super::item_stats::item_usable_by_id(&model, a.item_id);
                        (a, usable)
                    })
            };
            let Some((a, usable)) = a.filter(|(a, _)| a.item_id != 0) else {
                return Ok(values(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Nil,
                ]));
            };
            let Some(name) = &a.name else {
                // The template is in flight: the unknown object and the question mark.
                let unknown = lua
                    .globals()
                    .get::<Option<String>>("UNKNOWNOBJECT")?
                    .unwrap_or_default();
                return Ok(values(vec![
                    Value::String(lua.create_string(&unknown)?),
                    Value::String(lua.create_string("Interface\\Icons\\INV_Misc_QuestionMark")?),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Nil,
                ]));
            };
            Ok(values(vec![
                Value::String(lua.create_string(name)?),
                opt_str(lua, &a.texture)?,
                Value::Integer(i64::from(a.count)),
                Value::Integer(i64::from(a.quality)),
                flag(usable),
            ]))
        })?,
    )?;

    // GetInboxItemLink(messageIndex, attachIndex) (`0x56eca0`): the link, or no value.
    g.set(
        "GetInboxItemLink",
        lua.create_function(|lua, (index, attach): (Value, Value)| {
            let index = index_arg(
                lua,
                index,
                "Usage: GetInboxItemLink(messageIndex, attachIndex)",
            )?;
            let slot = attach_arg(lua, attach)?;
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                row(&model, index)
                    .and_then(|r| r.attachment(slot))
                    .and_then(|a| a.link.clone())
            };
            Ok(match link {
                Some(l) => values(vec![Value::String(lua.create_string(&l)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // InboxItemCanDelete(index) (`0x56f280`).
    g.set(
        "InboxItemCanDelete",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: InboxItemCanDelete(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(
                row(&model, index).is_some_and(MailInboxRow::can_delete),
            ))
        })?,
    )?;

    // CanComplainInboxItem(index) (`0x5713c0`), raising under its old name.
    g.set(
        "CanComplainInboxItem",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: InboxItemCanComplain(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(row(&model, index).is_some_and(|r| r.can_complain)))
        })?,
    )?;

    // ComplainInboxItem(index) (`0x571350`): `Complain` (`0x56faf0`) returns while the chat
    // complaint system's status is off, its value until `SMSG_FEATURE_SYSTEM_STATUS`, which is
    // not modelled.
    g.set(
        "ComplainInboxItem",
        lua.create_function(|lua, index: Value| {
            index_arg(lua, index, "Usage: ComplainInboxItem(index)")?;
            Ok(())
        })?,
    )?;

    g.set(
        "HasNewMail",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.has_new_mail))
        })?,
    )?;

    // GetLatestThreeSenders() (`0x56f3a0`): one value per delivered sender, nil while its name
    // is in flight.
    g.set(
        "GetLatestThreeSenders",
        lua.create_function(|lua, ()| {
            let senders = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .mail_latest_senders
                .clone();
            senders
                .iter()
                .map(|s| opt_str(lua, s))
                .collect::<mlua::Result<Vec<_>>>()
                .map(values)
        })?,
    )?;

    // CheckInbox() (`0x56e440`): with no command in flight and a mailbox open, the list query, at
    // most once a minute; a throttled call fires `MAIL_INBOX_UPDATE` on the cached rows.
    g.set(
        "CheckInbox",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox {
                return Ok(());
            }
            let now = model.now;
            if model.mail_next_update.is_some_and(|t| now < t) {
                queue(&mut model, "MAIL_INBOX_UPDATE", vec![]);
                return Ok(());
            }
            model.mail_next_update = Some(now + CHECK_INBOX_THROTTLE_SECS);
            model.mail_command_pending = true;
            model.mail_commands.push(MailCommand::GetList);
            Ok(())
        })?,
    )?;

    // TakeInboxMoney(index) (`0x56edf0`): a mail with money.
    g.set(
        "TakeInboxMoney",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: TakeInboxMoney(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox {
                return Ok(());
            }
            if let Some(id) = row(&model, index)
                .filter(|r| r.money != 0)
                .map(|r| r.message_id)
            {
                model
                    .mail_commands
                    .push(MailCommand::TakeMoney { message_id: id });
                model.mail_command_pending = true;
            }
            Ok(())
        })?,
    )?;

    // TakeInboxTextItem(index) (`0x56eef0`): a letter not yet copied.
    g.set(
        "TakeInboxTextItem",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: TakeInboxTextItem(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox {
                return Ok(());
            }
            if let Some(id) = row(&model, index)
                .filter(|r| r.has_letter && r.flags & flags::COPIED == 0)
                .map(|r| r.message_id)
            {
                model
                    .mail_commands
                    .push(MailCommand::TextItem { message_id: id });
                model.mail_command_pending = true;
            }
            Ok(())
        })?,
    )?;

    // ReturnInboxItem(index) (`0x56f000`): a player's returnable mail with something in it.
    g.set(
        "ReturnInboxItem",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: ReturnInboxItem(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox {
                return Ok(());
            }
            let Some(r) = row_mut(&mut model, index) else {
                return Ok(());
            };
            if r.delete_pending
                || r.message_type != 0
                || (r.num_attachments() == 0 && r.money == 0)
                || r.flags & flags::UNRETURNABLE != 0
            {
                return Ok(());
            }
            r.delete_pending = true;
            let id = r.message_id;
            model
                .mail_commands
                .push(MailCommand::Return { message_id: id });
            model.mail_command_pending = true;
            Ok(())
        })?,
    )?;

    // DeleteInboxItem(index) (`0x56f140`): a deletable mail.
    g.set(
        "DeleteInboxItem",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: DeleteInboxItem(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox {
                return Ok(());
            }
            let Some(r) = row_mut(&mut model, index) else {
                return Ok(());
            };
            if r.delete_pending || !r.can_delete() {
                return Ok(());
            }
            r.delete_pending = true;
            let id = r.message_id;
            model
                .mail_commands
                .push(MailCommand::Delete { message_id: id });
            model.mail_command_pending = true;
            Ok(())
        })?,
    )?;

    // TakeInboxItem(messageIndex, attachIndex) (`0x5711c0`): one attachment, refused with
    // `ERR_NOT_ENOUGH_MONEY` when the purse cannot pay the COD.
    g.set(
        "TakeInboxItem",
        lua.create_function(|lua, (index, attach): (Value, Value)| {
            let index = index_arg(
                lua,
                index,
                "Usage: TakeInboxItem(messageIndex, attachIndex)",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox {
                return Ok(());
            }
            let slot = attach_arg(lua, attach)?;
            let Some((id, cod, guid)) = row(&model, index).and_then(|r| {
                r.attachment(slot)
                    .filter(|a| a.item_id != 0)
                    .map(|a| (r.message_id, r.cod, a.item_guid_low))
            }) else {
                return Ok(());
            };
            if model.money < u64::from(cod) {
                model.ui_errors.push("ERR_NOT_ENOUGH_MONEY");
                return Ok(());
            }
            model.mail_commands.push(MailCommand::TakeItem {
                message_id: id,
                item_guid_low: guid,
                auto_loot: false,
            });
            model.mail_command_pending = true;
            Ok(())
        })?,
    )?;

    // AutoLootMailItem(index) (`0x571450`): the money, then the first attachment; the app takes
    // the rest one result at a time (`0x570410`, `0x570270`).
    g.set(
        "AutoLootMailItem",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: AutoLootMailItem(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox {
                return Ok(());
            }
            let Some(r) = row(&model, index).cloned() else {
                return Ok(());
            };
            if r.money != 0 {
                model.mail_commands.push(MailCommand::TakeMoney {
                    message_id: r.message_id,
                });
            }
            if model.money < u64::from(r.cod) {
                model.ui_errors.push("ERR_NOT_ENOUGH_MONEY");
                return Ok(());
            }
            let mut filled = r.attachments.iter().flatten().filter(|a| a.item_id != 0);
            if let Some(a) = filled.next() {
                model.mail_commands.push(MailCommand::TakeItem {
                    message_id: r.message_id,
                    item_guid_low: a.item_guid_low,
                    auto_loot: filled.next().is_some(),
                });
                model.mail_command_pending = true;
            }
            Ok(())
        })?,
    )?;

    // CloseMail() (`0x571a40`).
    g.set(
        "CloseMail",
        lua.create_function(|lua, ()| {
            close_mail(&mut crate::script::model_mut_of(lua));
            Ok(())
        })?,
    )?;

    // SendMail(target, subject, body) (`0x570910`): silent unless a mailbox is open with nothing
    // in flight, a target, a subject and a stationery; money with COD, or COD with no item, sends
    // nothing either.
    g.set(
        "SendMail",
        lua.create_function(|lua, (target, subject, body): (Value, Value, Value)| {
            let target = super::binding_abi::string_arg(
                lua,
                target,
                "Usage: SendMail(target, subject, body)",
            )?;
            let mut model = crate::script::model_mut_of(lua);
            if model.mail_command_pending || !model.mail_mailbox || model.mail_send_pending {
                return Ok(());
            }
            let Some(subject) = super::binding_abi::optional_string(lua, &subject) else {
                return Ok(());
            };
            if target.is_empty() || subject.is_empty() {
                return Ok(());
            }
            let body = super::binding_abi::optional_string(lua, &body).unwrap_or_default();
            if model.mail_stationery == 0 {
                return Ok(());
            }
            // Each attachment must still be the player's, else the slot drops (`0x570a7c`).
            let mut items = Vec::new();
            for i in 0..SEND_ARRAY {
                let Some(it) = model.mail_send_items[i].clone() else {
                    continue;
                };
                let present = model
                    .containers
                    .get(&it.bag)
                    .and_then(|c| c.slots.get(&it.slot))
                    .is_some_and(|s| s.item_id == it.item_id);
                if !present {
                    model.ui_errors.push("ERR_ITEM_NOT_FOUND");
                    model.mail_send_items[i] = None;
                    queue(&mut model, "MAIL_SEND_INFO_UPDATE", vec![]);
                    return Ok(());
                }
                items.push((i as u8, it.bag, it.slot));
            }
            let (money, cod) = (model.mail_send_money, model.mail_send_cod);
            if (money != 0 && cod != 0) || (money == 0 && cod != 0 && items.is_empty()) {
                return Ok(());
            }
            let request = MailSendRequest {
                target,
                subject,
                body,
                stationery: model.mail_stationery,
                package: if items.is_empty() {
                    0
                } else {
                    model.mail_package
                },
                money,
                cod,
                items,
            };
            model.mail_commands.push(MailCommand::Send(request));
            model.mail_send_pending = true;
            model.mail_command_pending = true;
            Ok(())
        })?,
    )?;

    // SetSendMailMoney(copper) (`0x5706c0`, `0x56fda0`): within the purse it stores, fires
    // `SEND_MAIL_MONEY_CHANGED` and answers 1; past it, `ERR_NOT_ENOUGH_MONEY` and nil.
    g.set(
        "SetSendMailMoney",
        lua.create_function(|lua, copper: Value| {
            let n = number_arg(lua, copper, "Usage: SetSendMailMoney(amount)")? as u32;
            let mut model = crate::script::model_mut_of(lua);
            if u64::from(n) > model.money {
                model.ui_errors.push("ERR_NOT_ENOUGH_MONEY");
                return Ok(Value::Nil);
            }
            model.mail_send_money = n;
            queue(&mut model, "SEND_MAIL_MONEY_CHANGED", vec![]);
            Ok(Value::Integer(1))
        })?,
    )?;

    g.set(
        "GetSendMailMoney",
        lua.create_function(|lua, ()| {
            Ok(i64::from(
                lua.app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data")
                    .mail_send_money,
            ))
        })?,
    )?;
    g.set(
        "GetSendMailCOD",
        lua.create_function(|lua, ()| {
            Ok(i64::from(
                lua.app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data")
                    .mail_send_cod,
            ))
        })?,
    )?;

    // SetSendMailCOD(copper) (`0x56d610`): stored only with an attachment.
    g.set(
        "SetSendMailCOD",
        lua.create_function(|lua, copper: Value| {
            let n = lua
                .coerce_number(copper)?
                .ok_or_else(|| mlua::Error::RuntimeError("Usage: SetSendMailCOD(amount)".into()))?;
            let mut model = crate::script::model_mut_of(lua);
            if num_send_items(&model) != 0 {
                model.mail_send_cod = n as i64 as u32;
                queue(&mut model, "SEND_MAIL_COD_CHANGED", vec![]);
            }
            Ok(())
        })?,
    )?;

    // GetSendMailItem(index) (`0x570750`): name, texture, stackCount, quality; `nil, nil, 0, 0`
    // for an empty or out-of-range slot.
    g.set(
        "GetSendMailItem",
        lua.create_function(|lua, index: Value| {
            let slot = send_slot_arg(lua, index)?;
            let item = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                slot.and_then(|s| send_item(&model, s)).cloned().map(|it| {
                    let count = cursor::held_count(&model, &it);
                    let inv_type = model
                        .item_templates
                        .get(&it.item_id)
                        .map(|t| t.inventory_type);
                    (it, count, inv_type)
                })
            };
            let Some((it, count, inv_type)) = item else {
                return Ok(values(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            let name = cursor::item_link_name(it.link.as_deref());
            // An item whose `InventoryType` is 0 answers quality -1 (`0x5708be`).
            let quality = match inv_type {
                Some(0) => -1,
                _ => i64::from(it.quality.unwrap_or(0)),
            };
            Ok(values(vec![
                if name.is_empty() {
                    Value::Nil
                } else {
                    Value::String(lua.create_string(&name)?)
                },
                opt_str(lua, &it.texture)?,
                Value::Integer(i64::from(count)),
                Value::Integer(quality),
            ]))
        })?,
    )?;

    // GetSendMailItemLink(index) (`0x56e3a0`): the attachment's link, or no value.
    g.set(
        "GetSendMailItemLink",
        lua.create_function(|lua, index: Value| {
            let slot = send_slot_arg(lua, index)?;
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                slot.and_then(|s| send_item(&model, s))
                    .and_then(|it| it.link.clone())
            };
            Ok(match link {
                Some(l) => values(vec![Value::String(lua.create_string(&l)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    // GetSendMailPrice() (`0x571b00`): 30 per attachment (30 with none), an uncarried
    // stationery's price, the package's cost and the enclosed money.
    g.set(
        "GetSendMailPrice",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let mut price = POSTAGE_PER_ITEM * (num_send_items(&model).max(1) as u32);
            if model.mail_stationery != 0 {
                if let Some(cost) = model
                    .mail_stationeries
                    .iter()
                    .find(|s| s.id == model.mail_stationery)
                    .and_then(|s| s.cost)
                {
                    price += cost;
                }
            }
            if model.mail_package != 0 {
                if let Some(p) = model
                    .mail_packages
                    .iter()
                    .find(|p| p.id == model.mail_package)
                {
                    price = price.wrapping_add(p.cost as u32);
                }
            }
            Ok(i64::from(price.wrapping_add(model.mail_send_money)))
        })?,
    )?;

    // ClickSendMailItemButton(index, clearItem) (`0x570550`).
    g.set(
        "ClickSendMailItemButton",
        lua.create_function(|lua, (index, clear): (Value, Value)| {
            let slot = lua
                .coerce_number(index)?
                .map(|n| super::binding_abi::ftol(n - 1.0) as u32 as usize);
            let clear = super::binding_abi::bool_or_default(Some(&clear), false);
            let mut model = crate::script::model_mut_of(lua);
            click_send_mail_item(&mut model, slot, clear);
            Ok(())
        })?,
    )?;

    // SetSendMailShowing(bool) (`0x56d570`): the Send tab is up, so a bag click attaches.
    g.set(
        "SetSendMailShowing",
        lua.create_function(|lua, showing: Value| {
            let Value::Boolean(showing) = showing else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SetSendMailShowing(bool)".into(),
                ));
            };
            crate::script::model_mut_of(lua).mail_send_showing = showing;
            Ok(())
        })?,
    )?;

    // RespondMailLockSendItem(slot, keepItem) (`0x56f410`): answers the refund-lock prompt.
    g.set(
        "RespondMailLockSendItem",
        lua.create_function(|lua, (slot, keep): (Value, Value)| {
            let slot = number_arg(lua, slot, "Usage: RespondMailLockSendItem(slot, keepItem)")?;
            let keep = super::binding_abi::bool_or_default(Some(&keep), false);
            let mut model = crate::script::model_mut_of(lua);
            if !model.mail_lock_send_items {
                return Ok(());
            }
            model.mail_lock_send_items = false;
            if !keep {
                let taken = usize::try_from(slot - 1)
                    .ok()
                    .filter(|&s| s < MAIL_SEND_SLOTS)
                    .and_then(|s| model.mail_send_items[s].take());
                if let Some(it) = taken {
                    cursor::queue_lock_changed(&mut model, it.bag, it.slot);
                }
                queue(&mut model, "MAIL_SEND_INFO_UPDATE", vec![]);
            }
            queue(&mut model, "MAIL_UNLOCK_SEND_ITEMS", vec![]);
            Ok(())
        })?,
    )?;

    // ── The stationery family ──

    g.set(
        "GetNumStationeries",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.mail_stationeries.len() as i64)
        })?,
    )?;

    // GetStationeryInfo(index) (`0x56df80`): name, icon, cost; three nils out of range.
    g.set(
        "GetStationeryInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetStationeryInfo(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let row = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.mail_stationeries.get(i));
            let Some(row) = row else {
                return Ok(values(vec![Value::Nil, Value::Nil, Value::Nil]));
            };
            Ok(values(vec![
                Value::String(lua.create_string(&row.name)?),
                Value::String(lua.create_string(&row.icon)?),
                row.cost
                    .map_or(Value::Nil, |c| Value::Integer(i64::from(c))),
            ]))
        })?,
    )?;

    // SelectStationery(index) (`0x56e0e0`): out of range stores 0.
    g.set(
        "SelectStationery",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SelectStationery(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            model.mail_stationery = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.mail_stationeries.get(i))
                .map_or(0, |row| row.id);
            Ok(())
        })?,
    )?;

    // GetSelectedStationeryTexture() (`0x56e170`): nil for no selection or an unknown id.
    g.set(
        "GetSelectedStationeryTexture",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let id = model.mail_stationery;
            let tex = (id != 0)
                .then(|| model.mail_stationeries.iter().find(|r| r.id == id))
                .flatten();
            Ok(match tex {
                Some(row) => Value::String(lua.create_string(&row.texture)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // ClearSendMail() (`0x56d5c0`): the form reset with its events; zero returns.
    g.set(
        "ClearSendMail",
        lua.create_function(|lua, ()| {
            clear_send_mail(&mut crate::script::model_mut_of(lua), true);
            Ok(MultiValue::new())
        })?,
    )?;

    // GetNumPackages() (`0x56e1e0`): `Package.dbc`'s row count.
    g.set(
        "GetNumPackages",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.mail_packages.len() as i64)
        })?,
    )?;

    // GetPackageInfo(index) (`0x56e210`): name, icon, cost; `nil, nil, 0` out of range.
    g.set(
        "GetPackageInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetPackageInfo(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let row = usize::try_from(index.wrapping_sub(1))
                .ok()
                .and_then(|i| model.mail_packages.get(i));
            Ok(match row {
                Some(row) => values(vec![
                    Value::String(lua.create_string(&row.name)?),
                    Value::String(lua.create_string(&row.icon)?),
                    Value::Integer(i64::from(row.cost)),
                ]),
                None => values(vec![Value::Nil, Value::Nil, Value::Integer(0)]),
            })
        })?,
    )?;

    // SelectPackage(index) (`0x56e310`): the row's id, or 0 out of range; zero returns.
    g.set(
        "SelectPackage",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SelectPackage(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            model.mail_package = usize::try_from(index.wrapping_sub(1))
                .ok()
                .and_then(|i| model.mail_packages.get(i))
                .map_or(0, |row| row.id);
            Ok(MultiValue::new())
        })?,
    )?;

    Ok(())
}

/// A send-slot argument: a number names slot `n - 1`, `None` past the array; anything else slot 0
/// (`0x570762`).
fn send_slot_arg(lua: &Lua, v: Value) -> mlua::Result<Option<usize>> {
    Ok(match lua.coerce_number(v)? {
        Some(n) => {
            let i = super::binding_abi::ftol(n - 1.0) as u32 as usize;
            (i < SEND_ARRAY).then_some(i)
        }
        None => Some(0),
    })
}

/// The mark-as-read (`0x56fe30`): an unread row turns read locally and queues the packet; a read
/// one, or none, does nothing.
fn mark_as_read(model: &mut Model, index: usize) {
    let Some(r) = row_mut(model, index).filter(|r| r.flags & flags::READ == 0) else {
        return;
    };
    r.flags |= flags::READ;
    let id = r.message_id;
    model
        .mail_commands
        .push(MailCommand::MarkRead { message_id: id });
}

/// `ClickSendMailItemButton` (`0x570550`). Without `clear`, a held item attaches to the slot (or
/// the first free one) and the slot's old item, if any, goes to the cursor; with no held item the
/// slot's item goes to the cursor. With `clear` the slot just empties and unlocks.
fn click_send_mail_item(model: &mut Model, slot: Option<usize>, clear: bool) {
    if clear {
        let taken = slot
            .filter(|&s| s < SEND_ARRAY)
            .and_then(|s| model.mail_send_items[s].take());
        if let Some(it) = taken {
            cursor::queue_lock_changed(model, it.bag, it.slot);
            queue(model, "MAIL_SEND_INFO_UPDATE", vec![]);
        }
        return;
    }
    // Only an item counts as held (`GetCursorItem`); any other payload reads as none.
    let held = match &model.cursor {
        Some(CursorPayload::Item(it)) => Some(it.clone()),
        _ => None,
    };
    let prior = slot.and_then(|s| send_item(model, s)).cloned();
    if let (Some(h), Some(p)) = (&held, &prior) {
        if h.bag == p.bag && h.slot == p.slot {
            // The slot's own item: the cursor just drops it (`0x570640`).
            cursor::clear_cursor(model);
            return;
        }
    }
    match held {
        Some(h) => {
            // The held item leaves the cursor for the slot.
            model.cursor = None;
            if !add_item_to_outgoing(model, slot, h.clone()) {
                model.cursor = Some(CursorPayload::Item(h));
                return;
            }
        }
        None => {
            if let Some(s) = slot.filter(|&s| s < MAIL_SEND_SLOTS) {
                model.mail_send_items[s] = None;
            }
        }
    }
    // The slot's old item, if any, goes onto the cursor (`0x570696`).
    model.cursor = prior.map(CursorPayload::Item);
    cursor::queue_cursor_update(model);
    queue(model, "MAIL_SEND_INFO_UPDATE", vec![]);
}

#[cfg(test)]
mod tests;
