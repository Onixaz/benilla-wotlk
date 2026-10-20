//! The mail feed: the app half of [`benilla_ui::script`]'s `mail` module, 12340's `CGMailInfo`.
//!
//! A mailbox opens locally, from a mailbox GameObject's use (`0x70c9d0`) or a mailbox NPC's
//! interaction (`0x6de11f`), or by `SMSG_SHOW_MAILBOX`; each ends in `TalkToMailbox`
//! (`0x56da60`), which resets the form and fires `MAIL_SHOW`, whose `CheckInbox()` asks for the
//! list. The VM gates the verbs and queues their packets; this side sends them, keeps the wire
//! rows and applies each result as the reference's handlers do.
//!
//! [`MailPending`] is the arrival layer behind `HasNewMail()`, the minimap icon and its sender
//! tooltip; it never reads the inbox, only the next-mail time and its three senders.

use benilla_protocol::messages::{
    mail_flags, mail_message_type, MailAttachment, MailListEntry, MAIL_ATTACHMENTS_MAX,
};
use bevy::prelude::*;

use benilla_ui::script::{
    MailAttachmentView, MailInboxRow, MailInvoice, MailState, ScriptValue, StationeryView,
    UiScript, STATIONERY_GM,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_items::{EquipError, EquipErrors};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

mod invoice;
mod net;
mod pending;

pub(crate) use pending::MailPending;

/// The item whose icon a mail with several attachments shows (`0x56e63c`, display 7913).
const MULTI_ITEM_DISPLAY: u32 = 7913;

/// The `Stationery.dbc` catalog; without it every mail gets no stationery texture.
#[derive(Resource)]
pub(crate) struct Stationery(pub(crate) benilla_formats::StationeryCatalog);

/// `Package.dbc`'s rows, in file order; without them `GetNumPackages()` answers 0.
#[derive(Resource)]
pub(crate) struct Packages(pub(crate) Vec<benilla_formats::PackageRow>);

/// A package row as the engine answers it: the icon under `StringLookups.dbc` row 3's folder
/// (`0x56e27f`).
fn package_view(row: &benilla_formats::PackageRow) -> benilla_ui::script::PackageView {
    benilla_ui::script::PackageView {
        id: row.id,
        name: row.name.clone(),
        icon: format!("Interface\\Icons\\{}", row.icon),
        cost: row.cost,
    }
}

/// One inbox row: the wire row, its attachments filed by slot, and the client's own mark.
pub(crate) struct MailRow {
    pub(crate) entry: MailListEntry,
    /// Sixteen slots; the list handler files each attachment as `0x571e29` does.
    pub(crate) slots: Vec<Option<MailAttachment>>,
    /// A return or delete is in flight (`+0x2940`).
    pub(crate) delete_pending: bool,
}

impl MailRow {
    /// Files the wire's attachments: a slot past 15 reads 0, and a slot already taken moves to the
    /// first free one; with none free the attachment is dropped (`0x571e31`-`0x571e81`).
    pub(crate) fn new(mut entry: MailListEntry) -> Self {
        let mut slots: Vec<Option<MailAttachment>> = vec![None; MAIL_ATTACHMENTS_MAX];
        for a in std::mem::take(&mut entry.attachments) {
            let wanted = usize::from(a.slot);
            let wanted = if wanted < MAIL_ATTACHMENTS_MAX {
                wanted
            } else {
                0
            };
            let slot = if slots[wanted].is_none() {
                Some(wanted)
            } else {
                slots.iter().position(Option::is_none)
            };
            if let Some(s) = slot {
                slots[s] = Some(a);
            }
        }
        Self {
            entry,
            slots,
            delete_pending: false,
        }
    }

    /// `GetNumAttachments` (`0x56d240`): slots holding an item.
    pub(crate) fn num_attachments(&self) -> usize {
        self.slots.iter().flatten().filter(|a| a.entry != 0).count()
    }

    /// A player sender's guid: a player mail's own, a calendar notice's from its subject
    /// (`0x572102`).
    fn sender_guid(&self) -> u64 {
        match self.entry.message_type {
            mail_message_type::CALENDAR => leading_u64(&self.entry.subject),
            _ => self.entry.sender_guid,
        }
    }

    /// The purge `RemoveMoney` (`0x56f770`) and `RemoveItem` (`0x56f8d0`) test after a take: an
    /// auction notice, a COD payment, or a letterless mail, once nothing is left in it.
    fn empty_after_take(&self) -> bool {
        let e = &self.entry;
        (e.message_type == mail_message_type::AUCTION
            || e.flags & mail_flags::COD_PAYMENT != 0
            || (e.body.is_empty() && e.mail_template_id == 0))
            && !self.delete_pending
            && e.money == 0
            && self.num_attachments() == 0
    }
}

/// `SStrToInt64`: the leading decimal digits.
fn leading_u64(s: &str) -> u64 {
    s.trim_start()
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0u64, |n, b| {
            n.wrapping_mul(10).wrapping_add(u64::from(b - b'0'))
        })
}

