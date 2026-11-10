//! The plate cast bar's state, as the reference's nameplate frame keeps it (`+0x2f4` casting,
//! `+0x2f5` channeling, `+0x2fc` fade): the cast setters push the target's cast into it
//! (`0x720e50`), and every frame advances it (`0x98e9f0`), so the bar runs on between packets.

use benilla_formats::SpellDisplay;
use benilla_ui::script::PlateCast;

/// What the plate's cast setter (`0x720e50`) reads off the target: its channel, else its cast.
/// Only the target's plate is fed; any other unit's reads `None`, the stop (`0x715b20`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CastSource {
    pub(crate) spell_id: u32,
    pub(crate) channel: bool,
    /// On the `GetTime` clock, in milliseconds.
    pub(crate) start_ms: f64,
    pub(crate) end_ms: f64,
    /// The unit's not-interruptible test (`0x71ab20`): the shield art in place of the border.
    pub(crate) not_interruptible: bool,
}

/// The casting colour, `(1, 0.7, 0)` (`0x98f0f3`, `[0x9e2ec0]` = 0.7).
const CASTING: [f32; 3] = [1.0, 0.7, 0.0];
/// A bar pushed already at its end turns red (`0x98f1c9`).
const FAILED: [f32; 3] = [1.0, 0.0, 0.0];
/// A bar that runs out on its own turns green (`0x98ed47`).
const FINISHED: [f32; 3] = [0.0, 1.0, 0.0];

/// One plate's cast bar.
#[derive(Clone, Debug, Default)]
pub(crate) struct PlateCastBar {
    shown: bool,
    min: f32,
    max: f32,
    value: f32,
    casting: bool,
    channeling: bool,
    /// Seconds left of the closing fade; the bar hides when it runs out.
    fade: f32,
    colour: [f32; 3],
    shielded: bool,
    icon: Option<String>,
    /// The source last pushed, so a push happens on a change, as the reference's setters call.
    source: Option<CastSource>,
}

