//! One bolt's beam between two points, the reference's lightning renderer (`0x9a8ec0`-`0x9ab3b0`).

use std::f32::consts::TAU;

use benilla_formats::{chain_flag, ChainEffect};
use benilla_world::particles::buffer::EffectVertex;
use bevy::math::{Vec2, Vec3};

/// The joint cap (`0x9ab320`).
const MAX_JOINTS: usize = 1000;
/// The beam length the joint count and radius see at most (`0x9ab3b0`).
const MAX_LENGTH: f32 = 10000.0;
/// The near-zero test the axes and the wave and arc offsets use (`0x9a9260`, `0x9a9980`).
const AXIS_EPSILON: f32 = 0.000_000_238_418_58;
/// The guard on the joint frame's Z axis divisor (`0x9a9260`).
const Z_AXIS_GUARD: f32 = 0.000_000_01;
/// A cross-section normal at most this long stays unnormalized (`0x9aa210`).
const NORMAL_EPSILON: f32 = 0.001;

/// One joint: its offset about the beam in the joint frame's X and Z, drifting at `vel`.
#[derive(Clone, Copy, Default)]
struct Joint {
    pos: Vec2,
    vel: Vec2,
    /// The last smoothed offset; `None` is the reference's `-1` sentinel, a fresh joint.
    smooth: Option<Vec2>,
    jump: f32,
}

/// The per-frame buffers every beam shares: points and vertex pairs, rebuilt each draw.
#[derive(Default)]
pub(crate) struct Scratch {
    points: Vec<Vec3>,
    pairs: Vec<(Vec3, Vec3)>,
    alphas: Vec<u8>,
}

/// One live bolt (`0x9a97b0`).
pub(super) struct Lightning {
    pub(super) src: Vec3,
    pub(super) dst: Vec3,
    /// Flag `0x2`: drawn and updated (`0x9a9770`).
    pub(super) visible: bool,
    /// Flag `0x1`: the joints exist, so a resize keeps the old ones (`0x9ab2e0`).
    joints_ready: bool,
    /// Flag `0x4`: flickered off (`0x9ab3b0`).
    off: bool,
    /// Flag `0x8`: a pulse running target to caster that has not yet wrapped (`0x9a8ec0`).
    pulse_back: bool,
    joints: Vec<Joint>,
    tex_offset: f32,
    tex_random: f32,
    wave_angle: f32,
    wave_spin: f32,
    wave_phase: f32,
    arc_angle: f32,
    arc_spin: f32,
    flicker_timer: f32,
    pulse_pos: f32,
    rng: u32,
}

/// The beam frame of `y`: a level X square to it and `Z = X × Y / |Y|` (`0x9a9260`), in Bevy axes.
fn xz_axes(y: Vec3) -> (Vec3, Vec3) {
    // WoW's `(Y.y, −Y.x, 0)`, with WoW `x = −bevy.z`, `y = −bevy.x`.
    let (wx, wy) = (-y.z, -y.x);
    let (ax, ay) = if wy.abs() >= AXIS_EPSILON {
        let s = 1.0 / (wy * wy + wx * wx).sqrt();
        (wy * s, -wx * s)
    } else {
        (wy, if wx <= 0.0 { 1.0 } else { -1.0 })
    };
    let x = Vec3::new(-ay, 0.0, -ax);
    let z = x.cross(y) / (y.length() + Z_AXIS_GUARD);
    (x, z)
}

/// The parabola every wave and arc rides, 0 at both ends and 1 at the middle (`0x9a9b30`).
fn envelope(t: f32) -> f32 {
    1.0 - (t - 0.5) * (t - 0.5) * 4.0
}

/// The beam's frame for one draw (`0x9a9980`).
struct BeamFrame {
    x: Vec3,
    z: Vec3,
    wave: Vec3,
    arc: Vec3,
}

