//! OWNER: feat/space-output. Small reverb, tempo-synced delay with crusher in feedback path.
use super::Transport;
use crate::params::Snapshot;

pub struct Space {}

impl Space {
    pub fn new() -> Self {
        Self {}
    }

    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {}

    /// Stub: pass-through.
    pub fn process(&mut self, _l: &mut [f32], _r: &mut [f32], _snap: &Snapshot, _t: &Transport) {}
}
