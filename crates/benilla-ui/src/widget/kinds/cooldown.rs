//! The `Cooldown` widget (factory `0x51a380`, ctor `0x5ecae0`), a `CSimpleFrame` that draws its
//! own sweep: a dark wedge over the part of the duration still to run, an optional edge line at
//! the sweep angle, and a one-second star flash when a forward sweep completes. Its state and its
//! paint geometry, both pure; `script::cooldown_widget` binds them to Lua and the tick.

/// The finish flash's length, ms: `OnUpdate` re-arms the clock for it (`0x5ec946`).
pub const COOLDOWN_FLASH_MS: u32 = 1000;

/// The flash's tint, `rgb(0x50, 0xa0, 0xff)` (`0x5ec894`..`0x5ec89c`, BGRA).
pub const COOLDOWN_FLASH_RGB: [f32; 3] = [80.0 / 255.0, 160.0 / 255.0, 1.0];

/// The wedge's share of the frame alpha, `a · 160 / 255` (`0x5ec7d4`: `a · 5 << 5`, then `/ 255`).
pub const COOLDOWN_SWIPE_ALPHA: f32 = 160.0 / 255.0;

/// The flash keys, one per sixth of [`COOLDOWN_FLASH_MS`] (`× 0.006`, `0xa1d1fc`), interpolated
/// linearly: its alpha share (`0xad1434`), its rotation in radians (`0xc24254`, filled from
/// `0xa1d214`..`0xa1d200`, an eighth of a turn in sixths: −π/8 and −π/4 are exact f32 values
/// there) and its UV span (`0xad1418`), below 1 a magnified star.
const FLASH_ALPHA: [f32; 7] = [0.0, 0.5, 1.0, 0.75, 0.5, 0.25, 0.0];
const FLASH_ANGLE: [f32; 7] = [
    0.0,
    -0.130_376_1,
    -0.259_181_4,
    -std::f32::consts::FRAC_PI_8,
    -0.518_362_8,
    -0.651_880_5,
    -std::f32::consts::FRAC_PI_4,
];
const FLASH_UV_SPAN: [f32; 7] = [
    1.0,
    0.699_300_7,
    0.540_540_5,
    0.628_930_8,
    0.751_879_7,
    0.625,
    0.869_565_2,
];

/// A `Cooldown` frame's clock and switches, zeroed as the ctor zeroes them (`0x5ecd2e`..`0x5ecd58`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CooldownState {
    /// The sweep's start, `GetTime()` ms truncated (`+0x29c`); the flash re-arms it.
    pub start_ms: u32,
    /// Its length, ms (`+0x2a0`); 1000 during the flash.
    pub duration_ms: u32,
    /// `now − start` as of the last update, ms (`+0x2a4`).
    pub elapsed_ms: u32,
    /// The flash is running (`+0x2a8`).
    pub flashing: bool,
    /// `SetReverse` (`+0x2ac`): the dark part is what has run, and no flash follows.
    pub reverse: bool,
    /// `SetDrawEdge` (`+0x3d8`).
    pub draw_edge: bool,
    /// A `SetCooldown` has loaded the art (`+0x2b0`, `+0x3d4`); until then nothing draws.
    pub armed: bool,
}

/// What an update did to the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CooldownStep {
    /// Still sweeping or flashing.
    Running,
    /// The flash ended: the frame hides (`0x5ec98a`).
    Hide,
}

impl CooldownState {
    /// `SetCooldown(start, duration)` past its argument checks (`0x5ecd70`): the clock restarts,
    /// the flash ends, and the art is loaded.
    pub fn set(&mut self, start_ms: u32, duration_ms: u32) {
        self.start_ms = start_ms;
        self.duration_ms = duration_ms;
        self.elapsed_ms = 0;
        self.flashing = false;
        self.armed = true;
    }

    /// The per-frame update of a shown cooldown (`0x5ec8f0`). `now − start` is unsigned
    /// (`0x5ec91b`), so a start still in the future counts as done.
    pub fn update(&mut self, now_ms: u32) -> CooldownStep {
        self.elapsed_ms = now_ms.wrapping_sub(self.start_ms);
        if self.elapsed_ms < self.duration_ms {
            return CooldownStep::Running;
        }
        if self.flashing {
            self.flashing = false;
            return CooldownStep::Hide;
        }
        if self.reverse {
            // A reversed sweep holds its last frame and never flashes (`0x5ec969`).
            self.elapsed_ms = self.duration_ms.wrapping_sub(1);
        } else {
            self.start_ms = now_ms;
            self.flashing = true;
            self.duration_ms = COOLDOWN_FLASH_MS;
            self.elapsed_ms = 0;
        }
        CooldownStep::Running
    }

