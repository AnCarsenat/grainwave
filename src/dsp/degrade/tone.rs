//! Tape age (2-pole low-pass, 20 kHz -> 2.5 kHz, exponential in `tape_age`) and head bump
//! (RBJ peaking EQ at 90 Hz, Q 1, `head_bump_db` gain).
use super::util::{clampf, flush, Fade, Smoother};
use crate::dsp::filter::{Svf, SvfCoefs};

const AGE_SMOOTH_S: f32 = 0.03;
const BUMP_SMOOTH_S: f32 = 0.03;
const FADE_S: f32 = 0.02;
const BUMP_HZ: f32 = 90.0;
const BUMP_Q: f32 = 1.0;
const BUTTERWORTH_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Biquad in transposed direct form II.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    s1: f32,
    s2: f32,
}

#[derive(Debug, Clone, Copy)]
struct BiquadCoefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoefs {
    const IDENTITY: Self = Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// RBJ cookbook peaking EQ.
    fn peaking(freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cs) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cs) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

impl Biquad {
    #[inline]
    fn process(&mut self, x: f32, c: &BiquadCoefs) -> f32 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }
}

pub(crate) struct Tone {
    sample_rate: f32,
    age_lp: [Svf; 2],
    age: Smoother,
    age_fade: Fade,
    bump: [Biquad; 2],
    bump_db: Smoother,
    bump_coefs: BiquadCoefs,
    bump_coefs_db: f32,
}

impl Tone {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            age_lp: [Svf::default(); 2],
            age: Smoother::new(0.0),
            age_fade: Fade::new(),
            bump: [Biquad::default(); 2],
            bump_db: Smoother::new(0.0),
            bump_coefs: BiquadCoefs::IDENTITY,
            bump_coefs_db: 0.0,
        }
    }

    pub fn reset(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.age_lp = [Svf::default(); 2];
        self.age.set_time(AGE_SMOOTH_S, sample_rate);
        self.age.reset(0.0);
        self.age_fade.set_time(FADE_S, sample_rate);
        self.age_fade.reset();
        self.bump = [Biquad::default(); 2];
        self.bump_db.set_time(BUMP_SMOOTH_S, sample_rate);
        self.bump_db.reset(0.0);
        self.bump_coefs = BiquadCoefs::IDENTITY;
        self.bump_coefs_db = 0.0;
    }

    /// Jump straight to these settings (first block after a reset): no sweep, no fade-in.
    pub fn prime(&mut self, tape_age: f32, head_bump_db: f32) {
        let age_t = clampf(tape_age, 0.0, 1.0);
        self.age.reset(age_t);
        self.age_fade.set_target(age_t > 0.0);
        self.age_fade.snap();
        self.bump_db.reset(clampf(head_bump_db, 0.0, 9.0));
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], tape_age: f32, head_bump_db: f32) {
        let sr = self.sample_rate;
        let age_t = clampf(tape_age, 0.0, 1.0);
        let bump_t = clampf(head_bump_db, 0.0, 9.0);
        self.age_fade.set_target(age_t > 0.0);
        let max_hz = 0.45 * sr;

        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            let age = self.age.next(age_t);
            if self.age_fade.active() {
                // 20 kHz * (2.5k / 20k)^age
                let fc = (20_000.0 * 0.125f32.powf(age)).min(max_hz);
                let c = SvfCoefs::new(fc, BUTTERWORTH_Q, sr);
                let m = self.age_fade.next();
                let yl = self.age_lp[0].lowpass(*xl, &c);
                let yr = self.age_lp[1].lowpass(*xr, &c);
                *xl += (yl - *xl) * m;
                *xr += (yr - *xr) * m;
                if self.age_fade.idle() {
                    self.age_lp = [Svf::default(); 2];
                }
            }

            // A peaking EQ at 0 dB is an exact identity (its states stay at 0), so it can be
            // skipped without any fade.
            let db = self.bump_db.next(bump_t);
            if db > 0.0 {
                if (db - self.bump_coefs_db).abs() > 0.005 {
                    self.bump_coefs = BiquadCoefs::peaking(BUMP_HZ, BUMP_Q, db, sr);
                    self.bump_coefs_db = db;
                }
                *xl = self.bump[0].process(*xl, &self.bump_coefs);
                *xr = self.bump[1].process(*xr, &self.bump_coefs);
            } else if self.bump_coefs_db != 0.0 {
                self.bump = [Biquad::default(); 2];
                self.bump_coefs_db = 0.0;
                self.bump_coefs = BiquadCoefs::IDENTITY;
            }
        }

        for f in self.age_lp.iter_mut() {
            f.flush_denormals();
        }
        for b in self.bump.iter_mut() {
            b.s1 = flush(b.s1);
            b.s2 = flush(b.s2);
        }
    }
}
