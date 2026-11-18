# Grain engine and input (`src/dsp/grains.rs`, `src/dsp/input.rs`)

Branch: `feat/grain-engine` (merged). No shared-file edits. New public items:
`grains::quantize_semitones` and `grains::MAX_VOICES`.

## Channel modes (`input.rs`)

| Mode | Encode | Decode |
|---|---|---|
| Stereo | no-op | no-op |
| Mono | `l = r = (l + r) / 2` | no-op |
| Mid/side | `l = (l + r) / 2` (M), `r = (l − r) / 2` (S) | `L = M + S`, `R = M − S` |

Mode switches are not crossfaded.

## Capture buffer

- Stereo circular buffer allocated in `reset()`: **32 s + 1 s guard + 8 samples**. About 13 MB at
  48 kHz, 50 MB at 192 kHz.
- Positions are absolute sample indices (`f64`) stored at `index % capacity`; writing always wraps
  over the full capacity.
- Effective length `L` (seconds in free mode, `bars × 4 × samples_per_beat` in synced mode,
  clamped to 32 s) only limits how far back grains reach and how the waveform is drawn. Changing
  `L` never creates a discontinuity.
- Freeze stops writes. Grains keep reading relative to the frozen write head.

## Scheduler

- **Free:** countdown of `sr / density_hz` with ±10 % × `position_jitter` timing jitter. The
  countdown is capped at 1.1 × the interval, so raising density takes effect immediately.
- **Synced:** one spawn each time `floor(beat / division.beats())` changes, beat computed per
  sample from `Transport::beat_pos`.

## Read position

1. `max_lookback = L × (0.1 + 0.9 × memory)`;
   `lookback = position × max_lookback ± position_jitter × max_lookback / 2`, clamped to
   `[0, max_lookback]`.
2. Raised to an analytic minimum for the grain's rate curve, so a forward grain never catches the
   write head and a reverse grain starts behind it.
3. Capped by recorded history, `L`, and remaining capacity.
4. Per-sample guard: reads clamped to ≤ write head − 3 and to data not yet overwritten. If a span
   is longer than history allows, the read holds at the write head rather than skipping.

## Rate and playback modes

- `rate = speed × 2^(st / 12)`, `st = pitch + uniform(±pitch_jitter) + midi_offset`, then quantized.
  Semitones clamped to ±48, rate to [1/1024, 16].
- **Quantize degrees** (relative to 0, next octave's root is a candidate):

  | Mode | Degrees |
  |---|---|
  | Octaves | 0 |
  | Fifths | 0, 7 |
  | Major | 0, 2, 4, 5, 7, 9, 11 |
  | Minor (natural) | 0, 2, 3, 5, 7, 8, 10 |
  | Pentatonic (major) | 0, 2, 4, 7, 9 |

- **Reverse:** reads backwards from the end of the grain's span.
- **Tape stop:** `rate × (1 − t)²`. **Tape start:** `rate × t²`. Position uses the exact integral
  of the curve (no drift). Tape stop is rolled first; tape start only if that missed, so the real
  tape-start probability is `(1 − tape_stop_chance) × tape_start_chance`.
- **Interpolation:** 4-point cubic Hermite.

## Envelopes

| Shape | Definition |
|---|---|
| Hann | standard raised cosine |
| Triangle | linear up, linear down |
| Square | flat with 2 ms linear edges |
| Exp decay | 5 ms linear attack, then `exp(−5x)` rescaled to hit exactly 0 at the end |

Envelope shape and pan are fixed at spawn, so changing them mid-grain can't click.

## Voices, spread, loudness

- Fixed pool of 40: up to 32 playing voices plus 8 slots for voices fading out after a steal.
  Active limit `max_voices` (1–32), or `min(max_voices, 8)` with Low CPU.
- **Stealing:** at the limit, the oldest voice fades out over 2 ms and the new grain takes a free
  slot. Only if all 40 slots are busy (more than 8 voices fading at once, e.g. right after
  `max_voices` drops a lot) is the oldest voice cut hard.
- **Spread:** `pan = spread × uniform(−1, 1)`, equal-power, scaled by √2 so a centered grain is at
  unity.
- **Loudness:** output × `1 / sqrt(max(1, density × size_s))`, 20 ms one-pole smoothed. Synced
  mode uses the equivalent Hz.
- The engine output fully replaces its input; dry is mixed back in `lib.rs`.

## MIDI pitch

- Held notes kept in a fixed-size stack; the most recent held note sets `offset = note − 60`.
- Mode on: grains spawn only while a note is held (silence after current grains end otherwise).
- Mode off: MIDI ignored for pitch and gating, but notes are still tracked so turning the mode on
  works immediately.
- Note-on with velocity 0 counts as note-off.

## Visual state

- Per block: voice positions, active and reverse bitmasks, write head.
- Buffer drawn as a loop of length `L`: `position = (index mod L) / L`.
- One of the 256 waveform slices refreshed per block (peak of |L|, |R| at that loop offset).

## Differences from the original spec

1. Capacity includes a 1 s guard so a grain reading the far end of a full 32 s window never hits
   overwritten audio.
2. Voice stealing fades over 2 ms instead of cutting.
3. Grains that would outrun history hold at the write head.
4. Low CPU caps voices at `min(max_voices, 8)`.

## Limitations

- Unfreezing leaves a seam between old and new audio; a grain crossing it may click.
- Lowering `max_voices` releases extra voices only at the next spawn.
- Visual positions update once per block.

## Tests

- **`grains.rs`:**
  - `silence_in_silence_out`
  - `produces_output_from_signal`
  - `finite_under_extreme_random_params`: 600 blocks, NaN/inf, random sizes and BPM, MIDI, freeze, all enums
  - `reverse_grains_read_backwards`
  - `forward_grains_never_pass_write_head`
  - `quantize_snapping`
  - `buffer_length`
  - `block_size_independent`: 1, 64, 512
  - `midi_gates_spawning_and_sets_pitch`
  - `voice_limit_and_stealing`
  - `full_voice_limit_never_hard_cuts`
  - `envelopes_are_bounded_and_close`
  - `waveform_fills_over_blocks`
- **`input.rs`:** `mid_side_round_trip`, `stereo_is_transparent`, `mono_sums`
