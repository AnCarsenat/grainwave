# Presets (`src/presets.rs`, `presets/`)

Branches: `feat/ui`, updated in `feat/ui-v2` (merged). All preset code runs on the GUI thread.

## Format

```json
{ "name": "Dusty Memory", "author": "Grainwave", "version": 1,
  "params": { "g_size": 0.42, "m_dust": 0.55 } }
```

- `params`: parameter ID → **normalized** value (0–1). `author`, `version` (default 1) and
  `params` are optional.
- Nested IDs (verified by test): `on_1` … `on_16`; `shape_N`, `rmode_N`, `hz_N`, `target_N`,
  `amt_N` for N = 1, 2.
- `freeze`, `r_rewind` and `r_tstop` are never stored or applied: they are live performance state.
- Optional `"time_patterns"`: a full `PatternBank`. When present it replaces the bank (sanitized,
  write lock, `pattern_version` bump); when absent the bank is left alone.
- Optional `"seq_lanes"`: six u32 lane words (bits 0–15 steps, bit 16 lane active). Replaces the
  sequencer lanes when present.
- `capture` writes both extras. Factory time presets point at pattern slots (`ts_pat`) instead of
  carrying a bank, so a user's edited patterns survive loading them.

## Applying

For every ID in `param_map()`:
1. Missing or non-finite value: use the parameter's default (so old presets survive new params).
2. Otherwise clamp to 0–1 and snap to the parameter's steps.
3. If it changed, `begin_set` / `set` / `end_set` through the `ParamSetter`; unchanged
   parameters are skipped.

## Storage

- **Factory:** `presets/*.json`, compiled in with `include_str!`. Values were generated from plain
  values with each parameter's `preview_normalized`, rounded to 4 decimals.
- **User:** `$XDG_CONFIG_HOME/grainwave/presets/`, fallback `~/.config/grainwave/presets/`.
  - File name from the preset name: lowercased, anything outside `[a-z0-9-]` becomes `_`, so no
    path tricks.
  - Same name overwrites. Broken files are skipped when listing.
- The list shows factory presets first, then user presets. It reloads when the editor opens, after
  a save, or on Rescan.

## API

`parse`, `to_json`, `factory_presets`, `capture(params, name)`, `targets(params, preset)`,
`apply(params, setter, preset)`, `user_dir`, `load_file`, `list_dir`, `list_user`, `all_presets`,
`file_name_for`, `save_to_dir`, `save_user`.

## Factory presets

Grain presets now set Grain amount (`g_amt` 0.6–1.0; grains are silent at 0) and are tamer than
v1: lower reverse, chaos, dust, dropouts, hiss and tape-stop chances.

| Name | Character |
|---|---|
| Init | Empty: everything default, audio untouched |
| Dusty Memory | Grains reaching back in memory, Dust, Age, crackle, head bump, soft low-pass |
| Reverse Rain | Mostly reversed wide grains, slow LFO on Position, reverb |
| Tape Eater | Long grains with tape stops/starts, Tape-first degrade, wow, drive |
| Broken Radio | Mono, crushed, band-passed with drift, hiss and crackle |
| Half-Speed Dream | Octave-down grains, big reverb, 1/4 delay, gentle wow |
| Stutter Crate | Synced grains, 1/16 stutter, swing, 10-bit crunch |
| Frozen Pad | Dense long grains for pads (press Freeze yourself) |
| Cassette Choir | Pentatonic pitch-jittered grains, Age, reverb |
| Gated Dust | 16-step gate, synced buffer, Dust, crushed triplet delay |
| Halftime Tape | Time pattern 2 (Halftime), slight wow, head bump, 12 kHz low-pass |
| Triplet Stutter | Time pattern 4 (1/12 triplet rolls), 12-bit, faint crackle and reverb |
| End-of-Bar Stop | Time pattern 6 (tape stop on beat 4), light drive and age |
| Clean Stairs | Time pattern 9 (Stairs 1/16), nothing else |
| Roll Out | Time pattern 5 (stutter roll), 22 kHz / 12-bit, a little reverb |
| Crush Every 4th | Sequencer: 5-bit / 8 kHz crush on steps 4, 8, 12, 16 |
| Space Throws | Sequencer: big reverb and 1/8 delay on steps 5–6 and 13–14 |
| Filter Chops | Sequencer: 900 Hz resonant low-pass on the off-16ths |

## Limitations

- No delete or rename for user presets.
- Saving under a factory name creates a separate user entry.

## Tests

`param_ids_include_nested_suffixes`, `factory_presets_parse`, `factory_preset_names_unique`,
`factory_keys_are_real_params_and_values_normalized`, `capture_roundtrip_through_json`,
`custom_values_roundtrip_and_snap`, `missing_keys_use_defaults`,
`out_of_range_values_are_clamped`, `save_and_list_dir`, `file_names_are_safe`,
`init_is_fully_clean`, `grain_presets_set_an_audible_amount`,
`time_and_sequencer_presets_are_well_formed`, `patterns_and_lanes_roundtrip`,
`absent_extras_leave_bank_and_lanes_alone`, `bank_from_preset_is_sanitized`.
