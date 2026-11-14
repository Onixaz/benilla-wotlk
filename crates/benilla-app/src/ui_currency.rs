//! The currency feed: `CurrencyTypes.dbc` and `CurrencyCategory.dbc`, and the player's known
//! currencies, token slots, point balances and currency item names into the VM. 12340 rebuilds the
//! list and fires `CURRENCY_DISPLAY_UPDATE` (`0x5b0660`) at world entry, on a class 10 item's
//! count change, on a watched slot change and when a listed row's record lands, and
//! `KNOWN_CURRENCY_TYPES_UPDATE` on the known-currencies watch (`0x5b0640`).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::CurrencyTables;
use benilla_protocol::field::{
    player_slot_of_field, FIELD_ITEM_STACK_COUNT, FIELD_PLAYER_KNOWN_CURRENCIES,
};
use benilla_protocol::messages::{ObjectFields, ObjectType, CURRENCY_TOKEN_SLOTS};
use benilla_ui::script::{
    CurrencyCatalogView, CurrencyCategoryView, CurrencyHoldings, CurrencyTypeView, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{FieldChanged, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_script::VmMemo;
use crate::ui_unit::UnitFeed;

/// The currency tables, absent when the client data failed to load.
#[derive(Resource)]
pub(crate) struct Currencies(pub(crate) CurrencyTables);

pub(crate) struct UiCurrencyPlugin;

impl Plugin for UiCurrencyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_currencies.after(AssetSet::Open))
            .add_systems(Update, feed_currencies.in_set(UnitFeed));
    }
}

fn load_currencies(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_currency_tables(&mut chain)
    };
    match loaded {
        Ok(t) => commands.insert_resource(Currencies(t)),
        Err(e) => warn!("ui_currency: CurrencyTypes.dbc failed to load — no currencies: {e:#}"),
    }
}

fn catalog_view(t: &CurrencyTables) -> CurrencyCatalogView {
    CurrencyCatalogView {
        types: t
            .types
            .iter()
            .map(|r| CurrencyTypeView {
                item_id: r.item_id,
                category_id: r.category_id,
                bit_index: r.bit_index,
            })
            .collect(),
        categories: t
            .categories
            .iter()
            .map(|c| CurrencyCategoryView {
                id: c.id,
                flags: c.flags,
                name: c.name.clone(),
            })
            .collect(),
    }
}

