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
use benilla_ui::script::{InspectView, InvSlotView, InventorySlots, ScriptValue};

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

/// The inspect target (`0xc24220`): set by `NotifyInspect` (`0x5e76cb`) and cleared by `ClearInspectPlayer` (`0x5e78ae`) when called.
#[derive(Resource, Default)]
pub(crate) struct InspectGuid(u64);

impl InspectGuid {
    pub(crate) fn get(&self) -> u64 {
        self.0
    }

    /// Set the target; whether it moved (`0x5e76c1`).
    pub(crate) fn set(&mut self, guid: u64) -> bool {
        std::mem::replace(&mut self.0, guid) != guid
    }
}

/// The world leave's inspect part (`0x5eaf70`): a held target, its honor reply, latch, teams and talent target go (`0x5eaff5`-`0x5eb022`).
pub(crate) fn leave_world(world: &mut World) {
    if !world
        .get_resource_mut::<InspectGuid>()
        .is_some_and(|mut g| g.set(0))
    {
        return;
    }
    // The talent target's reset with no player (`0x5eb022`).
    if let Some(mut talents) = world.get_resource_mut::<crate::ui_talent::TalentState>() {
        talents.0.set_inspect_target(0, None);
    }
    if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
        ui.write(|s| {
            s.set_inspect_honor(None);
            s.clear_inspect_arena_teams();
        });
    }
}

/// What the inspect verbs reach when called: the target and the send.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct InspectLive<'w> {
    guid: Option<ResMut<'w, InspectGuid>>,
    net: Option<Res<'w, NetCommands>>,
}

impl InspectLive<'_> {
    /// `CMSG_INSPECT` (`0x5228c3`), then the target (`0x52291a`); whether it moved (`0x5e76c1`), where the caller resets the talent target (`0x5e76f2`).
    pub(crate) fn notify(&mut self, guid: u64) -> bool {
        if let Some(net) = self.net.as_deref() {
            let _ = net.0.send(ClientCommand::Inspect { target: guid });
        }
        self.guid.as_mut().is_none_or(|g| g.set(guid))
    }

    /// `ClearInspectPlayer`'s target clear (`0x5e78ae`); whether one was held, where the caller resets the talent target (`0x5e78d5`).
    pub(crate) fn clear(&mut self) -> bool {
        self.guid.as_mut().is_some_and(|g| g.set(0))
    }
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
            .init_resource::<InspectGuid>()
            .init_resource::<InspectFeedState>()
            .init_resource::<InspectWorn>()
            .net_handler(K::InspectItems, on_inspect_items)
            .net_handler(K::InspectTalents, on_inspect_talents)
            .add_systems(Update, feed_inspect.in_set(UiFeed));
    }
}

fn on_inspect_items(
    In(ev): In<SessionEvent>,
    mut worn: ResMut<InspectWorn>,
    target: Res<InspectGuid>,
) {
    let SessionEvent::InspectItems { guid, items } = ev else {
        return;
    };
    if guid != target.get() {
        debug!("inspect: items for {guid:#x}, not the inspected player — skipped");
        return;
    }
    debug!("inspect: {} worn items for {guid:#x}", items.len());
    *worn = InspectWorn { guid, items };
}

