//! The app half of [`benilla_ui::script`]'s `mail` module, 12340's mail state.
//!
//! A mailbox opens locally, from a mailbox GameObject's use (`0x70c9d0`) or a mailbox NPC's
//! interaction (`0x6de11f`), or by `SMSG_SHOW_MAILBOX`; each ends in the mailbox open
//! (`0x56da60`), which resets the form and fires `MAIL_SHOW`, whose `CheckInbox()` asks for the
//! list. [`MailOpen`] is the client's inbox: the handlers move it and raise its events at their
//! steps, and the verbs read it at the call through [`MailLive`], each read leaving its callback on
//! a record in flight. The VM gates the verbs and queues their packets; the drain sends them.
//!
//! [`MailPending`] is the arrival layer behind `HasNewMail()`, the minimap icon and its sender
//! tooltip; it never reads the inbox, only the next-mail time and its three senders.

use benilla_protocol::messages::{
    mail_flags, mail_message_type, MailAttachment, MailListEntry, MAIL_ATTACHMENTS_MAX,
};
use bevy::prelude::*;

use benilla_ui::script::{
    MailAttachmentView, MailComplaint, MailInboxRow, MailInvoice, MailPacket, MailRead,
    StationeryView, UiScript, STATIONERY_GM,
};

use crate::entities::ItemDisplays;
use crate::items::{Items, RecordWait, RecordWaits};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_action::signals::{QueuedSignals, SignalSink};
use crate::ui_script::live::UiEvents;
use crate::ui_session::NpcSession;

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
#[derive(Clone)]
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

    /// The attachment count (`0x56d240`): slots holding an item.
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

    /// The purge the money and item takes (`0x56f770`, `0x56f8d0`) test after a take: an
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

/// The client's string-to-int64 parse: the leading decimal digits.
fn leading_u64(s: &str) -> u64 {
    s.trim_start()
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0u64, |n, b| {
            n.wrapping_mul(10).wrapping_add(u64::from(b - b'0'))
        })
}

/// A record a mail read left a name callback on: a player's name, or the creature's or game
/// object's record a sender's name reads (`0x56d960`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameRecord {
    Player(u64),
    Creature(u32),
    GameObject(u32),
}

/// Which callback a name read left, by what it raises once the name lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameCallback {
    /// The update queued, then `UPDATE_PENDING_MAIL` raised (`0x56d460`).
    UpdateAndPending,
    /// The update queued alone (`0x56d4b0`).
    UpdateOnly,
}

/// The client's mail state (`0xbeb030`-`0xbeb204`): the open mailbox, the inbox rows (kept past a
/// close, as the reference keeps them until a world change, `0x571730`), the auto-loot in flight,
/// the callbacks its reads left and the usable stationery.
#[derive(Resource, Default, Clone)]
pub(crate) struct MailOpen {
    /// The open mailbox (`[0xbeb030]`).
    pub(crate) mailbox: Option<u64>,
    /// Wire order, which is the 1-based display order (`[0xbeb204]`, count `[0xbeb1d8]`).
    pub(crate) rows: Vec<MailRow>,
    /// The server's whole count past the shown rows (`[0xbeb1dc]`).
    pub(crate) total: u32,
    /// The mail `AutoLootMailItem` is emptying one attachment per result (`[0xbeb1ec]`).
    auto_looting: Option<u32>,
    /// The name callbacks the reads left, one per miss: `0x56d960` links with its once flag 0.
    name_waits: Vec<(NameRecord, NameCallback)>,
    /// The stationery records the world entry's asks still wait on (`[0xbeb1d4]`).
    stationery_pending: u32,
    /// The usable stationery as last built (`0x56fc80`, count `[0xbeb1c8]`).
    stationery: Vec<StationeryView>,
}

