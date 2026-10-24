# benilla's `bevy_render` fork — what differs from upstream, and how to check

Upstream: [`bevyengine/bevy`](https://github.com/bevyengine/bevy), crate `bevy_render`, MIT OR
Apache-2.0, version `0.18.1` — the version the workspace lock resolves to. Wired in through
`[patch.crates-io]` in the workspace root, the way `kira` and `lua-src` are. The crate is copied
from the cargo registry as published (`src/`, `Cargo.toml`, `README.md` and the two licence
files); the registry's `Cargo.toml.orig`, `Cargo.lock` and cargo metadata files are left out.

## Why a fork exists at all

The stock Video window's Triple Buffering row (`gxTripleBuffer`) changes how many frames the
swapchain holds in flight, and the reference applies it at `RestartGx`, with no client restart.
`bevy_render` reads a window's `desired_maximum_frame_latency` only when it first creates that
window's surface: `extract_windows` copies it into the `ExtractedWindow` once, and the reconfigure
path in `create_surfaces` runs only on a size or present-mode change. The surface data is private,
so no code outside the crate can reconfigure it. Bevy's `main` branch has the same shape.

## Patch — `src/view/window/mod.rs`

`ExtractedWindow` gains `frame_latency_changed`, set by `extract_windows` when the window's
`desired_maximum_frame_latency` differs from the extracted copy, which it then updates. The flag
joins `size_changed` and `present_mode_changed` in the three places they decide a reconfigure:
`prepare_windows`' keep-the-swapchain-texture test, `need_surface_configuration`, and the
reconfigure branch of `create_surfaces`, which now also writes the latency into the surface
configuration.

## How to check

`diff -u <registry>/bevy_render-0.18.1/src/view/window/mod.rs
third_party/bevy_render/src/view/window/mod.rs` shows the patch and nothing else; every other file
is byte-identical to the registry crate. `benilla-app`'s `video` tests drive the window's latency
through `gxTripleBuffer` and a `RestartGx`.
