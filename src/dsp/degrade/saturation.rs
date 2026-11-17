//! Saturation stage: drive gain `1 + 19 × drive`, tape / tube / hard-clip curve run at
//! 1×/2×/4× through the halfband oversampler, output compensated by `1 / sqrt(gain)`.
//! Tube output is DC-blocked. Drive 0 is a true bypass.
//!
//! Switching mode or oversampling factor (which changes the curve or the latency) fades the
//! stage out, swaps the configuration, then fades back in, so it never clicks.
use super::oversample::{Halfband, Oversampler};
use super::util::{clampf, flush, Fade, Smoother};
use crate::params::SaturationMode;

const FADE_S: f32 = 0.01;
const DRIVE_SMOOTH_S: f32 = 0.02;
const DC_HZ: f32 = 10.0;

/// One-pole DC blocker: `y = x - x1 + R y1`.
#[derive(Debug, Clone, Copy, Default)]
struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    #[inline]
    fn process(&mut self, x: f32, r: f32) -> f32 {
        let y = x - self.x1 + r * self.y1;
        self.x1 = x;
        self.y1 = flush(y);
        y
    }
}

#[inline]
fn shape(mode: SaturationMode, x: f32) -> f32 {
    match mode {
        // Asymmetric tanh: negative half saturates later (even harmonics, tape-like).
        SaturationMode::Tape => {
            if x >= 0.0 {
                x.tanh()
            } else {
                (0.8 * x).tanh() * 1.25
            }
        }
        // x + 0.2x² then soft clip. The input is first soft-limited to ±2.5 so the
        // quadratic stays monotonic (its turning point is at -2.5).
        SaturationMode::Tube => {
            let u = 2.5 * (x * 0.4).tanh();
            (u + 0.2 * u * u).tanh()
        }
        SaturationMode::HardClip => x.clamp(-1.0, 1.0),
    }
}

pub(crate) struct Saturation {
    hb: Halfband,
    os: [Oversampler; 2],
    dc: [DcBlocker; 2],
    dc_r: f32,
    drive: Smoother,
    fade: Fade,
    mode: SaturationMode,
    factor: usize,
}

impl Saturation {
    pub fn new() -> Self {
        Self {
            hb: Halfband::design(),
            os: [Oversampler::new(); 2],
            dc: [DcBlocker::default(); 2],
            dc_r: 0.999,
            drive: Smoother::new(0.0),
            fade: Fade::new(),
            mode: SaturationMode::Tape,
            factor: 1,
        }
    }

    pub fn reset(&mut self, sample_rate: f32) {
        self.clear_state();
        self.dc_r = 1.0 - 2.0 * std::f32::consts::PI * DC_HZ / sample_rate;
        self.drive.set_time(DRIVE_SMOOTH_S, sample_rate);
        self.drive.reset(0.0);
        self.fade.set_time(FADE_S, sample_rate);
        self.fade.reset();
    }

    fn clear_state(&mut self) {
        for os in self.os.iter_mut() {
            os.reset();
        }
        self.dc = [DcBlocker::default(); 2];
    }

    pub fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        drive: f32,
        mode: SaturationMode,
        factor: usize,
    ) {
        let drive = clampf(drive, 0.0, 1.0);
        let factor = match factor {
            2 | 4 => factor,
            _ => 1,
        };
        let want_on = drive > 0.0;

        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            let same_config = mode == self.mode && factor == self.factor;
            if self.fade.idle() && !same_config {
                self.mode = mode;
                self.factor = factor;
                self.clear_state();
            }
            self.fade.set_target(want_on && mode == self.mode && factor == self.factor);
            let d = self.drive.next(drive);
            if self.fade.idle() {
                continue;
            }

            let gain = 1.0 + 19.0 * d;
            let comp = 1.0 / gain.sqrt();
            let m = self.fade.next();
            let cur_mode = self.mode;
            let f = |v: f32| shape(cur_mode, v * gain) * comp;

            let mut yl = self.os[0].process(*xl, self.factor, &self.hb, f);
            let mut yr = self.os[1].process(*xr, self.factor, &self.hb, f);
            if cur_mode == SaturationMode::Tube {
                yl = self.dc[0].process(yl, self.dc_r);
                yr = self.dc[1].process(yr, self.dc_r);
            }
            *xl += (yl - *xl) * m;
            *xr += (yr - *xr) * m;

            if self.fade.idle() {
                self.clear_state();
            }
        }
    }
}