impl MailOpen {
    pub(crate) fn index_of(&self, message_id: u32) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.entry.message_id == message_id)
    }

    /// The session's end: no mailbox, no rows, and the callbacks in flight gone with the socket.
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }

    /// The world entry's part (`0x571530`): no mailbox, no rows and no auto-loot; the stationery
    /// list empties for its rebuild. The callbacks stay linked on their records.
    fn enter_world(&mut self) {
        self.mailbox = None;
        self.rows.clear();
        self.total = 0;
        self.auto_looting = None;
        self.stationery.clear();
        self.stationery_pending = 0;
    }

    /// The first stationery a row with none takes (`0x5720c3`): the list's first, else none.
    fn default_stationery(&self) -> u32 {
        self.stationery.first().map_or(0, |s| s.id)
    }

    /// The name callbacks left on `record`, unlinked, in link order.
    pub(crate) fn take_name_waits(&mut self, record: NameRecord) -> Vec<NameCallback> {
        let mut taken = Vec::new();
        self.name_waits.retain(|&(r, callback)| {
            let hit = r == record;
            if hit {
                taken.push(callback);
            }
            !hit
        });
        taken
    }
}

/// The range guard ([`crate::ui_session`]) closes the window past the client's interaction gate;
/// the interaction close on the mailbox is `CloseMail(false)` (`0x51302d`).
impl NpcSession for MailOpen {
    fn npc(&self) -> Option<u64> {
        self.mailbox
    }

    fn close(&mut self) {
        self.mailbox = None;
        self.auto_looting = None;
    }
}

/// The mailbox open (`0x56da60`, `0x56dbc0`): the interact target set on the mailbox first (`0x56da67`, `0x56dbe2`), whose interaction close takes the window bound to the old one, then in its turn the mailbox stored, the form reset and `MAIL_SHOW`.
pub(crate) fn talk_to_mailbox(world: &mut World, mailbox: u64) {
    crate::ui_session::set_interact_target(world, mailbox);
    crate::ui_script::live::in_turn(world, move |w| {
        if let Some(mut mail) = w.get_resource_mut::<MailOpen>() {
            mail.mailbox = Some(mailbox);
        }
        if let Some(mut ui) = w.get_resource_mut::<UiEvents>() {
            ui.write(|script| {
                script.close_bank_interaction();
                script.open_mailbox();
            });
        }
    });
}

/// The interaction close's mail leg (`0x51302d` -> `0x571730` with 0), its target already zeroed: the window closes, then in turn the form clears, `MAIL_CLOSED` fires and the deferred query goes out.
pub(crate) fn close_by_interaction(world: &mut World) {
    if let Some(mut mail) = world.get_resource_mut::<MailOpen>() {
        NpcSession::close(&mut *mail);
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.write(UiScript::close_mailbox);
        // The deferred query after `MAIL_CLOSED` (`0x57179d`-`0x5717a7`).
        ui.world(|w| {
            if !w.contains_resource::<MailPending>() {
                return;
            }
            w.resource_scope(|w, mut pending: Mut<MailPending>| {
                if let Some(commands) = w.get_resource::<NetCommands>() {
                    if pending.take_do_pending_query() {
                        query_next_mail_time(&mut pending, commands);
                    }
                }
            });
        });
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
                    // The cascade's order: the time query, this, then the battlefield status.
                    send_query_next_mail_time_on_enter.after(crate::net::send_query_time),
                    update_pending_mail,
                ),
            );
    }
}

/// The mail state's world entry (`0x571530`) at each world-enter cascade, after the time query and
/// before the battlefield status: no mailbox and no rows, the form reset in the VM, each
/// stationery row's item record asked with its callback (`0x5715e3`) and the list built at once
/// with none in flight (`0x571602`), the senders cleared and `MSG_QUERY_NEXT_MAIL_TIME` sent,
/// stamping "no mail".
pub(crate) fn send_query_next_mail_time_on_enter(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut pending: ResMut<MailPending>,
    mut mail: Option<ResMut<MailOpen>>,
    items: Option<Res<Items>>,
    catalog: Option<Res<Stationery>>,
    commands: Res<NetCommands>,
    mut waits: Option<NonSendMut<RecordWaits>>,
    ui: Option<ResMut<UiEvents>>,
    mut ecs: Commands,
) {
    if cascades.read().next().is_none() {
        return;
    }
    if let Some(mail) = mail.as_deref_mut() {
        mail.enter_world();
        if let Some(mut ui) = ui {
            ui.write(UiScript::mail_enter_world);
            ecs.queue(crate::ui_script::live::fire_handler_events);
        }
        if let (Some(catalog), Some(items)) = (catalog.as_deref(), items.as_deref()) {
            for row in catalog.0.rows() {
                if items.template(row.item, 0, &commands).is_none() {
                    if let Some(waits) = waits.as_deref_mut() {
                        waits.wait(row.item, RecordWait::MailStationery, false);
                    }
                    mail.stationery_pending += 1;
                }
            }
            if mail.stationery_pending == 0 {
                ecs.queue(|world: &mut World| run_mail_system(world, build_stationery));
            }
        }
    }
    *pending = MailPending::default();
    query_next_mail_time(&mut pending, &commands);
}

