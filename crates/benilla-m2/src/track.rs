//! The M2 track type and the cubic track sampler.

/// One M2 track on the model's one timeline ([`crate::Tracks264`]): `interp`, `gseq`, a key window
/// per sequence and the keys. A sequence track keys inside each sequence's band, a `gseq` track
/// loops on its global sequence.
#[derive(Clone, Debug)]
pub struct M2Track<V> {
    /// `0` step (the previous key), nonzero linear: the scalar sampler's two-way test (`0x71af20`).
    pub interp: u16,
    /// Global-sequence index, `0xffff` = an ordinary sequence-timeline track.
    pub gseq: u16,
    /// The per-sequence key-index window `(lo, hi)`, one per sequence in file order, which the
    /// reference's key search reads before any timestamp (`0x713d50`; empty means search all
    /// keys). Brackets, not the clip's keys: `hi` often lands in a later sequence's band. A band
    /// with no keys resolves to `keys[lo]`.
    pub ranges: Vec<(u32, u32)>,
    /// `(absolute ms, value)` keys, file order (time-ascending within a band).
    pub keys: Vec<(u32, V)>,
}

impl<V> Default for M2Track<V> {
    fn default() -> Self {
        Self {
            interp: 0,
            gseq: 0,
            ranges: Vec::new(),
            keys: Vec::new(),
        }
    }
}

/// A scalar track: the M2Color alpha and the M2TextureWeight weight.
pub type M2ScalarTrack = M2Track<f32>;
/// A `C3Vector` track: the M2Color RGB and the texture-transform translation and scaling.
pub type M2Vec3Track = M2Track<[f32; 3]>;
/// A quaternion track (4×f32): the texture-transform rotation.
pub type M2QuatTrack = M2Track<[f32; 4]>;

impl<V: Copy + PartialEq> M2Track<V> {
    /// The track's value when every key holds the same one.
    pub fn constant(&self) -> Option<V> {
        let (_, first) = *self.keys.first()?;
        self.keys.iter().all(|&(_, v)| v == first).then_some(first)
    }
}

/// One key of a cubic track: the value, then the in and out tangents, each `sizeof(T)` apart. The
/// wide key is the stride whatever `interp` says: the reference addresses keys as `k*0x24`
/// (`0x716b51`) or `k*0xc` (`0x7173cc`) before dispatching, and step and linear read its `value`
/// (`Cameras\FlyByDwarf.m2`'s roll track is `interp = 0` with 12-byte keys).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M2SplineKey<V> {
    pub value: V,
    pub in_tan: V,
    pub out_tan: V,
}

/// A cubic `C3Vector` track: the M2Camera position and target.
pub type M2Vec3SplineTrack = M2Track<M2SplineKey<[f32; 3]>>;
/// A cubic scalar track: the M2Camera roll.
pub type M2ScalarSplineTrack = M2Track<M2SplineKey<f32>>;

/// A value a cubic track can hold.
pub trait CubicValue: Copy {
    /// `w0·p0 + w1·p1 + w2·p2 + w3·p3`.
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self;
    /// The linear leg in the reference's form `a + (b − a)·t` (`0x716cf1`), which rounds
    /// differently in f32 from `(1−t)·a + t·b`.
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl CubicValue for f32 {
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self {
        w[0] * p[0] + w[1] * p[1] + w[2] * p[2] + w[3] * p[3]
    }
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl CubicValue for [f32; 3] {
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self {
        std::array::from_fn(|j| w[0] * p[0][j] + w[1] * p[1][j] + w[2] * p[2][j] + w[3] * p[3][j])
    }
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t)
    }
}

impl<V: CubicValue> M2Track<M2SplineKey<V>> {
    /// Sample at absolute timeline `ms`, clamped at both ends, by the cubic loops' four-way
    /// `interp` dispatch (the bone loops have only two):
    ///
    /// - `0` step: `value[k0]` (`0x716b5f`).
    /// - `1` linear (`0x716cf1`).
    /// - `2` Bézier over `{value[k0], outTan[k0], inTan[k1], value[k1]}` (`0x716c41`), what every
    ///   `Cameras\*.m2` fly-by authors for position and target.
    /// - `3` Hermite over `value[k0]`, `outTan[k0]`, `value[k1]`, `inTan[k1]` (`0x716b9e`).
    ///
    /// A span takes the out tangent of the key it leaves and the in tangent of the key it enters.
    pub fn sample_ms(&self, ms: u32) -> Option<V> {
        let first = self.keys.first()?;
        let last = self.keys.last()?;
        if ms <= first.0 {
            return Some(first.1.value);
        }
        if ms >= last.0 {
            return Some(last.1.value);
        }
        // The key pair bracketing `ms` (keys are time-ascending within a band).
        let k1 = self.keys.partition_point(|&(t, _)| t <= ms);
        let (t0, a) = self.keys[k1 - 1];
        let (t1, b) = self.keys[k1];
        let t = if t1 > t0 {
            (ms - t0) as f32 / (t1 - t0) as f32
        } else {
            0.0
        };
        let (t2, t3) = (t * t, t * t * t);
        Some(match self.interp {
            0 => a.value,
            1 => V::lerp(a.value, b.value, t),
            2 => V::combine(
                [
                    (1.0 - t) * (1.0 - t) * (1.0 - t),
                    3.0 * t * (1.0 - t) * (1.0 - t),
                    3.0 * t2 * (1.0 - t),
                    t3,
                ],
                [a.value, a.out_tan, b.in_tan, b.value],
            ),
            _ => V::combine(
                [
                    2.0 * t3 - 3.0 * t2 + 1.0,
                    t3 - 2.0 * t2 + t,
                    3.0 * t2 - 2.0 * t3,
                    t3 - t2,
                ],
                [a.value, a.out_tan, b.value, b.in_tan],
            ),
        })
    }
}