/// The player's side; asks for an uncached currency item's record on the way.
fn holdings(
    store: &ObjectStore,
    objects: &Objects,
    tables: &CurrencyTables,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> CurrencyHoldings {
    let f = &store.0;
    let tokens = (0..CURRENCY_TOKEN_SLOTS)
        .filter_map(|i| f.player_currency_token_slot(i).filter(|&g| g != 0))
        .filter_map(|g| objects.object(g))
        .filter_map(|o| Some((o.object_entry()?, o.item_stack_count().unwrap_or(1))))
        .collect();
    let known = f.player_known_currencies();
    // Only a known row is listed, and only its record is asked for (`0x5b0030`).
    let currency_items: HashMap<u32, (Option<String>, Option<String>)> = tables
        .types
        .iter()
        .filter(|t| (1..=64).contains(&t.bit_index) && known & (1 << (t.bit_index - 1)) != 0)
        .map(|t| {
            let rec = items.template(t.item_id, 0, commands);
            let name = rec.map(|r| r.name.clone());
            let icon = rec
                .and_then(|r| icons.and_then(|i| i.catalog.get(r.display_info_id)))
                .and_then(|d| d.icon.clone());
            (t.item_id, (name, icon))
        })
        .collect();
    CurrencyHoldings {
        known,
        tokens,
        honor: f.player_honor_currency().unwrap_or(0),
        arena: f.player_arena_currency().unwrap_or(0),
        items: currency_items,
    }
}

/// The item class the currency list holds.
const CURRENCY_CLASS: u32 = 10;

/// What the feed remembers across frames: the template epoch, and the listed rows' items whose
/// record is still asked for.
#[derive(Default)]
struct CurrencyMemo {
    epoch: u64,
    waiting: HashSet<u32>,
}

/// Whether a player slot's change reaches the currency list: the backpack, keyring and token slots
/// carry its handler (`0x5d9c5b`, `0x5d9cb9`, `0x5d9ce8`).
fn watched_slot(slot: u8) -> bool {
    matches!(slot, 23..=38 | 86..=149)
}

/// One frame's edges against 12340's triggers, as (known currencies moved, display triggers), one
/// display trigger per change since each handler fires its own event. `item` is an object's entry
/// and owner, `dbc_class` an entry's `Item.dbc` class, `record_class` its cached record's class.
fn currency_edges<'a>(
    edges: impl IntoIterator<Item = &'a FieldChanged>,
    me: Entity,
    store: &ObjectFields,
    item: impl Fn(u64) -> Option<(u32, Option<u64>)>,
    dbc_class: impl Fn(u32) -> Option<u32>,
    record_class: impl Fn(u32) -> Option<u32>,
) -> (bool, usize) {
    let me_guid = store.object_guid();
    let (mut known, mut display) = (false, 0);
    // Each watched slot's old guid words, the high one second.
    let mut slots: Vec<(u8, [Option<u32>; 2])> = Vec::new();
    for e in edges {
        if e.entity == me {
            known |= (FIELD_PLAYER_KNOWN_CURRENCIES..FIELD_PLAYER_KNOWN_CURRENCIES + 2)
                .contains(&e.index);
            let Some((slot, hi)) = player_slot_of_field(e.index).filter(|&(s, _)| watched_slot(s))
            else {
                continue;
            };
            let at = slots
                .iter()
                .position(|&(s, _)| s == slot)
                .unwrap_or_else(|| {
                    slots.push((slot, [None, None]));
                    slots.len() - 1
                });
            slots[at].1[usize::from(hi)] = Some(e.old);
        } else if matches!(e.kind, ObjectType::Item | ObjectType::Container)
            && e.index == FIELD_ITEM_STACK_COUNT
        {
            // Our own item's count, when its cached record is class 10 (`0x70782c`).
            display += usize::from(item(e.guid).is_some_and(|(entry, owner)| {
                owner.is_some() && owner == me_guid && record_class(entry) == Some(CURRENCY_CLASS)
            }));
        }
    }
    let class_10 = |guid: u64| {
        guid != 0 && item(guid).is_some_and(|(entry, _)| dbc_class(entry) == Some(CURRENCY_CLASS))
    };
    for (slot, [lo, hi]) in slots {
        let new = store.player_slot(slot).unwrap_or(0);
        let old =
            u64::from(lo.unwrap_or(new as u32)) | u64::from(hi.unwrap_or((new >> 32) as u32)) << 32;
        // The old or new item is class 10 by `Item.dbc`, or the slot is a token slot (`0x5d98f2`).
        display += usize::from(slot >= 118 || class_10(old) || class_10(new));
    }
    (known, display)
}

/// How many waiting records landed for a listed row, dropping every landed one from `waiting`: the
/// build's cache callback per record, answered or denied (`0x5b01d7`, `0x5b0260`).
fn listed_record_landed(
    waiting: &mut HashSet<u32>,
    landed: impl Fn(u32) -> bool,
    listed: impl FnOnce() -> Vec<u32>,
) -> usize {
    let arrived: Vec<u32> = waiting.iter().copied().filter(|&id| landed(id)).collect();
    if arrived.is_empty() {
        return 0;
    }
    waiting.retain(|id| !arrived.contains(id));
    let listed = listed();
    arrived.iter().filter(|id| listed.contains(id)).count()
}

