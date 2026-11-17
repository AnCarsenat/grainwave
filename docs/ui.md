# Editor (`src/editor.rs`)

Branch: `feat/ui` (merged). egui 0.31 via `nih_plug_egui`. No shared-file edits.

> Not yet verified by eye: the GUI was only smoke-tested (debug standalone with
> `--backend dummy` opened the window and ran ~8 s without panicking). Layout polish at
> 960 × 640 still needs a visual check in a host.

## Window

960 × 640 default, resizable (minimum 800 × 560, resize corner). Repaints every ~33 ms.

## Layout

**Top bar (70 px)**
- "GRAINWAVE" title.
- Preset browser: `<`, dropdown with Factory and User headers, `>`, name field, Save, Rescan,
  status line.
- Randomize button with a Macros lock under it.
- Knobs: Dust, Chaos, Age, Mix, Output. Limiter toggle.

**Waveform (24 % of remaining height, 100–220 px)**
- 256 peak bars, square-root scaled, bluish when frozen.
- Write head: red line.
- Grains: forward = amber dots, reverse = sage triangles, spread over 8 vertical lanes so
  overlapping grains stay visible.
- Legend, FREEZE/FROZEN toggle top right.

**Panels:** two rows of four, each with its own scroll area and lock toggle.

| Row 1 | Row 2 |
|---|---|
| Input | Degrade |
| Grains | Filter |
| Playback | Space |
| Rhythm | Modulation |

- **Rhythm:** 16-cell gate grid (click to toggle, every 4th cell shaded, current step outlined
  while the gate is on; reads `gate_step` as an integer). Rewind and Tape stop are on while held,
  off on release.
- **Modulation:** per LFO a bipolar meter plus shape, rate mode, Hz, target, amount. Envelope
  follower: 0–1 meter plus target, amount, attack, release.

## Controls

- Custom rotary knobs: drag vertically, Shift for fine, double-click to reset. Pitch and amount
  knobs fill from the center.
- Enum parameters: combo boxes.
- Every edit goes through `ParamSetter` begin/set/end, so hosts record automation and undo.

## Theme

Dark brown-black background, cream text, amber accent, sage for reverse grains and meters, brick
red for freeze and held buttons.

## Randomize

- 9 lock groups: the 8 panels plus Macros. Locks live in editor state (not parameters) and reset
  when the editor reopens.
- Never touched: output gain, mix, limiter, freeze, rewind, tape stop, input gain, MIDI pitch,
  Low CPU, oversampling.
- Ranges kept musical: bits 4–16, low-pass in the upper part of its range, feedback ≤ 0.7, pitch
  either 0 or a musical interval.
- After randomizing, the preset name shows "(unsaved)".

## Differences from the original spec

- Randomize also skips input gain (loudness jumps), MIDI pitch (would silence grains with no
  notes held), Low CPU and oversampling (CPU).
- Macros have their own lock, since they are not in a panel.

## Limitations

- Closing the editor while Rewind or Tape stop is held leaves that parameter on.
- At 960 × 640 the Degrade and Modulation panels need scrolling.
- Knobs ignore the scroll wheel and don't accept typed values.
- No "modified" marker after editing a loaded preset.
