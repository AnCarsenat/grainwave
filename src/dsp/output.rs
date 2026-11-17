//! OWNER: feat/space-output. Output gain (smoothed) and safety limiter. Runs after dry/wet mix.
use crate::params::Snapshot;

pub struct Output {}

impl Output {
    pub fn new() -> Self {
        Self {}
    }

    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {}

    /// Stub: hard clamp so the stub is never dangerous.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], _snap: &Snapshot) {
        for s in l.iter_mut().chain(r.iter_mut()) {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}
