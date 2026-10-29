//! The charter sessions: the guild registrar, the arena registrar's petition vendor, the petition
//! window, and the verbs that found a guild or an arena team.
//!
//! `SMSG_PETITION_SHOWLIST` (`0x6d69f0`) opens the guild registrar for an NPC that is both a
//! petitioner and a tabard designer, and the petition vendor for any other; both are NPC sessions
//! that close on a walk-away. The petition window is bound to a charter item with no NPC, so
//! walking away leaves it open.
//!
//! `SMSG_PETITION_SHOW_SIGNATURES` carries no text: the name and requirement come from
//! `SMSG_PETITION_QUERY_RESPONSE`, the signer names from the name cache. It answers our request
//! and delivers an offer alike; only `owner` tells them apart.

use benilla_protocol::messages::{
    petition_result, PetitionQueryResponse, PetitionRename, PetitionShowList,
    PetitionShowListEntry, PetitionShowSignatures, PetitionSignResults,
};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::NpcSession;

mod feed;
mod lines;

/// The open registrar, apart from [`PetitionState`] so the walk-away guard closes only it.
#[derive(Resource, Default)]
pub(crate) struct GuildRegistrarState {
    open: Option<Registrar>,
}

#[derive(Clone, Debug)]
struct Registrar {
    /// What `BuyGuildCharter` addresses (`m_registrar`, `0xc0e570`).
    npc: u64,
    /// Price in copper, unsigned.
    cost: u32,
    /// The charter row's index, the bought record's `muid` (`0x59a116`).
    index: u32,
}

/// The `UNIT_NPC_FLAGS` bits that make a showlist's NPC the guild registrar, both of them
/// (`0x6d6aaf`); an NPC with fewer opens the petition vendor.
const REGISTRAR_NPC_FLAGS: u32 = crate::target::cursor_mode::npc_flags::PETITIONER
    | crate::target::cursor_mode::npc_flags::TABARDDESIGNER;

impl GuildRegistrarState {
    /// `SetRegistrar` (`0x599e00`): the first row is the registrar's charter.
    fn open(&mut self, list: &PetitionShowList) {
        let first = list.entries.first().cloned().unwrap_or_default();
        self.open = Some(Registrar {
            npc: list.npc,
            cost: first.charter_cost,
            index: first.index,
        });
    }

    /// The charter row's index, `CMSG_PETITION_BUY`'s `muid`.
    fn index(&self) -> u32 {
        self.open.as_ref().map_or(0, |r| r.index)
    }

    /// `GetGuildCharterCost()`: 0 with no registrar open.
    fn cost(&self) -> u32 {
        self.open.as_ref().map_or(0, |r| r.cost)
    }
}

impl NpcSession for GuildRegistrarState {
    fn npc(&self) -> Option<u64> {
        self.open.as_ref().map(|r| r.npc)
    }

    fn close(&mut self) {
        self.open = None;
    }
}

/// The arena registrar's session (`CGPetitionVendorInfo`, `0xc0f7c8`): its charter rows, up to
/// ten (`0x5a22f0`).
#[derive(Resource, Default)]
pub(crate) struct PetitionVendorState {
    npc: Option<u64>,
    items: Vec<PetitionShowListEntry>,
    /// The VM closed it itself and fired `PETITION_VENDOR_CLOSED` already.
    closed_by_vm: bool,
    /// `ShowPetitionList` fires `PETITION_VENDOR_UPDATE` with a vendor (`0x5a22e8`).
    update: bool,
}

/// The rows a vendor keeps (`0x5a2305`).
const VENDOR_ROWS: usize = 10;

impl PetitionVendorState {
    fn open(&mut self, list: PetitionShowList) {
        self.npc = Some(list.npc);
        self.items = list.entries;
        self.items.truncate(VENDOR_ROWS);
        self.update = true;
    }

    pub(crate) fn rows(&self) -> &[PetitionShowListEntry] {
        &self.items
    }
}

impl NpcSession for PetitionVendorState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.npc = None;
    }
}

