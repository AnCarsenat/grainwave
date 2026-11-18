//! OWNER: feat/rhythm. Stutter, swing, 16-step gate, rewind and tape-stop performance buttons.
//!
//! Per-sample chain (all in place on the block):
//! 1. Stutter boundary check on the swung `stutter_division` grid (may capture a slice).
//! 2. Write the live sample into the module's own 4 s circular buffer (unless frozen by rewind).
//! 3. Stutter mix: `live × (1 − Σg) + Σ g × slice` (two voices so slices can crossfade, 2 ms).
//! 4. Performance (rewind / tape stop): crossfaded over the result with a 10 ms ramp.
//! 5. Gate: 16 × 1/16 steps per bar on the swung grid, 3 ms gain ramps.
//!
//! The grid is computed per sample from `Transport::beat_pos` at block start plus
//! `i / samples_per_beat`, so host loops and seeks are picked up at the next block start
//! with no extra state: a step index change (forward or backward) is a boundary.
use super::rng::Rng;
use super::Transport;
use crate::params::Snapshot;
use crate::shared::VisualState;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Buffer capacity in seconds. 4 s per spec plus a small margin for interpolation.
const BUFFER_SECONDS: f32 = 4.1;
const STUTTER_XF_MS: f32 = 2.0;
const GATE_RAMP_MS: f32 = 3.0;
const PERF_XF_MS: f32 = 10.0;
/// Minimum read distance behind the newest sample (4-point Hermite needs i+2 <= newest).
const MIN_DIST: f64 = 2.0;
/// Tape-stop output gain is `min(1, rate × TAPE_AMP_SLOPE)`, reaching exact silence at rate 0.
const TAPE_AMP_SLOPE: f64 = 10.0;

#[derive(Clone, Copy, Default)]
struct StutterVoice {
    /// Absolute sample index (in `written` units) of the first slice sample.
    start: u64,
    len: usize,
    phase: usize,
    gain: f32,
    target: f32,
}

