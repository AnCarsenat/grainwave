//! Randomize with per-group locks and musically tame ranges.
//!
//! Never touched: power switches (`on_*`), section dry/wet amounts (`g_amt`, `r_amt`, `d_amt`,
//! `f_amt`, `s_amt`, `ts_mix`), the FX sequencer (`sq_on`, `sq_rate`, lanes), the time pattern
//! curves themselves, output mix/gain/limiter, freeze, rewind, tape stop, input gain, MIDI
//! pitch, Low CPU and oversampling.

use crate::dsp::rng::Rng;
use crate::params::GrainwaveParams;
use nih_plug::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Time,
    Input,
    Grains,
    Playback,
    Rhythm,
    Degrade,
    Filter,
    Space,
    Modulation,
    Macros,
}

pub const GROUP_COUNT: usize = 10;
pub const ALL_GROUPS: [Group; GROUP_COUNT] = [
    Group::Time,
    Group::Input,
    Group::Grains,
    Group::Playback,
    Group::Rhythm,
    Group::Degrade,
    Group::Filter,
    Group::Space,
    Group::Modulation,
    Group::Macros,
];

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Time => "Time",
            Group::Input => "Input",
            Group::Grains => "Grains",
            Group::Playback => "Playback",
            Group::Rhythm => "Rhythm",
            Group::Degrade => "Degrade",
            Group::Filter => "Filter",
            Group::Space => "Space",
            Group::Modulation => "Modulation",
            Group::Macros => "Macros",
        }
    }
}

/// Set a parameter to a random normalized value in `lo..hi`.
fn rnd<P: Param>(s: &ParamSetter, p: &P, rng: &mut Rng, lo: f32, hi: f32) {
    let n = lo + (hi - lo) * rng.next_f32();
    set_norm(s, p, n);
}

fn set_norm<P: Param>(s: &ParamSetter, p: &P, n: f32) {
    s.begin_set_parameter(p);
    s.set_parameter(p, p.preview_plain(n.clamp(0.0, 1.0)));
    s.end_set_parameter(p);
}

fn set_plain<P: Param>(s: &ParamSetter, p: &P, v: P::Plain) {
    s.begin_set_parameter(p);
    s.set_parameter(p, v);
    s.end_set_parameter(p);
}

fn pick<T: Copy>(rng: &mut Rng, items: &[T]) -> T {
    items[(rng.next_u32() as usize) % items.len()]
}

