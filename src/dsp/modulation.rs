//! OWNER: feat/modulation. 2 LFOs, input envelope follower, macros (Dust, Chaos, Age).
//! Runs first each block and rewrites `Snapshot` values before any DSP module reads them.
//!
//! Order per block (README "Modulation and macros"):
//! 1. Macros: Dust, then Age, then Chaos.
//! 2. LFOs and envelope follower: per-target offsets are summed, then applied once
//!    (`offset = source × amount × span`, log space for Hz/cutoff targets) and clamped.
//! 3. LFO and envelope values are published to `VisualState`.
//!
//! Modulation is block-rate: the LFO value is taken at the block start, then the phase
//! advances by `block_len`. The envelope follower runs per sample; its end-of-block value
//! is used.
use super::rng::Rng;
use super::Transport;
use crate::params::{LfoRate, LfoShape, ModTarget, Snapshot};
use crate::shared::{store_f32, VisualState};
use nih_plug::prelude::Enum;
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Target table
// ---------------------------------------------------------------------------

/// Range of a modulatable parameter. Must match `params.rs` exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetRange {
    pub min: f32,
    pub max: f32,
    /// Modulate in log space (Hz, cutoff, time, ratio parameters).
    pub log: bool,
}

const fn lin(min: f32, max: f32) -> Option<TargetRange> {
    Some(TargetRange {
        min,
        max,
        log: false,
    })
}

const fn log(min: f32, max: f32) -> Option<TargetRange> {
    Some(TargetRange {
        min,
        max,
        log: true,
    })
}

/// Range of each `ModTarget`; `None` for `ModTarget::None`.
pub fn target_range(t: ModTarget) -> Option<TargetRange> {
    use ModTarget::*;
    match t {
        None => Option::None,
        GrainSize => log(5.0, 500.0),
        Density => log(1.0, 64.0),
        Position => lin(0.0, 1.0),
        PositionJitter => lin(0.0, 1.0),
        Pitch => lin(-24.0, 24.0),
        PitchJitter => lin(0.0, 12.0),
        ReverseChance => lin(0.0, 1.0),
        Speed => log(0.25, 4.0),
        Spread => lin(0.0, 1.0),
        StutterChance => lin(0.0, 1.0),
        BitDepth => lin(1.0, 16.0),
        SampleRate => log(1000.0, 48000.0),
        WowDepth => lin(0.0, 1.0),
        FlutterDepth => lin(0.0, 1.0),
        Drive => lin(0.0, 1.0),
        TapeAge => lin(0.0, 1.0),
        LowpassCutoff => log(20.0, 20000.0),
        HighpassCutoff => log(20.0, 20000.0),
        ReverbMix => lin(0.0, 1.0),
        DelayMix => lin(0.0, 1.0),
        Mix => lin(0.0, 1.0),
    }
}

/// The `Snapshot` field a `ModTarget` writes to.
pub fn target_field(s: &mut Snapshot, t: ModTarget) -> Option<&mut f32> {
    use ModTarget::*;
    Some(match t {
        None => return Option::None,
        GrainSize => &mut s.grain_size_ms,
        Density => &mut s.density_hz,
        Position => &mut s.position,
        PositionJitter => &mut s.position_jitter,
        Pitch => &mut s.pitch_st,
        PitchJitter => &mut s.pitch_jitter_st,
        ReverseChance => &mut s.reverse_chance,
        Speed => &mut s.speed,
        Spread => &mut s.spread,
        StutterChance => &mut s.stutter_chance,
        BitDepth => &mut s.bit_depth,
        SampleRate => &mut s.sample_rate_hz,
        WowDepth => &mut s.wow_depth,
        FlutterDepth => &mut s.flutter_depth,
        Drive => &mut s.drive,
        TapeAge => &mut s.tape_age,
        LowpassCutoff => &mut s.lowpass_hz,
        HighpassCutoff => &mut s.highpass_hz,
        ReverbMix => &mut s.reverb_mix,
        DelayMix => &mut s.delay_mix,
        Mix => &mut s.mix,
    })
}

/// Apply a normalized offset (±1 = ± full span) to `v`, clamped to the range.
fn apply_offset(v: f32, offset: f32, r: TargetRange) -> f32 {
    let out = if r.log {
        let base = v.clamp(r.min, r.max);
        base * (offset * (r.max / r.min).ln()).exp()
    } else {
        v + offset * (r.max - r.min)
    };
    if out.is_finite() {
        out.clamp(r.min, r.max)
    } else {
        v.clamp(r.min, r.max)
    }
}

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