/// One step of a result's or list's effect on the window, applied by [`feed_mail`] in order.
pub(crate) enum MailEffect {
    /// A message line by key.
    Message(&'static str),
    /// An inventory error, through the equip-error table.
    EquipError(u32),
    /// A sent mail's form reset (`ClearSendMail(true)`).
    Sent,
    /// `CLOSE_INBOX_ITEM(index)`, 1-based.
    CloseInboxItem(u32),
    /// The rows changed: push them and fire `MAIL_INBOX_UPDATE`.
    InboxUpdate,
    /// The command in flight ended; `send` also ends a pending send.
    Done { send: bool },
    /// `MAIL_SUCCESS` on an OK result, else `MAIL_FAILED` (`0x571984`).
    Result { ok: bool },
}

/// The mail session: the open mailbox, the inbox rows (kept past a close, as the reference keeps
/// them until a world change, `0x571730`), and the queued effects.
#[derive(Resource, Default)]
pub(crate) struct MailOpen {
    /// The open mailbox (`m_object`).
    pub(crate) mailbox: Option<u64>,
    /// Wire order, which is the 1-based display order.
    pub(crate) rows: Vec<MailRow>,
    /// The server's whole count past the shown rows.
    pub(crate) total: u32,
    /// The mail `AutoLootMailItem` is emptying one attachment per result (`m_autoLooting`).
    auto_looting: Option<u32>,
    /// `TalkToMailbox` owed to the VM.
    open_requested: bool,
    /// The engine closed the window (range, despawn); the VM is owed `CloseMail(false)`.
    close_requested: bool,
    pub(crate) effects: Vec<MailEffect>,
    /// The first usable stationery's id, which a row without one takes (`0x5720c3`).
    default_stationery: u32,
}

impl MailOpen {
    /// `TalkToMailbox` (`0x56da60`): a mailbox right-click, a mailbox NPC or `SMSG_SHOW_MAILBOX`.
    pub(crate) fn click(&mut self, mailbox: u64) {
        self.mailbox = Some(mailbox);
        self.open_requested = true;
    }

    pub(crate) fn index_of(&self, message_id: u32) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.entry.message_id == message_id)
    }

    /// The session's end: no mailbox and no rows, as `CloseMail(true)` leaves them (`0x571730`).
    pub(crate) fn clear_session(&mut self) {
        *self = Self {
            default_stationery: self.default_stationery,
            ..Self::default()
        };
    }
}

/// The range guard ([`crate::ui_session`]) closes the window past the client's interaction gate;
/// `CloseInteraction` on the mailbox is `CloseMail(false)` (`0x51302d`).
impl NpcSession for MailOpen {
    fn npc(&self) -> Option<u64> {
        self.mailbox
    }

    fn close(&mut self) {
        self.mailbox = None;
        self.auto_looting = None;
        self.close_requested = true;
    }
}

pub(crate) struct UiMailPlugin;

impl Plugin for UiMailPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .add_systems(
                Update,
                (
                    // Range-close, feed, then drain after input, so each edge lands the same
                    // frame; the feed runs after `UnitFeed` so a row's tooltip reads the template.
                    close_npc_session_out_of_range::<MailOpen>.before(feed_mail),
                    feed_mail.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    drain_mail.after(UiInput),
                    // The cascade's order: the time query, this, then the battlefield status.
                    send_query_next_mail_time_on_enter.after(crate::net::send_query_time),
                ),
            );
    }
}