    /// What an update can move of the paint's inputs, as one key: an unarmed box draws nothing,
    /// so its clock does not count.
    pub fn sweep_key(&self) -> (bool, u32, bool) {
        (
            self.armed,
            if self.armed { self.elapsed_ms } else { 0 },
            self.armed && self.flashing,
        )
    }

    /// The paint for this state (`0x5ebd20` over the geometry `0x5ec1e0` builds), `None` while
    /// unarmed. `layout_scale` is the frame's layout scale (layout `+0x5c`), which the edge's UV
    /// rotation multiplies by (`0x5ec34a`).
    pub fn paint(&self, layout_scale: f32) -> Option<CooldownPaint> {
        if !self.armed {
            return None;
        }
        if self.flashing {
            // The wedge and edge colours are zero in the flash (`0x5ec84f`); a reversed one, which
            // never flashes, has nothing to show.
            if self.reverse {
                return None;
            }
            let t = self.elapsed_ms as f32 * 0.006;
            let i = (t as usize).min(5);
            let f = t - i as f32;
            let lerp = |k: &[f32; 7]| k[i] + (k[i + 1] - k[i]) * f;
            return Some(CooldownPaint {
                swipe: Vec::new(),
                edge: None,
                flash: Some(CooldownFlash {
                    uvs: rotated_uvs(lerp(&FLASH_ANGLE), lerp(&FLASH_UV_SPAN)),
                    alpha: lerp(&FLASH_ALPHA),
                }),
            });
        }
        let fraction = if self.duration_ms == 0 {
            1.0
        } else {
            self.elapsed_ms as f32 / self.duration_ms as f32
        };
        Some(CooldownPaint {
            swipe: swipe_rim(fraction, self.reverse),
            edge: self
                .draw_edge
                .then(|| rotated_uvs(fraction * -std::f32::consts::TAU, layout_scale)),
            flash: None,
        })
    }
}

/// A `Cooldown`'s paint, in unit coordinates over the frame's rect: `x` right and `y` down from
/// its top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct CooldownPaint {
    /// The dark wedge (ARTWORK, `BLEND`, `0x5ebd7d`): a fan about the centre over these rim points
    /// in order (indices `0xad13e8`), black at [`COOLDOWN_SWIPE_ALPHA`] of the frame alpha. Its
    /// texture, `star4.blp`, is opaque, so the tint alone shows. Empty when nothing is dark.
    pub swipe: Vec<[f32; 2]>,
    /// The `edge.blp` quad over the whole rect (OVERLAY, `0x5ebdf8`) under `SetDrawEdge`: the
    /// corner UVs, top-left, top-right, bottom-right, bottom-left, white at the frame alpha.
    pub edge: Option<[[f32; 2]; 4]>,
    /// The finish flash (ARTWORK, `ADD`, `0x5ebdae`).
    pub flash: Option<CooldownFlash>,
}

/// The `star4.blp` quad over the whole rect while a sweep finishes.
#[derive(Clone, Debug, PartialEq)]
pub struct CooldownFlash {
    /// Corner UVs, top-left, top-right, bottom-right, bottom-left.
    pub uvs: [[f32; 2]; 4],
    /// The keyed share of the frame alpha, tinted [`COOLDOWN_FLASH_RGB`].
    pub alpha: f32,
}

/// The rim points clockwise from twelve o'clock, corners and edge midpoints (the octant builders
/// `0x5eb130`, `0x5eb3a0`, `0x5eb670`, `0x5eb9a0`).
const RIM: [[f32; 2]; 9] = [
    [0.5, 0.0],
    [1.0, 0.0],
    [1.0, 0.5],
    [1.0, 1.0],
    [0.5, 1.0],
    [0.0, 1.0],
    [0.0, 0.5],
    [0.0, 0.0],
    [0.5, 0.0],
];

/// The wedge's rim at `fraction` of the sweep: octant `n = trunc(8·fraction)` and the point `f =
/// 8·fraction − n` of the way along its edge, linear along the square, not by angle (`0x5ec238`).
/// Forward, the dark part runs from that point clockwise back to twelve o'clock; reversed, from
/// twelve o'clock to the point.
pub fn swipe_rim(fraction: f32, reverse: bool) -> Vec<[f32; 2]> {
    let eighths = (fraction * 8.0).max(0.0);
    let n = (eighths as usize).min(8);
    if n >= 8 {
        return if reverse { RIM.to_vec() } else { Vec::new() };
    }
    let f = eighths - n as f32;
    let (a, b) = (RIM[n], RIM[n + 1]);
    let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
    if reverse {
        let mut rim = RIM[..=n].to_vec();
        rim.push(p);
        rim
    } else {
        let mut rim = vec![p];
        rim.extend_from_slice(&RIM[n + 1..]);
        rim
    }
}

