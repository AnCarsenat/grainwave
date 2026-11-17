//! OWNER: feat/degrade. Bitcrush, sample-rate reduction, wow/flutter, saturation (oversampled),
//! tape age, head bump, hiss, crackle, dropouts. Order from `Snapshot::degrade_order`.
use super::Transport;
use crate::params::Snapshot;

pub struct Degrade {}

impl Degrade {
    pub fn new() -> Self {
        Self {}
    }

    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {}

    /// Stub: pass-through.
    pub fn process(&mut self, _l: &mut [f32], _r: &mut [f32], _snap: &Snapshot, _t: &Transport) {}
}
