//! The trade window: [`net`] folds the two status packets into [`TradeSession`], the network
//! cells (`0xca0b68`-`0xca0ff8`), and makes the window's own writes and signals (`0xbfa400`-`0xbfa684`,
//! the VM's) at each packet's step; the verbs read both at the call; [`drain_trade`] sends the Lua
//! intents, and [`answer_trade_request`] answers an incoming request.

use benilla_protocol::messages::{TradeItem, TradeStatusExtended, TRADE_SLOT_COUNT};
#[cfg(test)]
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use benilla_ui::script::{TradeCells, TradeTargetItem};

use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore};
use crate::target::Selection;
use crate::ui_action::signals::SignalSink;
use crate::ui_party::GroupState;
use crate::ui_script::UiDrain;
use crate::ui_session::NpcSession;

mod net;

/// One side's window as the wire left it (`0xca0b80`, `0xca0b74`, `0xca0ff0`): slots 0-5 change
/// hands, slot 6 is the non-traded enchant slot.
#[derive(Default)]
struct TradeOffer {
    slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
    gold: u32,
    /// The spell on this side's enchant slot, 0 for none.
    enchant_spell_id: u32,
}

/// The trade's network cells, which [`net`] fills and the verbs read at the call; cleared on a
/// close and on disconnect.
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
    /// The own-window slots the server's last snapshot filled (`0xca0b80`), which a snapshot missing one denies (`0x704810`, `0x704873`).
    own_mirror: [bool; TRADE_SLOT_COUNT],
    /// Lines whose `%s` names a player not yet in the name cache, shown from its answer
    /// ([`net`]'s name handler): `ERR_INITIATE_TRADE_S`'s callback (`0x703c60`). They survive
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

/// `ERR_INITIATE_TRADE_S` owed on a name not yet cached, shown by the name's callback (`0x703c60`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NamedLine {
    /// The `GlobalStrings` key; the catalog names its surface.
    pub(crate) key: &'static str,
    /// Whose name fills the `%s`.
    pub(crate) who: u64,
}

