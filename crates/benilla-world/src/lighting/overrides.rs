//! The server's light overrides (`SMSG_OVERRIDE_LIGHT`): each fades in or out over the packet's
//! transition and blends after the area lights (`0x7f1360`); a map change drops them all
//! (`0x7f1d30`).

use bevy::prelude::*;

use benilla_formats::LightOverride;

/// One override and where its fade stands.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    light: u32,
    override_id: u32,
    /// Fading in (state 1) or out (state 2).
    fading_in: bool,
    fade: f32,
    duration_ms: u32,
}

/// The live overrides, in the order they arrived.
#[derive(Resource, Debug, Default)]
pub struct LightOverrides {
    entries: Vec<Entry>,
    /// The map they belong to; another map drops them.
    map: Option<u32>,
}

impl LightOverrides {
    /// The packet (`0x526530`): fade out the row's override, then, for a non-zero override, fade in
    /// the new one from 0 (`0x7f11a0`, `0x7f10a0`).
    pub fn apply(&mut self, light: u32, override_id: u32, transition_ms: u32) {
        self.remove(light, transition_ms);
        if override_id != 0 {
            self.entries.push(Entry {
                light,
                override_id,
                fading_in: true,
                fade: 0.0,
                duration_ms: transition_ms,
            });
        }
    }

    /// `0x7f11a0` marks the first entry for the row as fading out. Deviation: it takes the first
    /// still fading in, where the reference's first match can be one already fading out, which
    /// leaves the live override up for good.
    fn remove(&mut self, light: u32, transition_ms: u32) {
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.light == light && e.fading_in)
        {
            e.fading_in = false;
            e.duration_ms = transition_ms;
        }
    }

    /// Drop every override.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Advance every fade by `elapsed_ms` and drop the faded-out ones, as each blend pass does; a
    /// zero transition is instant. A map other than the overrides' own drops them first.
    pub(super) fn tick(&mut self, map: u32, elapsed_ms: f32) {
        if self.map != Some(map) {
            self.map = Some(map);
            self.entries.clear();
        }
        for e in &mut self.entries {
            let step = if e.duration_ms == 0 {
                1.0
            } else {
                elapsed_ms / e.duration_ms as f32
            };
            e.fade = if e.fading_in {
                (e.fade + step).min(1.0)
            } else {
                (e.fade - step).max(0.0)
            };
        }
        self.entries.retain(|e| e.fading_in || e.fade > 0.0);
    }

    /// The overrides at their current fades, for the blend.
    pub(super) fn current(&self) -> Vec<LightOverride> {
        self.entries
            .iter()
            .map(|e| LightOverride {
                light: e.light,
                override_id: e.override_id,
                fade: e.fade,
            })
            .collect()
    }

    /// Whether any override is live.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether [`Self::tick`] has work: a live override, or a map it has not seen.
    pub(super) fn needs_tick(&self, map: u32) -> bool {
        !self.entries.is_empty() || self.map != Some(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fades(o: &LightOverrides) -> Vec<(u32, f32)> {
        o.current()
            .iter()
            .map(|e| (e.override_id, e.fade))
            .collect()
    }

    /// In over the transition, out over the next, gone once faded; a new override on the same row
    /// fades in while the old one fades out.
    #[test]
    fn overrides_fade_in_and_out_over_their_transitions() {
        let mut o = LightOverrides::default();
        o.tick(0, 0.0);
        o.apply(10, 20, 1000);
        o.tick(0, 500.0);
        assert_eq!(fades(&o), vec![(20, 0.5)]);
        o.tick(0, 800.0);
        assert_eq!(fades(&o), vec![(20, 1.0)]);
        o.apply(10, 30, 2000);
        o.tick(0, 1000.0);
        assert_eq!(fades(&o), vec![(20, 0.5), (30, 0.5)]);
        o.apply(10, 0, 0);
        o.tick(0, 1000.0);
        assert_eq!(fades(&o), vec![], "a zero transition is instant");
    }

    /// `0x7f1d30`: a map change drops them.
    #[test]
    fn a_map_change_drops_every_override() {
        let mut o = LightOverrides::default();
        o.tick(0, 0.0);
        o.apply(10, 20, 0);
        o.tick(0, 1.0);
        assert!(!o.is_empty());
        o.tick(571, 1.0);
        assert!(o.is_empty());
    }
}
