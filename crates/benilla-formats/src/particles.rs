//! 3.3.5a (MD20 v264) M2 particle emitters: fixed 0x1dc-byte records (array at MD20+0x128/+0x12c)
//! at the reference loader's offsets (`0x833b40`). The emission tracks are read onto the model's
//! sequence bands ([`benilla_m2::Tracks264`]); rate and enabled bake one loop per sequence into
//! [`EmitTiming`], the other nine into [`EmitParams`].

use anyhow::Result;
use benilla_m2::{M2Model, M2ScalarTrack};

use crate::emit_timing::{EmitParams, EmitTiming};
use crate::models::SeqSlot;

/// Emitter spawn shape, file `emitterType` (+0x29, a byte; `0x832ef0` sizes the emitter by it):
/// 1 plane, 2 sphere, 3 spline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleShape {
    Plane,
    Sphere,
    Spline,
}

/// Particle blend, file `blendingType` (+0x28), the M2 blend enum. Mod (5) and mod2x (6) have no
/// modulate path yet and draw as `Alpha`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleBlend {
    /// `4` Add, `(SRC_ALPHA, ONE)`, and `3` NoAlphaAdd, which the reference draws `(ONE, ONE)`
    /// (device blend 10, the loader's switch at `0x833e21`): no depth write or sort. Flames and
    /// glows.
    Add,
    /// `2` Alpha: `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)`. Smoke.
    Alpha,
    /// `1` AlphaKey, for chips and debris: an alpha-tested opaque material (device blend 1).
    AlphaKey,
    /// `0` Opaque: blending off, depth write on, no alpha test. Also any unknown byte.
    Opaque,
}

/// One over-life channel, 3.3.5a's `M2FakeTrack`: keys at fixed16 fractions of the particle's
/// life (`i16 / 0x7fff`), sampled at its normalized age by `0x9793b0`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LifeTrack<V> {
    /// Key times, fractions of the life.
    pub times: Vec<f32>,
    pub values: Vec<V>,
}

impl<V: Copy> LifeTrack<V> {
    /// A one-key constant channel.
    pub fn constant(v: V) -> Self {
        Self {
            times: vec![0.0],
            values: vec![v],
        }
    }

    /// The value indices and fraction at normalized age `u`, by `0x9793b0`'s three arms: two
    /// keys span the whole life whatever their times say; three split at the middle key; more
    /// take the binary search `0x979330`. The fraction is not clamped, as age < life at a draw.
    fn locate(&self, u: f32) -> (usize, usize, f32) {
        match self.times.len() {
            0..=2 => (0, 1, u),
            3 => {
                let mid = self.times[1];
                if u < mid {
                    (0, 1, u / mid)
                } else {
                    (1, 2, (u - mid) / (1.0 - mid))
                }
            }
            n => {
                let k = self.times[1..n - 1].partition_point(|&t| t <= u);
                (
                    k,
                    k + 1,
                    (u - self.times[k]) / (self.times[k + 1] - self.times[k]),
                )
            }
        }
    }

    /// The channel at normalized age `u`: a one-value channel is that value (the callers' `count ==
    /// 1` arm, `0x979487`); otherwise `lerp` between the located pair. `None` with no values.
    pub fn sample(&self, u: f32, lerp: impl Fn(V, V, f32) -> V) -> Option<V> {
        match self.values.as_slice() {
            [] => None,
            [v] => Some(*v),
            values => {
                let (lo, hi, t) = self.locate(u);
                let (a, b) = (*values.get(lo)?, *values.get(hi)?);
                Some(lerp(a, b, t))
            }
        }
    }
}

/// The five over-life channels (+0x104..+0x15b), sampled at the particle's normalized age by
/// `0x979e90`.
#[derive(Debug, Clone, Default)]
pub struct OverLife {
    /// RGB, authored 0..255 (+0x104).
    pub color: LifeTrack<[f32; 3]>,
    /// Opacity, fixed16 (+0x114).
    pub alpha: LifeTrack<f32>,
    /// Half-extent per axis, yards (+0x124).
    pub scale: LifeTrack<[f32; 2]>,
    /// Head flipbook cell (+0x13c).
    pub head_cell: LifeTrack<u16>,
    /// Tail flipbook cell (+0x14c).
    pub tail_cell: LifeTrack<u16>,
}

/// One evaluation of the over-life channels, before the per-particle variances.
#[derive(Debug, Clone, Copy)]
pub struct OverLifeSample {
    /// RGB 0..1 and over-life opacity.
    pub color: [f32; 4],
    /// Half-extent per axis, yards.
    pub size: [f32; 2],
    /// `None` when the head channel has no keys, where the reference draws cell 0 or, on a
    /// random-cell emitter, the particle's own cell (`0x979f68`).
    pub head_cell: Option<u16>,
    pub tail_cell: u16,
}

/// A cell lerp: `fistp` of `lo + (hi − lo)·t`, the FPU's round-to-nearest-even (`0x9795aa`).
fn cell_lerp(a: u16, b: u16, t: f32) -> u16 {
    let v = f32::from(a) + (f32::from(b) - f32::from(a)) * t;
    v.round_ties_even() as i32 as u16
}

impl OverLife {
    /// Sample every channel at normalized age `u = age / life`. Colour bytes round as the
    /// reference's `fistp` (`0x979702`); an unkeyed channel takes white, 1, 1 yard or cell 0.
    pub fn sample(&self, u: f32) -> OverLifeSample {
        let lerp3 = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
        };
        let rgb = self.color.sample(u, lerp3).unwrap_or([255.0; 3]);
        let byte = |v: f32| f32::from((v.round_ties_even() as i32) as u8) / 255.0;
        let alpha = self
            .alpha
            .sample(u, |a, b, t| a + (b - a) * t)
            .unwrap_or(1.0);
        let size = self
            .scale
            .sample(u, |a, b, t| {
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            })
            .unwrap_or([1.0; 2]);
        OverLifeSample {
            color: [byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), alpha],
            size,
            head_cell: (!self.head_cell.times.is_empty())
                .then(|| self.head_cell.sample(u, cell_lerp))
                .flatten(),
            tail_cell: (!self.tail_cell.times.is_empty())
                .then(|| self.tail_cell.sample(u, cell_lerp))
                .flatten()
                .unwrap_or(0),
        }
    }
}

