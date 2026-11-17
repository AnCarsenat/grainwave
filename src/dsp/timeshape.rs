//! OWNER: feat/timeshape. Time shaper: per-cycle drawn curve sets how far behind live the
//! playback reads (see `pattern.rs` for curve semantics).
//!
//! Per-sample chain (all in place on the block):
//! 1. Write the live sample into a 16 s stereo delay line.
//! 2. Cycle position x: Sync = `frac(beat / cycle_beats + phase)` from the host playhead;
//!    MIDI = a note-on restarts x at `phase`, x runs one cycle then holds at its last value
//!    (the delay stays at the curve's end value until the next note). Before the first note
//!    in MIDI mode the module plays live (delay 0).
//! 3. Pending pattern switches (param edge or MIDI note % 12) are applied on the
//!    `ts_pattern_quantize` grid (immediately when Off or the transport is stopped).
//! 4. Target delay = `value_at(x, tension) × depth × cycle_samples`, clamped to the available
//!    history, then optionally one-pole smoothed (`ts_smooth`, 0..~200 ms, squared curve).
//! 5. Anti-click: a discontinuity (the per-sample delay change differs from the previous
//!    change by more than `JUMP_THRESHOLD` samples) starts a linear crossfade from the old read
//!    head (which keeps moving at its previous rate) to the new one over 0/2/6/15 ms.
//! 6. Read with 4-point cubic Hermite, then equal-gain (linear) mix with the live input.
//!
//! Pattern data: a preallocated local copy of the 12 curves, refreshed with `try_read` when
//! `pattern_version` changes. The audio thread never blocks; a busy lock retries next block.
use super::Transport;
use crate::params::{AntiClick, Snapshot, TimeTrigger};
use crate::pattern::{value_at, PatternBank, Point, MAX_POINTS, NUM_PATTERNS};
use crate::shared::{store_f32, VisualState};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock, TryLockError};

/// Delay line length: longest cycle (4 bars at 60 BPM) plus a guard for interpolation.
const BUFFER_SECONDS: f32 = 16.0;
const GUARD_SAMPLES: usize = 16;
/// A change in per-sample delay slope larger than this (in samples) counts as a jump.
const JUMP_THRESHOLD: f64 = 4.0;
/// Longest delay smoothing time at `ts_smooth = 1`.
const SMOOTH_MAX_S: f32 = 0.2;
/// Smoothing for the internal dry/wet mix.
const MIX_SMOOTH_MS: f32 = 20.0;
/// Largest f32 below 1.0, so a held cursor never wraps back to the cycle start.
const X_MAX: f32 = 1.0 - f32::EPSILON / 2.0;

pub struct TimeShaper {
    patterns: Arc<RwLock<PatternBank>>,
    version: Arc<AtomicU32>,
    visual: Arc<VisualState>,

    /// Audio-thread copy of the bank: NUM_PATTERNS vectors with MAX_POINTS capacity each.
    local: Vec<Vec<Point>>,
    seen_version: u32,

    sr: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    /// Index of the newest written sample.
    wpos: usize,
    /// Samples written since reset, saturating at the buffer length.
    filled: usize,

    // pattern selection
    active: usize,
    pending: Option<usize>,
    last_param_pattern: Option<i32>,

    // MIDI events (recorded by `note_on`, consumed at the next block start)
    note_pending: Option<u8>,
    note_trigger: bool,
    midi_started: bool,
    midi_elapsed: f64,

    // delay state, in samples
    delay: f64,
    prev_delay: f64,
    prev_slope: f64,
    // anti-click second head
    old_delay: f64,
    old_slope: f64,
    fade_pos: f64,
    fade_active: bool,

    mix: f32,
    mix_init: bool,
    mix_coef: f32,
}

