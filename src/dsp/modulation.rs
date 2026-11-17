//! OWNER: feat/modulation. 2 LFOs, input envelope follower, macros (Dust, Chaos, Age).
//! Runs first each block and rewrites `Snapshot` values before any DSP module reads them.
use super::Transport;
use crate::params::Snapshot;
use crate::shared::VisualState;
use std::sync::Arc;

pub struct Modulation {
    visual: Arc<VisualState>,
}

impl Modulation {
    pub fn new(visual: Arc<VisualState>) -> Self {
        Self { visual }
    }

    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {}

    /// `l`/`r` = input audio of this block (for the envelope follower). Stub: no modulation.
    pub fn process_block(&mut self, _snap: &mut Snapshot, _l: &[f32], _r: &[f32], _t: &Transport) {
        let _ = &self.visual;
    }
}