/// A spline (type 3) emitter's curve: a cubic Bézier chain (`3K+1` points: P, out, in, P, …)
/// walked by normalized arc length, whose knots the reference computes at load for the spawn
/// kernel. A segment's length here is a 16-chord sum; the reference's method is untraced.
#[derive(Debug, Clone)]
pub struct SplineData {
    /// Control points, model-local WoW axes (`3K+1`, from file+0x1c0/+0x1c4).
    pub points: Vec<[f32; 3]>,
    /// Cumulative normalized arc length at each segment boundary (`K+1` entries, 0 to 1).
    knots: Vec<f32>,
}

impl SplineData {
    /// From `3K+1` raw control points.
    pub fn new(points: Vec<[f32; 3]>) -> Option<Self> {
        let k = points.len().checked_sub(1)? / 3;
        if k == 0 || points.len() != 3 * k + 1 {
            return None;
        }
        let mut knots = vec![0.0f32];
        for seg in 0..k {
            let mut len = 0.0;
            let mut prev = Self::bezier(&points[3 * seg..3 * seg + 4], 0.0);
            for i in 1..=16 {
                let p = Self::bezier(&points[3 * seg..3 * seg + 4], i as f32 / 16.0);
                len += ((p[0] - prev[0]).powi(2)
                    + (p[1] - prev[1]).powi(2)
                    + (p[2] - prev[2]).powi(2))
                .sqrt();
                prev = p;
            }
            knots.push(knots[seg] + len);
        }
        let total = *knots.last().unwrap();
        if total > 0.0 {
            for kn in &mut knots {
                *kn /= total;
            }
        }
        Some(Self { points, knots })
    }

    fn bezier(p: &[[f32; 3]], u: f32) -> [f32; 3] {
        let w = [
            (1.0 - u).powi(3),
            3.0 * u * (1.0 - u).powi(2),
            3.0 * u * u * (1.0 - u),
            u.powi(3),
        ];
        std::array::from_fn(|c| (0..4).map(|i| w[i] * p[i][c]).sum())
    }

    fn bezier_deriv(p: &[[f32; 3]], u: f32) -> [f32; 3] {
        let w = [
            -3.0 * (1.0 - u).powi(2),
            3.0 * (1.0 - u) * (1.0 - 3.0 * u),
            3.0 * u * (2.0 - 3.0 * u),
            3.0 * u * u,
        ];
        std::array::from_fn(|c| (0..4).map(|i| w[i] * p[i][c]).sum())
    }

    fn locate(&self, t: f32) -> (usize, f32) {
        let k = self.knots.len() - 1;
        let seg = self.knots[1..k]
            .iter()
            .position(|&kn| t < kn)
            .unwrap_or(k - 1);
        let (a, b) = (self.knots[seg], self.knots[seg + 1]);
        (seg, ((t - a) / (b - a).max(1e-6)).clamp(0.0, 1.0))
    }

    /// The point at arc fraction `t`, clamped to the end points outside [0, 1].
    pub fn eval(&self, t: f32) -> [f32; 3] {
        if t <= 0.0 {
            return self.points[0];
        }
        if t >= 1.0 {
            return *self.points.last().unwrap();
        }
        let (seg, u) = self.locate(t);
        Self::bezier(&self.points[3 * seg..3 * seg + 4], u)
    }

    /// The unnormalized tangent at arc fraction `t` (its caller renormalizes).
    pub fn tangent(&self, t: f32) -> [f32; 3] {
        let (seg, u) = self.locate(t.clamp(0.0, 1.0));
        Self::bezier_deriv(&self.points[3 * seg..3 * seg + 4], u)
    }
}

