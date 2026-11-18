use super::*;
use crate::dsp::rng::Rng;
use crate::params::{GrainwaveParams, Oversampling, SaturationMode};

const SR: f32 = 48_000.0;
const ORDERS: [DegradeOrder; 3] =
    [DegradeOrder::DigitalFirst, DegradeOrder::TapeFirst, DegradeOrder::DriveFirst];
const OS: [Oversampling; 3] = [Oversampling::X1, Oversampling::X2, Oversampling::X4];
const MODES: [SaturationMode; 3] =
    [SaturationMode::Tape, SaturationMode::Tube, SaturationMode::HardClip];

fn snap() -> Snapshot {
    Snapshot::from_params(&GrainwaveParams::default())
}

fn transport(n: usize) -> Transport {
    Transport { sample_rate: SR, bpm: 120.0, beat_pos: 0.0, playing: true, block_len: n }
}

fn noise(n: usize, seed: u32, amp: f32) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..n).map(|_| rng.bipolar() * amp).collect()
}

fn sine(freq: f32, n: usize, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin())
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn run_sr(s: &Snapshot, l: &[f32], r: &[f32], block: usize, sr: f32) -> (Vec<f32>, Vec<f32>) {
    let mut d = Degrade::new();
    d.reset(sr, 512);
    let mut l = l.to_vec();
    let mut r = r.to_vec();
    for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        let n = cl.len();
        d.process(cl, cr, s, &transport(n));
    }
    (l, r)
}

fn run(s: &Snapshot, x: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
    run_sr(s, x, x, block, SR)
}

/// Gain (dB) of a sine through the chain, measured on the second half.
fn gain_db(s: &Snapshot, freq: f32) -> f32 {
    let n = 48_000;
    let x = sine(freq, n, 0.25);
    let (y, _) = run(s, &x, 256);
    20.0 * (rms(&y[n / 2..]) / rms(&x[n / 2..])).log10()
}

/// Every stage active with heavy, non-noise settings.
fn heavy(order: DegradeOrder, os: Oversampling, mode: SaturationMode) -> Snapshot {
    let mut s = snap();
    s.degrade_order = order;
    s.oversampling = os;
    s.saturation_mode = mode;
    s.bit_depth = 4.5;
    s.sample_rate_hz = 8000.0;
    s.wow_depth = 1.0;
    s.wow_rate_hz = 4.0;
    s.flutter_depth = 1.0;
    s.flutter_rate_hz = 30.0;
    s.drive = 1.0;
    s.tape_age = 1.0;
    s.head_bump_db = 9.0;
    s.dropout_chance = 1.0;
    s.dropout_depth = 1.0;
    s
}

#[test]
fn defaults_are_neutral() {
    let s = snap();
    assert_eq!(s.bit_depth, 16.0);
    assert_eq!(s.sample_rate_hz, 48_000.0);
    assert_eq!(s.drive, 0.0);
    assert_eq!(s.hiss, 0.0);
    assert_eq!(s.crackle_density, 0.0);
    assert_eq!(s.dropout_chance, 0.0);
}

#[test]
fn neutral_is_transparent_at_every_rate_and_order() {
    let x = noise(20_000, 1, 0.9);
    let y = noise(20_000, 2, 0.9);
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for order in ORDERS {
            for os in OS {
                let mut s = snap();
                s.degrade_order = order;
                s.oversampling = os;
                let (l, r) = run_sr(&s, &x, &y, 128, sr);
                assert_eq!(l, x, "{sr} {order:?} {os:?}");
                assert_eq!(r, y);
            }
        }
    }
}

#[test]
fn silence_in_silence_out_without_noise() {
    let zero = vec![0.0; 24_000];
    for order in ORDERS {
        for os in OS {
            for mode in MODES {
                let s = heavy(order, os, mode);
                let (l, r) = run(&s, &zero, 64);
                assert!(
                    l.iter().chain(r.iter()).all(|&v| v == 0.0),
                    "{order:?} {os:?} {mode:?}"
                );
            }
        }
    }
}

