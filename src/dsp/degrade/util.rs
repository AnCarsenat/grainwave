//! Small real-time helpers shared by the degrade chain and the filter.

/// Clamp that also maps NaN/inf to `lo` (modulated values can arrive broken).
#[inline]
pub(crate) fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        lo
    }
}

#[inline]
pub(crate) fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Flush values too small to matter (denormal protection for filter states). Non-finite
/// values are flushed too, so a state that somehow blew up recovers instead of latching.
#[inline]
pub(crate) fn flush(v: f32) -> f32 {
    if v.abs() < 1e-15 || !v.is_finite() {
        0.0
    } else {
        v
    }
}

/// Replace NaN/inf input samples with 0 so they never enter a recursive filter state.
#[inline]
pub(crate) fn sanitize(l: &mut [f32], r: &mut [f32]) {
    for x in l.iter_mut().chain(r.iter_mut()) {
        if !x.is_finite() {
            *x = 0.0;
        }
    }
}

/// One-pole parameter smoother. Snaps to the target once it is within `1e-6`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Smoother {
    value: f32,
    coef: f32,
}

impl Smoother {
    pub fn new(value: f32) -> Self {
        Self { value, coef: 1.0 }
    }

    /// Set the time constant in seconds at `sample_rate`.
    pub fn set_time(&mut self, seconds: f32, sample_rate: f32) {
        self.coef = 1.0 - (-1.0 / (seconds * sample_rate).max(1.0)).exp();
    }

    pub fn reset(&mut self, value: f32) {
        self.value = value;
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.value
    }

    #[inline]
    pub fn next(&mut self, target: f32) -> f32 {
        let d = target - self.value;
        if d.abs() < 1e-6 {
            self.value = target;
        } else {
            self.value += self.coef * d;
        }
        self.value
    }
}

/// Linear 0..1 crossfade used to switch a stage in and out of bypass without clicks.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fade {
    value: f32,
    step: f32,
    on: bool,
}

impl Fade {
    pub fn new() -> Self {
        Self { value: 0.0, step: 1.0, on: false }
    }

    pub fn set_time(&mut self, seconds: f32, sample_rate: f32) {
        self.step = 1.0 / (seconds * sample_rate).max(1.0);
    }

    pub fn reset(&mut self) {
        self.value = 0.0;
        self.on = false;
    }

    #[inline]
    pub fn set_target(&mut self, on: bool) {
        self.on = on;
    }

    /// Jump straight to the current target (used on the first block after a reset).
    pub fn snap(&mut self) {
        self.value = if self.on { 1.0 } else { 0.0 };
    }

    /// True while the stage must be computed (on, or still fading out).
    #[inline]
    pub fn active(&self) -> bool {
        self.on || self.value > 0.0
    }

    /// Fully off: the stage can be skipped and its state cleared.
    #[inline]
    pub fn idle(&self) -> bool {
        !self.active()
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.on {
            self.value = (self.value + self.step).min(1.0);
        } else {
            self.value = (self.value - self.step).max(0.0);
        }
        self.value
    }
}
