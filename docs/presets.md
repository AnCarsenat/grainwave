# Presets (`src/presets.rs`, `presets/`)

Branch: `feat/ui` (merged). All preset code runs on the GUI thread.

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

| # | Name | Character |
|---|---|---|
| 1 | Init | Empty: every parameter at its default |
| 2 | Dusty Memory | 120 ms grains reaching back in memory, Dust 0.55, Age 0.35, crackle, head bump, 6 kHz low-pass, light reverb, 80 % mix |
| 3 | Reverse Rain | 85 % reverse, 220 ms wide grains, slow smooth-random LFO on Position, 35 % reverb, light Chaos |
| 4 | Tape Eater | Long grains, tape-stop 45 %, tape-start 25 %, Tape-first order, heavy wow, drive, tape age, hiss, dropouts |
| 5 | Broken Radio | Mono, 6-bit at 6 kHz, hard clip Drive-first, 400 Hz–3.5 kHz band-pass with resonance and drift, random LFO on low-pass, hiss, crackle, −3 dB out |
| 6 | Half-Speed Dream | Speed 0.5 (octave down), 300 ms grains, big reverb, 1/4 delay, gentle wow, 5 kHz low-pass |
| 7 | Stutter Crate | 2-bar synced buffer, 1/8 synced density, 1/16 stutter at 45 %, 30 % swing, 10-bit at 22 kHz, crackle, head bump |
| 8 | Frozen Pad | 450 ms dense Hann grains across all memory, octave-quantized jitter, full spread, huge reverb, slow LFOs on Position and low-pass (press Freeze yourself) |
| 9 | Cassette Choir | 7 st pentatonic-quantized pitch jitter, wide spread, Age 0.6, reverb |
| 10 | Gated Dust | 16-step gate pattern, 1-bar synced buffer, Dust 0.4, crushed 1/8T delay, swing |

## Limitations

- No delete or rename for user presets.
- Saving under a factory name creates a separate user entry.

## Tests

`param_ids_include_nested_suffixes`, `factory_presets_parse`, `factory_preset_names_unique`,
`factory_keys_are_real_params_and_values_normalized`, `capture_roundtrip_through_json`,
`custom_values_roundtrip_and_snap`, `missing_keys_use_defaults`,
`out_of_range_values_are_clamped`, `save_and_list_dir`, `file_names_are_safe`.