/// `CGMailInfo::EnterWorld` (`0x571530`) at each world-enter cascade, after the time query and
/// before the battlefield status: the senders clear and `MSG_QUERY_NEXT_MAIL_TIME` goes out,
/// stamping "no mail".
pub(crate) fn send_query_next_mail_time_on_enter(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    commands: Res<NetCommands>,
    mut pending: ResMut<MailPending>,
) {
    if cascades.read().next().is_some() {
        *pending = MailPending::default();
        query_next_mail_time(&mut pending, &commands);
    }
}

/// `QueryNextMailTime` (`0x56ff70`).
fn query_next_mail_time(pending: &mut MailPending, commands: &NetCommands) {
    pending.on_query_sent();
    let _ = commands.0.send(ClientCommand::QueryNextMailTime);
}

/// `ReceivedMail` (`0x5704d0`), which `SMSG_RECEIVED_MAIL` and the auction notices call: with a
/// mailbox open it only defers a re-query to the close; otherwise it re-queries and keeps the
/// delay.
pub(crate) fn received_mail(
    delay: f32,
    source: &str,
    pending: &mut MailPending,
    mail: &MailOpen,
    commands: &NetCommands,
) {
    debug!(
        "ui_mail: ReceivedMail({delay}) from {source}, mailbox {}",
        if mail.mailbox.is_some() {
            "open"
        } else {
            "closed"
        }
    );
    if pending.received_mail(delay, mail.mailbox.is_some()) {
        let _ = commands.0.send(ClientCommand::QueryNextMailTime);
    }
}

/// [`received_mail`] for a handler outside the mailbox, as the auction notices are.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MailArrival<'w> {
    pending: ResMut<'w, MailPending>,
    mail: Res<'w, MailOpen>,
    commands: Res<'w, NetCommands>,
}

impl MailArrival<'_> {
    /// `ReceivedMail(delay)` (`0x5704d0`); `source` names the caller in the debug trace.
    pub(crate) fn arrive(&mut self, delay: f32, source: &str) {
        received_mail(delay, source, &mut self.pending, &self.mail, &self.commands);
    }
}

/// The message key a `SMSG_SEND_MAIL_RESULT` error shows, by 12340's switch (`0x5717b0`): codes
/// 6 to 13 and past 21 share `ERR_MAIL_DATABASE_ERROR`, and 20 shows nothing. `OK` shows
/// [`MAIL_SENT_KEY`] on a send and `EQUIP_ERROR` goes through the equip-error table.
pub(crate) fn mail_refusal(error: u32) -> Option<&'static str> {
    use benilla_protocol::messages::mail_error;
    Some(match error {
        mail_error::CANNOT_SEND_TO_SELF => "ERR_MAIL_TO_SELF",
        mail_error::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY",
        mail_error::RECIPIENT_NOT_FOUND => "ERR_MAIL_TARGET_NOT_FOUND",
        mail_error::NOT_YOUR_TEAM => "ERR_PLAYER_WRONG_FACTION",
        mail_error::TRIAL_ACCOUNT => "ERR_RESTRICTED_ACCOUNT",
        mail_error::RECIPIENT_CAP_REACHED => "ERR_MAIL_REACHED_CAP",
        mail_error::CANT_SEND_WRAPPED_COD => "ERR_MAIL_WRAPPED_COD",
        mail_error::MAIL_AND_CHAT_SUSPENDED => "ERR_USER_SQUELCHED",
        mail_error::TOO_MANY_ATTACHMENTS => "ERR_MAIL_TOO_MANY_ATTACHMENTS",
        mail_error::ATTACHMENT_INVALID => "ERR_MAIL_INVALID_ATTACHMENT",
        mail_error::SILENT => return None,
        mail_error::ITEM_HAS_EXPIRED => "ERR_MAIL_ATTACHMENT_EXPIRED",
        // INTERNAL_ERROR (6) and every other code.
        _ => "ERR_MAIL_DATABASE_ERROR",
    })
}

/// The yellow info line a successful send shows before the form reset (`0x57184f`).
pub(crate) const MAIL_SENT_KEY: &str = "ERR_MAIL_SENT";

