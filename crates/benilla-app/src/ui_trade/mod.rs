//! The trade window: [`net`] folds the two status packets into [`TradeSession`], [`feed_trade`]
//! pushes it into the VM and fires the trade events, [`drain_trade`] sends the Lua intents, and
//! [`answer_trade_request`] answers an incoming request.

use benilla_protocol::messages::{TradeItem, TradeStatusExtended, TRADE_SLOT_COUNT};
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, TradeSideState, TradeSlotItem, TradeState, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::target::Selection;
use crate::ui_party::GroupState;
use crate::ui_script::{UiDrain, UiFeed};
use crate::ui_session::NpcSession;

mod net;

/// One side's offer: slots 0-5 change hands, slot 6 is the non-traded enchant slot.
#[derive(Default)]
struct TradeOffer {
    slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    gold: u32,
    /// The spell on this side's enchant slot, 0 for none; stored, not shown yet.
    enchant_spell_id: u32,
}

/// The trade session [`net`] fills and [`feed_trade`] reads, cleared on a close and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct TradeSession {
    /// Set at our initiate or the ladder's accept, since `OPEN_WINDOW` carries no guid.
    partner: Option<u64>,
    /// Where each of our offered items sits, in the cursor's space.
    our_sources: [Option<(i64, u32)>; TRADE_SLOT_COUNT],
    /// An unanswered incoming `BEGIN_TRADE`, the initiator's guid; not yet a trade.
    request: Option<u64>,
    /// Our `CMSG_INITIATE_TRADE` is live: the reference's initiate latch, set only as it goes out
    /// and cleared by the status handler's tail, which codes 1, 2, 4, 7, 9 and 22 skip, so it
    /// survives `OPEN_WINDOW` for the life of a trade we opened.
    initiated: bool,
    /// Our initiate asked the open to take what the cursor holds (`0xca0fe4`, written at `0x703d90`).
    use_cursor: bool,
    /// `OPEN_WINDOW` arrived; `partner` is set before it.
    open: bool,
    /// `OPEN_WINDOW`'s trade id `[0xca0b68]`, which every snapshot must carry.
    trade_id: u32,
    /// Our own offer changes sent this trade, from 1 at the open `[0xca0b7c]`.
    own_state: u32,
    /// The partner offer's state from its last snapshot `[0xca0b70]`, echoed by our accept.
    their_state: u32,
    our: TradeOffer,
    their: TradeOffer,
    /// We pressed Trade: optimistic, dropped by `BACK_TO_TRADE`, an offer change or a close.
    our_accept: bool,
    /// The partner pressed Trade (`TRADE_ACCEPT`).
    their_accept: bool,
    /// The own-window slots the server's last snapshot filled (`0xca0b80`), which a snapshot missing one denies (`0x704810`, `0x704873`).
    own_mirror: [bool; TRADE_SLOT_COUNT],
    /// `CloseTrade` closed the window and signalled `TRADE_CLOSED` itself (`0x5879bc`).
    verb_closed: bool,
    /// Owed lines whose `%s` names a player, parked until [`NameCache`] answers. The reference
    /// prints `ERR_INITIATE_TRADE_S` from its name cache in the sender; the lines survive
    /// [`Self::close_window`], since that callback hangs off the name query, not the trade.
    named_lines: Vec<NamedLine>,
}

/// What a snapshot did to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfferUpdate {
    Applied,
    /// An own snapshot older than our last sent change, dropped (`0x7046d7`).
    Stale,
    /// Not the open trade's id: the reference sends `CMSG_CANCEL_TRADE` (`0x7046b6`).
    OtherTrade,
}

/// A line owed whose `%s` names a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NamedLine {
    /// The `GlobalStrings` key; the catalog names its surface.
    pub(crate) key: &'static str,
    /// Whose name fills the `%s`.
    pub(crate) who: u64,
    /// Print only while the player is streamed: the status arms read the live unit after an object
    /// lookup of `(guid, TYPEMASK_PLAYER)`, while `ERR_INITIATE_TRADE_S` goes through the name
    /// cache with no object test.
    pub(crate) needs_live_object: bool,
}

impl TradeSession {
    /// Our `CMSG_INITIATE_TRADE` goes out (`0x703cf0`): a fresh session with the target as partner, and whether the open takes the cursor's payload (`0x703d90`).
    pub(crate) fn initiate(&mut self, target: u64, use_cursor: bool) {
        *self = TradeSession {
            partner: Some(target),
            initiated: true,
            use_cursor,
            named_lines: vec![NamedLine {
                key: "ERR_INITIATE_TRADE_S",
                who: target,
                needs_live_object: false,
            }],
            ..Default::default()
        };
    }

    /// `BEGIN_TRADE`: record the request beside any live session, unanswered;
    /// [`answer_trade_request`] replies, and only [`Self::begin`] resets the session.
    pub(crate) fn request(&mut self, initiator: u64) {
        self.request = Some(initiator);
    }

    pub(crate) fn pending_request(&self) -> Option<u64> {
        self.request
    }

    /// Spend the pending request alone: refusing an offer does not cancel the trade you are in.
    pub(crate) fn refuse_request(&mut self) {
        self.request = None;
    }

    /// Accept the pending request as a fresh session; the server then opens both windows.
    pub(crate) fn begin(&mut self, partner: u64) {
        *self = TradeSession {
            partner: Some(partner),
            ..Default::default()
        };
    }

    /// The open's cursor test (`0x703c30`): our own initiate stands and it asked for the cursor.
    pub(crate) fn opens_with_cursor(&self) -> bool {
        self.initiated && self.use_cursor
    }

    /// `OPEN_WINDOW` (`0x704517`): both offers empty, both states 1, the trade id kept, and the partner setter's open drops both accepts (`0x587143`, `0x587149`); the feed's open edge fires `TRADE_SHOW`.
    pub(crate) fn open_window(&mut self, trade_id: u32) {
        self.open = true;
        self.our_accept = false;
        self.their_accept = false;
        self.trade_id = trade_id;
        self.own_state = 1;
        self.their_state = 1;
        self.our = TradeOffer::default();
        self.their = TradeOffer::default();
    }

    /// `SMSG_TRADE_STATUS_EXTENDED` as `0x704680` takes it: another trade's snapshot is answered
    /// with a cancel, an own snapshot behind our sent changes is dropped, and otherwise the side's
    /// offer is replaced whole, which drops both accepts.
    pub(crate) fn set_offer(&mut self, ext: &TradeStatusExtended) -> OfferUpdate {
        if ext.trade_id != self.trade_id {
            return OfferUpdate::OtherTrade;
        }
        if !ext.their_window && ext.state_index < self.own_state {
            return OfferUpdate::Stale;
        }
        let side = if ext.their_window {
            self.their_state = ext.side_state;
            &mut self.their
        } else {
            &mut self.our
        };
        side.slots = ext.slots;
        side.gold = ext.gold;
        side.enchant_spell_id = ext.enchant_spell_id;
        self.our_accept = false;
        self.their_accept = false;
        OfferUpdate::Applied
    }

    /// One own change went out (`CMSG_SET_TRADE_ITEM`, `_CLEAR_TRADE_ITEM`, `_SET_TRADE_GOLD`),
    /// each of which counts `[0xca0b7c]` up (`0x70415f`, `0x7041dc`, `0x70444f`).
    pub(crate) fn own_change_sent(&mut self) {
        self.own_state += 1;
    }

    /// The partner's item in the not-traded slot, whose change releases the enchanting item (`0x586f9e`).
    pub(crate) fn their_seventh(&self) -> Option<TradeItem> {
        self.their.slots[6]
    }

    /// The permanent enchant on the partner's item in the not-traded slot; `None` with no item.
    pub(crate) fn their_enchant_slot(&self) -> Option<u32> {
        self.their.slots[6].as_ref().map(|i| i.perm_enchant)
    }

    /// Whether our offer holds the item at `(bag, slot)` (`0x5864d0`).
    pub(crate) fn offers(&self, bag: i64, slot: u32) -> bool {
        self.our_sources.contains(&Some((bag, slot)))
    }

    /// The partner offer state our `CMSG_ACCEPT_TRADE` carries.
    pub(crate) fn their_state(&self) -> u32 {
        self.their_state
    }

