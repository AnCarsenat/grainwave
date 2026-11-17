//! OWNER: feat/space-output. Output gain (smoothed) and safety limiter. Runs after dry/wet mix.
//!
//! Per sample: NaN/inf -> 0, smoothed gain (20 ms one-pole), optional peak limiter
//! (instant attack, 100 ms release, -0.3 dBFS ceiling, stereo-linked), final clamp to +/-1.
use crate::params::Snapshot;

const GAIN_SMOOTH_MS: f32 = 20.0;
const RELEASE_MS: f32 = 100.0;
/// -0.3 dBFS.
pub const CEILING: f32 = 0.966_050_9;
/// +12 dB, the top of the output gain range.
const MAX_GAIN: f32 = 3.981_072;

pub struct Output {
    gain: f32,
    gain_c: f32,
    /// Current limiter gain (1 = no reduction).
    lim_gain: f32,
    release_c: f32,
    fresh: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

impl Output {
    pub fn new() -> Self {
        let mut o = Self {
            gain: 1.0,
            gain_c: 0.0,
            lim_gain: 1.0,
            release_c: 0.0,
            fresh: true,
        };
        o.reset(48_000.0, 0);
        o
    }

    pub fn reset(&mut self, sample_rate: f32, _max_block: usize) {
        let sr = sample_rate.max(1000.0);
        self.gain_c = (-1.0 / (GAIN_SMOOTH_MS * 0.001 * sr)).exp();
        self.release_c = (-1.0 / (RELEASE_MS * 0.001 * sr)).exp();
        self.gain = 1.0;
        self.lim_gain = 1.0;
        self.fresh = true;
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], snap: &Snapshot) {
        let target = if snap.output_gain.is_finite() {
            snap.output_gain.clamp(0.0, MAX_GAIN)
        } else {
            1.0
        };
        if self.fresh {
            self.gain = target;
            self.fresh = false;
        }
        let limit = snap.limiter;
        if !limit {
            self.lim_gain = 1.0;
        }

        for (sl, sr) in l.iter_mut().zip(r.iter_mut()) {
            let mut xl = if sl.is_finite() { *sl } else { 0.0 };
            let mut xr = if sr.is_finite() { *sr } else { 0.0 };

            self.gain = target + (self.gain - target) * self.gain_c;
            xl *= self.gain;
            xr *= self.gain;

            if limit {
                // Release toward unity, then clamp down instantly if this sample needs more.
                self.lim_gain = 1.0 + (self.lim_gain - 1.0) * self.release_c;
                let peak = xl.abs().max(xr.abs());
                if peak * self.lim_gain > CEILING {
                    self.lim_gain = CEILING / peak;
                }
                xl *= self.lim_gain;
                xr *= self.lim_gain;
            }

            *sl = xl.clamp(-1.0, 1.0);
            *sr = xr.clamp(-1.0, 1.0);
        }
        // Handle a mismatched right channel (contract says equal lengths; stay safe anyway).
        if r.len() > l.len() {
            for s in r[l.len()..].iter_mut() {
                *s = if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };
            }
        } else if l.len() > r.len() {
            for s in l[r.len()..].iter_mut() {
                *s = if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::rng::Rng;
    use crate::params::GrainwaveParams;

    const SR: f32 = 48_000.0;

    fn snap(gain: f32, limiter: bool) -> Snapshot {
        let mut s = Snapshot::from_params(&GrainwaveParams::default());
        s.output_gain = gain;
        s.limiter = limiter;
        s
    }

    fn out() -> Output {
        let mut o = Output::new();
        o.reset(SR, 512);
        o
    }

    #[test]
    fn ceiling_is_minus_0_3_db() {
        assert!((20.0 * CEILING.log10() + 0.3).abs() < 1e-4);
    }

    #[test]
    fn unity_is_transparent_below_ceiling() {
        let mut o = out();
        let mut rng = Rng::new(5);
        let input: Vec<f32> = (0..4096).map(|_| rng.bipolar() * 0.5).collect();
        let mut l = input.clone();
        let mut r = input.clone();
        o.process(&mut l, &mut r, &snap(1.0, true));
        assert_eq!(l, input);
        assert_eq!(r, input);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut o = out();
        let mut l = vec![0.0; 1024];
        let mut r = vec![0.0; 1024];
        o.process(&mut l, &mut r, &snap(3.0, true));
        assert!(l.iter().chain(r.iter()).all(|&x| x == 0.0));
    }

    #[test]
    fn limiter_keeps_peaks_below_ceiling() {
        let mut o = out();
        let mut rng = Rng::new(9);
        let s = snap(MAX_GAIN, true);
        for _ in 0..200 {
            let mut l: Vec<f32> = (0..512).map(|_| rng.bipolar() * 4.0).collect();
            let mut r: Vec<f32> = (0..512).map(|_| rng.bipolar() * 4.0).collect();
            o.process(&mut l, &mut r, &s);
            for &x in l.iter().chain(r.iter()) {
                assert!(x.abs() <= CEILING + 1e-6, "{x}");
            }
        }
    }

    #[test]
    fn limiter_releases() {
        let mut o = out();
        let s = snap(1.0, true);
        let mut l = vec![4.0; 64];
        let mut r = vec![4.0; 64];
        o.process(&mut l, &mut r, &s);
        assert!(o.lim_gain < 0.3);
        // 1 s of quiet signal: gain back near unity (10 time constants).
        for _ in 0..(SR as usize / 512) {
            let mut l = vec![0.1; 512];
            let mut r = vec![0.1; 512];
            o.process(&mut l, &mut r, &s);
        }
        assert!(o.lim_gain > 0.999);
    }

    #[test]
    fn without_limiter_clamps_to_unity() {
        let mut o = out();
        let mut l = vec![2.0, -3.0, 0.5];
        let mut r = vec![-2.0, 3.0, 0.98];
        o.process(&mut l, &mut r, &snap(1.0, false));
        assert_eq!(l, vec![1.0, -1.0, 0.5]);
        assert_eq!(r, vec![-1.0, 1.0, 0.98]);
    }

    #[test]
    fn nan_and_inf_become_zero() {
        for limiter in [true, false] {
            let mut o = out();
            let mut l = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25];
            let mut r = vec![0.25, f32::NAN, 0.0, f32::INFINITY];
            o.process(&mut l, &mut r, &snap(1.0, limiter));
            assert_eq!(l, vec![0.0, 0.0, 0.0, 0.25]);
            assert_eq!(r, vec![0.25, 0.0, 0.0, 0.0]);
        }
        // NaN gain parameter falls back to unity.
        let mut o = out();
        let mut l = vec![0.5; 8];
        let mut r = vec![0.5; 8];
        o.process(&mut l, &mut r, &snap(f32::NAN, true));
        assert!(l.iter().all(|&x| x == 0.5));
    }

    #[test]
    fn gain_change_is_smoothed() {
        let mut o = out();
        let mut l = vec![0.1; 512];
        let mut r = vec![0.1; 512];
        o.process(&mut l, &mut r, &snap(1.0, false));
        let mut l = vec![0.1; 4800];
        let mut r = vec![0.1; 4800];
        o.process(&mut l, &mut r, &snap(2.0, false));
        // First sample barely moves; after 100 ms (5 time constants) close to target.
        assert!(l[0] < 0.101);
        assert!((l[4799] - 0.2).abs() < 0.002);
        assert!(l.windows(2).all(|w| w[1] >= w[0]));
    }
}
