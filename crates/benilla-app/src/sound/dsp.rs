//! `Sound_EnableDSPEffects`: the named DSP chains of `SoundFilter.dbc`, which the reference builds
//! at sound init (`0x4ce7c0`) and attaches to a sound whose kit properties name one (`0x4ce250`).
//! Every shipped chain is a Death Knight's voice, `"Death Knight <race> <Male|Female>"` (the
//! reference's player filter name), put on a Death Knight player's vocals
//! ([`VoiceFilters::for_unit`]).
//!
//! The stages are FMOD's built-in DSPs, run here from their documented parameters (the reference's
//! DSP factory, `0x4cdf70`, hands `params[i + 1]` to parameter `i`):
//!
//! - pitch shift: pitch, a 512-point FFT, overlap 4;
//! - low and high pass: cutoff Hz and resonance, a biquad with Q = resonance;
//! - chorus: dry, three wet taps whose delay LFOs sit 90° apart, delay ms, rate Hz, depth, feedback;
//! - echo: delay ms, decay, dry, wet;
//! - volume: the chain's gain.
//!
//! An element whose `params[0]` is 0 is bypassed.

use std::f32::consts::PI;

use benilla_formats::{filter_type, SoundFilterCatalog, SoundFilterElem};
use benilla_protocol::ObjectFields;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::Frame;
use std::collections::HashMap;

/// The class byte of a Death Knight (`UNIT_FIELD_BYTES_0` byte 1).
const CLASS_DEATH_KNIGHT: u8 = 6;

/// One chain, by its index in [`VoiceFilters`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VoiceFilter(u16);

/// The chains the engine built at init, and which body voices which.
#[derive(Default)]
pub(crate) struct VoiceFilters {
    chains: Vec<Vec<SoundFilterElem>>,
    /// `(race, sex)` → the Death Knight chain for that body.
    by_body: HashMap<(u8, u8), VoiceFilter>,
}

impl VoiceFilters {
    /// Index the Death Knight chains by body: `race_file` gives each race's `ChrRaces` file
    /// string, the name's middle word (the reference's race file-name table).
    pub(crate) fn build(
        catalog: &SoundFilterCatalog,
        race_file: impl Fn(u8) -> Option<String>,
    ) -> Self {
        let mut filters = Self::default();
        for race in 1..=u8::MAX {
            let Some(file) = race_file(race) else {
                continue;
            };
            for (sex, word) in [(0u8, "Male"), (1u8, "Female")] {
                let Some(chain) = catalog.chain(&format!("Death Knight {file} {word}")) else {
                    continue;
                };
                let id = VoiceFilter(filters.chains.len() as u16);
                filters.chains.push(chain.to_vec());
                filters.by_body.insert((race, sex), id);
            }
        }
        filters
    }

    /// The chain a unit's vocals take: a Death Knight's, by race and sex
    /// (`DK_GetPlayerFilterName`: `"Female"` for a nonzero sex byte), else none.
    pub(crate) fn for_unit(&self, fields: &ObjectFields) -> Option<VoiceFilter> {
        if fields.unit_class() != Some(CLASS_DEATH_KNIGHT) {
            return None;
        }
        let race = fields.unit_race()?;
        let sex = u8::from(fields.unit_gender()? != 0);
        self.by_body.get(&(race, sex)).copied()
    }

    pub(crate) fn chain(&self, id: VoiceFilter) -> Option<&[SoundFilterElem]> {
        self.chains.get(usize::from(id.0)).map(Vec::as_slice)
    }
}

/// Builds one chain's stages as a single effect.
pub(crate) struct ChainBuilder(pub(crate) Vec<SoundFilterElem>);

impl EffectBuilder for ChainBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        let stages = self
            .0
            .iter()
            .filter(|e| e.enabled())
            .filter_map(Stage::new)
            .collect();
        (Box::new(DspChain { stages }), ())
    }
}

struct DspChain {
    stages: Vec<Stage>,
}

impl Effect for DspChain {
    fn init(&mut self, sample_rate: u32, _internal_buffer_size: usize) {
        for s in &mut self.stages {
            s.init(sample_rate as f32);
        }
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        for s in &mut self.stages {
            s.init(sample_rate as f32);
        }
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        for s in &mut self.stages {
            s.process(input);
        }
    }
}