pub fn randomize(p: &GrainwaveParams, s: &ParamSetter, locks: &[bool; GROUP_COUNT], rng: &mut Rng) {
    for group in ALL_GROUPS {
        if locks[group as usize] {
            continue;
        }
        match group {
            Group::Time => {
                use crate::params::TimeSync;
                let t = &p.time;
                set_plain(s, &t.pattern, 1 + (rng.next_u32() % 12) as i32);
                let sync = pick(rng, &[TimeSync::Quarter, TimeSync::Half, TimeSync::Bar, TimeSync::Bar, TimeSync::TwoBars]);
                set_plain(s, &t.sync, sync);
                rnd(s, &t.depth, rng, 0.5, 1.0);
                rnd(s, &t.smooth, rng, 0.0, 0.3);
                set_plain(s, &t.tension, if rng.chance(0.6) { 0.0 } else { rng.bipolar() * 0.5 });
            }
            Group::Input => {
                let i = &p.input;
                if rng.chance(0.3) {
                    rnd(s, &i.channel_mode, rng, 0.0, 1.0);
                } else {
                    set_norm(s, &i.channel_mode, 0.0);
                }
                rnd(s, &i.buffer_mode, rng, 0.0, 1.0);
                rnd(s, &i.buffer_bars, rng, 0.0, 1.0);
                rnd(s, &i.buffer_seconds, rng, 0.0, 1.0);
            }
            Group::Grains => {
                let g = &p.grain;
                rnd(s, &g.size_ms, rng, 0.3, 0.95);
                rnd(s, &g.density_mode, rng, 0.0, 1.0);
                rnd(s, &g.density_hz, rng, 0.2, 0.8);
                rnd(s, &g.density_division, rng, 0.0, 1.0);
                rnd(s, &g.position, rng, 0.0, 0.6);
                rnd(s, &g.position_jitter, rng, 0.0, 0.7);
                rnd(s, &g.memory, rng, 0.0, 1.0);
                rnd(s, &g.envelope, rng, 0.0, 1.0);
                rnd(s, &g.max_voices, rng, 0.3, 0.8);
                rnd(s, &g.spread, rng, 0.0, 1.0);
            }
            Group::Playback => {
                let pb = &p.playback;
                rnd(s, &pb.reverse_chance, rng, 0.0, 0.6);
                let pitch = if rng.chance(0.6) { 0.0 } else { pick(rng, &[-12.0, -7.0, -5.0, 5.0, 7.0, 12.0]) };
                set_plain(s, &pb.pitch_st, pitch);
                set_plain(s, &pb.pitch_jitter_st, if rng.chance(0.5) { 0.0 } else { rng.next_f32() * 5.0 });
                rnd(s, &pb.pitch_quantize, rng, 0.0, 1.0);
                rnd(s, &pb.tape_stop_chance, rng, 0.0, 0.3);
                rnd(s, &pb.tape_start_chance, rng, 0.0, 0.2);
                set_plain(s, &pb.speed, pick(rng, &[0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 1.5, 2.0]));
            }
            Group::Rhythm => {
                let r = &p.rhythm;
                rnd(s, &r.stutter_division, rng, 0.0, 1.0);
                rnd(s, &r.stutter_chance, rng, 0.0, 0.4);
                rnd(s, &r.swing, rng, 0.0, 0.5);
                set_plain(s, &r.gate_enabled, rng.chance(0.3));
                for (k, step) in r.gate_steps.iter().enumerate() {
                    set_plain(s, &step.on, k == 0 || rng.chance(0.7));
                }
                rnd(s, &r.tape_stop_time_ms, rng, 0.0, 1.0);
            }
            Group::Degrade => {
                let d = &p.degrade;
                rnd(s, &d.order, rng, 0.0, 1.0);
                set_plain(s, &d.bit_depth, 6.0 + 10.0 * rng.next_f32());
                rnd(s, &d.sample_rate_hz, rng, 0.45, 1.0);
                rnd(s, &d.wow_rate_hz, rng, 0.0, 1.0);
                rnd(s, &d.wow_depth, rng, 0.0, 0.5);
                rnd(s, &d.flutter_rate_hz, rng, 0.0, 1.0);
                rnd(s, &d.flutter_depth, rng, 0.0, 0.3);
                rnd(s, &d.saturation_mode, rng, 0.0, 1.0);
                rnd(s, &d.drive, rng, 0.0, 0.6);
                rnd(s, &d.tape_age, rng, 0.0, 0.7);
                rnd(s, &d.head_bump_db, rng, 0.0, 0.66);
                rnd(s, &d.hiss, rng, 0.0, 0.3);
                rnd(s, &d.crackle_density, rng, 0.0, 0.5);
                rnd(s, &d.crackle_level, rng, 0.1, 0.5);
                rnd(s, &d.dropout_chance, rng, 0.0, 0.2);
                rnd(s, &d.dropout_depth, rng, 0.3, 0.9);
            }
            Group::Filter => {
                let f = &p.filter;
                rnd(s, &f.lowpass_hz, rng, 0.6, 1.0);
                rnd(s, &f.lowpass_res, rng, 0.0, 0.6);
                rnd(s, &f.highpass_hz, rng, 0.0, 0.4);
                rnd(s, &f.highpass_res, rng, 0.0, 0.5);
                rnd(s, &f.drift, rng, 0.0, 0.6);
            }
            Group::Space => {
                let sp = &p.space;
                rnd(s, &sp.reverb_size, rng, 0.0, 1.0);
                rnd(s, &sp.reverb_mix, rng, 0.0, 0.45);
                rnd(s, &sp.delay_division, rng, 0.0, 1.0);
                set_plain(s, &sp.delay_feedback, 0.7 * rng.next_f32());
                rnd(s, &sp.delay_mix, rng, 0.0, 0.35);
                rnd(s, &sp.delay_crush, rng, 0.0, 1.0);
            }
            Group::Modulation => {
                let m = &p.modulation;
                for lfo in &m.lfos {
                    rnd(s, &lfo.shape, rng, 0.0, 1.0);
                    rnd(s, &lfo.rate_mode, rng, 0.0, 1.0);
                    rnd(s, &lfo.rate_hz, rng, 0.1, 0.8);
                    if rng.chance(0.5) {
                        set_norm(s, &lfo.target, 0.0);
                    } else {
                        rnd(s, &lfo.target, rng, 0.0, 1.0);
                    }
                    rnd(s, &lfo.amount, rng, 0.25, 0.75);
                }
                if rng.chance(0.5) {
                    set_norm(s, &m.env_target, 0.0);
                } else {
                    rnd(s, &m.env_target, rng, 0.0, 1.0);
                }
                rnd(s, &m.env_amount, rng, 0.25, 0.75);
                rnd(s, &m.env_attack_ms, rng, 0.0, 1.0);
                rnd(s, &m.env_release_ms, rng, 0.0, 1.0);
            }
            Group::Macros => {
                let mc = &p.macros;
                rnd(s, &mc.dust, rng, 0.0, 0.5);
                rnd(s, &mc.chaos, rng, 0.0, 0.35);
                rnd(s, &mc.age, rng, 0.0, 0.5);
            }
        }
    }
}