fn run_mail_system<M>(world: &mut World, system: impl IntoSystem<(), (), M> + 'static) {
    if let Err(e) = world.run_system_cached(system) {
        warn!("ui_mail: the stationery build did not run: {e}");
    }
}

/// The stationery list built (`0x56fc80`), with none of its records in flight.
fn build_stationery(mut mail: ResMut<MailOpen>, look: MailLookups) {
    if let Some(catalog) = look.stationery.as_deref() {
        mail.stationery = look.stationeries(catalog);
    }
}

/// A stationery record the world entry asked for answered (`0x571520`): one fewer in flight, and
/// the list rebuilt once none is (`0x571529`).
pub(crate) fn stationery_callback(world: &mut World) {
    run_mail_system(world, rebuild_stationery_on_callback);
}

fn rebuild_stationery_on_callback(mut mail: ResMut<MailOpen>, look: MailLookups) {
    mail.stationery_pending = mail.stationery_pending.saturating_sub(1);
    if mail.stationery_pending == 0 {
        if let Some(catalog) = look.stationery.as_deref() {
            if !look.self_q.is_empty() {
                mail.stationery = look.stationeries(catalog);
            }
        }
    }
}

/// The pending mail's per-frame step (`0x56ded0`, from the world frame's update at `0x4fa360`):
/// each countdown that lands signals `UPDATE_PENDING_MAIL` there.
fn update_pending_mail(
    time: Res<Time>,
    mut pending: ResMut<MailPending>,
    mut signals: QueuedSignals,
    mut ecs: Commands,
) {
    for _ in 0..pending.step(time.delta_secs()) {
        signals.signal("UPDATE_PENDING_MAIL");
    }
    signals.flush(&mut ecs);
}

/// `QueryNextMailTime` (`0x56ff70`).
fn query_next_mail_time(pending: &mut MailPending, commands: &NetCommands) {
    pending.on_query_sent();
    let _ = commands.0.send(ClientCommand::QueryNextMailTime);
}

/// `ReceivedMail` (`0x5704d0`), which `SMSG_RECEIVED_MAIL` and the auction notices call: with a
/// mailbox open it only defers a re-query to the close; otherwise it re-queries and keeps the
/// delay. True when it lit the icon, which the caller signals as `UPDATE_PENDING_MAIL` there.
pub(crate) fn received_mail(
    delay: f32,
    source: &str,
    pending: &mut MailPending,
    mail: &MailOpen,
    commands: &NetCommands,
) -> bool {
    debug!(
        "ui_mail: ReceivedMail({delay}) from {source}, mailbox {}",
        if mail.mailbox.is_some() {
            "open"
        } else {
            "closed"
        }
    );
    let received = pending.received_mail(delay, mail.mailbox.is_some());
    if received.query {
        let _ = commands.0.send(ClientCommand::QueryNextMailTime);
    }
    received.lit
}

/// [`received_mail`] for a handler outside the mailbox, as the auction notices are.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MailArrival<'w> {
    pending: ResMut<'w, MailPending>,
    mail: Res<'w, MailOpen>,
    commands: Res<'w, NetCommands>,
}

