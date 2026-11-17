//! OWNER: feat/rhythm. Stutter, swing, 16-step gate, rewind and tape-stop performance buttons.
use super::Transport;
use crate::params::Snapshot;

pub struct Rhythm {}

impl Rhythm {
    pub fn new() -> Self {
        Self {}
    }

    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {}

    /// Stub: pass-through.
    pub fn process(&mut self, _l: &mut [f32], _r: &mut [f32], _snap: &Snapshot, _t: &Transport) {}
}
