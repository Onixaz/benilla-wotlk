//! The UI sprite caches, released by use. The reference frees a texture once no region shows it:
//! `CSimpleTexture::SetTexture` closes the region's old handle, and at a zero refcount
//! `HandleClose` deletes the `CTexture`, whose GPU texture goes back to the allocation pool
//! (`~CTexture`, `TextureFreeGxTex`); showing it again decodes the file again.

use std::collections::HashMap;
use std::hash::Hash;

use bevy::image::Image;
use bevy::prelude::*;

/// Sprites by key, each with a used-since-the-last-sweep flag. Misses are cached as `None` and
/// never released, so a bad path never re-walks the chain per frame.
pub(super) struct SpriteCache<K> {
    map: HashMap<K, (Option<Handle<Image>>, bool)>,
}

impl<K> Default for SpriteCache<K> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash> SpriteCache<K> {
    /// A lookup, which counts as a use: `None` when the key was never asked for.
    pub(super) fn get(&mut self, key: &K) -> Option<Option<Handle<Image>>> {
        let (handle, used) = self.map.get_mut(key)?;
        *used = true;
        Some(handle.clone())
    }

    pub(super) fn insert(&mut self, key: K, handle: Option<Handle<Image>>) {
        self.map.insert(key, (handle, true));
    }

    /// Drop the cached misses, so a path asked before it existed gets a second look.
    pub(super) fn forget_misses(&mut self) {
        self.map.retain(|_, (handle, _)| handle.is_some());
    }

    /// Drop each sprite no lookup asked for since the last call and nothing else holds: the UI
    /// asks for every quad's sprite each frame, and a material or a held handle keeps it shown.
    pub(super) fn release_unused(&mut self) -> usize {
        let before = self.map.len();
        self.map.retain(|_, (handle, used)| match handle {
            None => true,
            Some(handle) => std::mem::take(used) || !only_held_here(handle),
        });
        before - self.map.len()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.map.len()
    }
}

/// Whether this cache's handle is the asset's last strong one.
fn only_held_here(handle: &Handle<Image>) -> bool {
    matches!(handle, Handle::Strong(arc) if std::sync::Arc::strong_count(arc) == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(images: &mut Assets<Image>) -> Handle<Image> {
        images.add(Image::default())
    }

    #[test]
    fn a_sprite_unasked_for_a_whole_sweep_and_held_nowhere_else_is_released() {
        let mut images = Assets::<Image>::default();
        let mut cache = SpriteCache::default();
        cache.insert("a", Some(image(&mut images)));
        // Inserted counts as used: the first sweep keeps it, the next one with no lookup drops it.
        assert_eq!(cache.release_unused(), 0);
        assert_eq!(cache.release_unused(), 1);
        assert_eq!(cache.get(&"a"), None);
    }

    #[test]
    fn a_lookup_between_sweeps_keeps_a_sprite() {
        let mut images = Assets::<Image>::default();
        let mut cache = SpriteCache::default();
        cache.insert("a", Some(image(&mut images)));
        for _ in 0..3 {
            assert!(cache.get(&"a").is_some_and(|h| h.is_some()));
            assert_eq!(cache.release_unused(), 0);
        }
    }

    #[test]
    fn a_sprite_something_else_holds_stays_until_it_lets_go() {
        let mut images = Assets::<Image>::default();
        let mut cache = SpriteCache::default();
        let held = image(&mut images);
        cache.insert("a", Some(held.clone()));
        cache.release_unused();
        assert_eq!(cache.release_unused(), 0, "a material still samples it");
        drop(held);
        assert_eq!(cache.release_unused(), 1);
    }

    #[test]
    fn a_cached_miss_is_never_released() {
        let mut cache = SpriteCache::<&str>::default();
        cache.insert("bad", None);
        cache.release_unused();
        assert_eq!(cache.release_unused(), 0);
        assert_eq!(cache.get(&"bad"), Some(None));
        cache.forget_misses();
        assert_eq!(cache.len(), 0);
    }
}
