//! The item packet handlers: `SMSG_INVENTORY_CHANGE_FAILURE` onto the error line, the pending
//! item locks and the loot latch, and `SMSG_OPEN_CONTAINER`'s `BAG_OPEN`, each in its handler's turn.

use benilla_protocol::messages::{InventoryFailure, InventoryFailureTail};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::EquipError;
use crate::net::{NetHandlerApp, ObjectStore, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;
use crate::ui_loot::LootLatch;
use crate::ui_script::live::UiEvents;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::InventoryFailure, on_inventory_failure)
        .net_handler(SessionEventKind::OpenContainer, on_open_container)
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

/// The item locks die with the session: the reference's lock is item-object state (`item+0x314`), and the objects are rebuilt at every world entry.
fn on_session_end(In(_): In<SessionEvent>, mut pending: ResMut<PendingItemOps>) {
    pending.clear_session();
}

/// `SMSG_OPEN_CONTAINER` (`0x6e32d0`): `BAG_OPEN` in the handler's turn.
fn on_open_container(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    if let SessionEvent::OpenContainer { item } = ev {
        ui.world(move |world| super::watch::open_container(world, item));
    }
}

fn on_inventory_failure(
    In(ev): In<SessionEvent>,
    mut latch: ResMut<LootLatch>,
    mut loot: ResMut<crate::ui_loot::LootState>,
    net: Res<crate::net::NetCommands>,
    (limits, mut errors): (
        Option<Res<crate::ui_trade::ItemLimitCategories>>,
        ResMut<crate::ui_action::UiErrorKeys>,
    ),
    mut ui: ResMut<UiEvents>,
) {
    if let SessionEvent::InventoryFailure(f) = ev {
        let limit = |id| {
            limits
                .as_ref()
                .and_then(|l| l.0.get(id))
                .map(|r| (r.quantity, r.name.clone()))
        };
        if let Some(line) = limit_line(f, limit) {
            errors.0.push(line);
        }
        inventory_failure(f, &mut ui);
        loot.inventory_failed(f.item, &mut latch, &net);
    }
}

/// A refused inventory operation (reason 0 never arrives: `benilla_protocol::events` drops it): its line (`0x6e3204`), then both items' unlocks (`0x6e3259`, `0x6e326f`), each in the handler's turn.
///
/// Not built: the item's client-side spell cancel (`0x6e3249`), as no spell is kept by its casting
/// item, and reason `0x51`'s bind confirm, which needs the named item held (`0x6e1a70`); AzerothCore
/// sends that item as 0.
fn inventory_failure(f: InventoryFailure, ui: &mut UiEvents) {
    debug!(
        "net: inventory failure {:#04x} (items {:#x} {:#x}, bag {}, {:?})",
        f.reason, f.item, f.item2, f.bag_slot, f.tail
    );
    let required_level = match f.tail {
        InventoryFailureTail::RequiredLevel(level) => Some(level),
        // `0x6e3192` and [`limit_line`] replace the plain line.
        InventoryFailureTail::BindConfirm { .. } | InventoryFailureTail::LimitCategory(_) => None,
        InventoryFailureTail::None => None,
    };
    if matches!(
        f.tail,
        InventoryFailureTail::None | InventoryFailureTail::RequiredLevel(_)
    ) {
        let e = EquipError {
            reason: u32::from(f.reason),
            required_level,
            bag_slot: f.bag_slot,
        };
        ui.world(move |world| show_equip_error(world, e));
    }
    ui.world(move |world| unlock(world, f));
}

