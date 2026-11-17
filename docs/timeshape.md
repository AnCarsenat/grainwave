# Time shaper (`src/dsp/timeshape.rs`, `src/pattern.rs`)

Branch: `feat/timeshape` (merged). A drawn curve per cycle sets how far behind live the playback
reads. Flat = untouched audio; slopes and steps give halftime, holds, stutters, tape stops and
reverses that stay locked to the beat.

## Curve semantics

`delay = value_at(x, tension) × depth × cycle_length`, `x` = position in the cycle (0..1).

| Curve shape | Result |
|---|---|
| y = 0 | Live audio |
| Rising, slope 1 | Playback stands still (hold) |
| Rising, slope 0.5 | Half speed |
| Rising, slope > 1 | Plays backwards |
| Hold steps rising | Stutter repeats |
| Drop | Jump forward toward live |

The time shaper can't play faster than live (that would need future audio); curves that ask for
it are clamped to the available history.

## Engine

- **Buffer:** 16 s + 16 samples stereo, allocated in `reset()`. Non-finite input stored as 0.
- **Cursor:**
  - Sync: `x = frac(beat / cycle_beats + phase)`, per sample from `beat_pos`.
  - MIDI: a note restarts `x` at `phase` on the next block; `x` runs one cycle then holds at the
    curve's end value until the next note. Before the first note: live.
- **Read:** delay clamped to recorded history. Fractional delays use 4-point cubic Hermite;
  whole-sample delays return the stored sample exactly.
- **Smooth:** one-pole on the delay, time `smooth² × 200 ms`.
- **Anti-click:** a jump = per-sample delay change differing from the previous change by more than
  4 samples. It starts a linear crossfade from the old read head (which keeps moving at its old
  rate) to the new one: Off / Low / Medium / High = 0 / 2 / 6 / 15 ms. Pattern switches, cycle
  wraps and tempo jumps all go through it.
- **Pattern switching:** a change of `ts_pat`, or a MIDI note with MIDI switch on
  (`pattern = note % 12`, so C = 1 … B = 12 in any octave), queues a switch, applied at the exact
  sample of the next Pattern quantize boundary (immediately when Off or transport stopped).
- **Pattern copy:** 12 preallocated `Vec<Point>` (capacity `MAX_POINTS`). When
  `pattern_version` changes, `try_read` refreshes them with `clear` + `extend_from_slice`; a busy
  lock retries next block. No allocation or blocking on the audio thread.
- **Mix:** equal-gain crossfade, 20 ms smoothed, exact at 0 and 1 (default is bit-exact
  pass-through).
- **Visual:** `ts_cursor`, `ts_value` (normalized delay), `ts_active` (integer), once per block.

## Factory patterns

Designed for sync 1/1 (at 1/2 they repeat twice per bar).

| # | Name | What it does |
|---|---|---|
| 1 | Off | Flat, untouched |
| 2 | Halftime | `y = x / 2`: first half of the bar stretched over the bar |
| 3 | Stutter 1/8 end | The 1/8 before the last quarter plays twice more |
| 4 | Triplet stutter 1/12 | Second half: two triplet-8th rolls on the 12-step grid (A A A, B B B) |
| 5 | Stutter roll | From beat 3, shrinking repeats: 2 × 1/8, 2 × 1/16, 4 × 1/32 |
| 6 | Tape stop | Beat 4 slows linearly to a stop |
| 7 | Reverse beat 4 | Beat 4 plays beat 3 backwards at normal speed |
| 8 | Repeat 1-1-3-3 | Beats play 1 1 3 3 |
| 9 | Stairs 1/16 | Each half bar: 4 live 16ths, then the next 16th × 4 |
| 10 | Swing drag | Each beat's first half stretched over 60 %, rest catches up |
| 11 | Beat 1 on 3 | Beats play 1 2 1 4 |
| 12 | Wobble | Two gentle S-curve swells per bar, about ±5 % speed |

## Section integration

The time shaper is the first stage after channel encode. Its power switch (`on_time`) and the FX
sequencer's Time lane crossfade it like every other section (see README, Signal flow).

## Limitations

- A new jump during a crossfade swaps heads, so up to half of that jump can step through; very
  steep tension curves can keep restarting the fade.
- Sync mode with the transport stopped: the delay freezes.
- Cycles longer than 16 s (4/1 below 60 BPM) are clamped.
- The first cycle after reset is shallower (limited by recorded history).
- MIDI notes apply per block, not sample-accurately.

## Tests

- **`timeshape.rs`:**
  - `default_is_bit_exact_passthrough`
  - `flat_full_depth_full_mix_is_passthrough`
  - `ramp_curve_holds_cycle_start`
  - `halftime_plays_half_speed`
  - `stutter_repeats_slice`
  - `quantized_switch_waits_for_grid`
  - `midi_note_switches_pattern`
  - `midi_trigger_restarts_cycle_then_holds`
  - `anti_click_bounds_jumps`
  - `finite_under_random_params`
  - `pattern_copy_follows_version`
- **`pattern.rs`:**
  - `flat_is_zero`
  - `linear_ramp`
  - `hold_steps`
  - `wraps_before_first_point`
  - `bank_has_twelve_and_first_is_flat`
  - `sanitize_fixes_garbage`
  - `factory_patterns_finite_and_in_range`
