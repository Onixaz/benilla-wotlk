//! The cast-cancel live probe (`WOW_PROBE=castcancel`): once in-world it uses the Hearthstone (a
//! 10 s cast), then 2 s into the bar presses `W` through `ButtonInput<KeyCode>`, the controller
//! path a player's key takes to `spell::local_self_cancel` and `CMSG_CANCEL_CAST`.
//!
//! With `WOW_CAST_TRACE=1` the `LOCAL self-cancel` line must land frames after the `SEND move
//! StartForward` line, and the server's `RECV CAST_RESULT failure` must follow without repainting.
//! Every bar phase change is logged ([`bar_timeline`]) so the hold, burst and fade are measured
//! from timestamps. Non-combat: a failed cancel only hearths the character home. The switches are
//! `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use crate::net::SelfPlayer;

pub(crate) struct ProbeCastCancelPlugin;

impl Plugin for ProbeCastCancelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProbeCastCancel>()
            .add_systems(Update, (cast_cancel_probe, bar_timeline));
    }
}

/// The bar's phase, read from the stock `CastingBarFrame`'s state fields.
const BAR_PHASE_CHUNK: &str = "return (function()\n\
    local f = CastingBarFrame\n\
    if not f then return \"noui\" end\n\
    if not f:IsVisible() then return \"hidden\" end\n\
    local txt = CastingBarText:GetText() or \"?\"\n\
    if f.casting then return \"casting|\" .. txt end\n\
    if f.channeling then return \"channel|\" .. txt end\n\
    if GetTime() < (f.holdTime or 0) then return \"hold|\" .. txt end\n\
    if f.flash then return \"burst|\" .. txt end\n\
    if f.fadeOut then return \"fade|\" .. txt end\n\
    return \"shown|\" .. txt\n\
 end)()";

/// Logs each cast-bar phase change with a timestamp. Stock `CastingBarFrame.lua` holds
/// `CASTING_BAR_HOLD_TIME` (1 s), then steps the flash by 0.2 and the fade by 0.05 once per
/// `OnUpdate` (`CastingBarFrame.lua:132-147`), so burst and fade last 5 and 20 frames.
fn bar_timeline(world: &mut World, mut last: Local<crate::ui_script::VmMemo<String>>) {
    let mut in_world = world.query_filtered::<(), With<SelfPlayer>>();
    if in_world.iter(world).next().is_none() {
        return;
    }
    let elapsed = world.resource::<Time<bevy::time::Real>>().elapsed_secs();
    // Session-keyed: a phase string left from the last login would swallow the first transition.
    crate::ui_script::safe_world::lend(world, |script| {
        let last = last.get(script);
        let Ok(state) = script.eval::<String>(BAR_PHASE_CHUNK) else {
            return;
        };
        if state != *last {
            info!("castcancel probe: bar {elapsed:.3}s {state}");
            *last = state;
        }
    });
}

/// Probe state: when we entered the world, and the next phase to run.
#[derive(Resource, Default)]
struct ProbeCastCancel {
    entered: Option<f32>,
    phase: u8,
    used_at: Option<f32>,
}

/// Finds the Hearthstone in the backpack by item link and uses it; `false` while the item query
/// is still in flight, so the caller retries each frame.
const USE_HEARTH_CHUNK: &str = "return (function()\n\
    for s = 1, GetContainerNumSlots(0) do\n\
      local link = GetContainerItemLink(0, s)\n\
      if link and string.find(link, \"item:6948\", 1, true) then\n\
        UseContainerItem(0, s)\n\
        return true\n\
      end\n\
    end\n\
    return false\n\
 end)()";

/// From 3 s after world entry, use the Hearthstone (deadline 10 s), press `W` 2 s into the cast
/// and release it 0.3 s later; every phase logs.
fn cast_cancel_probe(world: &mut World) {
    let mut in_world = world.query_filtered::<(), With<SelfPlayer>>();
    if in_world.iter(world).next().is_none() {
        return;
    }
    let now = world.resource::<Time<bevy::time::Real>>().elapsed_secs();
    let mut probe = world.resource_mut::<ProbeCastCancel>();
    let entered = *probe.entered.get_or_insert(now);
    let t = now - entered;
    let (phase, used_at) = (probe.phase, probe.used_at.unwrap_or(f32::MAX));
    match phase {
        0 if t >= 3.0 => {
            let used =
                crate::ui_script::safe_world::lend(world, |s| s.eval::<bool>(USE_HEARTH_CHUNK));
            let mut probe = world.resource_mut::<ProbeCastCancel>();
            match used {
                None => {
                    probe.phase = 99;
                    error!("castcancel probe: no UI VM — cannot use the Hearthstone");
                }
                Some(Ok(true)) => {
                    probe.phase = 1;
                    probe.used_at = Some(t);
                    info!("castcancel probe: using the Hearthstone (10 s cast opens the bar)");
                }
                Some(Ok(false)) if t >= 10.0 => {
                    probe.phase = 99;
                    error!("castcancel probe: no Hearthstone link resolved by t+10 s");
                }
                Some(Ok(false)) => {} // still resolving, retry next frame
                Some(Err(e)) => {
                    probe.phase = 99;
                    error!("castcancel probe: {e}");
                }
            }
        }
        1 if t >= used_at + 2.0 => {
            world.resource_mut::<ProbeCastCancel>().phase = 2;
            info!("castcancel probe: pressing W mid-cast — the local cancel should fire NOW");
            world
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyW);
        }
        2 if t >= used_at + 2.3 => {
            world.resource_mut::<ProbeCastCancel>().phase = 3;
            info!("castcancel probe: releasing W — read the cast-trace for the verdict");
            world
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(KeyCode::KeyW);
        }
        _ => {}
    }
}