#[test]
fn finite_under_randomized_extreme_params() {
    let wild = [f32::NAN, f32::NEG_INFINITY, -10.0, 0.0, 0.3, 1.0, 1.5, 1e6];
    let mut rng = Rng::new(1234);
    for order in ORDERS {
        for os in OS {
            let mut d = Degrade::new();
            d.reset(44_100.0, 512);
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            for block in 0..300 {
                let mut s = snap();
                s.degrade_order = order;
                s.oversampling = os;
                s.saturation_mode = MODES[block % 3];
                {
                    let mut pick = |scale: f32| {
                        if rng.chance(0.3) {
                            wild[(rng.next_u32() % wild.len() as u32) as usize] * scale
                        } else {
                            rng.next_f32() * scale
                        }
                    };
                    s.bit_depth = pick(16.0);
                    s.sample_rate_hz = pick(48_000.0);
                    s.wow_rate_hz = pick(4.0);
                    s.wow_depth = pick(1.0);
                    s.flutter_rate_hz = pick(30.0);
                    s.flutter_depth = pick(1.0);
                    s.drive = pick(1.0);
                    s.tape_age = pick(1.0);
                    s.head_bump_db = pick(9.0);
                    s.hiss = pick(1.0);
                    s.crackle_density = pick(1.0);
                    s.crackle_level = pick(1.0);
                    s.dropout_chance = pick(1.0);
                    s.dropout_depth = pick(1.0);
                }
                let n = 1 + (rng.next_u32() % 512) as usize;
                for i in 0..n {
                    l[i] = rng.bipolar() * 8.0;
                    r[i] = if i % 97 == 0 { 8.0 } else { rng.bipolar() };
                }
                d.process(&mut l[..n], &mut r[..n], &s, &transport(n));
                for v in l[..n].iter().chain(r[..n].iter()) {
                    assert!(v.is_finite() && v.abs() < 100.0, "{order:?} {os:?}: {v}");
                }
            }
        }
    }
}

#[test]
fn bitcrush_quantizes_to_expected_levels() {
    let x: Vec<f32> = (0..2001).map(|i| -1.0 + i as f32 * 0.001).collect();
    for (bits, q) in [(1.0, 1.0), (3.0, 4.0), (8.0, 128.0), (2.5, 2f32.powf(1.5))] {
        let mut s = snap();
        s.bit_depth = bits;
        let (y, _) = run(&s, &x, 512);
        for (&a, &b) in x.iter().zip(y.iter()) {
            let k = b * q;
            assert!((k - k.round()).abs() < 1e-4, "bits {bits}: {b} not on grid");
            assert!((a - b).abs() <= 0.5 / q + 1e-6, "bits {bits}: error too large");
        }
    }
    // 1 bit: only -1, 0, 1.
    let mut s = snap();
    s.bit_depth = 1.0;
    let (y, _) = run(&s, &x, 512);
    assert!(y.iter().all(|&v| v == -1.0 || v == 0.0 || v == 1.0));
}

#[test]
fn bitcrush_bypasses_at_16_bits() {
    let x = noise(4096, 7, 1.0);
    let mut s = snap();
    s.bit_depth = 16.0;
    assert_eq!(run(&s, &x, 64).0, x);
    s.bit_depth = 99.0; // modulated out of range: clamps to 16
    assert_eq!(run(&s, &x, 64).0, x);
}

#[test]
fn sample_rate_reduction_holds_samples() {
    let x = noise(4800, 3, 1.0);
    let mut s = snap();
    s.sample_rate_hz = 12_000.0;
    let (y, _) = run(&s, &x, 100);
    // 48k / 12k = 4: runs of 4 identical samples taken from the input.
    for (i, chunk) in y.chunks(4).enumerate() {
        assert!(chunk.iter().all(|&v| v == chunk[0]));
        assert_eq!(chunk[0], x[i * 4]);
    }
    // At or above the host rate: bypass.
    s.sample_rate_hz = 44_100.0;
    let (l, _) = run_sr(&s, &x, &x, 64, 44_100.0);
    assert_eq!(l, x);
}

#[test]
fn hard_clip_output_is_compensated() {
    let x = sine(200.0, 48_000, 1.0);
    let mut s = snap();
    s.saturation_mode = SaturationMode::HardClip;
    s.drive = 1.0;
    let (y, _) = run(&s, &x, 256);
    let bound = 1.0 / 20f32.sqrt();
    // Square-ish at the bound once the drive has settled.
    let peak = y[24_000..].iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!((peak - bound).abs() < 1e-5, "peak {peak}, bound {bound}");
}