impl PlateCastBar {
    /// Push `source` if it moved since the last push, valued at `now_ms`: a running cast fills
    /// from its start, a channel drains to its end; `None` is the stop's all-zero push.
    pub(crate) fn feed<'a>(
        &mut self,
        source: Option<CastSource>,
        now_ms: f64,
        spell: impl FnOnce(u32) -> Option<&'a SpellDisplay>,
    ) {
        if self.source == source {
            return;
        }
        self.source = source;
        match source {
            None => self.set_values(0.0, 0.0, 0.0, None, false, false),
            Some(s) => {
                let secs = |ms: f64| (ms * 0.001) as f32;
                let max = secs(s.end_ms - s.start_ms);
                let value = if s.channel {
                    secs(s.end_ms - now_ms)
                } else {
                    secs(now_ms - s.start_ms)
                };
                self.set_values(
                    0.0,
                    max,
                    value,
                    spell(s.spell_id),
                    s.channel,
                    s.not_interruptible,
                );
            }
        }
    }

    /// The setter (`0x98f040`). A value past the max changes nothing; no spell row, or a trade
    /// skill (`Attributes & 0x20`), hides the bar. A bar short of its end runs in the casting
    /// colour; one at its end starts the fade in red.
    fn set_values(
        &mut self,
        min: f32,
        max: f32,
        value: f32,
        spell: Option<&SpellDisplay>,
        channel: bool,
        not_interruptible: bool,
    ) {
        if max < value {
            return;
        }
        self.min = min;
        self.max = max;
        self.value = value.clamp(min, max);
        self.channeling = channel;
        let Some(spell) = spell.filter(|d| d.attributes & 0x20 == 0) else {
            self.hide();
            return;
        };
        let running = if channel { min != value } else { max != value };
        if running {
            self.colour = CASTING;
            self.casting = true;
            self.fade = 0.0;
        } else {
            self.colour = FAILED;
            self.casting = false;
            self.fade = 1.0;
        }
        self.shielded = not_interruptible;
        self.shown = true;
        // A spell whose icon row is missing keeps the last texture.
        if let Some(icon) = &spell.icon {
            self.icon = Some(icon.clone());
        }
    }

    /// `0x98e7e0`: hide the bar and its art and drop the cast and the fade.
    fn hide(&mut self) {
        if !self.shown {
            return;
        }
        self.shown = false;
        self.casting = false;
        self.fade = 0.0;
    }

    /// The frame's cast-bar step (`0x98e9f0`): with `showVKeyCastbar` off the bar hides; a fade
    /// counts down and then hides; a cast fills, a channel drains, and one running out turns green
    /// and starts the fade.
    pub(crate) fn tick(&mut self, dt: f32, enabled: bool) {
        if !enabled {
            self.hide();
            return;
        }
        if self.fade > 0.0 {
            self.fade -= dt;
            if self.fade <= 0.0 {
                self.hide();
            }
            return;
        }
        if !self.casting {
            self.hide();
            return;
        }
        let (value, done) = if self.channeling {
            let v = self.value - dt;
            (v.max(self.min), v < self.min)
        } else {
            let v = self.value + dt;
            (v.min(self.max), v > self.max)
        };
        if done {
            self.fade = 1.0;
            self.colour = FINISHED;
            self.casting = false;
        }
        self.value = value;
    }

    /// What the widgets draw, `None` while hidden. Deviation: the fade is linear over its second;
    /// the reference multiplies each alpha by the time left every frame (`0x98ebfb`), a decay
    /// that depends on the frame rate.
    pub(crate) fn view(&self) -> Option<PlateCast> {
        self.shown.then(|| {
            let alpha = if self.fade > 0.0 {
                self.fade.min(1.0)
            } else {
                1.0
            };
            let [r, g, b] = self.colour;
            PlateCast {
                min: self.min,
                max: self.max,
                value: self.value,
                colour: [r, g, b, alpha],
                shielded: self.shielded,
                icon: self.icon.clone(),
                alpha,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOLT: u32 = 133;

    fn bolt(id: u32) -> Option<&'static SpellDisplay> {
        static BOLT_ROW: std::sync::LazyLock<SpellDisplay> =
            std::sync::LazyLock::new(|| SpellDisplay {
                icon: Some(r"Interface\Icons\Spell_Fire_FlameBolt".into()),
                ..Default::default()
            });
        (id == BOLT).then(|| &*BOLT_ROW)
    }

    fn cast(start_ms: f64, end_ms: f64) -> CastSource {
        CastSource {
            spell_id: BOLT,
            channel: false,
            start_ms,
            end_ms,
            not_interruptible: false,
        }
    }

    /// A cast pushed half way fills from there in the casting colour, turns green at its end and
    /// hides one second later.
    #[test]
    fn a_cast_fills_then_fades_green() {
        let mut bar = PlateCastBar::default();
        bar.feed(Some(cast(10_000.0, 12_000.0)), 11_000.0, bolt);
        let v = bar.view().expect("shown");
        assert_eq!((v.min, v.max, v.value), (0.0, 2.0, 1.0));
        assert_eq!(v.colour, [1.0, 0.7, 0.0, 1.0]);
        assert_eq!(
            v.icon.as_deref(),
            Some(r"Interface\Icons\Spell_Fire_FlameBolt")
        );
        assert!(!v.shielded);
        bar.tick(0.5, true);
        assert_eq!(bar.view().unwrap().value, 1.5);
        bar.tick(0.6, true);
        let v = bar.view().unwrap();
        assert_eq!(
            (v.value, v.colour),
            (2.0, [0.0, 1.0, 0.0, 1.0]),
            "ran out: green"
        );
        bar.tick(0.5, true);
        assert_eq!(bar.view().unwrap().alpha, 0.5, "half way through the fade");
        bar.tick(0.5, true);
        assert!(bar.view().is_none(), "the fade ran out");
    }

    /// A channel drains from its time left; the shield replaces the border when it cannot be
    /// interrupted.
    #[test]
    fn a_channel_drains_and_shows_the_shield() {
        let mut bar = PlateCastBar::default();
        let source = CastSource {
            channel: true,
            not_interruptible: true,
            ..cast(0.0, 5_000.0)
        };
        bar.feed(Some(source), 1_000.0, bolt);
        let v = bar.view().unwrap();
        assert_eq!((v.max, v.value, v.shielded), (5.0, 4.0, true));
        bar.tick(1.0, true);
        assert_eq!(bar.view().unwrap().value, 3.0);
    }

    /// The stop (the cast ended, or the unit is no longer the target) hides at once, with no fade.
    #[test]
    fn the_stop_hides_at_once() {
        let mut bar = PlateCastBar::default();
        bar.feed(Some(cast(0.0, 2_000.0)), 500.0, bolt);
        assert!(bar.view().is_some());
        bar.feed(None, 600.0, bolt);
        assert!(bar.view().is_none());
    }

    /// `showVKeyCastbar` "0" hides the bar every frame and drops the cast, so turning it back on
    /// mid-cast shows nothing until the next push.
    #[test]
    fn the_cvar_hides_the_bar_and_drops_the_cast() {
        let mut bar = PlateCastBar::default();
        bar.feed(Some(cast(0.0, 2_000.0)), 0.0, bolt);
        bar.tick(0.1, false);
        assert!(bar.view().is_none());
        bar.tick(0.1, true);
        assert!(bar.view().is_none(), "no push since, nothing to show");
        bar.feed(Some(cast(0.0, 3_000.0)), 200.0, bolt);
        assert!(bar.view().is_some(), "a pushback re-pushes");
    }

    /// A trade skill or a spell with no row hides; a value past the max (an overdue cast) leaves
    /// the bar as it was; a cast pushed at its very end starts the fade in red.
    #[test]
    fn the_setter_gates() {
        let mut bar = PlateCastBar::default();
        let trade = SpellDisplay {
            attributes: 0x20,
            ..Default::default()
        };
        bar.feed(Some(cast(0.0, 2_000.0)), 0.0, |_| Some(&trade));
        assert!(bar.view().is_none(), "trade skill");
        bar.feed(Some(cast(0.0, 1_000.0)), 0.0, |_| None);
        assert!(bar.view().is_none(), "no spell row");
        bar.feed(Some(cast(0.0, 2_000.0)), 3_000.0, bolt);
        assert!(bar.view().is_none(), "overdue: no change");
        bar.feed(Some(cast(0.0, 2_500.0)), 2_500.0, bolt);
        assert_eq!(
            bar.view().unwrap().colour,
            [1.0, 0.0, 0.0, 1.0],
            "at its end: red"
        );
    }
}