/// An equip refusal's line, in turn with the events around it: reason 16 names the target bag's family when it resolves, reasons 1 and `0x57` fill the level; an empty or missing string shows nothing.
pub(crate) fn show_equip_error(world: &mut World, e: EquipError) {
    let family = (e.reason == 16)
        .then(|| {
            world
                .run_system_cached_with(
                    |In(slot): In<u8>,
                     me: Query<&ObjectStore, With<SelfPlayer>>,
                     objects: crate::net::Objects,
                     items: Res<crate::items::Items>,
                     families: Option<Res<super::ItemBagFamilies>>,
                     commands: Res<crate::net::NetCommands>| {
                        super::feed::bag_family_name(
                            me.iter().next(),
                            slot,
                            &objects,
                            &items,
                            families.as_deref().map(|c| &c.0),
                            &commands,
                        )
                    },
                    e.bag_slot,
                )
                .ok()
                .flatten()
        })
        .flatten();
    let key = match family {
        Some(_) => "ERR_WRONG_BAG_TYPE_SUBCLASS",
        None => super::equip_error::equip_error_key(e.reason),
    };
    let mut args = Vec::new();
    if let Some(d) = e.required_level {
        args.push(crate::ui_action::FillArg::D(i64::from(d)));
    }
    if let Some(family) = family {
        args.push(crate::ui_action::FillArg::S(family));
    }
    let Some(mut ui) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    crate::ui_action::show_in_turn(
        &mut ui,
        "ui_items",
        crate::ui_action::UiError::args(key, args),
    );
}

/// Both items' unlocks (`0x6e3259`, `0x6e326f`), each a locked item's lock edge at its place.
fn unlock(world: &mut World, f: InventoryFailure) {
    super::watch::unlock_item(world, f.item);
    super::watch::unlock_item(world, f.item2);
}

