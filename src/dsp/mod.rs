//! DSP modules. Each file is owned by ONE feature branch (see README "Branch ownership").
//!
//! Contract shared by every processor:
//! - `new()` and `reset()` may allocate. `process*()` must NEVER allocate, lock or do I/O
//!   (debug builds abort on allocation via nih-plug's `assert_process_allocs`).
//! - `process*()` works in place on one block of stereo audio: `l.len() == r.len() <= max_block`.
//! - Read parameter values only from `&Snapshot`, never from `GrainwaveParams`.
//! - Randomness: use `rng::Rng` (xorshift, allocation-free), seeded in `new()`.

pub mod degrade;
pub mod filter;
pub mod grains;
pub mod input;
pub mod modulation;
pub mod output;
pub mod rhythm;
pub mod rng;
pub mod space;
pub mod timeshape;

/// Host timing for the current block.
#[derive(Debug, Clone, Copy)]
pub struct Transport {
    pub sample_rate: f32,
    /// Host tempo, or 120 when the host gives none.
    pub bpm: f64,
    /// Position in quarter-note beats at the first sample of this block. When the host
    /// gives none, the plugin keeps its own free-running counter.
    pub beat_pos: f64,
    pub playing: bool,
    pub block_len: usize,
}

impl Transport {
    pub fn samples_per_beat(&self) -> f64 {
        self.sample_rate as f64 * 60.0 / self.bpm
    }
}

/// Size `buf` to `len` zeros. Reuses the allocation when the length is unchanged, so
/// `reset()` stays allocation-free after `initialize()` (nih-plug runs `Plugin::reset` on
/// the audio thread under `assert_process_allocs`).
pub fn realloc_or_clear(buf: &mut Vec<f32>, len: usize) {
    if buf.len() == len {
        buf.fill(0.0);
    } else {
        *buf = vec![0.0; len];
    }
}
