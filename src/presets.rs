//! OWNER: feat/ui. Preset save/load as JSON. Factory presets in `presets/*.json`,
//! user presets in `$XDG_CONFIG_HOME/grainwave/presets/` (fallback `~/.config/grainwave/presets/`).
//! Never called from the audio thread.
//!
//! File format (values are normalized 0..1, keyed by parameter ID):
//!
//! ```json
//! { "name": "Dusty Memory", "author": "Grainwave", "version": 1,
//!   "params": { "g_size": 0.42, "m_dust": 0.6 } }
//! ```
//!
//! Applying a preset sets every preset-able parameter: IDs present in the file get the stored
//! value, missing IDs get their default. Performance controls (`freeze`, `r_rewind`, `r_tstop`)
//! are never stored in or changed by presets.

use crate::params::GrainwaveParams;
use nih_plug::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Current preset file format version.
pub const PRESET_VERSION: u32 = 1;

/// Parameters that presets never store or touch (live performance state).
pub const EXCLUDED_IDS: &[&str] = &["freeze", "r_rewind", "r_tstop"];

/// Factory presets compiled into the binary, in browser order.
const FACTORY_JSON: &[&str] = &[
    include_str!("../presets/01_init.json"),
    include_str!("../presets/02_dusty_memory.json"),
    include_str!("../presets/03_reverse_rain.json"),
    include_str!("../presets/04_tape_eater.json"),
    include_str!("../presets/05_broken_radio.json"),
    include_str!("../presets/06_half_speed_dream.json"),
    include_str!("../presets/07_stutter_crate.json"),
    include_str!("../presets/08_frozen_pad.json"),
    include_str!("../presets/09_cassette_choir.json"),
    include_str!("../presets/10_gated_dust.json"),
];

fn default_version() -> u32 {
    PRESET_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_version")]
    pub version: u32,
    /// Normalized values keyed by parameter ID.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetSource {
    Factory,
    User(PathBuf),
}

#[derive(Debug, Clone)]
pub struct PresetEntry {
    pub preset: Preset,
    pub source: PresetSource,
}

impl PresetEntry {
    pub fn is_factory(&self) -> bool {
        self.source == PresetSource::Factory
    }
}

pub fn is_excluded(id: &str) -> bool {
    EXCLUDED_IDS.contains(&id)
}

/// Parse a preset from JSON text.
pub fn parse(json: &str) -> Result<Preset, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// Serialize a preset as pretty JSON.
pub fn to_json(preset: &Preset) -> String {
    // Serializing a struct of strings and floats cannot fail.
    serde_json::to_string_pretty(preset).unwrap_or_default()
}

/// All factory presets. Invalid entries (only possible during development) are skipped.
pub fn factory_presets() -> Vec<Preset> {
    FACTORY_JSON.iter().filter_map(|j| parse(j).ok()).collect()
}

/// Capture the current (unmodulated) parameter values as a preset.
pub fn capture(params: &GrainwaveParams, name: &str) -> Preset {
    let mut map = BTreeMap::new();
    for (id, ptr, _) in params.param_map() {
        if is_excluded(&id) {
            continue;
        }
        // SAFETY: the pointer comes from `params`, which outlives this call.
        let v = unsafe { ptr.unmodulated_normalized_value() };
        map.insert(id, round_normalized(v));
    }
    Preset {
        name: name.to_string(),
        author: "User".to_string(),
        version: PRESET_VERSION,
        params: map,
    }
}

/// Round to 6 decimals so files stay readable; well below any parameter's resolution.
fn round_normalized(v: f32) -> f32 {
    ((v as f64 * 1e6).round() / 1e6) as f32
}

/// The normalized value every preset-able parameter should get when `preset` is applied,
/// snapped to the parameter's steps. Missing IDs map to the parameter's default.
pub fn targets(params: &GrainwaveParams, preset: &Preset) -> Vec<(String, ParamPtr, f32)> {
    params
        .param_map()
        .into_iter()
        .filter(|(id, _, _)| !is_excluded(id))
        .map(|(id, ptr, _)| {
            // SAFETY: the pointer comes from `params`, which outlives this call.
            let n = unsafe {
                match preset.params.get(&id) {
                    Some(v) if v.is_finite() => {
                        let plain = ptr.preview_plain(v.clamp(0.0, 1.0));
                        ptr.preview_normalized(plain)
                    }
                    _ => ptr.default_normalized_value(),
                }
            };
            (id, ptr, n)
        })
        .collect()
}