#[test]
fn saturation_modes_add_harmonics_and_stay_bounded() {
    for os in OS {
        for mode in MODES {
            let mut s = snap();
            s.saturation_mode = mode;
            s.oversampling = os;
            s.drive = 0.7;
            let x = sine(220.0, 24_000, 0.8);
            let (y, _) = run(&s, &x, 256);
            assert!(y.iter().all(|v| v.is_finite() && v.abs() < 1.5), "{mode:?} {os:?}");
            let diff: Vec<f32> = x.iter().zip(&y).map(|(a, b)| a - b).collect();
            assert!(rms(&diff[12_000..]) > 0.01, "{mode:?} {os:?} did nothing");
        }
    }
}

#[test]
fn tube_output_is_dc_blocked() {
    let x = sine(100.0, 96_000, 0.9);
    let mut s = snap();
    s.saturation_mode = SaturationMode::Tube;
    s.drive = 1.0;
    let (y, _) = run(&s, &x, 256);
    let tail = &y[48_000..];
    let mean = tail.iter().sum::<f32>() / tail.len() as f32;
    assert!(mean.abs() < 1e-3, "DC {mean}");
}

#[test]
fn oversampling_reduces_aliasing() {
    // Hard-clipped 9 kHz sine: the 5th harmonic (45 kHz) aliases to 3 kHz at 1x.
    // Measure the 3 kHz component with a single DFT bin.
    let n = 48_000;
    let x = sine(9_000.0, n, 0.9);
    let alias = |os: Oversampling| {
        let mut s = snap();
        s.saturation_mode = SaturationMode::HardClip;
        s.drive = 1.0;
        s.oversampling = os;
        let (y, _) = run(&s, &x, 256);
        let (mut re, mut im) = (0f64, 0f64);
        for (i, v) in y[n / 2..].iter().enumerate() {
            let w = 2.0 * std::f64::consts::PI * 3000.0 * i as f64 / SR as f64;
            re += *v as f64 * w.cos();
            im += *v as f64 * w.sin();
        }
        (re * re + im * im).sqrt()
    };
    let a1 = alias(Oversampling::X1);
    let a4 = alias(Oversampling::X4);
    assert!(a4 < a1 * 0.25, "1x alias {a1}, 4x alias {a4}");
}

#[test]
fn wow_flutter_modulates_but_keeps_level() {
    let x = sine(1000.0, 48_000, 0.5);
    let mut s = snap();
    s.wow_depth = 1.0;
    s.flutter_depth = 1.0;
    let (y, _) = run(&s, &x, 256);
    let ratio = rms(&y[24_000..]) / rms(&x[24_000..]);
    assert!((ratio - 1.0).abs() < 0.05, "level ratio {ratio}");
    let diff: Vec<f32> = x.iter().zip(&y).map(|(a, b)| a - b).collect();
    assert!(rms(&diff[24_000..]) > 0.05);
}

#[test]
fn wow_with_no_modulation_is_a_5ms_delay() {
    // Depth targets at 0 bypass; check the delay itself via the fade-out tail instead:
    // with a tiny flutter depth the output is the input delayed by ~5 ms (240 samples).
    let x = sine(50.0, 48_000, 0.5);
    let mut s = snap();
    s.flutter_depth = 1e-6;
    let (y, _) = run(&s, &x, 256);
    for i in 24_000..24_100 {
        assert!((y[i] - x[i - 240]).abs() < 1e-3);
    }
}

#[test]
fn tape_age_rolls_off_highs() {
    let mut s = snap();
    s.tape_age = 1.0;
    assert!(gain_db(&s, 200.0).abs() < 0.5);
    assert!(gain_db(&s, 10_000.0) < -20.0);
}

#[test]
fn head_bump_boosts_90hz() {
    let mut s = snap();
    s.head_bump_db = 9.0;
    let g = gain_db(&s, 90.0);
    assert!((g - 9.0).abs() < 0.3, "bump {g} dB");
    assert!(gain_db(&s, 5000.0).abs() < 0.2);
}

