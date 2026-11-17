//! Grainwave: granular lofi destroyer.
//!
//! Signal flow per block (see README "Signal flow"):
//! input gain -> dry copy -> modulation (rewrites Snapshot) -> channel encode -> grains
//! -> rhythm -> degrade -> filter -> space -> channel decode -> dry/wet mix -> output.
//!
//! OWNERSHIP: shared file. Feature branches should not need to edit it. If you must,
//! keep the change minimal and mention it in your commit message.

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::Arc;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;
pub mod shared;

use dsp::{
    degrade::Degrade, filter::Filter, grains::GrainEngine, input, modulation::Modulation,
    output::Output, rhythm::Rhythm, space::Space, Transport,
};
use params::{GrainwaveParams, Snapshot};
use shared::VisualState;

pub struct Grainwave {
    params: Arc<GrainwaveParams>,
    visual: Arc<VisualState>,

    sample_rate: f32,
    free_beats: f64,
    last_mix: f32,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,

    modulation: Modulation,
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
        Self {
            params: Arc::new(GrainwaveParams::default()),
            modulation: Modulation::new(visual.clone()),
            grains: GrainEngine::new(visual.clone()),
            rhythm: Rhythm::new(visual.clone()),
            visual,
            sample_rate: 48_000.0,
            free_beats: 0.0,
            last_mix: 1.0,
            dry_l: Vec::new(),
            dry_r: Vec::new(),
            degrade: Degrade::new(),
            filter: Filter::new(),
            space: Space::new(),
            output: Output::new(),
        }
    }
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
        self.modulation.reset(self.sample_rate, max);
        self.grains.reset(self.sample_rate, max);
        self.rhythm.reset(self.sample_rate, max);
        self.degrade.reset(self.sample_rate, max);
        self.filter.reset(self.sample_rate, max);
        self.space.reset(self.sample_rate, max);
        self.output.reset(self.sample_rate, max);
        true
    }

    fn reset(&mut self) {
        let max = self.dry_l.len();
        self.modulation.reset(self.sample_rate, max);
        self.grains.reset(self.sample_rate, max);
        self.rhythm.reset(self.sample_rate, max);
        self.degrade.reset(self.sample_rate, max);
        self.filter.reset(self.sample_rate, max);
        self.space.reset(self.sample_rate, max);
        self.output.reset(self.sample_rate, max);
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
                NoteEvent::NoteOn { note, velocity, .. } => self.grains.note_on(note, velocity),
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

        self.modulation.process_block(&mut snap, l, r, &transport);

        input::encode(l, r, &snap);
        self.grains.process(l, r, &snap, &transport);
        self.rhythm.process(l, r, &snap, &transport);
        self.degrade.process(l, r, &snap, &transport);
        self.filter.process(l, r, &snap, &transport);
        self.space.process(l, r, &snap, &transport);
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
