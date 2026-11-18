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
// flags as future-incompatible; it's upstream code we can't change here. `unknown_lints`
// keeps older toolchains (which don't know this lint yet) building with -D warnings.
#![allow(unknown_lints, semicolon_in_expressions_from_non_local_macros)]

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

/// Per-sample FX sequencer gating for one section: the block's step of every sample plus the
/// section's lane word. Only built for active lanes that switch off somewhere in the block.
#[derive(Clone, Copy)]
struct SeqGate<'a> {
    steps: &'a [u8],
    lane: u32,
}

impl StageFade {
    /// True when the block needs the pre-section copy (fading, bypassed or sequencer-gated).
    fn needs_dry(&self, target: f32, gate: Option<SeqGate>) -> bool {
        gate.is_some() || !(self.gain >= 1.0 && target >= 1.0)
    }

    /// Blend `out = pre + (out - pre) * gain`, gain moving toward `target` with a one-pole.
    /// With a sequencer gate, the target drops to 0 on exactly the samples whose step is off.
    fn apply(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        (pre_l, pre_r): (&[f32], &[f32]),
        target: f32,
        gate: Option<SeqGate>,
        coef: f32,
    ) {
        let open = target.clamp(0.0, 1.0);
        let mut target = open;
        for i in 0..l.len() {
            if let Some(g) = gate {
                target = if g.lane & (1 << g.steps[i]) != 0 { open } else { 0.0 };
            }
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
    /// FX sequencer step of every sample in the current block.
    seq_steps: Vec<u8>,
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
            seq_steps: Vec::new(),
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
        for (fade, target) in self.fades.iter_mut().zip(stage_targets(&snap)) {
            fade.gain = target;
        }
        // Same for the dry/wet ramp: start at the current mix, not the pre-reset value.
        self.last_mix = if snap.mix.is_finite() { snap.mix.clamp(0.0, 1.0) } else { 1.0 };
        // Patterns may have been replaced by a state load: force the audio copy to refresh.
        self.params.pattern_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Routes one MIDI event to the grain engine (pitch) and the time shaper (pattern switch /
    /// trigger). A note-on with velocity 0 is a note-off (MIDI running status; VST3 passes it
    /// through as a note-on), so it must not switch or restart the time shaper.
    fn handle_note(&mut self, event: NoteEvent<()>) {
        match event {
            NoteEvent::NoteOn { note, velocity, .. } if velocity > 0.0 => {
                self.grains.note_on(note, velocity);
                self.time.note_on(note, velocity);
            }
            NoteEvent::NoteOn { note, .. } | NoteEvent::NoteOff { note, .. } => {
                self.grains.note_off(note)
            }
            _ => {}
        }
    }
}

/// Section target gains before sequencer gating: [time, grains, rhythm, degrade, filter, space].
/// gain = power switch × section mix. The FX sequencer then gates active lanes per sample
/// (see `seq_gate`).
fn stage_targets(s: &Snapshot) -> [f32; 6] {
    let on = |b: bool| if b { 1.0 } else { 0.0 };
    let t = [
        on(s.on_time),
        on(s.on_grains) * s.grain_amount,
        on(s.on_rhythm) * s.rhythm_mix,
        on(s.on_degrade) * s.degrade_mix,
        on(s.on_filter) * s.filter_mix,
        on(s.on_space) * s.space_mix,
    ];
    t.map(|g| if g.is_finite() { g.clamp(0.0, 1.0) } else { 0.0 })
}

/// Beat position at the start of this block.
///
/// Modules derive per-sample positions as `beat_pos + i / samples_per_beat`, so the block
/// start must advance with the audio. A stopped host keeps reporting the same playhead every
/// block, which would replay the same slice of every tempo grid (time shaper curve, stutter
/// and gate steps, synced grains, LFOs, sequencer) once per block. While the host is not
/// playing, or gives no usable position, the internal free-running counter is used instead;
/// it continues from the last host position, and pressing play is just a seek.
fn block_beat_pos(host_pos: Option<f64>, playing: bool, free_beats: f64) -> f64 {
    match host_pos {
        Some(p) if playing && p.is_finite() => p,
        _ => free_beats,
    }
}

/// FX sequencer step (0..15) at `beat_pos` on the host clock, or `None` when the sequencer is off.
fn seq_step(s: &Snapshot, beat_pos: f64) -> Option<usize> {
    if !s.seq_enabled {
        return None;
    }
    let step = (beat_pos / s.seq_rate.beats() + 1e-7).floor() as i64;
    Some(step.rem_euclid(params::SEQ_STEPS as i64) as usize)
}

/// Fills `out` with the sequencer step of every sample of the block (`beat_pos` at sample 0,
/// advancing `beats_per_sample`). Returns the bitmask of the steps that occur in the block, or
/// `None` when the sequencer is off.
fn fill_seq_steps(s: &Snapshot, beat_pos: f64, beats_per_sample: f64, out: &mut [u8]) -> Option<u32> {
    if !s.seq_enabled {
        return None;
    }
    let mut mask = 0u32;
    for (i, o) in out.iter_mut().enumerate() {
        let step = seq_step(s, beat_pos + i as f64 * beats_per_sample).unwrap_or(0);
        *o = step as u8;
        mask |= 1 << step;
    }
    Some(mask)
}

/// Gate for one section's lane, or `None` when the lane cannot switch the section off in this
/// block (sequencer off, lane inactive, or every step present in the block is on).
fn seq_gate(lane: u32, steps: &[u8], present: Option<u32>) -> Option<SeqGate<'_>> {
    let present = present?;
    let active = lane & params::SEQ_LANE_ACTIVE != 0;
    (active && present & !lane & 0xFFFF != 0).then_some(SeqGate { steps, lane })
}

/// Runs one section inside its fade.
macro_rules! stage {
    ($self:ident, $idx:expr, $target:expr, $gate:expr, $l:ident, $r:ident, $n:ident, $call:expr) => {{
        let target = $target;
        let gate = $gate;
        let fade = $self.fades[$idx];
        if fade.needs_dry(target, gate) {
            $self.pre_l[..$n].copy_from_slice($l);
            $self.pre_r[..$n].copy_from_slice($r);
            $call;
            let coef = $self.fade_coef;
            $self.fades[$idx].apply(
                $l,
                $r,
                (&$self.pre_l[..$n], &$self.pre_r[..$n]),
                target,
                gate,
                coef,
            );
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
        self.seq_steps = vec![0; max];
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
            self.handle_note(event);
        }

        let n = buffer.samples();
        if n == 0 {
            return ProcessStatus::Normal;
        }

        let host = context.transport();
        let bpm = host.tempo.unwrap_or(120.0).max(1.0);
        let beat_pos = block_beat_pos(host.pos_beats(), host.playing, self.free_beats);
        let transport = Transport {
            sample_rate: self.sample_rate,
            bpm,
            beat_pos,
            playing: host.playing,
            block_len: n,
        };
        let beats_per_sample = bpm / 60.0 / self.sample_rate as f64;
        self.free_beats = beat_pos + n as f64 * beats_per_sample;

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
        // FX sequencer: step per sample, so lanes switch on the step boundary itself rather than
        // at the next block start (blocks can be longer than a step).
        let present = fill_seq_steps(&snap, beat_pos, beats_per_sample, &mut self.seq_steps[..n]);
        self.visual.seq_step.store(
            present.map_or(u32::MAX, |_| self.seq_steps[n - 1] as u32),
            Ordering::Relaxed,
        );
        let targets = stage_targets(&snap);
        let steps = &self.seq_steps[..n];
        let gates: [Option<SeqGate>; 6] =
            std::array::from_fn(|i| seq_gate(snap.seq_lanes[i], steps, present));

        input::encode(l, r, &snap);
        stage!(self, 0, targets[0], gates[0], l, r, n, self.time.process(l, r, &snap, &transport));
        stage!(self, 1, targets[1], gates[1], l, r, n, self.grains.process(l, r, &snap, &transport));
        stage!(self, 2, targets[2], gates[2], l, r, n, self.rhythm.process(l, r, &snap, &transport));
        stage!(self, 3, targets[3], gates[3], l, r, n, self.degrade.process(l, r, &snap, &transport));
        stage!(self, 4, targets[4], gates[4], l, r, n, self.filter.process(l, r, &snap, &transport));
        stage!(self, 5, targets[5], gates[5], l, r, n, self.space.process(l, r, &snap, &transport));
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
        assert_eq!(stage_targets(&snap()), [1.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn power_and_mix_scale_targets() {
        let mut s = snap();
        s.on_degrade = false;
        s.space_mix = 0.25;
        s.grain_amount = 0.5;
        let t = stage_targets(&s);
        assert_eq!(t[3], 0.0);
        assert_eq!(t[5], 0.25);
        assert_eq!(t[1], 0.5);
    }

    #[test]
    fn sequencer_gates_only_active_lanes() {
        // Block where steps 2 and 3 occur.
        let steps = [2u8, 2, 3, 3];
        let present = Some((1 << 2) | (1 << 3));
        // Degrade lane active with only step 2 on: gated. Rhythm lane inactive: never gated.
        let degrade = SEQ_LANE_ACTIVE | (1 << 2);
        assert!(seq_gate(degrade, &steps, present).is_some());
        assert!(seq_gate(0, &steps, present).is_none(), "inactive lane must not gate");
        // Every present step on: no gating, so the fast path stays available.
        let both = SEQ_LANE_ACTIVE | (1 << 2) | (1 << 3);
        assert!(seq_gate(both, &steps, present).is_none());
        // Sequencer off.
        assert!(seq_gate(degrade, &steps, None).is_none());
    }

    #[test]
    fn sequencer_steps_change_mid_block() {
        // 120 BPM at 48 kHz, 1/16 steps: one step = 6000 samples. A 8192-sample block
        // starting on step 0 must switch to step 1 at sample 6000, not at the next block.
        let mut s = snap();
        s.seq_enabled = true;
        s.seq_rate = Division::Sixteenth;
        let bps = 120.0 / 60.0 / 48_000.0;
        let mut out = vec![0u8; 8192];
        assert_eq!(fill_seq_steps(&s, 0.0, bps, &mut out), Some(0b11));
        assert_eq!(out[5999], 0);
        assert_eq!(out[6000], 1);
        assert_eq!(out[8191], 1);
        // Wraps after 16 steps and handles negative (pre-roll) positions.
        assert_eq!(fill_seq_steps(&s, -0.25, bps, &mut out[..1]), Some(1 << 15));
        s.seq_enabled = false;
        assert_eq!(fill_seq_steps(&s, 0.0, bps, &mut out), None);
    }

    #[test]
    fn gate_closes_on_the_step_boundary_inside_a_block() {
        // Lane: step 0 on, step 1 off. Boundary at sample 100 of a 400-sample block.
        let mut steps = [0u8; 400];
        steps[100..].fill(1);
        let lane = SEQ_LANE_ACTIVE | 1;
        let gate = seq_gate(lane, &steps, Some(0b11));
        let mut fade = StageFade { gain: 1.0 };
        assert!(fade.needs_dry(1.0, gate));
        let pre = [0.0f32; 400];
        let (mut l, mut r) = ([1.0f32; 400], [1.0f32; 400]);
        fade.apply(&mut l, &mut r, (&pre, &pre), 1.0, gate, 0.05);
        assert_eq!(l[99], 1.0, "still open before the boundary");
        assert!(l[100] < 1.0, "starts closing at the boundary sample");
        assert!(l[399] < 1e-3, "closed by the end of the block");
        assert_eq!(l, r);
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
    fn stopped_host_keeps_beat_moving() {
        // Stopped host: same reported playhead every block. The block start must still advance.
        let spb_blocks = 512.0 * 120.0 / 60.0 / 48_000.0;
        let mut free = 7.3;
        let mut last = f64::NEG_INFINITY;
        for _ in 0..10 {
            let pos = block_beat_pos(Some(7.3), false, free);
            assert!(pos > last, "beat position must advance while stopped");
            last = pos;
            free = pos + spb_blocks;
        }
        // Playing: the host position wins (loops and seeks follow the host).
        assert_eq!(block_beat_pos(Some(2.0), true, 99.0), 2.0);
        // No position, or garbage: free-running counter.
        assert_eq!(block_beat_pos(None, true, 5.0), 5.0);
        assert_eq!(block_beat_pos(Some(f64::NAN), true, 5.0), 5.0);
    }

    #[test]
    fn velocity_zero_note_on_is_a_note_off_for_the_time_shaper() {
        let mut gw = Grainwave::default();
        gw.time.reset(48_000.0, 128);
        let mut s = snap();
        s.ts_pattern_quantize = params::PatternQuantize::Off;
        let t = |beat_pos| Transport {
            sample_rate: 48_000.0,
            bpm: 120.0,
            beat_pos,
            playing: true,
            block_len: 128,
        };
        let (mut l, mut r) = ([0.0f32; 128], [0.0f32; 128]);
        let active = |gw: &Grainwave| gw.visual.ts_active.load(Ordering::Relaxed);
        let on = |note, velocity| NoteEvent::NoteOn { timing: 0, voice_id: None, channel: 0, note, velocity };
        gw.time.process(&mut l, &mut r, &s, &t(0.0));
        assert_eq!(active(&gw), 0);

        // A real note-on (D) switches to pattern 3.
        gw.handle_note(on(62, 0.8));
        gw.time.process(&mut l, &mut r, &s, &t(0.1));
        assert_eq!(active(&gw), 2);
        // A velocity-0 note-on is a release (MIDI running-status note-off): E must not
        // switch to pattern 5 or restart the cycle.
        gw.handle_note(on(64, 0.0));
        gw.time.process(&mut l, &mut r, &s, &t(0.3));
        assert_eq!(active(&gw), 2, "velocity-0 note-on switched the pattern");
    }

    #[test]
    fn reset_restarts_mix_ramp_at_current_mix() {
        // A ramp left over from before the reset (e.g. a session that was at 25 % before a
        // state load restoring 100 %) must not be replayed on the first block after it.
        let mut gw = Grainwave::default();
        gw.last_mix = 0.25;
        gw.reset_modules();
        assert_eq!(gw.last_mix, gw.params.output.mix.value());
    }

    #[test]
    fn nan_mix_is_safe() {
        let mut s = snap();
        s.filter_mix = f32::NAN;
        assert_eq!(stage_targets(&s)[4], 0.0);
    }
}
