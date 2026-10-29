//! The currency feed: `CurrencyTypes.dbc` and `CurrencyCategory.dbc`, and the player's known
//! currencies, token slots, point balances and currency item names into the VM. 12340 rebuilds the
//! list and fires `CURRENCY_DISPLAY_UPDATE` at world entry, on a token slot change and when a
//! currency item's record lands (the currency table's world entry, `0x5b0660`), and
//! `KNOWN_CURRENCY_TYPES_UPDATE` on the known-currencies watch (`0x5b0640`).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::CurrencyTables;
use benilla_protocol::field::{FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1, FIELD_PLAYER_KNOWN_CURRENCIES};
use benilla_protocol::messages::CURRENCY_TOKEN_SLOTS;
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

/// Whether a field edge is the known-currencies u64 or a token slot.
fn edge_kind(index: u16) -> (bool, bool) {
    let known = (FIELD_PLAYER_KNOWN_CURRENCIES..FIELD_PLAYER_KNOWN_CURRENCIES + 2).contains(&index);
    let token = (FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1
        ..FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 2 * u16::from(CURRENCY_TOKEN_SLOTS))
        .contains(&index);
    (known, token)
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
    mut memory: Local<VmMemo<u64>>,
) {
    let entered = entered.read().count() > 0;
    let (Some(mut script), Some(tables)) = (script, tables) else {
        edges.clear();
        return;
    };
    let (epoch, vm_reset) = memory.get_reset(&script);
    if vm_reset || tables.is_changed() {
        script.set_currency_catalog(catalog_view(&tables.0));
    }
    let Some((me, store)) = self_q.iter().next() else {
        edges.clear();
        script.set_currency_holdings(None);
        return;
    };
    let (mut known_moved, mut tokens_moved, mut any_edge) = (false, false, false);
    for edge in edges.read() {
        // A token's stack count rides its own item object.
        any_edge = true;
        if edge.entity == me {
            let (k, t) = edge_kind(edge.index);
            known_moved |= k;
            tokens_moved |= t;
        }
    }
    // A landed currency record repaints the list, as the cache callback does (`0x5b01d7`).
    let template_epoch = items.template_epoch();
    let records_landed = std::mem::replace(epoch, template_epoch) != template_epoch;
    if vm_reset || entered || records_landed || any_edge || !changed_self.is_empty() {
        script.set_currency_holdings(Some(holdings(
            store,
            &objects,
            &tables.0,
            &items,
            icons.as_deref(),
            &commands,
        )));
    }
    if known_moved {
        script.fire_event("KNOWN_CURRENCY_TYPES_UPDATE", vec![]);
    }
    if entered || tokens_moved || records_landed {
        script.fire_event("CURRENCY_DISPLAY_UPDATE", vec![]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watched_edges_are_the_known_word_pair_and_the_token_slots() {
        assert_eq!(edge_kind(FIELD_PLAYER_KNOWN_CURRENCIES + 1), (true, false));
        assert_eq!(
            edge_kind(FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 63),
            (false, true)
        );
        assert_eq!(
            edge_kind(FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 64),
            (false, false)
        );
    }
}
