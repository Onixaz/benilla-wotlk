//! The item packet handlers: `SMSG_INVENTORY_CHANGE_FAILURE` onto the error line, the pending
//! item locks and the loot latch, and `SMSG_OPEN_CONTAINER` onto the `BAG_OPEN` queue.

use benilla_protocol::messages::{InventoryFailure, InventoryFailureTail};
use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{EquipError, EquipErrors, BAG_SLOT_FIRST, BANK_BAG_ID_FIRST};
use crate::net::{NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::pending_item_ops::{LockTransitions, PendingItemOps};
use crate::ui_loot::LootLatch;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::InventoryFailure, on_inventory_failure)
        .net_handler(SessionEventKind::OpenContainer, on_open_container)
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

/// The item locks and the queued unlocks die with the session: the reference's lock is item-object
/// state (`item+0x314`), and the objects are rebuilt at every world entry.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut pending: ResMut<PendingItemOps>,
    mut lock_cleared: ResMut<LockTransitions>,
) {
    pending.clear_session();
    lock_cleared.0.clear();
}

/// Container ids from `SMSG_OPEN_CONTAINER`, for [`super::feed::feed_containers`] to fire as
/// `BAG_OPEN(id)`: this handler has no `UiScript`.
#[derive(Resource, Default)]
pub(crate) struct BagOpens(pub(crate) Vec<i64>);

fn on_open_container(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut opens: ResMut<BagOpens>,
) {
    if let SessionEvent::OpenContainer { item } = ev {
        let store = self_q.single().ok().map(|s| &s.0);
        match container_of(self_guid.0, store, item) {
            Some(id) => opens.0.push(id),
            None => debug!("open container {item:#x}: not one of our bags — no BAG_OPEN"),
        }
    }
}

/// The reference's `BAG_OPEN` raiser: our own guid is the backpack (0), an equipped bag (slots
/// 19..22) is 1..4, a bank bag 5..10, and any other guid fires nothing. vmangos sends the packet
/// when an auto-equip puts a bag in a bag slot.
pub(super) fn container_of(
    self_guid: Option<u64>,
    store: Option<&ObjectFields>,
    item: u64,
) -> Option<i64> {
    if self_guid.is_some_and(|me| me == item) {
        return Some(0);
    }
    let store = store?;
    (0..4u8)
        .find(|i| store.player_inv_slot(BAG_SLOT_FIRST + i) == Some(item))
        .map(|i| i64::from(i) + 1)
        .or_else(|| {
            (0..super::BANK_BAGS)
                .find(|j| store.player_bank_bag_slot(*j) == Some(item))
                .map(|j| BANK_BAG_ID_FIRST + i64::from(j))
        })
}

fn on_inventory_failure(
    In(ev): In<SessionEvent>,
    mut equip_errors: ResMut<EquipErrors>,
    mut pending: ResMut<PendingItemOps>,
    mut lock_cleared: ResMut<LockTransitions>,
    mut latch: ResMut<LootLatch>,
    mut loot: ResMut<crate::ui_loot::LootState>,
    net: Res<crate::net::NetCommands>,
    (limits, mut errors): (
        Option<Res<crate::ui_trade::ItemLimitCategories>>,
        ResMut<crate::ui_action::UiErrorKeys>,
    ),
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
        inventory_failure(f, &mut equip_errors, &mut pending, &mut lock_cleared);
        loot.inventory_failed(f.item, &mut latch, &net);
    }
}