/// One 3.3.5a M2 particle emitter, positions and vectors model-local (WoW axes, Z up).
#[derive(Debug, Clone)]
pub struct ParticleEmitterDef {
    /// `particleColorIndex` (+0x2a): 11, 12 or 13 takes that set of a display's
    /// [`ParticleColors`] ([`Self::recolored`]).
    pub color_index: u16,
    pub flags: u32,
    pub position: [f32; 3],
    pub bone: u16,
    pub shape: ParticleShape,
    pub blend: ParticleBlend,
    /// Whether the scene light multiplies the quads (`lit_of`, from the raw blend byte).
    pub lit: bool,
    /// Geometry model (+0x18, an `M2Array<char>` path): spawns models, not quads.
    pub geometry_model: Option<String>,
    /// Recursion model (+0x20): up to 4 of its emitters run as children, driven once per live
    /// parent particle per frame at that particle's position.
    pub recursion_model: Option<String>,
    /// Particle texture, the `.blp` path from the M2 textures table (+0x16).
    pub texture: Option<String>,
    /// Atlas rows (+0x30) and `tile_cols` columns (+0x32), both non-zero powers of two; a bad pair
    /// keeps 1×1.
    pub tile_rows: u16,
    pub tile_cols: u16,
    /// Spawn rate (+0xb0) and the on/off gate (+0x1c8), baked one loop per sequence.
    pub timing: EmitTiming,
    /// The other nine emission tracks (+0x34..+0xf0), baked the same way and sampled each frame
    /// on the emitter's clock: they animate, so `value[0]` alone is wrong.
    pub params: EmitParams,
    /// Life variance (+0xac, runtime +0xa8): a particle lives `lifespan + seed·vary`, its seed a
    /// fixed16 in [−1, 1] drawn at birth, against the emitter's live lifespan every frame
    /// (`0x97de97`), floored at 1 ms.
    pub lifespan_vary: f32,
    /// Rate variance (+0xc4, runtime +0xa0): each frame's rate is `rate + S11·vary` (`0x97d952`).
    pub rate_vary: f32,
    /// Size variance per axis (+0x134/+0x138, runtime +0xe4/+0xe8): the over-life size times
    /// `max(1 + S11·vary, 1e-4)`, one roll for both axes unless [`Self::scale_vary_per_axis`]
    /// (`0x97a04b`).
    pub scale_vary: [f32; 2],
    /// Velocity drag (+0x174): each frame, after gravity, `vel −= min(dt·drag, 1)·vel`.
    pub drag: f32,
    /// Tail time (+0x15c, seconds): a tail particle draws a streak `|velocity| · tail_time` long
    /// behind its motion.
    pub tail_time: f32,
    /// Model particles' spin range (+0x188 min, +0x194 max, rad/s), rolled per birth.
    pub angular_velocity_min: [f32; 3],
    pub angular_velocity_max: [f32; 3],
    /// Scales a flag-0x40 emitter's inherited velocity (+0x170, runtime +0x14c, `0x97ed88`).
    pub inherit_scale: f32,
    /// Two (emitter speed, follow fraction) samples (+0x1b0..+0x1bc) for [`Self::follow_line`].
    pub follow_speed1: f32,
    pub follow_scale1: f32,
    pub follow_speed2: f32,
    pub follow_scale2: f32,
    /// Twinkle (+0x160..+0x16c): the half-size is the over-life size times [`Self::twinkle`], a
    /// multiplier skipped when `min == max`, so `{0,0}` burns steady at ramp size.
    pub twinkle_speed: f32,
    /// Below 1, a frame whose twinkle noise exceeds it draws no quad (`0x97bed8`).
    pub twinkle_percent: f32,
    pub twinkle_min: f32,
    pub twinkle_max: f32,
    /// Spline curve (+0x1c0 count, +0x1c4 offset, `3·⌊count/3⌋ + 1` points). A spline
    /// repurposes the tracks' `values[0]`: `area_length`/`area_width` are the spawn window in
    /// [0, 1], `vertical_range` a spin ψ about the tangent, `horizontal_range` a scatter jitter
    /// along the velocity, and the rate a load-time arc scale that the first rate sample overwrites.
    pub spline: Option<SplineData>,
    /// Quad rotation (+0x178 base, +0x17c its variance, +0x180 spin, +0x184 its variance): a
    /// particle turns `base + spin·age` rad, each rolled `x + S11·vary` per particle (`0x97a130`),
    /// and only while `spin` or its variance is non-zero (`0x97c282`).
    pub base_spin: f32,
    pub base_spin_vary: f32,
    pub spin: f32,
    pub spin_vary: f32,
    pub over_life: OverLife,
}

impl ParticleEmitterDef {
    /// This emitter under a display's particle colour override: its colour track's first three
    /// values become set `color_index − 11`'s start, mid and end, the colours `0x9795d0` lerps at
    /// the track's located keys. `None` where nothing changes: another index, or a one-value track,
    /// which `0x9795d0` returns as authored.
    pub fn recolored(&self, colors: &ParticleColors) -> Option<Self> {
        let set = colors
            .0
            .get(usize::from(self.color_index.checked_sub(11)?))?;
        if self.over_life.color.values.len() < 2 {
            return None;
        }
        let mut def = self.clone();
        for (v, c) in def.over_life.color.values.iter_mut().zip(set) {
            *v = *c;
        }
        Some(def)
    }

    /// File flag 0x10 (runtime 0x200): particles live emitter-local and the draw re-applies the
    /// live emitter matrix, so the cloud rides the emitter; clear, the spawn bakes them into world
    /// space, so a moving host lays a trail.
    pub fn model_space(&self) -> bool {
        self.flags & 0x10 != 0
    }
    /// File flag 0x4000 (runtime 0x80000): live particles move by [`Self::follow_line`]'s
    /// fraction (≤ 1) of the emitter's per-frame motion.
    pub fn follow_emitter(&self) -> bool {
        self.flags & 0x4000 != 0
    }
    /// `(slope, intercept)` through the two follow samples: the per-frame fraction is
    /// `clamp(slope·|Δpos|/dt + intercept, 0, 1)`. Equal speeds give no response.
    pub fn follow_line(&self) -> Option<(f32, f32)> {
        ((self.follow_speed2 - self.follow_speed1).abs() >= 1e-6).then(|| {
            let slope = (self.follow_scale2 - self.follow_scale1)
                / (self.follow_speed2 - self.follow_speed1);
            (slope, self.follow_scale1 - slope * self.follow_speed1)
        })
    }
    /// File flag 0x40 (runtime 0x800): births inherit the emitter's motion. Past each 1/30 s of
    /// accumulated dt the inherit velocity becomes `oneFrameΔ · ((1/30) / accum) · inherit_scale`
    /// (`0x97ed88`), and each birth adds `(1 + S11·speed_variation) · inherit` to its velocity.
    pub fn inherits_emitter_motion(&self) -> bool {
        self.flags & 0x40 != 0
    }
    /// File flag 0x20 (runtime 0x400): particle size scales with the emitter transform's scale
    /// (`0x97bf7e`); without it an instance-scaled prop scales only its particle positions.
    pub fn scale_size_by_instance(&self) -> bool {
        self.flags & 0x20 != 0
    }
    /// File flag 0x100 on a sphere (runtime 0x8000, mapped only for type 2): birth velocity is
    /// straight +Z instead of radial.
    pub fn sphere_up(&self) -> bool {
        self.flags & 0x100 != 0
    }
    /// File flag 0x80 on a sphere (runtime 0x1000, mapped only for type 2): a particle dies the
    /// frame `dot(stepVelocity, updatedPos) > 0`.
    pub fn kill_outbound(&self) -> bool {
        self.shape == ParticleShape::Sphere && self.flags & 0x80 != 0
    }
    /// File flag 0x400 (runtime 0x20000, `0x978d34`): the tail streak's `tail_time` is clamped to
    /// the particle's age, so the streak grows from zero at birth.
    pub fn tail_clamps_to_age(&self) -> bool {
        self.flags & 0x400 != 0
    }
    /// File flag 0x200 (runtime 0x10000): a geometry particle's tumble axes each flip sign with
    /// probability ½ (`0x979b08`), and half the quads turn the other way (`0x97c594`).
    pub fn random_spin_sign(&self) -> bool {
        self.flags & 0x200 != 0
    }
    /// File flag 0x2000 (runtime 0x40000): once per birth in world mode the client probes down
    /// for terrain, WMO or doodad geometry, and the quad stands on the surface.
    pub fn ground_snap(&self) -> bool {
        self.flags & 0x2000 != 0
    }
    /// File flag 0x8000, a one-shot burst: when `enabled && rate > 0` first holds it spawns
    /// `ftol(rate · density · LOD)` particles once, never pours `rate·dt`, and re-arms only when
    /// the gate falls.
    pub fn burst(&self) -> bool {
        self.flags & 0x8000 != 0
    }
    /// File flag 0x1000 (runtime 0x4000): the head quad lies flat in the emitter's model XY plane,
    /// spinning about its normal.
    pub fn xy_quad(&self) -> bool {
        self.flags & 0x1000 != 0
    }
    /// File flag 0x20000 (runtime 0x4, `0x978d0a`): the head quad is drawn (`0x97bfe8`).
    pub fn draws_head(&self) -> bool {
        self.flags & 0x20000 != 0
    }
    /// File flag 0x40000 (runtime 0x8): the tail streak is drawn.
    pub fn draws_tail(&self) -> bool {
        self.flags & 0x40000 != 0
    }
    /// File flag 0x10000 on an atlas of more than one cell (runtime 0x100000, `0x978e30`): a head
    /// with no cell keys draws a random cell per particle (`0x979f7e`).
    pub fn random_cell(&self) -> bool {
        self.flags & 0x10000 != 0 && u32::from(self.tile_rows) * u32::from(self.tile_cols) > 1
    }
    /// File flag 0x80000 (runtime 0x800000): [`Self::scale_vary`] rolls x and y separately.
    pub fn scale_vary_per_axis(&self) -> bool {
        self.flags & 0x80000 != 0
    }
    /// Whether the quad turns at all: only a non-zero spin or spin variance applies the rotation,
    /// base spin included (`0x97c282`).
    pub fn rotates(&self) -> bool {
        self.spin != 0.0 || self.spin_vary != 0.0
    }
    /// The gated twinkle size multiplier for a noise sample in [0, 1) from the caller's LUT at
    /// `twinkle_speed · age`: 1 when `min == max`.
    pub fn twinkle(&self, noise: f32) -> f32 {
        if (self.twinkle_max - self.twinkle_min).abs() < 1e-6 {
            1.0
        } else {
            noise * (self.twinkle_max - self.twinkle_min) + self.twinkle_min
        }
    }
}

