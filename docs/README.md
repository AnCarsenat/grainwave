# Grainwave docs

Implementation notes per module: what was built, where it differs from the spec in the main
[README](../README.md), limitations, and tests. The main README stays the source of truth for
the design; these files record the details.

| Module | Doc | Source |
|---|---|---|
| Time shaper, patterns | [timeshape.md](timeshape.md) | `src/dsp/timeshape.rs`, `src/pattern.rs` |
| Grain engine, channel modes | [grains.md](grains.md) | `src/dsp/grains.rs`, `src/dsp/input.rs` |
| Rhythm and performance | [rhythm.md](rhythm.md) | `src/dsp/rhythm.rs` |
| Degrade chain, filter | [degrade.md](degrade.md) | `src/dsp/degrade/`, `src/dsp/filter.rs` |
| Reverb, delay, output | [space-output.md](space-output.md) | `src/dsp/space.rs`, `src/dsp/output.rs` |
| LFOs, envelope, macros | [modulation.md](modulation.md) | `src/dsp/modulation.rs` |
| Editor | [ui.md](ui.md) | `src/editor.rs` |
| Presets | [presets.md](presets.md) | `src/presets.rs`, `presets/` |

## Cross-module notes

- **Integer `VisualState` fields:** `gate_step`, `ts_active` and `seq_step` (u32::MAX = off) are
  plain integers. Every other `VisualState` float is stored as bits and read with `load_f32`.
- **Stage fades** (`lib.rs`): every section from Time to Space is wrapped in a 10 ms crossfade
  driven by power × mix × sequencer step. Sections keep processing at gain 0.
- **Latency:** the plugin reports 0 latency. Oversampling in the degrade chain adds about 23
  samples at 2×, so with mix below 100 % the dry and wet paths are slightly misaligned.
- **Block-rate parameters:** `Snapshot` is taken once per block; modulation and sequencer steps
  are block-rate. Modules smooth internally.
- **Clock:** gate, stutter, time shaper, sequencer, synced LFOs and synced grains assume 4/4 and
  freeze if the host stops advancing `beat_pos`.
- **`reset()` is allocation-free** after `initialize()` (`dsp::realloc_or_clear`), because
  nih-plug calls `Plugin::reset` on the audio thread.

## Known issues to fix next

1. Report oversampling latency to the host, or delay the dry path to match.
2. Crossfade `degrade_order` changes and channel-mode switches; smooth the unfreeze seam.
3. Visual check of the v2 editor in a host (verified only by headless layout tests).
4. Rewind / Tape stop stay on if the editor closes while held (released when it reopens).
5. Sequencer steps and MIDI pattern switches are block-accurate, not sample-accurate.
6. No undo for curve and sequencer edits.
7. Add a `LICENSE` file (GPL-3.0) before the first public release.
