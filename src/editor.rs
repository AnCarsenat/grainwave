//! OWNER: feat/ui. egui editor: sections per parameter group, waveform with live grains,
//! preset browser, randomize with section locks.
use crate::params::GrainwaveParams;
use crate::shared::VisualState;
use nih_plug::prelude::*;
use std::sync::Arc;

/// Stub: no custom GUI (host shows generic sliders).
pub fn create(_params: Arc<GrainwaveParams>, _visual: Arc<VisualState>) -> Option<Box<dyn Editor>> {
    None
}