impl MailArrival<'_> {
    /// `ReceivedMail(delay)` (`0x5704d0`); `source` names the caller in the debug trace. True when
    /// it lit the icon: the caller signals `UPDATE_PENDING_MAIL` at its step.
    #[must_use]
    pub(crate) fn arrive(&mut self, delay: f32, source: &str) -> bool {
        received_mail(delay, source, &mut self.pending, &self.mail, &self.commands)
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
    social: ResMut<'w, crate::ui_social::SocialState>,
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

    /// An item's name with its random suffix (`0x706d70`); `None` while its
    /// template is in flight.
    fn item_name(&self, entry: u32, random_property: i32) -> Option<String> {
        let t = self.items.template(entry, 0, &self.commands)?;
        Some(self.rolls().name(&t.name, random_property as u32))
    }

    /// The sender's display name (`0x56d960`).
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
                &benilla_ui::script::ItemLinkFields {
                    enchant: a.enchants[0].id as i32,
                    gems: std::array::from_fn(|i| a.enchants[2 + i].id as i32),
                    ..benilla_ui::script::ItemLinkFields::roll(
                        a.random_property_id,
                        a.suffix_factor as i32,
                    )
                },
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
        military: bool,
    ) -> MailInboxRow {
        let e = &row.entry;
        let sender_guid = row.sender_guid();
        let sender = self.sender_name(
            e.sender_entry,
            u32::from(e.message_type),
            sender_guid,
            e.stationery,
            get_text,
        );
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
                // The notice's sentence: who, which event and when (`0x570cd0`); a name still in
                // flight prints as MSVC's `%s` of a null pointer.
                mail_message_type::CALENDAR => {
                    let text = |k: &str| get_text(k).unwrap_or_default();
                    let when = benilla_ui::script::date_time_text(
                        leading_u64(&e.body) as u32,
                        military,
                        &text,
                    );
                    let key = if e.flags & mail_flags::CALENDAR_INVITE_REMOVED != 0 {
                        "CALENDAR_INVITE_REMOVED_MAIL_BODY"
                    } else {
                        "CALENDAR_EVENT_REMOVED_MAIL_BODY"
                    };
                    let title = e.subject.split_once(':').map_or("", |(_, t)| t);
                    let who = sender.as_deref().unwrap_or("(null)");
                    Some(get_text(key).map_or_else(String::new, |t| {
                        benilla_ui::strings::fill(
                            &t,
                            &[
                                benilla_ui::strings::Arg::S(who),
                                benilla_ui::strings::Arg::S(title),
                                benilla_ui::strings::Arg::S(&when),
                            ],
                        )
                    }))
                }
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
        // The complaint gate (`0x56fa70`): a player's unreturned, non-GM mail from neither the
        // player nor a friend.
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
            sender,
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

    /// The usable stationery list (`0x56fc80`): rows always available or
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

/// The record a sender's name reads (`0x56d960`): a player's or calendar notice's guid, a
/// creature's or game object's entry; none for a GM's mail, an auction house or anything else.
fn sender_record(message_type: u32, guid: u64, entry: u32, stationery: u32) -> Option<NameRecord> {
    match message_type {
        0 | 5 if stationery == STATIONERY_GM => None,
        0 | 5 => Some(NameRecord::Player(guid)),
        3 => Some(NameRecord::Creature(entry)),
        4 => Some(NameRecord::GameObject(entry)),
        _ => None,
    }
}

/// The callbacks one verb's read of a row leaves, by the records it missed.
#[derive(Default)]
struct ReadMisses {
    /// Item records, each with the inbox's callback (`0x56d4b0`).
    items: Vec<u32>,
    names: Vec<(NameRecord, NameCallback)>,
}

impl MailLookups<'_, '_> {
    fn cached(&self, entry: u32) -> bool {
        self.items.template_cached(entry).is_some()
    }

    /// What `read` of `row`, resolved as `resolved`, leaves its callback on: each record the verb
    /// reads that is still in flight.
    fn read_misses(
        &self,
        row: &MailRow,
        read: MailRead,
        resolved: &MailInboxRow,
        get_text: &dyn Fn(&str) -> Option<String>,
    ) -> ReadMisses {
        let e = &row.entry;
        let mut misses = ReadMisses::default();
        let sender = || {
            sender_record(
                u32::from(e.message_type),
                row.sender_guid(),
                e.sender_entry,
                e.stationery,
            )
        };
        match read {
            MailRead::Header => {
                // The icon: a package's own; else a lone attachment's record (`0x56e668`).
                let package = self
                    .packages
                    .as_deref()
                    .is_some_and(|p| e.package != 0 && p.0.iter().any(|p| p.id == e.package));
                if !package && row.num_attachments() == 1 {
                    if let Some(a) = row.slots.iter().flatten().find(|a| a.entry != 0) {
                        if !self.cached(a.entry) {
                            misses.items.push(a.entry);
                        }
                    }
                }
                // The stationery's item (`0x56e711`).
                if let Some(item) = self
                    .stationery
                    .as_deref()
                    .and_then(|s| s.0.rows().iter().find(|r| r.id == e.stationery))
                    .map(|r| r.item)
                {
                    if !self.cached(item) {
                        misses.items.push(item);
                    }
                }
                // The sender's name (`0x56e76c`).
                if resolved.sender.is_none() {
                    misses
                        .names
                        .extend(sender().map(|r| (r, NameCallback::UpdateAndPending)));
                }
            }
            // A calendar notice's body names its sender (`0x570d2a`), once its template reads.
            MailRead::Text => {
                let key = if e.flags & mail_flags::CALENDAR_INVITE_REMOVED != 0 {
                    "CALENDAR_INVITE_REMOVED_MAIL_BODY"
                } else {
                    "CALENDAR_EVENT_REMOVED_MAIL_BODY"
                };
                if e.message_type == mail_message_type::CALENDAR
                    && !e.body.is_empty()
                    && get_text(key).is_some_and(|t| !t.is_empty())
                    && resolved.sender.is_none()
                {
                    misses
                        .names
                        .extend(sender().map(|r| (r, NameCallback::UpdateAndPending)));
                }
            }
            // The invoice's other player (`0x56eb79`).
            MailRead::Invoice => {
                if let Some(guid) = self.invoice_player(row) {
                    if self.names.peek(guid).is_none() {
                        misses
                            .names
                            .push((NameRecord::Player(guid), NameCallback::UpdateOnly));
                    }
                }
            }
            // The attachment's record (`0x571017`, `0x56ed9f`).
            MailRead::Item(slot) => {
                let a = match slot {
                    Some(s) => row.slots.get(s as usize).and_then(Option::as_ref),
                    None => row.slots.iter().flatten().next(),
                };
                if let Some(a) = a.filter(|a| a.entry != 0) {
                    if !self.cached(a.entry) {
                        misses.items.push(a.entry);
                    }
                }
            }
            MailRead::Plain => {}
        }
        misses
    }

    /// The guid an auction invoice names, when the row reads as one.
    fn invoice_player(&self, row: &MailRow) -> Option<u64> {
        let e = &row.entry;
        let auction = (e.message_type == mail_message_type::AUCTION)
            .then(|| invoice::parse_subject(&e.subject))?;
        if !auction.is_invoice() || e.body.is_empty() || !self.cached(auction.entry) {
            return None;
        }
        Some(invoice::parse_body(&e.body, invoice::body_fields(auction.result)).player_guid)
    }
}

/// The mail the verbs read at the call: the inbox, the arrival state and what a row resolves
/// against.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MailLive<'w, 's> {
    mail: ResMut<'w, MailOpen>,
    pending: ResMut<'w, MailPending>,
    look: MailLookups<'w, 's>,
}