/// The open charter and the petition-record cache.
#[derive(Resource, Default)]
pub(crate) struct PetitionState {
    open: Option<OpenCharter>,
    /// The record cache by petition id, asked once through [`QueryCache`].
    records: QueryCache<u32, Record>,
    /// Set while our sign is outstanding. A close's decline leg is gated on it, so closing while
    /// our own signature is in flight sends no decline.
    signing: bool,
    /// One step per landed or patched record. The charter tooltip's hover issues the record query,
    /// so `ui_items`' bag feed gates its rebuild on this or keeps the `None` it read first.
    records_epoch: u64,
    /// Lines composed at apply time, shown by the feed, which holds the `script` handle.
    lines: Vec<lines::Line>,
}

/// The charter the petition window is showing.
#[derive(Clone, Debug)]
struct OpenCharter {
    /// The charter item's guid, the handle every verb in the family takes.
    item: u64,
    /// The packet's owner; the originator's name and the close's decline test read it.
    owner: u64,
    petition_id: u32,
    /// In wire order; names resolve at feed time, so a late name repaints without a packet.
    signers: Vec<u64>,
}

/// One petition's record, from `SMSG_PETITION_QUERY_RESPONSE`.
#[derive(Clone, Debug, Default)]
struct Record {
    /// The record's owner (`+0x8`), not the packet's: `GetPetitionInfo`'s `isOriginator`
    /// compares against it, as does `CanSignPetition`'s owner refusal.
    owner: u64,
    /// The proposed guild or team's name.
    name: String,
    body_text: String,
    /// Signatures required (`+0x1110`) and that fit (`+0x1114`), pushed signed.
    min_signatures: i32,
    max_signatures: i32,
    /// 0 a guild charter, 1 an arena one (`+0x13bc`).
    static_type: u32,
}

/// An arena charter's `static_type`.
const STATIC_TYPE_ARENA: u32 = benilla_protocol::messages::PETITION_TYPE_ARENA;

impl crate::query_cache::AskOnce for PetitionState {
    fn clear_pending(&mut self) {
        self.records.clear_pending();
    }
}

impl PetitionState {
    /// `SMSG_PETITION_SHOW_SIGNATURES`: open this charter and ask for its record once.
    fn show(&mut self, sigs: PetitionShowSignatures, commands: &NetCommands) {
        let petition_id = sigs.petition_id;
        self.open = Some(OpenCharter {
            item: sigs.item,
            owner: sigs.owner,
            petition_id,
            signers: sigs.signatures.into_iter().map(|s| s.signer).collect(),
        });
        self.request_record(petition_id, sigs.item, commands);
    }

    /// Ask once per petition id; the feed's next rebuild reads the answer.
    fn request_record(&self, petition_id: u32, item: u64, commands: &NetCommands) {
        self.records.get_or_ask(petition_id, || {
            let _ = commands
                .0
                .send(ClientCommand::PetitionQuery { petition_id, item });
        });
    }

    /// One step per landed or patched record.
    pub(crate) fn records_epoch(&self) -> u64 {
        self.records_epoch
    }

    /// `SMSG_PETITION_QUERY_RESPONSE`: fill the cache.
    fn apply_record(&mut self, response: PetitionQueryResponse) {
        self.records_epoch = self.records_epoch.wrapping_add(1);
        self.records.insert(
            response.petition_id,
            Some(Record {
                owner: response.owner,
                name: response.name,
                body_text: response.body_text,
                min_signatures: response.min_signatures as i32,
                max_signatures: response.max_signatures as i32,
                static_type: response.static_type,
            }),
        );
    }

    /// The charter tooltip's line 3, the guild name and its master: the hover asks for the record,
    /// so an unseen charter reads `None` until the answer repaints it.
    pub(crate) fn tooltip_view(
        &mut self,
        petition_id: u32,
        item: u64,
        names: &NameCache,
        commands: &NetCommands,
    ) -> Option<benilla_ui::script::PetitionSlotView> {
        if petition_id == 0 {
            return None;
        }
        self.request_record(petition_id, item, commands);
        let rec = self.records.get(petition_id)?;
        let (is_charter, title, owner_guid) = (rec.static_type <= 1, rec.name.clone(), rec.owner);
        Some(benilla_ui::script::PetitionSlotView {
            is_charter,
            title,
            owner: names.resolve(owner_guid, commands).map(str::to_string),
        })
    }

