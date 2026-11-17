//! Wow and flutter: a 5 ms modulated delay line read with 4-point cubic (Catmull-Rom)
//! interpolation. Wow = 0.6 × sine + 0.4 × smoothed random (cosine-interpolated, one new
//! value per wow cycle), up to ±3 ms. Flutter = sine, up to ±0.3 ms. Both channels share the
//! same modulation, like a single tape transport.
//!
//! With both depths at 0 the stage is bypassed (no 5 ms latency). Turning it on or off
//! crossfades between the dry and delayed signal over 20 ms; the line is written even while
//! bypassed so its history is always valid.
use super::util::{clampf, Fade, Smoother};
use crate::dsp::rng::Rng;
use std::f32::consts::{PI, TAU};

const BASE_MS: f32 = 5.0;
const WOW_MS: f32 = 3.0;
const FLUTTER_MS: f32 = 0.3;
const FADE_S: f32 = 0.02;
const DEPTH_SMOOTH_S: f32 = 0.05;

pub(crate) struct WowFlutter {
    sample_rate: f32,
    buf: [Vec<f32>; 2],
    mask: usize,
    write: usize,
    wow_phase: f32,
    flutter_phase: f32,
    rnd_prev: f32,
    rnd_next: f32,
    wow_depth: Smoother,
    flutter_depth: Smoother,
    fade: Fade,
    rng: Rng,
}

#[inline]
fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

impl WowFlutter {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            buf: [Vec::new(), Vec::new()],
            mask: 0,
            write: 0,
            wow_phase: 0.0,
            flutter_phase: 0.0,
            rnd_prev: 0.0,
            rnd_next: 0.0,
            wow_depth: Smoother::new(0.0),
            flutter_depth: Smoother::new(0.0),
            fade: Fade::new(),
            rng: Rng::new(0x0057_0F1A),
        }
    }

    /// Allocates the delay line for the worst case at `sample_rate`.
    pub fn reset(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let max_ms = BASE_MS + WOW_MS + FLUTTER_MS;
        let len = ((max_ms * 0.001 * sample_rate).ceil() as usize + 8).next_power_of_two();
        for b in self.buf.iter_mut() {
            b.clear();
            b.resize(len, 0.0);
        }
        self.mask = len - 1;
        self.write = 0;
        self.wow_phase = 0.0;
        self.flutter_phase = 0.0;
        self.rng = Rng::new(0x0057_0F1A);
        self.rnd_prev = 0.0;
        self.rnd_next = self.rng.bipolar();
        self.wow_depth.set_time(DEPTH_SMOOTH_S, sample_rate);
        self.wow_depth.reset(0.0);
        self.flutter_depth.set_time(DEPTH_SMOOTH_S, sample_rate);
        self.flutter_depth.reset(0.0);
        self.fade.set_time(FADE_S, sample_rate);
        self.fade.reset();
    }

    #[inline]
    fn read(&self, ch: usize, delay: f32) -> f32 {
        let b = &self.buf[ch];
        let whole = delay.floor();
        let t = delay - whole;
        // Reading `delay` samples behind the newest sample (at `write`).
        let i1 = self.write.wrapping_sub(whole as usize);
        let s = |o: usize| b[o & self.mask];
        // Older samples are further back; t moves from i1 toward the older sample i1-1.
        let y0 = s(i1.wrapping_add(1));
        let y1 = s(i1);
        let y2 = s(i1.wrapping_sub(1));
        let y3 = s(i1.wrapping_sub(2));
        catmull_rom(y0, y1, y2, y3, t)
    }

    pub fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        wow_rate: f32,
        wow_depth: f32,
        flutter_rate: f32,
        flutter_depth: f32,
    ) {
        if self.mask == 0 {
            return; // not reset yet
        }
        let sr = self.sample_rate;
        let wow_inc = clampf(wow_rate, 0.05, 4.0) / sr;
        let flutter_inc = clampf(flutter_rate, 4.0, 30.0) / sr;
        let wd = clampf(wow_depth, 0.0, 1.0);
        let fd = clampf(flutter_depth, 0.0, 1.0);
        self.fade.set_target(wd > 0.0 || fd > 0.0);
        let ms_to_samples = 0.001 * sr;
        let max_delay = (self.mask - 3) as f32;

        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.write = (self.write + 1) & self.mask;
            self.buf[0][self.write] = *xl;
            self.buf[1][self.write] = *xr;
            if self.fade.idle() {
                continue;
            }

            self.wow_phase += wow_inc;
            if self.wow_phase >= 1.0 {
                self.wow_phase -= 1.0;
                self.rnd_prev = self.rnd_next;
                self.rnd_next = self.rng.bipolar();
            }
            self.flutter_phase += flutter_inc;
            if self.flutter_phase >= 1.0 {
                self.flutter_phase -= 1.0;
            }
            let ct = 0.5 - 0.5 * (PI * self.wow_phase).cos();
            let rnd = self.rnd_prev + (self.rnd_next - self.rnd_prev) * ct;
            let wow = 0.6 * (TAU * self.wow_phase).sin() + 0.4 * rnd;
            let flutter = (TAU * self.flutter_phase).sin();

            let ms = BASE_MS
                + WOW_MS * self.wow_depth.next(wd) * wow
                + FLUTTER_MS * self.flutter_depth.next(fd) * flutter;
            let delay = (ms * ms_to_samples).clamp(1.0, max_delay);

            let m = self.fade.next();
            let yl = self.read(0, delay);
            let yr = self.read(1, delay);
            *xl += (yl - *xl) * m;
            *xr += (yr - *xr) * m;
        }
    }
}