/// The corner UVs of a quad spun by `angle` about the texture centre and spanning `span` of it:
/// `0.5 + span · R(angle) · (corner − 0.5)` with `R = [[cos, −sin], [sin, cos]]` (`0x5ec344`,
/// `0x5ec611`), corners top-left, top-right, bottom-right, bottom-left.
pub fn rotated_uvs(angle: f32, span: f32) -> [[f32; 2]; 4] {
    let (sin, cos) = angle.sin_cos();
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| {
        let (du, dv) = (u - 0.5, v - 0.5);
        [
            0.5 + span * (cos * du - sin * dv),
            0.5 + span * (sin * du + cos * dv),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    /// The first octant moves along the top edge from twelve o'clock to the top-right corner, and
    /// the dark part is everything clockwise from there back to twelve.
    #[test]
    fn the_forward_wedge_shrinks_clockwise_from_twelve() {
        let rim = swipe_rim(0.0, false);
        assert_eq!(rim.len(), 9);
        assert!(
            close(rim[0], [0.5, 0.0]) && close(rim[8], [0.5, 0.0]),
            "full square"
        );
        let rim = swipe_rim(1.0 / 16.0, false);
        assert!(
            close(rim[0], [0.75, 0.0]),
            "half-way along the top-right edge: {rim:?}"
        );
        assert!(close(rim[1], [1.0, 0.0]));
        let rim = swipe_rim(0.5, false);
        assert!(
            close(rim[0], [0.5, 1.0]),
            "half-way is six o'clock: {rim:?}"
        );
        assert_eq!(rim.len(), 5);
        assert!(swipe_rim(1.0, false).is_empty());
    }

    /// Reversed, the dark part is what has run: from twelve o'clock to the moving point.
    #[test]
    fn the_reversed_wedge_grows_from_twelve() {
        let rim = swipe_rim(3.0 / 16.0, true);
        assert!(close(rim[0], [0.5, 0.0]));
        assert!(close(rim[1], [1.0, 0.0]));
        assert!(close(*rim.last().unwrap(), [1.0, 0.25]), "{rim:?}");
        assert_eq!(swipe_rim(1.0, true).len(), 9);
    }

    /// A forward sweep runs its duration, flashes for a second, then hides; a reversed one holds.
    #[test]
    fn a_sweep_flashes_then_hides_and_a_reversed_one_holds() {
        let mut c = CooldownState::default();
        assert!(c.paint(1.0).is_none(), "nothing draws before SetCooldown");
        c.set(10_000, 2_000);
        assert_eq!(c.update(11_000), CooldownStep::Running);
        assert_eq!(c.elapsed_ms, 1_000);
        assert_eq!(c.update(12_000), CooldownStep::Running);
        assert!(c.flashing && c.start_ms == 12_000 && c.duration_ms == COOLDOWN_FLASH_MS);
        let flash = c.paint(1.0).unwrap().flash.unwrap();
        assert_eq!(flash.alpha, 0.0, "the flash fades in from 0");
        c.update(12_333);
        let flash = c.paint(1.0).unwrap().flash.unwrap();
        assert!(
            (flash.alpha - 1.0).abs() < 0.01,
            "peak at a third: {}",
            flash.alpha
        );
        assert_eq!(c.update(13_000), CooldownStep::Hide);

        let mut r = CooldownState {
            reverse: true,
            ..Default::default()
        };
        r.set(0, 500);
        assert_eq!(r.update(900), CooldownStep::Running);
        assert!(!r.flashing && r.elapsed_ms == 499);
        assert_eq!(r.paint(1.0).unwrap().swipe.len(), 9, "held nearly full");
    }

    /// A start in the future is done at once: `now − start` wraps (`0x5ec91b`).
    #[test]
    fn a_future_start_counts_as_finished() {
        let mut c = CooldownState::default();
        c.set(5_000, 60_000);
        c.update(4_000);
        assert!(c.flashing);
    }

    /// The edge rides the sweep angle; unrotated, its UVs are the texture's own corners.
    #[test]
    fn the_edge_uvs_rotate_with_the_sweep() {
        let uv = rotated_uvs(0.0, 1.0);
        assert!(close(uv[0], [0.0, 0.0]) && close(uv[2], [1.0, 1.0]));
        let mut c = CooldownState {
            draw_edge: true,
            ..Default::default()
        };
        c.set(0, 4_000);
        c.update(1_000);
        let edge = c.paint(1.0).unwrap().edge.unwrap();
        // A quarter turn back: the top-left corner samples the texture's bottom-left.
        assert!(close(edge[0], [0.0, 1.0]), "{edge:?}");
    }
}