#[allow(clippy::too_many_arguments)]
fn feed_currencies(
    script: Option<NonSendMut<UiScript>>,
    tables: Option<Res<Currencies>>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    objects: Objects,
    self_q: Query<(Entity, &ObjectStore), With<SelfPlayer>>,
    changed_self: Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
    mut edges: MessageReader<FieldChanged>,
    mut entered: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut memory: Local<VmMemo<CurrencyMemo>>,
) {
    let entered = entered.read().count() > 0;
    let (Some(mut script), Some(tables)) = (script, tables) else {
        edges.clear();
        return;
    };
    let (memo, vm_reset) = memory.get_reset(&script);
    if vm_reset || tables.is_changed() {
        script.set_currency_catalog(catalog_view(&tables.0));
    }
    let Some((me, store)) = self_q.iter().next() else {
        edges.clear();
        script.set_currency_holdings(None);
        return;
    };
    // A token's stack count rides its own item object, so any edge refreshes the holdings.
    let any_edge = !edges.is_empty();
    let (known_moved, display_moved) = currency_edges(
        edges.read(),
        me,
        &store.0,
        |g| {
            objects
                .object(g)
                .and_then(|o| Some((o.object_entry()?, o.item_owner())))
        },
        |entry| {
            let row = icons.as_deref()?.item_dbc.get(&entry)?;
            Some(u32::from(row.class))
        },
        |entry| items.template_cached(entry).map(|r| r.class),
    );
    let template_epoch = items.template_epoch();
    let epoch_moved = std::mem::replace(&mut memo.epoch, template_epoch) != template_epoch;
    let records_landed = if epoch_moved {
        listed_record_landed(
            &mut memo.waiting,
            |id| items.template_cached(id).is_some() || items.template_answered_unknown(id),
            || script.currency_listed_items(),
        )
    } else {
        0
    };
    if vm_reset || entered || epoch_moved || any_edge || !changed_self.is_empty() {
        let h = holdings(
            store,
            &objects,
            &tables.0,
            &items,
            icons.as_deref(),
            &commands,
        );
        memo.waiting = h
            .items
            .iter()
            .filter(|(&id, (name, _))| name.is_none() && !items.template_answered_unknown(id))
            .map(|(&id, _)| id)
            .collect();
        script.set_currency_holdings(Some(h));
    }
    if known_moved {
        script.fire_event("KNOWN_CURRENCY_TYPES_UPDATE", vec![]);
    }
    // One event per trigger: the world entry, each item and slot change, each landed record.
    for _ in 0..usize::from(entered) + display_moved + records_landed {
        script.fire_event("CURRENCY_DISPLAY_UPDATE", vec![]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1;

    const ME: u64 = 0x10;
    const BADGE: u64 = 0x100;
    const CLOTH: u64 = 0x101;
    const THEIR_BADGE: u64 = 0x102;

    /// Two entities: our player and an item object.
    fn entities() -> (Entity, Entity) {
        let mut world = World::new();
        (world.spawn_empty().id(), world.spawn_empty().id())
    }

    /// The low guid word of player slot `slot`.
    fn slot_field(slot: u8) -> u16 {
        (0..u16::MAX)
            .find(|&i| player_slot_of_field(i) == Some((slot, false)))
            .unwrap()
    }

    /// Our player with `slots` holding item guids.
    fn player(slots: &[(u8, u64)]) -> ObjectFields {
        let mut pairs = vec![(0, ME as u32), (1, 0)];
        for &(slot, guid) in slots {
            pairs.push((slot_field(slot), guid as u32));
            pairs.push((slot_field(slot) + 1, (guid >> 32) as u32));
        }
        ObjectFields::from_pairs(&pairs)
    }

    fn edge(entity: Entity, guid: u64, kind: ObjectType, index: u16, old: u32) -> FieldChanged {
        FieldChanged {
            entity,
            guid,
            kind,
            index,
            old,
            new: 0,
        }
    }

    /// The badge is class 10 both ways, the cloth class 7; the third item is someone else's badge.
    fn run(me: Entity, store: &ObjectFields, edges: &[FieldChanged]) -> (bool, usize) {
        currency_edges(
            edges,
            me,
            store,
            |g| match g {
                BADGE => Some((29434, Some(ME))),
                CLOTH => Some((2589, Some(ME))),
                THEIR_BADGE => Some((29434, Some(0x20))),
                _ => None,
            },
            |entry| Some(if entry == 29434 { 10 } else { 7 }),
            |entry| (entry != 0).then_some(if entry == 29434 { 10 } else { 7 }),
        )
    }

    /// Our class 10 item's count fires; a non-token's, someone else's or an uncached record's not.
    #[test]
    fn a_token_count_change_fires_and_a_non_token_one_does_not() {
        let (me, item) = entities();
        let store = player(&[]);
        let count = |guid| edge(item, guid, ObjectType::Item, FIELD_ITEM_STACK_COUNT, 3);
        assert_eq!(run(me, &store, &[count(BADGE)]), (false, 1));
        assert_eq!(run(me, &store, &[count(CLOTH)]), (false, 0));
        assert_eq!(run(me, &store, &[count(THEIR_BADGE)]), (false, 0));
        let uncached = currency_edges(
            &[count(BADGE)],
            me,
            &store,
            |_| Some((29434, Some(ME))),
            |_| Some(10),
            |_| None,
        );
        assert_eq!(uncached, (false, 0), "the record is read without an ask");
    }

    /// A class 10 item into or out of a backpack slot fires, by its `Item.dbc` class; the same item
    /// in a bank slot, or a non-token in the backpack, does not.
    #[test]
    fn a_class_10_item_in_a_watched_non_token_slot_fires() {
        let (me, _) = entities();
        let slot_edge = |slot, old| edge(me, ME, ObjectType::Player, slot_field(slot), old);
        assert_eq!(
            run(me, &player(&[(23, BADGE)]), &[slot_edge(23, 0)]),
            (false, 1)
        );
        assert_eq!(
            run(me, &player(&[]), &[slot_edge(23, BADGE as u32)]),
            (false, 1),
            "the old item counts"
        );
        assert_eq!(
            run(me, &player(&[(100, BADGE)]), &[slot_edge(100, 0)]),
            (false, 1),
            "the keyring is watched"
        );
        assert_eq!(
            run(me, &player(&[(39, BADGE)]), &[slot_edge(39, 0)]),
            (false, 0)
        );
        assert_eq!(
            run(me, &player(&[(23, CLOTH)]), &[slot_edge(23, 0)]),
            (false, 0)
        );
    }

    /// Any change in a token slot fires, whatever the item's class.
    #[test]
    fn a_token_slot_change_fires_for_any_item() {
        let (me, _) = entities();
        let store = player(&[(149, CLOTH)]);
        let e = edge(
            me,
            ME,
            ObjectType::Player,
            FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 62,
            0,
        );
        assert_eq!(run(me, &store, &[e]), (false, 1));
    }

    /// The known-currencies pair moves the known types, not the display.
    #[test]
    fn the_known_pair_moves_the_known_types_only() {
        let (me, _) = entities();
        let e = edge(
            me,
            ME,
            ObjectType::Player,
            FIELD_PLAYER_KNOWN_CURRENCIES + 1,
            0,
        );
        assert_eq!(run(me, &player(&[]), &[e]), (true, 0));
    }

    /// An unrelated landing keeps the wait; a waiting record fires once listed, and a folded row's
    /// landing ends its wait without firing.
    #[test]
    fn only_a_listed_rows_record_landing_fires() {
        let mut waiting = HashSet::from([29434, 40752]);
        assert_eq!(
            listed_record_landed(&mut waiting, |id| id == 2589, || vec![29434, 40752]),
            0
        );
        assert_eq!(waiting.len(), 2);
        assert_eq!(
            listed_record_landed(&mut waiting, |id| id == 29434, || vec![29434, 40752]),
            1
        );
        assert_eq!(
            listed_record_landed(&mut waiting, |id| id == 40752, || vec![29434]),
            0
        );
        assert!(waiting.is_empty());
    }

    /// Each trigger in one frame fires its own event: two token counts and a token slot are three.
    #[test]
    fn each_trigger_in_a_frame_counts_once() {
        let (me, item) = entities();
        let store = player(&[(118, CLOTH)]);
        let count = |guid| edge(item, guid, ObjectType::Item, FIELD_ITEM_STACK_COUNT, 3);
        let slot = edge(
            me,
            ME,
            ObjectType::Player,
            FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1,
            0,
        );
        let both_words = edge(
            me,
            ME,
            ObjectType::Player,
            FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 1,
            0,
        );
        assert_eq!(
            run(me, &store, &[count(BADGE), count(BADGE), slot, both_words]),
            (false, 3),
            "a slot's two guid words are one change"
        );
        let mut waiting = HashSet::from([29434, 40752, 43228]);
        assert_eq!(
            listed_record_landed(&mut waiting, |_| true, || vec![29434, 40752]),
            2
        );
    }
}