/// `SMSG_INSPECT_TALENT`'s talents, for the inspected player alone (`0x6ce0da`).
fn on_inspect_talents(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::InspectTalents {
        guid,
        talents: info,
    } = ev
    else {
        return;
    };
    if world.get_resource::<InspectGuid>().map(InspectGuid::get) == Some(guid) {
        crate::ui_talent::inspect_talents(world, info);
    }
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
/// always-1 and durability, flags, locks and creator stay inert. Only the permanent and temporary
/// enchants broadcast, the two `u16` halves of `PLAYER_VISIBLE_ITEM_<slot>_ENCHANTMENT`.
fn inspect_slot_view(
    store: &ObjectStore,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    slot0: u8,
) -> Option<InvSlotView> {
    let entry = store.0.player_visible_item_entry(slot0)?;
    // 12340's visible-item slot is the entry and two enchants; no random-property roll streams, so
    // the name carries no suffix.
    let roll = 0;
    // By entry alone (guid 0), as the reference's item cache asks: there is no item object.
    let (name, quality, display) = match items.template(entry, 0, commands) {
        Some(t) => (
            Some(rolls.name(&t.name, roll)),
            t.quality,
            t.display_info_id,
        ),
        None => (None, 0, 0),
    };
    let link = name.as_ref().map(|n| {
        let fields = benilla_ui::script::ItemLinkFields {
            enchant: store.0.player_visible_item_enchant(slot0, 0).unwrap_or(0) as i32,
            random_property: roll as i32,
            ..Default::default()
        };
        crate::ui_items::item_link_full(entry, &fields, n, quality)
    });
    Some(InvSlotView {
        item_id: entry,
        icon: crate::ui_items::item_icon(icons, display),
        count: 1,
        quality: quality as i32,
        name,
        link,
        // The reference reads an item object's `ITEM_FIELD_FLAGS`, which inspected gear lacks, so
        // the tooltip prints the template's bind line, as the reference's does.
        already_bound: false,
        // The two broadcast enchants, with no charges and no countdown: there is no item object.
        enchants: crate::items::enchant_lines(
            (0..2).map(|j| {
                let id = store.0.player_visible_item_enchant(slot0, j).unwrap_or(0);
                (j, id as i32, 0, None)
            }),
            rolls.enchants,
        ),
        ..Default::default()
    })
}

fn feed_inspect(
    mut script: crate::ui_script::InTurn,
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
    let Some(mut script) = script.turn() else {
        return;
    };
    // A `/reload` keeps the latched token, and the fresh memo re-pushes the view.
    let memo = feed.vm.get(&script);

    // The tokens `NotifyInspect` sent for, and where `ClearInspectPlayer` ran among them: the last token latches unless a clear followed it, the booth follows.
    let notifies = script.take_inspect_notifies();
    let clear_at = script.take_inspect_clear();
    let notified = notifies.len();
    if let Some((token, guid)) = notifies.into_iter().last() {
        debug!("inspect: {token:?} -> {guid:#x}");
        target.token = Some(token);
    }
    if clear_at.is_some_and(|at| at >= notified) {
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

#[cfg(test)]
mod leave_tests {
    use super::*;

    /// `0x5eafeb`: the leave clears a held target and its talent tabs; with none it does nothing.
    #[test]
    fn the_world_leave_clears_the_inspect_target() {
        use benilla_ui::script::{TalentPages, TalentSide, TalentTabView};
        let tabs = || TalentPages {
            tabs: vec![TalentTabView::default()],
            talents: vec![Vec::new()],
        };
        let mut world = World::new();
        world.init_resource::<InspectGuid>();
        world.init_resource::<crate::ui_talent::TalentState>();
        let held = |world: &World| {
            world
                .resource::<crate::ui_talent::TalentState>()
                .0
                .pages(TalentSide::Inspect)
                .tabs
                .len()
        };
        world
            .resource_mut::<crate::ui_talent::TalentState>()
            .0
            .set_inspect_target(0x56, Some(tabs()));
        world.resource_mut::<InspectGuid>().set(0x56);
        leave_world(&mut world);
        assert_eq!(world.resource::<InspectGuid>().get(), 0);
        assert_eq!(held(&world), 0, "the talent target reset with no player");
        world
            .resource_mut::<crate::ui_talent::TalentState>()
            .0
            .set_inspect_target(0x56, Some(tabs()));
        leave_world(&mut world);
        assert_eq!(held(&world), 1, "no target held: nothing to clear");
    }

    /// The talent target resets inside the verbs: `NotifyInspect` on a move (`0x5e76f2`), `ClearInspectPlayer` on a held target (`0x5e78d5`); a guid that is no held player leaves no tabs (`0x5c9f8f`).
    #[test]
    fn the_inspect_verbs_reset_the_talent_target_at_the_call() {
        use benilla_ui::script::{LiveState, TalentPages, TalentSide, TalentTabView};
        let mut world = World::new();
        world.init_resource::<InspectGuid>();
        world.init_resource::<crate::ui_talent::TalentState>();
        let tabs = |world: &World| {
            world
                .resource::<crate::ui_talent::TalentState>()
                .0
                .pages(TalentSide::Inspect)
                .tabs
                .len()
        };
        let seed = |world: &mut World| {
            world
                .resource_mut::<crate::ui_talent::TalentState>()
                .0
                .set_inspect_target(
                    0x55,
                    Some(TalentPages {
                        tabs: vec![TalentTabView::default()],
                        talents: vec![Vec::new()],
                    }),
                );
        };
        let view = |world: &mut World, f: &dyn Fn(&mut dyn LiveState) -> bool| {
            crate::ui_script::live::with_world_view(world, |l| f(l))
        };
        seed(&mut world);
        assert!(view(&mut world, &|l| l.notify_inspect(0x56)));
        assert_eq!(
            tabs(&world),
            0,
            "a new target that is no held player: no tabs"
        );
        seed(&mut world);
        assert!(!view(&mut world, &|l| l.notify_inspect(0x56)));
        assert_eq!(tabs(&world), 1, "the same target keeps them");
        view(&mut world, &|l| {
            l.clear_inspect_target();
            true
        });
        assert_eq!(tabs(&world), 0, "a held target's clear resets them");
        seed(&mut world);
        view(&mut world, &|l| {
            l.clear_inspect_target();
            true
        });
        assert_eq!(tabs(&world), 1, "no target held: nothing to reset");
    }
}