    /// Place our item in trade slot `id_1based` client-side: vmangos sends a placement to the
    /// partner only (`TradeData.cpp:81`), never back to the placer.
    pub(crate) fn place_own_item(&mut self, id_1based: u32, item: TradeItem) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = Some(item);
            self.our_offer_changed();
        }
    }

    /// Clear our trade slot `id_1based` client-side.
    pub(crate) fn clear_own_item(&mut self, id_1based: u32) {
        if let Some(slot) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our.slots.get_mut(i as usize))
        {
            *slot = None;
            self.our_sources[id_1based as usize - 1] = None;
            self.our_offer_changed();
        }
    }

    /// Set our gold client-side: vmangos sends it to the partner only (`TradeData.cpp:118`).
    pub(crate) fn set_own_gold(&mut self, copper: u32) {
        self.our.gold = copper;
        self.our_offer_changed();
    }

    /// Any change to our offer drops both accepts, as vmangos does.
    fn our_offer_changed(&mut self) {
        self.our_accept = false;
        self.their_accept = false;
    }

    /// `TRADE_ACCEPT`: the partner pressed Trade.
    pub(crate) fn partner_accepted(&mut self) {
        self.their_accept = true;
    }

    /// `BACK_TO_TRADE`: an accept was withdrawn (an offer change, the partner's un-accept, or the
    /// 200 ms scam-delay bounce), so both drop.
    pub(crate) fn back_to_trade(&mut self) {
        self.our_accept = false;
        self.their_accept = false;
    }

    /// We pressed Trade: the optimistic local accept beside `CMSG_ACCEPT_TRADE`.
    pub(crate) fn accept(&mut self) {
        self.our_accept = true;
    }

    /// Our accept flag, as the reference keeps it: its unaccept reads the flag before it changes
    /// it or sends `CMSG_UNACCEPT_TRADE`.
    pub(crate) fn we_accepted(&self) -> bool {
        self.our_accept
    }

    /// `CancelTradeAccept`: the local side of `CMSG_UNACCEPT_TRADE`.
    pub(crate) fn unaccept(&mut self) {
        self.our_accept = false;
    }

    /// An applied own-window snapshot's denials (`0x704810`, `0x704873`): each slot the last one filled and this one leaves empty, as the mirror moves to this one.
    pub(crate) fn mirror_own(&mut self, ext: &TradeStatusExtended) -> Vec<u8> {
        let mut denied = Vec::new();
        for (i, slot) in ext.slots.iter().enumerate() {
            if self.own_mirror[i] && slot.is_none() {
                denied.push(i as u8);
                self.our_sources[i] = None;
            }
            self.own_mirror[i] = slot.is_some();
        }
        denied
    }

    /// `CloseTrade` closed the window in its call.
    fn closed_by_verb(&mut self) {
        self.verb_closed = self.open;
    }

    fn take_verb_closed(&mut self) -> bool {
        std::mem::take(&mut self.verb_closed)
    }

    /// Park a line that names a player.
    pub(crate) fn owe_named_line(&mut self, line: NamedLine) {
        self.named_lines.push(line);
    }

    /// Take everything owed; the caller parks back what it could not name yet.
    fn take_named_lines(&mut self) -> Vec<NamedLine> {
        std::mem::take(&mut self.named_lines)
    }

    /// The name a `%s` status line carries, or `None` when the reference prints nothing: cases 0, 5
    /// and 14 read the guid cell under the latch, both of which `InitiateTrade` sets, while the
    /// `BEGIN_TRADE` arm fills only the cell. So a refuser stays silent when vmangos echoes
    /// `IGNORE_YOU` to both sides (`TradeHandler.cpp:46`). The live-object guard rides
    /// [`NamedLine::needs_live_object`].
    pub(crate) fn line_names(&self) -> Option<u64> {
        self.initiated.then_some(self.partner).flatten()
    }

    /// Close the window, the reference's trade-partner reset to `(0, 0)`, from `CANCELED`,
    /// `COMPLETE`, `CLOSE_WINDOW` or the local `CloseTrade`: a full reset, a superset of
    /// [`Self::clear_pending`], keeping the lines the reference raises before it,
    /// [`Self::named_lines`].
    pub(crate) fn close_window(&mut self) {
        *self = TradeSession {
            verb_closed: self.verb_closed,
            named_lines: std::mem::take(&mut self.named_lines),
            ..Default::default()
        };
    }

    /// The status handler's tail: clear the initiate latch and the pending guid, on every code but
    /// 1, 2, 4, 7, 9 and 22, after the arm has read them. It never touches the window. `partner`
    /// stands for the pending guid only while the window is shut; once open it is the window
    /// partner, which stays.
    pub(crate) fn clear_pending(&mut self) {
        self.initiated = false;
        self.request = None;
        if !self.open {
            self.partner = None;
        }
    }

    /// Code 13's packet arm (`0x704584`): both offers' coins zero.
    pub(crate) fn zero_gold(&mut self) {
        self.our.gold = 0;
        self.their.gold = 0;
    }

    /// `TRADE_REJECTED` (case 9): drop the partner's accept and nothing else; vmangos never sends
    /// it.
    pub(crate) fn partner_unaccepted(&mut self) {
        self.their_accept = false;
    }

    /// `ONLY_CONJURED` and `NOT_ELIGIBLE` (cases 22 and 23, `0x587727`/`0x587742` → `0x586410`):
    /// empty the offending slot of our offer, or the gold for `0xff` (`0x7045eb`); the feed's diff
    /// raises the repaint events. Deviation: an
    /// out-of-range slot is dropped, where the reference indexes its 7-slot mirror unchecked,
    /// because an out-of-bounds write is not a behaviour to reproduce.
    pub(crate) fn bounce_own_offer(&mut self, slot: u8) {
        const MONEY: u8 = 0xff;
        if slot == MONEY {
            self.our.gold = 0;
        } else if let Some(cell) = self.our.slots.get_mut(usize::from(slot)) {
            *cell = None;
            self.our_sources[usize::from(slot)] = None;
        }
    }

    /// Disconnect: a full reset, owed signals included; a session teardown is not a trade event.
    pub(crate) fn clear_session(&mut self) {
        *self = TradeSession::default();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    // ── Test accessors, for `net`'s tests ──

    #[cfg(test)]
    pub(crate) fn take_named_lines_for_test(&mut self) -> Vec<NamedLine> {
        self.take_named_lines()
    }

    #[cfg(test)]
    pub(crate) fn partner_has_accepted(&self) -> bool {
        self.their_accept
    }

    /// Our own offer as `(gold, which slots are filled)`.
    #[cfg(test)]
    pub(crate) fn own_offer_for_test(&self) -> (u32, [bool; TRADE_SLOT_COUNT]) {
        (self.our.gold, self.our.slots.map(|s| s.is_some()))
    }
}

/// The `"npc"` portrait points at the partner only while the window is open.
impl NpcSession for TradeSession {
    fn npc(&self) -> Option<u64> {
        self.open.then_some(self.partner).flatten()
    }

    fn close(&mut self) {
        self.close_window();
    }
}

/// The interaction close's trade leg (`0x512ea4`): the partner setter with none, its target already zeroed, so its clear finds none (`0x5870f7`); the window closes and `CMSG_CANCEL_TRADE` goes out (`0x587116` -> `0x704040`).
pub(crate) fn close_by_interaction(world: &mut World) {
    if let Some(mut trade) = world.get_resource_mut::<TradeSession>() {
        trade.close_window();
    }
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(ClientCommand::CancelTrade);
    }
}

/// The world leave's trade part (`0x528d05` -> `0x5880c0`): the offer's items unlock, an open window releases the enchanting item, drops the interact target and closes, and the cancel goes out twice.
pub(crate) fn leave_world(world: &mut World) {
    use crate::ui_session::NpcSession;
    let partner = world
        .get_resource::<TradeSession>()
        .and_then(|trade| trade.npc());
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        ui.write(UiScript::unlock_trade_items);
        if let Some(partner) = partner {
            ui.write(|script| script.close_trade_partner(false));
            ui.world(move |w| crate::ui_session::clear_interact_target(w, partner));
        }
    }
    if let Some(mut trade) = world.get_resource_mut::<TradeSession>() {
        trade.close_window();
    }
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(ClientCommand::CancelTrade);
        let _ = net.0.send(ClientCommand::CancelTrade);
    }
}

