# benilla's `bevy_ecs` fork — what differs from upstream, and how to check

Upstream: [`bevyengine/bevy`](https://github.com/bevyengine/bevy), crate `bevy_ecs`, MIT OR
Apache-2.0, version `0.18.1` — the version the workspace lock resolves to. Wired in through
`[patch.crates-io]` in the workspace root, the way `bevy_render` and `wgpu-hal` are. The crate is
copied from the cargo registry as published (`src/`, `examples/`, `Cargo.toml`, `README.md` and the
two licence files); the registry's `Cargo.toml.orig`, `Cargo.lock`, `clippy.toml` and cargo metadata
files are left out.

## Why a fork exists at all

The frame slowed as a session travelled (#741, #540). A streamed unit gains its components one
insert at a time, and some are set and cleared at run time, so a session mints an archetype for
every set of components a unit ever passes through: about 700 at a fresh login on an empty map,
about 4,700 after fourteen teleports through the cities, of which about 100 hold an entity. Bevy
never frees an archetype.

A query that names a sparse-set component iterates archetypes, not tables, and upstream steps
through every archetype the query matches to ask each whether it is empty. That step costs about
4 ns, and about seventy of benilla's per-unit systems take it for every matched archetype every
frame: `net::apply::tag_self_player`, one such query and nothing else, went from 6 µs to 32 µs a
frame over a tour with the same thirty units in view. Bevy's `main` branch walks the same way.

## Patch — `src/archetype.rs`

`Archetypes` gains `occupancy`, a set with one bit per archetype, on while the archetype holds an
entity. Each `Archetype` keeps a pointer to its own bit (`OccupiedBit`) and writes it where its
entity list goes from empty to not and back: `allocate`, `swap_remove` and `clear_entities`. The
set is chunked and its chunks are boxed and never freed or moved, so the pointers stay good while
`Archetypes` grows. `Archetypes::occupied_among(matched)` iterates the archetypes that are both in
a caller's bitset and occupied, a word at a time, ascending.

## Patch — `src/query/iter.rs`, `src/query/state.rs`, `src/query/par_iter.rs`

A non-dense query takes its archetypes from `occupied_among(QueryState::matched_archetypes)`
where it walked `matched_storage_ids`: in the cursor's `next`, in `Iterator::fold` (the `for_each`
path), in `max_remaining` (the size hint), in the parallel batcher of
`par_fold_init_unchecked_manual`, and in `QueryParIter::get_batch_size`. The order is the one
upstream visits in, ascending archetype id, and an empty archetype yielded nothing before, so the
items and their order are unchanged. Dense queries, which walk tables, are untouched.

## How to check

`diff -ru <registry>/bevy_ecs-0.18.1/src third_party/bevy_ecs/src` shows the patch in those four
files and nothing else; every other file is byte-identical to the registry crate. The crate's own
`cargo test --lib` gives the same result patched and as published (805 pass; three error-reporting
tests fail in both outside a workspace). `benilla-world`'s `occupied_archetype_walk` test drives a
sparse query through the iterator, the fold path, the size hint and the parallel walk while
archetypes empty, refill and clear. The gain shows in a per-system timing of `tag_self_player`
before and after a teleport tour: flat with the patch, five times its fresh cost without.