/// The resources a row's resolution reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MailLookups<'w, 's> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    stationery: Option<Res<'w, Stationery>>,
    packages: Option<Res<'w, Packages>>,
    names: Res<'w, NameCache>,
    go_templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    houses: Option<Res<'w, crate::ui_auction::AuctionHouses>>,
    social: Res<'w, crate::ui_social::SocialState>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    states: Res<'w, crate::world_state::WorldStates>,
    commands: Res<'w, NetCommands>,
    objects: Objects<'w, 's>,
    self_q: Query<'w, 's, (&'static ObjectStore, &'static crate::net::Guid), With<SelfPlayer>>,
}

impl MailLookups<'_, '_> {
    fn rolls(&self) -> crate::items::RollCatalogs<'_> {
        crate::items::RollCatalogs {
            props: self.props.as_deref(),
            enchants: self.enchants.as_deref(),
        }
    }

    fn icon(&self, display_id: u32) -> Option<String> {
        crate::ui_items::item_icon(self.icons.as_deref(), display_id)
    }

    /// An item's name with its random suffix (`BuildItemName`, `0x706d70`); `None` while its
    /// template is in flight.
    fn item_name(&self, entry: u32, random_property: i32) -> Option<String> {
        let t = self.items.template(entry, 0, &self.commands)?;
        Some(self.rolls().name(&t.name, random_property as u32))
    }

    /// `GetSenderName` (`0x56d960`).
    fn sender_name(
        &self,
        entry: u32,
        message_type: u32,
        guid: u64,
        stationery: u32,
        get_text: &dyn Fn(&str) -> Option<String>,
    ) -> Option<String> {
        match message_type {
            0 | 5 if stationery == STATIONERY_GM => get_text("GM_EMAIL_NAME"),
            0 | 5 => self.names.resolve(guid, &self.commands).map(str::to_string),
            2 => self
                .houses
                .as_deref()
                .and_then(|h| h.0.get(entry))
                .map(|h| h.name.clone()),
            3 => self
                .names
                .resolve_creature(entry, 0, &self.commands)
                .map(str::to_string),
            4 => self
                .go_templates
                .name_by_entry(entry, &self.commands)
                .map(str::to_string),
            _ => None,
        }
    }

    fn attachment_view(&self, a: &MailAttachment) -> MailAttachmentView {
        let template = self.items.template(a.entry, 0, &self.commands);
        let name = template.map(|t| self.rolls().name(&t.name, a.random_property_id as u32));
        let quality = template.map_or(0, |t| {
            if t.inventory_type == 0 {
                -1
            } else {
                t.quality as i32
            }
        });
        let link = template.zip(name.as_deref()).map(|(t, n)| {
            crate::ui_items::item_link_full(
                a.entry,
                a.enchants[0].id,
                a.random_property_id as u32,
                a.suffix_factor,
                n,
                t.quality,
            )
        });
        MailAttachmentView {
            item_id: a.entry,
            item_guid_low: a.item_guid_low,
            texture: template.and_then(|t| self.icon(t.display_info_id)),
            name,
            count: a.count,
            quality,
            link,
            random_property_id: a.random_property_id,
        }
    }

    /// One wire row as the Lua-facing row, by the header (`0x56e520`), text (`0x570bd0`) and
    /// invoice (`0x56e9b0`) readers' laws; anything still in flight stays `None` until it lands.
    fn resolve_row(
        &self,
        row: &MailRow,
        macros: &crate::npc_text::MacroContext,
        get_text: &dyn Fn(&str) -> Option<String>,
    ) -> MailInboxRow {
        let e = &row.entry;
        let is_gm = e.stationery == STATIONERY_GM;
        let attachments: Vec<Option<MailAttachmentView>> = row
            .slots
            .iter()
            .map(|s| {
                s.as_ref()
                    .filter(|a| a.entry != 0)
                    .map(|a| self.attachment_view(a))
            })
            .collect();
        let n = attachments.iter().flatten().count();

        let package_icon = match self
            .packages
            .as_deref()
            .and_then(|p| p.0.iter().find(|p| p.id == e.package))
            .filter(|_| e.package != 0)
        {
            Some(p) => Some(package_view(p).icon),
            None if n > 1 => self.icon(MULTI_ITEM_DISPLAY),
            None => attachments
                .iter()
                .flatten()
                .next()
                .filter(|_| n == 1)
                .and_then(|a| a.texture.clone()),
        };
        let stationery_row = self
            .stationery
            .as_deref()
            .and_then(|s| s.0.rows().iter().find(|r| r.id == e.stationery));
        let stationery_icon = stationery_row
            .and_then(|r| self.items.template(r.item, 0, &self.commands))
            .and_then(|t| self.icon(t.display_info_id));

        // The auction notice's subject and invoice, once the item's template is cached; until
        // then the subject is empty (`0x56f678`).
        let auction = (e.message_type == mail_message_type::AUCTION)
            .then(|| invoice::parse_subject(&e.subject));
        let auction_name = auction.and_then(|a| self.item_name(a.entry, a.random_property));
        let mut subject = match (auction, &auction_name) {
            (Some(a), Some(name)) => invoice::subject_key(a.result)
                .and_then(get_text)
                .map(|t| t.replacen("%s", name, 1))
                .unwrap_or_default(),
            (Some(_), None) => String::new(),
            (None, _) if e.message_type == mail_message_type::CALENDAR => {
                let title = e.subject.split_once(':').map_or("", |(_, t)| t);
                let key = if e.flags & mail_flags::CALENDAR_INVITE_REMOVED != 0 {
                    "CALENDAR_INVITE_REMOVED_MAIL_SUBJECT"
                } else {
                    "CALENDAR_EVENT_REMOVED_MAIL_SUBJECT"
                };
                get_text(key)
                    .map(|t| t.replacen("%s", title, 1))
                    .unwrap_or_default()
            }
            // A template mail's subject comes from `MailTemplate.dbc` (`0x572177`), which is not
            // loaded (#245); the wire's stands.
            (None, _) => e.subject.clone(),
        };
        if e.flags & mail_flags::COD_PAYMENT != 0 {
            if let Some(t) = get_text("COD_PAYMENT") {
                subject = t.replacen("%s", &subject, 1);
            }
        }
        let invoice_item = auction
            .filter(|a| a.is_invoice() && auction_name.is_some())
            .map(|a| a.entry);

        let has_letter = !e.body.is_empty() || e.mail_template_id != 0;
        let body = if e.body.is_empty() {
            // A template mail's body is `MailTemplate.dbc`'s, not loaded (#245).
            None
        } else {
            match e.message_type {
                mail_message_type::AUCTION => None,
                // The profanity filter is not modelled.
                mail_message_type::NORMAL => Some(e.body.clone()),
                // The calendar notice's date line (`0x570cd0`) is not modelled; the wire's body
                // stands.
                mail_message_type::CALENDAR => Some(e.body.clone()),
                _ => Some(crate::npc_text::substitute(&e.body, macros)),
            }
        };
        let takeable = !(!has_letter
            || e.flags & mail_flags::COPIED != 0
            || is_gm
            || e.message_type == mail_message_type::CALENDAR
            || (e.message_type == mail_message_type::AUCTION && invoice_item.is_none()));

        let invoice = auction
            .filter(|_| invoice_item.is_some() && !e.body.is_empty())
            .map(|a| {
                let nums = invoice::parse_body(&e.body, invoice::body_fields(a.result));
                let (eta_hour, eta_minute) = invoice::decode_hour_minute(nums.eta);
                MailInvoice {
                    kind: a.invoice_kind(),
                    item_name: auction_name.clone().unwrap_or_default(),
                    player_name: self
                        .names
                        .resolve(nums.player_guid, &self.commands)
                        .map(str::to_string)
                        .unwrap_or_default(),
                    bid: nums.bid,
                    buyout: nums.buyout,
                    deposit: nums.deposit,
                    consignment: nums.consignment,
                    money_delay: nums.money_delay,
                    eta_hour,
                    eta_minute,
                }
            });

        let me = self.self_q.iter().next().map(|(_, g)| g.0);
        let sender_guid = row.sender_guid();
        // `CanComplain` (`0x56fa70`): a player's unreturned, non-GM mail from neither the player
        // nor a friend.
        let can_complain = me.is_some()
            && e.message_type == mail_message_type::NORMAL
            && e.flags & mail_flags::RETURNED == 0
            && !is_gm
            && Some(e.sender_guid) != me
            && !self.social.is_friend(e.sender_guid);

        MailInboxRow {
            message_id: e.message_id,
            message_type: e.message_type,
            flags: e.flags,
            money: e.money,
            cod: e.cod,
            days_left: e.expire_days,
            is_gm,
            has_letter,
            delete_pending: row.delete_pending,
            package_icon,
            stationery_icon,
            sender: self.sender_name(
                e.sender_entry,
                u32::from(e.message_type),
                sender_guid,
                e.stationery,
                get_text,
            ),
            subject,
            attachments,
            body,
            stationery_texture: stationery_row.map(|r| r.texture.clone()),
            takeable,
            is_invoice: invoice_item.is_some(),
            invoice,
            can_complain,
        }
    }

    /// The usable stationery list (`UpdateStationeryList`, `0x56fc80`): rows always available or
    /// carried in the bags, with a cached template, sorted as `0x56dd00` does: always-available
    /// first, then carried, then by buy price and name. A carried paper has no `cost`.
    fn stationeries(&self, catalog: &Stationery) -> Vec<StationeryView> {
        let store = self.self_q.iter().next().map(|(s, _)| s);
        let mut out: Vec<(bool, bool, u32, StationeryView)> = Vec::new();
        for row in catalog.0.rows() {
            let carried = store.is_some_and(|s| {
                crate::ui_items::count_of(
                    &s.0,
                    &self.objects,
                    row.item,
                    crate::ui_items::InventoryScope::CARRIED,
                ) > 0
            });
            let default = row.flags & 1 != 0;
            if !default && !carried {
                continue;
            }
            let Some(t) = self.items.template(row.item, 0, &self.commands) else {
                continue;
            };
            out.push((
                default,
                carried,
                t.buy_price,
                StationeryView {
                    id: row.id,
                    name: t.name.clone(),
                    icon: self.icon(t.display_info_id).unwrap_or_default(),
                    cost: (!carried).then_some(t.buy_price),
                    texture: row.texture.clone(),
                },
            ));
        }
        out.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.cmp(&a.1))
                .then(a.2.cmp(&b.2))
                .then_with(|| a.3.name.to_lowercase().cmp(&b.3.name.to_lowercase()))
        });
        out.into_iter().map(|(.., v)| v).collect()
    }
}

