# Degrade chain and filter (`src/dsp/degrade/`, `src/dsp/filter.rs`)

Branch: `feat/degrade` (merged). No shared-file edits.

## Layout

`src/dsp/degrade.rs` became a directory:

| File | Contents |
|---|---|
| `degrade/mod.rs` | `Degrade`, stage ordering, bitcrush, sample-and-hold |
| `degrade/oversample.rs` | Polyphase halfband up/downsampler |
| `degrade/saturation.rs` | Tape, tube, hard clip |
| `degrade/wow.rs` | Wow/flutter modulated delay |
| `degrade/tone.rs` | Tape age low-pass, head bump EQ |
| `degrade/noise.rs` | Hiss, crackle, dropouts |
| `degrade/util.rs` | Shared helpers (`pub(crate)`, also used by the filter) |
| `degrade/tests.rs` | Unit tests |

Each stage processes the whole block in place, in one of the three README orders.

## Stages

| Stage | Implementation |
|---|---|
| Bitcrush | `q = 2^(bits − 1)`, round; fractional bits; bypass at 16 |
| Sample rate | Sample-and-hold, phase accumulator shared by both channels; captures on the first sample after enabling; bypass at ≥ host rate **or** at the 48 kHz parameter max |
| Wow/flutter | 5 ms base delay. Wow = 0.6 × sine + 0.4 × cosine-interpolated random, up to ±3 ms. Flutter = sine, up to ±0.3 ms. Catmull-Rom read; both channels share modulation |
| Saturation | Gain `1 + 19 × drive`, output × `1 / sqrt(gain)`. See curves below |
| Oversampling | 47-tap Kaiser halfband (β 7), polyphase, cascaded for 4×; fixed-size state arrays |
| Tape age | TPT SVF low-pass, Q 0.707, cutoff `20 kHz × 0.125^age`, max 0.45 × sr |
| Head bump | RBJ peaking EQ, 90 Hz, Q 1, 0–9 dB |
| Hiss | Kellet economy pink filter, per channel, RMS −90 dB → −30 dB, off at 0 |
| Crackle | Up to 30 events/s × density; 0.5 ms decaying noise clicks × level with random channel balance; ~4 % pops (×2.5, 3 ms, low-passed) |
| Dropouts | Check every 50 ms; dip to `1 − depth` for 50–300 ms with 20 ms linear ramps |

### Saturation curves

- **Tape:** `tanh(x)` for x ≥ 0, `1.25 × tanh(0.8x)` below. No DC blocker.
- **Tube:** input soft-limited to `2.5 × tanh(0.4x)`, then `x + 0.2x²`, then `tanh`, then 10 Hz DC
  blocker. The soft limit stops the quadratic folding back past its turning point at −2.5.
- **Hard clip:** clamp ±1.

## Filter

- TPT SVF low-pass, then high-pass (12 dB/oct each).
- Resonance → Q 0.707–10, exponential.
- Drift: random walk stepping every 1/0.3 s (step 0.7, reflected at ±1), smoothed by two one-poles,
  scaled to ±1 octave × drift.
- Cutoffs smoothed in log2 and clamped to 20 Hz – 0.45 × sr.

## Click-free switching

- Saturation, wow/flutter, tape age, low-pass and high-pass fade in/out over 10–20 ms instead of
  switching instantly (the wow delay and oversampler latency would otherwise click). Wow briefly
  combs while it fades between dry and the 5 ms-delayed signal.
- Changing saturation mode or oversampling: fade to dry over 10 ms, swap, fade back.
- A filter at its neutral setting (low-pass 20 kHz / high-pass 20 Hz, resonance 0, drift 0) is
  crossfaded out, so the default chain is bit-transparent.

## Real-time safety

- Only heap buffer: wow delay line, allocated in `reset()` (`next_pow2(8.3 ms × sr + 8)`).
- Every parameter clamped; NaN/inf map to the minimum.
- Drive, depths, levels, tape age, head bump, cutoffs and resonance are smoothed.
- Each stage has its own fixed-seed RNG, so output is block-size independent.
- Denormals flushed in filter states.

## Differences from the original spec

1. Stage bypass uses crossfades (above).
2. Saturation mode / oversampling changes fade through dry.
3. Neutral filters are crossfaded out.
4. Sample-rate reduction also bypasses at its 48 kHz maximum, so a 96 kHz host is untouched at
   default settings.
5. Tube input soft limit; tape curve specifics (README said only "asymmetric tanh").
6. Resonance curve, drift step and crackle pop details were unspecified.

## Limitations

- **Oversampling latency is not reported:** about 23 samples at 2× (≈0.5 ms at 48 kHz), a bit more
  at 4×. Dry/wet mixing with the dry path therefore combs slightly when oversampling is on and mix
  is below 100 %.
- Changing `degrade_order` mid-stream re-routes instantly and can click.
- Halfband transition band ≈ ±4.5 kHz around the original Nyquist: small residual aliasing and
  slight passband droop above ~19 kHz at 44.1/48 kHz.
- Pink filter tuned for 44.1 kHz; hiss tilt shifts slightly at other rates.
- Filter recomputes coefficients every sample (`tan`, `powf`); could be cached.

## Tests

- **Degrade:**
  - `defaults_are_neutral`
  - `neutral_is_transparent_at_every_rate_and_order`
  - `silence_in_silence_out_without_noise`
  - `finite_under_randomized_extreme_params`
  - `bitcrush_quantizes_to_expected_levels`
  - `bitcrush_bypasses_at_16_bits`
  - `sample_rate_reduction_holds_samples`
  - `hard_clip_output_is_compensated`
  - `saturation_modes_add_harmonics_and_stay_bounded`
  - `tube_output_is_dc_blocked`
  - `oversampling_reduces_aliasing`
  - `wow_flutter_modulates_but_keeps_level`
  - `wow_with_no_modulation_is_a_5ms_delay`
  - `tape_age_rolls_off_highs`
  - `head_bump_boosts_90hz`
  - `hiss_level_tracks_parameter`
  - `crackle_produces_sparse_clicks`
  - `dropouts_dip_to_depth`
  - `block_size_independent`
  - `switching_modes_does_not_click`
- **Oversampler:** `halfband_dc_gain_is_unity`, `passband_is_flat`, `identity_round_trip_is_delayed_copy`
- **Filter:**
  - `neutral_is_transparent`
  - `lowpass_attenuates_above_cutoff`
  - `highpass_attenuates_below_cutoff`
  - `resonance_peaks_at_cutoff`
  - `stable_at_max_resonance`
  - `finite_with_extreme_params`
  - `silence_in_silence_out`
  - `drift_stays_within_one_octave`
  - `block_size_independent`
