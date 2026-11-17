//! OWNER: feat/space-output. Small reverb, tempo-synced delay with crusher in feedback path.
//!
//! Signal order inside this module: delay -> reverb (echoes get reverberated).
//!
//! - Reverb: 8-line feedback delay network, Householder mixing matrix, one-pole damping per
//!   line, decay time 0.3–6 s (exponential in `reverb_size`). Equal-power wet/dry.
//! - Delay: stereo, tempo synced (`delay_division`), max 4 s. Time changes crossfade between two
//!   read heads (50 ms, linear so the loop gain stays <= feedback). Feedback path: 6 kHz one-pole low-pass, then crusher (sample-and-hold
//!   plus bit reduction). Equal-power wet/dry.
//!
//! Both effects stop taking input when their mix is 0 but keep running so the tail decays
//! naturally; once their internal state is effectively silent they go idle (zero CPU) and the
//! dry signal passes through untouched.
use super::Transport;
use crate::params::Snapshot;
use std::f32::consts::FRAC_PI_2;

/// Values below this are flushed to zero in feedback paths (denormal protection).
const DENORMAL: f32 = 1e-15;
/// Internal state below this is treated as silence for idle detection.
const QUIET: f32 = 1e-9;
/// Parameter smoothing time.
const SMOOTH_MS: f32 = 20.0;

#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < DENORMAL {
        0.0
    } else {
        x
    }
}

#[inline]
fn finite_or(x: f32, default: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        default
    }
}

#[inline]
fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// One-pole smoothing coefficient for a time constant in ms.
fn smooth_coef(ms: f32, sr: f32) -> f32 {
    (-1.0 / (ms * 0.001 * sr)).exp()
}