impl TimeShaper {
    pub fn new(
        patterns: Arc<RwLock<PatternBank>>,
        version: Arc<AtomicU32>,
        visual: Arc<VisualState>,
    ) -> Self {
        let local = (0..NUM_PATTERNS).map(|_| Vec::with_capacity(MAX_POINTS)).collect();
        let mut s = Self {
            patterns,
            version,
            visual,
            local,
            seen_version: 0,
            sr: 44_100.0,
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            wpos: 0,
            filled: 0,
            active: 0,
            pending: None,
            last_param_pattern: None,
            note_pending: None,
            note_trigger: false,
            midi_started: false,
            midi_elapsed: 0.0,
            delay: 0.0,
            prev_delay: 0.0,
            prev_slope: 0.0,
            old_delay: 0.0,
            old_slope: 0.0,
            fade_pos: 1.0,
            fade_active: false,
            mix: 1.0,
            mix_init: false,
            mix_coef: 1.0,
        };
        // Blocking read is fine here (not the audio thread).
        let v = s.version.load(Ordering::Acquire);
        let patterns = s.patterns.clone();
        let guard = match patterns.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        copy_bank(&mut s.local, &guard);
        drop(guard);
        s.seen_version = v;
        s
    }

    pub fn reset(&mut self, sample_rate: f32, _max_block: usize) {
        self.sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 44_100.0 };
        let len = (BUFFER_SECONDS * self.sr).ceil() as usize + GUARD_SAMPLES;
        super::realloc_or_clear(&mut self.buf_l, len);
        super::realloc_or_clear(&mut self.buf_r, len);
        self.wpos = len - 1;
        self.filled = 0;
        self.pending = None;
        self.last_param_pattern = None;
        self.note_pending = None;
        self.note_trigger = false;
        self.midi_started = false;
        self.midi_elapsed = 0.0;
        self.delay = 0.0;
        self.prev_delay = 0.0;
        self.prev_slope = 0.0;
        self.old_delay = 0.0;
        self.old_slope = 0.0;
        self.fade_pos = 1.0;
        self.fade_active = false;
        self.mix_init = false;
        self.mix_coef = 1.0 - (-1.0 / (MIX_SMOOTH_MS * 0.001 * self.sr)).exp();
    }

    /// Records the note; it is applied (cycle restart / pattern switch) at the next block.
    pub fn note_on(&mut self, note: u8, _velocity: f32) {
        self.note_pending = Some(note);
        self.note_trigger = true;
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], snap: &Snapshot, t: &Transport) {
        let n = l.len().min(r.len());
        if self.buf_l.is_empty() || n == 0 {
            return;
        }
        self.refresh_patterns();

        // ---- sanitized parameters ----
        let depth = finite_or(snap.ts_depth, 0.0).clamp(0.0, 1.0) as f64;
        let phase = finite_or(snap.ts_phase, 0.0).clamp(0.0, 1.0) as f64;
        let tension = finite_or(snap.ts_tension, 0.0).clamp(-1.0, 1.0);
        let smooth = finite_or(snap.ts_smooth, 0.0).clamp(0.0, 1.0);
        let mix_target = finite_or(snap.ts_mix, 1.0).clamp(0.0, 1.0);
        let fade_len = match snap.ts_anti_click {
            AntiClick::Off => 0.0,
            AntiClick::Low => 0.002,
            AntiClick::Medium => 0.006,
            AntiClick::High => 0.015,
        } * self.sr as f64;
        let fade_step = if fade_len >= 1.0 { 1.0 / fade_len } else { 1.0 };
        let smooth_coef = if smooth > 0.0 {
            let tau = smooth * smooth * SMOOTH_MAX_S;
            (1.0 - (-1.0 / (tau * self.sr)).exp()) as f64
        } else {
            1.0
        };

        let bpm = if t.bpm.is_finite() && t.bpm > 0.0 { t.bpm } else { 120.0 };
        let spb = self.sr as f64 * 60.0 / bpm;
        let cap = self.buf_l.len();
        let cycle_beats = snap.ts_sync.beats();
        let cycle = (cycle_beats * spb).clamp(1.0, (cap - GUARD_SAMPLES) as f64);
        let beat0 = if t.beat_pos.is_finite() { t.beat_pos } else { 0.0 };
        let beat_step = 1.0 / spb;

        // ---- pattern switch requests ----
        let param_pat = (snap.ts_pattern.clamp(1, NUM_PATTERNS as i32) - 1) as usize;
        match self.last_param_pattern {
            None => {
                self.active = param_pat;
                self.pending = None;
            }
            Some(prev) if prev != snap.ts_pattern => self.pending = Some(param_pat),
            _ => {}
        }
        self.last_param_pattern = Some(snap.ts_pattern);

        if let Some(note) = self.note_pending.take() {
            if snap.ts_midi_switch {
                self.pending = Some(note as usize % NUM_PATTERNS);
            }
        }
        if self.note_trigger {
            self.note_trigger = false;
            if snap.ts_trigger == TimeTrigger::Midi {
                self.midi_started = true;
                self.midi_elapsed = 0.0;
            }
        }

        // Sample index at which a pending switch is applied (usize::MAX = not this block).
        let switch_at = match (self.pending, snap.ts_pattern_quantize.beats()) {
            (None, _) => usize::MAX,
            (Some(_), None) => 0,
            (Some(_), Some(_)) if !t.playing => 0,
            (Some(_), Some(g)) => {
                let q = beat0 / g;
                let to_boundary = (q.ceil() - q) * g;
                if to_boundary < 1e-9 {
                    0
                } else {
                    let idx = (to_boundary * spb).ceil();
                    if idx < n as f64 { idx as usize } else { usize::MAX }
                }
            }
        };

        let midi_step = 1.0 / cycle;
        let mut x_last = 0.0f32;
        let mut y_last = 0.0f32;

        for i in 0..n {
            if i == switch_at {
                if let Some(p) = self.pending.take() {
                    self.active = p;
                }
            }

            // 1. write
            self.wpos += 1;
            if self.wpos >= cap {
                self.wpos = 0;
            }
            self.buf_l[self.wpos] = if l[i].is_finite() { l[i] } else { 0.0 };
            self.buf_r[self.wpos] = if r[i].is_finite() { r[i] } else { 0.0 };
            if self.filled < cap {
                self.filled += 1;
            }
            let max_delay = self.filled.saturating_sub(4) as f64;

            // 2. cursor + target delay
            let (x, live) = match snap.ts_trigger {
                TimeTrigger::Sync => {
                    let beat = beat0 + i as f64 * beat_step;
                    (wrap01(beat / cycle_beats + phase), false)
                }
                TimeTrigger::Midi => {
                    if self.midi_started {
                        let e = self.midi_elapsed;
                        self.midi_elapsed = (self.midi_elapsed + midi_step).min(1.0);
                        let x = wrap01(phase + e);
                        let start = wrap01(phase);
                        if e >= 1.0 || (e > 0.5 && x == start) {
                            // Cycle done: hold just before the start point (end value).
                            (before(start), false)
                        } else {
                            (x, false)
                        }
                    } else {
                        (wrap01(phase), true)
                    }
                }
            };
            let y = if live { 0.0 } else { value_at(&self.local[self.active], x, tension) };
            let y = finite_or(y, 0.0).clamp(0.0, 1.0);
            let target = (y as f64 * depth * cycle).clamp(0.0, max_delay);

            // 3. smoothing
            if smooth_coef >= 1.0 {
                self.delay = target;
            } else {
                self.delay += smooth_coef * (target - self.delay);
                self.delay = self.delay.clamp(0.0, max_delay);
            }

            // 4. jump detection
            let slope = self.delay - self.prev_delay;
            if (slope - self.prev_slope).abs() > JUMP_THRESHOLD {
                if fade_len >= 1.0 {
                    if self.fade_active && self.fade_pos >= 0.5 {
                        // The new head was dominant: it becomes the old head.
                        self.old_delay = self.prev_delay;
                        self.old_slope = self.prev_slope;
                        self.fade_pos = 1.0 - self.fade_pos;
                    } else if !self.fade_active {
                        self.old_delay = self.prev_delay;
                        self.old_slope = self.prev_slope;
                        self.fade_pos = 0.0;
                    }
                    self.fade_active = true;
                }
                // Keep prev_slope: the slope after a jump is compared to the one before it.
            } else {
                self.prev_slope = slope;
            }
            self.prev_delay = self.delay;

            // 5. read
            let (mut wl, mut wr) = self.read(self.delay);
            if self.fade_active {
                self.old_delay = (self.old_delay + self.old_slope).clamp(0.0, max_delay);
                self.fade_pos += fade_step;
                if self.fade_pos >= 1.0 {
                    self.fade_active = false;
                    self.fade_pos = 1.0;
                } else {
                    let (ol, or) = self.read(self.old_delay);
                    let g = self.fade_pos as f32;
                    wl = ol * (1.0 - g) + wl * g;
                    wr = or * (1.0 - g) + wr * g;
                }
            }

            // 6. mix
            if !self.mix_init {
                self.mix = mix_target;
                self.mix_init = true;
            } else if self.mix != mix_target {
                self.mix += self.mix_coef * (mix_target - self.mix);
                if (self.mix - mix_target).abs() < 1e-5 {
                    self.mix = mix_target;
                }
            }
            let m = self.mix;
            if m >= 1.0 {
                l[i] = wl;
                r[i] = wr;
            } else if m > 0.0 {
                l[i] = l[i] * (1.0 - m) + wl * m;
                r[i] = r[i] * (1.0 - m) + wr * m;
            }

            x_last = x;
            y_last = y;
        }

        // ---- visuals ----
        store_f32(&self.visual.ts_cursor, x_last);
        let shown = if depth > 1e-6 {
            (self.delay / (depth * cycle)) as f32
        } else {
            y_last
        };
        store_f32(&self.visual.ts_value, finite_or(shown, 0.0).clamp(0.0, 1.0));
        self.visual.ts_active.store(self.active as u32, Ordering::Relaxed);
    }

    /// Copies the shared bank when its version changed. Never blocks.
    fn refresh_patterns(&mut self) {
        let v = self.version.load(Ordering::Acquire);
        if v == self.seen_version {
            return;
        }
        let guard = match self.patterns.try_read() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        copy_bank(&mut self.local, &guard);
        drop(guard);
        self.seen_version = v;
    }

    /// 4-point cubic Hermite read `d` samples behind the newest sample. `d` must lie in
    /// `0..=filled - 4`; integer delays return the stored sample exactly.
    #[inline]
    fn read(&self, d: f64) -> (f32, f32) {
        let cap = self.buf_l.len();
        let di = d.floor();
        let df = d - di;
        let di = di as usize;
        let at = |k: usize| -> usize {
            // index of the sample k steps behind the newest (k < cap)
            if k <= self.wpos { self.wpos - k } else { self.wpos + cap - k }
        };
        if df <= 0.0 {
            let j = at(di);
            return (self.buf_l[j], self.buf_r[j]);
        }
        // Read point lies between k1 = di + 1 (older) and k0 = di (newer).
        let f = (1.0 - df) as f32;
        let im1 = at(di + 2);
        let i0 = at(di + 1);
        let i1 = at(di);
        let i2 = at(di.saturating_sub(1));
        (
            hermite(self.buf_l[im1], self.buf_l[i0], self.buf_l[i1], self.buf_l[i2], f),
            hermite(self.buf_r[im1], self.buf_r[i0], self.buf_r[i1], self.buf_r[i2], f),
        )
    }
}

