//! A main-thread fan-out that never runs an unrelated task. Bevy's scope (behind `par_iter` and
//! `par_chunk_map_mut`) ticks the compute pool's shared executor while it waits, so a main-thread
//! system can pick up a render-world system queued on the same pool and return late (Dalaran,
//! M1: ~0.6 ms a frame of render work run inside main-thread systems). Here the caller runs its
//! own share and leaves the pool's queue to the pool.

use bevy::tasks::ComputeTaskPool;

/// Run `f` over `items` in chunks of `chunk`, the first on the calling thread and the rest on
/// the compute pool, without ticking the pool's shared executor.
pub fn for_each_chunk_mut<T: Send>(items: &mut [T], chunk: usize, f: impl Fn(&mut [T]) + Sync) {
    let chunk = chunk.max(1);
    if items.len() <= chunk {
        f(items);
        return;
    }
    let f = &f;
    ComputeTaskPool::get().scope_with_executor(false, None, |scope| {
        let mut chunks = items.chunks_mut(chunk);
        let own = chunks.next();
        for rest in chunks {
            scope.spawn(async move { f(rest) });
        }
        if let Some(own) = own {
            scope.spawn_on_scope(async move { f(own) });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_is_visited_once() {
        ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        for (n, chunk) in [(0, 4), (3, 4), (4, 4), (17, 4), (100, 1)] {
            let mut items = vec![0u32; n];
            for_each_chunk_mut(&mut items, chunk, |c| c.iter_mut().for_each(|v| *v += 1));
            assert!(items.iter().all(|&v| v == 1), "n={n} chunk={chunk}");
        }
    }
}