/// A limit-category refusal's line: the row's quantity then its name (`0x6e31c1`-`0x6e31cd`). A
/// row the table lacks shows nothing, where the reference stops on a fatal error (`0x6e31e3`).
fn limit_line(
    f: InventoryFailure,
    limit: impl FnOnce(u32) -> Option<(u32, String)>,
) -> Option<crate::ui_action::UiError> {
    let InventoryFailureTail::LimitCategory(id) = f.tail else {
        return None;
    };
    let key = super::equip_error::inventory_error_key(u32::from(f.reason))?;
    let (quantity, name) = limit(id)?;
    Some(crate::ui_action::UiError::args(
        key,
        vec![
            crate::ui_action::FillArg::D(i64::from(quantity)),
            crate::ui_action::FillArg::S(name),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(reason: u8, tail: InventoryFailureTail) -> InventoryFailure {
        InventoryFailure {
            reason,
            item: 0x4000_0000_0000_0007,
            item2: 0x4000_0000_0000_0008,
            bag_slot: 255,
            tail,
        }
    }

    /// The line first (`0x6e3204`), then both items unlock (`0x6e3259`, `0x6e326f`), each slot's `ITEM_LOCK_CHANGED` in the handler's turn.
    #[test]
    fn a_failure_shows_its_line_then_unlocks_both_items() {
        let mut ui = UiEvents::default();
        inventory_failure(
            failure(0x57, InventoryFailureTail::RequiredLevel(40)),
            &mut ui,
        );
        assert_eq!(
            ui.names(),
            ["<world>", "<world>"],
            "the line, then the unlock"
        );
        let mut world = World::new();
        world.init_resource::<UiEvents>();
        world.init_resource::<PendingItemOps>();
        world
            .resource_mut::<PendingItemOps>()
            .lock(0x4000_0000_0000_0007);
        world
            .resource_mut::<PendingItemOps>()
            .lock(0x4000_0000_0000_0008);
        world
            .resource_mut::<PendingItemOps>()
            .lock(0x4000_0000_0000_0009);
        unlock(
            &mut world,
            failure(0x57, InventoryFailureTail::RequiredLevel(40)),
        );
        let locks = world.resource::<PendingItemOps>();
        assert!(
            !locks.is_locked(0x4000_0000_0000_0007) && !locks.is_locked(0x4000_0000_0000_0008),
            "both named items unlock"
        );
        assert!(
            locks.is_locked(0x4000_0000_0000_0009),
            "an item the packet does not name stays locked"
        );
    }

    /// `0x6e31cd`: the limit lines take the row's quantity then its name, and no plain line.
    #[test]
    fn a_limit_category_refusal_names_the_row() {
        let f = failure(84, InventoryFailureTail::LimitCategory(12));
        let line = limit_line(f, |id| (id == 12).then(|| (2, "Healthstone".into()))).unwrap();
        assert_eq!(line.key, "ERR_ITEM_MAX_LIMIT_CATEGORY_COUNT_EXCEEDED_IS");
        assert_eq!(
            line.args,
            vec![
                crate::ui_action::FillArg::D(2),
                crate::ui_action::FillArg::S("Healthstone".into())
            ]
        );
        assert!(limit_line(f, |_| None).is_none(), "no row, no line");
        let mut ui = UiEvents::default();
        inventory_failure(f, &mut ui);
        assert_eq!(ui.names(), ["<world>"], "the unlock alone");
    }

    /// `0x6e321c`-`0x6e3232`: a failure on the latched item takes `CloseLoot(1, 0, 0)`.
    #[test]
    fn an_inventory_failure_on_the_looted_object_closes_its_loot() {
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = crate::net::NetCommands(tx);
        let mut loot = crate::ui_loot::LootState::default();
        loot.open(LOCKBOX, 1, 5, Vec::new());
        let mut latch = LootLatch(Some(LOCKBOX));

        // An unrelated item's failure must not end the session.
        loot.inventory_failed(0x1234, &mut latch, &net);
        assert_eq!(latch.0, Some(LOCKBOX));
        assert_eq!(loot.source(), Some(LOCKBOX));
        assert!(rx.try_recv().is_err());

        loot.inventory_failed(LOCKBOX, &mut latch, &net);
        assert_eq!(latch.0, None);
        assert_eq!(loot.source(), None);
        assert!(matches!(
            rx.try_recv(),
            Ok(crate::net::ClientCommand::LootRelease { guid }) if guid == LOCKBOX
        ));
        assert_eq!(loot.take_unlocks(), [LOCKBOX], "the close unlocks its item");
    }
    /// Driven through the real registration.
    #[test]
    fn the_session_end_drops_every_item_lock() {
        let mut app = App::new();
        app.init_resource::<PendingItemOps>()
            .init_resource::<UiEvents>()
            .init_resource::<LootLatch>()
            .init_resource::<crate::ui_loot::LootState>();
        register(&mut app);
        app.world_mut()
            .resource_mut::<PendingItemOps>()
            .lock(0x4000_0000_0000_0007);
        let epoch = app.world().resource::<PendingItemOps>().epoch();

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let pending = app.world().resource::<PendingItemOps>();
        assert!(pending.is_empty() && !pending.is_locked(0x4000_0000_0000_0007));
        assert_ne!(pending.epoch(), epoch);
    }
}

#[cfg(test)]
mod open_container_tests {
    use super::super::watch::{open_container, WatchedBags};
    use super::*;

    /// `0x5d7110`: our own guid is the backpack, a watched bag its id, a stranger nothing.
    #[test]
    fn the_bag_open_raiser_resolves_the_reference_container_ids() {
        let me = 0x0000_0000_0000_0042;
        let bag_2 = 0x4000_0000_0000_0b02;
        let bank_bag_3 = 0x4000_0000_0000_0b44;
        let mut world = World::new();
        world.init_resource::<UiEvents>();
        world.insert_resource(crate::net::SelfGuid(Some(me)));
        let mut watched = WatchedBags::default();
        watched.0[1] = bag_2;
        watched.0[7] = bank_bag_3;
        world.insert_resource(watched);
        for (item, fired) in [(me, 1), (bag_2, 1), (bank_bag_3, 1), (0xdead, 0)] {
            open_container(&mut world, item);
            assert_eq!(
                world.resource::<UiEvents>().names().len(),
                fired,
                "{item:#x}"
            );
            *world.resource_mut::<UiEvents>() = UiEvents::default();
        }
    }
}
