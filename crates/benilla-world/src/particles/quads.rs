//! Quad expansion: one pool of live particles into camera-facing (or XY-plane) quads, the
//! reference's quad writer (`0x97be80`), shared by child emitters.

use benilla_assets::coords::wow_to_bevy;
use benilla_formats::ParticleEmitterDef;
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::buffer::{
    EffectVertex, GpuParticle, GpuPool, GPU_DRAWS_HEAD, GPU_DRAWS_TAIL, GPU_RANDOM_SPIN_SIGN,
    GPU_ROTATES, GPU_TAIL_CLAMPS, GPU_TRACK_KEYS,
};
use super::{rand01, Particle};

/// The 128-entry twinkle noise table, uniform in [0, 1) like the reference's table; a fixed seed,
/// so the distribution matches but not the stream.
static TWINKLE_LUT: std::sync::LazyLock<[f32; 128]> = std::sync::LazyLock::new(|| {
    let mut s = 0xC0FF_EE11u32;
    let mut t = [0.0f32; 128];
    for v in &mut t {
        *v = rand01(&mut s);
    }
    t
});

/// Index `(floor(clamp(twinkleSpeed · age, 0, 255)) + phase) & 0x7f`; the reference's phase is a
/// pointer hash, ours a spawn-time random ([`Particle::phase`]).
fn twinkle_noise(twinkle_speed: f32, age: f32, phase: u32) -> f32 {
    let idx = ((twinkle_speed * age).clamp(0.0, 255.0) as u32).wrapping_add(phase) as usize & 0x7f;
    TWINKLE_LUT[idx]
}

/// `base + spin·age`, negated on a random-sign emitter (file flag 0x200) for a particle whose pool
/// slot has address bit 5 set (`0x97c594`), so half its cloud counter-rotates. Our bit 5 is
/// [`Particle::phase`]'s, stable per particle where a pool index would not be.
pub(super) fn spin_angle(base: f32, spin: f32, age: f32, random_sign: bool, phase: u32) -> f32 {
    let angle = spin * age + base;
    if random_sign && phase & 0x20 != 0 {
        -angle
    } else {
        angle
    }
}

/// The camera basis one frame of expansion billboards against.
pub(super) struct CamBasis {
    pub(crate) right: Vec3,
    pub(crate) up: Vec3,
}

/// How a cloud's stored coordinates reach the world (the two modes on [`super::Particle`]).
pub(super) struct DrawFrame {
    pub(crate) anchored: bool,
    /// World mode's ride frame `A`, the reference's `[ebp+8]`, folded on the world-mode leg; the
    /// identity off a transport. Model mode never reads it.
    pub(crate) ride: crate::ride_frame::StoredFrame,
    /// The owning model's render alpha, folded into every particle's alpha (`emitter+0x1a8`).
    pub(crate) alpha: f32,
    /// One model unit of half-extent in the stored frame: 1.0 in the world, a UI tile's pixels per
    /// unit (the reference's `768·√(a²+1)` FrameXML units, instance scale excluded).
    pub(crate) size_scale: f32,
}

/// One particle's world-space quad centre, shared by [`expand_quads`] and the depth dump.
pub(super) fn particle_center(frame: &DrawFrame, placement: &Transform, p: &Particle) -> Vec3 {
    if frame.anchored {
        // World mode (`0x10` clear): no `rt+0x1fc` fold, only the ride frame, `A · T · S`; off a
        // transport `A` is null, the store world.
        frame.ride.to_world(p.pos)
    } else {
        // Model mode (`0x10` set): the live emitter matrix, re-applied every frame.
        placement.transform_point(wow_to_bevy([p.pos.x, p.pos.y, p.pos.z]))
    }
}

/// The twinklePercent draw gate; must match [`expand_quads`]'s skip.
pub(super) fn draw_gated(def: &ParticleEmitterDef, p: &Particle) -> bool {
    let noise = twinkle_noise(def.twinkle_speed, p.age, p.phase);
    def.twinkle_percent < 1.0 && noise > def.twinkle_percent
}

