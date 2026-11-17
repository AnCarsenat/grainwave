# Modulation (`src/dsp/modulation.rs`)

Branch: `feat/modulation` (merged). Runs once per block, before any DSP, and rewrites `Snapshot`.

## Order of operations

1. **Macros:** Dust, then Age, then Chaos. A macro at 0 is skipped entirely, so values stay bit-exact.
2. **LFO 1 and LFO 2:** value sampled at the block start.
3. **Envelope follower:** runs per sample over the block, end-of-block value used.
4. **Routing:** offsets summed per target, then applied and clamped once.
5. **Visuals:** LFO values to `VisualState::lfo_values[0..2]`, envelope to `env_value`.

## Macros

Dust and Age use `v = lerp(v, target, amount)`. Sample-rate and low-pass targets interpolate in
log space.

| Macro | Targets |
|---|---|
| Dust | bit depth 6, sample rate 8 kHz (log), drive 0.6, tape age 0.7, hiss 0.35, crackle density 0.5, wow 0.35, flutter 0.25, low-pass 3 kHz (log), dropout chance 0.15 |
| Age | wow 0.6, flutter 0.4, hiss 0.4, tape age 0.8 |

Chaos (`c`):
- Position jitter, reverse, stutter, tape stop, tape start, dropout chance, filter drift:
  `clamp(v × (1 + 2c) + 0.15c, 0, 1)`
- Pitch jitter: `clamp(v × (1 + 2c) + 2c, 0, 12)` semitones

## LFOs

| Shape | Output (phase `p` in 0..1) |
|---|---|
| Sine | `sin(2πp)` |
| Triangle | starts at 0, rises (in phase with sine) |
| Saw | `2p − 1` |
| Square | +1 for the first half, −1 for the second |
| Random | new `Rng::bipolar` value each cycle |
| Smooth random | cosine interpolation from previous to new value across the cycle |

- **Free mode:** phase advances by `hz × block_len / sr`. A phase wrap starts a new random cycle.
- **Synced mode:** `phase = frac(beat_pos / cycle_beats)`. Cycle lengths in beats:
  1/1 = 4, 1/2 = 2, 1/4 = 1, 1/8 = 0.5, 1/16 = 0.25, 2 bars = 8, 4 bars = 16 (4/4 assumed).
  A new random value is drawn when `floor(beat_pos / cycle_beats)` changes.
- Fixed seeds: `0x4C464F31` (LFO 1), `0x4C464F32` (LFO 2). `reset()` re-seeds and zeroes phase.

## Envelope follower

- Input: `max(|l|, |r|)` per sample, capped at 1, NaN/inf treated as 0.
- Reads the signal after input gain, before channel encoding.
- One-pole attack/release: `coef = 1 − exp(−1 / (ms × 0.001 × sr))`.
- Output range 0..1.

## Routing

Each source adds `source × amount` to an offset array indexed by `ModTarget`. Each target with a
non-zero total offset is applied once:

- **Log targets:** `v × exp(offset × ln(max / min))`
- **Linear targets:** `v + offset × (max − min)`
- Result clamped to range; non-finite results guarded.

| Scale | Targets and ranges |
|---|---|
| Log | grain size 5–500 ms, density 1–64 Hz, speed 0.25–4×, sample rate 1–48 kHz, low-pass and high-pass 20 Hz–20 kHz |
| Linear | pitch −24..24 st, pitch jitter 0..12 st, bit depth 1..16 |
| Linear 0..1 | position, position jitter, reverse, spread, stutter, wow, flutter, drive, tape age, reverb mix, delay mix, mix |

The table lives in `target_range()` and `target_field()`. Keep it in sync with `params.rs` when
ranges change.

## Snapshot fields added

Appended to `Snapshot` (read only by this module):

```rust
pub lfo_shape: [LfoShape; 2],
pub lfo_rate_mode: [LfoRate; 2],
pub lfo_rate_hz: [f32; 2],
pub lfo_target: [ModTarget; 2],
pub lfo_amount: [f32; 2],
pub env_target: ModTarget,
pub env_amount: f32,
pub env_attack_ms: f32,
pub env_release_ms: f32,
```

## Differences from the original spec

- Offsets from several sources on one target are summed and clamped once, so source order does
  not matter.
- Pitch jitter after Chaos is clamped to its 0..12 range.
- Grain size, density and speed are modulated in log space, in addition to Hz and cutoff targets.
- Smooth random reaches each new value at the end of its cycle.
- Macro amounts are clamped to 0..1 and source amounts to −1..1 before use.

## Limitations

- Block-rate modulation: fast LFOs step on large host buffers. DSP modules smooth the values.
- A synced LFO freezes if the host stops advancing `beat_pos`.
- One target per source.

## Tests

`target_table_covers_every_target`, `neutral_settings_leave_snapshot_unchanged`,
`dust_full_hits_targets`, `dust_half_is_log_interpolated_for_hz`, `age_full_hits_targets`,
`chaos_formula`, `lfo_stays_in_range_for_all_shapes_and_modes`,
`synced_lfo_phase_matches_beat_pos`, `free_lfo_advances_by_block_len`,
`random_lfo_changes_once_per_cycle`, `modulated_values_always_clamped` (4000 random blocks),
`cutoff_modulated_in_log_space`, `envelope_follower_rises_and_falls`.
