//! OWNER: feat/degrade. Resonant low-pass + high-pass with slow random cutoff drift.
//!
//! Two TPT state-variable filters (Zavalishin / Simper form), 12 dB/oct each, in series:
//! low-pass then high-pass. Resonance 0..1 maps exponentially to Q 0.707..10. Cutoffs are
//! smoothed in log2 space, offset by a smoothed random walk (±1 octave × `filter_drift`,
//! about 0.3 Hz) and clamped to 20 Hz .. 0.45 × sample rate. A filter at its neutral setting
//! (low-pass at 20 kHz or high-pass at 20 Hz, no resonance, no drift) is crossfaded out so the
//! default chain is bit-transparent.
use super::degrade::util::{clampf, flush, sanitize, Fade, Smoother};
use super::rng::Rng;
use super::Transport;
use crate::params::Snapshot;

const Q_MIN: f32 = 0.707;
const Q_MAX: f32 = 10.0;
const MIN_HZ: f32 = 20.0;
const MAX_PARAM_HZ: f32 = 20_000.0;
const DRIFT_RATE_HZ: f32 = 0.3;
const SMOOTH_S: f32 = 0.02;
const FADE_S: f32 = 0.02;

/// Map resonance 0..1 to Q 0.707..10 (exponential, so the knob feels even).
#[inline]
pub(crate) fn res_to_q(res: f32) -> f32 {
    Q_MIN * (Q_MAX / Q_MIN).powf(clampf(res, 0.0, 1.0))
}

/// Coefficients of a TPT SVF.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SvfCoefs {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfCoefs {
    /// `cutoff_hz` must already be clamped below Nyquist.
    #[inline]
    pub fn new(cutoff_hz: f32, q: f32, sample_rate: f32) -> Self {
        let g = (std::f32::consts::PI * cutoff_hz / sample_rate).tan();
        let k = 1.0 / q.max(0.01);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        Self { k, a1, a2, a3 }
    }
}

/// One channel of a TPT state-variable filter.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Returns (low-pass, high-pass).
    #[inline]
    pub fn tick(&mut self, x: f32, c: &SvfCoefs) -> (f32, f32) {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, x - c.k * v1 - v2)
    }

    #[inline]
    pub fn lowpass(&mut self, x: f32, c: &SvfCoefs) -> f32 {
        self.tick(x, c).0
    }

    #[inline]
    pub fn highpass(&mut self, x: f32, c: &SvfCoefs) -> f32 {
        self.tick(x, c).1
    }

    pub fn flush_denormals(&mut self) {
        self.ic1 = flush(self.ic1);
        self.ic2 = flush(self.ic2);
    }
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Filter {
    sample_rate: f32,
    lp: [Svf; 2],
    hp: [Svf; 2],
    lp_fade: Fade,
    hp_fade: Fade,
    /// Cutoffs in log2(Hz).
    lp_log: Smoother,
    hp_log: Smoother,
    lp_res: Smoother,
    hp_res: Smoother,
    drift_amt: Smoother,
    /// Random walk: target, two cascaded one-pole stages (value in -1..1).
    drift_target: f32,
    drift_s1: f32,
    drift_s2: f32,
    drift_coef: f32,
    drift_timer: u32,
    drift_period: u32,
    rng: Rng,
}

