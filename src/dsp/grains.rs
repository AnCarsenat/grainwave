//! OWNER: feat/grain-engine. Capture buffer, grain scheduler, voices, playback modes, MIDI pitch.
//!
//! Positions in the capture buffer are tracked as *absolute* sample indices (`f64`): sample `k`
//! is the k-th sample ever written since `reset()`, stored at `k % capacity`. The effective
//! buffer length `L` only limits how far back grains may reach and how the buffer is mapped
//! for the GUI (a tape loop of length `L`: position = `(abs mod L) / L`). Writing always
//! wraps over the full capacity, so changing `L` never creates a discontinuity.
use super::rng::Rng;
use super::Transport;
use crate::params::{BufferMode, DensityMode, GrainEnvelope, PitchQuantize, Snapshot};
use crate::shared::{store_f32, VisualState, MAX_VISUAL_GRAINS, WAVEFORM_POINTS};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Size of the voice pool.
pub const MAX_VOICES: usize = 32;
/// Voice limit when `low_cpu` is on.
const LOW_CPU_VOICES: usize = 8;
/// Nominal capture capacity in seconds.
const CAPACITY_SECONDS: f32 = 32.0;
/// Extra capacity (s) so a grain reading at the far end of a 32 s window is never overwritten.
const CAPACITY_GUARD_SECONDS: f32 = 1.0;
/// Fade used when a voice is stolen (s).
const STEAL_FADE_SECONDS: f32 = 0.002;
/// Output loudness smoothing time constant (s).
const GAIN_SMOOTH_SECONDS: f32 = 0.02;
/// Hard limit on the total semitone offset and on the playback rate.
const MAX_SEMITONES: f32 = 48.0;
const MAX_RATE: f64 = 16.0;
/// Samples of headroom required around a read position (cubic Hermite reads p-1 .. p+2).
const READ_MARGIN: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RateShape {
    Normal,
    /// `rate × (1 − t)²`
    TapeStop,
    /// `rate × t²`
    TapeStart,
}

