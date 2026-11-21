//! The item lock, 12340's `item+0x394` bit 0, keyed on the item as 12340's is: a send locks its items (`0x706fe0`), and a slot's watch, the stack-count watch, the post-init, a refusal or a loot close unlocks one (`0x707020`).

use bevy::prelude::Resource;

/// The locked items by guid; the bag verbs read an item's lock at the call.
#[derive(Debug, Default, Resource)]
pub(crate) struct PendingItemOps {
    locked: Vec<u64>,
    /// Steps on every change, so a feed notices a lock clearing the frame the set empties.
    epoch: u64,
}

impl PendingItemOps {
    /// The lock (`0x706fe0`): `true` when the item was unlocked, the edge its events fire on; a zero guid locks nothing.
    pub(crate) fn lock(&mut self, guid: u64) -> bool {
        if guid == 0 || self.locked.contains(&guid) {
            return false;
        }
        self.locked.push(guid);
        self.epoch += 1;
        true
    }

    /// The unlock (`0x707020`): `true` when the item was locked, the edge its events fire on.
    pub(crate) fn unlock(&mut self, guid: u64) -> bool {
        let Some(at) = self.locked.iter().position(|&g| g == guid) else {
            return false;
        };
        self.locked.swap_remove(at);
        self.epoch += 1;
        true
    }

    /// Whether `guid` is locked (`item+0x394` bit 0).
    pub(crate) fn is_locked(&self, guid: u64) -> bool {
        guid != 0 && self.locked.contains(&guid)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.locked.is_empty()
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The item's destroy: its lock goes with the object, no event.
    pub(crate) fn forget(&mut self, guid: u64) {
        if let Some(at) = self.locked.iter().position(|&g| g == guid) {
            self.locked.swap_remove(at);
            self.epoch += 1;
        }
    }

    /// The session end: the item objects, and their locks, do not outlive it.
    pub(crate) fn clear_session(&mut self) {
        self.locked.clear();
        self.epoch += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::PendingItemOps;

    #[test]
    fn a_lock_and_an_unlock_each_fire_only_on_their_edge() {
        let mut p = PendingItemOps::default();
        assert!(!p.lock(0), "no item, no lock");
        assert!(p.lock(100));
        assert!(!p.lock(100), "already locked: no second event");
        assert!(p.is_locked(100) && !p.is_locked(200));
        assert!(!p.unlock(200), "an unlocked item fires nothing");
        let epoch = p.epoch();
        assert!(p.unlock(100));
        assert!(p.epoch() > epoch, "the feed's gate sees the set move");
        assert!(!p.unlock(100) && p.is_empty());
    }

    #[test]
    fn a_destroy_drops_the_lock_and_the_session_end_drops_every_lock() {
        let mut p = PendingItemOps::default();
        p.lock(100);
        p.lock(200);
        p.forget(100);
        assert!(!p.is_locked(100) && p.is_locked(200));
        p.clear_session();
        assert!(p.is_empty());
    }
}
