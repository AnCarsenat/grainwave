//! Grainwave: granular lofi destroyer.
//!
//! Signal flow per block (see README "Signal flow"):
//! input gain -> dry copy -> modulation (rewrites Snapshot) -> channel encode -> time shaper
//! -> grains -> rhythm -> degrade -> filter -> space -> channel decode -> dry/wet mix -> output.
//!
//! Every section from time shaper to space sits inside a `StageFade`: its power switch (and,
//! for grains, the Amount knob) crossfades between the section's input and output over ~10 ms.
//! Sections always keep processing, so switching one back on never replays stale audio.
//!
//! OWNERSHIP: shared file (coordinator).

// nih-plug's `nih_export_vst3!` ends in a statement-position semicolon that newer rustc
// flags as future-incompatible; it's upstream code we can't change here.
#![allow(semicolon_in_expressions_from_non_local_macros)]

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod pattern;
pub mod presets;
pub mod shared;

use dsp::{
    degrade::Degrade, filter::Filter, grains::GrainEngine, input, modulation::Modulation,
    output::Output, rhythm::Rhythm, space::Space, timeshape::TimeShaper, Transport,
};
use params::{GrainwaveParams, Snapshot};
use shared::{store_f32, VisualState};

const STAGE_FADE_MS: f32 = 10.0;

/// Smoothed wet gain for one section. 1 = section fully heard, 0 = bypassed.
#[derive(Clone, Copy)]
struct StageFade {
    gain: f32,
}

impl StageFade {
    /// True when the block needs the pre-section copy (fading or bypassed).
    fn needs_dry(&self, target: f32) -> bool {
        !(self.gain >= 1.0 && target >= 1.0)
    }

    /// Blend `out = pre + (out - pre) * gain`, gain moving toward `target` with a one-pole.
    fn apply(&mut self, l: &mut [f32], r: &mut [f32], pre_l: &[f32], pre_r: &[f32], target: f32, coef: f32) {
        let target = target.clamp(0.0, 1.0);
        for i in 0..l.len() {
            self.gain += (target - self.gain) * coef;
            let g = self.gain;
            l[i] = pre_l[i] + (l[i] - pre_l[i]) * g;
            r[i] = pre_r[i] + (r[i] - pre_r[i]) * g;
        }
        if (self.gain - target).abs() < 1e-4 {
            self.gain = target;
        }
    }
}

pub struct Grainwave {
    params: Arc<GrainwaveParams>,
    visual: Arc<VisualState>,

    sample_rate: f32,
    free_beats: f64,
    last_mix: f32,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    pre_l: Vec<f32>,
    pre_r: Vec<f32>,
    fade_coef: f32,
    /// [time, grains, rhythm, degrade, filter, space]
    fades: [StageFade; 6],

    modulation: Modulation,
    time: TimeShaper,
    grains: GrainEngine,
    rhythm: Rhythm,
    degrade: Degrade,
    filter: Filter,
    space: Space,
    output: Output,
}

impl Default for Grainwave {
    fn default() -> Self {
        let visual = Arc::new(VisualState::default());
        let params = Arc::new(GrainwaveParams::default());
        Self {
            modulation: Modulation::new(visual.clone()),
            time: TimeShaper::new(
                params.time_patterns.clone(),
                params.pattern_version.clone(),
                visual.clone(),
            ),
            grains: GrainEngine::new(visual.clone()),
            rhythm: Rhythm::new(visual.clone()),
            params,
            visual,
            sample_rate: 48_000.0,
            free_beats: 0.0,
            last_mix: 1.0,
            dry_l: Vec::new(),
            dry_r: Vec::new(),
            pre_l: Vec::new(),
            pre_r: Vec::new(),
            fade_coef: 0.002,
            fades: [StageFade { gain: 0.0 }; 6],
            degrade: Degrade::new(),
            filter: Filter::new(),
            space: Space::new(),
            output: Output::new(),
        }
    }
}

impl Grainwave {
    fn reset_modules(&mut self) {
        let max = self.dry_l.len();
        let sr = self.sample_rate;
        self.modulation.reset(sr, max);
        self.time.reset(sr, max);
        self.grains.reset(sr, max);
        self.rhythm.reset(sr, max);
        self.degrade.reset(sr, max);
        self.filter.reset(sr, max);
        self.space.reset(sr, max);
        self.output.reset(sr, max);
        // Start each section at its current target so loading never fades in.
        let snap = Snapshot::from_params(&self.params);
        for (fade, target) in self.fades.iter_mut().zip(stage_targets(&snap, None)) {
            fade.gain = target;
        }
        // Patterns may have been replaced by a state load: force the audio copy to refresh.
        self.params.pattern_version.fetch_add(1, Ordering::Relaxed);
    }
}