impl Lightning {
    /// A bolt from `src` to `dst`, hidden until shown (`0x9a9900`).
    pub(super) fn new(effect: &ChainEffect, src: Vec3, dst: Vec3, seed: u32) -> Self {
        let mut l = Self {
            src,
            dst,
            visible: false,
            joints_ready: false,
            off: false,
            pulse_back: false,
            joints: Vec::new(),
            tex_offset: 0.0,
            tex_random: 0.0,
            wave_angle: 0.0,
            wave_spin: 0.0,
            wave_phase: 0.0,
            arc_angle: 0.0,
            arc_spin: 0.0,
            flicker_timer: 0.0,
            pulse_pos: 0.0,
            rng: seed | 1,
        };
        l.reset(effect);
        l
    }

    /// One draw in `[0, 1)`, built as the reference's from 23 mantissa bits (`0x4c1590`).
    fn ufloat(&mut self) -> f32 {
        // Deviation: xorshift in place of the reference's generator; only the distribution shows.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        f32::from_bits((self.rng & 0x7F_FFFF) | 0x3F80_0000) - 1.0
    }

    /// One draw in `[−1, 1)` (`0x4c15c0`).
    fn sfloat(&mut self) -> f32 {
        let u = self.ufloat();
        if self.rng & 0x8000_0000 == 0 {
            -u
        } else {
            u
        }
    }

    /// `min + (max − min) × u` over one column pair (`0x9a8e80`).
    fn between(&mut self, [min, max]: [f32; 2]) -> f32 {
        (max - min) * self.ufloat() + min
    }

    /// Re-rolls angles, spins, flicker, pulse and texture start, and the joints after (`0x9a8ec0`).
    fn reset(&mut self, e: &ChainEffect) {
        self.joints_ready = false;
        self.off = false;
        self.wave_angle = self.between(e.wave_angle);
        self.wave_spin = self.between(e.wave_spin);
        self.wave_phase = if e.has(chain_flag::FIXED_WAVE_PHASE) {
            e.wave_phase
        } else {
            self.ufloat() * TAU
        };
        self.arc_angle = self.between(e.arc_angle);
        self.arc_spin = self.between(e.arc_spin);
        self.flicker_timer = self.between(e.flicker_on_s);
        self.pulse_pos = 0.0;
        if e.pulse_speed < 0.0 {
            self.pulse_back = true;
        }
        self.tex_random = self.ufloat();
    }

    /// A joint's fresh offset, drift and jump timer (`0x9a90c0`).
    fn randomize_joint(&mut self, e: &ChainEffect, i: usize, radius: f32) {
        let z = self.sfloat();
        let pos = Vec2::new(self.sfloat() * radius, radius * z);
        let speed = self.sfloat();
        let vel = Vec2::new(
            self.sfloat() * e.joint_move_speed,
            e.joint_move_speed * speed,
        );
        let jump = if self.joints_ready {
            self.ufloat() * e.joint_jump_s[1]
        } else {
            self.between(e.joint_jump_s)
        };
        let j = &mut self.joints[i];
        (j.pos, j.vel, j.jump) = (pos, vel, jump);
    }

    /// `trunc(length / avgSegLen) + 3` joints, or column 9 + 1, in `[2, 1000]` (`0x9ab2e0`).
    fn update_joint_count(&mut self, e: &ChainEffect, length: f32, radius: f32) {
        let base = if e.has(chain_flag::FIXED_JOINTS) {
            e.joint_count
        } else {
            let q = length / e.avg_seg_len;
            // `fistp` of a non-finite quotient stores the indefinite integer, low dword 0.
            let whole = if q.is_finite() && q.abs() < 9.0e18 {
                q.trunc() as i64 as i32
            } else {
                0
            };
            whole.wrapping_add(2)
        };
        let count = base.wrapping_add(1).clamp(2, MAX_JOINTS as i32) as usize;
        let old = if self.joints_ready {
            self.joints.len()
        } else {
            0
        };
        self.joints.resize(count, Joint::default());
        for i in old..count {
            self.randomize_joint(e, i, radius);
            self.joints[i].smooth = None;
        }
        self.joints_ready = true;
    }

