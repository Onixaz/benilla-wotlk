//! The charter sessions: the guild registrar, the arena registrar's petition vendor, the petition
//! window, and the verbs that found a guild or an arena team.
//!
//! `SMSG_PETITION_SHOWLIST` (`0x6d69f0`) opens the guild registrar for an NPC that is both a
//! petitioner and a tabard designer, and the petition vendor for any other; both take the interact
//! target. The petition window is bound to a charter item with no NPC, so walking away leaves it
//! open. Its handlers fire the events in their turn and the verbs read and act at the call.
//!
//! `SMSG_PETITION_SHOW_SIGNATURES` carries no text: the name and requirement come from
//! `SMSG_PETITION_QUERY_RESPONSE`, the signer names from the name cache; `PETITION_SHOW` waits for
//! the record and every signer name (`0x5cea60`, `0x5cebf0`, `0x5cf650`).

use benilla_protocol::messages::PetitionShowListEntry;
use benilla_ui::script::ArenaBannerChoice;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;
use crate::ui_script::live::UiEvents;

mod lines;
mod live;
mod net;

pub(crate) use live::{ActContext, PetitionLive};

/// The registrar (`0xc0e570`) and its charter row (`0xc0e558`), which outlives the close until the world entry (`0x599dd0`).
#[derive(Resource, Default)]
pub(crate) struct GuildRegistrarState {
    registrar: Option<u64>,
    row: PetitionShowListEntry,
}

impl GuildRegistrarState {
    /// The registrar the window is parked on.
    pub(crate) fn registrar(&self) -> Option<u64> {
        self.registrar
    }
}

/// The `UNIT_NPC_FLAGS` bits that make a showlist's NPC the guild registrar, both of them
/// (`0x6d6ab7`); an NPC with fewer opens the petition vendor.
const REGISTRAR_NPC_FLAGS: u32 = crate::target::cursor_mode::npc_flags::PETITIONER
    | crate::target::cursor_mode::npc_flags::TABARDDESIGNER;

impl crate::ui_session::NpcSession for GuildRegistrarState {
    fn npc(&self) -> Option<u64> {
        self.registrar
    }

    /// The zero alone; its event fires from [`close_registrar`].
    fn close(&mut self) {
        self.registrar = None;
    }
}

/// The arena registrar's vendor (`0xc0f7c8`), its rows (`0xc0f6d8`, at most ten, `0x5a22f0`), the banner a turn-in sends and the arena walk's mode (`0xc0f7e8`..).
#[derive(Resource)]
pub(crate) struct PetitionVendorState {
    vendor: Option<u64>,
    rows: Vec<PetitionShowListEntry>,
    banner: ArenaBannerChoice,
    /// The arena walk's last mode, which its record callback runs again (`0x6dd630`).
    turn_in: bool,
}

impl Default for PetitionVendorState {
    fn default() -> Self {
        Self {
            vendor: None,
            rows: Vec::new(),
            banner: live::unset_banner(),
            turn_in: false,
        }
    }
}

/// The rows a vendor keeps (`0x5a22eb`).
const VENDOR_ROWS: usize = 10;

impl crate::ui_session::NpcSession for PetitionVendorState {
    fn npc(&self) -> Option<u64> {
        self.vendor
    }

    /// The zero alone; its event fires from [`close_vendor`].
    fn close(&mut self) {
        self.vendor = None;
    }
}

/// A callback a petition record's ask left (`0x5cec20`, `0x6dd360`, `0x6dd630`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// The open petition's, which holds the record and may fire `PETITION_SHOW` (`0x5cea60`).
    Open,
    /// The guild charter turn-in's, which runs the turn-in again (`0x6dd3a0`).
    GuildTurnIn,
    /// The arena walk's, which runs it again and fires `PETITION_VENDOR_UPDATE` on a find.
    ArenaTurnIn,
}