const STRIDE: usize = 0x1dc;
const HDR_TEXTURES: usize = 0x50;
pub(crate) const HDR_RIBBONS: usize = 0x120;
const HDR_PARTICLES: usize = 0x128;

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_vec3(b: &[u8], o: usize) -> [f32; 3] {
    [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]
}

fn blend_of(v: u8) -> ParticleBlend {
    match v {
        3 | 4 => ParticleBlend::Add,
        2 | 5 | 6 => ParticleBlend::Alpha, // 5/6 (mod/mod2x): no modulate path yet
        1 => ParticleBlend::AlphaKey,
        _ => ParticleBlend::Opaque,
    }
}

/// The per-blend lighting table, indexed by the raw blend byte: the mod modes light nothing.
const LIGHTING_BY_BLEND: [bool; 7] = [true, true, true, true, true, false, false];

/// Whether the reference lights this emitter's quads: its material takes lighting from file bit
/// 0x1 clear (`0x833e71`) and the blend's table entry. Bit 0x1 is unlit; wowdev has it inverted.
fn lit_of(flags: u32, blend_byte: u8) -> bool {
    flags & 0x1 == 0
        && LIGHTING_BY_BLEND
            .get(usize::from(blend_byte))
            .copied()
            .unwrap_or(true)
}

fn shape_of(v: u8) -> ParticleShape {
    match v {
        2 => ParticleShape::Sphere,
        3 => ParticleShape::Spline,
        _ => ParticleShape::Plane,
    }
}

/// A display's particle colour override, one `ParticleColor.dbc` row: three sets of start, mid and
/// end colours, raw `0..=255` RGB. The particle colour replace (`0x4ea9e0`) hands set `i` to the
/// emitters whose `particleColorIndex` is `11 + i` (`0x825410`), and their colour track then lerps
/// these at its located keys instead of its own values (`0x97a990`, `0x9795d0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleColors(pub [[[f32; 3]; 3]; 3]);

impl ParticleColors {
    /// What an id with no row passes: every colour opaque green (`0x4eaa14`).
    pub const MISSING: Self = Self([[[0.0, 255.0, 0.0]; 3]; 3]);
}

/// `ParticleColor.dbc` by id.
#[derive(Debug, Clone, Default)]
pub struct ParticleColorTable(std::collections::HashMap<u32, ParticleColors>);

impl ParticleColorTable {
    /// The override a display's `ParticleColorID` names: `None` for 0, [`ParticleColors::MISSING`]
    /// for an id with no row.
    pub fn resolve(&self, id: u32) -> Option<ParticleColors> {
        (id != 0).then(|| self.0.get(&id).copied().unwrap_or(ParticleColors::MISSING))
    }
}

/// `ParticleColor.dbc`: ten `u32` columns, 40-byte rows.
pub(crate) fn particle_color_schema() -> benilla_dbc::Schema {
    let mut schema = benilla_dbc::Schema::new("ParticleColor");
    for i in 0..10 {
        schema.add_field(benilla_dbc::SchemaField::new(
            format!("c{i}"),
            benilla_dbc::FieldType::UInt32,
        ));
    }
    schema
}