    /// One tick of `dt` seconds (`0x9ab3b0`).
    pub(super) fn update(&mut self, e: &ChainEffect, dt: f32) {
        if !self.visible {
            return;
        }
        self.flicker_timer -= dt;
        if self.off {
            if self.flicker_timer > 0.0 {
                return;
            }
            self.reset(e);
        } else if e.has(chain_flag::FLICKER) && self.flicker_timer <= 0.0 {
            self.flicker_timer = self.between(e.flicker_off_s);
            self.off = true;
        }
        self.tex_offset = if e.tex_scroll_speed != 0.0 {
            (e.tex_scroll_speed * dt + self.tex_offset) % 1.0
        } else {
            0.0
        };
        let length = self.src.distance(self.dst).min(MAX_LENGTH);
        let radius = e.noise_scale * length + e.joint_offset_radius;
        self.update_joint_count(e, length, radius);
        for i in 1..self.joints.len() {
            self.joints[i].jump -= dt;
            if e.has(chain_flag::JOINT_JUMPS) && self.joints[i].jump < 0.0 {
                self.randomize_joint(e, i, radius);
            } else {
                let j = &mut self.joints[i];
                j.pos += j.vel * dt;
            }
        }
        self.wave_angle += self.wave_spin * dt;
        self.wave_phase -= e.wave_speed * dt;
        self.arc_angle += self.arc_spin * dt;
        if !e.has(chain_flag::PULSE) {
            return;
        }
        self.pulse_pos += e.pulse_speed * dt;
        if !self.pulse_back && !e.has(chain_flag::PULSE_LOOPS) {
            return;
        }
        let pulse_len = e.pulse_fade_length * 2.0 + e.pulse_on_length;
        if self.pulse_pos - pulse_len >= length {
            self.reset(e);
            self.pulse_back = false;
        } else if self.pulse_pos < 0.0 {
            self.reset(e);
            self.pulse_back = false;
            self.pulse_pos = pulse_len + length;
        }
    }

    /// The beam axes and this frame's wave and arc offsets (`0x9a9980`).
    fn frame(&self, e: &ChainEffect) -> BeamFrame {
        let (x, z) = xz_axes(self.dst - self.src);
        let swing = |angle: f32, height: f32| (x * angle.sin() + z * angle.cos()) * height;
        let wave = if e.wave_height.abs() >= AXIS_EPSILON && !e.has(chain_flag::SPIRAL_WAVE) {
            swing(self.wave_angle, e.wave_height)
        } else {
            Vec3::ZERO
        };
        let arc = if e.arc_height.abs() >= AXIS_EPSILON {
            swing(self.arc_angle, e.arc_height)
        } else {
            Vec3::ZERO
        };
        BeamFrame { x, z, wave, arc }
    }

    /// A joint's point: its smoothed offset in the joint frame, `t` along `y` (`0x9a9350`).
    fn build_point(
        joint: &mut Joint,
        e: &ChainEffect,
        base: Vec3,
        t: f32,
        (x, y, z): (Vec3, Vec3, Vec3),
        scale: f32,
    ) -> Vec3 {
        let smooth = match joint.smooth {
            None => joint.pos,
            Some(prev) => prev * e.joint_smoothness + joint.pos * (1.0 - e.joint_smoothness),
        };
        joint.smooth = Some(smooth);
        (x * smooth.x + z * smooth.y) * scale + y * t + base
    }

    /// Every joint about the straight line (`0x9a9ca0`).
    fn build_points(&mut self, e: &ChainEffect, f: &BeamFrame, points: &mut [Vec3]) {
        let last = points.len() - 1;
        let y = self.dst - self.src;
        for (i, p) in points.iter_mut().enumerate().take(last).skip(1) {
            let t = i as f32 * (1.0 / last as f32);
            *p = Self::build_point(&mut self.joints[i], e, self.src, t, (f.x, y, f.z), 1.0);
        }
    }

