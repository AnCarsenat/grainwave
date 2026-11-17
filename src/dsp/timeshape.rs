//! OWNER: feat/timeshape. Time shaper: per-cycle drawn curve sets how far behind live the
//! playback reads (see `pattern.rs` for curve semantics).
use super::Transport;
use crate::params::Snapshot;
use crate::pattern::PatternBank;
use crate::shared::VisualState;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, RwLock};

pub struct TimeShaper {
    #[allow(dead_code)]
    patterns: Arc<RwLock<PatternBank>>,
    #[allow(dead_code)]
    version: Arc<AtomicU32>,
    #[allow(dead_code)]
    visual: Arc<VisualState>,
}

impl TimeShaper {
    pub fn new(
        patterns: Arc<RwLock<PatternBank>>,
        version: Arc<AtomicU32>,
        visual: Arc<VisualState>,
    ) -> Self {
        Self { patterns, version, visual }
    }

    pub fn reset(&mut self, _sample_rate: f32, _max_block: usize) {}

    pub fn note_on(&mut self, _note: u8, _velocity: f32) {}

    /// Stub: pass-through.
    pub fn process(&mut self, _l: &mut [f32], _r: &mut [f32], _snap: &Snapshot, _t: &Transport) {}
}
