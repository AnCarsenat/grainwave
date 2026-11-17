//! Polyphase halfband 2× up/downsamplers, cascaded for 4×.
//!
//! The halfband FIR has 47 taps (Kaiser window, beta 7, about -70 dB stopband). Every other tap
//! is zero except the centre (0.5), so each polyphase branch is either a 24-tap FIR or a pure
//! delay. All state lives in fixed arrays: nothing allocates, ever.

/// Number of taps of the full halfband filter (must be 4k + 3 so the centre index is odd).
const TAPS: usize = 47;
const CENTER: usize = (TAPS - 1) / 2; // 23, odd
/// Non-zero even-index taps h[0], h[2], ..., h[46].
const EVEN: usize = TAPS.div_ceil(2); // 24
/// History length (power of two, >= EVEN and > the pure-delay branch length).
const HIST: usize = 32;
/// Pure-delay branch offsets (in samples at the lower rate).
const UP_DELAY: usize = (CENTER - 1) / 2; // 11
const DOWN_DELAY: usize = CENTER.div_ceil(2); // 12

/// Even-index taps of the halfband filter, normalised so the full filter has unity DC gain.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Halfband {
    even: [f32; EVEN],
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

impl Halfband {
    pub fn design() -> Self {
        const BETA: f64 = 7.0;
        let mut even = [0f32; EVEN];
        let mut tmp = [0f64; EVEN];
        let i0b = bessel_i0(BETA);
        let mut sum = 0.0;
        for (i, t) in tmp.iter_mut().enumerate() {
            let j = 2 * i;
            let m = j as f64 - CENTER as f64; // always odd
            let sinc = (std::f64::consts::FRAC_PI_2 * m).sin() / (std::f64::consts::PI * m);
            let r = m / CENTER as f64;
            let w = bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / i0b;
            *t = sinc * w;
            sum += *t;
        }
        // Even taps must sum to 0.5 (the centre tap gives the other 0.5).
        for (e, t) in even.iter_mut().zip(tmp.iter()) {
            *e = (*t * 0.5 / sum) as f32;
        }
        Self { even }
    }
}

/// Circular history written twice so a contiguous window can always be read.
#[derive(Debug, Clone, Copy)]
struct History {
    buf: [f32; 2 * HIST],
    pos: usize,
}

impl History {
    const fn new() -> Self {
        Self { buf: [0.0; 2 * HIST], pos: 0 }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = (self.pos + 1) & (HIST - 1);
        self.buf[self.pos] = x;
        self.buf[self.pos + HIST] = x;
    }

    /// Sample `k` steps in the past (0 = newest), `k < HIST`.
    #[inline]
    fn get(&self, k: usize) -> f32 {
        self.buf[self.pos + HIST - k]
    }

    /// FIR over the newest `EVEN` samples.
    #[inline]
    fn fir(&self, h: &[f32; EVEN]) -> f32 {
        let base = self.pos + HIST;
        let mut acc = 0.0;
        for (k, c) in h.iter().enumerate() {
            acc += c * self.buf[base - k];
        }
        acc
    }
}

/// 2× upsampler: one input sample in, two output samples out.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Up2 {
    hist: History,
}

impl Up2 {
    pub const fn new() -> Self {
        Self { hist: History::new() }
    }

    #[inline]
    pub fn process(&mut self, x: f32, hb: &Halfband) -> [f32; 2] {
        self.hist.push(x);
        // Zero-stuffing halves the level, so both branches carry a gain of 2.
        let y0 = 2.0 * self.hist.fir(&hb.even);
        let y1 = self.hist.get(UP_DELAY);
        [y0, y1]
    }
}

/// 2× downsampler: two input samples in, one output sample out.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Down2 {
    even: History,
    odd: History,
}

impl Down2 {
    pub const fn new() -> Self {
        Self { even: History::new(), odd: History::new() }
    }

    #[inline]
    pub fn process(&mut self, v: [f32; 2], hb: &Halfband) -> f32 {
        self.even.push(v[0]);
        self.odd.push(v[1]);
        self.even.fir(&hb.even) + 0.5 * self.odd.get(DOWN_DELAY)
    }
}

/// One channel of 1×/2×/4× oversampling around a per-sample nonlinearity.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Oversampler {
    up1: Up2,
    down1: Down2,
    up2: Up2,
    down2: Down2,
}

impl Oversampler {
    pub const fn new() -> Self {
        Self { up1: Up2::new(), down1: Down2::new(), up2: Up2::new(), down2: Down2::new() }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Run `f` at `factor` × the base rate (1, 2 or 4) and return one base-rate sample.
    #[inline]
    pub fn process(&mut self, x: f32, factor: usize, hb: &Halfband, mut f: impl FnMut(f32) -> f32) -> f32 {
        match factor {
            2 => {
                let u = self.up1.process(x, hb);
                self.down1.process([f(u[0]), f(u[1])], hb)
            }
            4 => {
                let u = self.up1.process(x, hb);
                let a = self.up2.process(u[0], hb);
                let b = self.up2.process(u[1], hb);
                let da = self.down2.process([f(a[0]), f(a[1])], hb);
                let db = self.down2.process([f(b[0]), f(b[1])], hb);
                self.down1.process([da, db], hb)
            }
            _ => f(x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_gain(factor: usize, freq: f32, sr: f32) -> f32 {
        let hb = Halfband::design();
        let mut os = Oversampler::new();
        let n = 8192;
        let mut sx = 0.0;
        let mut sy = 0.0;
        for i in 0..n {
            let x = (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin();
            let y = os.process(x, factor, &hb, |v| v);
            if i > 1024 {
                sx += x * x;
                sy += y * y;
            }
        }
        20.0 * (sy / sx).sqrt().log10()
    }

    #[test]
    fn halfband_dc_gain_is_unity() {
        let hb = Halfband::design();
        let s: f32 = hb.even.iter().sum();
        assert!((s - 0.5).abs() < 1e-6);
    }

    #[test]
    fn passband_is_flat() {
        for factor in [2, 4] {
            for freq in [100.0, 1000.0, 10_000.0, 16_000.0] {
                let g = sine_gain(factor, freq, 48_000.0);
                assert!(g.abs() < 0.2, "{factor}x at {freq} Hz: {g} dB");
            }
        }
    }

    #[test]
    fn identity_round_trip_is_delayed_copy() {
        // Linear-phase up/down with identity in between must reproduce the input, delayed.
        let hb = Halfband::design();
        let mut os = Oversampler::new();
        let mut out = Vec::new();
        let input: Vec<f32> = (0..400).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        for &x in &input {
            out.push(os.process(x, 2, &hb, |v| v));
        }
        // Total delay at the base rate is CENTER samples.
        for i in 100..400 {
            assert!((out[i] - input[i - CENTER]).abs() < 2e-3, "i={i}");
        }
    }
}