enum Stage {
    Pitch(Box<[PitchShift; 2]>, f32),
    Biquad(Biquad),
    Chorus(Chorus),
    Echo(Echo),
    Gain(f32),
}

impl Stage {
    fn new(e: &SoundFilterElem) -> Option<Self> {
        let p = &e.params;
        Some(match e.filter_type {
            filter_type::PITCH_SHIFT => {
                Stage::Pitch(Box::new([PitchShift::new(), PitchShift::new()]), p[1])
            }
            filter_type::LOW_PASS => Stage::Biquad(Biquad::new(false, p[1], p[2])),
            filter_type::HIGH_PASS => Stage::Biquad(Biquad::new(true, p[1], p[2])),
            filter_type::CHORUS => Stage::Chorus(Chorus {
                dry: p[1],
                wet: [p[2], p[3], p[4]],
                delay_ms: p[5],
                rate_hz: p[6],
                depth: p[7],
                feedback: p[8],
                ..Chorus::default()
            }),
            filter_type::ECHO => Stage::Echo(Echo {
                delay_ms: p[1],
                decay: p[2],
                dry: p[3],
                wet: p[4],
                ..Echo::default()
            }),
            filter_type::VOLUME => Stage::Gain(p[1]),
            _ => return None,
        })
    }

    fn init(&mut self, rate: f32) {
        match self {
            Stage::Pitch(ch, _) => ch.iter_mut().for_each(PitchShift::reset),
            Stage::Biquad(b) => b.init(rate),
            Stage::Chorus(c) => c.init(rate),
            Stage::Echo(e) => e.init(rate),
            Stage::Gain(_) => {}
        }
    }

