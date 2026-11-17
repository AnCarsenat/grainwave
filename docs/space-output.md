# Space and output (`src/dsp/space.rs`, `src/dsp/output.rs`)

Branch: `feat/space-output` (merged).

## Space

Inside the module the **delay runs first and feeds the reverb**, so echoes get reverberated.

Shared behavior:
- Non-finite input samples are replaced with 0 before either effect, so NaN can never get stuck
  in a feedback loop. All parameters are NaN-checked and clamped.
- Values below 1e-15 are flushed to zero in every feedback path (denormals).
- **Bypass:** when an effect's mix target is 0, its input send ramps to 0 over 20 ms while the
  effect keeps running, so the tail decays naturally behind a wet gain that is also ramping to 0.
  Once everything it writes stays below 1e-9 for longer than its longest buffer, it goes idle:
  no CPU, output exactly dry. At mix 0 from the start it is bit-exact transparent.

### Reverb: 8-line FDN

| Aspect | Value |
|---|---|
| Line lengths | 23.1, 28.7, 33.3, 37.9, 42.1, 47.9, 53.3, 59.9 ms, each bumped in `reset()` until coprime with the earlier ones |
| Mixing | Householder: `v − (2/N) × sum` |
| Damping | one-pole low-pass per line, 6.5 kHz (max 0.45 × sr) |
| Decay | `T60 = 0.3 × 20^size` (0.3–6 s, exponential); per-line gain `10^(−3 × len / (sr × T60))`, ramped per block |
| Stereo | left feeds even lines, right feeds odd; two ± sign patterns for the outputs |
| Gains | input 0.5, output 0.35 (by ear) |
| Wet/dry | equal-power `sin`/`cos`, 20 ms smoothed |

Size changes only decay, not line lengths.

### Delay

- Independent left/right buffers (not ping-pong), 4 s + 4 samples, allocated in `reset()`.
- Time `division.beats() × samples_per_beat`, clamped to 1 sample–4 s, linear interpolation.
  Non-finite tempo falls back to 120 BPM.
- **Time changes:** any change over 0.5 samples starts a 50 ms **linear** crossfade from the old
  read head to a new one. A change during a fade waits for it to finish.
- **Feedback loop:** 6 kHz one-pole low-pass, then crusher. Crusher = sample-and-hold at
  `sr × (1/12)^crush`, then quantize with `q = 2^(bits − 1)`, `bits = 16 − 12 × crush`.
  Bypassed at crush 0. At full crush: 4 bits at sr/12 (4 kHz at 48 kHz).
- Feedback clamped to 0–0.95, 20 ms smoothed. Wet/dry equal-power, 20 ms smoothed.

## Output

1. NaN/inf samples become 0.
2. Output gain: 20 ms one-pole on the linear gain. NaN gain falls back to 1. The first block after
   a reset jumps straight to the target.
3. Limiter (when on): stereo-linked. Gain releases toward 1 with a 100 ms one-pole and drops
   instantly to `ceiling / peak` when a sample would exceed the ceiling. Ceiling −0.3 dBFS
   (0.9660509), never exceeded. Turning it off resets its gain to 1.
4. Final clamp to ±1, always.

## Differences from the original spec

- **Delay crossfade is linear, not equal-power.** Equal-power gains can sum to about 1.41; with
  the tempo changing every block, that pushed loop gain above 1 and the feedback blew up (peak
  around 1e30 in the stability test). Linear gains sum to 1, keeping loop gain at or below the
  feedback setting.
- The crusher's sample-rate curve (full rate to sr/12) was not in the spec. It was chosen to reach
  4 kHz where bit depth reaches 4.
- Delay-into-reverb order and the reverb's 6.5 kHz damping were not in the spec.

## Limitations

- During a continuous tempo ramp the delay crossfades repeatedly; smooth, but slightly phasey. No
  tape-style pitch bend on time changes.
- No reverb pre-delay or line modulation; long tails at high size can sound slightly metallic.
- Crusher bit-depth steps happen once per block.
- With very loud input, 0.95 feedback can reach internal levels about 20× the input (bounded).
  The output limiter catches it.
- Space runs on the channel-encoded signal (mid/side when that mode is on), per the signal flow.
- CPU cost of per-sample `sin`/`cos` wet/dry not measured yet.

## Tests

- **Space:**
  - `transparent_at_mix_zero`
  - `silence_in_silence_out`
  - `stable_at_extremes`: 60 s of loud noise with random parameters and a new tempo every block
  - `delay_time_matches_division`: within 1 sample
  - `delay_time_change_is_smooth`
  - `reverb_tail_decays`
  - `idles_after_tail_when_mix_zero`
  - `nan_input_does_not_poison_state`
- **Output:**
  - `ceiling_is_minus_0_3_db`
  - `unity_is_transparent_below_ceiling`
  - `silence_in_silence_out`
  - `limiter_keeps_peaks_below_ceiling`
  - `limiter_releases`
  - `without_limiter_clamps_to_unity`
  - `nan_and_inf_become_zero`
  - `gain_change_is_smoothed`