/// `ITEM_FIELD_FLAGS`' trade-window bit (`0x708b66`).
const ITEM_FLAG_BOP_TRADEABLE: u32 = 0x100;

/// The template bit the binding-enchant test returns on (`0x7073f7`).
const ITEM_FLAG_PETITION: u32 = 0x2000;

/// The offer gates' reads off an item's fields, its template and, for a bag, the objects it holds (`0x708520`, `0x708b40`, `0x7073e0`, `0x587b2c`, `0x5125b0`, `0x707c60`).
pub(crate) fn item_facts(
    fields: &benilla_protocol::ObjectFields,
    template: Option<&benilla_protocol::messages::ItemInfo>,
    me: Option<u64>,
    enchants: Option<&crate::items::Enchants>,
    spells: Option<&crate::ui_action::Spells>,
    held: &dyn Fn(u64) -> bool,
) -> benilla_ui::script::TradeItemFacts {
    let flags = fields.item_flags().unwrap_or(0);
    let enchant_ids =
        || (0..12u8).filter_map(|slot| crate::items::live_enchant(fields, slot, enchants));
    let soulbound_enchant = template.is_none_or(|t| t.flags & ITEM_FLAG_PETITION == 0)
        && enchant_ids().any(|id| crate::items::binds(id, enchants));
    // The bag's own count is of slots holding an item object (`0x5125b0`).
    let bag_items = fields.is_container().then(|| {
        let slots = fields.container_num_slots().unwrap_or(0).min(36) as u8;
        (0..slots)
            .filter(|&i| fields.container_slot(i).is_some_and(|g| g != 0 && held(g)))
            .count() as u32
    });
    // A template use spell with one of the four effects, else any enchant casting a use spell (`0x707cf0`, `0x707d60`).
    let enchant_effect = |spell: u32| {
        spells
            .and_then(|s| s.catalog.get(spell))
            .is_some_and(|def| def.effects.iter().any(|e| matches!(e, 53 | 54 | 156 | 33)))
    };
    let enchants_on_use = template.is_some_and(|t| {
        t.spells
            .iter()
            .any(|s| s.spell_id != 0 && s.trigger == 0 && enchant_effect(s.spell_id))
    }) || enchant_ids()
        .any(|id| enchants.is_some_and(|c| c.0.use_spell(id) != 0));
    benilla_ui::script::TradeItemFacts {
        bound: flags & 1 != 0 || soulbound_enchant,
        trade_stamp: (flags & ITEM_FLAG_BOP_TRADEABLE != 0 && fields.item_owner() == me)
            .then(|| fields.item_create_played_time().unwrap_or(0)),
        soulbound_enchant,
        quest_bound: template.is_some_and(|t| t.bonding == 4),
        bag_items,
        enchants_on_use,
    }
}

pub(crate) struct UiTradePlugin;

/// Mirrors the `blockTrades` CVar into [`BlockTrades`].
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut block: ResMut<BlockTrades>) {
    if ev.is("blockTrades") {
        block.0 = ev.flag();
    }
}

/// `ItemLimitCategory.dbc`, which names a limit-category close; empty without client data.
#[derive(Resource, Default)]
pub(crate) struct ItemLimitCategories(pub(crate) benilla_formats::ItemLimitCategoryCatalog);

fn load_item_limit_categories(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_item_limit_categories(&mut chain)
    };
    match loaded {
        Ok(catalog) => commands.insert_resource(ItemLimitCategories(catalog)),
        Err(e) => warn!("ui_trade: ItemLimitCategory.dbc failed — no limit line names one: {e:#}"),
    }
}

impl Plugin for UiTradePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        net::register(app);
        app.init_resource::<TradeSession>()
            .init_resource::<BlockTrades>()
            .init_resource::<ItemLimitCategories>()
            .add_systems(
                Startup,
                load_item_limit_categories.after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    // The feed runs after `UnitFeed`, whose item templates it reads, and before
                    // the input pass, with the drain after it, so each lands the same frame. No
                    // range guard: vmangos cancels a trade on distance (`Player.cpp:6117`).
                    feed_trade.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    // Ahead of the press and the feed, so an accepted partner shows the frame the
                    // window opens.
                    answer_trade_request
                        .before(crate::ui_script::UiPress)
                        .before(feed_trade),
                    // The coinage-change reflex's trade half, after the world's fields land.
                    trim_offer_to_purse
                        .after(crate::ui_unit::UnitFeed)
                        .before(feed_trade),
                    drain_trade.in_set(UiDrain),
                ),
            );
    }
}

/// The `blockTrades` CVar (`0x51daec`), the Controls panel's Block Trades checkbox.
/// A resource, not a [`TradeSession`] field, because a disconnect clears the session; read only
/// by [`answer_trade_request`]'s leg 8.
#[derive(Resource, Default)]
pub(crate) struct BlockTrades(pub(crate) bool);

/// The object lookup of `(guid, TYPEMASK_PLAYER)`: the guid's live object, `None` when it is not a
/// streamed player. Asked by the ladder's leg 3 and by the `%s` lines' second guard.
fn streamed_player<'a>(
    index: &GuidIndex,
    stores: &'a Query<&ObjectStore>,
    guid: u64,
) -> Option<&'a ObjectStore> {
    if !benilla_protocol::guid::is_player(guid) {
        return None;
    }
    index.0.get(&guid).and_then(|e| stores.get(*e).ok())
}

/// Answer an incoming trade request with the reference's eight-leg ladder; only leg 8 prints, so
/// the order is behaviour. A system, not a packet handler, because leg 8 may wait on a name query.
/// There is no consent step: 1.12 registers `TRADE_REQUEST` (event `0x11d`) and never signals it,
/// so the stock `TRADE` popup never shows and a request that passes opens the window unasked.
fn answer_trade_request(
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    social: Res<crate::ui_social::SocialState>,
    cinematic: Res<crate::cinematic::Cinematic>,
    player: Res<crate::player::Player>,
    auction: Res<crate::ui_auction::AuctionOpen>,
    block_trades: Res<BlockTrades>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    let Some(initiator) = trade.pending_request() else {
        return;
    };

    // Legs 1 and 3 send nothing: our own initiate is live, or the initiator is no streamed player.
    // Leg 3 is asked before leg 2 here, while the reference asks the ignore list first; the two
    // differ for an ignored, unstreamed initiator.
    let initiator_store = streamed_player(&index, &stores, initiator);
    if trade.initiated || initiator_store.is_none() {
        info!(
            target: "trade",
            "BEGIN_TRADE from {initiator:#x} dropped without a reply (own initiate in flight: {}, \
             initiator resolved: {})",
            trade.initiated,
            initiator_store.is_some(),
        );
        trade.refuse_request();
        return;
    }

    // Leg 2, the ignore list: `CMSG_IGNORE_TRADE`, read by the initiator as "is ignoring you";
    // vmangos has no such check (`TradeHandler.cpp:567`).
    if social.is_ignored(initiator) {
        info!(target: "trade", "BEGIN_TRADE from an ignored {initiator:#x}; sending CMSG_IGNORE_TRADE");
        let _ = commands.0.send(ClientCommand::IgnoreTrade);
        trade.refuse_request();
        return;
    }

    // Legs 4-7, a silent busy: a dead or ghost initiator (`unit_reads_dead`, like `UnitIsDead`,
    // would refuse a feigning hunter), a cinematic, lost player control (not an in-world flag) or
    // an open auction house (its auctioneer, not a pending trade).
    let dead_or_ghost = initiator_store.is_some_and(|s| s.0.is_dead_or_ghost());
    if dead_or_ghost
        || cinematic.is_playing()
        || player.control_lost
        || auction.auctioneer.is_some()
    {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} refused silently; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        return;
    }

    // Leg 8, Block Trades, the one leg that names the initiator. The reference reads the live
    // unit; this may wait on a name query, with no timer, since vmangos cancels the trade to us
    // when the initiator leaves (`Player.cpp:11747`), which clears the request.
    if block_trades.0 {
        let Some(name) = names.resolve(initiator, &commands).map(str::to_string) else {
            return;
        };
        info!(target: "trade", "BEGIN_TRADE from {name} refused by BlockTrades; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        // One error raise of `(0xbb, name)`; catalog row `0xbb` makes it a chat line.
        errors
            .0
            .push(crate::ui_action::UiError::s("ERR_TRADE_BLOCKED_S", name));
        return;
    }

    // Nothing refuses it: accept, the bottom of the ladder.
    info!(target: "trade", "BEGIN_TRADE from {initiator:#x} accepted; sending CMSG_BEGIN_TRADE");
    trade.begin(initiator);
    let _ = commands.0.send(ClientCommand::BeginTrade);
}

