//! The currency tables, `CurrencyTypes.dbc` and `CurrencyCategory.dbc`, our currencies as the verbs read them at the call, and 12340's triggers: each rebuilds the display list, then fires `CURRENCY_DISPLAY_UPDATE` (`0x5b0660`) at world entry (`0x528299`), on a class 10 item's count (`0x707832`), on a watched slot (`0x5d991f`) and when a listed row's record lands (`0x5b01d7`, `0x5b0260`), or `KNOWN_CURRENCY_TYPES_UPDATE` (`0x5b0640`) on the known-currencies watch (`0x6e5401`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::CurrencyTables;
use benilla_protocol::messages::{ObjectFields, CURRENCY_TOKEN_SLOTS};
use benilla_ui::script::{
    CurrencyCatalogView, CurrencyCategoryView, CurrencyHoldings, CurrencyTypeView, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::NetCommands;
use crate::ui_script::live::UiEvents;

/// The currency tables as the verbs read them, absent when the client data failed to load.
#[derive(Resource)]
pub(crate) struct Currencies(pub(crate) CurrencyCatalogView);

pub(crate) struct UiCurrencyPlugin;

impl Plugin for UiCurrencyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_currencies.after(AssetSet::Open));
    }
}

fn load_currencies(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_currency_tables(&mut chain)
    };
    match loaded {
        Ok(t) => commands.insert_resource(Currencies(catalog_view(&t))),
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

/// What the currency verbs read live: the tables, the item cache and the bag icons, and the wire for a build's record asks.
#[derive(SystemParam)]
pub(crate) struct CurrencyLive<'w> {
    tables: Option<Res<'w, Currencies>>,
    items: Option<Res<'w, Items>>,
    icons: Option<Res<'w, ItemDisplays>>,
    commands: Option<Res<'w, NetCommands>>,
}

impl CurrencyLive<'_> {
    /// The tables, `None` before they load.
    pub(crate) fn catalog(&self) -> Option<&CurrencyCatalogView> {
        self.tables.as_deref().map(|t| &t.0)
    }

    /// Our currencies at the call, read off our fields, the token slots' items and the item cache; `object` finds a held object's fields by guid.
    pub(crate) fn holdings<'a>(
        &self,
        me: &ObjectFields,
        object: impl Fn(u64) -> Option<&'a ObjectFields>,
    ) -> CurrencyHoldings {
        let tokens = (0..CURRENCY_TOKEN_SLOTS)
            .filter_map(|i| me.player_currency_token_slot(i).filter(|&g| g != 0))
            .filter_map(object)
            .filter_map(|o| Some((o.object_entry()?, o.item_stack_count().unwrap_or(1))))
            .collect();
        let known = me.player_known_currencies();
        let items = self
            .catalog()
            .map(|c| {
                c.types
                    .iter()
                    .filter(|t| {
                        (1..=64).contains(&t.bit_index) && known & (1 << (t.bit_index - 1)) != 0
                    })
                    .map(|t| {
                        let rec = self
                            .items
                            .as_deref()
                            .and_then(|i| i.template_cached(t.item_id));
                        let name = rec.map(|r| r.name.clone());
                        let icon = rec.and_then(|r| {
                            crate::ui_items::item_icon(self.icons.as_deref(), r.display_info_id)
                        });
                        (t.item_id, (name, icon))
                    })
                    .collect()
            })
            .unwrap_or_default();
        CurrencyHoldings {
            known,
            tokens,
            honor: me.player_honor_currency().unwrap_or(0),
            arena: me.player_arena_currency().unwrap_or(0),
            items,
        }
    }

    /// Ask for an uncached record, once per entry as the cache does.
    pub(crate) fn ask(&self, id: u32) {
        if let (Some(items), Some(commands)) = (self.items.as_deref(), self.commands.as_deref()) {
            items.template(id, 0, commands);
        }
    }
}

/// The item class the currency list holds.
const CURRENCY_CLASS: u32 = 10;