    /// Major joints about the line, minor ones about the majors, the rest about those (`0x9a9dc0`).
    fn build_points_hierarchy(&mut self, e: &ChainEffect, points: &mut [Vec3]) {
        let last = points.len() - 1;
        // The reference never terminates on a zero step; no shipped hierarchy row has one.
        let minor = e.joints_per_minor_joint.max(1) as usize;
        let major = (minor * e.minor_joints_per_major_joint.max(1) as usize).max(1);
        let axes = |y: Vec3| {
            let (x, z) = xz_axes(y);
            (x, y, z)
        };
        let frame = axes(self.dst - self.src);
        let major_t = 1.0 / last as f32;
        for i in (major..last).step_by(major) {
            let t = i as f32 * major_t;
            points[i] = Self::build_point(
                &mut self.joints[i],
                e,
                points[0],
                t,
                frame,
                e.major_joint_scale,
            );
        }
        let mut next = major.min(last);
        let mut base = points[0];
        let mut frame = axes(points[next] - base);
        let mut step = minor as f32 / next as f32;
        let mut t = step;
        if minor < last {
            for i in (minor..last).step_by(minor) {
                if i == next {
                    next = (i + major).min(last);
                    base = points[i];
                    frame = axes(points[next] - base);
                    step = minor as f32 / (next - i) as f32;
                    t = step;
                } else {
                    points[i] = Self::build_point(
                        &mut self.joints[i],
                        e,
                        base,
                        t,
                        frame,
                        e.minor_joint_scale,
                    );
                    t += step;
                }
            }
            next = minor;
        } else {
            next = last;
        }
        base = points[0];
        frame = axes(points[next] - base);
        step = 1.0 / next as f32;
        t = step;
        for i in 1..last {
            if i == next {
                next = (i + minor).min(last);
                base = points[i];
                frame = axes(points[next] - base);
                step = 1.0 / (next - i) as f32;
                t = step;
            } else {
                points[i] = Self::build_point(&mut self.joints[i], e, base, t, frame, 1.0);
                t += step;
            }
        }
    }

    /// The wave's displacement at `t` along a beam `length` long (`0x9a9b30`).
    fn wave_at(&self, e: &ChainEffect, f: &BeamFrame, t: f32, length: f32) -> Vec3 {
        let env = envelope(t);
        let angle = t * e.wave_freq * length + self.wave_phase;
        if e.has(chain_flag::SPIRAL_WAVE) {
            let h = env * e.wave_height;
            f.z * (angle.sin() * h) + f.x * (angle.cos() * h)
        } else {
            f.wave * (env * angle.sin())
        }
    }

    /// The pulse's alpha per pair and the drawn strip `[start, start + count)` (`0x9a9490`).
    fn pulse(&self, e: &ChainEffect, length: f32, alphas: &mut [u8]) -> (usize, usize) {
        let pairs = alphas.len();
        let a = e.rgba[3];
        if !e.has(chain_flag::PULSE) {
            alphas.fill(a);
            return (0, pairs * 2);
        }
        let step = length / (pairs - 1) as f32;
        let fade_out_end = self.pulse_pos - e.pulse_fade_length;
        let fade_in_end = fade_out_end - e.pulse_on_length;
        let fade_in_start = fade_in_end - e.pulse_fade_length;
        let fade_in_scale = 255.0 / (fade_in_end - fade_in_start);
        let fade_out_scale = 255.0 / (self.pulse_pos - fade_out_end);
        let alpha_scale = a as f32 * (1.0 / 255.0);
        let fint = |v: f32| v.round_ties_even() as i32 as u8;
        let mut start = 0;
        let mut pos = 0.0f32;
        for (k, slot) in alphas.iter_mut().enumerate() {
            let i = k * 2;
            let alpha = if pos < fade_in_start {
                start = i;
                0
            } else if pos < fade_in_end {
                fint(alpha_scale * (pos - fade_in_start) * fade_in_scale)
            } else if fade_out_end > pos {
                a
            } else if pos >= self.pulse_pos {
                *slot = 0;
                return (start, i + 1 - start);
            } else {
                fint(alpha_scale * (self.pulse_pos - pos) * fade_out_scale)
            };
            *slot = alpha;
            pos += step;
        }
        (start, pairs * 2 - start)
    }

