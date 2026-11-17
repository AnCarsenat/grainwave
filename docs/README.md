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

- **`VisualState::gate_step` is an integer**, not f32 bits. Every other `VisualState` float is
  stored as bits and read with `load_f32`.
- **Latency:** the plugin reports 0 latency. Oversampling in the degrade chain adds about 23
  samples at 2×, so with mix below 100 % the dry and wet paths are slightly misaligned.
- **Block-rate parameters:** `Snapshot` is taken once per block and modulation is block-rate.
  Every module smooths internally, so automation and LFOs don't click, but fast LFOs step on large
  host buffers.
- **Clock:** gate, stutter, synced LFOs and synced grains assume 4/4 and freeze if the host stops
  advancing `beat_pos`.

## Known issues to fix next

1. Report oversampling latency to the host, or delay the dry path to match.
2. Crossfade `degrade_order` changes.
3. Release Rewind / Tape stop when the editor closes while they are held.
4. Visual check of the editor at 960 × 640 (Degrade and Modulation panels currently scroll).
5. Crossfade channel-mode switches and the unfreeze seam in the capture buffer.
6. Run the debug build in a host to exercise `assert_process_allocs` end to end.