impl RateShape {
    /// Integral of the normalized rate curve from 0 to `t` (fraction of `rate × len` travelled).
    #[inline]
    fn travelled(self, t: f64) -> f64 {
        match self {
            RateShape::Normal => t,
            RateShape::TapeStop => {
                let u = 1.0 - t;
                (1.0 - u * u * u) / 3.0
            }
            RateShape::TapeStart => t * t * t / 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    active: bool,
    /// Spawn order, for oldest-voice stealing.
    id: u64,
    /// Absolute buffer index of the lowest position of the grain's span.
    start: f64,
    /// Samples covered by the whole grain (`rate × len × travelled(1)`).
    travel: f64,
    rate: f64,
    len: f64,
    age: f64,
    reverse: bool,
    shape: RateShape,
    envelope: GrainEnvelope,
    gain_l: f32,
    gain_r: f32,
    /// Steal fade: 1 while playing normally, ramps to 0 once `releasing`.
    fade: f32,
    releasing: bool,
    /// Last read position (absolute), for the visual display.
    pos: f64,
}

impl Voice {
    const IDLE: Voice = Voice {
        active: false,
        id: 0,
        start: 0.0,
        travel: 0.0,
        rate: 1.0,
        len: 1.0,
        age: 0.0,
        reverse: false,
        shape: RateShape::Normal,
        envelope: GrainEnvelope::Hann,
        gain_l: 0.0,
        gain_r: 0.0,
        fade: 1.0,
        releasing: false,
        pos: 0.0,
    };

    /// Read position (absolute) at the voice's current age.
    #[inline]
    fn position(&self) -> f64 {
        let t = (self.age / self.len).clamp(0.0, 1.0);
        let off = self.rate * self.len * self.shape.travelled(t);
        if self.reverse {
            self.start + self.travel - off
        } else {
            self.start + off
        }
    }
}

/// Grain envelope value at `age` samples into a grain of `len` samples.
fn envelope(shape: GrainEnvelope, age: f64, len: f64, sr: f64) -> f32 {
    if len <= 0.0 || age < 0.0 || age >= len {
        return 0.0;
    }
    let t = age / len;
    let v = match shape {
        GrainEnvelope::Hann => 0.5 - 0.5 * (std::f64::consts::TAU * t).cos(),
        GrainEnvelope::Triangle => 1.0 - (2.0 * t - 1.0).abs(),
        GrainEnvelope::Square => {
            let edge = (0.002 * sr).min(len * 0.5).max(1.0);
            (age / edge).min((len - age) / edge).min(1.0)
        }
        GrainEnvelope::ExpDecay => {
            let attack = (0.005 * sr).min(len * 0.5).max(1.0);
            if age < attack {
                age / attack
            } else {
                // Normalized so it reaches exactly 0 at the end of the grain (no click).
                let x = (age - attack) / (len - attack).max(1.0);
                let end = (-5.0f64).exp();
                (((-5.0 * x).exp() - end) / (1.0 - end)).max(0.0)
            }
        }
    };
    v as f32
}

#[inline]
fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

/// Replace non-finite values with `default`, then clamp.
#[inline]
fn sane(v: f32, default: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

/// Snap a semitone offset to the nearest degree allowed by `q` (root = 0).
pub fn quantize_semitones(st: f32, q: PitchQuantize) -> f32 {
    let degrees: &[f32] = match q {
        PitchQuantize::Off => return st,
        PitchQuantize::Octaves => &[0.0],
        PitchQuantize::Fifths => &[0.0, 7.0],
        PitchQuantize::Major => &[0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0],
        PitchQuantize::Minor => &[0.0, 2.0, 3.0, 5.0, 7.0, 8.0, 10.0],
        PitchQuantize::Pentatonic => &[0.0, 2.0, 4.0, 7.0, 9.0],
    };
    let octave = (st / 12.0).floor();
    let within = st - octave * 12.0;
    // The next octave's root (12) is also a candidate.
    let mut best = 12.0;
    let mut best_dist = (within - 12.0).abs();
    for &d in degrees {
        let dist = (within - d).abs();
        if dist < best_dist {
            best = d;
            best_dist = dist;
        }
    }
    octave * 12.0 + best
}

/// Effective capture length in samples, clamped to `[1, max_len]`.
fn effective_len(snap: &Snapshot, t: &Transport, max_len: usize) -> usize {
    let sr = t.sample_rate as f64;
    let samples = match snap.buffer_mode {
        BufferMode::Free => sane(snap.buffer_seconds, 2.0, 0.5, 8.0) as f64 * sr,
        BufferMode::Synced => {
            let bars = snap.buffer_bars.clamp(1, 8) as f64;
            bars * 4.0 * spb(t)
        }
    };
    (samples.round() as usize).clamp(1, max_len.max(1))
}

/// Samples per beat, sanitized.
#[inline]
fn spb(t: &Transport) -> f64 {
    let v = t.samples_per_beat();
    if v.is_finite() {
        v.clamp(1.0, 1.0e9)
    } else {
        t.sample_rate as f64 * 0.5
    }
}

pub struct GrainEngine {
    visual: Arc<VisualState>,
    rng: Rng,
    sample_rate: f32,

    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    /// Capacity in samples (32 s + guard).
    cap: usize,
    /// Number of samples written since reset (absolute index of the next write).
    written: u64,

    voices: [Voice; MAX_VOICES],
    next_id: u64,

    /// Free mode: samples until the next spawn.
    countdown: f64,
    /// Synced mode: grid index of the last spawn.
    last_grid: Option<i64>,

    gain: f32,
    gain_coef: f32,
    fade_step: f32,

    /// Held MIDI notes, oldest first.
    held: [u8; 128],
    held_len: usize,

    wave_slice: usize,
}

impl GrainEngine {
    pub fn new(visual: Arc<VisualState>) -> Self {
        Self {
            visual,
            rng: Rng::new(0x6772_6169),
            sample_rate: 48_000.0,
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            cap: 0,
            written: 0,
            voices: [Voice::IDLE; MAX_VOICES],
            next_id: 0,
            countdown: 0.0,
            last_grid: None,
            gain: 1.0,
            gain_coef: 0.0,
            fade_step: 1.0,
            held: [0; 128],
            held_len: 0,
            wave_slice: 0,
        }
    }

    /// Allocate the capture buffer (32 s plus a 1 s guard) and clear all state.
    pub fn reset(&mut self, sample_rate: f32, _max_block: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        self.sample_rate = sr;
        let cap = ((CAPACITY_SECONDS + CAPACITY_GUARD_SECONDS) * sr).ceil() as usize + 8;
        if self.buf_l.len() != cap {
            self.buf_l = vec![0.0; cap];
            self.buf_r = vec![0.0; cap];
        } else {
            self.buf_l.fill(0.0);
            self.buf_r.fill(0.0);
        }
        self.cap = cap;
        self.written = 0;
        self.voices = [Voice::IDLE; MAX_VOICES];
        self.next_id = 0;
        self.countdown = 0.0;
        self.last_grid = None;
        self.gain = 1.0;
        self.gain_coef = (-1.0 / (GAIN_SMOOTH_SECONDS * sr)).exp();
        self.fade_step = 1.0 / (STEAL_FADE_SECONDS * sr).max(1.0);
        self.held_len = 0;
        self.wave_slice = 0;

        let v = &self.visual;
        v.grain_active.store(0, Ordering::Relaxed);
        v.grain_reverse.store(0, Ordering::Relaxed);
        store_f32(&v.write_head, 0.0);
        for w in v.waveform.iter() {
            store_f32(w, 0.0);
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: f32) {
        if velocity <= 0.0 {
            self.note_off(note);
            return;
        }
        self.remove_note(note);
        if self.held_len == self.held.len() {
            // Drop the oldest (cannot really happen: 128 distinct notes).
            self.held.copy_within(1.., 0);
            self.held_len -= 1;
        }
        self.held[self.held_len] = note;
        self.held_len += 1;
    }

    pub fn note_off(&mut self, note: u8) {
        self.remove_note(note);
    }

    fn remove_note(&mut self, note: u8) {
        if let Some(i) = self.held[..self.held_len].iter().position(|&n| n == note) {
            self.held.copy_within(i + 1..self.held_len, i);
            self.held_len -= 1;
        }
    }

    /// Most recently pressed note that is still held.
    fn last_held(&self) -> Option<u8> {
        if self.held_len > 0 {
            Some(self.held[self.held_len - 1])
        } else {
            None
        }
    }

    /// Largest usable `L` (keeps a grain's full span inside the capacity).
    fn max_len(&self) -> usize {
        let guard = (CAPACITY_GUARD_SECONDS * self.sample_rate) as usize;
        self.cap.saturating_sub(guard).max(1)
    }

    #[inline]
    fn read(&self, pos: f64) -> (f32, f32) {
        let cap = self.cap as i64;
        let base = pos.floor();
        let frac = (pos - base) as f32;
        let i0 = (base as i64).rem_euclid(cap) as usize;
        let im1 = if i0 == 0 { self.cap - 1 } else { i0 - 1 };
        let i1 = if i0 + 1 >= self.cap { i0 + 1 - self.cap } else { i0 + 1 };
        let i2 = if i1 + 1 >= self.cap { i1 + 1 - self.cap } else { i1 + 1 };
        let b = &self.buf_l;
        let l = hermite(b[im1], b[i0], b[i1], b[i2], frac);
        let b = &self.buf_r;
        let r = hermite(b[im1], b[i0], b[i1], b[i2], frac);
        (l, r)
    }

    /// In: input audio. Out: wet grain audio, in place.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], snap: &Snapshot, t: &Transport) {
        if self.cap == 0 {
            return;
        }
        let n = l.len().min(r.len());
        let sr = self.sample_rate as f64;
        let len_l = effective_len(snap, t, self.max_len());
        let spb = spb(t);

        // Block-rate parameters, clamped (modulation may push them out of range).
        let size_ms = sane(snap.grain_size_ms, 80.0, 5.0, 500.0);
        let grain_len = (size_ms as f64 * 0.001 * sr).max(4.0);
        let density_hz = sane(snap.density_hz, 12.0, 1.0, 64.0) as f64;
        let div_samples = (snap.density_division.beats() * spb).max(1.0);
        let interval = match snap.density_mode {
            DensityMode::Free => sr / density_hz,
            DensityMode::Synced => div_samples,
        };
        let eff_density = (sr / interval).min(sr);
        let target_gain = (1.0 / (eff_density * size_ms as f64 * 0.001).max(1.0).sqrt()) as f32;
        let pos_jitter = sane(snap.position_jitter, 0.0, 0.0, 1.0);
        let midi_ok = !snap.midi_pitch || self.held_len > 0;
        let voice_limit = {
            let v = snap.max_voices.clamp(1, MAX_VOICES as i32) as usize;
            if snap.low_cpu {
                v.min(LOW_CPU_VOICES)
            } else {
                v
            }
        };
        let freeze = snap.freeze;

        if snap.density_mode == DensityMode::Free {
            // Do not wait for a stale long interval after the density goes up.
            self.countdown = self.countdown.min(interval * 1.1);
        }

        for i in 0..n {
            // 1. Capture.
            if !freeze {
                let w = (self.written % self.cap as u64) as usize;
                self.buf_l[w] = l[i];
                self.buf_r[w] = r[i];
                self.written += 1;
            }

            // 2. Schedule.
            let spawn = match snap.density_mode {
                DensityMode::Free => {
                    self.last_grid = None;
                    self.countdown -= 1.0;
                    if self.countdown <= 0.0 {
                        let jit = 1.0 + 0.1 * pos_jitter as f64 * self.rng.bipolar() as f64;
                        self.countdown += interval * jit;
                        if self.countdown < 1.0 {
                            self.countdown = 1.0;
                        }
                        true
                    } else {
                        false
                    }
                }
                DensityMode::Synced => {
                    let beat = t.beat_pos + i as f64 / spb;
                    let idx = (beat / snap.density_division.beats()).floor() as i64;
                    let fire = self.last_grid != Some(idx);
                    self.last_grid = Some(idx);
                    fire
                }
            };
            if spawn && midi_ok {
                self.spawn(snap, grain_len, len_l, voice_limit, freeze);
            }

            // 3. Render voices.
            let newest = self.written as f64 - 1.0;
            let hi = newest - 2.0;
            let lo = self.written as f64 - self.cap as f64 + 2.0;
            let mut out_l = 0.0f32;
            let mut out_r = 0.0f32;
            for vi in 0..MAX_VOICES {
                if !self.voices[vi].active {
                    continue;
                }
                let v = self.voices[vi];
                let mut p = v.position();
                // Never read past the write head, never read overwritten data.
                if p > hi {
                    p = hi;
                }
                if p < lo {
                    p = lo;
                }
                let (sl, sr_) = self.read(p);
                let env = envelope(v.envelope, v.age, v.len, sr) * v.fade;
                out_l += sl * env * v.gain_l;
                out_r += sr_ * env * v.gain_r;

                let voice = &mut self.voices[vi];
                voice.pos = p;
                voice.age += 1.0;
                if voice.releasing {
                    voice.fade -= self.fade_step;
                }
                if voice.age >= voice.len || voice.fade <= 0.0 {
                    voice.active = false;
                }
            }

            self.gain = target_gain + (self.gain - target_gain) * self.gain_coef;
            l[i] = out_l * self.gain;
            r[i] = out_r * self.gain;
        }

        self.update_visual(len_l);
    }

    fn spawn(
        &mut self,
        snap: &Snapshot,
        grain_len: f64,
        len_l: usize,
        voice_limit: usize,
        freeze: bool,
    ) {
        // Voice allocation: release the oldest voices until there is room under the limit.
        loop {
            let playing = self.voices.iter().filter(|v| v.active && !v.releasing).count();
            if playing < voice_limit {
                break;
            }
            let oldest = self
                .voices
                .iter_mut()
                .filter(|v| v.active && !v.releasing)
                .min_by_key(|v| v.id);
            match oldest {
                Some(v) => v.releasing = true,
                None => break,
            }
        }
        let slot = match self.voices.iter().position(|v| !v.active) {
            Some(s) => s,
            None => {
                // Pool full of fading voices: hard-steal the oldest one.
                let mut best = 0;
                for (i, v) in self.voices.iter().enumerate() {
                    if v.id < self.voices[best].id {
                        best = i;
                    }
                }
                best
            }
        };

        // Rate.
        let midi_offset = match (snap.midi_pitch, self.last_held()) {
            (true, Some(note)) => note as f32 - 60.0,
            _ => 0.0,
        };
        let rng = &mut self.rng;
        let pitch = sane(snap.pitch_st, 0.0, -MAX_SEMITONES, MAX_SEMITONES);
        let pj = sane(snap.pitch_jitter_st, 0.0, 0.0, 24.0);
        let jitter = pj * rng.bipolar();
        let st = quantize_semitones(pitch + jitter + midi_offset, snap.pitch_quantize)
            .clamp(-MAX_SEMITONES, MAX_SEMITONES);
        let speed = sane(snap.speed, 1.0, 0.25, 4.0);
        let rate = (speed as f64 * 2f64.powf(st as f64 / 12.0)).clamp(1.0 / 1024.0, MAX_RATE);

        // Playback modes.
        let reverse = rng.chance(sane(snap.reverse_chance, 0.0, 0.0, 1.0));
        let shape = if rng.chance(sane(snap.tape_stop_chance, 0.0, 0.0, 1.0)) {
            RateShape::TapeStop
        } else if rng.chance(sane(snap.tape_start_chance, 0.0, 0.0, 1.0)) {
            RateShape::TapeStart
        } else {
            RateShape::Normal
        };
        let travel = rate * grain_len * shape.travelled(1.0);

        // Read position.
        let l_f = len_l as f64;
        let memory = sane(snap.memory, 0.0, 0.0, 1.0) as f64;
        let position = sane(snap.position, 0.0, 0.0, 1.0) as f64;
        let pos_jit = sane(snap.position_jitter, 0.0, 0.0, 1.0) as f64;
        let max_lookback = l_f * (0.1 + 0.9 * memory);
        let lookback = (position * max_lookback
            + pos_jit * max_lookback * 0.5 * rng.bipolar() as f64)
            .clamp(0.0, max_lookback);

        // Minimum lookback so the grain never catches the write head.
        let advance = if freeze { 0.0 } else { 1.0 };
        let required = if reverse {
            travel
        } else {
            match shape {
                RateShape::Normal => (rate - advance) * grain_len,
                RateShape::TapeStart => (rate / 3.0 - advance) * grain_len,
                RateShape::TapeStop => {
                    if advance == 0.0 {
                        travel
                    } else if rate <= 1.0 {
                        0.0
                    } else {
                        let ts = 1.0 - 1.0 / rate.sqrt();
                        rate * grain_len * shape.travelled(ts) - ts * grain_len
                    }
                }
            }
            .max(0.0)
        } + READ_MARGIN;
        // Maximum lookback: recorded history, window length, and capacity for the grain span.
        let available = (self.written as f64)
            .min(l_f)
            .min(self.cap as f64 - grain_len - travel - 2.0 * READ_MARGIN)
            .max(0.0);
        let lookback = lookback.max(required).min(available);
        let start = self.written as f64 - lookback;

        // Spread (equal-power, unity at center).
        let spread = sane(snap.spread, 0.0, 0.0, 1.0);
        let pan = spread * rng.bipolar();
        let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let gain_l = theta.cos() * std::f32::consts::SQRT_2;
        let gain_r = theta.sin() * std::f32::consts::SQRT_2;

        let id = self.next_id;
        self.next_id += 1;
        let v = Voice {
            active: true,
            id,
            start,
            travel,
            rate,
            len: grain_len,
            age: 0.0,
            reverse,
            shape,
            envelope: snap.envelope,
            gain_l,
            gain_r,
            fade: 1.0,
            releasing: false,
            pos: 0.0,
        };
        self.voices[slot] = Voice { pos: v.position(), ..v };
    }

    fn update_visual(&mut self, len_l: usize) {
        let vis = &self.visual;
        let l_f = len_l as f64;
        let mut active = 0u32;
        let mut reverse = 0u32;
        for (i, v) in self.voices.iter().enumerate().take(MAX_VISUAL_GRAINS) {
            if v.active {
                active |= 1 << i;
                if v.reverse {
                    reverse |= 1 << i;
                }
                store_f32(&vis.grain_pos[i], (v.pos.rem_euclid(l_f) / l_f) as f32);
            }
        }
        vis.grain_active.store(active, Ordering::Relaxed);
        vis.grain_reverse.store(reverse, Ordering::Relaxed);
        store_f32(&vis.write_head, ((self.written as f64).rem_euclid(l_f) / l_f) as f32);

        // Refresh one waveform slice. Slice s covers loop offsets k in [s·L/256, (s+1)·L/256);
        // offset k shows the most recent sample whose absolute index ≡ k (mod L).
        let s = self.wave_slice;
        self.wave_slice = (s + 1) % WAVEFORM_POINTS;
        let k0 = s * len_l / WAVEFORM_POINTS;
        let k1 = ((s + 1) * len_l / WAVEFORM_POINTS).max(k0 + 1).min(len_l);
        let mut peak = 0.0f32;
        if self.written > 0 {
            let newest = self.written - 1;
            let l64 = len_l as u64;
            let history = self.written.min(l64);
            for k in k0..k1 {
                let k = k as u64;
                let back = (newest + l64 - k % l64) % l64; // newest - a, in [0, L)
                if back >= history {
                    continue;
                }
                let a = newest - back;
                let idx = (a % self.cap as u64) as usize;
                peak = peak.max(self.buf_l[idx].abs()).max(self.buf_r[idx].abs());
            }
        }
        store_f32(&vis.waveform[s], peak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ChannelMode, Division, GrainwaveParams};

    const SR: f32 = 48_000.0;

    fn snap() -> Snapshot {
        Snapshot::from_params(&GrainwaveParams::default())
    }

    fn transport(bpm: f64, beat_pos: f64, n: usize) -> Transport {
        Transport { sample_rate: SR, bpm, beat_pos, playing: true, block_len: n }
    }

    fn engine() -> GrainEngine {
        let mut e = GrainEngine::new(Arc::new(VisualState::default()));
        e.reset(SR, 512);
        e
    }

    /// Runs `total` samples of `input(i)` through the engine in blocks of `block`.
    fn run(
        e: &mut GrainEngine,
        s: &Snapshot,
        total: usize,
        block: usize,
        input: impl Fn(usize) -> f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut out_l = Vec::with_capacity(total);
        let mut out_r = Vec::with_capacity(total);
        let mut beat = 0.0;
        let mut done = 0;
        while done < total {
            let n = block.min(total - done);
            let mut l: Vec<f32> = (done..done + n).map(&input).collect();
            let mut r = l.clone();
            let t = transport(120.0, beat, n);
            e.process(&mut l, &mut r, s, &t);
            beat += n as f64 * 120.0 / 60.0 / SR as f64;
            out_l.extend_from_slice(&l);
            out_r.extend_from_slice(&r);
            done += n;
        }
        (out_l, out_r)
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = engine();
        let mut s = snap();
        s.reverse_chance = 0.5;
        s.tape_stop_chance = 0.3;
        s.pitch_jitter_st = 7.0;
        let (l, r) = run(&mut e, &s, 48_000, 256, |_| 0.0);
        assert!(l.iter().chain(r.iter()).all(|&x| x == 0.0));
        // Grains were actually playing.
        assert!(e.next_id > 5);
    }

    #[test]
    fn produces_output_from_signal() {
        let mut e = engine();
        let s = snap();
        let (l, _) = run(&mut e, &s, 48_000, 512, |i| (i as f32 * 0.05).sin() * 0.5);
        let peak = l.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.05 && peak < 4.0, "peak {peak}");
    }

    #[test]
    fn finite_under_extreme_random_params() {
        let mut e = engine();
        let mut rng = Rng::new(1234);
        let envs = [
            GrainEnvelope::Hann,
            GrainEnvelope::Triangle,
            GrainEnvelope::Square,
            GrainEnvelope::ExpDecay,
        ];
        let quants = [
            PitchQuantize::Off,
            PitchQuantize::Octaves,
            PitchQuantize::Fifths,
            PitchQuantize::Major,
            PitchQuantize::Minor,
            PitchQuantize::Pentatonic,
        ];
        let divs = [
            Division::Quarter,
            Division::Eighth,
            Division::Sixteenth,
            Division::ThirtySecond,
            Division::EighthTriplet,
            Division::SixteenthTriplet,
        ];
        let mut beat = 0.0;
        let mut l = vec![0.0f32; 1024];
        let mut r = vec![0.0f32; 1024];
        for block in 0..600 {
            let mut s = snap();
            let wild = |rng: &mut Rng, lo: f32, hi: f32| lo + (hi - lo) * rng.next_f32();
            s.grain_size_ms = wild(&mut rng, -100.0, 2000.0);
            s.density_hz = wild(&mut rng, -10.0, 500.0);
            s.position = wild(&mut rng, -2.0, 3.0);
            s.position_jitter = wild(&mut rng, -2.0, 3.0);
            s.memory = wild(&mut rng, -2.0, 3.0);
            s.spread = wild(&mut rng, -2.0, 3.0);
            s.reverse_chance = wild(&mut rng, -1.0, 2.0);
            s.tape_stop_chance = wild(&mut rng, -1.0, 2.0);
            s.tape_start_chance = wild(&mut rng, -1.0, 2.0);
            s.pitch_st = wild(&mut rng, -200.0, 200.0);
            s.pitch_jitter_st = wild(&mut rng, -50.0, 80.0);
            s.speed = wild(&mut rng, -5.0, 50.0);
            s.buffer_seconds = wild(&mut rng, -5.0, 100.0);
            s.buffer_bars = (rng.next_u32() % 20) as i32 - 5;
            s.max_voices = (rng.next_u32() % 60) as i32 - 10;
            s.low_cpu = rng.chance(0.3);
            s.freeze = rng.chance(0.2);
            s.midi_pitch = rng.chance(0.2);
            s.envelope = envs[(rng.next_u32() % 4) as usize];
            s.pitch_quantize = quants[(rng.next_u32() % 6) as usize];
            s.density_division = divs[(rng.next_u32() % 6) as usize];
            s.density_mode = if rng.chance(0.5) { DensityMode::Free } else { DensityMode::Synced };
            s.buffer_mode = if rng.chance(0.5) { BufferMode::Free } else { BufferMode::Synced };
            if block % 50 == 0 {
                s.pitch_st = f32::NAN;
                s.speed = f32::INFINITY;
            }
            if rng.chance(0.1) {
                e.note_on((rng.next_u32() % 128) as u8, 1.0);
            }
            if rng.chance(0.1) {
                e.note_off((rng.next_u32() % 128) as u8);
            }
            let n = 1 + (rng.next_u32() % 1024) as usize;
            for i in 0..n {
                l[i] = rng.bipolar();
                r[i] = rng.bipolar();
            }
            let bpm = wild(&mut rng, 1.0, 400.0) as f64;
            let t = transport(bpm, beat, n);
            e.process(&mut l[..n], &mut r[..n], &s, &t);
            beat += n as f64 * bpm / 60.0 / SR as f64;
            for i in 0..n {
                assert!(l[i].is_finite() && r[i].is_finite(), "non-finite at block {block}");
                assert!(l[i].abs() < 100.0 && r[i].abs() < 100.0, "runaway at block {block}");
            }
        }
    }

    #[test]
    fn reverse_grains_read_backwards() {
        let mut e = engine();
        let mut s = snap();
        s.reverse_chance = 1.0;
        s.tape_stop_chance = 0.0;
        s.tape_start_chance = 0.0;
        s.position = 0.5;
        s.position_jitter = 0.0;
        // Fill the buffer with a rising ramp, then freeze it.
        run(&mut e, &s, 96_000, 512, |i| i as f32 / 96_000.0);
        s.freeze = true;
        s.max_voices = 1;
        s.density_hz = 1.0;
        s.grain_size_ms = 200.0;
        s.spread = 0.0;
        s.envelope = GrainEnvelope::Square;
        // Reset the scheduler so a grain spawns now.
        e.countdown = 0.0;
        e.voices = [Voice::IDLE; MAX_VOICES];
        let mut last_pos: Option<f64> = None;
        let mut last_out: Option<f32> = None;
        let mut checked = 0;
        for _ in 0..4_000 {
            let mut l = [0.0f32];
            let mut r = [0.0f32];
            e.process(&mut l, &mut r, &s, &transport(120.0, 0.0, 1));
            let v = e.voices.iter().find(|v| v.active).copied();
            let Some(v) = v else { break };
            assert!(v.reverse);
            if let Some(p) = last_pos {
                assert!(v.pos < p, "position must decrease: {} -> {}", p, v.pos);
            }
            last_pos = Some(v.pos);
            // On the envelope plateau, the ramp read backwards must fall.
            let edge = 0.002 * SR as f64 + 2.0;
            if v.age > edge && v.age < v.len - edge {
                if let Some(o) = last_out {
                    assert!(l[0] < o, "output must fall on a reversed ramp");
                    checked += 1;
                }
                last_out = Some(l[0]);
            }
        }
        assert!(checked > 1000, "only {checked} samples checked");
    }

    #[test]
    fn forward_grains_never_pass_write_head() {
        let mut e = engine();
        let mut s = snap();
        s.reverse_chance = 0.0;
        s.position = 0.0;
        s.position_jitter = 0.0;
        s.speed = 4.0;
        s.pitch_st = 24.0;
        run(&mut e, &s, 48_000, 64, |i| (i as f32 * 0.01).sin());
        let head = e.written as f64;
        for v in e.voices.iter().filter(|v| v.active) {
            assert!(v.pos <= head - 3.0 + 1e-9);
        }
    }

    #[test]
    fn quantize_snapping() {
        use PitchQuantize::*;
        assert_eq!(quantize_semitones(5.3, Off), 5.3);
        assert_eq!(quantize_semitones(5.0, Octaves), 0.0);
        assert_eq!(quantize_semitones(7.0, Octaves), 12.0);
        assert_eq!(quantize_semitones(-7.0, Octaves), -12.0);
        assert_eq!(quantize_semitones(4.0, Fifths), 7.0);
        assert_eq!(quantize_semitones(3.0, Fifths), 0.0);
        assert_eq!(quantize_semitones(10.0, Fifths), 12.0);
        assert_eq!(quantize_semitones(-5.0, Fifths), -5.0); // -12 + 7
        assert_eq!(quantize_semitones(1.2, Major), 2.0);
        assert_eq!(quantize_semitones(3.9, Major), 4.0);
        assert_eq!(quantize_semitones(6.4, Major), 7.0);
        assert_eq!(quantize_semitones(3.2, Minor), 3.0);
        assert_eq!(quantize_semitones(8.2, Minor), 8.0);
        assert_eq!(quantize_semitones(5.4, Pentatonic), 4.0);
        assert_eq!(quantize_semitones(5.6, Pentatonic), 7.0);
        assert_eq!(quantize_semitones(23.0, Pentatonic), 24.0);
        assert_eq!(quantize_semitones(-1.0, Major), -1.0); // B below the root
    }

    #[test]
    fn buffer_length() {
        let e = engine();
        let max = e.max_len();
        assert_eq!(max, (CAPACITY_SECONDS * SR) as usize + 8);
        let mut s = snap();
        s.buffer_mode = BufferMode::Free;
        s.buffer_seconds = 2.0;
        assert_eq!(effective_len(&s, &transport(120.0, 0.0, 64), max), 96_000);
        s.buffer_seconds = 100.0; // clamped to 8 s
        assert_eq!(effective_len(&s, &transport(120.0, 0.0, 64), max), 384_000);
        s.buffer_mode = BufferMode::Synced;
        s.buffer_bars = 2;
        // 2 bars × 4 beats × 24 000 samples per beat.
        assert_eq!(effective_len(&s, &transport(120.0, 0.0, 64), max), 192_000);
        s.buffer_bars = 8;
        // 8 bars at 30 BPM = 64 s, clamped to the 32 s capacity.
        assert_eq!(effective_len(&s, &transport(30.0, 0.0, 64), max), max);
        assert!(e.cap >= (CAPACITY_SECONDS * SR) as usize);
    }

    #[test]
    fn block_size_independent() {
        let mut s = snap();
        s.reverse_chance = 0.5;
        s.pitch_jitter_st = 5.0;
        s.position_jitter = 0.7;
        let input = |i: usize| ((i as f32) * 0.013).sin() * 0.5;
        let mut a = engine();
        let (la, _) = run(&mut a, &s, 20_000, 1, input);
        for block in [64, 512] {
            let mut b = engine();
            let (lb, _) = run(&mut b, &s, 20_000, block, input);
            for i in 0..la.len() {
                assert!((la[i] - lb[i]).abs() < 1e-5, "block {block} differs at {i}");
            }
        }
    }

    #[test]
    fn midi_gates_spawning_and_sets_pitch() {
        let mut e = engine();
        let mut s = snap();
        s.midi_pitch = true;
        s.pitch_jitter_st = 0.0;
        run(&mut e, &s, 24_000, 256, |_| 0.1);
        assert_eq!(e.next_id, 0, "no grains without a held note");
        e.note_on(60, 1.0);
        e.note_on(72, 1.0);
        e.note_off(60);
        run(&mut e, &s, 24_000, 256, |_| 0.1);
        assert!(e.next_id > 0);
        let v = e.voices.iter().max_by_key(|v| v.id).unwrap();
        assert!((v.rate - 2.0).abs() < 1e-9, "rate {}", v.rate);
        e.note_off(72);
        let before = e.next_id;
        run(&mut e, &s, 24_000, 256, |_| 0.1);
        assert_eq!(e.next_id, before);
        // MIDI ignored when the mode is off.
        s.midi_pitch = false;
        run(&mut e, &s, 24_000, 256, |_| 0.1);
        assert!(e.next_id > before);
    }

    #[test]
    fn voice_limit_and_stealing() {
        let mut e = engine();
        let mut s = snap();
        s.density_hz = 64.0;
        s.grain_size_ms = 500.0;
        s.max_voices = 32;
        s.low_cpu = true;
        run(&mut e, &s, 48_000, 256, |_| 0.1);
        let playing = e.voices.iter().filter(|v| v.active && !v.releasing).count();
        assert!(playing <= LOW_CPU_VOICES);
        assert!(playing > 0);
        // Visual bitmask matches active voices.
        let mask = e.visual.grain_active.load(Ordering::Relaxed);
        let active = e.voices.iter().filter(|v| v.active).count() as u32;
        assert_eq!(mask.count_ones(), active);
    }

    #[test]
    fn envelopes_are_bounded_and_close() {
        let len = 4800.0;
        for env in [
            GrainEnvelope::Hann,
            GrainEnvelope::Triangle,
            GrainEnvelope::Square,
            GrainEnvelope::ExpDecay,
        ] {
            assert!(envelope(env, 0.0, len, SR as f64) < 0.02);
            assert!(envelope(env, len - 1.0, len, SR as f64) < 0.02);
            for a in 0..4800 {
                let v = envelope(env, a as f64, len, SR as f64);
                assert!((0.0..=1.0).contains(&v));
            }
        }
    }

    #[test]
    fn waveform_fills_over_blocks() {
        let mut e = engine();
        let mut s = snap();
        s.channel_mode = ChannelMode::Stereo;
        run(&mut e, &s, 300 * 512, 512, |_| 0.5);
        for w in e.visual.waveform.iter() {
            assert!((crate::shared::load_f32(w) - 0.5).abs() < 1e-6);
        }
    }
}