    /// This frame's ribbon facing `cam` as quads, the texture matrix folded into `u` (`0x9aa210`).
    pub(super) fn render(
        &mut self,
        e: &ChainEffect,
        cam: Vec3,
        scratch: &mut Scratch,
        out: &mut Vec<EffectVertex>,
    ) {
        if !self.visible || self.off || self.joints.len() < 2 {
            return;
        }
        let n = self.joints.len();
        let Scratch {
            points,
            pairs,
            alphas,
        } = scratch;
        points.clear();
        points.resize(n, Vec3::ZERO);
        points[0] = self.src;
        points[n - 1] = self.dst;
        let f = self.frame(e);
        if e.has(chain_flag::HIERARCHY) {
            self.build_points_hierarchy(e, points);
        } else {
            self.build_points(e, &f, points);
        }

        let length = self.src.distance(self.dst);
        let width = e.half_width;
        let step = 1.0 / (n - 1) as f32;
        let wave = e.wave_height != 0.0;
        let arc = e.arc_height != 0.0;
        let shape = |p: Vec3, t: f32| {
            let mut p = p;
            if wave {
                p += self.wave_at(e, &f, t, length);
            }
            if arc {
                p += f.arc * envelope(t);
            }
            p
        };
        pairs.clear();
        pairs.resize(n, (Vec3::ZERO, Vec3::ZERO));
        if !e.has(chain_flag::FLAT) {
            // Each interior pair averages the offsets of the two segments it joins.
            let mut prev = points[0];
            let mut t = step;
            for k in 1..n {
                let point = shape(points[k], t);
                let view = (point + prev) * 0.5 - cam;
                let normal = (point - prev).cross(view);
                let normal = if normal.length() > NORMAL_EPSILON {
                    normal.normalize()
                } else {
                    normal
                };
                let off = normal * width;
                pairs[k - 1].0 += (prev + off) * 0.5;
                pairs[k - 1].1 += (prev - off) * 0.5;
                pairs[k] = ((point + off) * 0.5, (point - off) * 0.5);
                prev = point;
                t += step;
            }
        } else {
            let view = (self.dst + self.src) * 0.5 - cam;
            let normal = (self.dst - self.src).cross(view);
            let normal = if normal.length() > NORMAL_EPSILON {
                normal.normalize()
            } else {
                normal
            };
            let off = normal * width;
            let mut t = 0.0;
            for (pair, &p) in pairs.iter_mut().zip(points.iter()) {
                let point = shape(p, t);
                *pair = (point + off, point - off);
                t += step;
            }
        }
        pairs[0] = (points[0], points[0]);
        pairs[n - 1] = (points[n - 1], points[n - 1]);

        alphas.clear();
        alphas.resize(n, 0);
        let (start, count) = self.pulse(e, length, alphas);
        // Deviation: no CPU frustum test (`0x9aa781`), as the GPU clips a beam for the same result.
        if count <= 2 {
            return;
        }

        let tiled = e.has(chain_flag::TILE_BY_LENGTH);
        let tex_scale = if tiled {
            length / e.texture_length
        } else {
            1.0
        };
        let mut tex_offset = self.tex_random + self.tex_offset;
        if tiled && e.has(chain_flag::TILE_OFFSET) {
            tex_offset += length / e.texture_length;
        }
        let [r, g, b, _] = e.rgba.map(|c| c as f32 / 255.0);
        let vertex = |k: usize, top: bool| {
            let (pos, v) = match (k == 0 || k == n - 1, top) {
                (true, _) => (pairs[k].0, 0.5),
                (false, true) => (pairs[k].0, 0.0),
                (false, false) => (pairs[k].1, 1.0),
            };
            EffectVertex {
                pos: pos.to_array(),
                uv: [k as f32 * step * tex_scale - tex_offset, v],
                color: [r, g, b, alphas[k] as f32 / 255.0],
            }
        };
        // The strip as quads `[b0, b1, t1, t0]`; an odd count ends on one triangle (`0x9aae66`).
        let end = start + count;
        let first = start / 2;
        let full = end / 2;
        for k in first..full.saturating_sub(1) {
            out.extend([
                vertex(k, false),
                vertex(k + 1, false),
                vertex(k + 1, true),
                vertex(k, true),
            ]);
        }
        if end % 2 == 1 && full >= 1 && full < n {
            let k = full - 1;
            out.extend([
                vertex(k, false),
                vertex(k + 1, true),
                vertex(k + 1, true),
                vertex(k, true),
            ]);
        }
    }