impl MailLive<'_, '_> {
    /// The shown rows and the server's whole count.
    pub(crate) fn counts(&self) -> (usize, u32) {
        (self.mail.rows.len(), self.mail.total)
    }

    /// Row `index` (0-based) resolved now, `read` leaving its callbacks on the records it missed.
    pub(crate) fn row(
        &mut self,
        index: usize,
        read: MailRead,
        get_text: &dyn Fn(&str) -> Option<String>,
        military: bool,
        waits: Option<&mut RecordWaits>,
    ) -> Option<MailInboxRow> {
        let (resolved, misses) = {
            let row = self.mail.rows.get(index)?;
            // `None` until the player is named; a letter body re-substitutes when it lands.
            let subject = crate::npc_text::player_identity(
                &self.look.self_q,
                &self.look.names,
                &self.look.commands,
            );
            let macros = crate::npc_text::MacroContext {
                subject: subject.as_ref(),
                states: &self.look.states,
                achievement: None,
            };
            let resolved = self.look.resolve_row(row, &macros, get_text, military);
            let misses = self.look.read_misses(row, read, &resolved, get_text);
            (resolved, misses)
        };
        if let Some(waits) = waits {
            for entry in misses.items {
                waits.wait(entry, RecordWait::MailInbox, false);
            }
        }
        self.mail.name_waits.extend(misses.names);
        Some(resolved)
    }