/// The petition window (`0xc22b20`..) and the petition-record cache.
#[derive(Resource, Default)]
pub(crate) struct PetitionState {
    /// The petition's guid, the charter item every verb in the family takes.
    petition: Option<u64>,
    /// The packet's owner (`0xc22b28`).
    owner: u64,
    /// The id the record callback compares (`0xc22b30`), zeroed by a close.
    petition_id: u32,
    /// Our sign went out (`0xc22b34`): a close sends no decline.
    signed: bool,
    /// The signers in wire order (`0xc22b44`, counted by `0xc22b38`).
    signers: Vec<u64>,
    /// The signer names asked with the window's callback, one per ask (`0xc22b3c`).
    pending: Vec<u64>,
    /// A record is held for the open petition (`0xc22b40`).
    held: bool,
    /// The record cache by petition id, asked once through [`QueryCache`].
    records: QueryCache<u32, Record>,
    /// The record callbacks still waiting, in ask order.
    waits: Vec<(u32, Wait)>,
}

/// One petition's record, from `SMSG_PETITION_QUERY_RESPONSE`.
#[derive(Clone, Debug, Default)]
struct Record {
    /// The record's owner (`+0x8`), not the packet's: `GetPetitionInfo`'s `isOriginator`, the
    /// close's decline and `CanSignPetition` compare against it.
    owner: u64,
    /// The proposed guild or team's name.
    name: String,
    body_text: String,
    /// Signatures required (`+0x1110`) and that fit (`+0x1114`), pushed signed.
    min_signatures: i32,
    max_signatures: i32,
    /// 0 a guild charter, 1 an arena one (`+0x13bc`).
    static_type: u32,
    /// The level window `OfferPetition` checks a target against (`0x5cf220`), 0 for none.
    min_level: u32,
    max_level: u32,
}

/// An arena charter's `static_type`.
const STATIC_TYPE_ARENA: u32 = benilla_protocol::messages::PETITION_TYPE_ARENA;

impl crate::query_cache::AskOnce for PetitionState {
    fn clear_pending(&mut self) {
        self.records.clear_pending();
    }
}

impl PetitionState {
    /// The record the window holds, `GetPetitionInfo`'s (`0xc22b40`).
    fn held_record(&self) -> Option<&Record> {
        self.held
            .then(|| self.records.get(self.petition_id))
            .flatten()
    }

    /// A record read with a callback: the cached record, else `None` with `wait` linked and the record asked once.
    fn record_or_wait(
        &mut self,
        id: u32,
        item: u64,
        wait: Wait,
        commands: &NetCommands,
    ) -> Option<&Record> {
        if self.records.get(id).is_none() {
            self.waits.push((id, wait));
            self.request_record(id, item, commands);
            return None;
        }
        self.records.get(id)
    }

    /// Ask once per petition id.
    fn request_record(&self, petition_id: u32, item: u64, commands: &NetCommands) {
        self.records.get_or_ask(petition_id, || {
            let _ = commands
                .0
                .send(ClientCommand::PetitionQuery { petition_id, item });
        });
    }

    /// The close leg of the window's setter (`0x5ceef0`): with a petition open, the decline unless our sign went out, no record is held or we own it (`0x5cef0d`-`0x5cef36`), the signers cleared (`0x5cef8b`) and the guids zeroed; true when it closed one, for the caller to fire `PETITION_CLOSED`.
    fn close_open(&mut self, me: u64, commands: &NetCommands) -> bool {
        let Some(item) = self.petition else {
            return false;
        };
        let owned = self.held_record().is_some_and(|r| r.owner == me);
        if !self.signed && self.held && !owned {
            let _ = commands.0.send(ClientCommand::PetitionDecline { item });
        }
        self.clear_signatures();
        self.petition = None;
        self.owner = 0;
        true
    }

    /// The store leg of the setter (`0x5cefb6`): the guids and id, the record held or asked with the window's callback, the latch cleared.
    fn store(&mut self, petition: Option<u64>, id: u32, owner: u64, commands: &NetCommands) {
        self.petition = petition;
        self.owner = owner;
        self.petition_id = id;
        self.signed = false;
        self.held = match petition {
            Some(item) => self
                .record_or_wait(id, item, Wait::Open, commands)
                .is_some(),
            None => false,
        };
    }

    /// The signer list's clear (`0x5cec60`): the name callbacks cancelled and the count zeroed.
    fn clear_signatures(&mut self) {
        self.pending.clear();
        self.signers.clear();
    }