    /// The joint count, for tests.
    #[cfg(test)]
    pub(super) fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// Whether the bolt is flickered off, for tests.
    #[cfg(test)]
    pub(super) fn is_off(&self) -> bool {
        self.off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight, plain row: two-yard segments, no joint offset, no wave, arc, flicker or pulse.
    fn plain() -> ChainEffect {
        ChainEffect {
            avg_seg_len: 2.0,
            half_width: 0.5,
            texture_length: 2.0,
            rgba: [255, 128, 64, 200],
            blend_mode: 3,
            ..Default::default()
        }
    }

    fn shown(e: &ChainEffect, src: Vec3, dst: Vec3) -> Lightning {
        let mut l = Lightning::new(e, src, dst, 7);
        l.visible = true;
        l.update(e, 0.0);
        l
    }

    fn draw(l: &mut Lightning, e: &ChainEffect, cam: Vec3) -> Vec<EffectVertex> {
        let mut out = Vec::new();
        l.render(e, cam, &mut Scratch::default(), &mut out);
        out
    }

    #[test]
    fn the_joint_count_is_length_over_segment_plus_three_capped_at_1000() {
        let e = plain();
        for (len, want) in [(0.0, 3), (1.9, 3), (2.0, 4), (10.0, 8)] {
            let l = shown(&e, Vec3::ZERO, Vec3::X * len);
            assert_eq!(l.joint_count(), want, "{len} yd");
        }
        let fine = ChainEffect {
            avg_seg_len: 0.001,
            ..plain()
        };
        assert_eq!(shown(&fine, Vec3::ZERO, Vec3::X * 50.0).joint_count(), 1000);
        let fixed = ChainEffect {
            flags: chain_flag::FIXED_JOINTS,
            joint_count: 27,
            ..plain()
        };
        assert_eq!(shown(&fixed, Vec3::ZERO, Vec3::X * 3.0).joint_count(), 28);
        let zero = ChainEffect {
            avg_seg_len: 0.0,
            ..plain()
        };
        assert_eq!(
            shown(&zero, Vec3::ZERO, Vec3::X * 5.0).joint_count(),
            3,
            "an infinite quotient stores 0"
        );
    }

    #[test]
    fn a_plain_beam_is_a_straight_spindle_of_quads_in_the_row_colour() {
        let e = plain();
        let mut l = shown(&e, Vec3::ZERO, Vec3::X * 10.0);
        let v = draw(&mut l, &e, Vec3::new(5.0, 0.0, 10.0));
        let n = l.joint_count();
        assert_eq!(v.len(), (n - 1) * 4, "one quad per segment");
        // The ends pinch to the endpoints at v = 0.5.
        assert_eq!(v[0].pos, [0.0, 0.0, 0.0]);
        assert_eq!(v[3].pos, [0.0, 0.0, 0.0]);
        assert_eq!((v[0].uv[1], v[3].uv[1]), (0.5, 0.5));
        let last = v.len() - 4;
        assert_eq!(v[last + 1].pos, [10.0, 0.0, 0.0]);
        // An interior pair spans 2 × half-width square to the view.
        let (bottom, top) = (Vec3::from(v[1].pos), Vec3::from(v[2].pos));
        assert!((top.distance(bottom) - 1.0).abs() < 1e-4, "{top} {bottom}");
        assert!((v[2].uv[1], v[1].uv[1]) == (0.0, 1.0));
        let c = v[1].color;
        assert!((c[0] - 1.0).abs() < 1e-6 && (c[1] - 128.0 / 255.0).abs() < 1e-6);
        assert!((c[3] - 200.0 / 255.0).abs() < 1e-6, "the row alpha");
    }

    #[test]
    fn u_tiles_by_length_and_slides_with_the_scroll() {
        let e = ChainEffect {
            flags: chain_flag::TILE_BY_LENGTH,
            texture_length: 5.0,
            tex_scroll_speed: 0.25,
            ..plain()
        };
        let mut l = shown(&e, Vec3::ZERO, Vec3::X * 10.0);
        let base = l.tex_random;
        l.update(&e, 1.0);
        let v = draw(&mut l, &e, Vec3::new(5.0, 0.0, 10.0));
        let last = v.len() - 4;
        let du = v[last + 1].uv[0] - v[0].uv[0];
        assert!((du - 2.0).abs() < 1e-4, "10 yd over 5-yd tiles: {du}");
        let want = -(base + 0.25);
        assert!((v[0].uv[0] - want).abs() < 1e-5, "{} vs {want}", v[0].uv[0]);
    }

    #[test]
    fn the_scroll_wraps_on_one_keeping_its_sign() {
        let e = ChainEffect {
            tex_scroll_speed: -2.0,
            ..plain()
        };
        let mut l = shown(&e, Vec3::ZERO, Vec3::X * 10.0);
        l.update(&e, 0.3);
        assert!((l.tex_offset + 0.6).abs() < 1e-5);
        l.update(&e, 0.3);
        assert!((l.tex_offset + 0.2).abs() < 1e-5, "{}", l.tex_offset);
    }

    #[test]
    fn a_flickering_beam_goes_dark_for_its_off_time() {
        let e = ChainEffect {
            flags: chain_flag::FLICKER,
            flicker_on_s: [0.1, 0.1],
            flicker_off_s: [0.5, 0.5],
            ..plain()
        };
        let mut l = shown(&e, Vec3::ZERO, Vec3::X * 10.0);
        l.update(&e, 0.2);
        assert!(l.is_off());
        assert!(
            draw(&mut l, &e, Vec3::Z * 10.0).is_empty(),
            "off draws nothing"
        );
        l.update(&e, 0.3);
        assert!(l.is_off(), "still within the off time");
        l.update(&e, 0.3);
        assert!(!l.is_off(), "back on after it");
    }

    #[test]
    fn a_pulse_lights_only_its_window_and_a_loop_wraps() {
        let e = ChainEffect {
            flags: chain_flag::PULSE | chain_flag::PULSE_LOOPS,
            pulse_speed: 10.0,
            pulse_on_length: 2.0,
            pulse_fade_length: 1.0,
            avg_seg_len: 0.5,
            ..plain()
        };
        let mut l = shown(&e, Vec3::ZERO, Vec3::X * 20.0);
        l.update(&e, 1.0);
        let v = draw(&mut l, &e, Vec3::new(10.0, 0.0, 10.0));
        assert!(!v.is_empty());
        // The lit span runs from pulse − 4 yd to the head at 10 yd.
        let xs: Vec<f32> = v.iter().map(|x| x.pos[0]).collect();
        let (lo, hi) = xs
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(lo >= 5.0 && hi <= 10.6, "drawn {lo}..{hi}");
        assert!(v.iter().any(|x| (x.color[3] - 200.0 / 255.0).abs() < 1e-6));
        // Past the far end by its own length, it restarts from the caster.
        l.update(&e, 1.5);
        assert!(l.pulse_pos < 1.0, "wrapped to {}", l.pulse_pos);
    }

    #[test]
    fn a_negative_pulse_starts_at_the_target_end() {
        let e = ChainEffect {
            flags: chain_flag::PULSE,
            pulse_speed: -10.0,
            pulse_on_length: 2.0,
            pulse_fade_length: 1.0,
            ..plain()
        };
        let mut l = shown(&e, Vec3::ZERO, Vec3::X * 20.0);
        l.update(&e, 0.01);
        assert!(
            (l.pulse_pos - 24.0).abs() < 1e-4,
            "length + 2 × fade + on, from the first tick's wrap: {}",
            l.pulse_pos
        );
    }

    #[test]
    fn jumping_joints_stay_within_the_radius_about_the_line() {
        let e = ChainEffect {
            flags: chain_flag::JOINT_JUMPS,
            joint_offset_radius: 0.3,
            avg_seg_len: 1.0,
            ..plain()
        };
        let (a, b) = (Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0));
        let mut l = shown(&e, a, b);
        let mut s = Scratch::default();
        for _ in 0..20 {
            l.update(&e, 0.016);
            let mut out = Vec::new();
            l.render(&e, Vec3::new(5.0, 2.0, -5.0), &mut s, &mut out);
            for p in &s.points {
                let off = Vec2::new(p.x, p.y).length();
                assert!(off <= 0.3 * 2f32.sqrt() + 1e-4, "{p}");
            }
        }
    }