    fn process(&mut self, frames: &mut [Frame]) {
        match self {
            Stage::Pitch(ch, pitch) => {
                let pitch = *pitch;
                let [left, right] = &mut **ch;
                for f in frames {
                    f.left = left.next(f.left, pitch);
                    f.right = right.next(f.right, pitch);
                }
            }
            Stage::Biquad(b) => frames.iter_mut().for_each(|f| *f = b.next(*f)),
            Stage::Chorus(c) => frames.iter_mut().for_each(|f| *f = c.next(*f)),
            Stage::Echo(e) => frames.iter_mut().for_each(|f| *f = e.next(*f)),
            Stage::Gain(g) => frames.iter_mut().for_each(|f| *f *= *g),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Delay line

/// A stereo delay line read at a fractional delay.
#[derive(Default)]
struct DelayLine {
    buf: Vec<Frame>,
    at: usize,
}

impl DelayLine {
    fn init(&mut self, frames: usize) {
        self.buf = vec![Frame::ZERO; frames.max(2)];
        self.at = 0;
    }

    fn push(&mut self, f: Frame) {
        self.buf[self.at] = f;
        self.at = (self.at + 1) % self.buf.len();
    }

    /// The frame `delay` frames before the last push (0 is the last push), linearly interpolated.
    fn read(&self, delay: f32) -> Frame {
        let len = self.buf.len();
        let delay = delay.clamp(0.0, (len - 2) as f32);
        let whole = delay.floor() as usize;
        let frac = delay - whole as f32;
        let a = self.buf[(self.at + 2 * len - 1 - whole) % len];
        let b = self.buf[(self.at + 2 * len - 2 - whole) % len];
        a * (1.0 - frac) + b * frac
    }
}

// ---------------------------------------------------------------------------------------------
// Echo

#[derive(Default)]
struct Echo {
    delay_ms: f32,
    decay: f32,
    dry: f32,
    wet: f32,
    line: DelayLine,
    delay: f32,
}

impl Echo {
    fn init(&mut self, rate: f32) {
        self.delay = (self.delay_ms / 1000.0 * rate).max(1.0);
        self.line.init(self.delay as usize + 2);
    }

    fn next(&mut self, x: Frame) -> Frame {
        // Read before the push: the line's last frame is a frame ago.
        let echoed = self.line.read(self.delay - 1.0);
        self.line.push(x + echoed * self.decay);
        x * self.dry + echoed * self.wet
    }
}

// ---------------------------------------------------------------------------------------------
// Chorus

#[derive(Default)]
struct Chorus {
    dry: f32,
    wet: [f32; 3],
    delay_ms: f32,
    rate_hz: f32,
    depth: f32,
    feedback: f32,
    line: DelayLine,
    rate: f32,
    phase: f32,
    last: Frame,
}

impl Chorus {
    fn init(&mut self, rate: f32) {
        self.rate = rate;
        let max = self.delay_ms * (1.0 + self.depth) / 1000.0 * rate;
        self.line.init(max as usize + 3);
        self.phase = 0.0;
        self.last = Frame::ZERO;
    }

    fn next(&mut self, x: Frame) -> Frame {
        self.line.push(x + self.last * self.feedback);
        let base = self.delay_ms / 1000.0 * self.rate;
        let mut out = x * self.dry;
        for (tap, &wet) in self.wet.iter().enumerate() {
            let lfo = (self.phase + tap as f32 * PI / 2.0).sin();
            let tapped = self.line.read(base * (1.0 + self.depth * lfo));
            if tap == 0 {
                self.last = tapped;
            }
            out += tapped * wet;
        }
        self.phase = (self.phase + 2.0 * PI * self.rate_hz / self.rate) % (2.0 * PI);
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Biquad

/// An RBJ low- or high-pass at Q = resonance.
struct Biquad {
    high: bool,
    cutoff: f32,
    q: f32,
    b: [f32; 3],
    a: [f32; 2],
    x: [Frame; 2],
    y: [Frame; 2],
}

impl Biquad {
    fn new(high: bool, cutoff: f32, resonance: f32) -> Self {
        Self {
            high,
            cutoff,
            q: resonance.max(0.1),
            b: [1.0, 0.0, 0.0],
            a: [0.0, 0.0],
            x: [Frame::ZERO; 2],
            y: [Frame::ZERO; 2],
        }
    }

    fn init(&mut self, rate: f32) {
        let w = 2.0 * PI * (self.cutoff / rate).clamp(1e-4, 0.49);
        let alpha = w.sin() / (2.0 * self.q);
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let (b0, b1) = if self.high {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        self.b = [b0 / a0, b1 / a0, b0 / a0];
        self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
        self.x = [Frame::ZERO; 2];
        self.y = [Frame::ZERO; 2];
    }

    fn next(&mut self, x: Frame) -> Frame {
        let y = x * self.b[0] + self.x[0] * self.b[1] + self.x[1] * self.b[2]
            - self.y[0] * self.a[0]
            - self.y[1] * self.a[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

// ---------------------------------------------------------------------------------------------
// Pitch shift

/// FMOD's pitch shifter at the size the reference sets (its DSP factory, parameter 1 = 512).
const FFT: usize = 512;
/// FMOD's default overlap.
const OVERLAP: usize = 4;
const STEP: usize = FFT / OVERLAP;

/// A phase-vocoder pitch shifter for one channel: each hop of [`STEP`] samples takes a windowed
/// [`FFT`]-point frame, moves every bin's magnitude and true frequency up or down by the pitch
/// factor and overlap-adds the resynthesis. Latency is `FFT - STEP` samples.
struct PitchShift {
    input: [f32; FFT],
    output: [f32; FFT],
    accum: [f32; 2 * FFT],
    last_phase: [f32; FFT / 2 + 1],
    sum_phase: [f32; FFT / 2 + 1],
    re: [f32; FFT],
    im: [f32; FFT],
    magn: [f32; FFT / 2 + 1],
    freq: [f32; FFT / 2 + 1],
    rover: usize,
}

impl PitchShift {
    fn new() -> Self {
        Self {
            input: [0.0; FFT],
            output: [0.0; FFT],
            accum: [0.0; 2 * FFT],
            last_phase: [0.0; FFT / 2 + 1],
            sum_phase: [0.0; FFT / 2 + 1],
            re: [0.0; FFT],
            im: [0.0; FFT],
            magn: [0.0; FFT / 2 + 1],
            freq: [0.0; FFT / 2 + 1],
            rover: FFT - STEP,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn next(&mut self, x: f32, pitch: f32) -> f32 {
        let latency = FFT - STEP;
        self.input[self.rover] = x;
        let y = self.output[self.rover - latency];
        self.rover += 1;
        if self.rover >= FFT {
            self.rover = latency;
            self.frame(pitch);
        }
        y
    }

    /// One analysis-shift-synthesis hop over the last [`FFT`] input samples.
    fn frame(&mut self, pitch: f32) {
        let expected = 2.0 * PI * STEP as f32 / FFT as f32;
        for k in 0..FFT {
            self.re[k] = self.input[k] * window(k);
            self.im[k] = 0.0;
        }
        fft(&mut self.re, &mut self.im, false);
        // Analysis: each bin's magnitude and true frequency in bins.
        let mut ana_magn = [0.0f32; FFT / 2 + 1];
        let mut ana_freq = [0.0f32; FFT / 2 + 1];
        for k in 0..=FFT / 2 {
            let (re, im) = (self.re[k], self.im[k]);
            let phase = im.atan2(re);
            let mut delta = phase - self.last_phase[k] - k as f32 * expected;
            self.last_phase[k] = phase;
            delta -= 2.0 * PI * (delta / (2.0 * PI)).round();
            ana_magn[k] = 2.0 * (re * re + im * im).sqrt();
            ana_freq[k] = k as f32 + delta * OVERLAP as f32 / (2.0 * PI);
        }
        // Shift.
        self.magn.fill(0.0);
        self.freq.fill(0.0);
        for k in 0..=FFT / 2 {
            let to = (k as f32 * pitch).round() as usize;
            if to <= FFT / 2 {
                self.magn[to] += ana_magn[k];
                self.freq[to] = ana_freq[k] * pitch;
            }
        }
        // Synthesis.
        for k in 0..=FFT / 2 {
            let delta = (self.freq[k] - k as f32) * 2.0 * PI / OVERLAP as f32 + k as f32 * expected;
            self.sum_phase[k] = (self.sum_phase[k] + delta) % (2.0 * PI);
            let (s, c) = self.sum_phase[k].sin_cos();
            self.re[k] = self.magn[k] * c;
            self.im[k] = self.magn[k] * s;
        }
        for k in FFT / 2 + 1..FFT {
            self.re[k] = 0.0;
            self.im[k] = 0.0;
        }
        fft(&mut self.re, &mut self.im, true);
        let scale = 2.0 / (FFT as f32 / 2.0 * OVERLAP as f32);
        for k in 0..FFT {
            self.accum[k] += window(k) * self.re[k] * scale;
        }
        self.output[..STEP].copy_from_slice(&self.accum[..STEP]);
        self.accum.copy_within(STEP..STEP + FFT, 0);
        self.input.copy_within(STEP.., 0);
    }
}

/// The Hann window over one frame.
fn window(k: usize) -> f32 {
    0.5 - 0.5 * (2.0 * PI * k as f32 / FFT as f32).cos()
}

/// An in-place radix-2 FFT over [`FFT`] points; `inverse` flips the twiddles' sign and does not
/// scale.
fn fft(re: &mut [f32; FFT], im: &mut [f32; FFT], inverse: bool) {
    let bits = FFT.trailing_zeros();
    for i in 0..FFT {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= FFT {
        let angle = sign * 2.0 * PI / len as f32;
        let (ws, wc) = angle.sin_cos();
        for start in (0..FFT).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wc - ci * ws, cr * ws + ci * wc);
            }
        }
        len *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(filter_type: u32, params: [f32; 9]) -> SoundFilterElem {
        SoundFilterElem {
            filter_type,
            params,
        }
    }

    /// The dominant frequency of `samples` at 48 kHz, by zero crossings.
    fn crossings_hz(samples: &[f32]) -> f32 {
        let n = samples
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        n as f32 * 48_000.0 / samples.len() as f32
    }

    fn run(builder: ChainBuilder, input: &[f32]) -> Vec<f32> {
        let (mut effect, ()) = builder.build();
        effect.init(48_000, 128);
        let info = kira::info::MockInfoBuilder::new().build();
        let mut out = Vec::with_capacity(input.len());
        for block in input.chunks(128) {
            let mut frames: Vec<Frame> = block.iter().map(|&s| Frame::from_mono(s)).collect();
            effect.process(&mut frames, 1.0 / 48_000.0, &info);
            out.extend(frames.iter().map(|f| f.left));
        }
        out
    }

    fn sine(hz: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * hz * i as f32 / 48_000.0).sin() * 0.5)
            .collect()
    }

    /// A pitch of 0.9 lowers a 1 kHz tone by a tenth.
    #[test]
    fn the_pitch_shifter_moves_a_tone_by_its_factor() {
        let tone = sine(1000.0, 48_000);
        let out = run(
            ChainBuilder(vec![elem(
                filter_type::PITCH_SHIFT,
                [1.0, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            )]),
            &tone,
        );
        let hz = crossings_hz(&out[4_800..]);
        assert!((hz - 900.0).abs() < 30.0, "{hz}");
    }

    /// The echo is the dry level plus the wet copy `delay` ms later.
    #[test]
    fn the_echo_repeats_an_impulse_after_its_delay() {
        let mut impulse = vec![0.0; 9_600];
        impulse[0] = 1.0;
        let out = run(
            ChainBuilder(vec![elem(
                filter_type::ECHO,
                [1.0, 50.0, 0.0, 0.25, 1.0, 0.0, 0.0, 0.0, 0.0],
            )]),
            &impulse,
        );
        assert!((out[0] - 0.25).abs() < 1e-6);
        assert!((out[2_400] - 1.0).abs() < 1e-3, "{}", out[2_400]);
        assert!(out[1..2_390].iter().all(|s| s.abs() < 1e-6));
    }

    /// A low pass at 1 kHz keeps a 200 Hz tone and takes most of a 10 kHz one.
    #[test]
    fn the_low_pass_keeps_the_bottom() {
        let lp = || {
            ChainBuilder(vec![elem(
                filter_type::LOW_PASS,
                [1.0, 1000.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            )])
        };
        let peak = |s: &[f32]| s[4_800..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak(&run(lp(), &sine(200.0, 9_600))) > 0.45);
        assert!(peak(&run(lp(), &sine(10_000.0, 9_600))) < 0.05);
    }

    /// A bypassed element (`params[0] == 0`) leaves the signal alone; so does an unknown type.
    #[test]
    fn a_bypassed_element_is_silent_in_the_chain() {
        let tone = sine(440.0, 2_048);
        let out = run(
            ChainBuilder(vec![
                elem(
                    filter_type::ECHO,
                    [0.0, 50.0, 0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
                ),
                elem(99, [1.0; 9]),
            ]),
            &tone,
        );
        assert_eq!(out, tone);
    }

    /// On the install: a Death Knight body of every race and sex has a chain, another class none,
    /// and each chain runs a voice-band tone to a finite, nonsilent signal.
    #[test]
    fn every_death_knight_body_has_a_chain_that_runs() {
        // `UNIT_FIELD_BYTES_0` (race, class, gender) in the 3.3.5a descriptor.
        const FIELD_UNIT_BYTES_0: u16 = 0x24;
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_sound_filter_catalog(&mut chain).expect("filters");
        let races = benilla_formats::CharCreateCatalog::load(&mut chain).expect("races");
        let filters = VoiceFilters::build(&catalog, |r| races.race_file(r).map(str::to_string));
        let body = |race: u32, class: u32, sex: u32| {
            ObjectFields::from_pairs(&[(FIELD_UNIT_BYTES_0, race | class << 8 | sex << 16)])
        };
        for race in [1, 2, 3, 4, 5, 6, 7, 8, 10, 11] {
            assert_eq!(
                filters.for_unit(&body(race, 1, 0)),
                None,
                "a warrior is dry"
            );
            for sex in [0, 1] {
                let id = filters
                    .for_unit(&body(race, u32::from(CLASS_DEATH_KNIGHT), sex))
                    .unwrap_or_else(|| panic!("race {race} sex {sex}: no chain"));
                let elems = filters.chain(id).expect("the id's chain").to_vec();
                let out = run(ChainBuilder(elems), &sine(300.0, 24_000));
                assert!(out.iter().all(|s| s.is_finite()), "race {race} sex {sex}");
                let peak = out[12_000..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!(peak > 0.05, "race {race} sex {sex}: peak {peak}");
            }
        }
    }

    /// The chorus keeps the dry signal and adds its taps.
    #[test]
    fn the_chorus_adds_its_taps_to_the_dry_signal() {
        let mut impulse = vec![0.0; 4_800];
        impulse[0] = 1.0;
        let out = run(
            ChainBuilder(vec![elem(
                filter_type::CHORUS,
                [1.0, 1.0, 0.5, 0.0, 0.0, 10.0, 1.0, 0.0, 0.0],
            )]),
            &impulse,
        );
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert!((out[480] - 0.5).abs() < 0.05, "{}", out[480]);
    }
}