    /// The mark-as-read (`0x56fe30`) of row `index` (0-based): with the row unread, the packet goes
    /// out, the row turns read, the senders with nothing unread drop out and a waiting icon goes
    /// dark once every row is read; the signals each step raised, in order, none for no unread row.
    pub(crate) fn mark_read(&mut self, index: usize) -> Vec<&'static str> {
        let mailbox = self.mail.mailbox.unwrap_or(0);
        let Some(row) = self
            .mail
            .rows
            .get_mut(index)
            .filter(|r| r.entry.flags & mail_flags::READ == 0)
        else {
            return Vec::new();
        };
        let _ = self.look.commands.0.send(ClientCommand::MailMarkAsRead {
            mailbox,
            mail_id: row.entry.message_id,
        });
        row.entry.flags |= mail_flags::READ;
        let rows = &self.mail.rows;
        let mut signals = Vec::new();
        let unread = |guid: u64, entry: u32| {
            rows.iter().any(|r| {
                r.entry.flags & mail_flags::READ == 0
                    && ((guid != 0 && guid == r.sender_guid())
                        || (entry != 0 && entry == r.entry.sender_entry))
            })
        };
        if self.pending.update_recent_senders(unread) {
            signals.push("UPDATE_PENDING_MAIL");
        }
        let all_read = rows.iter().all(|r| r.entry.flags & mail_flags::READ != 0);
        if self.pending.on_mail_read(all_read) {
            signals.push("UPDATE_PENDING_MAIL");
        }
        signals.push("MAIL_INBOX_UPDATE");
        signals
    }

    /// A return or delete in flight on row `index` (0-based) (`+0x2940`).
    pub(crate) fn mark_delete_pending(&mut self, index: usize) {
        if let Some(row) = self.mail.rows.get_mut(index) {
            row.delete_pending = true;
        }
    }

    /// `HasNewMail()`'s test (`0x56f350`).
    pub(crate) fn has_new(&self) -> bool {
        self.pending.has_new_mail()
    }

    /// The delivered senders' names (`0x56f3a0`), a miss leaving the sender's callback
    /// (`0x56f3e0`) and reading nil.
    pub(crate) fn latest_senders(
        &mut self,
        get_text: &dyn Fn(&str) -> Option<String>,
    ) -> Vec<Option<String>> {
        let mut misses = Vec::new();
        let names = self
            .pending
            .delivered_senders()
            .map(|s| {
                let name =
                    self.look
                        .sender_name(s.entry, s.message_type, s.guid, s.stationery, get_text);
                if name.is_none() {
                    misses.extend(sender_record(s.message_type, s.guid, s.entry, s.stationery));
                }
                name
            })
            .collect();
        self.mail.name_waits.extend(
            misses
                .into_iter()
                .map(|r| (r, NameCallback::UpdateAndPending)),
        );
        names
    }

    /// The usable stationery, rebuilt first on `rebuild` with no record in flight and a player to
    /// read (`0x571abd`, `0x56fc80`).
    pub(crate) fn stationeries(&mut self, rebuild: bool) -> Vec<StationeryView> {
        if rebuild && self.mail.stationery_pending == 0 && !self.look.self_q.is_empty() {
            if let Some(catalog) = self.look.stationery.as_deref() {
                self.mail.stationery = self.look.stationeries(catalog);
            }
        }
        self.mail.stationery.clone()
    }

    /// `Package.dbc`'s rows, in file order.
    pub(crate) fn packages(&self) -> Vec<benilla_ui::script::PackageView> {
        self.look
            .packages
            .as_deref()
            .map(|p| p.0.iter().map(package_view).collect())
            .unwrap_or_default()
    }
}