/// One particle's rendered half-extents, x and y; must match the `half` term in [`expand_quads`].
pub(super) fn particle_half(
    def: &ParticleEmitterDef,
    placement: &Transform,
    p: &Particle,
    size_scale: f32,
) -> [f32; 2] {
    let noise = twinkle_noise(def.twinkle_speed, p.age, p.phase);
    let u_age = (p.age / p.life).clamp(0.0, 1.0);
    let scale = if def.scale_size_by_instance() {
        placement.scale.x.max(1e-4)
    } else {
        size_scale
    };
    let size = def.over_life.sample(u_age).size;
    let k = def.twinkle(noise) * scale;
    [size[0] * p.size_mul[0] * k, size[1] * p.size_mul[1] * k]
}

/// Expand one pool into the shared stream: world-space quads, corners in perimeter order for the
/// lane's `[0,1,2, 0,2,3]` indices.
pub(super) fn expand_quads(
    def: &ParticleEmitterDef,
    particles: &[Particle],
    frame: &DrawFrame,
    placement: &Transform,
    cam: &CamBasis,
    out: &mut Vec<EffectVertex>,
) {
    let anchored = frame.anchored;
    let (cam_right, cam_up) = (cam.right, cam.up);
    // Size takes the instance scale only when flagged, else the lane's size unit.
    let scale = if def.scale_size_by_instance() {
        placement.scale.x.max(1e-4)
    } else {
        frame.size_scale
    };
    // XY quads (file flag 0x1000) lie flat in the emitter's XY plane under the live model matrix,
    // its scale included, in both modes. The basis carries the emitter frame's R(+Z, 90°) itself,
    // since births take it at emission: X̂ to Ŷ, Ŷ to −X̂.
    let plane_basis = def.xy_quad().then(|| {
        let s = placement.scale.x.max(1e-4);
        (
            placement.rotation * (wow_to_bevy([0.0, 1.0, 0.0]) * s),
            placement.rotation * (wow_to_bevy([-1.0, 0.0, 0.0]) * s),
        )
    });
    // Atlas cell: `col = idx & (cols − 1)`, `row = idx >> log2(cols)`, cols a power of two. The row
    // is never clamped: past the last cell V ≥ 1 and the repeat sampler wraps to row 0, which 553
    // emitters reach at a flipbook's tail.
    let (cols, rows) = (def.tile_cols, def.tile_rows);
    let (inv_cols, inv_rows) = (1.0 / cols as f32, 1.0 / rows as f32);
    let cell_uv = |idx: u16| {
        let cx = f32::from(idx & (cols - 1));
        let cy = f32::from(idx >> cols.trailing_zeros());
        (
            (cx * inv_cols, (cx + 1.0) * inv_cols),
            (cy * inv_rows, (cy + 1.0) * inv_rows),
        )
    };
    for p in particles {
        let noise = twinkle_noise(def.twinkle_speed, p.age, p.phase);
        // The twinklePercent gate: below 1, a sample above it draws no quad.
        if def.twinkle_percent < 1.0 && noise > def.twinkle_percent {
            continue;
        }
        let u_age = (p.age / p.life).clamp(0.0, 1.0);
        let ol = def.over_life.sample(u_age);
        let mut rgba = ol.color;
        // The model's render alpha scales alpha alone, never RGB.
        rgba[3] *= frame.alpha;
        // Raw authored gamma RGB: additive stacks sum in gamma like the reference's bytes, and
        // the FFX chain's scene reads apply their 255 saturation (`ffx_glow.wgsl`), not this.
        let center = particle_center(frame, placement, p);
        // The half-extents per axis (corners ±1.0), times the particle's size roll, the gated
        // twinkle and the flagged instance scale (`0x97bf88`); a twinkle range with min == max
        // burns steady.
        let k = def.twinkle(noise) * scale;
        let (half_x, half_y) = (
            ol.size[0] * p.size_mul[0] * k,
            ol.size[1] * p.size_mul[1] * k,
        );
        // Deviation: quads keep pool order within a cloud, without the reference's back-to-front
        // sort, because only an alpha-blended cloud can show the order; between clouds, each draw
        // record's anchor sorts it.
        let mut push_quad = |corners: [Vec3; 4], quv: [[f32; 2]; 4]| {
            for (c, t) in corners.iter().zip(quv) {
                out.push(EffectVertex {
                    pos: c.to_array(),
                    uv: t,
                    color: rgba,
                });
            }
        };
        // Head quad (file flag 0x20000), drawn first (`0x97bfe8`): a billboard, or the XY plane.
        if def.draws_head() {
            let ((u0, u1), (v0, v1)) = cell_uv(ol.head_cell.unwrap_or(p.cell));
            let (base_r, base_u) = plane_basis.unwrap_or((cam_right, cam_up));
            // Rotation by [`spin_angle`], only while the spin or its variance is non-zero
            // (`0x97c282`); on an XY quad this is the reference's Rodrigues about the plane
            // normal, as `normal × base_r = base_u`.
            let (r, u) = if def.rotates() {
                let angle = spin_angle(p.base_spin, p.spin, p.age, def.random_spin_sign(), p.phase);
                let (sa, ca) = angle.sin_cos();
                (
                    (base_r * ca + base_u * sa) * half_x,
                    (base_u * ca - base_r * sa) * half_y,
                )
            } else {
                (base_r * half_x, base_u * half_y)
            };
            push_quad(
                [
                    center - r - u,
                    center + r - u,
                    center + r + u,
                    center - r + u,
                ],
                [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
            );
        }
        // Tail quad (file flag 0x40000): |velocity|·tailTime back along the motion, 2·half wide in
        // screen space, U from 0 at the particle to 1 at the tip, a billboard when the velocity is
        // view-parallel; its own flipbook cell (file +0x14c). Its width takes the x extent.
        if def.draws_tail() {
            let half = half_x;
            let ((u0, u1), (v0, v1)) = cell_uv(ol.tail_cell);
            let vel_world = if anchored {
                frame.ride.dir_to_world(p.vel)
            } else {
                placement.rotation * (placement.scale * wow_to_bevy(p.vel.to_array()))
            };
            let t_eff = if def.tail_clamps_to_age() {
                def.tail_time.min(p.age)
            } else {
                def.tail_time
            };
            let tail = -vel_world * t_eff;
            let (tr, tu) = (tail.dot(cam_right), tail.dot(cam_up));
            let l2 = tr * tr + tu * tu;
            if l2 < 7.7e-4 {
                // Degenerate: the reference's plain-billboard fallback.
                let (r, u) = (cam_right * half, cam_up * half);
                push_quad(
                    [
                        center - r - u,
                        center + r - u,
                        center + r + u,
                        center - r + u,
                    ],
                    [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
                );
            } else {
                let inv_l = half / l2.sqrt();
                let perp = (cam_up * tr - cam_right * tu) * inv_l;
                push_quad(
                    [
                        center - perp,
                        center + perp,
                        center + tail + perp,
                        center + tail - perp,
                    ],
                    [[u0, v1], [u0, v0], [u1, v0], [u1, v1]],
                );
            }
        }
    }
}

/// The twinkle table the GPU expansion samples, the same values [`twinkle_noise`] reads.
pub(crate) fn twinkle_table() -> &'static [f32; 128] {
    &TWINKLE_LUT
}

/// One particle for the GPU expansion; its pool row is set at commit.
pub(super) fn gpu_particle(p: &Particle) -> GpuParticle {
    GpuParticle {
        pos: p.pos.to_array(),
        age: p.age,
        vel: p.vel.to_array(),
        life: p.life,
        size_mul: p.size_mul,
        base_spin: p.base_spin,
        spin: p.spin,
        phase: p.phase,
        cell: u32::from(p.cell),
        pool: 0,
        _pad: 0,
    }
}

/// A track's keys for the GPU: `(count, times, values)`, `None` when it needs the CPU sampler
/// (more than [`GPU_TRACK_KEYS`] keys, or times and values of different lengths past one key).
fn gpu_track<V: Copy + Default, const N: usize>(
    track: &benilla_formats::LifeTrack<V>,
) -> Option<(u32, [f32; N], [V; N])> {
    let n = track.values.len();
    if n > N || (n > 1 && track.times.len() != n) {
        return None;
    }
    let mut times = [0.0; N];
    let mut values = [V::default(); N];
    for i in 0..n {
        times[i] = track.times.get(i).copied().unwrap_or(0.0);
        values[i] = track.values[i];
    }
    Some((n as u32, times, values))
}

/// The def's half of a pool's [`GpuPool`], built once per emitter: the over-life tracks, flags,
/// atlas grid and twinkle terms. `None` when a track is past the GPU sampler or the atlas is not a
/// power-of-two grid, and the pool takes the CPU writer.
pub(super) fn pool_template(def: &ParticleEmitterDef) -> Option<GpuPool> {
    let ol = &def.over_life;
    let (cn, ct, cv) = gpu_track::<[f32; 3], GPU_TRACK_KEYS>(&ol.color)?;
    let (an, at, av) = gpu_track::<f32, GPU_TRACK_KEYS>(&ol.alpha)?;
    let (sn, st, sv) = gpu_track::<[f32; 2], GPU_TRACK_KEYS>(&ol.scale)?;
    let (hn, ht, hv) = gpu_track::<u16, GPU_TRACK_KEYS>(&ol.head_cell)?;
    let (tn, tt, tv) = gpu_track::<u16, GPU_TRACK_KEYS>(&ol.tail_cell)?;
    if !def.tile_cols.is_power_of_two() {
        return None;
    }
    // An unkeyed cell channel draws the particle's own cell (head) or cell 0 (tail): times empty.
    let hn = if ol.head_cell.times.is_empty() { 0 } else { hn };
    let tn = if ol.tail_cell.times.is_empty() { 0 } else { tn };
    let flag = |on: bool, bit: u32| if on { bit } else { 0 };
    let flags = flag(def.draws_head(), GPU_DRAWS_HEAD)
        | flag(def.draws_tail(), GPU_DRAWS_TAIL)
        | flag(def.rotates(), GPU_ROTATES)
        | flag(def.random_spin_sign(), GPU_RANDOM_SPIN_SIGN)
        | flag(def.tail_clamps_to_age(), GPU_TAIL_CLAMPS);
    Some(GpuPool {
        color_t: ct,
        color_v: cv.map(|c| [c[0], c[1], c[2], 0.0]),
        alpha_t: at,
        alpha_v: av,
        scale_t: st,
        scale_v: [
            [sv[0][0], sv[0][1], sv[1][0], sv[1][1]],
            [sv[2][0], sv[2][1], sv[3][0], sv[3][1]],
        ],
        head_t: ht,
        head_v: hv.map(f32::from),
        tail_t: tt,
        tail_v: tv.map(f32::from),
        counts: [cn, an, sn, hn],
        counts2: [
            tn,
            flags,
            u32::from(def.tile_cols),
            u32::from(def.tile_rows),
        ],
        params: [0.0, 0.0, def.twinkle_speed, def.twinkle_percent],
        params2: [def.twinkle_min, def.twinkle_max, def.tail_time, 0.0],
        ..GpuPool::default()
    })
}

/// This frame's [`GpuPool`] from its [`pool_template`]: what [`expand_quads`] reads of the draw
/// frame, the placement and the camera, plus the committed light the CPU path folds after
/// expanding.
pub(super) fn pool_record(
    template: &GpuPool,
    def: &ParticleEmitterDef,
    frame: &DrawFrame,
    placement: &Transform,
    cam: &CamBasis,
    committed: Option<[f32; 3]>,
) -> GpuPool {
    // World mode draws through the ride frame, model mode through the live placement on the
    // WoW-to-Bevy axes (`particle_center`); a tail's velocity takes the same linear part.
    let mat = if frame.anchored {
        frame.ride.matrix().unwrap_or(Affine3A::IDENTITY)
    } else {
        let axes = Affine3A::from_mat3(Mat3::from_cols(
            wow_to_bevy([1.0, 0.0, 0.0]),
            wow_to_bevy([0.0, 1.0, 0.0]),
            wow_to_bevy([0.0, 0.0, 1.0]),
        ));
        Affine3A::from_scale_rotation_translation(
            placement.scale,
            placement.rotation,
            placement.translation,
        ) * axes
    };
    // Row i: output coordinate i's linear terms and translation, `dot(row.xyz, p) + row.w`.
    let m = Mat4::from(mat);
    let rows = [
        m.row(0).to_array(),
        m.row(1).to_array(),
        m.row(2).to_array(),
    ];
    let scale = if def.scale_size_by_instance() {
        placement.scale.x.max(1e-4)
    } else {
        frame.size_scale
    };
    let (base_r, base_u) = if def.xy_quad() {
        let s = placement.scale.x.max(1e-4);
        (
            placement.rotation * (wow_to_bevy([0.0, 1.0, 0.0]) * s),
            placement.rotation * (wow_to_bevy([-1.0, 0.0, 0.0]) * s),
        )
    } else {
        (cam.right, cam.up)
    };
    let v4 = |v: Vec3| [v.x, v.y, v.z, 0.0];
    let light = match (def.lit, committed) {
        (true, Some(c)) => [c[0], c[1], c[2], 1.0],
        _ => [0.0; 4],
    };
    let mut pool = *template;
    pool.mat = rows;
    pool.base_r = v4(base_r);
    pool.base_u = v4(base_u);
    pool.cam_r = v4(cam.right);
    pool.cam_u = v4(cam.up);
    pool.params[0] = frame.alpha;
    pool.params[1] = scale;
    pool.light = light;
    pool
}

#[cfg(test)]
mod tests {
    use super::{expand_quads, spin_angle, CamBasis, DrawFrame, Particle};
    use benilla_formats::LifeTrack;
    use bevy::prelude::{Quat, Transform, Vec3};

    /// The `0x97c594` negate: a random-sign emitter flips its bit-5 half, whatever the sign.
    #[test]
    fn a_random_sign_emitter_counter_rotates_the_bit5_half() {
        assert_eq!(
            spin_angle(0.25, 3.0, 0.5, true, 0x20),
            -1.75,
            "bit 5 set: negated"
        );
        assert_eq!(
            spin_angle(0.25, 3.0, 0.5, true, 0x1f),
            1.75,
            "bit 5 clear: kept"
        );
        assert_eq!(
            spin_angle(0.25, -3.0, 0.5, false, 0x20),
            -1.25,
            "no flag: kept"
        );
        assert_eq!(spin_angle(0.0, 0.0, 0.5, true, 0xff), 0.0);
    }

    fn particle() -> Particle {
        Particle {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            age: 0.0,
            life: 1.0,
            life_seed: 0.0,
            size_mul: [1.0; 2],
            base_spin: 0.0,
            spin: 0.0,
            cell: 0,
            phase: 0,
            fresh: false,
            quat: Quat::IDENTITY,
            angvel: Vec3::ZERO,
        }
    }

    fn cam() -> CamBasis {
        CamBasis {
            right: Vec3::X,
            up: Vec3::Y,
        }
    }

    fn frame(alpha: f32) -> DrawFrame {
        DrawFrame {
            anchored: true,
            ride: crate::ride_frame::StoredFrame::default(),
            alpha,
            size_scale: 1.0,
        }
    }

    /// A 3.3.5a quad takes its x and y extents apart, each times the particle's size roll.
    #[test]
    fn the_head_quad_takes_x_and_y_extents_and_the_size_roll() {
        let mut def = crate::particles::tests::plain_def();
        def.over_life.scale = LifeTrack::constant([1.0, 2.0]);
        let mut p = particle();
        p.size_mul = [0.5, 1.5];
        let mut out = Vec::new();
        expand_quads(
            &def,
            std::slice::from_ref(&p),
            &frame(1.0),
            &Transform::IDENTITY,
            &cam(),
            &mut out,
        );
        assert_eq!(out.len(), 4);
        assert_eq!(out[2].pos, [0.5, 3.0, 0.0], "corner +r+u: 1·0.5 by 2·1.5");
        // No head flag, no head quad.
        def.flags &= !0x20000;
        out.clear();
        expand_quads(
            &def,
            std::slice::from_ref(&p),
            &frame(1.0),
            &Transform::IDENTITY,
            &cam(),
            &mut out,
        );
        assert!(out.is_empty());
    }

    /// Base spin turns the quad only while spin or its variance is non-zero (`0x97c282`).
    #[test]
    fn base_spin_alone_does_not_turn_the_quad() {
        let mut def = crate::particles::tests::plain_def();
        let mut p = particle();
        p.base_spin = std::f32::consts::FRAC_PI_2;
        let corner = |def: &benilla_formats::ParticleEmitterDef| {
            let mut out = Vec::new();
            expand_quads(
                def,
                std::slice::from_ref(&p),
                &frame(1.0),
                &Transform::IDENTITY,
                &cam(),
                &mut out,
            );
            out[2].pos
        };
        assert_eq!(corner(&def), [1.0, 1.0, 0.0], "unturned");
        def.spin_vary = 0.1;
        let c = corner(&def);
        assert!(
            (c[0] + 1.0).abs() < 1e-6 && (c[1] - 1.0).abs() < 1e-6,
            "turned 90°: {c:?}"
        );
    }

    /// The reference folds `emitter+0x1a8`, the model's `+0x19c`, into the alpha byte
    /// alone.
    #[test]
    fn the_models_render_alpha_scales_particle_alpha_and_nothing_else() {
        let mut def = crate::particles::tests::plain_def();
        def.over_life.color = LifeTrack::constant([204.0, 102.0, 51.0]);
        def.over_life.alpha = LifeTrack::constant(0.5);
        let pool = [particle()];
        let shoot = |alpha| {
            let mut out = Vec::new();
            expand_quads(
                &def,
                &pool,
                &frame(alpha),
                &Transform::IDENTITY,
                &cam(),
                &mut out,
            );
            out[0].color
        };

        let opaque = shoot(1.0);
        assert_eq!(
            opaque,
            [0.8, 0.4, 0.2, 0.5],
            "no fade: the authored channels"
        );
        let half = shoot(0.5);
        assert_eq!(
            [half[0], half[1], half[2]],
            [0.8, 0.4, 0.2],
            "RGB is untouched by the model alpha"
        );
        assert!((half[3] - 0.25).abs() < 1e-6, "alpha halves: {half:?}");
        assert_eq!(shoot(0.0)[3], 0.0, "an invisible model draws nothing");
    }

    /// The GPU record's rows place a particle and turn its velocity as the CPU writer does, in
    /// model mode through the placement and in world mode through the ride frame.
    #[test]
    fn the_gpu_record_maps_points_and_velocities_as_the_cpu_writer() {
        use super::{particle_center, pool_record};
        use bevy::math::Affine3A;
        let def = crate::particles::tests::plain_def();
        let placement = Transform {
            translation: Vec3::new(10.0, -4.0, 2.5),
            rotation: Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.3),
            scale: Vec3::splat(2.0),
        };
        let mut p = particle();
        p.pos = Vec3::new(0.3, -1.2, 4.0);
        p.vel = Vec3::new(-2.0, 0.5, 1.5);
        let mut ride = crate::ride_frame::StoredFrame::default();
        ride.retarget(Some((
            bevy::prelude::Entity::PLACEHOLDER,
            Affine3A::from_rotation_translation(
                Quat::from_rotation_y(1.1),
                Vec3::new(5.0, 1.0, -3.0),
            ),
        )));
        for anchored in [false, true] {
            let frame = DrawFrame {
                anchored,
                ride,
                alpha: 1.0,
                size_scale: 1.0,
            };
            let template = super::pool_template(&def).expect("plain pool");
            let pool = pool_record(&template, &def, &frame, &placement, &cam(), None);
            let apply = |v: Vec3, w: f32| {
                Vec3::from_array(std::array::from_fn(|i| {
                    let r = pool.mat[i];
                    r[0] * v.x + r[1] * v.y + r[2] * v.z + r[3] * w
                }))
            };
            let center = particle_center(&frame, &placement, &p);
            assert!(
                apply(p.pos, 1.0).abs_diff_eq(center, 1e-4),
                "point, anchored {anchored}"
            );
            let vel = if anchored {
                ride.dir_to_world(p.vel)
            } else {
                placement.rotation * (placement.scale * super::wow_to_bevy(p.vel.to_array()))
            };
            assert!(
                apply(p.vel, 0.0).abs_diff_eq(vel, 1e-4),
                "velocity, anchored {anchored}"
            );
        }
    }

    /// A track past the GPU's keys or an atlas off the power-of-two grid stays on the CPU writer.
    #[test]
    fn a_pool_past_the_gpu_sampler_takes_the_cpu_writer() {
        use super::pool_template;
        let def = crate::particles::tests::plain_def();
        let pool = |def: &benilla_formats::ParticleEmitterDef| pool_template(def);
        assert!(pool(&def).is_some());
        let mut long = def.clone();
        long.over_life.alpha = LifeTrack {
            times: vec![0.0, 0.2, 0.4, 0.6, 1.0],
            values: vec![1.0; 5],
        };
        assert!(pool(&long).is_none(), "five alpha keys");
        let mut grid = def.clone();
        grid.tile_cols = 3;
        assert!(pool(&grid).is_none(), "three columns");
    }
}