/// A refused inventory operation (reason 0 never arrives: `benilla_protocol::events` drops it):
/// queues the error line and clears both items' pending locks (`0x6e3259`, `0x6e326f`), the
/// unlocked slots waiting in [`LockTransitions`] for [`super::feed::feed_containers`], since this
/// site has no `UiScript`.
///
/// Not built: the item's client-side spell cancel (`0x6e3249`), as no spell is kept by its casting
/// item, and reason `0x51`'s bind confirm, which needs the named item held (`0x6e1a70`); AzerothCore
/// sends that item as 0.
fn inventory_failure(
    f: InventoryFailure,
    equip_errors: &mut EquipErrors,
    pending: &mut PendingItemOps,
    lock_cleared: &mut LockTransitions,
) {
    debug!(
        "net: inventory failure {:#04x} (items {:#x} {:#x}, bag {}, {:?})",
        f.reason, f.item, f.item2, f.bag_slot, f.tail
    );
    let required_level = match f.tail {
        InventoryFailureTail::RequiredLevel(level) => Some(level),
        // `0x6e3192` and [`limit_line`] replace the plain line.
        InventoryFailureTail::BindConfirm { .. } | InventoryFailureTail::LimitCategory(_) => {
            return unlock(f, pending, lock_cleared);
        }
        InventoryFailureTail::None => None,
    };
    equip_errors.0.push(EquipError {
        reason: u32::from(f.reason),
        required_level,
        bag_slot: f.bag_slot,
    });
    unlock(f, pending, lock_cleared);
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

fn unlock(f: InventoryFailure, pending: &mut PendingItemOps, lock_cleared: &mut LockTransitions) {
    lock_cleared.0.extend(pending.clear_by_failure(f.item));
    lock_cleared.0.extend(pending.clear_by_guid(f.item2));
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

    /// Both items unlock (`0x6e3259`, `0x6e326f`), and the level fills the plain line.
    #[test]
    fn a_failure_unlocks_both_items_and_keeps_the_level() {
        let mut errors = EquipErrors::default();
        let mut pending = PendingItemOps::default();
        let mut cleared = LockTransitions::default();
        pending.add([
            (0, 3, 0x4000_0000_0000_0007, 1),
            (0, 4, 0x4000_0000_0000_0008, 1),
        ]);
        inventory_failure(
            failure(0x57, InventoryFailureTail::RequiredLevel(40)),
            &mut errors,
            &mut pending,
            &mut cleared,
        );
        assert!(pending.is_empty());
        assert_eq!(cleared.0.len(), 2);
        assert_eq!(errors.0[0].required_level, Some(40));
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
        let mut errors = EquipErrors::default();
        inventory_failure(
            f,
            &mut errors,
            &mut PendingItemOps::default(),
            &mut LockTransitions::default(),
        );
        assert!(errors.0.is_empty());
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
        app.init_resource::<EquipErrors>()
            .init_resource::<PendingItemOps>()
            .init_resource::<LockTransitions>()
            .init_resource::<BagOpens>()
            .init_resource::<LootLatch>()
            .init_resource::<crate::ui_loot::LootState>()
            .init_resource::<SelfGuid>();
        register(&mut app);
        app.world_mut()
            .resource_mut::<PendingItemOps>()
            .add([(0, 3, 0x4000_0000_0000_0007, 1)]);
        app.world_mut()
            .resource_mut::<LockTransitions>()
            .0
            .push((0, 5));
        let epoch = app.world().resource::<PendingItemOps>().epoch();

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let pending = app.world().resource::<PendingItemOps>();
        assert!(pending.is_empty() && !pending.contains(0, 3));
        assert_ne!(pending.epoch(), epoch, "the feed's gate sees the set move");
        assert!(
            app.world().resource::<LockTransitions>().0.is_empty(),
            "no ITEM_LOCK_CHANGED for the old session's slots"
        );
    }
}

#[cfg(test)]
mod open_container_tests {
    use super::*;
    use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;

    /// `PLAYER_FIELD_BANK_BAG_SLOT_1`, field 1572 (private to the fields module).
    const BANK_BAG_SLOT_1: u16 = 1572;

    /// A self descriptor holding the given guids at the given first-dword indices.
    fn store(guids: &[(u16, u64)]) -> ObjectFields {
        let pairs: Vec<(u16, u32)> = guids
            .iter()
            .flat_map(|(i, g)| [(*i, *g as u32), (*i + 1, (*g >> 32) as u32)])
            .collect();
        ObjectFields::from_pairs(&pairs)
    }

    #[test]
    fn the_bag_open_raiser_resolves_the_reference_container_ids() {
        let me = 0x0000_0000_0000_0042;
        let bag_in_slot_20 = 0x4000_0000_0000_0b02; // the second equipped bag → container 2
        let bank_bag_3 = 0x4000_0000_0000_0b44; // the fourth bank bag → container 5 + 3
        let s = store(&[
            (FIELD_PLAYER_INV_SLOT_HEAD + 2 * 20, bag_in_slot_20),
            (BANK_BAG_SLOT_1 + 2 * 3, bank_bag_3),
        ]);
        assert_eq!(
            container_of(Some(me), Some(&s), me),
            Some(0),
            "our own guid is the backpack"
        );
        assert_eq!(container_of(Some(me), Some(&s), bag_in_slot_20), Some(2));
        assert_eq!(container_of(Some(me), Some(&s), bank_bag_3), Some(8));
        assert_eq!(
            container_of(Some(me), Some(&s), 0xdead),
            None,
            "a stranger's guid fires nothing"
        );
        assert_eq!(
            container_of(None, None, me),
            None,
            "no self, no store: nothing to match"
        );
    }
}