impl StutterVoice {
    #[inline]
    fn is_free(&self) -> bool {
        self.gain <= 0.0 && self.target <= 0.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PerfKind {
    Rewind,
    TapeStop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PerfMode {
    Idle,
    Rewind,
    TapeStop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TapePhase {
    Slowing,
    Stopped,
    SpinUp,
    Done,
}

pub struct Rhythm {
    visual: Arc<VisualState>,
    sample_rate: f32,

    // Circular buffer.
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    cap: usize,
    /// Number of samples written so far; the newest sample is at `written - 1`.
    written: u64,
    /// Rewind freezes writes so the material being rewound stays intact.
    frozen: bool,

    rng: Rng,

    // Stutter.
    voices: [StutterVoice; 2],
    cur_voice: Option<usize>,
    last_stutter_step: Option<i64>,
    stutter_xf: usize,

    // Gate.
    gate_gain: f32,
    gate_step: f32,
    gate_idx: u32,

    // Performance (rewind / tape stop).
    prev_rewind: bool,
    prev_tape_stop: bool,
    request: Option<PerfKind>,
    mode: PerfMode,
    perf_gain: f32,
    perf_target: f32,
    perf_step: f32,
    perf_xf: f64,
    /// Read distance behind the newest sample.
    dist: f64,
    rewind_t: f64,
    tape_phase: TapePhase,
    /// sqrt of the tape playback rate (rate = q²).
    tape_q: f64,
}

impl Rhythm {
    pub fn new(visual: Arc<VisualState>) -> Self {
        Self {
            visual,
            sample_rate: 48_000.0,
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            cap: 0,
            written: 0,
            frozen: false,
            rng: Rng::new(0x5259_5448),
            voices: [StutterVoice::default(); 2],
            cur_voice: None,
            last_stutter_step: None,
            stutter_xf: 96,
            gate_gain: 1.0,
            gate_step: 1.0 / 144.0,
            gate_idx: 0,
            prev_rewind: false,
            prev_tape_stop: false,
            request: None,
            mode: PerfMode::Idle,
            perf_gain: 0.0,
            perf_target: 0.0,
            perf_step: 1.0 / 480.0,
            perf_xf: 480.0,
            dist: MIN_DIST,
            rewind_t: 0.0,
            tape_phase: TapePhase::Done,
            tape_q: 1.0,
        }
    }

    pub fn reset(&mut self, sample_rate: f32, _max_block: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        self.sample_rate = sr;
        self.cap = ((sr * BUFFER_SECONDS).ceil() as usize).max(64);
        super::realloc_or_clear(&mut self.buf_l, self.cap);
        super::realloc_or_clear(&mut self.buf_r, self.cap);
        self.written = 0;
        self.frozen = false;
        self.rng = Rng::new(0x5259_5448);

        self.voices = [StutterVoice::default(); 2];
        self.cur_voice = None;
        self.last_stutter_step = None;
        self.stutter_xf = ((sr * STUTTER_XF_MS / 1000.0).round() as usize).max(1);

        self.gate_gain = 1.0;
        self.gate_step = 1.0 / (sr * GATE_RAMP_MS / 1000.0).max(1.0);
        self.gate_idx = 0;

        self.prev_rewind = false;
        self.prev_tape_stop = false;
        self.request = None;
        self.mode = PerfMode::Idle;
        self.perf_gain = 0.0;
        self.perf_target = 0.0;
        self.perf_xf = (sr as f64 * PERF_XF_MS as f64 / 1000.0).max(1.0);
        self.perf_step = 1.0 / self.perf_xf as f32;
        self.dist = MIN_DIST;
        self.rewind_t = 0.0;
        self.tape_phase = TapePhase::Done;
        self.tape_q = 1.0;
        self.visual.gate_step.store(0, Ordering::Relaxed);
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], snap: &Snapshot, t: &Transport) {
        if self.cap == 0 {
            return; // reset() not called yet: pass-through.
        }
        let n = l.len().min(r.len());
        let sr = self.sample_rate as f64;

        // ---- parameters (clamped) ----
        let chance = finite_or(snap.stutter_chance, 0.0).clamp(0.0, 1.0);
        let swing = finite_or(snap.swing, 0.0).clamp(0.0, 1.0) as f64;
        let stop_ms = finite_or(snap.tape_stop_time_ms, 600.0).clamp(50.0, 4000.0) as f64;
        let stop_samples = (stop_ms * 0.001 * sr).max(1.0);
        let div_beats = snap.stutter_division.beats();
        let spb = {
            let s = t.samples_per_beat();
            if s.is_finite() && s >= 1.0 { s } else { sr * 0.5 }
        };
        let inv_spb = 1.0 / spb;
        let beat0 = if t.beat_pos.is_finite() { t.beat_pos } else { 0.0 };
        // Slice length: one stutter division, limited so a held slice survives in the buffer.
        let slice_len = ((div_beats * spb).round() as usize)
            .clamp(4 * self.stutter_xf, self.cap / 4);

        self.update_perf_control(snap.rewind, snap.tape_stop);

        for i in 0..n {
            let beat = beat0 + i as f64 * inv_spb;

            // ---- stutter boundary (before writing, so the slice is the previous step) ----
            let step = swung_step(beat, div_beats, swing);
            if let Some(last) = self.last_stutter_step {
                if step != last {
                    self.stutter_boundary(chance, slice_len);
                }
            }
            self.last_stutter_step = Some(step);

            // ---- record ----
            let live_l = l[i];
            let live_r = r[i];
            if !self.frozen {
                let w = (self.written % self.cap as u64) as usize;
                self.buf_l[w] = live_l;
                self.buf_r[w] = live_r;
                self.written += 1;
            }

            // ---- stutter mix ----
            let (mut yl, mut yr) = self.stutter_sample(live_l, live_r);

            // ---- performance ----
            if self.mode != PerfMode::Idle {
                let (pl, pr) = self.perf_sample(stop_samples);
                let g = self.perf_gain;
                yl = yl * (1.0 - g) + pl * g;
                yr = yr * (1.0 - g) + pr * g;
                self.advance_perf_gain();
            }

            // ---- gate ----
            let gstep = swung_step(beat, 0.25, swing);
            let idx = gstep.rem_euclid(16) as usize;
            self.gate_idx = idx as u32;
            let gtarget = if !snap.gate_enabled || snap.gate_steps[idx] { 1.0 } else { 0.0 };
            if self.gate_gain < gtarget {
                self.gate_gain = (self.gate_gain + self.gate_step).min(gtarget);
            } else if self.gate_gain > gtarget {
                self.gate_gain = (self.gate_gain - self.gate_step).max(gtarget);
            }
            if self.gate_gain != 1.0 {
                yl *= self.gate_gain;
                yr *= self.gate_gain;
            }

            l[i] = yl;
            r[i] = yr;
        }

        self.visual.gate_step.store(self.gate_idx, Ordering::Relaxed);
    }

    // ------------------------------------------------------------------ stutter

    fn stutter_boundary(&mut self, chance: f32, slice_len: usize) {
        let roll = self.rng.chance(chance);
        if !roll {
            if let Some(c) = self.cur_voice.take() {
                self.voices[c].target = 0.0;
            }
            return;
        }
        // Keep repeating the held slice if it is still valid at the current tempo.
        if let Some(c) = self.cur_voice {
            let v = self.voices[c];
            let age = self.written.saturating_sub(v.start) + self.stutter_xf as u64;
            if v.len == slice_len && age + 2 * slice_len as u64 + 64 < self.cap as u64 {
                return;
            }
        }
        if self.written < slice_len as u64 {
            return; // not enough history yet
        }
        if let Some(c) = self.cur_voice.take() {
            self.voices[c].target = 0.0;
        }
        let slot = if self.voices[0].is_free() {
            0
        } else if self.voices[1].is_free() {
            1
        } else if self.voices[0].gain <= self.voices[1].gain {
            0
        } else {
            1
        };
        self.voices[slot] = StutterVoice {
            start: self.written - slice_len as u64,
            len: slice_len,
            phase: 0,
            gain: if self.voices[slot].is_free() { 0.0 } else { self.voices[slot].gain },
            target: 1.0,
        };
        self.cur_voice = Some(slot);
    }

    #[inline]
    fn stutter_sample(&mut self, live_l: f32, live_r: f32) -> (f32, f32) {
        let mut gsum = 0.0f32;
        let mut sl = 0.0f32;
        let mut sr = 0.0f32;
        let xf = self.stutter_xf;
        let xf_step = 1.0 / xf as f32;
        let cap = self.cap as i64;
        for vi in 0..2 {
            let v = self.voices[vi];
            if v.is_free() {
                continue;
            }
            let mut v = v;
            // Safety: if the slice is about to be overwritten (no boundary for a long time),
            // fade it out.
            let age = self.written.saturating_sub(v.start) + xf as u64;
            if age + 64 > self.cap as u64 {
                v.target = 0.0;
                if self.cur_voice == Some(vi) {
                    self.cur_voice = None;
                }
            }
            let p = v.phase;
            let idx = (v.start as i64 + p as i64).rem_euclid(cap) as usize;
            let mut vl = self.buf_l[idx];
            let mut vr = self.buf_r[idx];
            let tail = v.len - xf;
            if p >= tail {
                // Loop crossfade: blend the tail into the material just before the slice start,
                // so the wrap back to phase 0 is continuous.
                let j = p - tail;
                let a = j as f32 * xf_step;
                let pidx = (v.start as i64 - xf as i64 + j as i64).rem_euclid(cap) as usize;
                vl = vl * (1.0 - a) + self.buf_l[pidx] * a;
                vr = vr * (1.0 - a) + self.buf_r[pidx] * a;
            }
            sl += vl * v.gain;
            sr += vr * v.gain;
            gsum += v.gain;

            v.phase = if p + 1 >= v.len { 0 } else { p + 1 };
            if v.gain < v.target {
                v.gain = (v.gain + xf_step).min(v.target);
            } else if v.gain > v.target {
                v.gain = (v.gain - xf_step).max(v.target);
            }
            self.voices[vi] = v;
        }
        if gsum == 0.0 {
            return (live_l, live_r);
        }
        let live = (1.0 - gsum).max(0.0);
        (live_l * live + sl, live_r * live + sr)
    }

    // -------------------------------------------------------------- performance

    /// Block-rate control: edges of the momentary buttons drive the state machine.
    fn update_perf_control(&mut self, rewind: bool, tape_stop: bool) {
        if rewind && !self.prev_rewind {
            self.request = Some(PerfKind::Rewind);
        }
        if tape_stop && !self.prev_tape_stop {
            self.request = Some(PerfKind::TapeStop);
        }
        if !rewind && self.request == Some(PerfKind::Rewind) {
            self.request = if tape_stop { Some(PerfKind::TapeStop) } else { None };
        }
        if !tape_stop && self.request == Some(PerfKind::TapeStop) {
            self.request = if rewind { Some(PerfKind::Rewind) } else { None };
        }
        self.prev_rewind = rewind;
        self.prev_tape_stop = tape_stop;

        match (self.mode, self.request) {
            (PerfMode::Idle, Some(PerfKind::Rewind)) => {
                self.mode = PerfMode::Rewind;
                self.frozen = true;
                self.dist = MIN_DIST;
                self.rewind_t = 0.0;
                self.perf_target = 1.0;
            }
            (PerfMode::Idle, Some(PerfKind::TapeStop)) => {
                self.mode = PerfMode::TapeStop;
                self.tape_phase = TapePhase::Slowing;
                self.tape_q = 1.0;
                self.dist = MIN_DIST;
                self.perf_target = 1.0;
            }
            (PerfMode::Idle, None) => {}
            (PerfMode::Rewind, Some(PerfKind::Rewind)) => self.perf_target = 1.0,
            // Released, or switching to tape stop: fade out first, the new mode starts once idle.
            (PerfMode::Rewind, _) => self.perf_target = 0.0,
            (PerfMode::TapeStop, Some(PerfKind::TapeStop)) => {
                if matches!(self.tape_phase, TapePhase::SpinUp | TapePhase::Done) {
                    self.tape_phase = TapePhase::Slowing;
                }
                self.perf_target = 1.0;
            }
            (PerfMode::TapeStop, None) => {
                if matches!(self.tape_phase, TapePhase::Slowing | TapePhase::Stopped) {
                    self.tape_phase = TapePhase::SpinUp;
                }
            }
            (PerfMode::TapeStop, Some(PerfKind::Rewind)) => self.perf_target = 0.0,
        }
    }

    #[inline]
    fn perf_sample(&mut self, stop_samples: f64) -> (f32, f32) {
        let valid = self.written.min(self.cap as u64) as f64;
        let max_dist = (valid - 4.0).min(self.cap as f64 - 8.0);
        let amp = match self.mode {
            PerfMode::Rewind => {
                let speed = 1.0 + 2.0 * self.rewind_t.min(1.0);
                self.rewind_t += 1.0 / self.sample_rate as f64;
                // Writes are frozen, so the distance grows by the read speed only.
                self.dist = (self.dist + speed).min(max_dist).max(MIN_DIST);
                // Fade to silence when the rewind runs out of recorded material.
                ((max_dist - self.dist) / self.perf_xf).clamp(0.0, 1.0)
            }
            PerfMode::TapeStop => {
                match self.tape_phase {
                    TapePhase::Slowing => {
                        self.tape_q -= 1.0 / stop_samples;
                        if self.tape_q <= 0.0 {
                            self.tape_q = 0.0;
                            self.tape_phase = TapePhase::Stopped;
                        }
                    }
                    TapePhase::Stopped => {}
                    TapePhase::SpinUp => {
                        self.tape_q += 2.0 / stop_samples;
                        if self.tape_q >= 1.0 {
                            self.tape_q = 1.0;
                            self.tape_phase = TapePhase::Done;
                            self.perf_target = 0.0;
                        }
                    }
                    TapePhase::Done => {}
                }
                let rate = self.tape_q * self.tape_q;
                if self.tape_phase == TapePhase::Stopped {
                    // Silent: park the read head near "now" so spin-up starts from fresh audio
                    // and a long hold can never read overwritten material.
                    self.dist = MIN_DIST;
                } else {
                    // Writes continue, so the distance grows by (1 - rate) per sample.
                    self.dist = (self.dist + 1.0 - rate).min(max_dist).max(MIN_DIST);
                }
                (rate * TAPE_AMP_SLOPE).min(1.0)
            }
            PerfMode::Idle => return (0.0, 0.0),
        };
        if amp <= 0.0 {
            return (0.0, 0.0);
        }
        let pos = (self.written as f64 - 1.0) - self.dist;
        let (sl, sr) = self.read_cubic(pos);
        (sl * amp as f32, sr * amp as f32)
    }

    #[inline]
    fn advance_perf_gain(&mut self) {
        if self.perf_gain < self.perf_target {
            self.perf_gain = (self.perf_gain + self.perf_step).min(self.perf_target);
        } else if self.perf_gain > self.perf_target {
            self.perf_gain = (self.perf_gain - self.perf_step).max(self.perf_target);
        }
        if self.perf_gain <= 0.0 && self.perf_target <= 0.0 {
            self.mode = PerfMode::Idle;
            self.frozen = false;
            self.tape_phase = TapePhase::Done;
            self.tape_q = 1.0;
        }
    }

    /// 4-point cubic Hermite read at an absolute (fractional) sample position.
    #[inline]
    fn read_cubic(&self, pos: f64) -> (f32, f32) {
        let cap = self.cap as i64;
        let i = pos.floor();
        let f = (pos - i) as f32;
        let i = i as i64;
        let idx = |k: i64| (i + k).rem_euclid(cap) as usize;
        let (a, b, c, d) = (idx(-1), idx(0), idx(1), idx(2));
        (
            hermite(self.buf_l[a], self.buf_l[b], self.buf_l[c], self.buf_l[d], f),
            hermite(self.buf_r[a], self.buf_r[b], self.buf_r[c], self.buf_r[d], f),
        )
    }
}

#[inline]
fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

/// Step index on a grid of `len` beats where every odd step starts `swing × 0.5 × len` late.
#[inline]
fn swung_step(beat: f64, len: f64, swing: f64) -> i64 {
    // Tiny bias so a boundary that falls exactly on a sample is resolved the same way whatever
    // the block size (beat_pos + i / spb accumulates different rounding per block layout).
    let x = beat / len + 1e-7;
    let k = x.floor();
    let frac = x - k;
    let k = k as i64;
    if k.rem_euclid(2) == 1 && frac < swing * 0.5 {
        k - 1
    } else {
        k
    }
}

#[inline]
fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{Division, GrainwaveParams};

    const SR: f32 = 48_000.0;

    fn setup() -> (Rhythm, Snapshot, Arc<VisualState>) {
        let visual = Arc::new(VisualState::default());
        let mut r = Rhythm::new(visual.clone());
        r.reset(SR, 512);
        let snap = Snapshot::from_params(&GrainwaveParams::default());
        (r, snap, visual)
    }

    fn transport(beat_pos: f64, n: usize) -> Transport {
        Transport { sample_rate: SR, bpm: 120.0, beat_pos, playing: true, block_len: n }
    }

    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        (0..len).map(|_| rng.bipolar() * 0.5).collect()
    }

    /// Runs the whole signal through in blocks of `block`, advancing beat_pos at 120 BPM.
    fn run(
        rh: &mut Rhythm,
        snap: &Snapshot,
        l: &mut [f32],
        r: &mut [f32],
        block: usize,
        start_beat: f64,
    ) {
        let spb = SR as f64 * 0.5;
        let mut pos = 0;
        while pos < l.len() {
            let end = (pos + block).min(l.len());
            let t = transport(start_beat + pos as f64 / spb, end - pos);
            rh.process(&mut l[pos..end], &mut r[pos..end], snap, &t);
            pos = end;
        }
    }

    #[test]
    fn passthrough_when_everything_off() {
        let (mut rh, snap, _) = setup();
        let inl = noise(48_000, 1);
        let inr = noise(48_000, 2);
        let (mut l, mut r) = (inl.clone(), inr.clone());
        run(&mut rh, &snap, &mut l, &mut r, 256, 0.0);
        assert_eq!(l, inl);
        assert_eq!(r, inr);
    }

    #[test]
    fn silence_in_silence_out() {
        let (mut rh, mut snap, _) = setup();
        snap.stutter_chance = 1.0;
        snap.swing = 0.6;
        snap.gate_enabled = true;
        for (k, s) in snap.gate_steps.iter_mut().enumerate() {
            *s = k % 3 != 0;
        }
        snap.tape_stop_time_ms = 100.0;
        let n = 256;
        let spb = SR as f64 * 0.5;
        for b in 0..800 {
            snap.rewind = (b / 60) % 3 == 1;
            snap.tape_stop = (b / 50) % 4 == 2;
            let mut l = vec![0.0f32; n];
            let mut r = vec![0.0f32; n];
            rh.process(&mut l, &mut r, &snap, &transport((b * n) as f64 / spb, n));
            assert!(l.iter().chain(r.iter()).all(|&x| x == 0.0));
        }
    }

    #[test]
    fn finite_under_random_params() {
        let (mut rh, mut snap, _) = setup();
        let mut rng = Rng::new(77);
        let divs = [
            Division::Quarter,
            Division::Eighth,
            Division::Sixteenth,
            Division::ThirtySecond,
            Division::EighthTriplet,
            Division::SixteenthTriplet,
        ];
        let mut beat = 0.0f64;
        for _ in 0..3000 {
            snap.stutter_chance = rng.next_f32() * 3.0 - 1.0;
            snap.swing = rng.next_f32() * 3.0 - 1.0;
            snap.stutter_division = divs[(rng.next_u32() % 6) as usize];
            snap.gate_enabled = rng.chance(0.5);
            for s in snap.gate_steps.iter_mut() {
                *s = rng.chance(0.5);
            }
            snap.rewind = rng.chance(0.3);
            snap.tape_stop = rng.chance(0.3);
            snap.tape_stop_time_ms = rng.next_f32() * 10_000.0 - 1000.0;
            let n = 1 + (rng.next_u32() % 512) as usize;
            let bpm = 20.0 + rng.next_f32() as f64 * 400.0;
            if rng.chance(0.05) {
                beat = rng.next_f32() as f64 * 64.0 - 8.0; // loop / seek
            }
            let t = Transport { sample_rate: SR, bpm, beat_pos: beat, playing: true, block_len: n };
            beat += n as f64 * bpm / 60.0 / SR as f64;
            let mut l: Vec<f32> = (0..n).map(|_| rng.bipolar()).collect();
            let mut r: Vec<f32> = (0..n).map(|_| rng.bipolar()).collect();
            rh.process(&mut l, &mut r, &snap, &t);
            for &x in l.iter().chain(r.iter()) {
                assert!(x.is_finite() && x.abs() < 4.0, "bad sample {x}");
            }
        }
    }

    #[test]
    fn gate_mutes_off_steps() {
        let (mut rh, mut snap, visual) = setup();
        snap.gate_enabled = true;
        for (k, s) in snap.gate_steps.iter_mut().enumerate() {
            *s = k % 2 == 0;
        }
        let step = 6000; // 1/16 at 120 BPM, 48 kHz
        let len = step * 16;
        let mut l = vec![1.0f32; len];
        let mut r = vec![1.0f32; len];
        run(&mut rh, &snap, &mut l, &mut r, 64, 0.0);
        for k in 0..16 {
            let mid = k * step + step / 2;
            let expect = if k % 2 == 0 { 1.0 } else { 0.0 };
            assert!((l[mid] - expect).abs() < 1e-6, "step {k}: {}", l[mid]);
            assert!((r[mid] - expect).abs() < 1e-6);
            // 3 ms ramp (144 samples) is done shortly after the boundary.
            let after = k * step + 200;
            assert!((l[after] - expect).abs() < 1e-6, "step {k} after ramp: {}", l[after]);
        }
        // No hard jumps: the ramp limits per-sample change to 1/144.
        for w in l.windows(2) {
            assert!((w[1] - w[0]).abs() <= 1.0 / 144.0 + 1e-6);
        }
        assert_eq!(visual.gate_step.load(Ordering::Relaxed), 15);
    }

    #[test]
    fn gate_swing_delays_odd_steps() {
        let (mut rh, mut snap, _) = setup();
        snap.gate_enabled = true;
        snap.swing = 1.0; // odd steps start half a step late
        for (k, s) in snap.gate_steps.iter_mut().enumerate() {
            *s = k % 2 == 0;
        }
        let step = 6000;
        let mut l = vec![1.0f32; step * 4];
        let mut r = l.clone();
        run(&mut rh, &snap, &mut l, &mut r, 64, 0.0);
        // Step 1 nominally starts at 6000; with full swing it starts at 9000.
        assert!((l[8500] - 1.0).abs() < 1e-6);
        assert!(l[9500].abs() < 1e-6);
        // Step 2 starts on the grid at 12000.
        assert!((l[12_500] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stutter_repeats_audio_when_chance_is_one() {
        let (mut rh, mut snap, _) = setup();
        snap.stutter_chance = 1.0;
        snap.stutter_division = Division::Sixteenth;
        let len = 6000;
        let inl = noise(len * 5, 3);
        let inr = noise(len * 5, 4);
        let (mut l, mut r) = (inl.clone(), inr.clone());
        run(&mut rh, &snap, &mut l, &mut r, 128, 0.0);
        // First step passes through untouched.
        assert_eq!(&l[..len], &inl[..len]);
        // From the first boundary on, the slice [0, len) repeats (outside the 2 ms edges).
        for rep in 1..5 {
            for k in 200..(len - 200) {
                let n = rep * len + k;
                assert!((l[n] - inl[k]).abs() < 1e-6, "rep {rep} k {k}");
                assert!((r[n] - inr[k]).abs() < 1e-6);
            }
        }
        // And it is not just the live signal.
        let diff: f32 = (len * 2..len * 3).map(|n| (l[n] - inl[n]).abs()).sum();
        assert!(diff > 100.0);
    }

    #[test]
    fn tape_stop_reaches_silence_then_recovers() {
        let (mut rh, mut snap, _) = setup();
        snap.tape_stop_time_ms = 200.0;
        let n = 64;
        let spb = SR as f64 * 0.5;
        let total_blocks = 600;
        let release_block = 300; // 0.4 s
        let mut rng = Rng::new(9);
        for b in 0..total_blocks {
            snap.tape_stop = b < release_block;
            let inl: Vec<f32> = (0..n).map(|_| rng.bipolar() * 0.5).collect();
            let mut l = inl.clone();
            let mut r = inl.clone();
            rh.process(&mut l, &mut r, &snap, &transport((b * n) as f64 / spb, n));
            let t_ms = (b * n) as f32 / SR * 1000.0;
            if (215.0..399.0).contains(&t_ms) {
                assert!(l.iter().chain(r.iter()).all(|x| x.abs() < 1e-9), "not silent at {t_ms} ms");
            }
            // Spin-up takes 100 ms, then a 10 ms crossfade back to live.
            if t_ms > 400.0 + 100.0 + 15.0 {
                assert_eq!(l, inl, "not live at {t_ms} ms");
            }
        }
    }

    #[test]
    fn rewind_plays_backwards() {
        let (mut rh, mut snap, _) = setup();
        let n = 64;
        let spb = SR as f64 * 0.5;
        // Slow ramp input, so backwards playback shows up as a falling signal.
        let mut sample = 0usize;
        let mut out = Vec::new();
        for b in 0..400 {
            snap.rewind = b >= 200;
            let inl: Vec<f32> = (0..n).map(|k| (sample + k) as f32 * 1e-5).collect();
            sample += n;
            let mut l = inl.clone();
            let mut r = inl.clone();
            rh.process(&mut l, &mut r, &snap, &transport((b * n) as f64 / spb, n));
            if b >= 210 {
                out.extend_from_slice(&l);
            }
        }
        for w in out.windows(2) {
            assert!(w[1] <= w[0] + 1e-6, "rewind output should fall");
        }
        assert!(out[0] - out[out.len() - 1] > 0.05);
    }

    #[test]
    fn block_size_independent_without_buttons() {
        let mut outs = Vec::new();
        for &block in &[1usize, 64, 512] {
            let (mut rh, mut snap, _) = setup();
            snap.stutter_chance = 0.5;
            snap.swing = 0.3;
            snap.stutter_division = Division::EighthTriplet;
            snap.gate_enabled = true;
            snap.gate_steps[3] = false;
            snap.gate_steps[6] = false;
            let mut l = noise(96_000, 5);
            let mut r = noise(96_000, 6);
            run(&mut rh, &snap, &mut l, &mut r, block, 0.37);
            outs.push((l, r));
        }
        for o in &outs[1..] {
            for (a, b) in o.0.iter().zip(outs[0].0.iter()) {
                assert!((a - b).abs() < 1e-6);
            }
        }
    }
}