/// Apply a preset through the host (each parameter gets begin/set/end so hosts record undo
/// and automation). GUI thread only.
pub fn apply(params: &GrainwaveParams, setter: &ParamSetter, preset: &Preset) {
    let ctx = setter.raw_context;
    for (_, ptr, n) in targets(params, preset) {
        // SAFETY: `ptr` belongs to the plugin's own parameter object, which the wrapper knows.
        unsafe {
            if (ptr.unmodulated_normalized_value() - n).abs() < 1e-7 {
                continue;
            }
            ctx.raw_begin_set_parameter(ptr);
            ctx.raw_set_parameter_normalized(ptr, n);
            ctx.raw_end_set_parameter(ptr);
        }
    }
}

/// `$XDG_CONFIG_HOME/grainwave/presets/`, falling back to `~/.config/grainwave/presets/`.
pub fn user_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() && Path::new(&x).is_absolute() => PathBuf::from(x),
        _ => {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            PathBuf::from(home).join(".config")
        }
    };
    Some(base.join("grainwave").join("presets"))
}

/// Load one preset file.
pub fn load_file(path: &Path) -> Result<Preset, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// All readable `*.json` presets in `dir`, sorted by name (case-insensitive).
/// Unreadable or malformed files are skipped.
pub fn list_dir(dir: &Path) -> Vec<PresetEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PresetEntry> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .filter_map(|p| {
            load_file(&p).ok().map(|preset| PresetEntry { preset, source: PresetSource::User(p) })
        })
        .collect();
    out.sort_by_key(|e| e.preset.name.to_lowercase());
    out
}

/// User presets from the user preset directory.
pub fn list_user() -> Vec<PresetEntry> {
    user_dir().map(|d| list_dir(&d)).unwrap_or_default()
}

/// Factory presets followed by user presets.
pub fn all_presets() -> Vec<PresetEntry> {
    let mut out: Vec<PresetEntry> = factory_presets()
        .into_iter()
        .map(|preset| PresetEntry { preset, source: PresetSource::Factory })
        .collect();
    out.extend(list_user());
    out
}

/// File name for a preset name: lowercase ASCII alphanumerics, other characters become `_`.
pub fn file_name_for(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' })
        .collect();
    if s.trim_matches('_').is_empty() {
        s = "preset".to_string();
    }
    format!("{s}.json")
}