/// Copy the bank into preallocated storage (no allocation: capacity is MAX_POINTS).
fn copy_bank(local: &mut [Vec<Point>], bank: &PatternBank) {
    for (i, dst) in local.iter_mut().enumerate() {
        dst.clear();
        if let Some(p) = bank.patterns.get(i) {
            let k = p.points.len().min(MAX_POINTS);
            dst.extend_from_slice(&p.points[..k]);
        }
        if dst.is_empty() {
            dst.push(Point::new(0.0, 0.0));
        }
    }
}

#[inline]
fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

#[inline]
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

/// Largest f32 cursor strictly before `x` in cycle order (wrapping 0 to the cycle end).
#[inline]
fn before(x: f32) -> f32 {
    if x <= 0.0 { X_MAX } else { f32::from_bits(x.to_bits() - 1) }
}

/// Wraps into 0..1 as f32, never returning 1.0 (which `value_at` would wrap to 0).
#[inline]
fn wrap01(v: f64) -> f32 {
    if !v.is_finite() {
        return 0.0;
    }
    ((v - v.floor()) as f32).clamp(0.0, X_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::rng::Rng;
    use crate::params::{GrainwaveParams, PatternQuantize, TimeSync};
    use crate::pattern::{factory_bank, Pattern};
    use crate::shared::load_f32;

    const SR: f32 = 48_000.0;
    /// Samples per beat at 120 BPM.
    const SPB: usize = 24_000;

    struct Rig {
        ts: TimeShaper,
        bank: Arc<RwLock<PatternBank>>,
        version: Arc<AtomicU32>,
        visual: Arc<VisualState>,
        snap: Snapshot,
    }

    fn rig_with(bank: PatternBank) -> Rig {
        let bank = Arc::new(RwLock::new(bank));
        let version = Arc::new(AtomicU32::new(1));
        let visual = Arc::new(VisualState::default());
        let mut ts = TimeShaper::new(bank.clone(), version.clone(), visual.clone());
        ts.reset(SR, 1024);
        let snap = Snapshot::from_params(&GrainwaveParams::default());
        Rig { ts, bank, version, visual, snap }
    }

    fn rig() -> Rig {
        rig_with(factory_bank())
    }

    /// Factory bank with pattern `idx` replaced.
    fn bank_with(idx: usize, points: Vec<Point>) -> PatternBank {
        let mut b = factory_bank();
        b.patterns[idx] = Pattern { name: "test".into(), points };
        b
    }

    fn transport(beat_pos: f64, n: usize) -> Transport {
        Transport { sample_rate: SR, bpm: 120.0, beat_pos, playing: true, block_len: n }
    }

    /// Runs the signal through in blocks, advancing the playhead at 120 BPM from `start_beat`.
    fn run(rig: &mut Rig, l: &mut [f32], r: &mut [f32], block: usize, start_beat: f64) {
        let mut pos = 0;
        while pos < l.len() {
            let end = (pos + block).min(l.len());
            let t = transport(start_beat + pos as f64 / SPB as f64, end - pos);
            rig.ts.process(&mut l[pos..end], &mut r[pos..end], &rig.snap, &t);
            pos = end;
        }
    }

    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        (0..len).map(|_| rng.bipolar() * 0.5).collect()
    }

    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|n| n as f32 / SR).collect()
    }

    fn bits_eq(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
    }

    #[test]
    fn default_is_bit_exact_passthrough() {
        let mut rig = rig();
        let l_in = noise(30_000, 1);
        let r_in = noise(30_000, 2);
        let (mut l, mut r) = (l_in.clone(), r_in.clone());
        run(&mut rig, &mut l, &mut r, 333, 0.37);
        assert!(bits_eq(&l, &l_in));
        assert!(bits_eq(&r, &r_in));
    }

    #[test]
    fn flat_full_depth_full_mix_is_passthrough() {
        for (sync, ac, smooth) in [
            (TimeSync::Quarter, AntiClick::High, 0.5),
            (TimeSync::FourBars, AntiClick::Off, 1.0),
            (TimeSync::SixteenthTriplet, AntiClick::Low, 0.0),
        ] {
            let mut rig = rig();
            rig.snap.ts_pattern = 1;
            rig.snap.ts_depth = 1.0;
            rig.snap.ts_mix = 1.0;
            rig.snap.ts_sync = sync;
            rig.snap.ts_anti_click = ac;
            rig.snap.ts_smooth = smooth;
            let input = noise(20_000, 3);
            let (mut l, mut r) = (input.clone(), input.clone());
            run(&mut rig, &mut l, &mut r, 128, 1.0);
            assert!(bits_eq(&l, &input));
        }
    }

    #[test]
    fn ramp_curve_holds_cycle_start() {
        // y = x over a cycle: delay grows one sample per sample, the read head stands still.
        let mut rig = rig_with(bank_with(1, vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)]));
        rig.snap.ts_pattern = 2;
        rig.snap.ts_sync = TimeSync::Quarter;
        rig.snap.ts_anti_click = AntiClick::Off;
        let input = ramp(3 * SPB);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut rig, &mut l, &mut r, 256, 0.0);
        let held = input[SPB];
        for n in SPB + 4..2 * SPB - 4 {
            assert!((l[n] - held).abs() < 1e-3, "n {n}: {} vs {held}", l[n]);
        }
    }

    #[test]
    fn halftime_plays_half_speed() {
        let mut rig = rig();
        rig.snap.ts_pattern = 2; // factory "Halftime"
        rig.snap.ts_sync = TimeSync::Quarter;
        rig.snap.ts_anti_click = AntiClick::Off;
        let input = ramp(3 * SPB);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut rig, &mut l, &mut r, 512, 0.0);
        for k in 4..SPB - 4 {
            let expect = (SPB as f32 + k as f32 / 2.0) / SR;
            let got = l[SPB + k];
            assert!((got - expect).abs() < 1e-4, "k {k}: {got} vs {expect}");
        }
    }

    #[test]
    fn stutter_repeats_slice() {
        let mut rig = rig();
        rig.snap.ts_pattern = 3; // factory "Stutter 1/8 end"
        rig.snap.ts_sync = TimeSync::Quarter;
        rig.snap.ts_anti_click = AntiClick::Off;
        let input = noise(3 * SPB, 7);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut rig, &mut l, &mut r, 256, 0.0);
        // Cycle 2: the eighth of the cycle at 5/8 plays live, then twice more.
        let c = SPB;
        let e = SPB / 8;
        for j in 2..e - 2 {
            let src = input[c + 5 * e + j];
            assert_eq!(l[c + 5 * e + j], src);
            assert_eq!(l[c + 6 * e + j], src);
            assert_eq!(l[c + 7 * e + j], src);
        }
        // Before the stutter: live.
        assert_eq!(&l[c + 10..c + 5 * e], &input[c + 10..c + 5 * e]);
    }

    #[test]
    fn quantized_switch_waits_for_grid() {
        let mut rig = rig();
        rig.snap.ts_pattern_quantize = PatternQuantize::Beat;
        let mut a = vec![0.0f32; 512];
        let mut b = vec![0.0f32; 512];
        let active = |rig: &Rig| rig.visual.ts_active.load(Ordering::Relaxed);

        rig.ts.process(&mut a[..256], &mut b[..256], &rig.snap, &transport(0.0, 256));
        assert_eq!(active(&rig), 0);
        rig.snap.ts_pattern = 3;
        rig.ts.process(&mut a[..256], &mut b[..256], &rig.snap, &transport(0.3, 256));
        assert_eq!(active(&rig), 0, "switched before the grid");
        rig.ts.process(&mut a[..256], &mut b[..256], &rig.snap, &transport(0.6, 256));
        assert_eq!(active(&rig), 0, "switched before the grid");
        // This block crosses beat 1.0 (512 samples = 0.0213 beats).
        rig.ts.process(&mut a, &mut b, &rig.snap, &transport(0.99, 512));
        assert_eq!(active(&rig), 2);

        // Quantize Off: immediate.
        rig.snap.ts_pattern_quantize = PatternQuantize::Off;
        rig.snap.ts_pattern = 5;
        rig.ts.process(&mut a[..256], &mut b[..256], &rig.snap, &transport(1.3, 256));
        assert_eq!(active(&rig), 4);

        // Transport stopped: immediate even with a grid.
        rig.snap.ts_pattern_quantize = PatternQuantize::Bar;
        rig.snap.ts_pattern = 6;
        let mut t = transport(1.5, 256);
        t.playing = false;
        rig.ts.process(&mut a[..256], &mut b[..256], &rig.snap, &t);
        assert_eq!(active(&rig), 5);
    }

    #[test]
    fn midi_note_switches_pattern() {
        let mut rig = rig();
        rig.snap.ts_pattern_quantize = PatternQuantize::Off;
        let mut a = vec![0.0f32; 128];
        let mut b = vec![0.0f32; 128];
        rig.ts.process(&mut a, &mut b, &rig.snap, &transport(0.0, 128));
        rig.ts.note_on(62, 1.0);
        rig.ts.process(&mut a, &mut b, &rig.snap, &transport(0.1, 128));
        assert_eq!(rig.visual.ts_active.load(Ordering::Relaxed), 62 % 12);
        // Switch disabled: note ignored for selection.
        rig.snap.ts_midi_switch = false;
        rig.ts.note_on(65, 1.0);
        rig.ts.process(&mut a, &mut b, &rig.snap, &transport(0.2, 128));
        assert_eq!(rig.visual.ts_active.load(Ordering::Relaxed), 62 % 12);
    }

    #[test]
    fn midi_trigger_restarts_cycle_then_holds() {
        let mut rig = rig_with(bank_with(1, vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)]));
        rig.snap.ts_pattern = 2;
        rig.snap.ts_trigger = TimeTrigger::Midi;
        rig.snap.ts_midi_switch = false;
        rig.snap.ts_sync = TimeSync::Quarter;
        rig.snap.ts_anti_click = AntiClick::Off;
        let cursor = |rig: &Rig| load_f32(&rig.visual.ts_cursor);

        // Before any note: live.
        let input = noise(SPB, 9);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut rig, &mut l, &mut r, 256, 0.0);
        assert!(bits_eq(&l, &input));

        rig.ts.note_on(60, 1.0);
        let mut l = ramp(256);
        let mut r = l.clone();
        rig.ts.process(&mut l, &mut r, &rig.snap, &transport(7.3, 256));
        assert!(cursor(&rig) < 0.02, "cursor {}", cursor(&rig));

        // After more than one cycle the cursor holds at the cycle end, delay stays at its max.
        let mut l = ramp(2 * SPB);
        let mut r = l.clone();
        run(&mut rig, &mut l, &mut r, 256, 8.0);
        assert!(cursor(&rig) > 0.999, "cursor {}", cursor(&rig));
        assert!(load_f32(&rig.visual.ts_value) > 0.99);
        // Held delay of about one cycle: output follows the ramp one cycle late.
        let n = 2 * SPB - 10;
        assert!((l[n] - (n - SPB) as f32 / SR).abs() < 1e-3, "{} at {n}", l[n]);

        // A new note restarts.
        rig.ts.note_on(60, 1.0);
        let mut l = vec![0.0; 256];
        let mut r = l.clone();
        rig.ts.process(&mut l, &mut r, &rig.snap, &transport(12.0, 256));
        assert!(cursor(&rig) < 0.02);
    }

    fn max_step(x: &[f32]) -> f32 {
        x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn anti_click_bounds_jumps() {
        let steps = vec![
            Point::hold(0.0, 0.0),
            Point::hold(0.25, 0.3),
            Point::hold(0.5, 0.1),
            Point::hold(0.75, 0.6),
        ];
        let sine: Vec<f32> = (0..4 * SPB)
            .map(|n| 0.5 * (std::f32::consts::TAU * 233.0 * n as f32 / SR).sin())
            .collect();
        let natural = max_step(&sine);
        let render = |ac: AntiClick| {
            let mut rig = rig_with(bank_with(1, steps.clone()));
            rig.snap.ts_pattern = 2;
            rig.snap.ts_sync = TimeSync::Quarter;
            rig.snap.ts_anti_click = ac;
            let (mut l, mut r) = (sine.clone(), sine.clone());
            run(&mut rig, &mut l, &mut r, 256, 0.0);
            // Skip the first cycle (history still filling).
            max_step(&l[SPB..])
        };
        let off = render(AntiClick::Off);
        let medium = render(AntiClick::Medium);
        assert!(off > 0.1, "test pattern should click without anti-click: {off}");
        assert!(medium < natural + 0.01, "medium {medium} natural {natural}");
    }

    #[test]
    fn finite_under_random_params() {
        let mut rig = rig();
        let mut rng = Rng::new(0xC0FFEE);
        let syncs = [
            TimeSync::Sixteenth,
            TimeSync::Bar,
            TimeSync::FourBars,
            TimeSync::EighthTriplet,
            TimeSync::QuarterDotted,
        ];
        let quants = [PatternQuantize::Off, PatternQuantize::QuarterBeat, PatternQuantize::Bar];
        let acs = [AntiClick::Off, AntiClick::Low, AntiClick::Medium, AntiClick::High];
        let wild = |rng: &mut Rng, lo: f32, hi: f32| -> f32 {
            match rng.next_u32() % 10 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => -1e9,
                _ => lo + rng.next_f32() * (hi - lo),
            }
        };
        let mut beat = 0.0f64;
        for blk in 0..300u32 {
            rig.snap.ts_depth = wild(&mut rng, 0.0, 1.0);
            rig.snap.ts_phase = wild(&mut rng, 0.0, 1.0);
            rig.snap.ts_tension = wild(&mut rng, -1.0, 1.0);
            rig.snap.ts_smooth = wild(&mut rng, 0.0, 1.0);
            rig.snap.ts_mix = wild(&mut rng, 0.0, 1.0);
            rig.snap.ts_pattern = (rng.next_u32() % 20) as i32 - 4;
            rig.snap.ts_sync = syncs[rng.next_u32() as usize % syncs.len()];
            rig.snap.ts_pattern_quantize = quants[rng.next_u32() as usize % quants.len()];
            rig.snap.ts_anti_click = acs[rng.next_u32() as usize % acs.len()];
            rig.snap.ts_trigger =
                if rng.chance(0.5) { TimeTrigger::Sync } else { TimeTrigger::Midi };
            rig.snap.ts_midi_switch = rng.chance(0.5);
            if rng.chance(0.2) {
                rig.ts.note_on((rng.next_u32() % 128) as u8, 1.0);
            }
            if rng.chance(0.1) {
                beat = (rng.next_f32() * 100.0) as f64; // seek
            }
            if blk % 50 == 7 {
                // Random pattern edit + version bump.
                let mut b = rig.bank.write().unwrap();
                let p = &mut b.patterns[(rng.next_u32() % 12) as usize];
                p.points.clear();
                for _ in 0..1 + rng.next_u32() % 10 {
                    p.points.push(Point::hold(rng.next_f32(), rng.next_f32()));
                }
                p.sanitize();
                drop(b);
                rig.version.fetch_add(1, Ordering::Release);
            }
            let n = 1 + (rng.next_u32() % 512) as usize;
            let mut l = noise(n, blk + 1);
            let mut r = noise(n, blk + 1000);
            let mut t = transport(if rng.chance(0.05) { f64::NAN } else { beat }, n);
            t.bpm = 20.0 + rng.next_f32() as f64 * 280.0;
            t.playing = rng.chance(0.8);
            rig.ts.process(&mut l, &mut r, &rig.snap, &t);
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "block {blk}");
            beat += n as f64 * t.bpm / 60.0 / SR as f64;
        }
        assert!(load_f32(&rig.visual.ts_value).is_finite());
        assert!(load_f32(&rig.visual.ts_cursor).is_finite());
    }

    #[test]
    fn pattern_copy_follows_version() {
        let mut rig = rig();
        rig.snap.ts_pattern = 2;
        rig.snap.ts_sync = TimeSync::Quarter;
        rig.snap.ts_anti_click = AntiClick::Off;
        // Make pattern 2 flat and bump: pass-through.
        rig.bank.write().unwrap().patterns[1] = Pattern::flat("flat");
        rig.version.fetch_add(1, Ordering::Release);
        let input = noise(2 * SPB, 11);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut rig, &mut l, &mut r, 256, 0.0);
        assert!(bits_eq(&l, &input));

        // Edit to a constant y = 0.25 (delay = 6000 samples) while holding the write lock:
        // process must not block and must keep the old copy.
        {
            let mut b = rig.bank.write().unwrap();
            b.patterns[1].points = vec![Point::new(0.0, 0.25)];
            rig.version.fetch_add(1, Ordering::Release);
            let mut l = noise(256, 12);
            let l_in = l.clone();
            let mut r = l.clone();
            rig.ts.process(&mut l, &mut r, &rig.snap, &transport(2.0, 256));
            assert!(bits_eq(&l, &l_in));
            drop(b);
        }
        // Lock released: picked up at the next block.
        let input2 = noise(SPB, 13);
        let (mut l, mut r) = (input2.clone(), input2.clone());
        run(&mut rig, &mut l, &mut r, 256, 3.0);
        for n in 6000..SPB {
            assert_eq!(l[n], input2[n - 6000]);
        }
    }
}
