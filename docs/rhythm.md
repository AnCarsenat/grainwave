# Rhythm (`src/dsp/rhythm.rs`)

Branch: `feat/rhythm` (merged). Stutter, swing, 16-step gate, rewind and tape stop.

Constructor: `Rhythm::new(visual: Arc<VisualState>)` (the only `lib.rs` change). `reset()`
allocates a 4.1 s stereo circular buffer (4 s + interpolation margin) and clears all state.

## Per-sample order

1. Check for a stutter boundary (a new slice is the step that just ended).
2. Write the live sample to the buffer.
3. Stutter mix: `live × (1 − Σg) + Σ g × slice`.
4. Rewind or tape stop, crossfaded in/out over 10 ms.
5. Gate.

## Grid

- Beat at sample `i`: `beat_pos + i / samples_per_beat`. Tempo is per block; host loops and seeks
  are picked up at the next block start.
- Any change of step index (forward or backward) is a boundary.
- Odd steps start `swing × 0.5 × step` late.
- Stutter uses the `stutter_division` grid; the gate uses a 1/16 grid.
- A 1e-7 step bias makes boundaries land on the same sample at any block size.

## Stutter

- At each boundary, roll `stutter_chance`. On success, capture the last division-length slice and
  loop it.
- **While the roll keeps succeeding, the same slice keeps looping.** It does not grab a new slice
  at every boundary. A new slice is taken (with crossfade) only when the tempo changes the slice
  length or the slice is about to be overwritten.
- Two voices give a 2 ms crossfade between slices, plus 2 ms in from and out to live audio. The
  loop point crossfades the tail into the audio just before the slice start, so the wrap is
  seamless.
- Slice length clamped to 8 ms – ¼ of the buffer.

## Gate

- 16 steps over one 4/4 bar, linear 3 ms gain ramps.
- Writes the current step to `VisualState::gate_step` as a **plain integer** (0–15), not f32 bits.
- Gain is exactly 1.0 when the gate is off.

## Momentary buttons

Edges come from consecutive Snapshots. If both buttons are held, the most recent press wins.
Switching from one to the other fades the first out over 10 ms; the second starts at the next
block.

### Rewind
- Stops writing to the buffer so the audio being rewound stays intact.
- Reads backwards with 4-point Hermite interpolation at speed `1 + 2 × min(t, 1)` (1× to 3× over 1 s).
- Fades to silence when it runs out of recorded audio (about 1.7 s of holding).
- Writing resumes after the 10 ms fade-out.

### Tape stop
- Rate `q²`, with `q` falling linearly over `tape_stop_time_ms` (50–4000 ms), so the slowdown is
  quadratic.
- Output gain `min(1, 10 × rate)` gives true silence at a stop (no held DC).
- 10 ms fade-in on press hides the 2-sample interpolation offset.
- While stopped, the read head is parked about 2 samples behind "now", so spin-up plays fresh audio.
- On release, `q` rises over half the stop time, then crossfades to live over 10 ms.
- Pressing during spin-up resumes slowdown from the current rate; releasing during slowdown spins
  up from the current rate.

## Differences from the original spec

1. Stutter keeps the same slice while the roll succeeds. Read literally, the spec captures a new
   slice at every boundary; with slice length equal to the division, that is a one-step delay and
   chance = 1 never repeats anything.
2. The buffer stops recording during rewind.
3. Tape stop has a 10 ms fade-in, and the read head follows "now" while stopped.
4. Tape-stop silence comes from the rate-linked gain.
5. `gate_step` is an integer, not f32 bits.
6. Button changes apply at block rate. Switching between rewind and tape stop takes 10 ms plus up
   to one block.

## Limitations

- Gate assumes a 4-beat bar; the host time signature is ignored.
- With the host transport stopped and `beat_pos` fixed, the grid freezes: no new stutter
  boundaries, the gate holds its step.
- Holding rewind more than about 1.7 s at 3× exhausts the buffer and goes silent.
- With swing on, step lengths are uneven, so a looping stutter slice drifts against swung
  boundaries.
- Tape-stop audio is linearly attenuated below 10 % rate.

## Tests

`passthrough_when_everything_off` (bit-exact), `silence_in_silence_out`,
`finite_under_random_params` (3000 blocks, random BPM, block size, seeks),
`gate_mutes_off_steps`, `gate_swing_delays_odd_steps`, `stutter_repeats_audio_when_chance_is_one`,
`tape_stop_reaches_silence_then_recovers`, `rewind_plays_backwards`,
`block_size_independent_without_buttons` (1, 64, 512).