/// Save a preset into `dir` (created if needed), overwriting a file with the same name.
pub fn save_to_dir(dir: &Path, preset: &Preset) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file_name_for(&preset.name));
    std::fs::write(&path, to_json(preset)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Save a preset into the user preset directory.
pub fn save_user(preset: &Preset) -> Result<PathBuf, String> {
    let dir = user_dir().ok_or_else(|| "no home or XDG config directory".to_string())?;
    save_to_dir(&dir, preset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ids(params: &GrainwaveParams) -> HashSet<String> {
        params.param_map().into_iter().map(|(id, _, _)| id).collect()
    }

    #[test]
    fn param_ids_include_nested_suffixes() {
        let p = GrainwaveParams::default();
        let ids = ids(&p);
        for id in ["on_1", "on_16", "shape_1", "shape_2", "amt_2", "hz_1", "rmode_2", "target_1"] {
            assert!(ids.contains(id), "missing {id}; have {ids:?}");
        }
        for id in EXCLUDED_IDS {
            assert!(ids.contains(*id), "excluded id {id} is not a real param");
        }
    }

    #[test]
    fn factory_presets_parse() {
        assert_eq!(factory_presets().len(), FACTORY_JSON.len());
        for j in FACTORY_JSON {
            let p = parse(j).expect("factory preset parses");
            assert!(!p.name.is_empty());
            assert_eq!(p.version, PRESET_VERSION);
        }
    }

    #[test]
    fn factory_preset_names_unique() {
        let names: HashSet<String> = factory_presets().into_iter().map(|p| p.name).collect();
        assert_eq!(names.len(), FACTORY_JSON.len());
    }

    #[test]
    fn factory_keys_are_real_params_and_values_normalized() {
        let p = GrainwaveParams::default();
        let ids = ids(&p);
        for preset in factory_presets() {
            for (k, v) in &preset.params {
                assert!(ids.contains(k), "{}: unknown param id {k}", preset.name);
                assert!(!is_excluded(k), "{}: excluded id {k} in preset", preset.name);
                assert!((0.0..=1.0).contains(v), "{}: {k} = {v} out of range", preset.name);
            }
        }
    }

    #[test]
    fn capture_roundtrip_through_json() {
        let p = GrainwaveParams::default();
        let captured = capture(&p, "Round trip");
        assert!(!captured.params.contains_key("freeze"));
        let json = to_json(&captured);
        let parsed = parse(&json).unwrap();
        assert_eq!(parsed, captured);
        // Applying the captured preset targets exactly the current values.
        for (id, ptr, n) in targets(&p, &parsed) {
            let cur = unsafe { ptr.unmodulated_normalized_value() };
            assert!((cur - n).abs() < 1e-5, "{id}: {cur} vs {n}");
        }
    }

    #[test]
    fn custom_values_roundtrip_and_snap() {
        let p = GrainwaveParams::default();
        let mut preset = Preset {
            name: "x".into(),
            author: String::new(),
            version: 1,
            params: BTreeMap::new(),
        };
        preset.params.insert("g_size".into(), 0.42);
        preset.params.insert("g_env".into(), 0.4); // 4 variants: snaps to 1/3
        preset.params.insert("on_3".into(), 0.0);
        let parsed = parse(&to_json(&preset)).unwrap();
        let t: BTreeMap<String, f32> =
            targets(&p, &parsed).into_iter().map(|(id, _, n)| (id, n)).collect();
        assert!((t["g_size"] - 0.42).abs() < 1e-4);
        assert!((t["g_env"] - 1.0 / 3.0).abs() < 1e-4);
        assert_eq!(t["on_3"], 0.0);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let p = GrainwaveParams::default();
        let preset = parse(r#"{ "name": "Sparse", "params": { "m_dust": 0.5, "bogus_id": 0.3 } }"#)
            .unwrap();
        assert_eq!(preset.version, PRESET_VERSION);
        let t = targets(&p, &preset);
        for (id, ptr, n) in &t {
            if id == "m_dust" {
                assert!((n - 0.5).abs() < 1e-5);
            } else {
                let d = unsafe { ptr.default_normalized_value() };
                assert!((d - n).abs() < 1e-6, "{id}");
            }
        }
        assert!(!t.iter().any(|(id, _, _)| is_excluded(id)));
        // Minimal file: only a name.
        let bare = parse(r#"{ "name": "Bare" }"#).unwrap();
        assert!(bare.params.is_empty());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let p = GrainwaveParams::default();
        let preset = parse(r#"{ "name": "x", "params": { "mix": 3.0, "m_age": -1.0 } }"#).unwrap();
        for (id, _, n) in targets(&p, &preset) {
            assert!((0.0..=1.0).contains(&n), "{id}");
        }
    }

    #[test]
    fn save_and_list_dir() {
        let dir = std::env::temp_dir().join(format!("grainwave_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = GrainwaveParams::default();
        let preset = capture(&p, "My Weird/Preset");
        let path = save_to_dir(&dir, &preset).unwrap();
        assert_eq!(path.file_name().unwrap(), "my_weird_preset.json");
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        let listed = list_dir(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].preset, preset);
        assert!(!listed[0].is_factory());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name_for("Dusty Memory"), "dusty_memory.json");
        assert_eq!(file_name_for("../../etc"), "______etc.json");
        assert_eq!(file_name_for("  "), "preset.json");
    }
}
