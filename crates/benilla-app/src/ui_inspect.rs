//! The inspect feed: another player's public descriptor as the slot views the inspect window's
//! Lua reads. Their inventory guids are server-private, so there are no item objects, only the
//! `PLAYER_VISIBLE_ITEM_*` entries and their templates, which is why the reference's inspect doll
//! shows no counts and no durability.
//!
//! `NotifyInspect(unit)` sends `CMSG_INSPECT`, which also sets the server-side selection, and
//! latches the token; the window paints without waiting for `SMSG_INSPECT`, a bare guid echo.
//! `UNIT_INVENTORY_CHANGED` and `UNIT_LEVEL` fire for the inspected token on change, a first
//! resolve included (`InspectPaperDollFrame.lua:2-5`, `82`).

use bevy::prelude::*;

use benilla_protocol::messages::InspectItem;
use benilla_protocol::{SessionEvent, SessionEventKind as K};
use benilla_ui::script::{InspectView, InvSlotView, InventorySlots, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore};
use crate::portrait::InspectBooth;
use crate::ui_script::UiFeed;

/// The reference's `InspectFrame.unit`. The token, not the guid, is the identity: the reference
/// re-reads it on every retarget, so `"target"` follows the selection.
#[derive(Resource, Default)]
pub(crate) struct InspectTarget {
    pub(crate) token: Option<String>,
}

/// `SMSG_INSPECT_TALENT`'s worn items of the last player inspected (`0x6cdf30`): their twelve
/// enchant slots, which the visible-item fields do not carry, and which count toward that player's
/// gem conditions.
#[derive(Resource, Default)]
struct InspectWorn {
    guid: u64,
    items: Vec<InspectItem>,
}

/// The last view pushed and level seen, per VM ([`crate::ui_script::VmMemo`]), so a `/reload`'s
/// new VM gets the view re-pushed and its events re-fired.
#[derive(Resource, Default)]
struct InspectFeedState {
    vm: crate::ui_script::VmMemo<InspectFeedMemo>,
}

/// The per-VM change bases.
#[derive(Default)]
struct InspectFeedMemo {
    last: Option<InspectView>,
    last_level: Option<u32>,
}

pub(crate) struct InspectUiPlugin;

impl Plugin for InspectUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InspectTarget>()
            .init_resource::<InspectFeedState>()
            .init_resource::<InspectWorn>()
            .net_handler(K::InspectItems, on_inspect_items)
            .add_systems(Update, feed_inspect.in_set(UiFeed));
    }
}

fn on_inspect_items(In(ev): In<SessionEvent>, mut worn: ResMut<InspectWorn>) {
    let SessionEvent::InspectItems { guid, items } = ev else {
        return;
    };
    debug!("inspect: {} worn items for {guid:#x}", items.len());
    *worn = InspectWorn { guid, items };
}

/// The inspected player's gem counts (`0x6de4d0`): meta, red, yellow and blue, each gem in a
/// counted item's sockets once per colour bit.
fn inspect_gem_counts(
    items: &[InspectItem],
    enchants: Option<&crate::items::Enchants>,
) -> [i32; 4] {
    let mut counts = [0; 4];
    let Some(enchants) = enchants else {
        return counts;
    };
    for item in items.iter().filter(|i| i.counts_gems) {
        for &id in &item.enchants[2..=4] {
            let Some(t) = enchants.1.gem_type(id).filter(|&t| t != 0) else {
                continue;
            };
            for (bit, n) in counts.iter_mut().enumerate() {
                if t & (1 << bit) != 0 {
                    *n += 1;
                }
            }
        }
    }
    counts
}

/// One inspected slot from the target's visible-item entry; `slot0` is the 0-based
/// `EQUIPMENT_SLOT_*` index, `slot0 + 1` in Lua. With no item object the count is the reference's
/// always-1 and durability, flags, locks and creator stay inert. Only the enchants broadcast, at
/// `PLAYER_VISIBLE_ITEM_<slot>_0 + 1 + j`, and 1.12 fills PERM and TEMP alone (vmangos
/// `SetVisibleItemSlot`, `MAX_INSPECTED_ENCHANTMENT_SLOT`).
fn inspect_slot_view(
    store: &ObjectStore,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    slot0: u8,
) -> Option<InvSlotView> {
    let entry = store.0.player_visible_item_entry(slot0)?;
    // The roll, the properties field's low word as the reference's inspect leg reads it into
    // `+0x424`, names the suffix only: the visible item carries no enchant slots past 1.
    let roll = store.0.player_visible_item_properties(slot0);
    // By entry alone (guid 0), as the reference's item cache asks: there is no item object.
    let (name, quality, display) = match items.template(entry, 0, commands) {
        Some(t) => (
            Some(rolls.name(&t.name, roll)),
            t.quality,
            t.display_info_id,
        ),
        None => (None, 0, 0),
    };
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link_full(entry, 0, roll, 0, n, quality));
    Some(InvSlotView {
        item_id: entry,
        icon: icons
            .and_then(|i| i.catalog.get(display))
            .and_then(|d| d.icon.clone()),
        count: 1,
        quality: quality as i32,
        name,
        link,
        // The reference reads an item object's `ITEM_FIELD_FLAGS`, which inspected gear lacks, so
        // the tooltip prints the template's bind line, as the reference's does.
        already_bound: false,
        // All 7 slots, as the reference's inspect leg copies them, with no charges and no
        // countdown: there is no item object.
        enchants: crate::items::enchant_lines(
            (0..7).map(|j| {
                let id = store.0.player_visible_item_enchant(slot0, j).unwrap_or(0);
                (j, id as i32, 0, None)
            }),
            rolls.enchants,
        ),
        ..Default::default()
    })
}