/// The rebuild, then `CURRENCY_DISPLAY_UPDATE` (`0x5b0660`), in turn with the handler's events.
pub(crate) fn display_update(ui: &mut UiEvents) {
    ui.write(|script: &mut UiScript| {
        script.rebuild_currency_list();
        script.fire_event("CURRENCY_DISPLAY_UPDATE", vec![]);
    });
}

/// The UI's world entry (`0x528299`): the rebuild and `CURRENCY_DISPLAY_UPDATE`, past the stable's (`0x528244`).
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        display_update(&mut ui);
    }
}

/// The known-currencies watch (`0x6e5401`): the rebuild, then `KNOWN_CURRENCY_TYPES_UPDATE` (`0x5b0640`).
pub(crate) fn known_update(ui: &mut UiEvents) {
    ui.write(|script: &mut UiScript| {
        script.rebuild_currency_list();
        script.fire_event("KNOWN_CURRENCY_TYPES_UPDATE", vec![]);
    });
}

/// Whether a player slot's change reaches the currency list: the backpack, keyring and token slots carry its handler (`0x5d9c5b`, `0x5d9cb9`, `0x5d9ce8`).
pub(crate) fn watched_slot(slot: u8) -> bool {
    matches!(slot, 23..=38 | 86..=149)
}

/// The slot handler's currency leg (`0x5d98ee`-`0x5d991f`): the old or the new item is class 10 by its object's class (`0x5d98f2`, `0x5d9905`), or the slot is a token slot (`0x5d9915`).
pub(crate) fn slot_fires(
    slot: u8,
    old: u64,
    new: u64,
    class_of: impl Fn(u64) -> Option<u32>,
) -> bool {
    let class_10 = |guid: u64| guid != 0 && class_of(guid) == Some(CURRENCY_CLASS);
    class_10(old) || class_10(new) || (118..=149).contains(&slot)
}

/// An item's stack count moved (`0x707720`): our own item whose cached record is class 10 fires (`0x70782c` -> `0x707832`); an uncached record is read without an ask.
pub(crate) fn item_count_fires(
    owner: Option<u64>,
    me: Option<u64>,
    record_class: Option<u32>,
) -> bool {
    owner.is_some() && owner == me && record_class == Some(CURRENCY_CLASS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BADGE: u64 = 0x100;
    const CLOTH: u64 = 0x101;

    fn class(guid: u64) -> Option<u32> {
        match guid {
            BADGE => Some(10),
            CLOTH => Some(7),
            _ => None,
        }
    }

    /// A class 10 item into or out of a backpack or keyring slot fires; a non-token there does not; any change in a token slot fires whatever the item.
    #[test]
    fn the_slot_leg_fires_on_a_token_either_way_or_a_token_slot() {
        assert!(slot_fires(23, 0, BADGE, class));
        assert!(slot_fires(23, BADGE, 0, class), "the old item counts");
        assert!(slot_fires(100, 0, BADGE, class), "the keyring is watched");
        assert!(!slot_fires(23, 0, CLOTH, class));
        assert!(
            slot_fires(149, 0, CLOTH, class),
            "a token slot fires for any item"
        );
        assert!(slot_fires(118, CLOTH, 0, class));
        assert!(
            !slot_fires(23, 0, 0x999, class),
            "an unheld object has no class"
        );
        assert!(watched_slot(23) && watched_slot(149) && !watched_slot(39) && !watched_slot(22));
    }

    /// Our class 10 item's count fires; a non-token's, someone else's or an uncached record's not.
    #[test]
    fn a_token_count_change_fires_and_a_non_token_one_does_not() {
        assert!(item_count_fires(Some(1), Some(1), Some(10)));
        assert!(!item_count_fires(Some(1), Some(1), Some(7)));
        assert!(!item_count_fires(Some(2), Some(1), Some(10)));
        assert!(
            !item_count_fires(Some(1), Some(1), None),
            "read without an ask"
        );
        assert!(!item_count_fires(None, None, Some(10)));
    }
}
