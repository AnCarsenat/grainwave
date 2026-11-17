//! OWNER: feat/grain-engine. Channel mode encode/decode around the wet chain.
use crate::params::{ChannelMode, Snapshot};

/// Called before the wet chain. Stub: no-op (stereo).
pub fn encode(_l: &mut [f32], _r: &mut [f32], _snap: &Snapshot) {
    let _ = ChannelMode::Stereo;
}

/// Called after the wet chain. Stub: no-op (stereo).
pub fn decode(_l: &mut [f32], _r: &mut [f32], _snap: &Snapshot) {}