/// Load `ParticleColor.dbc`: id, then `Start[3]`, `Mid[3]` and `End[3]`, each a packed BGRA
/// `CImVector`.
pub fn load_particle_colors(chain: &mut crate::Chain) -> Result<ParticleColorTable> {
    const PATH: &str = "DBFilesClient\\ParticleColor.dbc";
    let bytes = chain.read_file(PATH)?;
    let rs = crate::dbc::parse(&bytes, particle_color_schema(), "ParticleColor")?;
    let rgb = |v: u32| {
        [
            ((v >> 16) & 0xff) as f32,
            ((v >> 8) & 0xff) as f32,
            (v & 0xff) as f32,
        ]
    };
    let mut out = std::collections::HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = crate::dbc::u32_at(r, 0) else {
            continue;
        };
        let at = |c: usize| rgb(crate::dbc::u32_at(r, c).unwrap_or(0));
        out.insert(
            id,
            ParticleColors(std::array::from_fn(|set| {
                [at(1 + set), at(4 + set), at(7 + set)]
            })),
        );
    }
    Ok(ParticleColorTable(out))
}

/// The version-264 model's texture paths, by index (header +0x50, records of 16 bytes with the
/// filename `M2Array<char>` at +0x08).
pub(crate) fn texture_paths_264(b: &[u8]) -> Vec<Option<String>> {
    let (n, o) = (
        le_u32(b, HDR_TEXTURES) as usize,
        le_u32(b, HDR_TEXTURES + 4) as usize,
    );
    if n > 0x10000 {
        return Vec::new();
    }
    (0..n)
        .map_while(|i| {
            let r = o.checked_add(i * 16)?;
            (r + 16 <= b.len()).then_some(r)
        })
        .map(|r| {
            let (len, at) = (le_u32(b, r + 8) as usize, le_u32(b, r + 12) as usize);
            let s = b.get(at..at.checked_add(len)?)?;
            let s = String::from_utf8_lossy(s)
                .trim_end_matches('\0')
                .to_string();
            (!s.is_empty()).then_some(s)
        })
        .collect()
}

/// A fake track's `(times, values)` arrays (`0x10` bytes at `at`), `size`-byte values decoded by
/// `read`; empty where the file cannot hold them.
fn life_track<V>(
    b: &[u8],
    at: usize,
    size: usize,
    read: impl Fn(&[u8], usize) -> V,
) -> LifeTrack<V> {
    let (tn, to) = (le_u32(b, at) as usize, le_u32(b, at + 4) as usize);
    let (vn, vo) = (le_u32(b, at + 8) as usize, le_u32(b, at + 12) as usize);
    let fits = |n: usize, o: usize, s: usize| {
        n <= 0x10000 && o.checked_add(n * s).is_some_and(|end| end <= b.len())
    };
    if !fits(tn, to, 2) || !fits(vn, vo, size) {
        return LifeTrack {
            times: Vec::new(),
            values: Vec::new(),
        };
    }
    LifeTrack {
        times: (0..tn)
            .map(|i| f32::from(le_u16(b, to + i * 2) as i16) / 32767.0)
            .collect(),
        values: (0..vn).map(|i| read(b, vo + i * size)).collect(),
    }
}