/// The feed's per-VM memos, bundled under Bevy's parameter limit.
#[derive(Default)]
struct MailMemos {
    inbox: Option<MailState>,
    senders: Option<Vec<Option<String>>>,
    has_new_mail: Option<bool>,
    stationeries: Option<Vec<StationeryView>>,
    packages_pushed: bool,
}

fn feed_mail(
    script: Option<NonSendMut<UiScript>>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
    look: MailLookups,
    mut sink: crate::ui_action::MessageSink,
    mut equip_errors: ResMut<EquipErrors>,
    time: Res<Time>,
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut memos: Local<crate::ui_script::VmMemo<MailMemos>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memo = memos.get(&script);

    // `CGMailInfo::EnterWorld` (`0x571530`): the form, the flags and the rows reset.
    if cascades.read().next().is_some() {
        script.mail_enter_world();
        mail.clear_session();
    }

    if !memo.packages_pushed {
        if let Some(packages) = look.packages.as_deref() {
            memo.packages_pushed = true;
            script.set_mail_packages(packages.0.iter().map(package_view).collect());
        }
    }
    // The usable stationery, asked ahead of any mailbox so the list is whole when
    // `SendMailFrame_Reset` selects row 1 on show. Only with a player: an ask sent before the
    // socket exists is dropped and never re-asked.
    let usable = look
        .stationery
        .as_deref()
        .filter(|_| !look.self_q.is_empty())
        .map(|c| look.stationeries(c))
        .unwrap_or_default();
    if let Some(first) = usable.first() {
        mail.default_stationery = first.id;
    }
    if memo.stationeries.as_ref() != Some(&usable) {
        memo.stationeries = Some(usable.clone());
        script.set_mail_stationeries(usable);
    }

    if std::mem::take(&mut mail.open_requested) {
        script.open_mailbox();
    }
    if std::mem::take(&mut mail.close_requested) {
        script.close_mailbox();
    }

    // `None` until the player is named; a letter body re-substitutes when it lands.
    let subject = crate::npc_text::player_identity(&look.self_q, &look.names, &look.commands);
    let macros = crate::npc_text::MacroContext {
        subject: subject.as_ref(),
        states: &look.states,
        achievement: None,
    };
    let snapshot = |script: &UiScript, mail: &MailOpen| {
        let get_text = |key: &str| script.lua().globals().get::<String>(key).ok();
        MailState {
            inbox: mail
                .rows
                .iter()
                .map(|r| look.resolve_row(r, &macros, &get_text))
                .collect(),
            total: mail.total,
        }
    };

    // The results' effects, in the reference's order: the message, the row's change with its
    // events, the pending flags, then `MAIL_SUCCESS` or `MAIL_FAILED`.
    let mut inbox_fired = false;
    for effect in std::mem::take(&mut mail.effects) {
        match effect {
            MailEffect::Message(key) => {
                let line = crate::ui_action::keyed_line(&script, key);
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_mail", line);
            }
            MailEffect::EquipError(reason) => equip_errors.0.push(EquipError {
                reason: reason as u8,
                required_level: None,
                // The result names no bag slot: 255 is the wire's "own array" sentinel.
                bag_slot: 255,
            }),
            MailEffect::Sent => script.mail_sent(),
            MailEffect::CloseInboxItem(index) => {
                script.fire_event("CLOSE_INBOX_ITEM", vec![ScriptValue::Int(i64::from(index))]);
            }
            MailEffect::InboxUpdate => {
                let fresh = snapshot(&script, &mail);
                script.set_mail(fresh.clone());
                memo.inbox = Some(fresh);
                script.fire_event("MAIL_INBOX_UPDATE", vec![]);
                inbox_fired = true;
            }
            MailEffect::Done { send } => script.mail_command_done(send),
            MailEffect::Result { ok } => {
                script.fire_event(if ok { "MAIL_SUCCESS" } else { "MAIL_FAILED" }, vec![]);
            }
        }
    }

    // A template, sender name or body landing repaints, as the reference's query callbacks
    // signal `MAIL_INBOX_UPDATE` (`0x56d4b0`, `0x56d460`).
    let fresh = snapshot(&script, &mail);
    if memo.inbox.as_ref() != Some(&fresh) {
        script.set_mail(fresh.clone());
        memo.inbox = Some(fresh);
        if !inbox_fired {
            script.fire_event("MAIL_INBOX_UPDATE", vec![]);
        }
    }

    pending.step(time.delta_secs());
    let has_new_mail = pending.has_new_mail();
    if memo.has_new_mail != Some(has_new_mail) {
        script.set_has_new_mail(has_new_mail);
        memo.has_new_mail = Some(has_new_mail);
    }
    let senders: Vec<Option<String>> = {
        let get_text = |key: &str| script.lua().globals().get::<String>(key).ok();
        pending
            .delivered_senders()
            .map(|s| look.sender_name(s.entry, s.message_type, s.guid, s.stationery, &get_text))
            .collect()
    };
    if memo.senders.as_ref() != Some(&senders) {
        // A sender's name landing signals as the name callback does (`0x56d47a`).
        if memo.senders.is_some() {
            pending.notify_senders();
        }
        script.set_mail_latest_senders(senders.clone());
        memo.senders = Some(senders);
    }
    if pending.take_notify() {
        script.fire_event("UPDATE_PENDING_MAIL", vec![]);
    }
}