fn feed_inspect(
    script: Option<NonSendMut<UiScript>>,
    mut target: ResMut<InspectTarget>,
    mut feed: ResMut<InspectFeedState>,
    mut booth: ResMut<InspectBooth>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    // `SpellItemEnchantment` names the enchant lines, `ItemRandomProperties` the roll suffix.
    catalogs: (
        Option<Res<crate::items::Enchants>>,
        Option<Res<crate::items::RandomProperties>>,
    ),
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    selection: Res<crate::target::Selection>,
    group: Res<crate::ui_party::GroupState>,
    worn: Res<InspectWorn>,
) {
    let Some(mut script) = script else {
        return;
    };
    // A `/reload` keeps the latched token, and the fresh memo re-pushes the view.
    let memo = feed.vm.get(&script);

    // `NotifyInspect(unit)`: send `CMSG_INSPECT` for the unit's guid and latch its token.
    for token in script.take_inspect_notifies() {
        match crate::ui_unit::player_token_guid(&token, &selection, &group) {
            Some(guid) => {
                debug!("inspect: {token:?} -> {guid:#x}; sending CMSG_INSPECT");
                let _ = commands.0.send(ClientCommand::Inspect { target: guid });
                target.token = Some(token);
            }
            None => {
                debug!("inspect: {token:?} did not resolve to a player guid — nothing sent");
            }
        }
    }
    // `ClearInspectPlayer()`, from `InspectFrame_OnHide`: the resolve stops and the booth empties.
    if script.take_inspect_clear() {
        target.token = None;
    }

    // Re-resolve the latched token every frame, so the window follows a retarget as the
    // reference's `PLAYER_TARGET_CHANGED` arm does.
    let resolved = target.token.as_ref().and_then(|token| {
        let guid = crate::ui_unit::player_token_guid(token, &selection, &group)?;
        let entity = *index.0.get(&guid)?;
        Some((token.clone(), guid, entity))
    });

    let Some((token, guid, entity)) = resolved else {
        // Nothing inspected, or the target is not streamed: clear the view and empty the booth.
        if memo.last.is_some() {
            script.set_inspect(None);
            memo.last = None;
            memo.last_level = None;
        }
        booth.unit = None;
        return;
    };

    booth.unit = Some(entity);
    // The stock Lua turns the doll through the pane (`InspectModelFrame:SetRotation`).
    booth.yaw = script.model_pane_facing("InspectModelFrame");

    let Ok(store) = stores.get(entity) else {
        return;
    };
    let mut slots: InventorySlots = Default::default();
    for slot in 1..=19u8 {
        slots[usize::from(slot)] = inspect_slot_view(
            store,
            &items,
            icons.as_deref(),
            crate::items::RollCatalogs {
                enchants: catalogs.0.as_deref(),
                props: catalogs.1.as_deref(),
            },
            &commands,
            slot - 1,
        );
    }
    // The inspect packet's worn items replace the visible enchants with all twelve slots.
    let worn = if worn.guid == guid {
        worn.items.as_slice()
    } else {
        &[]
    };
    for item in worn {
        let Some(view) = slots
            .get_mut(usize::from(item.slot) + 1)
            .and_then(Option::as_mut)
            .filter(|v| v.item_id == item.entry)
        else {
            continue;
        };
        view.enchants = crate::items::enchant_lines(
            (0..12u8).map(|j| (j, item.enchants[usize::from(j)] as i32, 0, None)),
            catalogs.0.as_deref(),
        );
    }
    let view = InspectView {
        unit: token.clone(),
        guid,
        slots,
        gem_counts: inspect_gem_counts(worn, catalogs.0.as_deref()),
    };
    if memo.last.as_ref() != Some(&view) {
        script.set_inspect(Some(view.clone()));
        memo.last = Some(view);
        // `InspectPaperDollItemSlotButton_OnEvent` filters it on `arg1` being the inspected unit.
        script.fire_event(
            "UNIT_INVENTORY_CHANGED",
            vec![ScriptValue::Str(token.clone())],
        );
    }
    let level = store.0.unit_level();
    if memo.last_level != level {
        memo.last_level = level;
        script.fire_event("UNIT_LEVEL", vec![ScriptValue::Str(token)]);
    }
}