/// A model's particle emitters, textures resolved through its textures table; empty for a model
/// with none.
pub fn parse_m2_particle_emitters(model: &M2Model) -> Result<Vec<ParticleEmitterDef>> {
    let bytes = model.bytes();
    let count = le_u32(bytes, HDR_PARTICLES) as usize;
    let base = le_u32(bytes, HDR_PARTICLES + 4) as usize;
    if count == 0 || count > 256 || base + count * STRIDE > bytes.len() {
        return Ok(Vec::new());
    }
    let textures = texture_paths_264(bytes);
    let mut tracks = model.tracks();

    // Each sequence's band and loop flag (flags bit 0 clear loops); a model with none gets one
    // whole-timeline slot so its tracks still bake.
    let mut seq_slots: Vec<SeqSlot> = tracks
        .sequences()
        .into_iter()
        .enumerate()
        .map(|(index, (band, flags))| SeqSlot {
            index,
            band,
            looping: flags & 1 == 0,
        })
        .collect();
    if seq_slots.is_empty() {
        seq_slots.push(SeqSlot {
            index: 0,
            band: (0, u32::MAX),
            looping: true,
        });
    }
    // Global-sequence durations (header +0x14/+0x18): a gseq-tagged track loops on its own clock.
    let gseq: Vec<u32> = {
        let (n, o) = (le_u32(bytes, 0x14) as usize, le_u32(bytes, 0x18) as usize);
        (0..n)
            .map_while(|i| (o + i * 4 + 4 <= bytes.len()).then(|| le_u32(bytes, o + i * 4)))
            .collect()
    };

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = base + i * STRIDE;
        let flags = le_u32(bytes, e + 0x04);
        let texture = textures
            .get(usize::from(le_u16(bytes, e + 0x16)))
            .cloned()
            .flatten();
        let shape = shape_of(bytes[e + 0x29]);
        // The geometry and recursion model paths (`M2Array<char>`, file-relative offsets).
        let model_path = |cnt_off: usize| -> Option<String> {
            let n = le_u32(bytes, e + cnt_off) as usize;
            let ofs = le_u32(bytes, e + cnt_off + 4) as usize;
            if n < 2 || ofs.checked_add(n)? > bytes.len() {
                return None;
            }
            let s = String::from_utf8_lossy(&bytes[ofs..ofs + n])
                .trim_end_matches('\0')
                .to_string();
            (!s.is_empty()).then_some(s)
        };
        let spline = (shape == ParticleShape::Spline)
            .then(|| {
                let q = le_u32(bytes, e + 0x1c0) as usize;
                let ofs = le_u32(bytes, e + 0x1c4) as usize;
                let n = 3 * (q / 3) + 1;
                if !(3..=0x10000).contains(&q) || ofs + n * 12 > bytes.len() {
                    return None;
                }
                SplineData::new((0..n).map(|p| le_vec3(bytes, ofs + p * 12)).collect())
            })
            .flatten();
        let tiles = match (le_u16(bytes, e + 0x30), le_u16(bytes, e + 0x32)) {
            (r, c) if r.is_power_of_two() && c.is_power_of_two() => (r, c),
            _ => (1, 1),
        };
        let over_life = OverLife {
            color: life_track(bytes, e + 0x104, 12, le_vec3),
            alpha: life_track(bytes, e + 0x114, 2, |b, o| {
                f32::from(le_u16(b, o) as i16) / 32767.0
            }),
            scale: life_track(bytes, e + 0x124, 8, |b, o| [le_f32(b, o), le_f32(b, o + 4)]),
            head_cell: life_track(bytes, e + 0x13c, 2, le_u16),
            tail_cell: life_track(bytes, e + 0x14c, 2, le_u16),
        };
        // The emission tracks, onto the model's bands; a sequence without keys takes the
        // channel default, as an unkeyed channel does.
        let mut f = |at: usize, default: f32| -> M2ScalarTrack {
            tracks.float(e + at, default).unwrap_or_default()
        };
        let params = [
            f(0x34, 0.0), // speed
            f(0x48, 0.0), // speedVar
            f(0x5c, 0.0), // latitude
            f(0x70, 0.0), // longitude
            f(0x84, 0.0), // gravity
            f(0x98, 1.0), // lifespan
            f(0xc8, 0.0), // areaLength
            f(0xdc, 0.0), // areaWidth
            f(0xf0, 0.0), // zSource
        ];
        let rate = f(0xb0, 0.0);
        let enabled = tracks
            .byte(e + 0x1c8, 1)
            .map(|mut t| {
                for k in &mut t.keys {
                    k.1 = f32::from(k.1 != 0.0);
                }
                t
            })
            .unwrap_or_default();
        out.push(ParticleEmitterDef {
            color_index: le_u16(bytes, e + 0x2a),
            flags,
            position: le_vec3(bytes, e + 0x08),
            bone: le_u16(bytes, e + 0x14),
            shape,
            spline,
            geometry_model: model_path(0x18),
            recursion_model: model_path(0x20),
            blend: blend_of(bytes[e + 0x28]),
            lit: lit_of(flags, bytes[e + 0x28]),
            texture,
            tile_rows: tiles.0,
            tile_cols: tiles.1,
            timing: EmitTiming::bake(&rate, &enabled, &seq_slots, &gseq),
            params: EmitParams::bake(
                [
                    &params[0], &params[1], &params[2], &params[3], &params[4], &params[5],
                    &params[6], &params[7], &params[8],
                ],
                &seq_slots,
                &gseq,
            ),
            lifespan_vary: le_f32(bytes, e + 0xac),
            rate_vary: le_f32(bytes, e + 0xc4),
            scale_vary: [le_f32(bytes, e + 0x134), le_f32(bytes, e + 0x138)],
            tail_time: le_f32(bytes, e + 0x15c),
            twinkle_speed: le_f32(bytes, e + 0x160),
            twinkle_percent: le_f32(bytes, e + 0x164),
            twinkle_min: le_f32(bytes, e + 0x168),
            twinkle_max: le_f32(bytes, e + 0x16c),
            inherit_scale: le_f32(bytes, e + 0x170),
            drag: le_f32(bytes, e + 0x174),
            base_spin: le_f32(bytes, e + 0x178),
            base_spin_vary: le_f32(bytes, e + 0x17c),
            spin: le_f32(bytes, e + 0x180),
            spin_vary: le_f32(bytes, e + 0x184),
            angular_velocity_min: le_vec3(bytes, e + 0x188),
            angular_velocity_max: le_vec3(bytes, e + 0x194),
            follow_speed1: le_f32(bytes, e + 0x1b0),
            follow_scale1: le_f32(bytes, e + 0x1b4),
            follow_speed2: le_f32(bytes, e + 0x1b8),
            follow_scale2: le_f32(bytes, e + 0x1bc),
            over_life,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_one_is_the_unlit_flag_and_mod_blends_never_light() {
        assert!(
            lit_of(0x0002, 2),
            "an emitter that CLEARS 0x1 takes the light"
        );
        assert!(!lit_of(0x0029, 4), "0x1 set: unlit, whatever the blend");
        assert!(!lit_of(0x0021, 2));
        for blend in 0u8..=4 {
            assert!(lit_of(0x0000, blend), "blend {blend} lights");
        }
        assert!(!lit_of(0x0000, 5), "Mod never lights");
        assert!(!lit_of(0x0000, 6), "Mod2x never lights");
        // A byte past the 7-entry table folds to `Opaque`, which lights.
        assert!(lit_of(0x0000, 7));
    }

    fn track<V: Copy>(times: &[i16], values: &[V]) -> LifeTrack<V> {
        LifeTrack {
            times: times.iter().map(|&t| f32::from(t) / 32767.0).collect(),
            values: values.to_vec(),
        }
    }

    /// `0x9793b0`'s three arms: two keys span the life whatever their times; three split at the
    /// middle key (Northrend's colour at 16186, its alpha at 6553); more search their segment.
    #[test]
    fn life_tracks_locate_by_the_reference_arms() {
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let two = track(&[5000, 9000], &[0.0, 1.0]);
        assert_eq!(two.sample(0.25, lerp), Some(0.25), "times ignored");
        let mid = 6553.0 / 32767.0;
        let three = track(&[0, 6553, 32767], &[0.0, 1.0, 0.0]);
        assert!((three.sample(mid * 0.5, lerp).unwrap() - 0.5).abs() < 1e-5);
        assert_eq!(three.sample(mid, lerp), Some(1.0), "the middle key opens B");
        let late = three.sample(mid + (1.0 - mid) * 0.75, lerp).unwrap();
        assert!((late - 0.25).abs() < 1e-5, "{late}");
        let four = track(&[0, 8192, 16384, 32767], &[0.0, 10.0, 20.0, 40.0]);
        let at = |u: f32| four.sample(u, lerp).unwrap();
        assert!((at(0.125) - 5.0).abs() < 1e-3);
        assert!((at(0.375) - 15.0).abs() < 1e-3);
        assert!((at(0.75) - 30.0).abs() < 1e-2);
        assert_eq!(
            track(&[0, 32767], &[7.0]).sample(0.9, lerp),
            Some(7.0),
            "one value"
        );
        assert_eq!(track::<f32>(&[], &[]).sample(0.5, lerp), None);
    }

    /// Cells round to nearest, ties to even, as `fistp` does; colours land on bytes.
    #[test]
    fn over_life_rounds_cells_and_colour_bytes() {
        let ol = OverLife {
            color: track(&[0, 32767], &[[0.0, 100.0, 255.0], [255.0, 101.0, 255.0]]),
            alpha: track(&[0, 32767], &[1.0, 0.0]),
            scale: track(&[0, 32767], &[[1.0, 2.0], [3.0, 6.0]]),
            head_cell: track(&[0, 32767], &[0, 3]),
            tail_cell: LifeTrack::default(),
        };
        let s = ol.sample(0.5);
        assert_eq!(s.color[0], 128.0 / 255.0, "127.5 rounds to the even byte");
        assert_eq!(s.color[1], 100.0 / 255.0, "100.5 rounds to the even byte");
        assert_eq!(s.color[3], 0.5);
        assert_eq!(s.size, [2.0, 4.0], "x and y ride their own keys");
        assert_eq!(s.head_cell, Some(2), "1.5 rounds to 2");
        assert_eq!(s.tail_cell, 0, "an unkeyed tail draws cell 0");
        assert_eq!(ol.sample(1.0 / 6.0).head_cell, Some(0), "0.5 rounds to 0");
    }

    /// Straight segments of 1 and 3 yd (control points at thirds, so each cubic is linear) put
    /// the boundary at t = 0.25, not 0.5.
    #[test]
    fn spline_chain_is_arc_length_parameterized() {
        let x = |v: f32| [v, 0.0, 0.0];
        let s = SplineData::new(vec![
            x(0.0),
            x(1.0 / 3.0),
            x(2.0 / 3.0),
            x(1.0), // segment 0: 1 yd
            x(2.0),
            x(3.0),
            x(4.0), // segment 1: 3 yd
        ])
        .expect("3K+1 chain");
        assert!((s.eval(0.25)[0] - 1.0).abs() < 1e-4, "boundary at arc 1/4");
        assert!((s.eval(0.625)[0] - 2.5).abs() < 1e-4, "mid of segment 1");
        assert_eq!(s.eval(-0.5), [0.0, 0.0, 0.0], "clamp to first point");
        assert_eq!(s.eval(1.5), [4.0, 0.0, 0.0], "clamp to last point");
        let tan = s.tangent(0.1);
        assert!(tan[0] > 0.0 && tan[1] == 0.0 && tan[2] == 0.0, "+X tangent");
        assert!(SplineData::new(vec![x(0.0)]).is_none(), "below one segment");
    }

    #[test]
    fn a_hostile_emitter_count_yields_nothing() {
        use benilla_m2::fixture::{self, Model};
        let mut m = Model::new();
        m.put_arr(HDR_PARTICLES, u32::MAX as usize, 0);
        let model = m.parse(&fixture::empty_skin()).unwrap();
        assert!(parse_m2_particle_emitters(&model).unwrap().is_empty());
        // A plausible count whose table the file cannot hold is refused too.
        let end = m.b.len() as u32;
        m.put_arr(HDR_PARTICLES, 1, end);
        let model = m.parse(&fixture::empty_skin()).unwrap();
        assert!(parse_m2_particle_emitters(&model).unwrap().is_empty());
    }

    /// The WotLK login scene's Northrend emitters: forty of them,
    /// their over-life channels split at the issue's measured midpoints.
    #[test]
    fn the_northrend_glue_scene_authors_forty_emitters() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(
            &chain,
            "Interface\\Glues\\Models\\UI_MainMenu_Northrend\\UI_MainMenu_Northrend.m2",
        )
        .expect("read the Northrend glue scene");
        let defs = parse_m2_particle_emitters(&bytes).expect("parse emitters");
        assert_eq!(defs.len(), 40);
        assert!(
            defs.iter().all(|d| d.texture.is_some()),
            "every emitter has a texture"
        );
        let mid = |t: &[f32]| (t.len() == 3).then(|| (t[1] * 32767.0).round() as i32);
        assert!(
            defs.iter()
                .any(|d| mid(&d.over_life.color.times) == Some(16186)
                    && mid(&d.over_life.alpha.times) == Some(6553)),
            "colour midpoint 16186 beside alpha midpoint 6553"
        );
        assert!(
            defs.iter().all(|d| d.draws_head() || d.draws_tail()),
            "every emitter draws something"
        );
        assert!(
            defs.iter().any(|d| d.timing.peak_rate() > 0.0),
            "the scene emits"
        );
    }

    #[test]
    fn the_waterfall_emitter_is_lit() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(
            &chain,
            "World\\Azeroth\\Elwynn\\PassiveDoodads\\Waterfall\\ElwynnTallWaterfall01.m2",
        )
        .expect("read ElwynnTallWaterfall01.m2");
        let defs = parse_m2_particle_emitters(&bytes).expect("parse emitters");
        assert_eq!(defs.len(), 1);
        assert_eq!(
            defs[0].flags, 0x20002,
            "authored flag word: 0x2 and the head quad"
        );
        assert!(defs[0].draws_head() && !defs[0].draws_tail());
        assert_eq!(defs[0].blend, ParticleBlend::Alpha);
        assert!(defs[0].lit, "the spray sheet is shaded by the world");
    }

    /// `BloodSpurt.m2`, the melee impact flash: emitters 1 to 3 are keyed bursts.
    #[test]
    fn real_blood_spurt_emitters_are_keyed_bursts() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let bytes = crate::read_m2(&chain, "Particles\\BloodSpurts\\BloodSpurt.m2")
            .expect("read BloodSpurt.m2");
        let defs = parse_m2_particle_emitters(&bytes).expect("parse emitters");
        assert_eq!(defs.len(), 4);
        // Emitter 0, the red spray: 100/s at t = 0.
        assert_eq!(defs[0].timing.rate(None, 0.0, 0.0), 100.0);
        // Emitter 3, the starflash: first key 0, 20/s within 67..100 ms, 0 again by 133 ms.
        let flash = &defs[3];
        assert!(flash
            .texture
            .as_deref()
            .is_some_and(|t| t.to_ascii_uppercase().contains("STARFLASH")));
        assert_eq!(flash.blend, ParticleBlend::Add);
        assert_eq!(flash.timing.rate(None, 0.0, 0.0), 0.0, "silent at t=0");
        assert_eq!(flash.timing.peak_rate(), 20.0);
        assert_eq!(flash.timing.rate(None, 0.080, 0.0), 20.0);
        assert_eq!(flash.timing.rate(None, 0.200, 0.0), 0.0);
        // Emitters 1 and 2, the glowball droplets: the same burst shape, peaking at 200.
        assert_eq!(defs[1].timing.peak_rate(), 200.0);
        assert_eq!(defs[2].timing.peak_rate(), 200.0);
        // The spray's enabled track cuts emission at 500 ms, where its rate track goes negative.
        assert!(defs[0].timing.emitting(None, 0.4, 0.0));
        assert!(!defs[0].timing.emitting(None, 0.6, 0.0));
    }

    /// `Feint_Impact_Chest.m2`: the plume (e0) and crescents (e3) set flag 0x8000, one puff at
    /// their 67 ms key; `Eviscerate_Cast_Hands.m2` has the same 1.6 s shape with the flag clear.
    #[test]
    fn real_feint_impact_authors_burst_emitters() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let impact = parse_m2_particle_emitters(
            &crate::read_m2(&chain, "Spells\\Feint_Impact_Chest.m2")
                .expect("read Feint_Impact_Chest.m2"),
        )
        .expect("parse emitters");
        assert_eq!(impact.len(), 4);
        assert!(impact[0].burst(), "plume is a one-shot burst");
        assert!(!impact[1].burst(), "lava pours (enabled window 0–333 ms)");
        assert!(!impact[2].burst(), "dust pours (enabled window 67–200 ms)");
        assert!(impact[3].burst(), "crescents are a one-shot burst");
        // A step rate: 0 before the 67 ms key, its full value from it on.
        assert_eq!(impact[0].timing.rate(None, 0.050, 0.0), 0.0);
        assert_eq!(impact[0].timing.rate(None, 0.067, 0.0), 30.0);
        assert_eq!(impact[0].timing.rate(None, 1.500, 0.0), 30.0);
        let cast = parse_m2_particle_emitters(
            &crate::read_m2(&chain, "Spells\\Eviscerate_Cast_Hands.m2")
                .expect("read Eviscerate_Cast_Hands.m2"),
        )
        .expect("parse emitters");
        assert!(
            cast.iter().all(|e| !e.burst()),
            "the cast-hands flame is continuous — same asset shape, opposite flag"
        );
    }

    /// `FlameStrike_Area.m2`'s four fire columns: type-3 chains of 16 and 19 points, no spin.
    #[test]
    fn real_flamestrike_authors_spline_chains() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let defs = parse_m2_particle_emitters(
            &crate::read_m2(&chain, "Spells\\FlameStrike_Area.m2")
                .expect("read FlameStrike_Area.m2"),
        )
        .expect("parse emitters");
        let splines: Vec<_> = defs
            .iter()
            .filter(|d| d.shape == ParticleShape::Spline)
            .collect();
        assert_eq!(splines.len(), 4, "the four fire-column emitters");
        for d in &splines {
            let s = d.spline.as_ref().expect("chain parses");
            assert_eq!(s.points.len() % 3, 1, "3K+1 control points");
            assert!(s.points.len() >= 16);
            let now = d.params.sample(None, 0.0, 0.0);
            assert_eq!(now.area_length, 0.0, "tMin");
            assert_eq!(now.area_width, 1.0, "tMax");
            assert_eq!(now.vertical_range, 0.0, "no tangent spin");
            assert!(d.burst(), "one puff of standing flames");
            assert!(
                (5.0..15.0).contains(&s.points[0][2]),
                "z {}",
                s.points[0][2]
            );
            assert_eq!(s.eval(0.0), s.points[0], "t=0 is the first point");
        }
    }

    /// Fireball (spell 133): the cast flash is on for its clip's first 200 ms and the impact plume
    /// ramps 0 → 60 over 133 ms of clip time, though seq 0 spans [1000, 2600] of the file.
    #[test]
    #[ignore = "3.3.5a's MoltenBlast_Impact_Chest keys its plume differently; re-measure (#51)"]
    fn real_fireball_effects_rebase_to_clip_time() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let cast = parse_m2_particle_emitters(
            &crate::read_m2(&chain, "Spells\\Fire_Cast_Hand.m2").expect("read Fire_Cast_Hand.m2"),
        )
        .expect("parse emitters");
        assert_eq!(cast.len(), 3);
        for em in &cast {
            assert!(
                em.timing.emitting(None, 0.1, 0.0),
                "the flash is ON at its start"
            );
            assert!(
                !em.timing.emitting(None, 0.25, 0.0),
                "and OFF from 200 ms — the 1.0 s clip does not burn through"
            );
        }
        let impact = parse_m2_particle_emitters(
            &crate::read_m2(&chain, "Spells\\MoltenBlast_Impact_Chest.m2")
                .expect("read MoltenBlast_Impact_Chest.m2"),
        )
        .expect("parse emitters");
        assert_eq!(impact.len(), 6);
        assert_eq!(impact[0].timing.rate(None, 0.0, 0.0), 0.0);
        assert_eq!(
            impact[0].timing.rate(None, 0.133, 0.0),
            60.0,
            "the plume bursts at impact"
        );
        assert!(impact[1].timing.emitting(None, 0.1, 0.0));
        assert!(
            !impact[1].timing.emitting(None, 0.4, 0.0),
            "shockwave window is 300 ms"
        );
        assert!(
            !impact[4].timing.emitting(None, 0.05, 0.0),
            "smoke starts staggered…"
        );
        assert!(impact[4].timing.emitting(None, 0.2, 0.0));
        assert!(
            !impact[4].timing.emitting(None, 0.7, 0.0),
            "…and ends by 567 ms"
        );
    }
}