#[test]
fn hiss_level_tracks_parameter() {
    let zero = vec![0.0; 96_000];
    let mut s = snap();
    s.hiss = 1.0;
    let (y, _) = run(&s, &zero, 256);
    let db = 20.0 * rms(&y[48_000..]).log10();
    assert!((db + 30.0).abs() < 3.0, "hiss at {db} dB");
    s.hiss = 0.5;
    let (y, _) = run(&s, &zero, 256);
    let db = 20.0 * rms(&y[48_000..]).log10();
    assert!((db + 60.0).abs() < 3.0, "hiss at {db} dB");
}

#[test]
fn crackle_produces_sparse_clicks() {
    let zero = vec![0.0; 96_000];
    let mut s = snap();
    s.crackle_density = 1.0;
    s.crackle_level = 1.0;
    let (y, _) = run(&s, &zero, 256);
    let peak = y.iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.05 && peak <= 1.0, "peak {peak}");
    // Mostly silent between clicks.
    let silent = y.iter().filter(|v| **v == 0.0).count();
    assert!(silent > y.len() / 2);
}

#[test]
fn dropouts_dip_to_depth() {
    let ones = vec![1.0; 48_000 * 4];
    let mut s = snap();
    s.dropout_chance = 1.0;
    s.dropout_depth = 0.6;
    let (y, _) = run(&s, &ones, 512);
    let min = y.iter().fold(1f32, |m, v| m.min(*v));
    assert!((min - 0.4).abs() < 1e-4, "min gain {min}");
    assert!(y.iter().all(|v| (0.4 - 1e-4..=1.0).contains(v)));
    // Ramps: no step larger than one 20 ms ramp increment.
    let max_step = y.windows(2).fold(0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(max_step <= 1.0 / 960.0 + 1e-6);
}

#[test]
fn block_size_independent() {
    for order in ORDERS {
        let mut s = heavy(order, Oversampling::X2, SaturationMode::Tape);
        s.dropout_chance = 0.3;
        s.hiss = 0.6;
        s.crackle_density = 0.8;
        let x = noise(8192, 11, 0.5);
        let a = run(&s, &x, 1);
        let b = run(&s, &x, 64);
        let c = run(&s, &x, 512);
        assert_eq!(a, b, "{order:?}");
        assert_eq!(a, c, "{order:?}");
    }
}

#[test]
fn switching_modes_does_not_click() {
    // A low sine with a mid-stream saturation mode / oversampling change: the fade makes the
    // biggest sample-to-sample step stay close to the signal's own slope.
    let x = sine(100.0, 48_000, 0.3);
    let mut d = Degrade::new();
    d.reset(SR, 512);
    let mut y = x.clone();
    let mut r = x.clone();
    for (k, (cl, cr)) in y.chunks_mut(512).zip(r.chunks_mut(512)).enumerate() {
        let mut s = snap();
        s.drive = 0.5;
        s.saturation_mode = MODES[(k / 10) % 3];
        s.oversampling = OS[(k / 7) % 3];
        let n = cl.len();
        d.process(cl, cr, &s, &transport(n));
    }
    let max_step = y.windows(2).fold(0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(max_step < 0.03, "max step {max_step}");
}

#[test]
fn non_finite_input_does_not_latch() {
    // A single NaN/inf from upstream (e.g. a bypassed Time section passing host input
    // through) must not poison the filter / DC-blocker / EQ states forever.
    for order in ORDERS {
        for mode in MODES {
            for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let s = heavy(order, Oversampling::X2, mode);
                let mut x = sine(440.0, 48_000, 0.25);
                x[1000] = bad;
                let (l, r) = run(&s, &x, 256);
                for v in l[1000..].iter().chain(r[1000..].iter()) {
                    assert!(v.is_finite(), "{order:?} {mode:?} {bad}: stuck at {v}");
                }
            }
        }
    }
}

#[test]
fn starts_at_current_settings_after_reset() {
    // Tape age 1 (2.5 kHz low-pass) must dull a 12 kHz tone from the first block after a
    // reset, not sweep down from 20 kHz while fading in.
    let mut s = snap();
    s.tape_age = 1.0;
    let x = sine(12_000.0, 2400, 0.25);
    let (y, _) = run(&s, &x, 256);
    let ratio = rms(&y[48..]) / rms(&x[48..]);
    assert!(ratio < 0.1, "first 50 ms passed {ratio} of the input");
}