#[inline]
fn lerp(v: f32, target: f32, a: f32) -> f32 {
    v + (target - v) * a
}

/// Interpolate in log space (for Hz values). Both values must be > 0.
#[inline]
fn lerp_log(v: f32, target: f32, a: f32) -> f32 {
    let v = v.max(1e-6);
    (v.ln() + (target.ln() - v.ln()) * a).exp()
}

#[inline]
fn chaos_scale(v: f32, c: f32) -> f32 {
    (v * (1.0 + 2.0 * c) + 0.15 * c).clamp(0.0, 1.0)
}

/// Apply Dust, Age and Chaos (in that order) to `s`.
pub fn apply_macros(s: &mut Snapshot) {
    let dust = s.dust.clamp(0.0, 1.0);
    if dust > 0.0 {
        s.bit_depth = lerp(s.bit_depth, 6.0, dust);
        s.sample_rate_hz = lerp_log(s.sample_rate_hz, 8000.0, dust);
        s.drive = lerp(s.drive, 0.6, dust);
        s.tape_age = lerp(s.tape_age, 0.7, dust);
        s.hiss = lerp(s.hiss, 0.35, dust);
        s.crackle_density = lerp(s.crackle_density, 0.5, dust);
        s.wow_depth = lerp(s.wow_depth, 0.35, dust);
        s.flutter_depth = lerp(s.flutter_depth, 0.25, dust);
        s.lowpass_hz = lerp_log(s.lowpass_hz, 3000.0, dust);
        s.dropout_chance = lerp(s.dropout_chance, 0.15, dust);
    }

    let age = s.age.clamp(0.0, 1.0);
    if age > 0.0 {
        s.wow_depth = lerp(s.wow_depth, 0.6, age);
        s.flutter_depth = lerp(s.flutter_depth, 0.4, age);
        s.hiss = lerp(s.hiss, 0.4, age);
        s.tape_age = lerp(s.tape_age, 0.8, age);
    }

    let c = s.chaos.clamp(0.0, 1.0);
    if c > 0.0 {
        s.position_jitter = chaos_scale(s.position_jitter, c);
        s.reverse_chance = chaos_scale(s.reverse_chance, c);
        s.stutter_chance = chaos_scale(s.stutter_chance, c);
        s.tape_stop_chance = chaos_scale(s.tape_stop_chance, c);
        s.tape_start_chance = chaos_scale(s.tape_start_chance, c);
        s.dropout_chance = chaos_scale(s.dropout_chance, c);
        s.filter_drift = chaos_scale(s.filter_drift, c);
        s.pitch_jitter_st = (s.pitch_jitter_st * (1.0 + 2.0 * c) + 2.0 * c).clamp(0.0, 12.0);
    }
}

// ---------------------------------------------------------------------------
// LFO
// ---------------------------------------------------------------------------

/// Cycle length in quarter-note beats for a synced rate (4/4 assumed for bars).
pub fn synced_cycle_beats(rate: LfoRate) -> Option<f64> {
    match rate {
        LfoRate::Free => None,
        LfoRate::Whole => Some(4.0),
        LfoRate::Half => Some(2.0),
        LfoRate::Quarter => Some(1.0),
        LfoRate::Eighth => Some(0.5),
        LfoRate::Sixteenth => Some(0.25),
        LfoRate::TwoBars => Some(8.0),
        LfoRate::FourBars => Some(16.0),
    }
}

#[derive(Debug, Clone, Copy)]
struct Lfo {
    /// Phase in [0, 1).
    phase: f64,
    /// Cycle counter in synced mode, to detect new cycles for random shapes.
    last_cycle: i64,
    rand_prev: f32,
    rand_cur: f32,
    rng: Rng,
}

impl Lfo {
    fn new(seed: u32) -> Self {
        let mut rng = Rng::new(seed);
        let rand_cur = rng.bipolar();
        Self {
            phase: 0.0,
            last_cycle: i64::MIN,
            rand_prev: 0.0,
            rand_cur,
            rng,
        }
    }

    fn new_cycle(&mut self) {
        self.rand_prev = self.rand_cur;
        self.rand_cur = self.rng.bipolar();
    }

