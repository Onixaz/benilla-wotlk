//! `Sound_EnableSoftwareHRTF`: the reference initialises FMOD with `FMOD_INIT_SOFTWARE_HRTF`
//! (`0x10`, the engine init at `0x87d420`), which puts a lowpass on every 3D software voice. Its
//! cutoff is open while the sound is within 90° of the listener's forward axis and falls linearly
//! to 4000 Hz directly behind (FMOD's `HRTFMinAngle` 180, `HRTFMaxAngle` 360, `HRTFFreq` 4000; WoW
//! never calls `setAdvancedSettings`). The filter is two cascaded one-pole RC stages, bypassed at the
//! open cutoff.
//!
//! The main thread works out each voice's cutoff from the listener pose ([`cutoff_for`]) and
//! stores it in the voice's [`HrtfCutoff`]; the effect reads it once per block.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use bevy::math::{Quat, Vec3};
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;

/// The cutoff at which the filter is bypassed: the sound is in front of the listener.
pub(super) const OPEN_HZ: f32 = 22050.0;

/// The cutoff directly behind the listener.
const BEHIND_HZ: f32 = 4000.0;

/// One voice's cutoff in Hz, written by the main thread and read by the audio thread.
#[derive(Debug)]
pub(crate) struct HrtfCutoff(AtomicU32);

impl HrtfCutoff {
    fn new() -> Self {
        Self(AtomicU32::new(OPEN_HZ.to_bits()))
    }

    pub(super) fn set(&self, hz: f32) {
        self.0.store(hz.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// The cutoff for a sound at `pos` heard by a listener at `listener` facing `rot` (Bevy space,
/// forward `-Z`). The angle is taken in the listener's right/forward plane, as FMOD's pan angle
/// is; a sound straight above or below reads as in front.
pub(super) fn cutoff_for(listener: Vec3, rot: Quat, pos: Vec3) -> f32 {
    let dir = pos - listener;
    let lr = dir.dot(rot * Vec3::X);
    let fb = dir.dot(rot * Vec3::NEG_Z);
    if lr == 0.0 && fb == 0.0 {
        return OPEN_HZ;
    }
    let angle = lr.abs().atan2(fb).to_degrees();
    if angle <= 90.0 {
        OPEN_HZ
    } else {
        let strength = ((angle - 90.0) / 90.0).min(1.0);
        BEHIND_HZ + (OPEN_HZ - BEHIND_HZ) * (1.0 - strength)
    }
}

/// The one-pole stage's coefficient at `hz` for `rate`: the RC time constant up to `rate / π`,
/// then a straight ramp to 1 (no filter) at [`OPEN_HZ`].
fn coefficient(hz: f32, rate: f32) -> f32 {
    if hz >= OPEN_HZ {
        return 1.0;
    }
    let threshold = rate / std::f32::consts::PI;
    if hz <= threshold {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * hz);
        let dt = 1.0 / rate;
        dt / (rc + dt)
    } else {
        2.0 / 3.0 + (hz - threshold) / (3.0 * (OPEN_HZ - threshold))
    }
}

/// Builds the lowpass for one voice and hands back its cutoff cell.
pub(super) struct HrtfBuilder;

impl EffectBuilder for HrtfBuilder {
    type Handle = Arc<HrtfCutoff>;

    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        let cutoff = Arc::new(HrtfCutoff::new());
        let effect = HrtfLowpass {
            cutoff: Arc::clone(&cutoff),
            rate: 48_000.0,
            tc: 1.0,
            y1: Frame::ZERO,
            y2: Frame::ZERO,
        };
        (Box::new(effect), cutoff)
    }
}

struct HrtfLowpass {
    cutoff: Arc<HrtfCutoff>,
    rate: f32,
    /// The coefficient the last block ended on; a move ramps across the next block.
    tc: f32,
    y1: Frame,
    y2: Frame,
}

impl Effect for HrtfLowpass {
    fn init(&mut self, sample_rate: u32, _internal_buffer_size: usize) {
        self.rate = sample_rate as f32;
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.rate = sample_rate as f32;
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        let target = coefficient(self.cutoff.get(), self.rate);
        if target >= 1.0 && self.tc >= 1.0 {
            return;
        }
        if self.tc >= 1.0 {
            // Re-engaging: start from the signal, not from a stale tail.
            if let Some(first) = input.first() {
                self.y1 = *first;
                self.y2 = *first;
            }
        }
        let from = self.tc;
        let n = input.len().max(1) as f32;
        for (i, frame) in input.iter_mut().enumerate() {
            let tc = from + (target - from) * ((i + 1) as f32 / n);
            let keep = 1.0 - tc;
            self.y1 = *frame * tc + self.y1 * keep;
            self.y2 = self.y1 * tc + self.y2 * keep;
            *frame = self.y2;
        }
        self.tc = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open in front and to the side, 4000 Hz straight behind, halfway at 135°.
    #[test]
    fn the_cutoff_falls_from_the_side_to_behind() {
        let at = Vec3::ZERO;
        let rot = Quat::IDENTITY;
        assert_eq!(cutoff_for(at, rot, Vec3::NEG_Z), OPEN_HZ);
        assert_eq!(cutoff_for(at, rot, Vec3::X), OPEN_HZ);
        assert_eq!(cutoff_for(at, rot, Vec3::NEG_X), OPEN_HZ);
        assert!((cutoff_for(at, rot, Vec3::Z) - BEHIND_HZ).abs() < 1e-2);
        let diag = cutoff_for(at, rot, Vec3::new(1.0, 0.0, 1.0));
        assert!((diag - (BEHIND_HZ + OPEN_HZ) / 2.0).abs() < 1.0, "{diag}");
        assert_eq!(cutoff_for(at, rot, Vec3::Y), OPEN_HZ);
        // A listener turned around hears the same sound from in front.
        let turned = Quat::from_rotation_y(std::f32::consts::PI);
        assert_eq!(cutoff_for(at, turned, Vec3::Z), OPEN_HZ);
    }

    /// The coefficient is 1 (bypass) at the open cutoff and the RC value below `rate / π`.
    #[test]
    fn the_stage_coefficient_is_the_rc_time_constant() {
        assert_eq!(coefficient(OPEN_HZ, 48_000.0), 1.0);
        let rc = 1.0 / (2.0 * std::f32::consts::PI * BEHIND_HZ);
        let dt = 1.0 / 48_000.0;
        assert!((coefficient(BEHIND_HZ, 48_000.0) - dt / (rc + dt)).abs() < 1e-6);
        let above = coefficient(20_000.0, 48_000.0);
        assert!(above > 2.0 / 3.0 && above < 1.0, "{above}");
    }

    /// Behind the listener the filter takes the top off a full-band signal; in front it is bypassed.
    #[test]
    fn behind_is_duller_than_in_front() {
        let (mut effect, cutoff) = HrtfBuilder.build();
        effect.init(48_000, 128);
        let info = kira::info::MockInfoBuilder::new().build();
        // A Nyquist-rate square, all top end.
        let tone = |n: usize| -> Vec<Frame> {
            (0..n)
                .map(|i| Frame::from_mono(if i % 2 == 0 { 1.0 } else { -1.0 }))
                .collect()
        };
        let mut front = tone(256);
        effect.process(&mut front, 1.0 / 48_000.0, &info);
        assert_eq!(front[200].left, 1.0);
        cutoff.set(BEHIND_HZ);
        for _ in 0..4 {
            let mut behind = tone(256);
            effect.process(&mut behind, 1.0 / 48_000.0, &info);
            front = behind;
        }
        assert!(front[200].left.abs() < 0.2, "{}", front[200].left);
    }
}