    /// The owner's half of a sign: append the signer, as the reference does, since no fresh list
    /// comes. Returns the name if cached, the condition for `ERR_PETITION_SIGNED_S`.
    fn append_signer(
        &mut self,
        item: u64,
        signer: u64,
        names: &NameCache,
        commands: &NetCommands,
    ) -> Option<String> {
        let open = self.open.as_mut().filter(|o| o.item == item)?;
        if !open.signers.contains(&signer) {
            open.signers.push(signer);
        }
        names.resolve(signer, commands).map(str::to_string)
    }

    /// The `MSG_PETITION_DECLINE` `ClosePetition()` sends, only when a petition is open, no sign of
    /// ours is in flight, a record is cached, and we are not the owner. The reference reads that
    /// owner off the cached record; this reads the packet's.
    fn decline_on_close(&mut self, me: u64, commands: &NetCommands) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if self.signing || !self.records.answered(open.petition_id) || open.owner == me {
            return;
        }
        let _ = commands
            .0
            .send(ClientCommand::PetitionDecline { item: open.item });
    }

    /// The local half of `ClosePetition()`; the decline, if any, is `decline_on_close`.
    fn close(&mut self) {
        self.open = None;
    }

    /// The session end, with no decline. The record cache stays: it is keyed by the server's
    /// petition id, and the world-enter release re-arms its asks.
    fn clear_session(&mut self) {
        self.open = None;
        self.signing = false;
        self.lines.clear();
    }

    /// What `SignPetition`, `OfferPetition` and `RenamePetition` address.
    fn open_item(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.item)
    }

    /// The open charter's team size for `CMSG_OFFER_PETITION` (`0x5cf3a4`): an arena record's
    /// `min_signatures + 1`, 0 for any other.
    fn offer_team_size(&self) -> u32 {
        self.open
            .as_ref()
            .and_then(|o| self.records.get(o.petition_id))
            .filter(|r| r.static_type == STATIC_TYPE_ARENA)
            .map_or(0, |r| (r.min_signatures + 1) as u32)
    }

    /// An arena charter whose record is cached and, for a turn-in of `team_size`, whose
    /// requirement is `team_size − 1` (`TurnInArenaCharter`, `0x6dd680`); the item's record is
    /// asked once on the way.
    pub(crate) fn is_arena_charter(
        &self,
        petition_id: u32,
        item: u64,
        team_size: Option<u32>,
        commands: &NetCommands,
    ) -> bool {
        self.request_record(petition_id, item, commands);
        self.records.get(petition_id).is_some_and(|r| {
            r.static_type == STATIC_TYPE_ARENA
                && team_size.is_none_or(|n| r.min_signatures + 1 == n as i32)
        })
    }
}

/// The two resources, the registrar's walk-away guard, and the feed and drain.
pub(crate) struct UiPetitionPlugin;

impl Plugin for UiPetitionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<PetitionState>(app);
        app.init_resource::<GuildRegistrarState>()
            .init_resource::<PetitionVendorState>()
            .init_resource::<PetitionState>()
            .add_systems(
                Update,
                (
                    // Before the feed, so a walk-away fires GUILD_REGISTRAR_CLOSED the same frame.
                    crate::ui_session::close_npc_session_out_of_range::<GuildRegistrarState>
                        .before(feed::feed_petition),
                    crate::ui_session::close_npc_session_out_of_range::<PetitionVendorState>
                        .before(feed::feed_petition),
                    feed::feed_petition.in_set(UiFeed),
                    feed::drain_petition.after(UiInput),
                ),
            );
    }
}