/// A wire [`TradeItem`] as the Lua row: name and quality from the item-template cache once it
/// answers, the icon from `display_id` via `ItemDisplayInfo.dbc`; `enchantment` is not filled yet.
fn resolve_slot(
    item: &TradeItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> TradeSlotItem {
    let template = items.template(item.entry, 0, commands);
    let name = template.map(|t| t.name.clone());
    let quality = template.map(|t| t.quality);
    let texture = crate::ui_items::item_icon(icons, item.display_id);
    // Nil until the template answers: the link embeds the name and the quality colour.
    let link = match (name.as_deref(), quality) {
        (Some(n), Some(q)) => Some(crate::ui_items::item_link_full(
            item.entry,
            &benilla_ui::script::ItemLinkFields {
                enchant: item.perm_enchant as i32,
                gems: item.socket_enchants.map(|g| g as i32),
                ..benilla_ui::script::ItemLinkFields::roll(
                    item.random_prop_id as i32,
                    item.suffix_factor as i32,
                )
            },
            n,
            q,
        )),
        _ => None,
    };
    TradeSlotItem {
        item_id: item.entry,
        name,
        texture,
        count: item.count.max(1),
        quality,
        enchantment: None,
        link,
    }
}

/// Our bag item at cursor-space `(bag, 1-based slot)` as a wire [`TradeItem`], for our own column
/// only; `None` for an empty or unstreamed slot. The item stays in the bag until the trade ends.
fn own_item_at(
    bag: i64,
    slot: u32,
    store: Option<&ObjectStore>,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
) -> Option<TradeItem> {
    let (guid, count) = crate::ui_items::slot_guid_count(store, bag, slot, objects);
    if guid == 0 {
        return None;
    }
    let entry = objects.object(guid).and_then(|f| f.object_entry())?;
    let display_id = items
        .template(entry, guid, commands)
        .map(|t| t.display_info_id)
        .unwrap_or(0);
    Some(TradeItem {
        entry,
        display_id,
        count,
        wrapped: false,
        gift_creator: 0,
        perm_enchant: 0,
        socket_enchants: [0; 3],
        creator: 0,
        charges: 0,
        suffix_factor: 0,
        random_prop_id: 0,
        lock_id: 0,
        max_durability: 0,
        durability: 0,
    })
}

fn resolve_side(
    offer: &TradeOffer,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> TradeSideState {
    let mut slots: [Option<TradeSlotItem>; TRADE_SLOT_COUNT] = Default::default();
    for (i, s) in offer.slots.iter().enumerate() {
        slots[i] = s.map(|it| resolve_slot(&it, items, icons, commands));
    }
    TradeSideState {
        slots,
        gold: offer.gold,
    }
}

fn snapshot(
    trade: &TradeSession,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<TradeState> {
    if !trade.open {
        return None;
    }
    Some(TradeState {
        player: resolve_side(&trade.our, items, icons, commands),
        target: resolve_side(&trade.their, items, icons, commands),
    })
}

/// Push the trade into the VM and fire its events on a change, diffed against `Local` memory;
/// `TRADE_ACCEPT_UPDATE` fires after `TRADE_SHOW`, whose handler hides the highlights.
fn feed_trade(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    mut trade: ResMut<TradeSession>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    // The `%s` lines' live-object guard, the same reads as the ladder's leg 3.
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    mut last: Local<crate::ui_script::VmMemo<Option<TradeState>>>,
    mut last_open: Local<crate::ui_script::VmMemo<bool>>,
    mut last_player_gold: Local<crate::ui_script::VmMemo<u32>>,
    mut last_their_gold: Local<crate::ui_script::VmMemo<u32>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_open = last_open.get(&script);
    let last_player_gold = last_player_gold.get(&script);
    let last_their_gold = last_their_gold.get(&script);

    let fresh = snapshot(&trade, &items, icons.as_deref(), &commands);
    let opened = !*last_open && trade.is_open();
    let closed = *last_open && !trade.is_open();
    let changed = fresh != *last;
    if changed {
        script.set_trade(fresh.clone());
    }
    // The partner's per-slot event (`0x586f96`), arg1 the 1-based slot; our own slots signal at the step that writes them (`0x586a7e`, `0x58648c`).
    if changed && trade.is_open() && !opened {
        let empty = TradeState::default();
        let old = last.as_ref().unwrap_or(&empty);
        if let Some(new) = fresh.as_ref() {
            let changed_slots = |mine: &[Option<TradeSlotItem>],
                                 theirs: &[Option<TradeSlotItem>]| {
                mine.iter()
                    .zip(theirs.iter())
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(i, _)| ScriptValue::Int(i as i64 + 1))
                    .collect::<Vec<_>>()
            };
            for slot in changed_slots(&new.target.slots, &old.target.slots) {
                script.fire_event("TRADE_TARGET_ITEM_CHANGED", vec![slot]);
            }
        }
    }
    // The owed `%s` lines, resolved here where the name cache is; a miss waits for the query it
    // started. The catalog row, read by `ui_action::feed_actions`, decides where each lands.
    for line in trade.take_named_lines() {
        // The status arms print nothing once the player is no longer an object.
        if line.needs_live_object && streamed_player(&index, &stores, line.who).is_none() {
            continue;
        }
        match names.resolve(line.who, &commands).map(str::to_string) {
            Some(name) => errors.0.push(crate::ui_action::UiError::s(line.key, name)),
            // Not cached: `resolve` has asked, so park it and retry next frame.
            None => trade.owe_named_line(line),
        }
    }
    // `CloseTrade` signalled its own close (`0x5879bc`).
    let verb_closed = trade.take_verb_closed();
    if opened {
        // The partner setter's open leg (`0x587129`): the folded coins are the last offer signalled (`0x5872ba`), so they signal once.
        if let Some(offer) = script.open_trade_partner(trade.opens_with_cursor()) {
            trade.our.gold = offer;
            *last_player_gold = offer;
        }
        script.fire_event("TRADE_SHOW", vec![]);
    } else if closed && !verb_closed {
        script.fire_event("TRADE_CLOSED", vec![]);
    } else if trade.is_open() && changed {
        script.fire_event("TRADE_UPDATE", vec![]);
    }

    // `TRADE_ACCEPT_UPDATE(mine, theirs)` when the session's flags leave the VM's while open; the accept verbs write and signal their own (`0x586712`, `0x586763`), and a send of theirs still waiting for the drain is not yet in the session.
    let accept = (trade.our_accept, trade.their_accept);
    if trade.is_open() && !script.trade_accept_sends_wait() && script.trade_accepts() != accept {
        script.set_trade_accepts(accept);
        script.fire_event(
            "TRADE_ACCEPT_UPDATE",
            vec![
                ScriptValue::Int(i64::from(accept.0)),
                ScriptValue::Int(i64::from(accept.1)),
            ],
        );
    }

    // Our gold reaches the money input through `PLAYER_TRADE_MONEY` (`TradeFrame.xml:565`), never
    // `TRADE_UPDATE`, so a repaint never overwrites what the player is typing.
    let player_gold = trade.our.gold;
    if trade.is_open() && player_gold != *last_player_gold {
        // The session's own trim of the offer reaches the VM's coins here.
        script.set_trade_offer_gold(player_gold);
        script.fire_event("PLAYER_TRADE_MONEY", vec![]);
    }
    *last_player_gold = if trade.is_open() { player_gold } else { 0 };
    // The partner's gold: `TRADE_MONEY_CHANGED`, which the `TARGET_TRADE` money frame repaints on
    // (`MoneyFrame.lua:133`). The reference fires it, beside `PLAYER_TRADE_MONEY`; which side each
    // reports is inferred from `MoneyFrame.lua`, not traced.
    let their_gold = trade.their.gold;
    if trade.is_open() && their_gold != *last_their_gold {
        script.fire_event("TRADE_MONEY_CHANGED", vec![]);
    }
    *last_their_gold = if trade.is_open() { their_gold } else { 0 };

    *last = fresh;
    *last_open = trade.is_open();
}

/// Drain the Lua intents into the trade `CMSG`s. `CloseTrade` clears the session and sends
/// `CMSG_CANCEL_TRADE`.
fn drain_trade(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    selection: Res<Selection>,
    group: Res<GroupState>,
    objects: Objects,
    items: Res<Items>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut queue: Commands,
) {
    let Some(mut script) = script else {
        return;
    };
    // Our bag contents, for the own-column item resolve.
    let store = self_q.iter().next();

    for token in script.take_trade_initiates() {
        match resolve_trade_target(&token, &selection, &group) {
            Some(guid) => {
                info!(target: "trade", "initiate: {token:?} -> {guid:#x}; sending CMSG_INITIATE_TRADE");
                // `InitiateTrade` asks for no cursor (`0x5226e1`); the drop on a player does (#1382).
                trade.initiate(guid, false);
                let _ = commands
                    .0
                    .send(ClientCommand::InitiateTrade { target: guid });
            }
            None => {
                info!(target: "trade", "initiate: {token:?} did not resolve to a player guid — nothing sent");
            }
        }
    }

    // The accept verbs' sends in call order, whatever the window's state (`0x58671a` -> `0x703f50`, `0x58676b` -> `0x703fd0`).
    for accept in script.take_trade_accept_sends() {
        if accept {
            trade.accept();
            let _ = commands.0.send(ClientCommand::AcceptTrade {
                their_state: trade.their_state(),
            });
        } else {
            trade.unaccept();
            let _ = commands.0.send(ClientCommand::UnacceptTrade);
        }
    }
    // Each money offer sends `CMSG_SET_TRADE_GOLD` whatever the window's state (`0x7043c0`, `0x704220`, `0x704320`); the server sends it to the partner only, so our own gold is set here.
    for copper in script.take_trade_money_sends() {
        trade.set_own_gold(copper);
        info!(target: "trade", "set gold: {copper} copper; sending CMSG_SET_TRADE_GOLD");
        let _ = commands.0.send(ClientCommand::SetTradeGold { copper });
        trade.own_change_sent();
    }
    // Placements and clears, sent whatever the window's state (`0x7040b0`, `0x7041a0`): the UI id is 1-based, the wire slot 0-based, and the cursor's bag and slot map to the wire position.
    for send in script.take_trade_item_sends() {
        match send {
            benilla_ui::script::TradeItemSend::Set { id, bag, slot } => {
                let Some((wire_bag, wire_slot)) = crate::ui_items::wire_pos(bag, slot) else {
                    info!(target: "trade", "set item: slot {id} <- bag {bag}/{slot} has no wire position — dropped");
                    continue;
                };
                let Some(trade_slot) = id.checked_sub(1).and_then(|n| u8::try_from(n).ok()) else {
                    continue;
                };
                // The server echoes a placement to the partner only, so our column is filled here.
                if let Some(item) = own_item_at(bag, slot, store, &objects, &items, &commands) {
                    trade.place_own_item(id, item);
                    if let Some(source) = trade.our_sources.get_mut(id as usize - 1) {
                        *source = Some((bag, slot));
                    }
                }
                info!(target: "trade", "set item: slot {id} <- bag {bag}/{slot} (wire {wire_bag}/{wire_slot}); sending CMSG_SET_TRADE_ITEM");
                let _ = commands.0.send(ClientCommand::SetTradeItem {
                    trade_slot,
                    bag: wire_bag,
                    slot: wire_slot,
                });
                trade.own_change_sent();
            }
            benilla_ui::script::TradeItemSend::Clear { id } => {
                let Some(trade_slot) = id.checked_sub(1).and_then(|n| u8::try_from(n).ok()) else {
                    continue;
                };
                trade.clear_own_item(id);
                info!(target: "trade", "clear item: slot {id}; sending CMSG_CLEAR_TRADE_ITEM");
                let _ = commands
                    .0
                    .send(ClientCommand::ClearTradeItem { trade_slot });
                trade.own_change_sent();
            }
        }
    }
    if script.take_trade_close() {
        // The cancel goes out whatever the window's state (`0x5879c4` -> `0x704040`).
        let _ = commands.0.send(ClientCommand::CancelTrade);
        trade.closed_by_verb();
        // The partner setter with none clears the interact target on the partner (`0x5870f7`).
        if let Some(partner) = trade.npc() {
            queue.queue(move |w: &mut World| {
                crate::ui_session::clear_interact_target(w, partner);
                crate::ui_script::live::fire_handler_events(w);
            });
        }
        trade.close_window();
    }
    // The `TRADE` popup's verbs: an empty `0x117` or `0x11C` and no local teardown; the status
    // reply drives the window.
    if script.take_trade_begin() {
        let _ = commands.0.send(ClientCommand::BeginTrade);
    }
    if script.take_trade_cancel() {
        let _ = commands.0.send(ClientCommand::CancelTrade);
    }
}

/// The coinage-change reflex: an offer above the purse is trimmed to the purse.
fn trimmed_offer(offer: u32, purse: u32) -> Option<u32> {
    (offer > purse).then_some(purse)
}

/// On each `PLAYER_FIELD_COINAGE` change with a trade open, re-send an offer past the purse as an
/// absolute `CMSG_SET_TRADE_GOLD` of the purse; the coin sound and `PLAYER_MONEY` fire elsewhere.
fn trim_offer_to_purse(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    mut prev: Local<Option<u32>>,
) {
    let Some(money) = self_q
        .iter()
        .next()
        .and_then(|store| store.0.player_money())
    else {
        *prev = None;
        return;
    };
    let old = prev.replace(money);
    if !matches!(old, Some(p) if p != money) || !trade.is_open() {
        return;
    }
    if let Some(trimmed) = trimmed_offer(trade.our.gold, money) {
        trade.set_own_gold(trimmed);
        let _ = commands
            .0
            .send(ClientCommand::SetTradeGold { copper: trimmed });
        trade.own_change_sent();
    }
}

/// The player guid a UnitPopup token names, through [`crate::ui_unit::player_token_guid`].
fn resolve_trade_target(token: &str, selection: &Selection, group: &GroupState) -> Option<u64> {
    crate::ui_unit::player_token_guid(token, selection, group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};

    fn wire_item(entry: u32) -> TradeItem {
        TradeItem {
            entry,
            display_id: 42,
            count: 3,
            wrapped: false,
            gift_creator: 0,
            perm_enchant: 0,
            socket_enchants: [0; 3],
            creator: 0,
            charges: 0,
            suffix_factor: 0,
            random_prop_id: 0,
            lock_id: 0,
            max_durability: 0,
            durability: 0,
        }
    }

    /// The offer gates' reads (`0x708520`, `0x708b66`, `0x7073e0`, `0x5125b0`): the bound bit or a binding enchant, the trade window's stamp only on our own flagged item, and a bag's count of the item objects it holds.
    #[test]
    fn the_offer_gates_read_the_items_fields() {
        use benilla_protocol::field::*;
        use benilla_protocol::ObjectFields;
        let cat = crate::items::Enchants(
            benilla_formats::EnchantCatalog::from_rows(
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
                std::collections::HashMap::from([(11, 0x1), (13, 0x0)]),
            ),
            Default::default(),
        );
        let facts = |pairs: &[(u16, u32)]| {
            item_facts(
                &ObjectFields::from_pairs(pairs),
                None,
                Some(0x42),
                Some(&cat),
                None,
                &|guid| guid == 0x4000_0000_0000_0009,
            )
        };
        let plain = facts(&[(FIELD_ITEM_FLAGS, 0)]);
        assert_eq!(plain, Default::default());

        let bound = facts(&[(FIELD_ITEM_FLAGS, 0x1)]);
        assert!(bound.bound && !bound.soulbound_enchant && bound.trade_stamp.is_none());

        let enchanted = facts(&[(FIELD_ITEM_FLAGS, 0), (FIELD_ITEM_ENCHANTMENT_1_1, 11)]);
        assert!(enchanted.bound && enchanted.soulbound_enchant);
        let other = facts(&[(FIELD_ITEM_FLAGS, 0), (FIELD_ITEM_ENCHANTMENT_1_1, 13)]);
        assert!(!other.bound && !other.soulbound_enchant);

        // The stamp needs the flag and our own guid as the owner (`0x708b66`, `0x708b8a`).
        let ours = facts(&[
            (FIELD_ITEM_FLAGS, 0x101),
            (FIELD_ITEM_OWNER, 0x42),
            (FIELD_ITEM_CREATE_PLAYED_TIME, 900),
        ]);
        assert_eq!(ours.trade_stamp, Some(900));
        let theirs = facts(&[(FIELD_ITEM_FLAGS, 0x101), (FIELD_ITEM_OWNER, 0x43)]);
        assert!(theirs.bound && theirs.trade_stamp.is_none());

        // A bag counts the slots whose item object is held.
        let bag = facts(&[
            (FIELD_OBJECT_TYPE, 0x7),
            (FIELD_CONTAINER_NUM_SLOTS, 4),
            (FIELD_CONTAINER_SLOT_1, 0x9),
            (FIELD_CONTAINER_SLOT_1 + 1, 0x4000_0000),
            (FIELD_CONTAINER_SLOT_1 + 2, 0xa),
            (FIELD_CONTAINER_SLOT_1 + 3, 0x4000_0000),
        ]);
        assert_eq!(bag.bag_items, Some(1));
        assert_eq!(plain.bag_items, None);
    }

    #[test]
    fn open_close_state_machine() {
        let mut s = TradeSession::default();
        assert!(!s.is_open());

        s.initiate(0x1234, false);
        assert_eq!(s.partner, Some(0x1234));
        assert!(!s.is_open());
        assert_eq!(s.npc(), None);

        s.open_window(0);
        assert!(s.is_open());
        assert_eq!(s.npc(), Some(0x1234));

        s.close_window();
        assert!(!s.is_open());
        assert_eq!(s.partner, None);
        assert_eq!(s.npc(), None);
    }

    #[test]
    fn an_incoming_request_is_not_yet_a_trade() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        assert_eq!(s.pending_request(), Some(0xABCD));
        assert_eq!(s.partner, None, "an unanswered request has no partner");
        assert!(!s.is_open(), "BEGIN_TRADE does not itself open the window");
        assert_eq!(s.npc(), None);
    }

    #[test]
    fn accepting_a_request_promotes_the_initiator_to_partner() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        s.begin(0xABCD);
        assert_eq!(s.partner, Some(0xABCD));
        assert_eq!(s.pending_request(), None, "the request is spent");
        assert!(!s.is_open(), "the window waits for OPEN_WINDOW");
    }

    #[test]
    fn closing_clears_a_pending_request() {
        let mut s = TradeSession::default();
        s.request(0xABCD);
        s.close_window();
        assert_eq!(s.pending_request(), None);
    }

    #[test]
    fn a_request_leaves_a_live_trade_alone() {
        let mut s = TradeSession::default();
        s.initiate(0x1, false);
        s.open_window(0);
        s.partner_accepted();

        s.request(0x2);
        assert_eq!(s.pending_request(), Some(0x2));
        assert_eq!(s.partner, Some(0x1), "the live trade is untouched");
        assert!(s.is_open());
        assert!(s.their_accept);

        s.refuse_request();
        assert_eq!(s.pending_request(), None);
        assert_eq!(s.partner, Some(0x1), "refusing an offer is not cancelling");
        assert!(s.is_open());
    }

    #[test]
    fn extended_fills_the_right_side_and_resets_accepts() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window(0);
        s.partner_accepted();
        assert!(s.their_accept);

        let mut slots = [None; TRADE_SLOT_COUNT];
        slots[0] = Some(wire_item(2589));
        let ext = snapshot_of(true, 500, slots);
        assert_eq!(s.set_offer(&ext), OfferUpdate::Applied);
        assert!(s.their.slots[0].is_some());
        assert_eq!(s.their.gold, 500);
        assert_eq!(s.our.gold, 0);
        assert!(!s.their_accept, "an offer change resets accepts");

        let our_ext = snapshot_of(false, 999, [None; TRADE_SLOT_COUNT]);
        assert_eq!(s.set_offer(&our_ext), OfferUpdate::Applied);
        assert_eq!(s.our.gold, 999);
        assert_eq!(s.their.gold, 500, "the other side is untouched");
    }

    /// A snapshot for trade id 0 with state index 1 and side state 3.
    fn snapshot_of(
        their_window: bool,
        gold: u32,
        slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    ) -> TradeStatusExtended {
        TradeStatusExtended {
            their_window,
            trade_id: 0,
            state_index: 1,
            side_state: 3,
            gold,
            enchant_spell_id: 0,
            slots,
        }
    }

    /// The partner's offer replaces the whole column: a slot it no longer lists empties.
    #[test]
    fn a_fresh_partner_snapshot_clears_the_slots_it_omits() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window(0);
        let mut slots = [None; TRADE_SLOT_COUNT];
        slots[0] = Some(wire_item(2589));
        slots[3] = Some(wire_item(2590));
        s.set_offer(&snapshot_of(true, 0, slots));
        slots[0] = None;
        s.set_offer(&snapshot_of(true, 0, slots));
        assert!(s.their.slots[0].is_none() && s.their.slots[3].is_some());
    }

    /// A snapshot of another trade id is not applied and asks for a cancel (`0x7046ae`).
    #[test]
    fn another_trades_snapshot_is_refused() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window(5);
        let mut slots = [None; TRADE_SLOT_COUNT];
        slots[0] = Some(wire_item(2589));
        assert_eq!(
            s.set_offer(&snapshot_of(true, 7, slots)),
            OfferUpdate::OtherTrade
        );
        assert!(s.their.slots[0].is_none() && s.their.gold == 0);
    }

    /// Our own snapshot behind the changes we sent is dropped; the partner's never is.
    #[test]
    fn an_own_snapshot_behind_our_sends_is_stale() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window(0);
        s.own_change_sent();
        let own = snapshot_of(false, 40, [None; TRADE_SLOT_COUNT]);
        assert_eq!(s.set_offer(&own), OfferUpdate::Stale, "index 1 < our 2");
        assert_eq!(s.our.gold, 0);
        let theirs = snapshot_of(true, 40, [None; TRADE_SLOT_COUNT]);
        assert_eq!(s.set_offer(&theirs), OfferUpdate::Applied);
        assert_eq!(
            s.their_state(),
            3,
            "the accept will echo the partner's state"
        );
    }

    #[test]
    fn accept_glow_tracks_both_sides() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window(0);
        s.accept();
        s.partner_accepted();
        assert_eq!((s.our_accept, s.their_accept), (true, true));
        s.back_to_trade();
        assert_eq!((s.our_accept, s.their_accept), (false, false));
    }

    #[test]
    fn own_offer_is_tracked_optimistically() {
        let mut s = TradeSession::default();
        s.begin(0x7);
        s.open_window(0);
        s.accept();
        s.partner_accepted();
        assert_eq!((s.our_accept, s.their_accept), (true, true));

        s.place_own_item(2, wire_item(1234));
        assert!(s.our.slots[1].is_some(), "our slot 2 shows the placed item");
        assert_eq!(
            (s.our_accept, s.their_accept),
            (false, false),
            "changing our offer resets both accepts"
        );

        s.set_own_gold(500);
        assert_eq!(s.our.gold, 500);

        s.clear_own_item(2);
        assert!(s.our.slots[1].is_none(), "clearing empties the slot");
    }

    #[test]
    fn resolve_target_reads_the_selection_and_roster() {
        // "player" is yourself, and there is no roster for "party3".
        let sel = Selection::default();
        let group = GroupState::default();
        assert_eq!(resolve_trade_target("player", &sel, &group), None);
        assert_eq!(resolve_trade_target("party3", &sel, &group), None);
    }

    #[test]
    fn status_codes_are_stable() {
        assert_eq!(TradeStatus::OpenWindow { trade_id: 0 }.code(), 2);
        assert_eq!(TradeStatus::Accept.code(), 4);
        assert_eq!(TradeStatus::BackToTrade.code(), 7);
    }

    #[test]
    fn drain_initiate_resolves_the_selection_and_ships_the_cmsg() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection {
                target: None,
                guid: Some(0x7), // high 16 bits 0: a player guid
                ..Default::default()
            });
        app.insert_non_send_resource(UiScript::new().unwrap());

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("InitiateTrade('target')")
            .unwrap();

        app.world_mut().run_system_once(drain_trade).unwrap();

        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::InitiateTrade { target: 0x7 })
            ),
            "the drain ships CMSG_INITIATE_TRADE against the resolved target guid"
        );
        assert_eq!(
            app.world().resource::<TradeSession>().partner,
            Some(0x7),
            "the initiator records the partner so OPEN_WINDOW can name it"
        );
    }

    /// The accept verbs' sends go out in call order, and the coins', with no window standing (`0x58671a` -> `0x703f50`, `0x58676b` -> `0x703fd0`, `0x7043c0`).
    #[test]
    fn the_drain_sends_the_accepts_and_the_coins_with_no_window() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .init_resource::<Selection>()
            .insert_resource(NetCommands(tx));
        let mut vm = UiScript::new().unwrap();
        vm.set_money(1_000);
        vm.run("CancelTradeAccept() AcceptTrade() CancelTradeAccept() SetTradeMoney(500)")
            .unwrap();
        app.insert_non_send_resource(vm);
        app.world_mut().run_system_once(drain_trade).unwrap();
        let sent: Vec<String> = rx.try_iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(
            sent,
            [
                "AcceptTrade { their_state: 0 }",
                "UnacceptTrade",
                "SetTradeGold { copper: 500 }"
            ]
        );
        let trade = app.world().resource::<TradeSession>();
        assert!(!trade.is_open() && !trade.our_accept);
        assert_eq!(trade.our.gold, 500);
    }

    /// The feed signals an accept only when the verb has not already written and signalled it (`0x586712`); the partner's, from its status, it still signals.
    #[test]
    fn the_feed_does_not_signal_the_verbs_accept_again() {
        let (mut app, _rx) = feed_app();
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window(1);
        }
        app.update();
        let seen = |app: &mut App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<String>("return table.concat(seen, ',')")
                .unwrap()
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                "seen = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_ACCEPT_UPDATE') \
                 f:SetScript('OnEvent', function(_, _, mine, theirs) seen[#seen + 1] = mine .. ':' .. theirs end) \
                 AcceptTrade()",
            )
            .unwrap();
        assert_eq!(seen(&mut app), "1:0");
        // The feed ahead of the drain leaves the verb's flag alone; so does the feed after it.
        app.update();
        assert_eq!(seen(&mut app), "1:0");
        let drain = |app: &mut App| {
            let sends = app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_trade_accept_sends();
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            for accept in sends {
                if accept {
                    trade.accept();
                } else {
                    trade.unaccept();
                }
            }
        };
        drain(&mut app);
        app.update();
        assert_eq!(seen(&mut app), "1:0");
        app.world_mut()
            .resource_mut::<TradeSession>()
            .partner_accepted();
        app.update();
        assert_eq!(seen(&mut app), "1:0,1:1");
        // A status that drops both between the drain and the next feed is still signalled.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("CancelTradeAccept() AcceptTrade()")
            .unwrap();
        drain(&mut app);
        app.world_mut()
            .resource_mut::<TradeSession>()
            .back_to_trade();
        app.update();
        assert_eq!(seen(&mut app), "1:0,1:1,0:1,1:1,0:0");
    }

    /// The open takes the cursor's coins only for an initiate of ours that asked for the cursor (`0x703c30`), and the folded coins signal once, ahead of `TRADE_SHOW` (`0x5872ba`-`0x5872d8`).
    #[test]
    fn the_open_folds_the_cursors_coins_only_for_an_initiate_that_asked() {
        let (mut app, _rx) = feed_app();
        // The purse the fold's test reads off the player (`0x704283`).
        let purse = benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE, 1_000),
        ]);
        let me = app.world_mut().spawn(ObjectStore(purse)).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0x42, me);
        app.insert_resource(crate::net::SelfGuid(Some(0x42)));
        {
            let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
            vm.set_money(1_000);
            vm.run(
                "seen = {} local f = CreateFrame('Frame') \
                 for _, e in ipairs({'PLAYER_TRADE_MONEY', 'PLAYER_MONEY', 'TRADE_SHOW'}) do f:RegisterEvent(e) end \
                 f:SetScript('OnEvent', function(_, e) seen[#seen + 1] = e end) \
                 PickupPlayerMoney(300) seen = {}",
            )
            .unwrap();
        }
        let seen = |app: &mut App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<String>("local s = table.concat(seen, ',') seen = {} return s")
                .unwrap()
        };
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.initiate(0x7, false);
            trade.open_window(1);
        }
        app.update();
        assert_eq!(seen(&mut app), "TRADE_SHOW");
        assert_eq!(app.world().resource::<TradeSession>().our.gold, 0);

        app.world_mut()
            .resource_mut::<TradeSession>()
            .close_window();
        app.update();
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.initiate(0x7, true);
            trade.open_window(2);
        }
        app.update();
        app.update();
        assert_eq!(seen(&mut app), "PLAYER_TRADE_MONEY,PLAYER_MONEY,TRADE_SHOW");
        assert_eq!(app.world().resource::<TradeSession>().our.gold, 300);
        let vm = app.world().non_send_resource::<UiScript>();
        assert_eq!(vm.eval::<i64>("return GetCursorMoney()").unwrap(), 0);
        assert_eq!(vm.eval::<i64>("return GetPlayerTradeMoney()").unwrap(), 300);
    }

    /// An app running [`feed_trade`] with a live VM and `0x7` named in the cache.
    fn feed_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut names = NameCache::default();
        names.insert_player(0x7, "Grubbis".to_string(), None);
        app.init_resource::<TradeSession>()
            .init_resource::<Items>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<GuidIndex>()
            .insert_resource(names)
            .insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_trade);
        (app, rx)
    }

    /// The reference prints the line from its own sender, not on a reply; row `0xb9` is chat.
    #[test]
    fn initiating_a_trade_says_so() {
        let (mut app, _rx) = feed_app();
        app.world_mut()
            .resource_mut::<TradeSession>()
            .initiate(0x7, false);
        app.update();

        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_INITIATE_TRADE_S",
                "Grubbis".to_string()
            )],
        );
        assert_eq!(
            benilla_ui::messages::kind_of("ERR_INITIATE_TRADE_S"),
            benilla_ui::messages::MsgKind::Chat,
        );
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .len(),
            1,
            "the debt is spent once, not re-raised every frame"
        );
    }

    #[test]
    fn a_naming_status_line_needs_a_live_object_and_the_initiate_line_does_not() {
        let (mut app, _rx) = feed_app();
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.initiate(0x7, false); // parks ERR_INITIATE_TRADE_S, needs_live_object: false
            trade.owe_named_line(NamedLine {
                key: "ERR_PLAYER_BUSY_S",
                who: 0x7,
                needs_live_object: true,
            });
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .iter()
                .map(|e| e.key)
                .collect::<Vec<_>>(),
            vec!["ERR_INITIATE_TRADE_S"],
            "0x7 is cached, not a streamed object: the status line drops, the initiate line stays"
        );

        // Stream the player, ask again: now it prints.
        let e = app.world_mut().spawn(ObjectStore::default()).id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(0x7, e);
        app.world_mut()
            .resource_mut::<crate::ui_action::UiErrorKeys>()
            .0
            .clear();
        app.world_mut()
            .resource_mut::<TradeSession>()
            .owe_named_line(NamedLine {
                key: "ERR_PLAYER_BUSY_S",
                who: 0x7,
                needs_live_object: true,
            });
        app.update();
        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_PLAYER_BUSY_S",
                "Grubbis".to_string()
            )],
        );
    }

    #[test]
    fn the_initiate_line_waits_for_the_name_and_survives_the_refusal() {
        let (mut app, _rx) = feed_app();
        app.world_mut()
            .resource_mut::<TradeSession>()
            .initiate(0x99, false); // no name cached
        app.update();
        assert!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .is_empty(),
            "no name, no line — and never a line with an unfilled %s"
        );

        // The server refuses before the name lands; the debt survives the close.
        app.world_mut()
            .resource_mut::<TradeSession>()
            .close_window();
        app.world_mut().resource_mut::<NameCache>().insert_player(
            0x99,
            "Skarrid".to_string(),
            None,
        );
        app.update();
        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_INITIATE_TRADE_S",
                "Skarrid".to_string()
            )],
        );
    }

    #[test]
    fn a_request_that_survives_the_ladder_is_accepted() {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::BeginTrade)),
            "nothing refused it, so the client answers — the server opens both windows"
        );
        let trade = app.world().resource::<TradeSession>();
        assert_eq!(trade.partner, Some(0x7));
        assert_eq!(trade.pending_request(), None, "the request is spent");
        assert!(
            !trade.initiated,
            "a trade we did not start never sets the initiate latch"
        );
        assert!(
            app.world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .is_empty(),
            "and an accepted request says nothing — only leg 8 speaks"
        );
    }

    #[test]
    fn block_trades_refuses_and_says_so() {
        let (mut app, rx) = request_app(true, false, true);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();

        assert!(matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)));
        assert_eq!(
            app.world().resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::s(
                "ERR_TRADE_BLOCKED_S",
                "Grubbis".to_string()
            )],
            "the refusal raises the reference's own error `(0xbb, name)`"
        );
        assert_eq!(
            benilla_ui::messages::kind_of("ERR_TRADE_BLOCKED_S"),
            benilla_ui::messages::MsgKind::Chat,
        );
    }

    #[test]
    fn the_silent_legs_refuse_without_a_word() {
        for (cinematic, controlled) in [(true, true), (false, false)] {
            let (mut app, rx) = request_app(false, cinematic, controlled);
            app.world_mut().resource_mut::<TradeSession>().request(0x7);
            app.update();
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)),
                "cinematic={cinematic} controlled={controlled}: refused as busy"
            );
            assert!(
                app.world()
                    .resource::<crate::ui_action::UiErrorKeys>()
                    .0
                    .is_empty(),
                "cinematic={cinematic} controlled={controlled}: a silent leg says nothing"
            );
        }
    }

    #[test]
    fn an_open_auction_house_refuses_the_request() {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut()
            .resource_mut::<crate::ui_auction::AuctionOpen>()
            .auctioneer = Some(0x1234);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)));
    }

    #[test]
    fn an_ignored_initiator_is_refused_as_ignored() {
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut()
            .resource_mut::<crate::ui_social::SocialState>()
            .set_ignores_for_test(vec![0x7]);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::IgnoreTrade)),
            "the ignore leg does NOT answer busy"
        );
        assert!(app
            .world()
            .resource::<crate::ui_action::UiErrorKeys>()
            .0
            .is_empty());
    }

    #[test]
    fn a_dead_or_ghost_initiator_is_refused() {
        // The `UNIT_FIELD_HEALTH`, `UNIT_FIELD_MAXHEALTH` and `PLAYER_FLAGS` field ids, and
        // `PLAYER_FLAGS_GHOST`.
        const HEALTH: u16 = benilla_protocol::field::FIELD_UNIT_HEALTH;
        const MAXHEALTH: u16 = benilla_protocol::field::FIELD_UNIT_MAXHEALTH;
        const PLAYER_FLAGS: u16 = benilla_protocol::field::FIELD_PLAYER_FLAGS;
        const GHOST: u32 = 0x10;
        for (label, fields) in [
            ("dead", vec![(HEALTH, 0u32), (MAXHEALTH, 100)]),
            (
                "ghost",
                vec![(HEALTH, 1u32), (MAXHEALTH, 100), (PLAYER_FLAGS, GHOST)],
            ),
        ] {
            let (mut app, rx) = request_app(false, false, true);
            let entity = app.world().resource::<GuidIndex>().0[&0x7];
            app.world_mut().entity_mut(entity).insert(ObjectStore(
                benilla_protocol::messages::ObjectFields::from_pairs(&fields),
            ));
            app.world_mut().resource_mut::<TradeSession>().request(0x7);
            app.update();
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::BusyTrade)),
                "{label}: refused as busy"
            );
        }
    }

    #[test]
    fn two_legs_answer_with_no_packet_at_all() {
        let answered = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter().any(|c| {
                matches!(
                    c,
                    ClientCommand::BusyTrade
                        | ClientCommand::IgnoreTrade
                        | ClientCommand::BeginTrade
                        | ClientCommand::CancelTrade
                )
            })
        };

        // Leg 1: we asked somebody else first, and the request arrives over it.
        let (mut app, rx) = request_app(false, false, true);
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.initiate(0x9, false);
            trade.request(0x7);
        }
        app.update();
        assert!(
            !answered(&rx),
            "an initiate of our own in flight drops the request in silence"
        );
        let trade = app.world().resource::<TradeSession>();
        assert_eq!(
            trade.pending_request(),
            None,
            "…and spends it, so it is not retried every frame"
        );
        assert_eq!(trade.partner, Some(0x9), "…and our own trade survives it");

        // Leg 3: nothing of that guid is streamed.
        let (mut app, rx) = request_app(false, false, true);
        app.world_mut().resource_mut::<GuidIndex>().0.remove(&0x7);
        app.world_mut().resource_mut::<TradeSession>().request(0x7);
        app.update();
        assert!(
            !answered(&rx),
            "an unresolvable initiator drops the request in silence"
        );
    }

    fn request_app(
        block_trades: bool,
        cinematic: bool,
        controlled: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut names = NameCache::default();
        // Named, so leg 8 reaches its line rather than parking on a name query.
        names.insert_player(0x7, "Grubbis".to_string(), None);
        names.insert_player(0x8, "Skarrid".to_string(), None);
        app.init_resource::<TradeSession>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_social::SocialState>()
            .init_resource::<crate::ui_auction::AuctionOpen>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::player::Player>()
            .insert_resource(names)
            .insert_resource(NetCommands(tx))
            .insert_resource(BlockTrades(block_trades))
            .insert_resource(if cinematic {
                crate::cinematic::Cinematic::playing_for_test()
            } else {
                crate::cinematic::Cinematic::default()
            });
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .control_lost = !controlled;
        // Streamed, since leg 3 drops a request from a guid that resolves to nothing.
        for guid in [0x7u64, 0x8] {
            let e = app.world_mut().spawn(ObjectStore::default()).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
        }
        app.add_systems(Update, answer_trade_request);
        (app, rx)
    }

    /// `SetTradeMoney`'s send asks for no window (`0x586870` -> `0x7043c0`: the purse test `0x70441e`, then the send `0x704463`).
    #[test]
    fn drain_money_ships_set_trade_gold_with_or_without_a_window() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection::default());
        let mut vm = UiScript::new().unwrap();
        vm.set_money(20_000); // the purse `SetTradeMoney` is gated on
        app.insert_non_send_resource(vm);

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SetTradeMoney(500)")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::SetTradeGold { copper: 500 })
            ),
            "the offer goes out with no window standing"
        );

        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window(0);
        }
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SetTradeMoney(12345)")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::SetTradeGold { copper: 12_345 })
            ),
            "an open-window money offer ships CMSG_SET_TRADE_GOLD"
        );
    }

    #[test]
    fn drain_item_clear_ships_clear_trade_item_zero_based() {
        use bevy::ecs::system::RunSystemOnce;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<GuidIndex>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Selection::default());
        app.insert_non_send_resource(UiScript::new().unwrap());
        {
            let mut trade = app.world_mut().resource_mut::<TradeSession>();
            trade.begin(0x7);
            trade.open_window(0);
        }

        // Our slot 1 filled from the pack, then an empty-cursor click on it, which takes the item up (`0x587c2f`).
        {
            let mut vm = app.world_mut().non_send_resource_mut::<UiScript>();
            let mut slots = std::collections::HashMap::new();
            slots.insert(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2589,
                    count: 1,
                    ..Default::default()
                },
            );
            vm.set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    bag_family: 0,
                    name: None,
                    num_slots: 16,
                    slots,
                }),
            );
            vm.set_trade(Some(benilla_ui::script::TradeState::default()));
            vm.run("PickupContainerItem(0, 1) ClickTradeButton(1) ClickTradeButton(1)")
                .unwrap();
            assert!(
                vm.cursor_item().is_some(),
                "the slot's item is back on the cursor"
            );
        }
        app.world_mut().run_system_once(drain_trade).unwrap();
        // The placement, then the clear, in call order (`0x7040b0`, `0x7041a0`).
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::SetTradeItem {
                trade_slot: 0,
                bag: 255,
                slot: 23
            })
        ));
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::ClearTradeItem { trade_slot: 0 })
            ),
            "clearing our filled UI slot 1 ships CMSG_CLEAR_TRADE_ITEM for wire slot 0"
        );
        assert!(!app.world().resource::<TradeSession>().offers(0, 1));
    }
}
