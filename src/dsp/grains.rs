//! OWNER: feat/grain-engine. Capture buffer, grain scheduler, voices, playback modes, MIDI pitch.
use super::Transport;
use crate::params::Snapshot;
use crate::shared::VisualState;
use std::sync::Arc;

pub struct GrainEngine {
    visual: Arc<VisualState>,
}

impl GrainEngine {
    pub fn new(visual: Arc<VisualState>) -> Self {
        Self { visual }
    }

    /// Allocate buffers for the largest possible capture (8 bars at 40 BPM or 8 s, whichever is larger).
    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {}

    pub fn note_on(&mut self, _note: u8, _velocity: f32) {}
    pub fn note_off(&mut self, _note: u8) {}

    /// In: input audio. Out: wet grain audio, in place. Stub: pass-through.
    pub fn process(&mut self, _l: &mut [f32], _r: &mut [f32], _snap: &Snapshot, _t: &Transport) {
        let _ = &self.visual;
    }
}