/// One-pole step toward `target`; snaps when close so bypass checks can hit exact values.
#[inline]
fn smooth(cur: f32, target: f32, c: f32) -> f32 {
    let v = target + (cur - target) * c;
    if (v - target).abs() < 1e-6 {
        target
    } else {
        v
    }
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

// ---------------------------------------------------------------------------
// Reverb
// ---------------------------------------------------------------------------

const LINES: usize = 8;
/// Line lengths in ms (sample-rate independent). Converted and nudged to be mutually prime.
const LINE_MS: [f32; LINES] = [23.1, 28.7, 33.3, 37.9, 42.1, 47.9, 53.3, 59.9];
const OUT_SIGN_L: [f32; LINES] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const OUT_SIGN_R: [f32; LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
const DAMP_HZ: f32 = 6500.0;
const T60_MIN: f32 = 0.3;
const T60_MAX: f32 = 6.0;
/// Input and output gain of the network (keeps the tail around unity loudness).
const REVERB_IN_GAIN: f32 = 0.5;
const REVERB_OUT_GAIN: f32 = 0.35;

struct Reverb {
    sr: f32,
    /// All lines in one flat buffer; line `i` occupies `offset[i] .. offset[i] + len[i]`.
    buf: Vec<f32>,
    offset: [usize; LINES],
    len: [usize; LINES],
    pos: [usize; LINES],
    damp_state: [f32; LINES],
    damp_a: f32,
    gain: [f32; LINES],
    mix: f32,
    send: f32,
    smooth_c: f32,
    quiet: usize,
    max_len: usize,
    fresh: bool,
}

impl Reverb {
    fn new() -> Self {
        Self {
            sr: 48_000.0,
            buf: Vec::new(),
            offset: [0; LINES],
            len: [1; LINES],
            pos: [0; LINES],
            damp_state: [0.0; LINES],
            damp_a: 1.0,
            gain: [0.0; LINES],
            mix: 0.0,
            send: 0.0,
            smooth_c: 0.0,
            quiet: usize::MAX,
            max_len: 1,
            fresh: true,
        }
    }

    fn reset(&mut self, sr: f32) {
        self.sr = sr.max(1000.0);
        let mut total = 0;
        for i in 0..LINES {
            let mut n = ((LINE_MS[i] * 0.001 * self.sr).round() as usize).max(2);
            // Nudge until coprime with every previous line (avoids coinciding echoes).
            while (0..i).any(|j| gcd(n, self.len[j]) != 1) {
                n += 1;
            }
            self.len[i] = n;
            self.offset[i] = total;
            total += n;
        }
        self.buf = vec![0.0; total];
        self.pos = [0; LINES];
        self.damp_state = [0.0; LINES];
        self.max_len = self.len.iter().copied().max().unwrap_or(1);
        let fc = DAMP_HZ.min(0.45 * self.sr);
        self.damp_a = 1.0 - (-std::f32::consts::TAU * fc / self.sr).exp();
        self.smooth_c = smooth_coef(SMOOTH_MS, self.sr);
        self.gain = [0.0; LINES];
        self.mix = 0.0;
        self.send = 0.0;
        self.quiet = usize::MAX;
        self.fresh = true;
    }

    fn target_gains(&self, size: f32) -> [f32; LINES] {
        let t60 = T60_MIN * (T60_MAX / T60_MIN).powf(size);
        let mut g = [0.0; LINES];
        for i in 0..LINES {
            // -60 dB after t60 seconds: g = 10^(-3 * len / (sr * t60)).
            g[i] = 10f32.powf(-3.0 * self.len[i] as f32 / (self.sr * t60));
        }
        g
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], size: f32, mix_target: f32) {
        let n = l.len().min(r.len());
        if n == 0 || self.buf.is_empty() {
            return;
        }
        let send_target = if mix_target > 0.0 { 1.0 } else { 0.0 };
        let target_g = self.target_gains(size);
        if self.fresh {
            self.mix = mix_target;
            self.send = send_target;
            self.gain = target_g;
            self.fresh = false;
        }

        // Idle: no input, no audible wet, tail already silent.
        if mix_target == 0.0 && self.mix == 0.0 && self.send == 0.0 && self.quiet > self.max_len {
            self.gain = target_g;
            return;
        }

        // Feedback gains ramp linearly across the block.
        let inv_n = 1.0 / n as f32;
        let mut g_step = [0.0; LINES];
        for i in 0..LINES {
            g_step[i] = (target_g[i] - self.gain[i]) * inv_n;
        }

        let c = self.smooth_c;
        let a = self.damp_a;
        for k in 0..n {
            self.mix = smooth(self.mix, mix_target, c);
            self.send = smooth(self.send, send_target, c);
            let in_l = l[k] * self.send * REVERB_IN_GAIN;
            let in_r = r[k] * self.send * REVERB_IN_GAIN;

            let mut v = [0.0f32; LINES];
            let mut out_l = 0.0;
            let mut out_r = 0.0;
            let mut sum = 0.0;
            for i in 0..LINES {
                self.gain[i] += g_step[i];
                let y = self.buf[self.offset[i] + self.pos[i]];
                out_l += OUT_SIGN_L[i] * y;
                out_r += OUT_SIGN_R[i] * y;
                let d = flush(self.damp_state[i] + a * (y - self.damp_state[i]));
                self.damp_state[i] = d;
                v[i] = d * self.gain[i];
                sum += v[i];
            }
            // Householder reflection: H = I - (2/N) * 1 * 1^T (orthogonal, lossless).
            let h = sum * (2.0 / LINES as f32);
            let mut energy = in_l.abs() + in_r.abs();
            for i in 0..LINES {
                let inj = if i % 2 == 0 { in_l } else { in_r };
                let w = flush(v[i] - h + inj);
                energy += w.abs();
                self.buf[self.offset[i] + self.pos[i]] = w;
                self.pos[i] += 1;
                if self.pos[i] >= self.len[i] {
                    self.pos[i] = 0;
                }
            }
            if energy < QUIET {
                self.quiet = self.quiet.saturating_add(1);
            } else {
                self.quiet = 0;
            }

            let theta = self.mix * FRAC_PI_2;
            let (wet, dry) = theta.sin_cos();
            l[k] = l[k] * dry + out_l * REVERB_OUT_GAIN * wet;
            r[k] = r[k] * dry + out_r * REVERB_OUT_GAIN * wet;
        }
        self.gain = target_g;
    }
}

// ---------------------------------------------------------------------------
// Delay
// ---------------------------------------------------------------------------

const DELAY_MAX_S: f32 = 4.0;
const XFADE_MS: f32 = 50.0;
const DELAY_LP_HZ: f32 = 6000.0;
const FEEDBACK_MAX: f32 = 0.95;
/// Crusher sample rate at full crush is `sr / CRUSH_SR_DIV` (48 kHz -> 4 kHz, matching 4 bits).
const CRUSH_SR_DIV: f32 = 12.0;

struct Delay {
    sr: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    w: usize,
    max_delay: f32,
    /// Current read-head delay in samples, and the one being faded to.
    cur_d: f32,
    next_d: f32,
    /// Crossfade progress 0..1, `None` when not fading.
    fade: Option<f32>,
    fade_inc: f32,
    lp_a: f32,
    lp_l: f32,
    lp_r: f32,
    hold_l: f32,
    hold_r: f32,
    hold_phase: f32,
    mix: f32,
    send: f32,
    fb: f32,
    smooth_c: f32,
    quiet: usize,
    fresh: bool,
}

impl Delay {
    fn new() -> Self {
        Self {
            sr: 48_000.0,
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            w: 0,
            max_delay: 1.0,
            cur_d: 1.0,
            next_d: 1.0,
            fade: None,
            fade_inc: 0.0,
            lp_a: 1.0,
            lp_l: 0.0,
            lp_r: 0.0,
            hold_l: 0.0,
            hold_r: 0.0,
            hold_phase: 0.0,
            mix: 0.0,
            send: 0.0,
            fb: 0.0,
            smooth_c: 0.0,
            quiet: usize::MAX,
            fresh: true,
        }
    }

    fn reset(&mut self, sr: f32) {
        self.sr = sr.max(1000.0);
        let max = (DELAY_MAX_S * self.sr).ceil() as usize;
        // +4 so a read at exactly max delay (with interpolation) never reaches the write head.
        self.buf_l = vec![0.0; max + 4];
        self.buf_r = vec![0.0; max + 4];
        self.w = 0;
        self.max_delay = max as f32;
        self.cur_d = 1.0;
        self.next_d = 1.0;
        self.fade = None;
        self.fade_inc = 1.0 / (XFADE_MS * 0.001 * self.sr);
        let fc = DELAY_LP_HZ.min(0.45 * self.sr);
        self.lp_a = 1.0 - (-std::f32::consts::TAU * fc / self.sr).exp();
        self.lp_l = 0.0;
        self.lp_r = 0.0;
        self.hold_l = 0.0;
        self.hold_r = 0.0;
        self.hold_phase = 0.0;
        self.smooth_c = smooth_coef(SMOOTH_MS, self.sr);
        self.mix = 0.0;
        self.send = 0.0;
        self.fb = 0.0;
        self.quiet = usize::MAX;
        self.fresh = true;
    }

    /// Linear-interpolated read `d` samples behind the write head (`d >= 1`).
    #[inline]
    fn read(buf: &[f32], w: usize, d: f32) -> f32 {
        let len = buf.len();
        let di = d.floor();
        let frac = d - di;
        let di = di as usize;
        let i0 = (w + len - di) % len;
        let i1 = (i0 + len - 1) % len;
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        delay_samples: f32,
        feedback: f32,
        crush: f32,
        mix_target: f32,
    ) {
        let n = l.len().min(r.len());
        if n == 0 || self.buf_l.is_empty() {
            return;
        }
        let target_d = delay_samples.clamp(1.0, self.max_delay);
        let send_target = if mix_target > 0.0 { 1.0 } else { 0.0 };
        if self.fresh {
            self.mix = mix_target;
            self.send = send_target;
            self.fb = feedback;
            self.cur_d = target_d;
            self.next_d = target_d;
            self.fresh = false;
        }

        let buf_len = self.buf_l.len();
        if mix_target == 0.0 && self.mix == 0.0 && self.send == 0.0 && self.quiet > buf_len {
            self.cur_d = target_d;
            self.next_d = target_d;
            self.fade = None;
            self.fb = feedback;
            return;
        }

        // Start a crossfade to the new time (a pending change waits for the current fade).
        if self.fade.is_none() && (target_d - self.cur_d).abs() > 0.5 {
            self.next_d = target_d;
            self.fade = Some(0.0);
        }

        // Crusher settings (per block): bits 16 -> 4, rate sr -> sr / 12. Bypass at crush 0.
        let crush_on = crush > 0.0;
        let bits = 16.0 - 12.0 * crush;
        let q = 2f32.powf(bits - 1.0);
        let inv_q = 1.0 / q;
        let hold_inc = (1.0 / CRUSH_SR_DIV).powf(crush);

        let c = self.smooth_c;
        let a = self.lp_a;
        for k in 0..n {
            self.mix = smooth(self.mix, mix_target, c);
            self.send = smooth(self.send, send_target, c);
            self.fb = smooth(self.fb, feedback, c);

            let (yl, yr) = match self.fade {
                None => (
                    Self::read(&self.buf_l, self.w, self.cur_d),
                    Self::read(&self.buf_r, self.w, self.cur_d),
                ),
                Some(f) => {
                    // Linear crossfade between the old and new read heads. Gains sum to 1, so
                    // the feedback loop gain never exceeds `feedback` (equal-power would not).
                    let (gn, go) = (f, 1.0 - f);
                    let yl = Self::read(&self.buf_l, self.w, self.cur_d) * go
                        + Self::read(&self.buf_l, self.w, self.next_d) * gn;
                    let yr = Self::read(&self.buf_r, self.w, self.cur_d) * go
                        + Self::read(&self.buf_r, self.w, self.next_d) * gn;
                    let f = f + self.fade_inc;
                    if f >= 1.0 {
                        self.cur_d = self.next_d;
                        self.fade = None;
                    } else {
                        self.fade = Some(f);
                    }
                    (yl, yr)
                }
            };

            // Feedback path: low-pass, then crusher.
            self.lp_l = flush(self.lp_l + a * (yl - self.lp_l));
            self.lp_r = flush(self.lp_r + a * (yr - self.lp_r));
            let (mut fl, mut fr) = (self.lp_l, self.lp_r);
            if crush_on {
                self.hold_phase += hold_inc;
                if self.hold_phase >= 1.0 {
                    self.hold_phase -= 1.0;
                    self.hold_l = (fl * q).round() * inv_q;
                    self.hold_r = (fr * q).round() * inv_q;
                }
                fl = self.hold_l;
                fr = self.hold_r;
            }

            let wl = flush(l[k] * self.send + fl * self.fb);
            let wr = flush(r[k] * self.send + fr * self.fb);
            self.buf_l[self.w] = wl;
            self.buf_r[self.w] = wr;
            self.w += 1;
            if self.w >= buf_len {
                self.w = 0;
            }
            if wl.abs() + wr.abs() < QUIET {
                self.quiet = self.quiet.saturating_add(1);
            } else {
                self.quiet = 0;
            }

            let (wet, dry) = (self.mix * FRAC_PI_2).sin_cos();
            l[k] = l[k] * dry + yl * wet;
            r[k] = r[k] * dry + yr * wet;
        }
    }
}

// ---------------------------------------------------------------------------
// Space
// ---------------------------------------------------------------------------

pub struct Space {
    reverb: Reverb,
    delay: Delay,
}

impl Default for Space {
    fn default() -> Self {
        Self::new()
    }
}

impl Space {
    pub fn new() -> Self {
        Self {
            reverb: Reverb::new(),
            delay: Delay::new(),
        }
    }

    pub fn reset(&mut self, sample_rate: f32, _max_block: usize) {
        self.reverb.reset(sample_rate);
        self.delay.reset(sample_rate);
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], snap: &Snapshot, t: &Transport) {
        // Never let a non-finite sample into the feedback networks.
        for s in l.iter_mut().chain(r.iter_mut()) {
            *s = sanitize(*s);
        }

        let spb = t.samples_per_beat();
        let spb = if spb.is_finite() && spb > 0.0 { spb } else { 24_000.0 };
        let delay_samples = (snap.delay_division.beats() * spb) as f32;
        let fb = finite_or(snap.delay_feedback, 0.0).clamp(0.0, FEEDBACK_MAX);
        let crush = finite_or(snap.delay_crush, 0.0).clamp(0.0, 1.0);
        let dmix = finite_or(snap.delay_mix, 0.0).clamp(0.0, 1.0);
        self.delay.process(l, r, delay_samples, fb, crush, dmix);

        let size = finite_or(snap.reverb_size, 0.5).clamp(0.0, 1.0);
        let rmix = finite_or(snap.reverb_mix, 0.0).clamp(0.0, 1.0);
        self.reverb.process(l, r, size, rmix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::rng::Rng;
    use crate::params::{Division, GrainwaveParams};

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 256;

    fn snap() -> Snapshot {
        let mut s = Snapshot::from_params(&GrainwaveParams::default());
        s.reverb_mix = 0.0;
        s.delay_mix = 0.0;
        s
    }

    fn transport(bpm: f64) -> Transport {
        Transport {
            sample_rate: SR,
            bpm,
            beat_pos: 0.0,
            playing: true,
            block_len: BLOCK,
        }
    }

    fn space() -> Space {
        let mut s = Space::new();
        s.reset(SR, BLOCK);
        s
    }

    /// Runs `input` (mono, duplicated to both channels) through `sp` in blocks; returns left.
    fn run(sp: &mut Space, input: &[f32], s: &Snapshot, t: &Transport) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(BLOCK) {
            let mut l = chunk.to_vec();
            let mut r = chunk.to_vec();
            sp.process(&mut l, &mut r, s, t);
            out.extend_from_slice(&l);
        }
        out
    }

    fn impulse(len: usize) -> Vec<f32> {
        let mut v = vec![0.0; len];
        v[0] = 1.0;
        v
    }

    #[test]
    fn transparent_at_mix_zero() {
        let mut sp = space();
        let mut rng = Rng::new(7);
        let input: Vec<f32> = (0..SR as usize).map(|_| rng.bipolar() * 0.5).collect();
        let mut s = snap();
        s.delay_feedback = 0.95;
        s.reverb_size = 1.0;
        let out = run(&mut sp, &input, &s, &transport(120.0));
        for (a, b) in input.iter().zip(out.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut sp = space();
        let mut s = snap();
        s.reverb_mix = 1.0;
        s.delay_mix = 1.0;
        s.delay_feedback = 0.95;
        s.delay_crush = 1.0;
        let out = run(&mut sp, &vec![0.0; SR as usize], &s, &transport(120.0));
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn stable_at_extremes() {
        let mut sp = space();
        let mut s = snap();
        s.reverb_mix = 1.0;
        s.reverb_size = 1.0;
        s.delay_mix = 1.0;
        s.delay_feedback = 0.95;
        let mut rng = Rng::new(3);
        let divs = [
            Division::Quarter,
            Division::Eighth,
            Division::Sixteenth,
            Division::ThirtySecond,
            Division::EighthTriplet,
            Division::SixteenthTriplet,
        ];
        let mut peak = 0.0f32;
        // 60 s of loud noise, randomizing crush, division and tempo (and out-of-range values).
        for b in 0..(60 * SR as usize / BLOCK) {
            if b % 200 == 0 {
                s.delay_crush = rng.next_f32() * 1.4 - 0.2;
                s.delay_feedback = 0.9 + rng.next_f32() * 0.3; // clamps to 0.95
                s.reverb_size = rng.next_f32() * 1.5;
                s.delay_division = divs[(rng.next_u32() % 6) as usize];
            }
            let t = transport(40.0 + rng.next_f32() as f64 * 200.0);
            let mut l: Vec<f32> = (0..BLOCK).map(|_| rng.bipolar()).collect();
            let mut r: Vec<f32> = (0..BLOCK).map(|_| rng.bipolar()).collect();
            sp.process(&mut l, &mut r, &s, &t);
            for &x in l.iter().chain(r.iter()) {
                assert!(x.is_finite());
                peak = peak.max(x.abs());
            }
        }
        assert!(peak < 100.0, "peak {peak}");
    }

    #[test]
    fn delay_time_matches_division() {
        for (div, bpm) in [
            (Division::Eighth, 120.0),
            (Division::Quarter, 90.0),
            (Division::SixteenthTriplet, 140.0),
        ] {
            let mut sp = space();
            let mut s = snap();
            s.delay_mix = 1.0;
            s.delay_feedback = 0.0;
            s.delay_division = div;
            let t = transport(bpm);
            let expected = (div.beats() * t.samples_per_beat()).round() as usize;
            let out = run(&mut sp, &impulse(expected + 4096), &s, &t);
            let (idx, _) = out
                .iter()
                .enumerate()
                .skip(1)
                .fold((0, 0.0f32), |(bi, bv), (i, &v)| {
                    if v.abs() > bv {
                        (i, v.abs())
                    } else {
                        (bi, bv)
                    }
                });
            assert!(
                (idx as i64 - expected as i64).abs() <= 1,
                "{div:?}: echo at {idx}, expected {expected}"
            );
        }
    }

    #[test]
    fn delay_time_change_is_smooth() {
        let mut sp = space();
        let mut s = snap();
        s.delay_mix = 1.0;
        s.delay_feedback = 0.5;
        let t = transport(120.0);
        // Steady sine, then switch division: no sample-to-sample jump beyond what the sine does.
        let w = std::f32::consts::TAU * 220.0 / SR;
        let input: Vec<f32> = (0..(3 * SR as usize)).map(|i| (i as f32 * w).sin() * 0.5).collect();
        let half = input.len() / 2 / BLOCK * BLOCK;
        let mut out = run(&mut sp, &input[..half], &s, &t);
        s.delay_division = Division::SixteenthTriplet;
        out.extend(run(&mut sp, &input[half..], &s, &t));
        let max_step = out[half - 10..half + 6000]
            .windows(2)
            .map(|p| (p[1] - p[0]).abs())
            .fold(0.0f32, f32::max);
        // A 220 Hz sine of amplitude A moves at most A*w per sample; output amplitude < 2.
        assert!(max_step < 2.0 * w * 1.5, "max step {max_step}");
    }

    #[test]
    fn reverb_tail_decays() {
        let mut sp = space();
        let mut s = snap();
        s.reverb_mix = 1.0;
        s.reverb_size = 0.0; // 0.3 s
        let len = 2 * SR as usize;
        let out = run(&mut sp, &impulse(len), &s, &transport(120.0));
        let rms = |a: &[f32]| (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt();
        let early = rms(&out[1000..5800]);
        let late = rms(&out[len - 4800..]);
        assert!(early > 1e-4, "no reverb output");
        assert!(late < early * 1e-3, "early {early} late {late}");
        // Larger size decays more slowly.
        let mut sp2 = space();
        s.reverb_size = 1.0;
        let out2 = run(&mut sp2, &impulse(len), &s, &transport(120.0));
        assert!(rms(&out2[len - 4800..]) > late * 10.0);
    }

    #[test]
    fn idles_after_tail_when_mix_zero() {
        let mut sp = space();
        let mut s = snap();
        s.reverb_mix = 1.0;
        s.reverb_size = 0.0;
        run(&mut sp, &impulse(BLOCK), &s, &transport(120.0));
        s.reverb_mix = 0.0;
        run(&mut sp, &vec![0.0; 3 * SR as usize], &s, &transport(120.0));
        assert!(sp.reverb.quiet > sp.reverb.max_len);
    }

    #[test]
    fn nan_input_does_not_poison_state() {
        let mut sp = space();
        let mut s = snap();
        s.reverb_mix = 0.5;
        s.delay_mix = 0.5;
        s.delay_feedback = 0.9;
        let mut input = vec![0.1; SR as usize];
        input[100] = f32::NAN;
        input[200] = f32::INFINITY;
        let out = run(&mut sp, &input, &s, &transport(120.0));
        assert!(out.iter().all(|x| x.is_finite()));
        // NaN parameters too.
        s.reverb_size = f32::NAN;
        s.delay_feedback = f32::NAN;
        let out = run(&mut sp, &input, &s, &transport(f64::NAN));
        assert!(out.iter().all(|x| x.is_finite()));
    }
}