/// The inbound arms, one per packet. None fires an event or prints: lines queue on
/// [`PetitionState::lines`] for the feed, since a packet handler has no `script` handle.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    use crate::net::{GuidIndex, ObjectStore, SelfGuid};
    use crate::ui_social::SocialState;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PetitionShowList, on_show_list)
            .net_handler(K::PetitionShowSignatures, on_show_signatures)
            .net_handler(K::PetitionQueryResponse, on_query_response)
            .net_handler(K::PetitionSignResults, on_sign_results)
            .net_handler(K::TurnInPetitionResults, on_turn_in_results)
            .net_handler(K::PetitionDeclined, on_declined)
            .net_handler(K::PetitionRenamed, on_renamed)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The charter and the registrar end with the session, or the next login shows the old charter
    /// and closing it sends a decline; the walk-away guard has no self player to measure from.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        mut registrar: ResMut<GuildRegistrarState>,
        mut vendor: ResMut<PetitionVendorState>,
    ) {
        petition.clear_session();
        registrar.close();
        vendor.close();
    }

    /// The flags gate reads the live NPC; an unstreamed guid fails, as the client's resolve does.
    fn on_show_list(
        In(ev): In<SessionEvent>,
        mut registrar: ResMut<GuildRegistrarState>,
        mut vendor: ResMut<PetitionVendorState>,
        index: Res<GuidIndex>,
        stores: Query<&ObjectStore>,
    ) {
        if let SessionEvent::PetitionShowList(list) = ev {
            let flags = index
                .0
                .get(&list.npc)
                .and_then(|e| stores.get(*e).ok())
                .map(|s| s.0.unit_npc_flags());
            show_list(&mut registrar, &mut vendor, list, flags);
        }
    }

    /// The ignore list is consulted before anything else.
    fn on_show_signatures(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        social: Res<SocialState>,
        commands: Res<NetCommands>,
    ) {
        if let SessionEvent::PetitionShowSignatures(sigs) = ev {
            let ignored = social.is_ignored(sigs.owner);
            show_signatures(&mut petition, sigs, ignored, &commands);
        }
    }

    fn on_query_response(In(ev): In<SessionEvent>, mut petition: ResMut<PetitionState>) {
        if let SessionEvent::PetitionQueryResponse(response) = ev {
            query_response(&mut petition, response);
        }
    }

    fn on_sign_results(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        names: Res<NameCache>,
        self_guid: Res<SelfGuid>,
        commands: Res<NetCommands>,
    ) {
        if let SessionEvent::PetitionSignResults(results) = ev {
            sign_results(
                &mut petition,
                &names,
                self_guid.0.unwrap_or(0),
                results,
                &commands,
            );
        }
    }

    fn on_turn_in_results(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        mut registrar: ResMut<GuildRegistrarState>,
    ) {
        if let SessionEvent::TurnInPetitionResults { result } = ev {
            turn_in_results(&mut petition, &mut registrar, result);
        }
    }

    fn on_declined(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        names: Res<NameCache>,
    ) {
        if let SessionEvent::PetitionDeclined { player } = ev {
            declined(&mut petition, &names, player);
        }
    }

    fn on_renamed(In(ev): In<SessionEvent>, mut petition: ResMut<PetitionState>) {
        if let SessionEvent::PetitionRenamed(rename) = ev {
            renamed(&mut petition, rename);
        }
    }

    /// `SMSG_PETITION_SHOWLIST` (`0x6d69f0`); `npc_flags` is the NPC's live `UNIT_NPC_FLAGS`. An
    /// NPC that does not resolve as a unit drops the list.
    pub(crate) fn show_list(
        registrar: &mut GuildRegistrarState,
        vendor: &mut PetitionVendorState,
        list: PetitionShowList,
        npc_flags: Option<u32>,
    ) {
        let Some(flags) = npc_flags else {
            debug!(
                "ui_petition: showlist from unstreamed {:#x} — dropped",
                list.npc
            );
            return;
        };
        if flags & REGISTRAR_NPC_FLAGS == REGISTRAR_NPC_FLAGS {
            registrar.open(&list);
        } else {
            vendor.open(list);
        }
    }

    /// `SMSG_PETITION_SHOW_SIGNATURES`: our charter or one offered to us. An ignored owner
    /// suppresses the whole update, silently.
    pub(crate) fn show_signatures(
        petition: &mut PetitionState,
        sigs: PetitionShowSignatures,
        owner_ignored: bool,
        commands: &NetCommands,
    ) {
        if owner_ignored {
            debug!(
                "ui_petition: charter from ignored owner {:#x} — whole update dropped",
                sigs.owner
            );
            return;
        }
        petition.show(sigs, commands);
    }

    /// `SMSG_PETITION_QUERY_RESPONSE`: the record cache fill.
    pub(crate) fn query_response(petition: &mut PetitionState, response: PetitionQueryResponse) {
        petition.apply_record(response);
    }

    /// `SMSG_PETITION_SIGN_RESULTS`: signer and owner get identical copies, told apart by the
    /// player guid. Another's signature is appended, result unread; ours prints its line, clears
    /// the latch, and on success closes the window.
    pub(crate) fn sign_results(
        petition: &mut PetitionState,
        names: &NameCache,
        self_guid: u64,
        results: PetitionSignResults,
        commands: &NetCommands,
    ) {
        if results.player != self_guid {
            let cached = petition.append_signer(results.item, results.player, names, commands);
            if let Some(name) = cached {
                petition.lines.push(lines::signed_by_other(&name));
            }
            return;
        }
        petition.signing = false;
        if let Some(line) = lines::my_sign_line(results.result) {
            petition.lines.push(line);
        }
        if results.result == petition_result::OK {
            petition.close();
        }
    }

    /// `SMSG_TURN_IN_PETITION_RESULTS`: success prints nothing and closes the registrar (the
    /// reference fires `GUILD_REGISTRAR_CLOSED`); only the two refusals print, both red.
    pub(crate) fn turn_in_results(
        petition: &mut PetitionState,
        registrar: &mut GuildRegistrarState,
        result: u32,
    ) {
        if result == petition_result::OK {
            // The server destroyed the charter, so the item-bound window closes too.
            petition.close();
            registrar.close();
            return;
        }
        if let Some(line) = lines::turn_in_line(result) {
            petition.lines.push(line);
        }
    }

    /// `MSG_PETITION_DECLINE` inbound: a line only if the decliner's name is already cached, with
    /// no query or retry, so this arm takes no command channel.
    pub(crate) fn declined(petition: &mut PetitionState, names: &NameCache, player: u64) {
        if let Some(name) = names.peek(player) {
            let line = lines::declined_line(name);
            petition.lines.push(line);
        }
    }

    /// `MSG_PETITION_RENAME` inbound: the echo's name overwrites the cached title in place, with no
    /// re-query (which then fires `PETITION_SHOW`).
    pub(crate) fn renamed(petition: &mut PetitionState, rename: PetitionRename) {
        let Some(id) = petition
            .open
            .as_ref()
            .filter(|o| o.item == rename.item)
            .map(|o| o.petition_id)
        else {
            return;
        };
        if !petition.records.answered(id) {
            petition.records.insert(id, Some(Record::default()));
        }
        if let Some(record) = petition.records.get_mut(id) {
            record.name = rename.name;
        }
        // A patched title is a changed record: the tooltip's line 3 reads it too.
        petition.records_epoch = petition.records_epoch.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::PetitionSignature;
    use crossbeam_channel::unbounded;

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = unbounded();
        (NetCommands(tx), rx)
    }

    fn show_list(npc: u64, rows: &[u32]) -> PetitionShowList {
        PetitionShowList {
            npc,
            entries: rows
                .iter()
                .enumerate()
                .map(|(i, cost)| PetitionShowListEntry {
                    index: i as u32 + 1,
                    charter_entry: 5863,
                    charter_display_id: 16161,
                    charter_cost: *cost,
                    charter_type: 0,
                    required_signatures: 9,
                })
                .collect(),
        }
    }

    /// The open charter's cached title, `GetPetitionInfo`'s second return.
    fn open_title(p: &PetitionState) -> Option<&str> {
        let open = p.open.as_ref()?;
        p.records.get(open.petition_id).map(|r| r.name.as_str())
    }

    fn signatures(item: u64, owner: u64, id: u32, signers: &[u64]) -> PetitionShowSignatures {
        PetitionShowSignatures {
            item,
            owner,
            petition_id: id,
            signatures: signers
                .iter()
                .map(|s| PetitionSignature {
                    signer: *s,
                    unknown: 0,
                })
                .collect(),
        }
    }

    /// Both flags open the guild registrar on the first row; any fewer the petition vendor.
    #[test]
    fn the_showlist_picks_the_registrar_or_the_vendor_by_npc_flags() {
        let mut r = GuildRegistrarState::default();
        let mut v = PetitionVendorState::default();
        net::show_list(
            &mut r,
            &mut v,
            show_list(0x2a1f, &[1000]),
            Some(REGISTRAR_NPC_FLAGS),
        );
        assert_eq!((r.npc(), r.cost(), r.index()), (Some(0x2a1f), 1000, 1));
        assert_eq!(v.npc(), None);

        let mut r = GuildRegistrarState::default();
        net::show_list(
            &mut r,
            &mut v,
            show_list(0x77, &[800_000, 1_200_000, 2_000_000]),
            Some(crate::target::cursor_mode::npc_flags::PETITIONER),
        );
        assert_eq!(r.npc(), None);
        assert_eq!((v.npc(), v.rows().len()), (Some(0x77), 3));

        let mut v = PetitionVendorState::default();
        net::show_list(&mut r, &mut v, show_list(0x78, &[1]), None);
        assert_eq!(
            (r.npc(), v.npc()),
            (None, None),
            "an unstreamed NPC drops the list"
        );
    }

    #[test]
    fn walking_away_from_the_registrar_leaves_an_open_charter_alone() {
        let (commands, _rx) = commands();
        let mut registrar = GuildRegistrarState::default();
        let mut petition = PetitionState::default();
        registrar.open(&show_list(0x2a1f, &[1000]));
        petition.show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);

        registrar.close();
        assert_eq!(registrar.npc(), None);
        assert_eq!(
            petition.open_item(),
            Some(0x99),
            "the charter window is item-bound and survives the walk-away"
        );
    }

    #[test]
    fn the_record_query_is_asked_once_per_petition() {
        let (commands, rx) = commands();
        let mut petition = PetitionState::default();
        petition.show(signatures(0x99, 0xaa, 7, &[]), &commands);
        petition.show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);
        let sent: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            sent.len(),
            1,
            "one query for two opens of the same petition"
        );
        assert!(matches!(
            sent[0],
            ClientCommand::PetitionQuery {
                petition_id: 7,
                item: 0x99
            }
        ));

        // The answer landing does not re-arm the ask.
        petition.apply_record(PetitionQueryResponse {
            petition_id: 7,
            name: "Legacy".into(),
            min_signatures: 9,
            ..Default::default()
        });
        petition.show(signatures(0x99, 0xaa, 7, &[0xbb, 0xcc]), &commands);
        assert!(rx.try_iter().next().is_none(), "still no second query");
        assert_eq!(open_title(&petition), Some("Legacy"));
    }

    #[test]
    fn a_rename_echo_patches_the_cached_record() {
        let (commands, _rx) = commands();
        let mut petition = PetitionState::default();
        petition.show(signatures(0x99, 0xaa, 7, &[]), &commands);
        petition.apply_record(PetitionQueryResponse {
            petition_id: 7,
            name: "First".into(),
            min_signatures: 9,
            ..Default::default()
        });
        net::renamed(
            &mut petition,
            PetitionRename {
                item: 0x99,
                name: "Second".into(),
            },
        );
        assert_eq!(open_title(&petition), Some("Second"));

        // An echo for a charter we do not have open is not ours to apply.
        net::renamed(
            &mut petition,
            PetitionRename {
                item: 0xdead,
                name: "Wrong".into(),
            },
        );
        assert_eq!(open_title(&petition), Some("Second"));
    }

    /// Through the real registration: a new login's VM never runs the old one's `OnHide`.
    #[test]
    fn the_session_end_closes_the_charter_and_forgets_the_sign() {
        let (commands, _rx) = commands();
        let mut app = App::new();
        app.add_plugins(UiPetitionPlugin);
        {
            let mut petition = app.world_mut().resource_mut::<PetitionState>();
            petition.show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);
            petition.signing = true;
        }
        app.world_mut()
            .resource_mut::<GuildRegistrarState>()
            .open(&show_list(0x2a1f, &[1000]));

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        assert_eq!(app.world().resource::<GuildRegistrarState>().npc(), None);
        let petition = app.world().resource::<PetitionState>();
        assert_eq!(
            petition.open_item(),
            None,
            "no charter carried into the next login"
        );
        assert!(
            !petition.signing,
            "no sign of the old session still in flight"
        );
    }
}
