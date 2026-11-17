//! Lock-free state shared between the audio thread (writer) and the editor (reader).
//! f32 values are stored as bits in `AtomicU32`. Use `Relaxed` ordering; values are cosmetic.

use std::sync::atomic::{AtomicU32, Ordering};

pub const MAX_VISUAL_GRAINS: usize = 32;
pub const WAVEFORM_POINTS: usize = 256;

pub struct VisualState {
    /// Read position of each grain voice, 0..1 across the capture buffer.
    pub grain_pos: [AtomicU32; MAX_VISUAL_GRAINS],
    /// Bit i set = voice i active.
    pub grain_active: AtomicU32,
    /// Bit i set = voice i playing in reverse.
    pub grain_reverse: AtomicU32,
    /// Write head, 0..1 across the capture buffer.
    pub write_head: AtomicU32,
    /// Peak magnitude per slice of the capture buffer, for the waveform display.
    pub waveform: [AtomicU32; WAVEFORM_POINTS],
    /// Current modulation output per LFO (-1..1) and envelope follower (0..1).
    pub lfo_values: [AtomicU32; 2],
    pub env_value: AtomicU32,
    /// Current step of the 16-step gate (0..15). Plain integer, NOT f32 bits:
    /// read with `.load(Ordering::Relaxed)`, never `load_f32`.
    pub gate_step: AtomicU32,
}

impl Default for VisualState {
    fn default() -> Self {
        Self {
            grain_pos: std::array::from_fn(|_| AtomicU32::new(0)),
            grain_active: AtomicU32::new(0),
            grain_reverse: AtomicU32::new(0),
            write_head: AtomicU32::new(0),
            waveform: std::array::from_fn(|_| AtomicU32::new(0)),
            lfo_values: std::array::from_fn(|_| AtomicU32::new(0)),
            env_value: AtomicU32::new(0),
            gate_step: AtomicU32::new(0),
        }
    }
}

#[inline]
pub fn store_f32(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

#[inline]
pub fn load_f32(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}