impl MailLive<'_, '_> {
    /// Send `packet` to the open mailbox inside the verb's call; nothing with none open.
    pub(crate) fn send(&mut self, packet: MailPacket) {
        use benilla_protocol::messages::mail_delete_reason;
        let Some(mailbox) = self.mail.mailbox else {
            return;
        };
        let command = match packet {
            MailPacket::GetList => ClientCommand::GetMailList { mailbox },
            MailPacket::MarkRead { message_id } => ClientCommand::MailMarkAsRead {
                mailbox,
                mail_id: message_id,
            },
            MailPacket::TakeMoney { message_id } => ClientCommand::MailTakeMoney {
                mailbox,
                mail_id: message_id,
            },
            MailPacket::TakeItem {
                message_id,
                item_guid_low,
                auto_loot,
            } => {
                if auto_loot {
                    self.mail.auto_looting = Some(message_id);
                }
                ClientCommand::MailTakeItem {
                    mailbox,
                    mail_id: message_id,
                    item_guid_low,
                }
            }
            MailPacket::TextItem { message_id } => ClientCommand::MailCreateTextItem {
                mailbox,
                mail_id: message_id,
            },
            MailPacket::Return { message_id } => {
                let Some(i) = self.mail.index_of(message_id) else {
                    return;
                };
                ClientCommand::MailReturnToSender {
                    mailbox,
                    mail_id: message_id,
                    sender: self.mail.rows[i].entry.sender_guid,
                }
            }
            MailPacket::Delete { message_id } => ClientCommand::MailDelete {
                mailbox,
                mail_id: message_id,
                reason: mail_delete_reason::PLAYER,
            },
            MailPacket::Send(req) => ClientCommand::SendMail {
                mailbox,
                receiver: req.target,
                subject: req.subject,
                body: req.body,
                stationery: req.stationery,
                package: req.package,
                items: req.items,
                money: req.money,
                cod: req.cod,
            },
        };
        debug!("ui_mail: sending {command:?}");
        let _ = self.look.commands.0.send(command);
    }

    /// The complaint about the sender of `message_id` (`0x56faf0`): nothing with the complaint
    /// system off; a sender already complained about is told back for its line (`0x56fb60`);
    /// else they join the list (`0x56fbb4`) and the report goes out (`0x56fc0f`).
    pub(crate) fn complain(&mut self, message_id: u32) -> MailComplaint {
        if self.look.social.complaint_status() == 0 {
            return MailComplaint::Refused;
        }
        let Some(index) = self.mail.index_of(message_id) else {
            return MailComplaint::Refused;
        };
        let sender = self.mail.rows[index].entry.sender_guid;
        if self.look.social.is_complained(sender) {
            return MailComplaint::Again;
        }
        self.look.social.add_complained(sender);
        let _ = self.look.commands.0.send(ClientCommand::ComplainMail {
            sender,
            mail_id: message_id,
        });
        MailComplaint::Filed(index)
    }

    /// The open mailbox, `None` with none open.
    pub(crate) fn mailbox(&self) -> Option<u64> {
        self.mail.mailbox
    }

    /// The close's zeroing of the mailbox and the auto-loot (`0x571a6a`-`0x571a7e`).
    pub(crate) fn end_session(&mut self) {
        NpcSession::close(&mut *self.mail);
    }

    /// The close's tail (`0x571a99`, `0x5717a7`): the query an arrival deferred while the mailbox
    /// was open, sent once.
    pub(crate) fn close_query(&mut self) {
        if self.pending.take_do_pending_query() {
            query_next_mail_time(&mut self.pending, &self.look.commands);
        }
    }
}
