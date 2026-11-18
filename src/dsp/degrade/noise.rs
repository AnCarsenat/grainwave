//! Noise stages (hiss, crackle) and dropouts. Each owns its own fixed-seed RNG so the output
//! does not depend on block size.
use super::util::{clampf, db_to_gain, Smoother};
use crate::dsp::rng::Rng;

const HISS_SMOOTH_S: f32 = 0.03;
/// Normalises the Kellet pink filter output to roughly unit RMS for uniform white input
/// in [-1, 1) (the filter's RMS is about 1.72 for that input).
const PINK_NORM: f32 = 0.58;

/// Paul Kellet's "economy" pink filter (3 poles). Pink-ish across common sample rates.
#[derive(Debug, Clone, Copy, Default)]
struct Pink {
    b0: f32,
    b1: f32,
    b2: f32,
}

impl Pink {
    #[inline]
    fn process(&mut self, white: f32) -> f32 {
        self.b0 = 0.99765 * self.b0 + white * 0.099_046;
        self.b1 = 0.963 * self.b1 + white * 0.296_516_4;
        self.b2 = 0.57 * self.b2 + white * 1.052_691_3;
        self.b0 + self.b1 + self.b2 + white * 0.1848
    }
}

/// Hiss: pink-ish noise, -90 dB (hiss -> 0+) to -30 dB (hiss = 1) RMS. Off at 0.
pub(crate) struct Hiss {
    pink: [Pink; 2],
    level: Smoother,
    rng: Rng,
}

impl Hiss {
    pub fn new() -> Self {
        Self { pink: [Pink::default(); 2], level: Smoother::new(0.0), rng: Rng::new(0x4155_5353) }
    }

    pub fn reset(&mut self, sample_rate: f32) {
        self.pink = [Pink::default(); 2];
        self.level.set_time(HISS_SMOOTH_S, sample_rate);
        self.level.reset(0.0);
        self.rng = Rng::new(0x4155_5353);
    }

    fn target(hiss: f32) -> f32 {
        let hiss = clampf(hiss, 0.0, 1.0);
        if hiss > 0.0 {
            db_to_gain(-90.0 + 60.0 * hiss) * PINK_NORM
        } else {
            0.0
        }
    }

    /// Jump straight to this level (first block after a reset).
    pub fn prime(&mut self, hiss: f32) {
        self.level.reset(Self::target(hiss));
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], hiss: f32) {
        let target = Self::target(hiss);
        if target == 0.0 && self.level.value() == 0.0 {
            return;
        }
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            let g = self.level.next(target);
            let nl = self.pink[0].process(self.rng.bipolar());
            let nr = self.pink[1].process(self.rng.bipolar());
            *xl += nl * g;
            *xr += nr * g;
        }
    }
}

/// Vinyl crackle: random clicks at up to 30/s × density, 0.5 ms decaying noise bursts with a
/// random amplitude × level; about 1 in 25 events is a larger, longer, duller pop.
pub(crate) struct Crackle {
    sample_rate: f32,
    env: f32,
    decay: f32,
    click_decay: f32,
    pop_decay: f32,
    /// One-pole low-pass coefficient for the current event (1 = unfiltered).
    lp_coef: f32,
    lp: [f32; 2],
    gain: [f32; 2],
    rng: Rng,
}

impl Crackle {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            env: 0.0,
            decay: 0.0,
            click_decay: 0.0,
            pop_decay: 0.0,
            lp_coef: 1.0,
            lp: [0.0; 2],
            gain: [1.0; 2],
            rng: Rng::new(0x00C2_AC1E),
        }
    }

    pub fn reset(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.env = 0.0;
        // Envelope falls by e^-4 over the click length.
        self.click_decay = (-4.0 / (0.0005 * sample_rate).max(1.0)).exp();
        self.pop_decay = (-4.0 / (0.003 * sample_rate).max(1.0)).exp();
        self.decay = self.click_decay;
        self.lp_coef = 1.0;
        self.lp = [0.0; 2];
        self.gain = [1.0; 2];
        self.rng = Rng::new(0x00C2_AC1E);
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], density: f32, level: f32) {
        let density = clampf(density, 0.0, 1.0);
        let level = clampf(level, 0.0, 1.0);
        if density == 0.0 && self.env == 0.0 {
            return;
        }
        let p = 30.0 * density / self.sample_rate;
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            if density > 0.0 && self.rng.chance(p) {
                let mut amp = level * (0.1 + 0.9 * self.rng.next_f32()) * 0.5;
                if self.rng.chance(0.04) {
                    amp = (amp * 2.5).min(1.0);
                    self.decay = self.pop_decay;
                    self.lp_coef = 0.25;
                } else {
                    self.decay = self.click_decay;
                    self.lp_coef = 1.0;
                }
                if amp > self.env {
                    self.env = amp;
                }
                self.gain = [0.7 + 0.3 * self.rng.bipolar(), 0.7 + 0.3 * self.rng.bipolar()];
            }
            if self.env == 0.0 {
                continue;
            }
            let n = self.rng.bipolar();
            for ch in 0..2 {
                self.lp[ch] += self.lp_coef * (n - self.lp[ch]);
            }
            *xl += self.lp[0] * self.env * self.gain[0];
            *xr += self.lp[1] * self.env * self.gain[1];
            self.env *= self.decay;
            if self.env < 1e-6 {
                self.env = 0.0;
                self.lp = [0.0; 2];
            }
        }
    }
}

/// Dropouts: every 50 ms, with probability `chance`, dip to `1 - depth` for 50-300 ms with
/// 20 ms linear ramps.
pub(crate) struct Dropouts {
    sample_rate: f32,
    tick_len: u32,
    tick: u32,
    remaining: u32,
    gain: f32,
    step: f32,
    rng: Rng,
}

impl Dropouts {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            tick_len: 2400,
            tick: 0,
            remaining: 0,
            gain: 1.0,
            step: 1.0 / 960.0,
            rng: Rng::new(0xD209_0075),
        }
    }

    pub fn reset(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.tick_len = (0.05 * sample_rate).max(1.0) as u32;
        self.tick = 0;
        self.remaining = 0;
        self.gain = 1.0;
        self.step = 1.0 / (0.02 * sample_rate).max(1.0);
        self.rng = Rng::new(0xD209_0075);
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], chance: f32, depth: f32) {
        let chance = clampf(chance, 0.0, 1.0);
        let floor = 1.0 - clampf(depth, 0.0, 1.0);
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.tick += 1;
            if self.tick >= self.tick_len {
                self.tick = 0;
                if self.remaining == 0 && chance > 0.0 && self.rng.chance(chance) {
                    let secs = 0.05 + 0.25 * self.rng.next_f32();
                    self.remaining = (secs * self.sample_rate) as u32;
                }
            }
            let target = if self.remaining > 0 {
                self.remaining -= 1;
                floor
            } else {
                1.0
            };
            if self.gain < target {
                self.gain = (self.gain + self.step).min(target);
            } else if self.gain > target {
                self.gain = (self.gain - self.step).max(target);
            }
            if self.gain != 1.0 {
                *xl *= self.gain;
                *xr *= self.gain;
            }
        }
    }
}
