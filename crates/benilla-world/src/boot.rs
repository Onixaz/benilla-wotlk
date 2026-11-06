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
        // IO takes half the cores, floor 2 and ceiling 8. Pipeline compiles and collider builds
        // share async compute, so the loading cover's pipeline warm waits on its width: it takes
        // half the cores too, floor 1 and ceiling 4. Compute, where every frame's parallel passes
        // run, keeps the size the quarter split left it ([`pool_split`]), so the IO and async
        // threads ride on top, idle outside a load. Workers spawn at default QoS, behind any background
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
                    min_threads: split.async_compute,
                    max_threads: split.async_compute,
                    percent: 1.0,
                    on_thread_destroy: None,
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

/// The IO, async-compute and compute pools' thread counts on `cores`, with an optional compute
/// override.
struct PoolSplit {
    io: usize,
    async_compute: usize,
    compute: usize,
}

/// IO gets half of `cores`, clamped to 2..=8, and async compute half, clamped to 1..=4. Compute
/// gets what Bevy's quarter split for the two (2..=8 and 1..=4) leaves, at least 1, unless
/// `compute_override` names it.
fn pool_split(cores: usize, compute_override: Option<usize>) -> PoolSplit {
    let share = |percent: f32| (cores as f32 * percent).round() as usize;
    let quarter_io = share(0.25).min(cores).clamp(2, 8);
    let async_compute = share(0.25)
        .min(cores.saturating_sub(quarter_io))
        .clamp(1, 4);
    PoolSplit {
        io: share(0.5).min(cores).clamp(2, 8),
        async_compute: share(0.5).min(cores).clamp(1, 4),
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

    fn split(cores: usize, compute_override: Option<usize>) -> (usize, usize, usize) {
        let s = pool_split(cores, compute_override);
        (s.io, s.async_compute, s.compute)
    }

    #[test]
    fn io_and_async_widen_while_compute_keeps_the_quarter_split_remainder() {
        assert_eq!(split(8, None), (4, 4, 4));
        assert_eq!(split(12, None), (6, 4, 6));
        assert_eq!(split(32, None), (8, 4, 20));
    }

    #[test]
    fn small_machines_keep_their_floors() {
        assert_eq!(split(2, None), (2, 1, 1));
        assert_eq!(split(4, None), (2, 2, 1));
    }

    #[test]
    fn a_compute_override_wins() {
        assert_eq!(split(8, Some(1)), (4, 4, 1));
    }
}
