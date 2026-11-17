//! Colors, layout constants and the egui style.

use nih_plug_egui::egui::{self, vec2, Color32, Context, Stroke};

pub const BG: Color32 = Color32::from_rgb(0x1b, 0x18, 0x16);
pub const PANEL: Color32 = Color32::from_rgb(0x25, 0x21, 0x1e);
pub const PANEL_EDGE: Color32 = Color32::from_rgb(0x3b, 0x34, 0x2e);
pub const WELL: Color32 = Color32::from_rgb(0x14, 0x12, 0x10);
pub const TEXT: Color32 = Color32::from_rgb(0xd9, 0xcc, 0xb8);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8f, 0x84, 0x76);
pub const ACCENT: Color32 = Color32::from_rgb(0xd8, 0x9c, 0x56); // amber
pub const ACCENT_2: Color32 = Color32::from_rgb(0x86, 0xb0, 0xa0); // sage, reverse grains
pub const HOT: Color32 = Color32::from_rgb(0xc8, 0x65, 0x55); // brick red, momentary/freeze
pub const WAVE: Color32 = Color32::from_rgb(0x6f, 0x63, 0x57);
pub const KNOB_TRACK: Color32 = Color32::from_rgb(0x3e, 0x37, 0x31);
pub const KNOB_BODY: Color32 = Color32::from_rgb(0x30, 0x2a, 0x25);
pub const CELL_OFF: Color32 = Color32::from_rgb(0x2e, 0x29, 0x25);
pub const CELL_OFF_BEAT: Color32 = Color32::from_rgb(0x3a, 0x33, 0x2d);
pub const GRID_LINE: Color32 = Color32::from_rgb(0x26, 0x22, 0x1f);
pub const GRID_LINE_STRONG: Color32 = Color32::from_rgb(0x38, 0x31, 0x2b);

/// One color per effect unit, so each section reads as its own pedal.
pub const C_TIME: Color32 = Color32::from_rgb(0x6c, 0xb4, 0xc0);
pub const C_GRAINS: Color32 = ACCENT;
pub const C_RHYTHM: Color32 = Color32::from_rgb(0xd0, 0x76, 0x5e);
pub const C_DEGRADE: Color32 = Color32::from_rgb(0xc4, 0xb0, 0x4e);
pub const C_FILTER: Color32 = Color32::from_rgb(0x8c, 0xb8, 0x6a);
pub const C_SPACE: Color32 = Color32::from_rgb(0x9a, 0x8c, 0xd0);
pub const C_MOD: Color32 = Color32::from_rgb(0xc8, 0x7f, 0xa8);

/// Colors of the six sequencer lanes / stage fades: time, grains, rhythm, degrade, filter, space.
pub const LANE_COLORS: [Color32; 6] = [C_TIME, C_GRAINS, C_RHYTHM, C_DEGRADE, C_FILTER, C_SPACE];
pub const LANE_NAMES: [&str; 6] = ["Time", "Grains", "Rhythm", "Degrade", "Filter", "Space"];

// Layout budget (logical pixels). Every page computes its rects from these, so nothing can
// overflow its area: see `mod.rs` for the totals at the default and minimum window sizes.
pub const MIN_W: f32 = 820.0;
pub const MIN_H: f32 = 560.0;
pub const MARGIN: f32 = 8.0;
pub const GAP: f32 = 6.0;
pub const TOP_H: f32 = 46.0;
pub const PAD: f32 = 6.0;
pub const HEADER_H: f32 = 20.0;
pub const TAB_H: f32 = 24.0;

pub fn apply_theme(ctx: &Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = WELL;
    v.faint_bg_color = PANEL;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(0x6e, 0x4c, 0x2a);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, PANEL_EDGE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.bg_fill = Color32::from_rgb(0x33, 0x2d, 0x28);
    w.inactive.weak_bg_fill = Color32::from_rgb(0x33, 0x2d, 0x28);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.bg_fill = Color32::from_rgb(0x45, 0x3c, 0x34);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x45, 0x3c, 0x34);
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.bg_fill = Color32::from_rgb(0x55, 0x47, 0x3a);
    w.active.weak_bg_fill = Color32::from_rgb(0x55, 0x47, 0x3a);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(6.0, 2.0);
        s.spacing.combo_width = 90.0;
        s.spacing.interact_size.y = 18.0;
        s.interaction.selectable_labels = false;
    });
}

/// Mix `c` toward the panel color; `t` = 1 keeps `c`.
pub fn dim(c: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| (b as f32 + (a as f32 - b as f32) * t).round() as u8;
    Color32::from_rgb(lerp(c.r(), PANEL.r()), lerp(c.g(), PANEL.g()), lerp(c.b(), PANEL.b()))
}