impl Filter {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            lp: [Svf::default(); 2],
            hp: [Svf::default(); 2],
            lp_fade: Fade::new(),
            hp_fade: Fade::new(),
            lp_log: Smoother::new(MAX_PARAM_HZ.log2()),
            hp_log: Smoother::new(MIN_HZ.log2()),
            lp_res: Smoother::new(0.0),
            hp_res: Smoother::new(0.0),
            drift_amt: Smoother::new(0.0),
            drift_target: 0.0,
            drift_s1: 0.0,
            drift_s2: 0.0,
            drift_coef: 0.0,
            drift_timer: 0,
            drift_period: 1,
            rng: Rng::new(0x5EED_F117),
        }
    }

    pub fn reset(&mut self, sample_rate: f32, _max_block: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        self.sample_rate = sr;
        for f in self.lp.iter_mut().chain(self.hp.iter_mut()) {
            f.reset();
        }
        for f in [&mut self.lp_fade, &mut self.hp_fade] {
            f.set_time(FADE_S, sr);
            f.reset();
        }
        for s in [
            &mut self.lp_log,
            &mut self.hp_log,
            &mut self.lp_res,
            &mut self.hp_res,
            &mut self.drift_amt,
        ] {
            s.set_time(SMOOTH_S, sr);
        }
        self.lp_log.reset(MAX_PARAM_HZ.log2());
        self.hp_log.reset(MIN_HZ.log2());
        self.lp_res.reset(0.0);
        self.hp_res.reset(0.0);
        self.drift_amt.reset(0.0);
        self.drift_target = 0.0;
        self.drift_s1 = 0.0;
        self.drift_s2 = 0.0;
        // Two cascaded one-poles with corner at the drift rate.
        self.drift_coef = 1.0 - (-2.0 * std::f32::consts::PI * DRIFT_RATE_HZ / sr).exp();
        self.drift_period = (sr / DRIFT_RATE_HZ).max(1.0) as u32;
        self.drift_timer = 0;
        self.rng = Rng::new(0x5EED_F117);
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], snap: &Snapshot, _t: &Transport) {
        let sr = self.sample_rate;
        let max_hz = 0.45 * sr;
        let lp_hz = clampf(snap.lowpass_hz, MIN_HZ, MAX_PARAM_HZ);
        let hp_hz = clampf(snap.highpass_hz, MIN_HZ, MAX_PARAM_HZ);
        let lp_res = clampf(snap.lowpass_res, 0.0, 1.0);
        let hp_res = clampf(snap.highpass_res, 0.0, 1.0);
        let drift = clampf(snap.filter_drift, 0.0, 1.0);

        // Neutral filters are crossfaded out entirely.
        self.lp_fade
            .set_target(lp_hz < MAX_PARAM_HZ - 0.5 || lp_res > 0.0 || drift > 0.0);
        self.hp_fade
            .set_target(hp_hz > MIN_HZ + 0.01 || hp_res > 0.0 || drift > 0.0);

        let lp_target = lp_hz.log2();
        let hp_target = hp_hz.log2();
        let log_min = MIN_HZ.log2();
        let log_max = max_hz.max(MIN_HZ).log2();

        let n = l.len().min(r.len());
        // A NaN/inf would latch forever in the SVF states.
        sanitize(&mut l[..n], &mut r[..n]);
        for i in 0..n {
            // Random walk: new step every ~1/0.3 s, smoothed by two one-poles.
            self.drift_timer += 1;
            if self.drift_timer >= self.drift_period {
                self.drift_timer = 0;
                let t = self.drift_target + self.rng.bipolar() * 0.7;
                // Reflect at the bounds so the walk keeps moving.
                self.drift_target = if t > 1.0 {
                    2.0 - t
                } else if t < -1.0 {
                    -2.0 - t
                } else {
                    t
                };
            }
            self.drift_s1 += self.drift_coef * (self.drift_target - self.drift_s1);
            self.drift_s2 += self.drift_coef * (self.drift_s1 - self.drift_s2);
            let offset = self.drift_s2 * self.drift_amt.next(drift);

            let lp_log = self.lp_log.next(lp_target);
            let hp_log = self.hp_log.next(hp_target);
            let lq = res_to_q(self.lp_res.next(lp_res));
            let hq = res_to_q(self.hp_res.next(hp_res));

            let (mut xl, mut xr) = (l[i], r[i]);

            if self.lp_fade.active() {
                let f = (lp_log + offset).clamp(log_min, log_max).exp2();
                let c = SvfCoefs::new(f, lq, sr);
                let m = self.lp_fade.next();
                let yl = self.lp[0].lowpass(xl, &c);
                let yr = self.lp[1].lowpass(xr, &c);
                xl += (yl - xl) * m;
                xr += (yr - xr) * m;
                if self.lp_fade.idle() {
                    self.lp[0].reset();
                    self.lp[1].reset();
                }
            }

            if self.hp_fade.active() {
                let f = (hp_log + offset).clamp(log_min, log_max).exp2();
                let c = SvfCoefs::new(f, hq, sr);
                let m = self.hp_fade.next();
                let yl = self.hp[0].highpass(xl, &c);
                let yr = self.hp[1].highpass(xr, &c);
                xl += (yl - xl) * m;
                xr += (yr - xr) * m;
                if self.hp_fade.idle() {
                    self.hp[0].reset();
                    self.hp[1].reset();
                }
            }

            l[i] = xl;
            r[i] = xr;
        }

        for f in self.lp.iter_mut().chain(self.hp.iter_mut()) {
            f.flush_denormals();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::GrainwaveParams;

    const SR: f32 = 48_000.0;

    fn snap() -> Snapshot {
        Snapshot::from_params(&GrainwaveParams::default())
    }

    fn transport(n: usize) -> Transport {
        Transport { sample_rate: SR, bpm: 120.0, beat_pos: 0.0, playing: true, block_len: n }
    }

    fn sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Run `input` through a fresh filter in blocks of `block`.
    fn run(s: &Snapshot, input: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let mut f = Filter::new();
        f.reset(SR, 512);
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            let n = cl.len();
            f.process(cl, cr, s, &transport(n));
        }
        (l, r)
    }

    /// Gain in dB of a sine at `freq`, measured after the filter settles.
    fn gain_db(s: &Snapshot, freq: f32) -> f32 {
        let n = 48_000;
        let x = sine(freq, n);
        let (y, _) = run(s, &x, 256);
        20.0 * (rms(&y[n / 2..]) / rms(&x[n / 2..])).log10()
    }

    #[test]
    fn neutral_is_transparent() {
        let mut rng = Rng::new(3);
        let x: Vec<f32> = (0..10_000).map(|_| rng.bipolar()).collect();
        let (l, r) = run(&snap(), &x, 128);
        assert_eq!(l, x);
        assert_eq!(r, x);
    }

    #[test]
    fn lowpass_attenuates_above_cutoff() {
        let mut s = snap();
        s.lowpass_hz = 1000.0;
        assert!(gain_db(&s, 100.0).abs() < 0.5);
        // 3 octaves above a 12 dB/oct cutoff: about -36 dB.
        assert!(gain_db(&s, 8000.0) < -30.0);
    }

    #[test]
    fn highpass_attenuates_below_cutoff() {
        let mut s = snap();
        s.highpass_hz = 2000.0;
        assert!(gain_db(&s, 10_000.0).abs() < 0.5);
        assert!(gain_db(&s, 250.0) < -30.0);
    }

    #[test]
    fn resonance_peaks_at_cutoff() {
        let mut s = snap();
        s.lowpass_hz = 1000.0;
        s.lowpass_res = 1.0;
        // Q = 10 -> +20 dB at the cutoff.
        let g = gain_db(&s, 1000.0);
        assert!((g - 20.0).abs() < 1.0, "peak {g} dB");
    }

    #[test]
    fn stable_at_max_resonance() {
        let mut f = Filter::new();
        f.reset(SR, 512);
        let mut rng = Rng::new(9);
        let mut s = snap();
        s.lowpass_res = 1.0;
        s.highpass_res = 1.0;
        s.filter_drift = 1.0;
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        for block in 0..2000 {
            // Sweep cutoffs hard every block, including the extremes.
            s.lowpass_hz = 20.0 * 1000f32.powf(rng.next_f32());
            s.highpass_hz = 20.0 * 1000f32.powf(rng.next_f32());
            if block % 7 == 0 {
                s.lowpass_hz = 20_000.0;
                s.highpass_hz = 20.0;
            }
            for i in 0..512 {
                l[i] = rng.bipolar();
                r[i] = if i % 64 == 0 { 1.0 } else { 0.0 };
            }
            f.process(&mut l, &mut r, &s, &transport(512));
            for v in l.iter().chain(r.iter()) {
                assert!(v.is_finite() && v.abs() < 100.0, "unstable: {v}");
            }
        }
    }

    #[test]
    fn finite_with_extreme_params() {
        let mut f = Filter::new();
        f.reset(44_100.0, 512);
        let mut rng = Rng::new(77);
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        let wild = [f32::NAN, f32::INFINITY, -1e9, -1.0, 0.0, 0.5, 1.0, 2.0, 1e9];
        for _ in 0..1000 {
            let mut s = snap();
            let mut pick = || wild[(rng.next_u32() % wild.len() as u32) as usize];
            s.lowpass_hz = pick() * 20_000.0;
            s.highpass_hz = pick() * 20_000.0;
            s.lowpass_res = pick();
            s.highpass_res = pick();
            s.filter_drift = pick();
            let n = 1 + (rng.next_u32() % 512) as usize;
            for i in 0..n {
                l[i] = rng.bipolar() * 4.0;
                r[i] = rng.bipolar() * 4.0;
            }
            f.process(&mut l[..n], &mut r[..n], &s, &transport(n));
            assert!(l[..n].iter().chain(r[..n].iter()).all(|v| v.is_finite()));
        }
    }

    #[test]
    fn non_finite_input_does_not_latch() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut s = snap();
            s.lowpass_hz = 2000.0;
            s.highpass_hz = 100.0;
            s.lowpass_res = 0.5;
            let mut x = sine(440.0, 48_000);
            x[1000] = bad;
            let (l, r) = run(&s, &x, 256);
            assert!(
                l[1000..].iter().chain(r[1000..].iter()).all(|v| v.is_finite()),
                "{bad} latched in the filter state"
            );
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut s = snap();
        s.lowpass_hz = 800.0;
        s.highpass_hz = 200.0;
        s.lowpass_res = 1.0;
        s.highpass_res = 1.0;
        s.filter_drift = 1.0;
        let (l, r) = run(&s, &vec![0.0; 48_000], 64);
        assert!(l.iter().chain(r.iter()).all(|&v| v == 0.0));
    }

    #[test]
    fn drift_stays_within_one_octave() {
        let mut f = Filter::new();
        f.reset(SR, 512);
        let mut s = snap();
        s.filter_drift = 1.0;
        s.lowpass_hz = 1000.0;
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        let (mut lo, mut hi) = (0f32, 0f32);
        for _ in 0..(SR as usize * 60 / 512) {
            f.process(&mut l, &mut r, &s, &transport(512));
            lo = lo.min(f.drift_s2);
            hi = hi.max(f.drift_s2);
        }
        assert!(lo >= -1.0 && hi <= 1.0);
        // It actually moves over a minute.
        assert!(hi - lo > 0.2, "drift range {lo}..{hi}");
    }

    #[test]
    fn block_size_independent() {
        let mut s = snap();
        s.lowpass_hz = 3000.0;
        s.highpass_hz = 150.0;
        s.lowpass_res = 0.6;
        s.filter_drift = 0.8;
        let mut rng = Rng::new(5);
        let x: Vec<f32> = (0..4096).map(|_| rng.bipolar()).collect();
        let a = run(&s, &x, 1);
        let b = run(&s, &x, 64);
        let c = run(&s, &x, 512);
        assert_eq!(a, b);
        assert_eq!(a, c);
    }
}