    #[test]
    fn the_hierarchy_scales_major_joints_and_keeps_the_ends() {
        let e = ChainEffect {
            flags: chain_flag::HIERARCHY,
            joint_offset_radius: 0.1,
            joints_per_minor_joint: 3,
            minor_joints_per_major_joint: 4,
            minor_joint_scale: 3.7,
            major_joint_scale: 8.45,
            avg_seg_len: 0.334,
            ..plain()
        };
        let (a, b) = (Vec3::ZERO, Vec3::X * 30.0);
        let mut l = shown(&e, a, b);
        let mut s = Scratch::default();
        let mut out = Vec::new();
        l.render(&e, Vec3::new(15.0, 0.0, 20.0), &mut s, &mut out);
        let n = s.points.len();
        assert_eq!((s.points[0], s.points[n - 1]), (a, b));
        assert!(s.points.iter().all(|p| p.is_finite()));
        // A point strays at most its major, minor and own offsets from the line.
        let bound = 0.1 * 2f32.sqrt() * (8.45 + 3.7 + 1.0) + 1e-3;
        assert!(s
            .points
            .iter()
            .all(|p| Vec2::new(p.y, p.z).length() <= bound));
        assert!(!out.is_empty());
    }

    #[test]
    fn the_axes_are_square_to_the_beam_and_level() {
        for y in [
            Vec3::X,
            Vec3::NEG_Z,
            Vec3::new(3.0, 1.0, -2.0),
            Vec3::Y * 4.0,
        ] {
            let (x, z) = xz_axes(y);
            assert!(x.dot(y).abs() < 1e-5, "{y}");
            assert!(x.y.abs() < 1e-6, "X is level");
            assert!(z.dot(y).abs() < 1e-4 && z.dot(x).abs() < 1e-5);
            assert!((x.length() - 1.0).abs() < 1e-5 && (z.length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn an_arc_bows_the_middle_and_spares_the_ends() {
        let e = ChainEffect {
            arc_height: 2.0,
            ..plain()
        };
        let mut l = shown(&e, Vec3::ZERO, Vec3::X * 10.0);
        let v = draw(&mut l, &e, Vec3::new(5.0, 0.0, 20.0));
        let mid = v.len() / 8 * 4;
        let p = (Vec3::from(v[mid].pos) + Vec3::from(v[mid + 3].pos)) * 0.5;
        assert!(p.y.abs() > 1.0, "bowed: {p}");
        assert_eq!(v[0].pos, [0.0, 0.0, 0.0]);
    }
}