    /// Value for the current phase, in [-1, 1].
    fn value(&self, shape: LfoShape) -> f32 {
        let p = self.phase as f32;
        let v = match shape {
            LfoShape::Sine => (TAU * p).sin(),
            LfoShape::Triangle => {
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            LfoShape::Saw => 2.0 * p - 1.0,
            LfoShape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::Random => self.rand_cur,
            LfoShape::SmoothRandom => {
                let w = 0.5 - 0.5 * (PI * p).cos();
                self.rand_prev + (self.rand_cur - self.rand_prev) * w
            }
        };
        v.clamp(-1.0, 1.0)
    }

    /// Returns the value at the block start, then advances by `t.block_len` samples.
    fn tick(&mut self, shape: LfoShape, rate: LfoRate, hz: f32, t: &Transport) -> f32 {
        match synced_cycle_beats(rate) {
            Some(cycle) => {
                let pos = t.beat_pos / cycle;
                let cycle_idx = pos.floor();
                let frac = pos - cycle_idx;
                self.phase = if frac.is_finite() {
                    frac.clamp(0.0, 1.0 - f64::EPSILON)
                } else {
                    0.0
                };
                let idx = if cycle_idx.is_finite() {
                    cycle_idx as i64
                } else {
                    0
                };
                if idx != self.last_cycle {
                    if self.last_cycle != i64::MIN {
                        self.new_cycle();
                    }
                    self.last_cycle = idx;
                }
                self.value(shape)
            }
            None => {
                self.last_cycle = i64::MIN;
                let v = self.value(shape);
                let sr = (t.sample_rate as f64).max(1.0);
                let hz = (hz as f64).clamp(0.0, 1000.0);
                let next = self.phase + hz * t.block_len as f64 / sr;
                if next >= 1.0 {
                    self.new_cycle();
                }
                self.phase = next.fract();
                v
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Modulation
// ---------------------------------------------------------------------------

const LFO_SEEDS: [u32; 2] = [0x4C46_4F31, 0x4C46_4F32];

pub struct Modulation {
    visual: Arc<VisualState>,
    lfos: [Lfo; 2],
    env: f32,
}

impl Modulation {
    pub fn new(visual: Arc<VisualState>) -> Self {
        Self {
            visual,
            lfos: [Lfo::new(LFO_SEEDS[0]), Lfo::new(LFO_SEEDS[1])],
            env: 0.0,
        }
    }

    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {
        self.lfos = [Lfo::new(LFO_SEEDS[0]), Lfo::new(LFO_SEEDS[1])];
        self.env = 0.0;
    }

    /// `l`/`r` = input audio of this block (for the envelope follower).
    pub fn process_block(&mut self, snap: &mut Snapshot, l: &[f32], r: &[f32], t: &Transport) {
        // 1. Macros.
        apply_macros(snap);

        // 2. Sources.
        let mut lfo_vals = [0.0f32; 2];
        for (i, lfo) in self.lfos.iter_mut().enumerate() {
            lfo_vals[i] = lfo.tick(
                snap.lfo_shape[i],
                snap.lfo_rate_mode[i],
                snap.lfo_rate_hz[i],
                t,
            );
        }
        let env = self.follow(l, r, snap.env_attack_ms, snap.env_release_ms, t.sample_rate);

        // Sum normalized offsets per target, then apply each once.
        let mut offsets = [0.0f32; ModTarget::COUNT];
        for i in 0..2 {
            let amt = snap.lfo_amount[i].clamp(-1.0, 1.0);
            offsets[snap.lfo_target[i] as usize] += lfo_vals[i] * amt;
        }
        offsets[snap.env_target as usize] += env * snap.env_amount.clamp(-1.0, 1.0);

        for (idx, &off) in offsets.iter().enumerate().skip(1) {
            if off == 0.0 || !off.is_finite() {
                continue;
            }
            let target = ModTarget::from_index(idx);
            if let (Some(range), Some(field)) = (target_range(target), target_field(snap, target)) {
                *field = apply_offset(*field, off, range);
            }
        }

        // 3. Visuals.
        store_f32(&self.visual.lfo_values[0], lfo_vals[0]);
        store_f32(&self.visual.lfo_values[1], lfo_vals[1]);
        store_f32(&self.visual.env_value, env);
    }

    /// Peak follower with one-pole attack/release. Returns the end-of-block value in 0..1.
    fn follow(&mut self, l: &[f32], r: &[f32], att_ms: f32, rel_ms: f32, sr: f32) -> f32 {
        let sr = sr.max(1.0);
        let coeff = |ms: f32| 1.0 - (-1.0 / (ms.max(0.01) * 0.001 * sr)).exp();
        let a_att = coeff(att_ms);
        let a_rel = coeff(rel_ms);
        let mut env = self.env;
        for (&x, &y) in l.iter().zip(r.iter()) {
            let mut peak = x.abs().max(y.abs());
            if !peak.is_finite() {
                peak = 0.0;
            }
            let peak = peak.min(1.0);
            let a = if peak > env { a_att } else { a_rel };
            env += (peak - env) * a;
        }
        if env < 1e-15 || !env.is_finite() {
            env = 0.0;
        }
        self.env = env.clamp(0.0, 1.0);
        self.env
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::GrainwaveParams;
    use crate::shared::load_f32;

    const ALL_TARGETS: usize = ModTarget::COUNT;
    const SHAPES: [LfoShape; 6] = [
        LfoShape::Sine,
        LfoShape::Triangle,
        LfoShape::Saw,
        LfoShape::Square,
        LfoShape::Random,
        LfoShape::SmoothRandom,
    ];

    fn base() -> Snapshot {
        Snapshot::from_params(&GrainwaveParams::default())
    }

    fn transport(beat_pos: f64, block_len: usize) -> Transport {
        Transport {
            sample_rate: 48_000.0,
            bpm: 120.0,
            beat_pos,
            playing: true,
            block_len,
        }
    }

    fn module() -> (Modulation, Arc<VisualState>) {
        let v = Arc::new(VisualState::default());
        let mut m = Modulation::new(v.clone());
        m.reset(48_000.0, 512);
        (m, v)
    }

    fn all_fields(s: &mut Snapshot) -> Vec<f32> {
        let mut out = Vec::new();
        for i in 1..ALL_TARGETS {
            out.push(*target_field(s, ModTarget::from_index(i)).unwrap());
        }
        out.extend([
            s.hiss,
            s.crackle_density,
            s.dropout_chance,
            s.tape_stop_chance,
            s.tape_start_chance,
            s.filter_drift,
        ]);
        out
    }

    #[test]
    fn target_table_covers_every_target() {
        assert_eq!(ModTarget::Mix as usize, ModTarget::COUNT - 1);
        assert_eq!(ModTarget::variants().len(), ModTarget::COUNT);
        let mut s = base();
        assert!(target_range(ModTarget::None).is_none());
        for i in 1..ALL_TARGETS {
            let t = ModTarget::from_index(i);
            let r = target_range(t).unwrap();
            assert!(r.min < r.max);
            let v = *target_field(&mut s, t).unwrap();
            assert!(v >= r.min && v <= r.max, "{t:?} default {v} outside range");
        }
    }

    #[test]
    fn neutral_settings_leave_snapshot_unchanged() {
        let (mut m, _) = module();
        for shape in SHAPES {
            let mut s = base();
            s.lfo_shape = [shape; 2];
            s.lfo_target = [ModTarget::LowpassCutoff, ModTarget::Pitch];
            s.env_target = ModTarget::Drive;
            let mut before = s;
            let l = [0.8f32; 64];
            m.process_block(&mut s, &l, &l, &transport(0.3, 64));
            assert_eq!(all_fields(&mut before), all_fields(&mut s));
        }
    }

    #[test]
    fn dust_full_hits_targets() {
        let mut s = base();
        s.dust = 1.0;
        apply_macros(&mut s);
        let close = |a: f32, b: f32| (a - b).abs() <= 1e-3 * b.abs().max(1.0);
        assert!(close(s.bit_depth, 6.0));
        assert!(close(s.sample_rate_hz, 8000.0));
        assert!(close(s.drive, 0.6));
        assert!(close(s.tape_age, 0.7));
        assert!(close(s.hiss, 0.35));
        assert!(close(s.crackle_density, 0.5));
        assert!(close(s.wow_depth, 0.35));
        assert!(close(s.flutter_depth, 0.25));
        assert!(close(s.lowpass_hz, 3000.0));
        assert!(close(s.dropout_chance, 0.15));
    }

    #[test]
    fn dust_half_is_log_interpolated_for_hz() {
        let mut s = base(); // sample rate 48 kHz, low-pass 20 kHz
        s.dust = 0.5;
        apply_macros(&mut s);
        assert!((s.sample_rate_hz - (48000.0f32 * 8000.0).sqrt()).abs() < 1.0);
        assert!((s.lowpass_hz - (20000.0f32 * 3000.0).sqrt()).abs() < 1.0);
    }

    #[test]
    fn age_full_hits_targets() {
        let mut s = base();
        s.age = 1.0;
        apply_macros(&mut s);
        assert!((s.wow_depth - 0.6).abs() < 1e-6);
        assert!((s.flutter_depth - 0.4).abs() < 1e-6);
        assert!((s.hiss - 0.4).abs() < 1e-6);
        assert!((s.tape_age - 0.8).abs() < 1e-6);
    }

    #[test]
    fn chaos_formula() {
        let mut s = base();
        s.chaos = 0.5;
        s.position_jitter = 0.1;
        s.reverse_chance = 0.25;
        s.stutter_chance = 0.0;
        s.tape_stop_chance = 0.9;
        s.pitch_jitter_st = 1.0;
        apply_macros(&mut s);
        // v * 2 + 0.075
        assert!((s.position_jitter - 0.275).abs() < 1e-6);
        assert!((s.reverse_chance - 0.575).abs() < 1e-6);
        assert!((s.stutter_chance - 0.075).abs() < 1e-6);
        assert!((s.tape_start_chance - 0.075).abs() < 1e-6);
        assert!((s.dropout_chance - 0.075).abs() < 1e-6);
        assert!((s.filter_drift - 0.075).abs() < 1e-6);
        assert_eq!(s.tape_stop_chance, 1.0); // clamped
                                             // 1 * 2 + 1
        assert!((s.pitch_jitter_st - 3.0).abs() < 1e-6);
    }

    #[test]
    fn lfo_stays_in_range_for_all_shapes_and_modes() {
        let rates = [
            LfoRate::Free,
            LfoRate::Sixteenth,
            LfoRate::Whole,
            LfoRate::FourBars,
        ];
        for shape in SHAPES {
            for rate in rates {
                let mut lfo = Lfo::new(7);
                let mut beat = 0.0;
                for _ in 0..5000 {
                    let t = transport(beat, 37);
                    let v = lfo.tick(shape, rate, 13.7, &t);
                    assert!((-1.0..=1.0).contains(&v), "{shape:?} {rate:?} {v}");
                    beat += 37.0 / t.samples_per_beat() * 3.3;
                }
            }
        }
    }

    #[test]
    fn synced_lfo_phase_matches_beat_pos() {
        let (mut m, v) = module();
        let mut s = base();
        s.lfo_shape = [LfoShape::Saw, LfoShape::Saw];
        s.lfo_rate_mode = [LfoRate::Quarter, LfoRate::TwoBars];
        // beat 9.25: quarter -> phase 0.25 -> saw -0.5; 2 bars (8 beats) -> phase 1.25/8.
        m.process_block(&mut s, &[0.0; 16], &[0.0; 16], &transport(9.25, 16));
        assert!((load_f32(&v.lfo_values[0]) + 0.5).abs() < 1e-5);
        let expect = 2.0 * (1.25 / 8.0) - 1.0;
        assert!((load_f32(&v.lfo_values[1]) - expect as f32).abs() < 1e-5);
        // Independent of history: jump elsewhere.
        m.process_block(&mut s, &[0.0; 16], &[0.0; 16], &transport(100.75, 16));
        assert!((load_f32(&v.lfo_values[0]) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn free_lfo_advances_by_block_len() {
        let mut lfo = Lfo::new(1);
        let t = transport(0.0, 480); // 480 samples at 48 kHz, 1 Hz -> 0.01 cycle
        lfo.tick(LfoShape::Saw, LfoRate::Free, 1.0, &t);
        assert!((lfo.phase - 0.01).abs() < 1e-9);
    }

    #[test]
    fn random_lfo_changes_once_per_cycle() {
        let mut lfo = Lfo::new(3);
        let first = lfo.tick(LfoShape::Random, LfoRate::Quarter, 0.0, &transport(0.1, 16));
        let same = lfo.tick(LfoShape::Random, LfoRate::Quarter, 0.0, &transport(0.9, 16));
        assert_eq!(first, same);
        let next = lfo.tick(LfoShape::Random, LfoRate::Quarter, 0.0, &transport(1.1, 16));
        assert_ne!(first, next);
        // Smooth random starts at the previous value at the cycle boundary.
        let mut a = Lfo::new(5);
        a.tick(
            LfoShape::SmoothRandom,
            LfoRate::Quarter,
            0.0,
            &transport(0.5, 16),
        );
        let held = a.rand_cur;
        let v = a.tick(
            LfoShape::SmoothRandom,
            LfoRate::Quarter,
            0.0,
            &transport(1.0, 16),
        );
        assert!((v - held).abs() < 1e-6);
    }

    #[test]
    fn modulated_values_always_clamped() {
        let (mut m, _) = module();
        let mut rng = Rng::new(12345);
        for n in 0..4000 {
            let mut s = base();
            s.dust = rng.next_f32();
            s.age = rng.next_f32();
            s.chaos = rng.next_f32();
            for i in 0..2 {
                s.lfo_shape[i] = SHAPES[(rng.next_u32() % 6) as usize];
                s.lfo_rate_mode[i] = LfoRate::from_index((rng.next_u32() % 8) as usize);
                s.lfo_rate_hz[i] = 0.01 + rng.next_f32() * 20.0;
                s.lfo_target[i] = ModTarget::from_index((rng.next_u32() % 22) as usize);
                s.lfo_amount[i] = rng.bipolar();
            }
            s.env_target = ModTarget::from_index((rng.next_u32() % 22) as usize);
            s.env_amount = rng.bipolar();
            let l = [rng.bipolar() * 2.0; 32];
            m.process_block(&mut s, &l, &l, &transport(n as f64 * 0.37, 32));
            for i in 1..ALL_TARGETS {
                let t = ModTarget::from_index(i);
                let r = target_range(t).unwrap();
                let v = *target_field(&mut s, t).unwrap();
                assert!(v.is_finite() && v >= r.min && v <= r.max, "{t:?} = {v}");
            }
        }
    }

    #[test]
    fn cutoff_modulated_in_log_space() {
        let (mut m, _) = module();
        let mut s = base();
        s.lowpass_hz = 1000.0;
        s.lfo_shape = [LfoShape::Square, LfoShape::Sine];
        s.lfo_target = [ModTarget::LowpassCutoff, ModTarget::None];
        s.lfo_amount = [0.1, 0.0];
        // Square at phase 0 = +1: 1000 * 1000^0.1.
        m.process_block(&mut s, &[0.0; 8], &[0.0; 8], &transport(0.0, 8));
        assert!((s.lowpass_hz - 1000.0 * 1000f32.powf(0.1)).abs() < 0.5);

        let r = target_range(ModTarget::LowpassCutoff).unwrap();
        assert!((apply_offset(1000.0, -0.1, r) - 1000.0 / 1000f32.powf(0.1)).abs() < 0.1);
        assert_eq!(apply_offset(1000.0, 1.0, r), 20000.0);
        assert_eq!(apply_offset(1000.0, -1.0, r), 20.0);
        // Linear target: offset × span.
        let p = target_range(ModTarget::Pitch).unwrap();
        assert!((apply_offset(0.0, 0.25, p) - 12.0).abs() < 1e-6);
    }

    #[test]
    fn envelope_follower_rises_and_falls() {
        let (mut m, v) = module();
        let mut s = base();
        s.env_attack_ms = 10.0;
        s.env_release_ms = 100.0;
        s.env_target = ModTarget::Mix;
        s.env_amount = -1.0;
        let loud = [0.9f32; 480];
        let quiet = [0.0f32; 480];
        let mut prev = 0.0;
        for _ in 0..10 {
            let mut snap = s;
            m.process_block(&mut snap, &loud, &loud, &transport(0.0, 480));
            let e = load_f32(&v.env_value);
            assert!(e >= prev && e <= 1.0);
            prev = e;
        }
        assert!(prev > 0.85, "attack too slow: {prev}");
        let mut snap = s;
        m.process_block(&mut snap, &loud, &loud, &transport(0.0, 480));
        assert!(snap.mix < 0.2); // mix 1.0 - env
        for _ in 0..50 {
            let mut snap = s;
            m.process_block(&mut snap, &quiet, &quiet, &transport(0.0, 480));
            let e = load_f32(&v.env_value);
            assert!(e <= prev);
            prev = e;
        }
        assert!(prev < 0.01, "release too slow: {prev}");
    }
}