impl TradeSession {
    /// Our `CMSG_INITIATE_TRADE` goes out (`0x703cf0`): a fresh session with the target as partner, and whether the open takes the cursor's payload (`0x703d90`).
    pub(crate) fn initiate(&mut self, target: u64, use_cursor: bool) {
        *self = TradeSession {
            partner: Some(target),
            initiated: true,
            use_cursor,
            named_lines: std::mem::take(&mut self.named_lines),
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

    /// `OPEN_WINDOW` (`0x704517`): both offers empty, both states 1, the trade id kept; the partner setter's open follows in the window (`0x5874cb`).
    pub(crate) fn open_window(&mut self, trade_id: u32) {
        self.open = true;
        self.trade_id = trade_id;
        self.own_state = 1;
        self.their_state = 1;
        self.our = TradeOffer::default();
        self.their = TradeOffer::default();
    }

    /// `SMSG_TRADE_STATUS_EXTENDED` as `0x704680` takes it: another trade's snapshot is answered
    /// with a cancel, an own snapshot behind our sent changes is dropped, and otherwise the side's
    /// offer is replaced whole; the window's offer update follows (`0x7048b2`).
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
        OfferUpdate::Applied
    }

    /// One own change went out (`CMSG_SET_TRADE_ITEM`, `_CLEAR_TRADE_ITEM`, `_SET_TRADE_GOLD`),
    /// each of which counts `[0xca0b7c]` up (`0x70415f`, `0x7041dc`, `0x70444f`).
    pub(crate) fn own_change_sent(&mut self) {
        self.own_state += 1;
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

    /// Our item at `(bag, slot)` went out to trade slot `id_1based` (`0x7040b0`).
    pub(crate) fn offer_from(&mut self, id_1based: u32, bag: i64, slot: u32) {
        if let Some(source) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our_sources.get_mut(i as usize))
        {
            *source = Some((bag, slot));
        }
    }

    /// Our trade slot `id_1based` was cleared (`0x7041a0`).
    pub(crate) fn clear_own_item(&mut self, id_1based: u32) {
        if let Some(source) = id_1based
            .checked_sub(1)
            .and_then(|i| self.our_sources.get_mut(i as usize))
        {
            *source = None;
        }
    }

    /// Our offered coins, as each money send writes them (`0x70445d`).
    pub(crate) fn set_own_gold(&mut self, copper: u32) {
        self.our.gold = copper;
    }

    /// The cells the verbs read at the call: the partner's coins (`0xca0b78`) and each window's enchant spell (`0xca0ff0`).
    pub(crate) fn cells(&self) -> TradeCells {
        TradeCells {
            target_gold: self.their.gold,
            enchant_spells: [self.our.enchant_spell_id, self.their.enchant_spell_id],
        }
    }

    /// The partner's window as the window's offer update copies it (`0x586dc0` over `0xca0db0`).
    pub(crate) fn their_items(&self) -> [TradeTargetItem; TRADE_SLOT_COUNT] {
        self.their.slots.map(|slot| {
            slot.map_or_else(TradeTargetItem::default, |it| TradeTargetItem {
                item_id: it.entry,
                count: it.count,
                enchant: it.perm_enchant,
                gems: it.socket_enchants,
                creator: it.creator,
                wrapped: it.wrapped,
                gift_creator: it.gift_creator,
                charges: it.charges,
                suffix_factor: it.suffix_factor,
                random_property: it.random_prop_id as i32,
                lock_id: it.lock_id,
                max_durability: it.max_durability,
                durability: it.durability,
            })
        })
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

    /// Park a line that names a player.
    pub(crate) fn owe_named_line(&mut self, line: NamedLine) {
        self.named_lines.push(line);
    }

    /// Take the lines owed on `who`'s name.
    pub(crate) fn take_named_lines_for(&mut self, who: u64) -> Vec<NamedLine> {
        let (taken, kept) = std::mem::take(&mut self.named_lines)
            .into_iter()
            .partition(|l| l.who == who);
        self.named_lines = kept;
        taken
    }

    /// The name a `%s` status line carries, or `None` when the reference prints nothing: cases 0, 5
    /// and 14 read the guid cell under the latch, both of which `InitiateTrade` sets, while the
    /// `BEGIN_TRADE` arm fills only the cell. So a refuser stays silent when vmangos echoes
    /// `IGNORE_YOU` to both sides (`TradeHandler.cpp:46`). The live-object guard is
    /// [`unit_name_line`]'s caller's.
    pub(crate) fn line_names(&self) -> Option<u64> {
        self.initiated.then_some(self.partner).flatten()
    }

    /// Close the window, the reference's trade-partner reset to `(0, 0)`, from `CANCELED`,
    /// `COMPLETE`, `CLOSE_WINDOW` or the local `CloseTrade`: a full reset, a superset of
    /// [`Self::clear_pending`], keeping the lines the reference raises before it,
    /// [`Self::named_lines`].
    pub(crate) fn close_window(&mut self) {
        *self = TradeSession {
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

    /// `ONLY_CONJURED` and `NOT_ELIGIBLE` (cases 22 and 23, `0x587727`/`0x587742` → `0x586410`):
    /// empty the offending slot of our offer, or the gold for `0xff` (`0x7045eb`); the window's
    /// deny raises the repaint events. Deviation: an
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

    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    // ── Test accessors, for `net`'s tests ──

    #[cfg(test)]
    pub(crate) fn take_named_lines_for_test(&mut self) -> Vec<NamedLine> {
        std::mem::take(&mut self.named_lines)
    }

    /// Our first trade slot offered from `(bag, slot)`.
    #[cfg(test)]
    pub(crate) fn offer_from_for_test(&mut self, bag: i64, slot: u32) {
        self.our_sources[0] = Some((bag, slot));
    }

    /// Our own offer as `(gold, which slots are offered from a bag)`.
    #[cfg(test)]
    pub(crate) fn own_offer_for_test(&self) -> (u32, [bool; TRADE_SLOT_COUNT]) {
        (self.our.gold, self.our_sources.map(|s| s.is_some()))
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

/// The interaction close's trade leg (`0x512ea4`): the partner setter with none, its target already zeroed, so its clear finds none (`0x5870f7`): the enchanting item unlocks, the partner zeroes and `TRADE_CLOSED` fires (`0x58710e`); then `CMSG_CANCEL_TRADE` goes out (`0x587116` -> `0x704040`).
pub(crate) fn close_by_interaction(world: &mut World) {
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        ui.write(|script| script.close_trade_partner(false));
        ui.write(benilla_ui::script::UiScript::trade_partner_closed);
    }
    if let Some(mut trade) = world.get_resource_mut::<TradeSession>() {
        trade.close_window();
    }
    if let Some(net) = world.get_resource::<NetCommands>() {
        let _ = net.0.send(ClientCommand::CancelTrade);
    }
}

/// The world leave's trade part (`0x528d05` -> `0x5880c0`): the offer's items unlock, an open window releases the enchanting item, drops the interact target and closes with `TRADE_CLOSED`, and the cancel goes out twice.
pub(crate) fn leave_world(world: &mut World) {
    use crate::ui_session::NpcSession;
    let partner = world
        .get_resource::<TradeSession>()
        .and_then(|trade| trade.npc());
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        ui.write(benilla_ui::script::UiScript::unlock_trade_items);
        if let Some(partner) = partner {
            ui.write(|script| script.close_trade_partner(false));
            ui.world(move |w| crate::ui_session::clear_interact_target(w, partner));
            ui.write(benilla_ui::script::UiScript::trade_partner_closed);
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

/// The UI world entry's trade part (`0x5281ef` -> `0x5860a0`): the window's state zeroes.
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        ui.write(benilla_ui::script::UiScript::enter_trade_world);
    }
}

/// A line whose `%s` is a unit's name as its live object reads it (`0x72a000`): the name cache's
/// entry, asked on a miss, and `UNKNOWNOBJECT` meanwhile (`0x72a271`), so the line shows at its step.
pub(crate) fn unit_name_line(
    key: &'static str,
    who: u64,
    names: Option<&NameCache>,
    commands: &NetCommands,
) -> crate::ui_action::UiError {
    use crate::ui_action::{FillArg, UiError};
    match names.and_then(|n| n.resolve(who, commands)) {
        Some(name) => UiError::s(key, name.to_string()),
        None => UiError::args(key, vec![FillArg::Global("UNKNOWNOBJECT".into())]),
    }
}

/// `ERR_INITIATE_TRADE_S` off the name cache at the initiate (`0x703db6`, `0x703dc5`), or, on a
/// miss, owed to the name's callback (`0x703c60`, [`net`]'s name handler).
pub(crate) fn initiate_line(
    who: u64,
    trade: &mut TradeSession,
    names: &NameCache,
    commands: &NetCommands,
    sink: &mut dyn SignalSink,
) {
    const KEY: &str = "ERR_INITIATE_TRADE_S";
    match names.resolve(who, commands).map(str::to_string) {
        Some(name) => sink.line(crate::ui_action::UiError::s(KEY, name)),
        None => trade.owe_named_line(NamedLine { key: KEY, who }),
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
                    answer_trade_request.before(crate::ui_script::UiPress),
                    crate::ui_script::raises::then_fire(drain_trade).in_set(UiDrain),
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
    mut signals: crate::ui_action::signals::QueuedSignals,
    mut ecs: Commands,
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

    // Leg 8, Block Trades, the one leg that names the initiator: the busy goes out (`0x58748b`),
    // then the line with the live unit's name, `UNKNOWNOBJECT` while uncached (`0x587496`).
    if block_trades.0 {
        info!(target: "trade", "BEGIN_TRADE from {initiator:#x} refused by BlockTrades; sending CMSG_BUSY_TRADE");
        let _ = commands.0.send(ClientCommand::BusyTrade);
        trade.refuse_request();
        // One error raise of `(0xbb, name)` (`0x5874a1`); catalog row `0xbb` makes it a chat line.
        signals.line(unit_name_line(
            "ERR_TRADE_BLOCKED_S",
            initiator,
            Some(&names),
            &commands,
        ));
        signals.flush(&mut ecs);
        return;
    }

    // Nothing refuses it: accept, the bottom of the ladder.
    info!(target: "trade", "BEGIN_TRADE from {initiator:#x} accepted; sending CMSG_BEGIN_TRADE");
    trade.begin(initiator);
    let _ = commands.0.send(ClientCommand::BeginTrade);
}

/// Drain the Lua intents into the trade `CMSG`s. `CloseTrade` clears the session and sends
/// `CMSG_CANCEL_TRADE`.
fn drain_trade(
    mut script: crate::ui_script::InTurn,
    mut trade: ResMut<TradeSession>,
    commands: Res<NetCommands>,
    selection: Res<Selection>,
    group: Res<GroupState>,
    names: Res<NameCache>,
) {
    let Some(mut script) = script.turn() else {
        return;
    };

    for token in script.take_trade_initiates() {
        match resolve_trade_target(&token, &selection, &group) {
            Some(guid) => {
                info!(target: "trade", "initiate: {token:?} -> {guid:#x}; sending CMSG_INITIATE_TRADE");
                // `InitiateTrade` asks for no cursor (`0x5226e1`); the drop on a player does (#1382).
                trade.initiate(guid, false);
                let _ = commands
                    .0
                    .send(ClientCommand::InitiateTrade { target: guid });
                initiate_line(guid, &mut trade, &names, &commands, &mut *script);
            }
            None => {
                info!(target: "trade", "initiate: {token:?} did not resolve to a player guid — nothing sent");
            }
        }
    }

    // The accept verbs' sends in call order, whatever the window's state (`0x58671a` -> `0x703f50`, `0x58676b` -> `0x703fd0`), and our accept's drop in an offer update (`0x58696b`).
    for accept in script.take_trade_accept_sends() {
        if accept {
            let _ = commands.0.send(ClientCommand::AcceptTrade {
                their_state: trade.their_state(),
            });
        } else {
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
                trade.offer_from(id, bag, slot);
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
        // The cancel goes out whatever the window's state (`0x5879c4` -> `0x704040`); the verb cleared the interact target in its call (`0x5879a5`).
        let _ = commands.0.send(ClientCommand::CancelTrade);
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

/// The player guid a UnitPopup token names, through [`crate::ui_unit::player_token_guid`].
fn resolve_trade_target(token: &str, selection: &Selection, group: &GroupState) -> Option<u64> {
    crate::ui_unit::player_token_guid(token, selection, group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{TradeStatus, TradeStatusExtended};
    use benilla_protocol::SessionEvent;

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

        s.request(0x2);
        assert_eq!(s.pending_request(), Some(0x2));
        assert_eq!(s.partner, Some(0x1), "the live trade is untouched");
        assert!(s.is_open());

        s.refuse_request();
        assert_eq!(s.pending_request(), None);
        assert_eq!(s.partner, Some(0x1), "refusing an offer is not cancelling");
        assert!(s.is_open());
    }

    #[test]
    fn extended_fills_the_right_side() {
        let mut s = TradeSession::default();
        s.begin(0x1);
        s.open_window(0);

        let mut slots = [None; TRADE_SLOT_COUNT];
        slots[0] = Some(wire_item(2589));
        let ext = snapshot_of(true, 500, slots);
        assert_eq!(s.set_offer(&ext), OfferUpdate::Applied);
        assert!(s.their.slots[0].is_some());
        assert_eq!(s.their.gold, 500);
        assert_eq!(s.our.gold, 0);
        assert_eq!(s.cells().target_gold, 500, "the cells the verbs read");

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

    /// Our offer's sources follow the sends: a placement records where the item sits, a clear forgets it (`0x5864d0` reads them).
    #[test]
    fn own_offer_sources_follow_the_sends() {
        let mut s = TradeSession::default();
        s.begin(0x7);
        s.open_window(0);
        s.offer_from(2, 0, 5);
        assert!(s.offers(0, 5));
        s.set_own_gold(500);
        assert_eq!(s.our.gold, 500);
        s.clear_own_item(2);
        assert!(!s.offers(0, 5), "clearing forgets the source");
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
            .init_resource::<NameCache>()
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
            .init_resource::<NameCache>()
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
        assert!(!trade.is_open());
        assert_eq!(trade.our.gold, 500);
    }

    /// An app running [`drain_trade`] with a live VM, the trade's handlers, and `0x7` named in the cache.
    fn drain_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut names = NameCache::default();
        names.insert_player(0x7, "Grubbis".to_string(), None);
        app.init_resource::<TradeSession>()
            .init_resource::<GroupState>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .insert_resource(names)
            .insert_resource(NetCommands(tx));
        net::register(&mut app);
        let vm = UiScript::new().unwrap();
        vm.run("ERR_INITIATE_TRADE_S = 'You have requested to trade with %s.'")
            .unwrap();
        app.insert_non_send_resource(vm);
        (app, rx)
    }

    /// Initiate `guid` through the verb and the drain.
    fn initiate(app: &mut App, guid: u64) {
        use bevy::ecs::system::RunSystemOnce;
        app.insert_resource(Selection {
            guid: Some(guid),
            ..Default::default()
        });
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("InitiateTrade('target')")
            .unwrap();
        app.world_mut().run_system_once(drain_trade).unwrap();
    }

    /// The system lines a handler's display added to the chat log (`0x509dd0`).
    fn chat(app: &App) -> Vec<String> {
        app.world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_lines()
    }

    fn shown(app: &mut App) -> Vec<&'static str> {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_shown_keys()
    }

    /// The initiate shows its line off the name cache at the send (`0x703db6`, `0x703dc5`); row `0xb9` is chat.
    #[test]
    fn initiating_a_trade_says_so() {
        let (mut app, _rx) = drain_app();
        initiate(&mut app, 0x7);
        assert_eq!(shown(&mut app), ["ERR_INITIATE_TRADE_S"]);
        assert_eq!(
            benilla_ui::messages::kind_of("ERR_INITIATE_TRADE_S"),
            benilla_ui::messages::MsgKind::Chat,
        );
        assert!(app
            .world_mut()
            .resource_mut::<TradeSession>()
            .take_named_lines_for_test()
            .is_empty());
    }

    /// A name not cached asks and waits: its answer runs the callback and shows the line, after a refusal closed the trade too (`0x703c60`); a negative answer shows nothing.
    #[test]
    fn the_initiate_line_waits_for_the_names_answer_and_survives_the_refusal() {
        let (mut app, _rx) = drain_app();
        initiate(&mut app, 0x99);
        assert!(
            shown(&mut app).is_empty(),
            "never a line with an unfilled %s"
        );
        app.world_mut()
            .resource_mut::<TradeSession>()
            .close_window();
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::PlayerName {
                guid: 0x99,
                name: "Skarrid".into(),
                realm: String::new(),
                race: 1,
                gender: 0,
                class: 1,
                declensions: None,
            }],
        );
        assert_eq!(chat(&app), ["You have requested to trade with Skarrid."]);

        initiate(&mut app, 0x98);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::PlayerNameMiss {
                guid: 0x98,
                miss: benilla_protocol::messages::NameQueryMiss::Deny,
            }],
        );
        assert_eq!(chat(&app).len(), 1, "a denied name shows nothing");
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
            chat(&app).is_empty(),
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
            chat(&app),
            ["Grubbis is blocking trades."],
            "the refusal raises the reference's own error `(0xbb, name)`, a chat line"
        );
        assert_eq!(
            benilla_ui::messages::kind_of("ERR_TRADE_BLOCKED_S"),
            benilla_ui::messages::MsgKind::Chat,
        );
    }

    /// An initiator whose name is not cached is refused at once, its line naming `UNKNOWNOBJECT` (`0x587496` -> `0x72a271`), with the name asked.
    #[test]
    fn block_trades_names_an_uncached_initiator_unknown() {
        let (mut app, rx) = request_app(true, false, true);
        let e = app.world_mut().spawn(ObjectStore::default()).id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(0x9, e);
        app.world_mut().resource_mut::<TradeSession>().request(0x9);
        app.update();
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(sent.iter().any(|c| matches!(c, ClientCommand::BusyTrade)));
        assert!(
            sent.iter()
                .any(|c| matches!(c, ClientCommand::NameQuery { guid: 0x9 })),
            "{sent:?}"
        );
        assert_eq!(chat(&app), ["Unknown is blocking trades."]);
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
                chat(&app).is_empty(),
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
        assert!(chat(&app).is_empty());
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
            .init_resource::<crate::ui_script::live::UiEvents>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
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
        let vm = UiScript::new().unwrap();
        vm.run("ERR_TRADE_BLOCKED_S = '%s is blocking trades.' UNKNOWNOBJECT = 'Unknown'")
            .unwrap();
        app.insert_non_send_resource(vm);
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
            .init_resource::<NameCache>()
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
            .init_resource::<NameCache>()
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
            vm.open_trade_partner(0x7, false);
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
