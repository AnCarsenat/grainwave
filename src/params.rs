//! All Grainwave parameters, plus `Snapshot`, a plain-value copy taken once per block.
//!
//! OWNERSHIP: this file is shared by every feature branch. Do not rename or remove
//! parameters or change their IDs (that breaks saved sessions). If a branch needs a
//! new parameter, add it at the end of its own group and its own Snapshot section.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ChannelMode {
    Stereo,
    Mono,
    #[name = "Mid/Side"]
    MidSide,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum BufferMode {
    #[name = "Bars (synced)"]
    Synced,
    #[name = "Seconds (free)"]
    Free,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum DensityMode {
    #[name = "Free (Hz)"]
    Free,
    #[name = "Synced"]
    Synced,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum GrainEnvelope {
    Hann,
    Triangle,
    Square,
    #[name = "Exp decay"]
    ExpDecay,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum PitchQuantize {
    Off,
    Octaves,
    Fifths,
    Major,
    Minor,
    Pentatonic,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Division {
    #[name = "1/4"]
    Quarter,
    #[name = "1/8"]
    Eighth,
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/32"]
    ThirtySecond,
    #[name = "1/8T"]
    EighthTriplet,
    #[name = "1/16T"]
    SixteenthTriplet,
}

impl Division {
    /// Length of one division in quarter-note beats.
    pub fn beats(self) -> f64 {
        match self {
            Division::Quarter => 1.0,
            Division::Eighth => 0.5,
            Division::Sixteenth => 0.25,
            Division::ThirtySecond => 0.125,
            Division::EighthTriplet => 1.0 / 3.0,
            Division::SixteenthTriplet => 1.0 / 6.0,
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum SaturationMode {
    Tape,
    Tube,
    #[name = "Hard clip"]
    HardClip,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum DegradeOrder {
    /// crush -> sample rate -> wow/flutter -> saturation -> tape age -> noise -> dropouts
    #[name = "Digital first"]
    DigitalFirst,
    /// wow/flutter -> saturation -> tape age -> crush -> sample rate -> noise -> dropouts
    #[name = "Tape first"]
    TapeFirst,
    /// saturation -> crush -> sample rate -> wow/flutter -> tape age -> noise -> dropouts
    #[name = "Drive first"]
    DriveFirst,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Oversampling {
    #[name = "1x"]
    X1,
    #[name = "2x"]
    X2,
    #[name = "4x"]
    X4,
}

impl Oversampling {
    pub fn factor(self) -> usize {
        match self {
            Oversampling::X1 => 1,
            Oversampling::X2 => 2,
            Oversampling::X4 => 4,
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum LfoShape {
    Sine,
    Triangle,
    Saw,
    Square,
    Random,
    #[name = "Smooth random"]
    SmoothRandom,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum LfoRate {
    #[name = "Free (Hz)"]
    Free,
    #[name = "1/1"]
    Whole,
    #[name = "1/2"]
    Half,
    #[name = "1/4"]
    Quarter,
    #[name = "1/8"]
    Eighth,
    #[name = "1/16"]
    Sixteenth,
    #[name = "2 bars"]
    TwoBars,
    #[name = "4 bars"]
    FourBars,
}

/// Every parameter a modulation source can target. Keep `None` first.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ModTarget {
    None,
    #[name = "Grain size"]
    GrainSize,
    Density,
    Position,
    #[name = "Position jitter"]
    PositionJitter,
    Pitch,
    #[name = "Pitch jitter"]
    PitchJitter,
    #[name = "Reverse chance"]
    ReverseChance,
    Speed,
    Spread,
    #[name = "Stutter chance"]
    StutterChance,
    #[name = "Bit depth"]
    BitDepth,
    #[name = "Sample rate"]
    SampleRate,
    #[name = "Wow depth"]
    WowDepth,
    #[name = "Flutter depth"]
    FlutterDepth,
    Drive,
    #[name = "Tape age"]
    TapeAge,
    #[name = "Low-pass cutoff"]
    LowpassCutoff,
    #[name = "High-pass cutoff"]
    HighpassCutoff,
    #[name = "Reverb mix"]
    ReverbMix,
    #[name = "Delay mix"]
    DelayMix,
    Mix,
}

impl ModTarget {
    pub const COUNT: usize = 22;
}

// ---------------------------------------------------------------------------
// Parameter groups
// ---------------------------------------------------------------------------

fn percent(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

#[derive(Params)]
pub struct InputParams {
    #[id = "in_gain"]
    pub input_gain: FloatParam,
    #[id = "ch_mode"]
    pub channel_mode: EnumParam<ChannelMode>,
    #[id = "buf_mode"]
    pub buffer_mode: EnumParam<BufferMode>,
    #[id = "buf_bars"]
    pub buffer_bars: IntParam,
    #[id = "buf_secs"]
    pub buffer_seconds: FloatParam,
    #[id = "freeze"]
    pub freeze: BoolParam,
}

impl Default for InputParams {
    fn default() -> Self {
        Self {
            input_gain: FloatParam::new(
                "Input gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            channel_mode: EnumParam::new("Channel mode", ChannelMode::Stereo),
            buffer_mode: EnumParam::new("Buffer mode", BufferMode::Free),
            buffer_bars: IntParam::new("Buffer bars", 2, IntRange::Linear { min: 1, max: 8 })
                .with_unit(" bars"),
            buffer_seconds: FloatParam::new(
                "Buffer length",
                2.0,
                FloatRange::Skewed { min: 0.5, max: 8.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit(" s")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            freeze: BoolParam::new("Freeze", false),
        }
    }
}

#[derive(Params)]
pub struct GrainParams {
    #[id = "g_size"]
    pub size_ms: FloatParam,
    #[id = "g_dmode"]
    pub density_mode: EnumParam<DensityMode>,
    #[id = "g_dens"]
    pub density_hz: FloatParam,
    #[id = "g_ddiv"]
    pub density_division: EnumParam<Division>,
    #[id = "g_pos"]
    pub position: FloatParam,
    #[id = "g_jit"]
    pub position_jitter: FloatParam,
    #[id = "g_mem"]
    pub memory: FloatParam,
    #[id = "g_env"]
    pub envelope: EnumParam<GrainEnvelope>,
    #[id = "g_voices"]
    pub max_voices: IntParam,
    #[id = "g_spread"]
    pub spread: FloatParam,
    #[id = "g_lowcpu"]
    pub low_cpu: BoolParam,
}

impl Default for GrainParams {
    fn default() -> Self {
        Self {
            size_ms: FloatParam::new(
                "Grain size",
                80.0,
                FloatRange::Skewed { min: 5.0, max: 500.0, factor: FloatRange::skew_factor(-1.5) },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            density_mode: EnumParam::new("Density mode", DensityMode::Free),
            density_hz: FloatParam::new(
                "Density",
                12.0,
                FloatRange::Skewed { min: 1.0, max: 64.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            density_division: EnumParam::new("Density division", Division::Sixteenth),
            position: percent("Position", 0.0),
            position_jitter: percent("Position jitter", 0.1),
            memory: percent("Memory", 0.0),
            envelope: EnumParam::new("Grain envelope", GrainEnvelope::Hann),
            max_voices: IntParam::new("Max voices", 16, IntRange::Linear { min: 1, max: 32 }),
            spread: percent("Stereo spread", 0.3),
            low_cpu: BoolParam::new("Low CPU", false),
        }
    }
}

#[derive(Params)]
pub struct PlaybackParams {
    #[id = "p_rev"]
    pub reverse_chance: FloatParam,
    #[id = "p_pitch"]
    pub pitch_st: FloatParam,
    #[id = "p_pjit"]
    pub pitch_jitter_st: FloatParam,
    #[id = "p_quant"]
    pub pitch_quantize: EnumParam<PitchQuantize>,
    #[id = "p_stop"]
    pub tape_stop_chance: FloatParam,
    #[id = "p_start"]
    pub tape_start_chance: FloatParam,
    #[id = "p_speed"]
    pub speed: FloatParam,
    #[id = "p_midi"]
    pub midi_pitch: BoolParam,
}

impl Default for PlaybackParams {
    fn default() -> Self {
        Self {
            reverse_chance: percent("Reverse chance", 0.0),
            pitch_st: FloatParam::new("Pitch", 0.0, FloatRange::Linear { min: -24.0, max: 24.0 })
                .with_unit(" st")
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            pitch_jitter_st: FloatParam::new(
                "Pitch jitter",
                0.0,
                FloatRange::Linear { min: 0.0, max: 12.0 },
            )
            .with_unit(" st")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            pitch_quantize: EnumParam::new("Pitch quantize", PitchQuantize::Off),
            tape_stop_chance: percent("Tape-stop chance", 0.0),
            tape_start_chance: percent("Tape-start chance", 0.0),
            speed: FloatParam::new(
                "Speed",
                1.0,
                FloatRange::Skewed { min: 0.25, max: 4.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit("x")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            midi_pitch: BoolParam::new("MIDI pitch", false),
        }
    }
}

#[derive(Params)]
pub struct GateStepParams {
    #[id = "on"]
    pub on: BoolParam,
}

impl Default for GateStepParams {
    fn default() -> Self {
        Self { on: BoolParam::new("Step", true) }
    }
}

#[derive(Params)]
pub struct RhythmParams {
    #[id = "r_stut"]
    pub stutter_division: EnumParam<Division>,
    #[id = "r_stch"]
    pub stutter_chance: FloatParam,
    #[id = "r_swing"]
    pub swing: FloatParam,
    #[id = "r_gate"]
    pub gate_enabled: BoolParam,
    #[nested(array, group = "Gate steps")]
    pub gate_steps: [GateStepParams; 16],
    #[id = "r_rewind"]
    pub rewind: BoolParam,
    #[id = "r_tstop"]
    pub tape_stop: BoolParam,
    #[id = "r_tstopt"]
    pub tape_stop_time_ms: FloatParam,
}

impl Default for RhythmParams {
    fn default() -> Self {
        Self {
            stutter_division: EnumParam::new("Stutter division", Division::Sixteenth),
            stutter_chance: percent("Stutter chance", 0.0),
            swing: percent("Swing", 0.0),
            gate_enabled: BoolParam::new("Gate", false),
            gate_steps: Default::default(),
            rewind: BoolParam::new("Rewind", false),
            tape_stop: BoolParam::new("Tape stop", false),
            tape_stop_time_ms: FloatParam::new(
                "Tape-stop time",
                600.0,
                FloatRange::Skewed { min: 50.0, max: 4000.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
        }
    }
}

#[derive(Params)]
pub struct DegradeParams {
    #[id = "d_order"]
    pub order: EnumParam<DegradeOrder>,
    #[id = "d_bits"]
    pub bit_depth: FloatParam,
    #[id = "d_srate"]
    pub sample_rate_hz: FloatParam,
    #[id = "d_wowr"]
    pub wow_rate_hz: FloatParam,
    #[id = "d_wowd"]
    pub wow_depth: FloatParam,
    #[id = "d_flr"]
    pub flutter_rate_hz: FloatParam,
    #[id = "d_fld"]
    pub flutter_depth: FloatParam,
    #[id = "d_satm"]
    pub saturation_mode: EnumParam<SaturationMode>,
    #[id = "d_drive"]
    pub drive: FloatParam,
    #[id = "d_age"]
    pub tape_age: FloatParam,
    #[id = "d_bump"]
    pub head_bump_db: FloatParam,
    #[id = "d_hiss"]
    pub hiss: FloatParam,
    #[id = "d_crdens"]
    pub crackle_density: FloatParam,
    #[id = "d_crlvl"]
    pub crackle_level: FloatParam,
    #[id = "d_dropch"]
    pub dropout_chance: FloatParam,
    #[id = "d_dropd"]
    pub dropout_depth: FloatParam,
    #[id = "d_os"]
    pub oversampling: EnumParam<Oversampling>,
}

impl Default for DegradeParams {
    fn default() -> Self {
        Self {
            order: EnumParam::new("Degrade order", DegradeOrder::DigitalFirst),
            bit_depth: FloatParam::new("Bit depth", 16.0, FloatRange::Linear { min: 1.0, max: 16.0 })
                .with_unit(" bit")
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            sample_rate_hz: FloatParam::new(
                "Sample rate",
                48000.0,
                FloatRange::Skewed { min: 1000.0, max: 48000.0, factor: FloatRange::skew_factor(-1.5) },
            )
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            wow_rate_hz: FloatParam::new(
                "Wow rate",
                0.5,
                FloatRange::Skewed { min: 0.05, max: 4.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            wow_depth: percent("Wow depth", 0.0),
            flutter_rate_hz: FloatParam::new(
                "Flutter rate",
                8.0,
                FloatRange::Skewed { min: 4.0, max: 30.0, factor: FloatRange::skew_factor(-1.0) },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            flutter_depth: percent("Flutter depth", 0.0),
            saturation_mode: EnumParam::new("Saturation", SaturationMode::Tape),
            drive: percent("Drive", 0.0),
            tape_age: percent("Tape age", 0.0),
            head_bump_db: FloatParam::new("Head bump", 0.0, FloatRange::Linear { min: 0.0, max: 9.0 })
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            hiss: percent("Hiss", 0.0),
            crackle_density: percent("Crackle density", 0.0),
            crackle_level: percent("Crackle level", 0.3),
            dropout_chance: percent("Dropout chance", 0.0),
            dropout_depth: percent("Dropout depth", 0.6),
            oversampling: EnumParam::new("Oversampling", Oversampling::X1),
        }
    }
}

#[derive(Params)]
pub struct FilterParams {
    #[id = "f_lp"]
    pub lowpass_hz: FloatParam,
    #[id = "f_lpq"]
    pub lowpass_res: FloatParam,
    #[id = "f_hp"]
    pub highpass_hz: FloatParam,
    #[id = "f_hpq"]
    pub highpass_res: FloatParam,
    #[id = "f_drift"]
    pub drift: FloatParam,
}

fn cutoff(name: &str, default: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed { min: 20.0, max: 20000.0, factor: FloatRange::skew_factor(-2.0) },
    )
    .with_smoother(SmoothingStyle::Logarithmic(30.0))
    .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
    .with_string_to_value(formatters::s2v_f32_hz_then_khz())
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            lowpass_hz: cutoff("Low-pass", 20000.0),
            lowpass_res: percent("Low-pass resonance", 0.0),
            highpass_hz: cutoff("High-pass", 20.0),
            highpass_res: percent("High-pass resonance", 0.0),
            drift: percent("Filter drift", 0.0),
        }
    }
}

#[derive(Params)]
pub struct SpaceParams {
    #[id = "s_rvsize"]
    pub reverb_size: FloatParam,
    #[id = "s_rvmix"]
    pub reverb_mix: FloatParam,
    #[id = "s_dldiv"]
    pub delay_division: EnumParam<Division>,
    #[id = "s_dlfb"]
    pub delay_feedback: FloatParam,
    #[id = "s_dlmix"]
    pub delay_mix: FloatParam,
    #[id = "s_dlcrush"]
    pub delay_crush: FloatParam,
}

impl Default for SpaceParams {
    fn default() -> Self {
        Self {
            reverb_size: percent("Reverb size", 0.5),
            reverb_mix: percent("Reverb mix", 0.0),
            delay_division: EnumParam::new("Delay time", Division::Eighth),
            delay_feedback: FloatParam::new(
                "Delay feedback",
                0.35,
                FloatRange::Linear { min: 0.0, max: 0.95 },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            delay_mix: percent("Delay mix", 0.0),
            delay_crush: percent("Delay crush", 0.0),
        }
    }
}

#[derive(Params)]
pub struct LfoParams {
    #[id = "shape"]
    pub shape: EnumParam<LfoShape>,
    #[id = "rmode"]
    pub rate_mode: EnumParam<LfoRate>,
    #[id = "hz"]
    pub rate_hz: FloatParam,
    #[id = "target"]
    pub target: EnumParam<ModTarget>,
    #[id = "amt"]
    pub amount: FloatParam,
}

impl Default for LfoParams {
    fn default() -> Self {
        Self {
            shape: EnumParam::new("LFO shape", LfoShape::Sine),
            rate_mode: EnumParam::new("LFO rate mode", LfoRate::Free),
            rate_hz: FloatParam::new(
                "LFO rate",
                0.5,
                FloatRange::Skewed { min: 0.01, max: 20.0, factor: FloatRange::skew_factor(-2.0) },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            target: EnumParam::new("LFO target", ModTarget::None),
            amount: FloatParam::new("LFO amount", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 })
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
        }
    }
}

#[derive(Params)]
pub struct ModulationParams {
    #[nested(array, group = "LFO")]
    pub lfos: [LfoParams; 2],
    #[id = "env_target"]
    pub env_target: EnumParam<ModTarget>,
    #[id = "env_amt"]
    pub env_amount: FloatParam,
    #[id = "env_att"]
    pub env_attack_ms: FloatParam,
    #[id = "env_rel"]
    pub env_release_ms: FloatParam,
}

impl Default for ModulationParams {
    fn default() -> Self {
        Self {
            lfos: Default::default(),
            env_target: EnumParam::new("Envelope target", ModTarget::None),
            env_amount: FloatParam::new(
                "Envelope amount",
                0.0,
                FloatRange::Linear { min: -1.0, max: 1.0 },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            env_attack_ms: FloatParam::new(
                "Envelope attack",
                10.0,
                FloatRange::Skewed { min: 0.1, max: 500.0, factor: FloatRange::skew_factor(-1.5) },
            )
            .with_unit(" ms"),
            env_release_ms: FloatParam::new(
                "Envelope release",
                150.0,
                FloatRange::Skewed { min: 1.0, max: 2000.0, factor: FloatRange::skew_factor(-1.5) },
            )
            .with_unit(" ms"),
        }
    }
}

#[derive(Params)]
pub struct MacroParams {
    #[id = "m_dust"]
    pub dust: FloatParam,
    #[id = "m_chaos"]
    pub chaos: FloatParam,
    #[id = "m_age"]
    pub age: FloatParam,
}

impl Default for MacroParams {
    fn default() -> Self {
        Self {
            dust: percent("Dust", 0.0),
            chaos: percent("Chaos", 0.0),
            age: percent("Age", 0.0),
        }
    }
}

#[derive(Params)]
pub struct OutputParams {
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "out_gain"]
    pub output_gain: FloatParam,
    #[id = "limiter"]
    pub limiter: BoolParam,
}

impl Default for OutputParams {
    fn default() -> Self {
        Self {
            mix: percent("Mix", 1.0).with_smoother(SmoothingStyle::Linear(20.0)),
            output_gain: FloatParam::new(
                "Output gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 12.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            limiter: BoolParam::new("Limiter", true),
        }
    }
}

/// Per-section power switches. Off = the section is crossfaded out over ~10 ms but keeps
/// running underneath, so switching it back on never replays stale audio.
#[derive(Params)]
pub struct SectionParams {
    #[id = "on_time"]
    pub time: BoolParam,
    #[id = "on_grain"]
    pub grains: BoolParam,
    /// How much of the grain engine replaces the signal (0 = untouched). Same crossfade as the switch.
    #[id = "g_amt"]
    pub grain_amount: FloatParam,
    #[id = "on_rhythm"]
    pub rhythm: BoolParam,
    #[id = "on_degrade"]
    pub degrade: BoolParam,
    #[id = "on_filter"]
    pub filter: BoolParam,
    #[id = "on_space"]
    pub space: BoolParam,
    #[id = "on_mod"]
    pub modulation: BoolParam,
}

impl Default for SectionParams {
    fn default() -> Self {
        Self {
            time: BoolParam::new("Time on", true),
            grains: BoolParam::new("Grains on", true),
            grain_amount: percent("Grain amount", 0.0),
            rhythm: BoolParam::new("Rhythm on", true),
            degrade: BoolParam::new("Degrade on", true),
            filter: BoolParam::new("Filter on", true),
            space: BoolParam::new("Space on", true),
            modulation: BoolParam::new("Modulation on", true),
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum TimeSync {
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/8"]
    Eighth,
    #[name = "1/4"]
    Quarter,
    #[name = "1/2"]
    Half,
    #[name = "1/1"]
    Bar,
    #[name = "2/1"]
    TwoBars,
    #[name = "4/1"]
    FourBars,
    #[name = "1/16T"]
    SixteenthTriplet,
    #[name = "1/8T"]
    EighthTriplet,
    #[name = "1/4T"]
    QuarterTriplet,
    #[name = "1/2T"]
    HalfTriplet,
    #[name = "1/16."]
    SixteenthDotted,
    #[name = "1/8."]
    EighthDotted,
    #[name = "1/4."]
    QuarterDotted,
}

impl TimeSync {
    /// Cycle length in quarter-note beats.
    pub fn beats(self) -> f64 {
        match self {
            TimeSync::Sixteenth => 0.25,
            TimeSync::Eighth => 0.5,
            TimeSync::Quarter => 1.0,
            TimeSync::Half => 2.0,
            TimeSync::Bar => 4.0,
            TimeSync::TwoBars => 8.0,
            TimeSync::FourBars => 16.0,
            TimeSync::SixteenthTriplet => 0.25 * 2.0 / 3.0,
            TimeSync::EighthTriplet => 0.5 * 2.0 / 3.0,
            TimeSync::QuarterTriplet => 2.0 / 3.0,
            TimeSync::HalfTriplet => 4.0 / 3.0,
            TimeSync::SixteenthDotted => 0.375,
            TimeSync::EighthDotted => 0.75,
            TimeSync::QuarterDotted => 1.5,
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum TimeTrigger {
    /// Cycle follows the host playhead.
    Sync,
    /// Any MIDI note restarts the cycle; it runs once then holds.
    #[name = "MIDI"]
    Midi,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum PatternQuantize {
    Off,
    #[name = "1/4 beat"]
    QuarterBeat,
    #[name = "1/2 beat"]
    HalfBeat,
    #[name = "1 beat"]
    Beat,
    #[name = "2 beats"]
    TwoBeats,
    #[name = "1 bar"]
    Bar,
}

impl PatternQuantize {
    /// Switch grid in beats, `None` = immediate.
    pub fn beats(self) -> Option<f64> {
        match self {
            PatternQuantize::Off => None,
            PatternQuantize::QuarterBeat => Some(0.25),
            PatternQuantize::HalfBeat => Some(0.5),
            PatternQuantize::Beat => Some(1.0),
            PatternQuantize::TwoBeats => Some(2.0),
            PatternQuantize::Bar => Some(4.0),
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum AntiClick {
    Off,
    Low,
    Medium,
    High,
}

/// Time shaper: a drawn curve per cycle sets how far behind live the playback reads.
/// y = 0 plays live (dry), a rising slope of 1 holds/repeats, a slope of 0.5 is half speed,
/// steps jump back (stutter). The 12 patterns themselves live in `pattern.rs`.
#[derive(Params)]
pub struct TimeParams {
    #[id = "ts_pat"]
    pub pattern: IntParam,
    #[id = "ts_sync"]
    pub sync: EnumParam<TimeSync>,
    #[id = "ts_trig"]
    pub trigger: EnumParam<TimeTrigger>,
    #[id = "ts_patq"]
    pub pattern_quantize: EnumParam<PatternQuantize>,
    #[id = "ts_midisw"]
    pub midi_switch: BoolParam,
    #[id = "ts_depth"]
    pub depth: FloatParam,
    #[id = "ts_phase"]
    pub phase: FloatParam,
    #[id = "ts_tension"]
    pub tension: FloatParam,
    #[id = "ts_smooth"]
    pub smooth: FloatParam,
    #[id = "ts_aclick"]
    pub anti_click: EnumParam<AntiClick>,
    #[id = "ts_mix"]
    pub mix: FloatParam,
}

impl Default for TimeParams {
    fn default() -> Self {
        Self {
            pattern: IntParam::new("Time pattern", 1, IntRange::Linear { min: 1, max: 12 }),
            sync: EnumParam::new("Time sync", TimeSync::Bar),
            trigger: EnumParam::new("Time trigger", TimeTrigger::Sync),
            pattern_quantize: EnumParam::new("Pattern quantize", PatternQuantize::Beat),
            midi_switch: BoolParam::new("MIDI switches pattern", true),
            depth: percent("Time depth", 1.0),
            phase: percent("Time phase", 0.0),
            tension: FloatParam::new("Time tension", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 })
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            smooth: percent("Time smooth", 0.0),
            anti_click: EnumParam::new("Anti-click", AntiClick::Medium),
            mix: percent("Time mix", 1.0),
        }
    }
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

#[derive(Params)]
pub struct GrainwaveParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    /// The 12 editable time-shaper patterns. GUI writes (then bumps `pattern_version`),
    /// audio thread copies with `try_read` when the version changes. See `pattern.rs`.
    #[persist = "time-patterns"]
    pub time_patterns: Arc<std::sync::RwLock<crate::pattern::PatternBank>>,
    /// Not persisted. Bumped by whoever writes `time_patterns`.
    pub pattern_version: Arc<std::sync::atomic::AtomicU32>,

    #[nested(group = "Sections")]
    pub sections: SectionParams,
    #[nested(group = "Time")]
    pub time: TimeParams,

    #[nested(group = "Input")]
    pub input: InputParams,
    #[nested(group = "Grains")]
    pub grain: GrainParams,
    #[nested(group = "Playback")]
    pub playback: PlaybackParams,
    #[nested(group = "Rhythm")]
    pub rhythm: RhythmParams,
    #[nested(group = "Degrade")]
    pub degrade: DegradeParams,
    #[nested(group = "Filter")]
    pub filter: FilterParams,
    #[nested(group = "Space")]
    pub space: SpaceParams,
    #[nested(group = "Modulation")]
    pub modulation: ModulationParams,
    #[nested(group = "Macros")]
    pub macros: MacroParams,
    #[nested(group = "Output")]
    pub output: OutputParams,
}

impl Default for GrainwaveParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(960, 640),
            time_patterns: Arc::new(std::sync::RwLock::new(crate::pattern::PatternBank::default())),
            pattern_version: Arc::new(std::sync::atomic::AtomicU32::new(1)),
            sections: Default::default(),
            time: Default::default(),
            input: Default::default(),
            grain: Default::default(),
            playback: Default::default(),
            rhythm: Default::default(),
            degrade: Default::default(),
            filter: Default::default(),
            space: Default::default(),
            modulation: Default::default(),
            macros: Default::default(),
            output: Default::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot: plain values read once per block. DSP modules read ONLY this.
// The modulation module mutates it (LFOs, envelope follower, macros) before DSP runs.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    // input
    pub input_gain: f32,
    pub channel_mode: ChannelMode,
    pub buffer_mode: BufferMode,
    pub buffer_bars: i32,
    pub buffer_seconds: f32,
    pub freeze: bool,
    // grain
    pub grain_size_ms: f32,
    pub density_mode: DensityMode,
    pub density_hz: f32,
    pub density_division: Division,
    pub position: f32,
    pub position_jitter: f32,
    pub memory: f32,
    pub envelope: GrainEnvelope,
    pub max_voices: i32,
    pub spread: f32,
    pub low_cpu: bool,
    // playback
    pub reverse_chance: f32,
    pub pitch_st: f32,
    pub pitch_jitter_st: f32,
    pub pitch_quantize: PitchQuantize,
    pub tape_stop_chance: f32,
    pub tape_start_chance: f32,
    pub speed: f32,
    pub midi_pitch: bool,
    // rhythm
    pub stutter_division: Division,
    pub stutter_chance: f32,
    pub swing: f32,
    pub gate_enabled: bool,
    pub gate_steps: [bool; 16],
    pub rewind: bool,
    pub tape_stop: bool,
    pub tape_stop_time_ms: f32,
    // degrade
    pub degrade_order: DegradeOrder,
    pub bit_depth: f32,
    pub sample_rate_hz: f32,
    pub wow_rate_hz: f32,
    pub wow_depth: f32,
    pub flutter_rate_hz: f32,
    pub flutter_depth: f32,
    pub saturation_mode: SaturationMode,
    pub drive: f32,
    pub tape_age: f32,
    pub head_bump_db: f32,
    pub hiss: f32,
    pub crackle_density: f32,
    pub crackle_level: f32,
    pub dropout_chance: f32,
    pub dropout_depth: f32,
    pub oversampling: Oversampling,
    // filter
    pub lowpass_hz: f32,
    pub lowpass_res: f32,
    pub highpass_hz: f32,
    pub highpass_res: f32,
    pub filter_drift: f32,
    // space
    pub reverb_size: f32,
    pub reverb_mix: f32,
    pub delay_division: Division,
    pub delay_feedback: f32,
    pub delay_mix: f32,
    pub delay_crush: f32,
    // macros (raw values; modulation module applies them)
    pub dust: f32,
    pub chaos: f32,
    pub age: f32,
    // output
    pub mix: f32,
    pub output_gain: f32,
    pub limiter: bool,
    // modulation (sources and routing; read only by dsp::modulation)
    pub lfo_shape: [LfoShape; 2],
    pub lfo_rate_mode: [LfoRate; 2],
    pub lfo_rate_hz: [f32; 2],
    pub lfo_target: [ModTarget; 2],
    pub lfo_amount: [f32; 2],
    pub env_target: ModTarget,
    pub env_amount: f32,
    pub env_attack_ms: f32,
    pub env_release_ms: f32,
    // sections (read by lib.rs stage crossfades)
    pub on_time: bool,
    pub on_grains: bool,
    pub grain_amount: f32,
    pub on_rhythm: bool,
    pub on_degrade: bool,
    pub on_filter: bool,
    pub on_space: bool,
    pub on_modulation: bool,
    // time shaper
    pub ts_pattern: i32,
    pub ts_sync: TimeSync,
    pub ts_trigger: TimeTrigger,
    pub ts_pattern_quantize: PatternQuantize,
    pub ts_midi_switch: bool,
    pub ts_depth: f32,
    pub ts_phase: f32,
    pub ts_tension: f32,
    pub ts_smooth: f32,
    pub ts_anti_click: AntiClick,
    pub ts_mix: f32,
}

impl Snapshot {
    pub fn from_params(p: &GrainwaveParams) -> Self {
        let mut gate_steps = [true; 16];
        for (dst, step) in gate_steps.iter_mut().zip(p.rhythm.gate_steps.iter()) {
            *dst = step.on.value();
        }
        Self {
            input_gain: p.input.input_gain.value(),
            channel_mode: p.input.channel_mode.value(),
            buffer_mode: p.input.buffer_mode.value(),
            buffer_bars: p.input.buffer_bars.value(),
            buffer_seconds: p.input.buffer_seconds.value(),
            freeze: p.input.freeze.value(),

            grain_size_ms: p.grain.size_ms.value(),
            density_mode: p.grain.density_mode.value(),
            density_hz: p.grain.density_hz.value(),
            density_division: p.grain.density_division.value(),
            position: p.grain.position.value(),
            position_jitter: p.grain.position_jitter.value(),
            memory: p.grain.memory.value(),
            envelope: p.grain.envelope.value(),
            max_voices: p.grain.max_voices.value(),
            spread: p.grain.spread.value(),
            low_cpu: p.grain.low_cpu.value(),

            reverse_chance: p.playback.reverse_chance.value(),
            pitch_st: p.playback.pitch_st.value(),
            pitch_jitter_st: p.playback.pitch_jitter_st.value(),
            pitch_quantize: p.playback.pitch_quantize.value(),
            tape_stop_chance: p.playback.tape_stop_chance.value(),
            tape_start_chance: p.playback.tape_start_chance.value(),
            speed: p.playback.speed.value(),
            midi_pitch: p.playback.midi_pitch.value(),

            stutter_division: p.rhythm.stutter_division.value(),
            stutter_chance: p.rhythm.stutter_chance.value(),
            swing: p.rhythm.swing.value(),
            gate_enabled: p.rhythm.gate_enabled.value(),
            gate_steps,
            rewind: p.rhythm.rewind.value(),
            tape_stop: p.rhythm.tape_stop.value(),
            tape_stop_time_ms: p.rhythm.tape_stop_time_ms.value(),

            degrade_order: p.degrade.order.value(),
            bit_depth: p.degrade.bit_depth.value(),
            sample_rate_hz: p.degrade.sample_rate_hz.value(),
            wow_rate_hz: p.degrade.wow_rate_hz.value(),
            wow_depth: p.degrade.wow_depth.value(),
            flutter_rate_hz: p.degrade.flutter_rate_hz.value(),
            flutter_depth: p.degrade.flutter_depth.value(),
            saturation_mode: p.degrade.saturation_mode.value(),
            drive: p.degrade.drive.value(),
            tape_age: p.degrade.tape_age.value(),
            head_bump_db: p.degrade.head_bump_db.value(),
            hiss: p.degrade.hiss.value(),
            crackle_density: p.degrade.crackle_density.value(),
            crackle_level: p.degrade.crackle_level.value(),
            dropout_chance: p.degrade.dropout_chance.value(),
            dropout_depth: p.degrade.dropout_depth.value(),
            oversampling: p.degrade.oversampling.value(),

            lowpass_hz: p.filter.lowpass_hz.value(),
            lowpass_res: p.filter.lowpass_res.value(),
            highpass_hz: p.filter.highpass_hz.value(),
            highpass_res: p.filter.highpass_res.value(),
            filter_drift: p.filter.drift.value(),

            reverb_size: p.space.reverb_size.value(),
            reverb_mix: p.space.reverb_mix.value(),
            delay_division: p.space.delay_division.value(),
            delay_feedback: p.space.delay_feedback.value(),
            delay_mix: p.space.delay_mix.value(),
            delay_crush: p.space.delay_crush.value(),

            dust: p.macros.dust.value(),
            chaos: p.macros.chaos.value(),
            age: p.macros.age.value(),

            mix: p.output.mix.value(),
            output_gain: p.output.output_gain.value(),
            limiter: p.output.limiter.value(),

            lfo_shape: [p.modulation.lfos[0].shape.value(), p.modulation.lfos[1].shape.value()],
            lfo_rate_mode: [
                p.modulation.lfos[0].rate_mode.value(),
                p.modulation.lfos[1].rate_mode.value(),
            ],
            lfo_rate_hz: [p.modulation.lfos[0].rate_hz.value(), p.modulation.lfos[1].rate_hz.value()],
            lfo_target: [p.modulation.lfos[0].target.value(), p.modulation.lfos[1].target.value()],
            lfo_amount: [p.modulation.lfos[0].amount.value(), p.modulation.lfos[1].amount.value()],
            env_target: p.modulation.env_target.value(),
            env_amount: p.modulation.env_amount.value(),
            env_attack_ms: p.modulation.env_attack_ms.value(),
            env_release_ms: p.modulation.env_release_ms.value(),

            on_time: p.sections.time.value(),
            on_grains: p.sections.grains.value(),
            grain_amount: p.sections.grain_amount.value(),
            on_rhythm: p.sections.rhythm.value(),
            on_degrade: p.sections.degrade.value(),
            on_filter: p.sections.filter.value(),
            on_space: p.sections.space.value(),
            on_modulation: p.sections.modulation.value(),

            ts_pattern: p.time.pattern.value(),
            ts_sync: p.time.sync.value(),
            ts_trigger: p.time.trigger.value(),
            ts_pattern_quantize: p.time.pattern_quantize.value(),
            ts_midi_switch: p.time.midi_switch.value(),
            ts_depth: p.time.depth.value(),
            ts_phase: p.time.phase.value(),
            ts_tension: p.time.tension.value(),
            ts_smooth: p.time.smooth.value(),
            ts_anti_click: p.time.anti_click.value(),
            ts_mix: p.time.mix.value(),
        }
    }
}
