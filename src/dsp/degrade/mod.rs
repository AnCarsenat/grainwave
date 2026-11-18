//! OWNER: feat/degrade. Bitcrush, sample-rate reduction, wow/flutter, saturation (oversampled),
//! tape age, head bump, hiss, crackle, dropouts. Order from `Snapshot::degrade_order`.
//!
//! Each stage processes the whole block in place, in the order given by `degrade_order`.
//! Every stage is a true bypass at its neutral setting, so the default chain is bit-transparent.
use super::Transport;
use crate::params::{DegradeOrder, Snapshot};

mod noise;
mod oversample;
mod saturation;
mod tone;
pub(crate) mod util;
mod wow;

use noise::{Crackle, Dropouts, Hiss};
use saturation::Saturation;
use tone::Tone;
use util::clampf;
use wow::WowFlutter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Crush,
    SampleRate,
    Wow,
    Saturation,
    /// Tape age + head bump.
    Tone,
    /// Hiss + crackle.
    Noise,
    Dropouts,
}

const DIGITAL_FIRST: [Stage; 7] = [
    Stage::Crush,
    Stage::SampleRate,
    Stage::Wow,
    Stage::Saturation,
    Stage::Tone,
    Stage::Noise,
    Stage::Dropouts,
];
const TAPE_FIRST: [Stage; 7] = [
    Stage::Wow,
    Stage::Saturation,
    Stage::Tone,
    Stage::Crush,
    Stage::SampleRate,
    Stage::Noise,
    Stage::Dropouts,
];
const DRIVE_FIRST: [Stage; 7] = [
    Stage::Saturation,
    Stage::Crush,
    Stage::SampleRate,
    Stage::Wow,
    Stage::Tone,
    Stage::Noise,
    Stage::Dropouts,
];

fn stages(order: DegradeOrder) -> &'static [Stage; 7] {
    match order {
        DegradeOrder::DigitalFirst => &DIGITAL_FIRST,
        DegradeOrder::TapeFirst => &TAPE_FIRST,
        DegradeOrder::DriveFirst => &DRIVE_FIRST,
    }
}

/// Bitcrush: `q = 2^(bits - 1)`, `y = round(x q) / q`. Bypass at 16 bits.
fn crush(l: &mut [f32], r: &mut [f32], bits: f32) {
    let bits = clampf(bits, 1.0, 16.0);
    if bits >= 16.0 {
        return;
    }
    let q = (bits - 1.0).exp2();
    let inv = 1.0 / q;
    for x in l.iter_mut().chain(r.iter_mut()) {
        *x = (*x * q).round() * inv;
    }
}

const MAX_REDUCE_HZ: f32 = 48_000.0;

/// Sample-and-hold rate reduction driven by a phase accumulator. Bypass at >= host rate.
struct SampleHold {
    phase: f32,
    hold: [f32; 2],
    active: bool,
}

impl SampleHold {
    fn new() -> Self {
        Self { phase: 0.0, hold: [0.0; 2], active: false }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], target_hz: f32, host_sr: f32) {
        let target = clampf(target_hz, 1_000.0, MAX_REDUCE_HZ);
        // The parameter maximum also bypasses, so hosts above 48 kHz stay clean by default.
        if target >= host_sr || target >= MAX_REDUCE_HZ {
            self.active = false;
            return;
        }
        if !self.active {
            // Capture on the very first sample after (re)activation.
            self.active = true;
            self.phase = 1.0;
        }
        let inc = target / host_sr;
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.hold = [*xl, *xr];
            }
            self.phase += inc;
            *xl = self.hold[0];
            *xr = self.hold[1];
        }
    }
}

impl Default for Degrade {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Degrade {
    sample_rate: f32,
    sample_hold: SampleHold,
    wow: WowFlutter,
    saturation: Saturation,
    tone: Tone,
    hiss: Hiss,
    crackle: Crackle,
    dropouts: Dropouts,
    /// True until the first block after a reset: smoothed settings then jump to their targets.
    fresh: bool,
}

impl Degrade {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            sample_hold: SampleHold::new(),
            wow: WowFlutter::new(),
            saturation: Saturation::new(),
            tone: Tone::new(),
            hiss: Hiss::new(),
            crackle: Crackle::new(),
            dropouts: Dropouts::new(),
            fresh: true,
        }
    }

    pub fn reset(&mut self, sample_rate: f32, _max_block: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        self.sample_rate = sr;
        self.sample_hold.reset();
        self.wow.reset(sr);
        self.saturation.reset(sr);
        self.tone.reset(sr);
        self.hiss.reset(sr);
        self.crackle.reset(sr);
        self.dropouts.reset(sr);
        self.fresh = true;
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], snap: &Snapshot, _t: &Transport) {
        let n = l.len().min(r.len());
        let (l, r) = (&mut l[..n], &mut r[..n]);
        // A NaN/inf would latch forever in the filter / DC-blocker / EQ states.
        util::sanitize(l, r);
        if self.fresh {
            // Start at the current settings instead of ramping up from neutral.
            self.fresh = false;
            self.wow.prime(snap.wow_depth, snap.flutter_depth);
            self.saturation.prime(snap.drive, snap.saturation_mode, snap.oversampling.factor());
            self.tone.prime(snap.tape_age, snap.head_bump_db);
            self.hiss.prime(snap.hiss);
        }
        for stage in stages(snap.degrade_order) {
            match stage {
                Stage::Crush => crush(l, r, snap.bit_depth),
                Stage::SampleRate => {
                    self.sample_hold.process(l, r, snap.sample_rate_hz, self.sample_rate)
                }
                Stage::Wow => self.wow.process(
                    l,
                    r,
                    snap.wow_rate_hz,
                    snap.wow_depth,
                    snap.flutter_rate_hz,
                    snap.flutter_depth,
                ),
                Stage::Saturation => self.saturation.process(
                    l,
                    r,
                    snap.drive,
                    snap.saturation_mode,
                    snap.oversampling.factor(),
                ),
                Stage::Tone => self.tone.process(l, r, snap.tape_age, snap.head_bump_db),
                Stage::Noise => {
                    self.hiss.process(l, r, snap.hiss);
                    self.crackle.process(l, r, snap.crackle_density, snap.crackle_level);
                }
                Stage::Dropouts => {
                    self.dropouts.process(l, r, snap.dropout_chance, snap.dropout_depth)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