/// The VM's mail packets out, each as its verb's reference send does.
fn drain_mail(
    script: Option<NonSendMut<UiScript>>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
) {
    use benilla_protocol::messages::mail_delete_reason;
    use benilla_ui::script::MailCommand;
    let Some(mut script) = script else {
        return;
    };
    for command in script.take_mail_commands() {
        if let MailCommand::Close = command {
            // `CloseMail(false)`'s tail (`0x571a99`): a read or an arrival while open re-asks.
            debug!("ui_mail: the window closed");
            mail.mailbox = None;
            mail.auto_looting = None;
            if pending.take_do_pending_query() {
                query_next_mail_time(&mut pending, &commands);
            }
            continue;
        }
        let Some(mailbox) = mail.mailbox else {
            continue; // the window closed under the verb
        };
        let send = |cmd: ClientCommand| {
            debug!("ui_mail: sending {cmd:?}");
            let _ = commands.0.send(cmd);
        };
        match command {
            MailCommand::GetList => send(ClientCommand::GetMailList { mailbox }),
            MailCommand::MarkRead { message_id } => {
                send(ClientCommand::MailMarkAsRead {
                    mailbox,
                    mail_id: message_id,
                });
                mark_read(&mut mail, &mut pending, message_id);
            }
            MailCommand::TakeMoney { message_id } => send(ClientCommand::MailTakeMoney {
                mailbox,
                mail_id: message_id,
            }),
            MailCommand::TakeItem {
                message_id,
                item_guid_low,
                auto_loot,
            } => {
                send(ClientCommand::MailTakeItem {
                    mailbox,
                    mail_id: message_id,
                    item_guid_low,
                });
                if auto_loot {
                    mail.auto_looting = Some(message_id);
                }
            }
            MailCommand::TextItem { message_id } => send(ClientCommand::MailCreateTextItem {
                mailbox,
                mail_id: message_id,
            }),
            MailCommand::Return { message_id } => {
                if let Some(i) = mail.index_of(message_id) {
                    mail.rows[i].delete_pending = true;
                    send(ClientCommand::MailReturnToSender {
                        mailbox,
                        mail_id: message_id,
                        sender: mail.rows[i].entry.sender_guid,
                    });
                }
            }
            MailCommand::Delete { message_id } => {
                if let Some(i) = mail.index_of(message_id) {
                    mail.rows[i].delete_pending = true;
                }
                send(ClientCommand::MailDelete {
                    mailbox,
                    mail_id: message_id,
                    reason: mail_delete_reason::PLAYER,
                });
            }
            MailCommand::Send(req) => {
                // Each attachment's guid is read off its bag slot as the send fires.
                let store = self_q.iter().next();
                let mut items = Vec::with_capacity(req.items.len());
                let mut lost = None;
                for &(send_slot, bag, slot) in &req.items {
                    match store.and_then(|s| {
                        crate::ui_items::slot_guid(&s.0, bag, (slot.max(1) - 1) as u8, &objects)
                    }) {
                        Some(guid) => items.push((send_slot, guid)),
                        None => {
                            lost = Some(send_slot);
                            break;
                        }
                    }
                }
                if let Some(send_slot) = lost {
                    // The item left its slot: `ERR_ITEM_NOT_FOUND`, no packet (`0x570a7c`).
                    mail.effects.push(MailEffect::Message("ERR_ITEM_NOT_FOUND"));
                    script.drop_mail_send_item(send_slot);
                    script.mail_command_done(true);
                    continue;
                }
                send(ClientCommand::SendMail {
                    mailbox,
                    receiver: req.target,
                    subject: req.subject,
                    body: req.body,
                    stationery: req.stationery,
                    package: req.package,
                    items,
                    money: req.money,
                    cod: req.cod,
                });
            }
            MailCommand::Close => unreachable!("handled above"),
        }
    }
}

/// `MarkAsRead`'s local half (`0x56fe30`): the row turns read, the senders with nothing unread
/// drop out, the icon goes dark once all is read, and the close will re-ask.
fn mark_read(mail: &mut MailOpen, pending: &mut MailPending, message_id: u32) {
    let Some(i) = mail.index_of(message_id) else {
        return;
    };
    mail.rows[i].entry.flags |= mail_flags::READ;
    let unread = |guid: u64, entry: u32| {
        mail.rows.iter().any(|r| {
            r.entry.flags & mail_flags::READ == 0
                && ((guid != 0 && guid == r.entry.sender_guid)
                    || (entry != 0 && entry == r.entry.sender_entry))
        })
    };
    pending.update_recent_senders(unread);
    let all_read = mail
        .rows
        .iter()
        .all(|r| r.entry.flags & mail_flags::READ != 0);
    pending.on_mail_read(all_read);
    mail.effects.push(MailEffect::InboxUpdate);
}
