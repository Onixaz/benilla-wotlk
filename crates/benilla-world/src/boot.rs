//! The tuned Bevy boot: the `DefaultPlugins` set every benilla binary stands on. The engine tuning
//! is shared; the `Window` is the caller's.

use bevy::app::{PluginGroupBuilder, TaskPoolOptions, TaskPoolPlugin};
use bevy::prelude::*;

use crate::thread_qos;

/// `DefaultPlugins` with benilla's engine tuning applied, around the caller's primary window.
pub fn tuned_default_plugins(primary_window: Window) -> PluginGroupBuilder {
    // `WOW_THREADS=1` serialises the frame's systems, a diagnostic: a defect that survives it is
    // not a race between two systems. `WOW_COMPUTE_THREADS=<n>` pins the pool's size, a
    // measurement lever for the per-spawn wake cost.
    let serial = std::env::var("WOW_THREADS").ok().as_deref() == Some("1");
    let split = pool_split(
        bevy::tasks::available_parallelism(),
        if serial { Some(1) } else { compute_threads() },
    );
    DefaultPlugins
        .set(WindowPlugin {
            primary_window: Some(primary_window),
            ..default()
        })
        // Deliberately no `AssetPlugin::file_path`: a baked source path resolves only on the build
        // machine. Every shader is embedded (`embedded://<crate>/shaders/…`), so no root is read.
        // Quiet wgpu/naga; the ring keeps the last stderr lines for the crash report (`log_ring`).
        .set(bevy::log::LogPlugin {
            filter: "wgpu=error,naga=warn".into(),
            custom_layer: |_| Some(Box::new(crate::log_ring::LogRing)),
            ..default()
        })
        // Asset loads parse synchronously on the IO pool, so a dense teleport waits on its width:
        // IO takes half the cores, floor 2 and ceiling 8. Compute, where every frame's parallel
        // passes run, keeps the size the quarter split left it ([`pool_split`]), so the IO threads
        // ride on top, idle outside a load. Workers spawn at default QoS, behind any background
        // build: compute is user-interactive, IO and async compute user-initiated, and
        // `ThreadQosPlugin` promotes the render thread from inside.
        .set(TaskPoolPlugin {
            task_pool_options: TaskPoolOptions {
                io: bevy::app::TaskPoolThreadAssignmentPolicy {
                    min_threads: split.io,
                    max_threads: split.io,
                    percent: 1.0,
                    on_thread_spawn: Some(std::sync::Arc::new(|| {
                        thread_qos::promote_current_thread(thread_qos::QosClass::UserInitiated)
                    })),
                    on_thread_destroy: None,
                },
                async_compute: bevy::app::TaskPoolThreadAssignmentPolicy {
                    on_thread_spawn: Some(std::sync::Arc::new(|| {
                        thread_qos::promote_current_thread(thread_qos::QosClass::UserInitiated)
                    })),
                    ..TaskPoolOptions::default().async_compute
                },
                compute: bevy::app::TaskPoolThreadAssignmentPolicy {
                    on_thread_spawn: Some(std::sync::Arc::new(|| {
                        thread_qos::promote_current_thread(thread_qos::QosClass::UserInteractive)
                    })),
                    min_threads: split.compute,
                    max_threads: split.compute,
                    ..TaskPoolOptions::default().compute
                },
                ..default()
            },
        })
        // Sound is kira behind our own mixer; `bevy_audio` is off by feature (workspace
        // `Cargo.toml`). Kept though they look idle: gizmos (bowstring, fishing line), sprites (the
        // FrameXML quad pass), picking, TextPlugin (glue text), PostProcessPlugin (glow bloom) and
        // ScenePlugin (avian's collider backend reads `SceneSpawner`); the ForwardDecal family is
        // registered inside `PbrPlugin::build`, so it cannot be disabled on its own.
        //
        // M2/WMO/ADT load through our own `mpq://` loaders; there is no glTF.
        .disable::<bevy::gltf::GltfPlugin>()
        // No bevy AA: no Fxaa/TAA/SMAA/CAS component anywhere (MSAA is core render, unaffected).
        .disable::<bevy::anti_alias::AntiAliasPlugin>()
        // No gamepad input; 1.12's bindings are keyboard/mouse.
        .disable::<bevy::gilrs::GilrsPlugin>()
}

/// The IO and compute pools' thread counts on `cores`, with an optional compute override.
struct PoolSplit {
    io: usize,
    compute: usize,
}

/// IO gets half of `cores`, clamped to 2..=8. Compute gets what Bevy's quarter split for IO and
/// async compute (2..=8 and 1..=4) leaves, at least 1, unless `compute_override` names it.
fn pool_split(cores: usize, compute_override: Option<usize>) -> PoolSplit {
    let share = |percent: f32| (cores as f32 * percent).round() as usize;
    let quarter_io = share(0.25).min(cores).clamp(2, 8);
    let async_compute = share(0.25)
        .min(cores.saturating_sub(quarter_io))
        .clamp(1, 4);
    PoolSplit {
        io: share(0.5).min(cores).clamp(2, 8),
        compute: compute_override
            .unwrap_or_else(|| cores.saturating_sub(quarter_io + async_compute).max(1)),
    }
}

/// `WOW_COMPUTE_THREADS=<n>`: the compute pool's thread count, unset for Bevy's share.
fn compute_threads() -> Option<usize> {
    std::env::var("WOW_COMPUTE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n >= 1)
}

#[cfg(test)]
mod tests {
    use super::pool_split;

    #[test]
    fn io_doubles_while_compute_keeps_the_quarter_split_remainder() {
        let s = pool_split(8, None);
        assert_eq!((s.io, s.compute), (4, 4));
        let s = pool_split(12, None);
        assert_eq!((s.io, s.compute), (6, 6));
        let s = pool_split(32, None);
        assert_eq!((s.io, s.compute), (8, 20));
    }

    #[test]
    fn small_machines_keep_their_floors() {
        let s = pool_split(2, None);
        assert_eq!((s.io, s.compute), (2, 1));
        let s = pool_split(4, None);
        assert_eq!((s.io, s.compute), (2, 1));
    }

    #[test]
    fn a_compute_override_wins() {
        let s = pool_split(8, Some(1));
        assert_eq!((s.io, s.compute), (4, 1));
    }
}
