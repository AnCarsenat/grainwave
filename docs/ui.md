# Editor (`src/editor/`)

Branch: `feat/ui-v2` (merged; replaces the v1 editor). egui 0.31 via `nih_plug_egui`.

> Layout is verified by headless egui tests (every widget on screen, no overlaps, ≥ 12 px at
> 820 × 560, 960 × 640 and 1400 × 900), not yet by eye in a host.

## Files

| File | Contents |
|---|---|
| `mod.rs` | Editor creation, state, page switching |
| `theme.rs` | Colors, section colors |
| `widgets.rs` | Knob, vertical fader, power LED |
| `curve.rs` | Time-shaper curve editor and pattern buttons |
| `sequencer.rs` | FX sequencer grid |
| `simple.rs` | Simple page |
| `advanced.rs` | Advanced page and tabs |
| `randomize.rs` | Randomize with lock groups |
| `screenshot.rs` | Test-only offscreen renderer for `docs/screenshots/` (`cargo test --release --lib screenshots -- --ignored`) |

## Window

Default 960 × 640, minimum 820 × 560. Every region gets a rect from fixed budgets; widgets draw
only inside their rect. Repaints about every 33 ms.

**Top bar (46 px):** name and status, preset browser (`<`, dropdown, `>`, "Save…" menu with name
field, Save, Rescan), Randomize with a locks menu, Simple/Advanced toggle, and right-aligned Mix,
Output and Limiter.

## Simple page

| Window | Content | Time shaper | Units row | Sequencer |
|---|---|---|---|---|
| 960 × 640 | 572 | 300 (canvas 238) | 128 | 132 |
| 820 × 560 | 492 | 220 (canvas 158) | 128 | 132 |

Below the minimum size, the time shaper and sequencer fall back to two tabs.

1. **Time shaper:** curve canvas, 12 pattern buttons, Sync, Depth, Smooth, Mix, Trigger, Quantize.
2. **Units row:** seven separated "pedals":

   | Unit | Dry/wet fader | Knobs |
   |---|---|---|
   | Time | `ts_mix` | Depth |
   | Grains | `g_amt` | Size |
   | Rhythm | `r_amt` | Stutter chance |
   | Degrade | `d_amt` | Dust |
   | Filter | `f_amt` | Low-pass |
   | Space | `s_amt` | Reverb mix |
   | Mod | none | Chaos, plus two LFO meters |

   Each unit has a power LED (`on_*`) and its section color. Units dim from `section_gain`, so
   they pulse when the sequencer gates them.
3. **FX sequencer** (below).

## Advanced page

Tabs: Time, Grains (+ Playback + Input), Rhythm, Degrade, Filter, Space, Modulation, Sequencer.
Each tab has a header with the section's power switch, dry/wet fader, a hint, and randomize locks;
content sits in titled boxes on an auto-fit grid that shrinks cells instead of overflowing (no
scroll areas). The Grains tab holds the waveform with grain markers and Freeze, and warns when
grain amount is 0.

## Curve editor

| Action | Result |
|---|---|
| Click a pattern button | Sets `ts_pat` (outline = viewed, fill = playing `ts_active`) |
| Click empty canvas | Adds a point (inherits the split segment's shape), draggable at once |
| Drag point | Moves it; x stays between neighbours |
| Shift + drag | Bends tension |
| Right-click point | Cycles Curve → S-curve → Hold (circle, diamond, square) |
| Double-click point | Deletes it (at least one point kept) |
| Snap selector | Off / 1/8 / 1/12 / 1/16 on x and y; hold Alt to bypass |

The curve is drawn from `value_at` with the global tension, plus a cursor line (`ts_cursor`) and
a dot at `ts_value`. The pattern name is editable; Reset restores the slot from `factory_bank()`.
Each edit: write lock → edit → `sanitize()` → release → `pattern_version.fetch_add(1)`.

## FX sequencer

- Header: Run (`sq_on`), Step (`sq_rate`), live gain dots for the six stages.
- 6 lanes × 16 steps. The LED before a lane toggles lane-active (bit 16).
- Click toggles a cell; press-drag paints the first cell's new value. Writes use `fetch_xor`
  only when a bit changes.
- Right-click a lane name: All, Clear, Every 2nd, Every 4th, Off-beats, Last of 4, Invert.
- Beats of 4 are spaced and shaded; the current column (`seq_step`) is outlined.

## Randomize

Lock groups: Time (pattern, sync, depth, smooth, tension), each panel, Macros. Never touched:
power switches, section amounts, sequencer, mix, output, limiter, input gain, MIDI pitch, Low
CPU, oversampling, freeze, momentary buttons. Ranges are deliberately tame.

## Limitations

- Closing the editor while Rewind or Tape stop is held leaves it on until the editor reopens (it
  is released on the first frame after reopening, or whenever the button is not drawn). The
  editor build callback has no `ParamSetter` to release it on close.
- Curve and lane edits have no undo (they are state, not parameters); they are saved with the
  plugin state and in presets.
- Long labels in very small cells are clipped.

## Tests

`every_page_fits_at_min_and_default_size`, `tiny_window_does_not_panic`,
`degenerate_window_sizes_do_not_panic`, `cells_always_fit`, `cells_in_an_empty_rect_are_finite`,
`all_three_regions_fit_at_min_and_default`, `move_point_stays_between_neighbours`,
`edits_bump_version_and_add_points_in_order`, `snap_labels_include_triplets`.
