# benilla's `wgpu-hal` fork — what differs from upstream, and how to check

Upstream: [`gfx-rs/wgpu`](https://github.com/gfx-rs/wgpu), crate `wgpu-hal`, MIT OR Apache-2.0,
version `27.0.4` — the version the workspace lock resolves to. Wired in through
`[patch.crates-io]` in the workspace root, the way `bevy_render` is. The crate is copied from the
cargo registry as published (`src/`, `examples/`, `build.rs`, `Cargo.toml`, `README.md` and the two
licence files); the registry's `Cargo.toml.orig`, `Cargo.lock` and cargo metadata files are left
out.

## Why a fork exists at all

`wgpu-core` stages every `Queue::write_buffer` and `write_texture` through a fresh
`MAP_WRITE | COPY_SRC` buffer flagged `MemoryFlags::TRANSIENT`, and destroys it once the
submission that read it has completed. Vulkan and DX12 suballocate those from a heap; Metal makes
each one a whole `MTLBuffer`, created under the device's one mutex and released through IOKit. The
render world's prepare systems write in parallel, so they queue on that mutex, and the thread that
submits pays every release. At Dalaran (map 571, 5842.4 595.6 650.7, ~505 units streamed) on an
M1, a `sample` of the client put ~1,570 samples in `StagingBuffer::new` across the compute pool
and ~320 in the render thread's `TempResource` drops over 15 s; with the patch, 21 and 4.

## Patch — `src/metal/staging_pool.rs`, `src/metal/device.rs`, `src/metal/mod.rs`, `src/metal/adapter.rs`

`Device` gains a `staging_pool`: free staging buffers in power-of-two size classes from 4 KiB to
16 MiB, holding at most 64 MiB. `create_buffer` gives a `TRANSIENT`, map-write, not map-read
request a pooled buffer of its class, or allocates one at the class size, and records the class on
the `Buffer` (`staging_class`; `None` everywhere else, `buffer_from_raw` included). A request past
16 MiB allocates and frees as upstream. `destroy_buffer` returns a classed buffer to the pool, or
releases it past the cap. `wgpu-core` destroys a staging buffer only once the GPU is done with it
(its pending-writes temp resources are triaged on the submission's completion), so the next write
may fill it.

## How to check

`diff -ru <registry>/wgpu-hal-27.0.4 third_party/wgpu-hal` shows the patch and this file, nothing
else. A `sample` of a Dalaran run shows the effect: next to no
`StagingBuffer::new` under the render world's prepare systems, and no `-[AGXBuffer dealloc]` under
`Queue::maintain`.