/// Section target gains: [time, grains, rhythm, degrade, filter, space].
/// gain = power switch × section mix × FX sequencer step (when that lane is active).
/// `seq_step` is the current sequencer step (0..15), or `None` when the sequencer is off.
fn stage_targets(s: &Snapshot, seq_step: Option<usize>) -> [f32; 6] {
    let on = |b: bool| if b { 1.0 } else { 0.0 };
    let mut t = [
        on(s.on_time),
        on(s.on_grains) * s.grain_amount,
        on(s.on_rhythm) * s.rhythm_mix,
        on(s.on_degrade) * s.degrade_mix,
        on(s.on_filter) * s.filter_mix,
        on(s.on_space) * s.space_mix,
    ];
    if let Some(step) = seq_step {
        for (gain, lane) in t.iter_mut().zip(s.seq_lanes) {
            if lane & params::SEQ_LANE_ACTIVE != 0 && lane & (1 << step) == 0 {
                *gain = 0.0;
            }
        }
    }
    t.map(|g| if g.is_finite() { g.clamp(0.0, 1.0) } else { 0.0 })
}

/// Current FX sequencer step from the host clock, or `None` when the sequencer is off.
fn seq_step(s: &Snapshot, beat_pos: f64) -> Option<usize> {
    if !s.seq_enabled {
        return None;
    }
    let step = (beat_pos / s.seq_rate.beats() + 1e-7).floor();
    Some(step.rem_euclid(params::SEQ_STEPS as f64) as usize)
}

/// Runs one section inside its fade.
macro_rules! stage {
    ($self:ident, $idx:expr, $target:expr, $l:ident, $r:ident, $n:ident, $call:expr) => {{
        let target = $target;
        let fade = $self.fades[$idx];
        if fade.needs_dry(target) {
            $self.pre_l[..$n].copy_from_slice($l);
            $self.pre_r[..$n].copy_from_slice($r);
            $call;
            let coef = $self.fade_coef;
            $self.fades[$idx].apply($l, $r, &$self.pre_l[..$n], &$self.pre_r[..$n], target, coef);
        } else {
            $call;
        }
        store_f32(&$self.visual.section_gain[$idx], $self.fades[$idx].gain);
    }};
}

impl Plugin for Grainwave {
    const NAME: &'static str = "Grainwave";
    const VENDOR: &'static str = "Grainwave";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames::const_default(),
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.visual.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = config.sample_rate;
        let max = config.max_buffer_size as usize;
        self.dry_l = vec![0.0; max];
        self.dry_r = vec![0.0; max];
        self.pre_l = vec![0.0; max];
        self.pre_r = vec![0.0; max];
        self.fade_coef = 1.0 - (-1.0 / (STAGE_FADE_MS * 0.001 * self.sample_rate)).exp();
        self.reset_modules();
        true
    }

    fn reset(&mut self) {
        self.reset_modules();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // MIDI (block accurate is fine for grain pitch).
        while let Some(event) = context.next_event() {
            match event {
                NoteEvent::NoteOn { note, velocity, .. } => {
                    self.grains.note_on(note, velocity);
                    self.time.note_on(note, velocity);
                }
                NoteEvent::NoteOff { note, .. } => self.grains.note_off(note),
                _ => {}
            }
        }

        let n = buffer.samples();
        if n == 0 {
            return ProcessStatus::Normal;
        }

        let host = context.transport();
        let bpm = host.tempo.unwrap_or(120.0).max(1.0);
        let beat_pos = host.pos_beats().unwrap_or(self.free_beats);
        let transport = Transport {
            sample_rate: self.sample_rate,
            bpm,
            beat_pos,
            playing: host.playing,
            block_len: n,
        };
        self.free_beats = beat_pos + n as f64 * bpm / 60.0 / self.sample_rate as f64;

        let mut snap = Snapshot::from_params(&self.params);

        let channels = buffer.as_slice();
        let (left, right) = channels.split_at_mut(1);
        let l = &mut *left[0];
        let r = &mut *right[0];

        // Input gain (smoothed per sample), then keep a dry copy.
        for i in 0..n {
            let g = self.params.input.input_gain.smoothed.next();
            l[i] *= g;
            r[i] *= g;
        }
        self.dry_l[..n].copy_from_slice(l);
        self.dry_r[..n].copy_from_slice(r);

        if snap.on_modulation {
            self.modulation.process_block(&mut snap, l, r, &transport);
        }
        let step = seq_step(&snap, beat_pos);
        self.visual
            .seq_step
            .store(step.map_or(u32::MAX, |s| s as u32), Ordering::Relaxed);
        let targets = stage_targets(&snap, step);

        input::encode(l, r, &snap);
        stage!(self, 0, targets[0], l, r, n, self.time.process(l, r, &snap, &transport));
        stage!(self, 1, targets[1], l, r, n, self.grains.process(l, r, &snap, &transport));
        stage!(self, 2, targets[2], l, r, n, self.rhythm.process(l, r, &snap, &transport));
        stage!(self, 3, targets[3], l, r, n, self.degrade.process(l, r, &snap, &transport));
        stage!(self, 4, targets[4], l, r, n, self.filter.process(l, r, &snap, &transport));
        stage!(self, 5, targets[5], l, r, n, self.space.process(l, r, &snap, &transport));
        input::decode(l, r, &snap);

        // Dry/wet with a per-block linear ramp (mix can be modulated).
        let start = self.last_mix;
        let end = snap.mix.clamp(0.0, 1.0);
        let step = (end - start) / n as f32;
        for i in 0..n {
            let m = start + step * (i + 1) as f32;
            l[i] = self.dry_l[i] * (1.0 - m) + l[i] * m;
            r[i] = self.dry_r[i] * (1.0 - m) + r[i] * m;
        }
        self.last_mix = end;

        self.output.process(l, r, &snap);

        ProcessStatus::Normal
    }
}