    /// The charter tooltip's line 3, the guild name and its master: the hover asks for the record,
    /// so an unseen charter reads `None` until the answer repaints it.
    pub(crate) fn tooltip_view(
        &self,
        petition_id: u32,
        item: u64,
        names: &crate::names::NameCache,
        commands: &NetCommands,
    ) -> Option<benilla_ui::script::PetitionSlotView> {
        if petition_id == 0 {
            return None;
        }
        self.request_record(petition_id, item, commands);
        let rec = self.records.get(petition_id)?;
        Some(benilla_ui::script::PetitionSlotView {
            is_charter: rec.static_type <= 1,
            title: rec.name.clone(),
            owner: names.resolve(rec.owner, commands).map(str::to_string),
        })
    }
}

/// Fire a walk's steps in the handler's turn.
fn fire_steps(world: &mut World, steps: Vec<benilla_ui::script::PetitionStep>) {
    use benilla_ui::script::{PetitionArg, PetitionStep};
    let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    for s in steps {
        match s {
            PetitionStep::Fire(event) => ui.fire(event, vec![]),
            PetitionStep::Line(key, arg) => {
                let line = match arg {
                    PetitionArg::None => crate::ui_action::UiError::key(key),
                    PetitionArg::S(s) => crate::ui_action::UiError::s(key, s),
                    PetitionArg::D(d) => {
                        crate::ui_action::UiError::args(key, vec![crate::ui_action::FillArg::D(d)])
                    }
                };
                crate::ui_action::show_in_turn(&mut ui, "ui_petition", line);
            }
        }
    }
}

/// The registrar's close (`0x599e90`): the interact target cleared on it, whose interaction close runs the registrar leg first when it held it (`0x512ff1`), then the zero and `GUILD_REGISTRAR_CLOSED`.
pub(crate) fn close_registrar(world: &mut World) {
    let Some(npc) = world
        .get_resource::<GuildRegistrarState>()
        .and_then(|r| r.registrar)
    else {
        return;
    };
    crate::ui_session::clear_interact_target(world, npc);
    zero_registrar(world);
}

/// The interaction close's registrar leg with the target already zeroed (`0x512ff1`).
pub(crate) fn close_registrar_by_interaction(world: &mut World) {
    if world
        .get_resource::<GuildRegistrarState>()
        .is_some_and(|r| r.registrar.is_some())
    {
        zero_registrar(world);
    }
}

fn zero_registrar(world: &mut World) {
    if let Some(mut r) = world.get_resource_mut::<GuildRegistrarState>() {
        r.registrar = None;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("GUILD_REGISTRAR_CLOSED", vec![]);
    }
}

/// The vendor's close (`0x5a2880`): as the registrar's, through the vendor leg (`0x513083`).
pub(crate) fn close_vendor(world: &mut World) {
    let Some(npc) = world
        .get_resource::<PetitionVendorState>()
        .and_then(|v| v.vendor)
    else {
        return;
    };
    crate::ui_session::clear_interact_target(world, npc);
    zero_vendor(world);
}

/// The interaction close's vendor leg with the target already zeroed (`0x513083`).
pub(crate) fn close_vendor_by_interaction(world: &mut World) {
    if world
        .get_resource::<PetitionVendorState>()
        .is_some_and(|v| v.vendor.is_some())
    {
        zero_vendor(world);
    }
}

fn zero_vendor(world: &mut World) {
    if let Some(mut v) = world.get_resource_mut::<PetitionVendorState>() {
        v.vendor = None;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("PETITION_VENDOR_CLOSED", vec![]);
    }
}

/// The world entry's charter parts: the registrar's row (`0x528217` -> `0x599dd0`), the window's globals (`0x528221` -> `0x5cf580`) and the vendor's count and button (`0x528258` -> `0x5a1f50`), none firing.
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut r) = world.get_resource_mut::<GuildRegistrarState>() {
        r.row = PetitionShowListEntry::default();
    }
    if let Some(mut p) = world.get_resource_mut::<PetitionState>() {
        p.petition = None;
        p.owner = 0;
        p.petition_id = 0;
        p.signers.clear();
        p.pending.clear();
        p.held = false;
    }
    if let Some(mut v) = world.get_resource_mut::<PetitionVendorState>() {
        v.rows.clear();
    }
    net::forget_button(world);
}

/// The three resources, the handlers and the range guard.
pub(crate) struct UiPetitionPlugin;

impl Plugin for UiPetitionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<PetitionState>(app);
        app.init_resource::<GuildRegistrarState>()
            .init_resource::<PetitionVendorState>()
            .init_resource::<PetitionState>();
    }
}

#[cfg(test)]
mod tests;