impl ClapPlugin for Grainwave {
    const CLAP_ID: &'static str = "dev.grainwave.grainwave";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Granular lofi destroyer: reverse grains, tape stops, crush, wow and flutter");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Granular,
        ClapFeature::Distortion,
        ClapFeature::Glitch,
    ];
}

impl Vst3Plugin for Grainwave {
    const VST3_CLASS_ID: [u8; 16] = *b"GrainwaveLofiFx1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Distortion,
        Vst3SubCategory::Modulation,
    ];
}

nih_export_clap!(Grainwave);
nih_export_vst3!(Grainwave);

#[cfg(test)]
mod tests {
    use super::*;
    use params::{Division, SEQ_LANE_ACTIVE};

    fn snap() -> Snapshot {
        Snapshot::from_params(&GrainwaveParams::default())
    }

    #[test]
    fn default_targets_are_clean() {
        // Time on (flat pattern = transparent), grains amount 0, the rest on at mix 1.
        assert_eq!(stage_targets(&snap(), None), [1.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn power_and_mix_scale_targets() {
        let mut s = snap();
        s.on_degrade = false;
        s.space_mix = 0.25;
        s.grain_amount = 0.5;
        let t = stage_targets(&s, None);
        assert_eq!(t[3], 0.0);
        assert_eq!(t[5], 0.25);
        assert_eq!(t[1], 0.5);
    }

    #[test]
    fn sequencer_gates_only_active_lanes() {
        let mut s = snap();
        s.seq_enabled = true;
        // Degrade lane active with only step 2 on; rhythm lane inactive with no steps.
        s.seq_lanes[3] = SEQ_LANE_ACTIVE | (1 << 2);
        s.seq_lanes[2] = 0;
        assert_eq!(stage_targets(&s, Some(2))[3], 1.0);
        assert_eq!(stage_targets(&s, Some(3))[3], 0.0);
        assert_eq!(stage_targets(&s, Some(3))[2], 1.0, "inactive lane must not gate");
    }

    #[test]
    fn sequencer_step_follows_clock() {
        let mut s = snap();
        assert_eq!(seq_step(&s, 3.0), None);
        s.seq_enabled = true;
        s.seq_rate = Division::Sixteenth;
        assert_eq!(seq_step(&s, 0.0), Some(0));
        assert_eq!(seq_step(&s, 0.25), Some(1));
        assert_eq!(seq_step(&s, 3.99), Some(15));
        assert_eq!(seq_step(&s, 4.0), Some(0));
        s.seq_rate = Division::Eighth;
        assert_eq!(seq_step(&s, 1.0), Some(2));
    }

    #[test]
    fn nan_mix_is_safe() {
        let mut s = snap();
        s.filter_mix = f32::NAN;
        assert_eq!(stage_targets(&s, None)[4], 0.0);
    }
}
