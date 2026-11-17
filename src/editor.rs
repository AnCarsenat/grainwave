//! OWNER: feat/ui. egui editor: sections per parameter group, waveform with live grains,
//! preset browser, randomize with section locks.
//!
//! Everything here runs on the GUI thread. The editor reads `VisualState` (atomics written by
//! the audio thread) and writes parameters only through `ParamSetter` begin/set/end.

use crate::dsp::rng::Rng;
use crate::params::GrainwaveParams;
use crate::presets::{self, PresetEntry, PresetSource};
use crate::shared::{load_f32, VisualState, MAX_VISUAL_GRAINS, WAVEFORM_POINTS};
use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, vec2, Align, Align2, Color32, ComboBox, Context, CornerRadius, FontId, Frame,
    Layout, Margin, Pos2, Rect, Response, ScrollArea, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use nih_plug_egui::{create_egui_editor, resizable_window::ResizableWindow};
use std::f32::consts::PI;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Theme: muted warm colors on a dark brown-black background.
// ---------------------------------------------------------------------------

const BG: Color32 = Color32::from_rgb(0x1b, 0x18, 0x16);
const PANEL: Color32 = Color32::from_rgb(0x25, 0x21, 0x1e);
const PANEL_EDGE: Color32 = Color32::from_rgb(0x3b, 0x34, 0x2e);
const WELL: Color32 = Color32::from_rgb(0x14, 0x12, 0x10);
const TEXT: Color32 = Color32::from_rgb(0xd9, 0xcc, 0xb8);
const TEXT_DIM: Color32 = Color32::from_rgb(0x8f, 0x84, 0x76);
const ACCENT: Color32 = Color32::from_rgb(0xd8, 0x9c, 0x56); // amber
const ACCENT_2: Color32 = Color32::from_rgb(0x86, 0xb0, 0xa0); // sage, reverse grains
const HOT: Color32 = Color32::from_rgb(0xc8, 0x65, 0x55); // brick red, momentary/freeze
const WAVE: Color32 = Color32::from_rgb(0x6f, 0x63, 0x57);
const KNOB_TRACK: Color32 = Color32::from_rgb(0x3e, 0x37, 0x31);

const KNOB_DIAMETER: f32 = 30.0;
const CELL_W: f32 = 54.0;
const CELL_H: f32 = 62.0;
const TOP_BAR_H: f32 = 70.0;

/// Repaint interval so the waveform, grains and meters animate (~30 fps).
const REPAINT: Duration = Duration::from_millis(33);

// ---------------------------------------------------------------------------
// Editor state (GUI only, never saved into parameters)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Input,
    Grains,
    Playback,
    Rhythm,
    Degrade,
    Filter,
    Space,
    Modulation,
    Macros,
}

const SECTION_COUNT: usize = 9;
const ALL_SECTIONS: [Section; SECTION_COUNT] = [
    Section::Input,
    Section::Grains,
    Section::Playback,
    Section::Rhythm,
    Section::Degrade,
    Section::Filter,
    Section::Space,
    Section::Modulation,
    Section::Macros,
];

pub struct EditorState {
    /// Randomize locks, indexed by `Section as usize`.
    locks: [bool; SECTION_COUNT],
    presets: Vec<PresetEntry>,
    current: Option<usize>,
    save_name: String,
    status: String,
    rng: Rng,
    /// Whether this editor is currently holding a momentary button down.
    rewind_held: bool,
    tape_stop_held: bool,
}

impl Default for EditorState {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
            .unwrap_or(1);
        Self {
            locks: [false; SECTION_COUNT],
            presets: Vec::new(),
            current: None,
            save_name: String::new(),
            status: String::new(),
            rng: Rng::new(seed),
            rewind_held: false,
            tape_stop_held: false,
        }
    }
}

impl EditorState {
    fn reload_presets(&mut self) {
        let current_name = self.current.and_then(|i| self.presets.get(i)).map(|e| e.preset.name.clone());
        self.presets = presets::all_presets();
        self.current = current_name.and_then(|n| self.presets.iter().position(|e| e.preset.name == n));
    }
}

pub fn create(params: Arc<GrainwaveParams>, visual: Arc<VisualState>) -> Option<Box<dyn Editor>> {
    let egui_state = params.editor_state.clone();
    create_egui_editor(
        params.editor_state.clone(),
        EditorState::default(),
        |ctx, state| {
            apply_theme(ctx);
            state.reload_presets();
        },
        move |ctx, setter, state| {
            ctx.request_repaint_after(REPAINT);
            ResizableWindow::new("grainwave-window")
                .min_size(Vec2::new(800.0, 560.0))
                .show(ctx, egui_state.as_ref(), |ui| {
                    draw(ui, &params, &visual, setter, state);
                });
        },
    )
}

fn apply_theme(ctx: &Context) {
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
    });
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

fn draw(
    ui: &mut Ui,
    params: &GrainwaveParams,
    visual: &VisualState,
    setter: &ParamSetter,
    state: &mut EditorState,
) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, BG);

    // Top bar.
    ui.allocate_ui_with_layout(
        vec2(full.width(), TOP_BAR_H),
        Layout::left_to_right(Align::Center),
        |ui| top_bar(ui, params, setter, state),
    );
    ui.add_space(4.0);

    // Waveform.
    let remaining = ui.available_height();
    let wave_h = (remaining * 0.24).clamp(100.0, 220.0);
    let (wave_rect, _) = ui.allocate_exact_size(vec2(full.width(), wave_h), Sense::hover());
    waveform(ui, wave_rect, params, visual, setter);
    ui.add_space(6.0);

    // Two rows of four panels.
    let spacing = 6.0;
    let row_h = ((ui.available_height() - spacing) / 2.0).max(120.0);
    let col_w = (full.width() - 3.0 * spacing) / 4.0;
    let rows: [[Section; 4]; 2] = [
        [Section::Input, Section::Grains, Section::Playback, Section::Rhythm],
        [Section::Degrade, Section::Filter, Section::Space, Section::Modulation],
    ];
    for row in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            for section in row {
                ui.allocate_ui_with_layout(
                    vec2(col_w, row_h),
                    Layout::top_down(Align::Min),
                    |ui| panel(ui, section, col_w, row_h, params, visual, setter, state),
                );
            }
        });
        ui.add_space(spacing - ui.spacing().item_spacing.y);
    }
}

fn section_title(s: Section) -> &'static str {
    match s {
        Section::Input => "INPUT",
        Section::Grains => "GRAINS",
        Section::Playback => "PLAYBACK",
        Section::Rhythm => "RHYTHM",
        Section::Degrade => "DEGRADE",
        Section::Filter => "FILTER",
        Section::Space => "SPACE",
        Section::Modulation => "MODULATION",
        Section::Macros => "MACROS",
    }
}

#[allow(clippy::too_many_arguments)]
fn panel(
    ui: &mut Ui,
    section: Section,
    w: f32,
    h: f32,
    params: &GrainwaveParams,
    visual: &VisualState,
    setter: &ParamSetter,
    state: &mut EditorState,
) {
    let margin = 6.0;
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, PANEL_EDGE))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::same(margin as i8))
        .show(ui, |ui| {
            let inner = vec2(w - 2.0 * margin - 2.0, h - 2.0 * margin - 2.0);
            ui.set_min_size(inner);
            ui.set_max_size(inner);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(section_title(section)).color(ACCENT).strong().size(12.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    lock_toggle(ui, &mut state.locks[section as usize]);
                });
            });
            ScrollArea::vertical()
                .id_salt(section_title(section))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                        section_contents(ui, section, params, visual, setter, state);
                    });
                });
        });
}

fn lock_toggle(ui: &mut Ui, locked: &mut bool) {
    let text = egui::RichText::new(if *locked { "LOCKED" } else { "lock" })
        .size(10.0)
        .color(if *locked { HOT } else { TEXT_DIM });
    if ui
        .add(egui::Button::new(text).small().selected(*locked))
        .on_hover_text("Locked sections are skipped by Randomize")
        .clicked()
    {
        *locked = !*locked;
    }
}

fn section_contents(
    ui: &mut Ui,
    section: Section,
    p: &GrainwaveParams,
    visual: &VisualState,
    s: &ParamSetter,
    state: &mut EditorState,
) {
    match section {
        Section::Input => {
            let i = &p.input;
            knob(ui, &i.input_gain, s, "In gain");
            combo(ui, "ch_mode", &i.channel_mode, s, "Channel");
            combo(ui, "buf_mode", &i.buffer_mode, s, "Buffer");
            knob(ui, &i.buffer_bars, s, "Bars");
            knob(ui, &i.buffer_seconds, s, "Seconds");
        }
        Section::Grains => {
            let g = &p.grain;
            knob(ui, &g.size_ms, s, "Size");
            combo(ui, "g_dmode", &g.density_mode, s, "Density");
            knob(ui, &g.density_hz, s, "Rate");
            combo(ui, "g_ddiv", &g.density_division, s, "Division");
            knob(ui, &g.position, s, "Position");
            knob(ui, &g.position_jitter, s, "Jitter");
            knob(ui, &g.memory, s, "Memory");
            knob(ui, &g.spread, s, "Spread");
            knob(ui, &g.max_voices, s, "Voices");
            combo(ui, "g_env", &g.envelope, s, "Envelope");
            toggle_cell(ui, &g.low_cpu, s, "Low CPU");
        }
        Section::Playback => {
            let pb = &p.playback;
            knob(ui, &pb.reverse_chance, s, "Reverse");
            knob_bipolar(ui, &pb.pitch_st, s, "Pitch");
            knob(ui, &pb.pitch_jitter_st, s, "P. jitter");
            combo(ui, "p_quant", &pb.pitch_quantize, s, "Quantize");
            knob(ui, &pb.speed, s, "Speed");
            knob(ui, &pb.tape_stop_chance, s, "Stop %");
            knob(ui, &pb.tape_start_chance, s, "Start %");
            toggle_cell(ui, &pb.midi_pitch, s, "MIDI pitch");
        }
        Section::Rhythm => rhythm_contents(ui, p, visual, s, state),
        Section::Degrade => {
            let d = &p.degrade;
            combo(ui, "d_order", &d.order, s, "Order");
            knob(ui, &d.bit_depth, s, "Bits");
            knob(ui, &d.sample_rate_hz, s, "Rate");
            knob(ui, &d.wow_rate_hz, s, "Wow Hz");
            knob(ui, &d.wow_depth, s, "Wow");
            knob(ui, &d.flutter_rate_hz, s, "Flut. Hz");
            knob(ui, &d.flutter_depth, s, "Flutter");
            combo(ui, "d_satm", &d.saturation_mode, s, "Saturation");
            knob(ui, &d.drive, s, "Drive");
            combo(ui, "d_os", &d.oversampling, s, "Oversample");
            knob(ui, &d.tape_age, s, "Tape age");
            knob(ui, &d.head_bump_db, s, "Bump");
            knob(ui, &d.hiss, s, "Hiss");
            knob(ui, &d.crackle_density, s, "Crackle");
            knob(ui, &d.crackle_level, s, "Crk lvl");
            knob(ui, &d.dropout_chance, s, "Dropout");
            knob(ui, &d.dropout_depth, s, "Drop dpt");
        }
        Section::Filter => {
            let f = &p.filter;
            knob(ui, &f.lowpass_hz, s, "Low-pass");
            knob(ui, &f.lowpass_res, s, "LP res");
            knob(ui, &f.highpass_hz, s, "High-pass");
            knob(ui, &f.highpass_res, s, "HP res");
            knob(ui, &f.drift, s, "Drift");
        }
        Section::Space => {
            let sp = &p.space;
            knob(ui, &sp.reverb_size, s, "Rev size");
            knob(ui, &sp.reverb_mix, s, "Rev mix");
            combo(ui, "s_dldiv", &sp.delay_division, s, "Delay");
            knob(ui, &sp.delay_feedback, s, "Feedback");
            knob(ui, &sp.delay_mix, s, "Dly mix");
            knob(ui, &sp.delay_crush, s, "Crush");
        }
        Section::Modulation => modulation_contents(ui, p, visual, s),
        Section::Macros => {}
    }
}

fn rhythm_contents(
    ui: &mut Ui,
    p: &GrainwaveParams,
    visual: &VisualState,
    s: &ParamSetter,
    state: &mut EditorState,
) {
    let r = &p.rhythm;
    combo(ui, "r_stut", &r.stutter_division, s, "Stutter");
    knob(ui, &r.stutter_chance, s, "Chance");
    knob(ui, &r.swing, s, "Swing");
    knob(ui, &r.tape_stop_time_ms, s, "Stop time");
    ui.end_row();

    // Gate on/off + 16-step grid.
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            toggle(ui, &r.gate_enabled, s, "Gate");
            momentary(ui, &r.rewind, s, "Rewind", &mut state.rewind_held);
            momentary(ui, &r.tape_stop, s, "Tape stop", &mut state.tape_stop_held);
        });
        gate_grid(ui, p, visual, s);
    });
}

fn gate_grid(ui: &mut Ui, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let r = &p.rhythm;
    let gap = 2.0;
    let avail = ui.available_width().max(120.0);
    let cell_w = ((avail - 15.0 * gap) / 16.0).clamp(8.0, 22.0);
    let cell_h = 18.0;
    let (rect, _) = ui.allocate_exact_size(vec2(16.0 * cell_w + 15.0 * gap, cell_h), Sense::hover());
    let current = visual.gate_step.load(Ordering::Relaxed) as usize % 16;
    let gate_on = r.gate_enabled.value();
    for (i, step) in r.gate_steps.iter().enumerate() {
        let x = rect.left() + i as f32 * (cell_w + gap);
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(cell_w, cell_h));
        let resp = ui.interact(cell, ui.id().with(("gate_step", i)), Sense::click());
        let on = step.on.value();
        if resp.clicked() {
            s.begin_set_parameter(&step.on);
            s.set_parameter(&step.on, !on);
            s.end_set_parameter(&step.on);
        }
        let beat = i % 4 == 0;
        let fill = match (on, gate_on) {
            (true, true) => ACCENT,
            (true, false) => Color32::from_rgb(0x7a, 0x60, 0x44),
            (false, _) => {
                if beat {
                    Color32::from_rgb(0x3a, 0x33, 0x2d)
                } else {
                    Color32::from_rgb(0x2e, 0x29, 0x25)
                }
            }
        };
        ui.painter().rect_filled(cell, 2.0, fill);
        if gate_on && i == current {
            ui.painter().rect_stroke(cell.expand(1.0), 2.0, Stroke::new(2.0, TEXT), StrokeKind::Outside);
        } else if resp.hovered() {
            ui.painter().rect_stroke(cell, 2.0, Stroke::new(1.0, TEXT_DIM), StrokeKind::Inside);
        }
        resp.on_hover_text(format!("Step {}", i + 1));
    }
}

fn modulation_contents(ui: &mut Ui, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let m = &p.modulation;
    for (i, lfo) in m.lfos.iter().enumerate() {
        let value = load_f32(&visual.lfo_values[i]);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("LFO {}", i + 1)).size(11.0).color(TEXT_DIM));
                meter(ui, value, true);
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                combo(ui, ("shape", i), &lfo.shape, s, "Shape");
                combo(ui, ("rmode", i), &lfo.rate_mode, s, "Rate mode");
                knob(ui, &lfo.rate_hz, s, "Hz");
                combo(ui, ("target", i), &lfo.target, s, "Target");
                knob_bipolar(ui, &lfo.amount, s, "Amount");
            });
        });
        ui.end_row();
    }
    let env = load_f32(&visual.env_value);
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ENVELOPE").size(11.0).color(TEXT_DIM));
            meter(ui, env, false);
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            combo(ui, "env_target", &m.env_target, s, "Target");
            knob_bipolar(ui, &m.env_amount, s, "Amount");
            knob(ui, &m.env_attack_ms, s, "Attack");
            knob(ui, &m.env_release_ms, s, "Release");
        });
    });
}

/// Small horizontal meter. Bipolar meters fill from the center.
fn meter(ui: &mut Ui, value: f32, bipolar: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(90.0, 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, WELL);
    let v = if value.is_finite() { value } else { 0.0 };
    let fill_rect = if bipolar {
        let v = v.clamp(-1.0, 1.0);
        let c = rect.center().x;
        let x = c + v * rect.width() * 0.5;
        Rect::from_x_y_ranges(c.min(x)..=c.max(x), rect.y_range())
    } else {
        let v = v.clamp(0.0, 1.0);
        Rect::from_x_y_ranges(rect.left()..=rect.left() + v * rect.width(), rect.y_range())
    };
    painter.rect_filled(fill_rect, 2.0, ACCENT_2);
    if bipolar {
        painter.line_segment(
            [pos2(rect.center().x, rect.top()), pos2(rect.center().x, rect.bottom())],
            Stroke::new(1.0, TEXT_DIM),
        );
    }
}

// ---------------------------------------------------------------------------
// Top bar: name, presets, randomize, macros, mix, output
// ---------------------------------------------------------------------------

fn top_bar(ui: &mut Ui, p: &GrainwaveParams, s: &ParamSetter, state: &mut EditorState) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new("GRAINWAVE").size(20.0).strong().color(ACCENT));
        ui.label(egui::RichText::new("granular lofi destroyer").size(10.0).color(TEXT_DIM));
    });
    ui.separator();
    preset_browser(ui, p, s, state);
    ui.separator();
    ui.vertical(|ui| {
        if ui
            .add(egui::Button::new(egui::RichText::new("Randomize").color(ACCENT)).min_size(vec2(84.0, 22.0)))
            .on_hover_text("Randomize unlocked sections (never mix, output, limiter or freeze)")
            .clicked()
        {
            randomize(p, s, state);
            state.current = None;
            state.status = "Randomized".into();
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("macros").size(10.0).color(TEXT_DIM));
            lock_toggle(ui, &mut state.locks[Section::Macros as usize]);
        });
    });
    ui.separator();
    ui.spacing_mut().item_spacing.x = 2.0;
    knob(ui, &p.macros.dust, s, "Dust");
    knob(ui, &p.macros.chaos, s, "Chaos");
    knob(ui, &p.macros.age, s, "Age");
    ui.separator();
    knob(ui, &p.output.mix, s, "Mix");
    knob(ui, &p.output.output_gain, s, "Output");
    ui.vertical(|ui| {
        ui.add_space(14.0);
        toggle(ui, &p.output.limiter, s, "Limiter");
    });
}

fn preset_browser(ui: &mut Ui, p: &GrainwaveParams, s: &ParamSetter, state: &mut EditorState) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let n = state.presets.len();
            let mut load: Option<usize> = None;
            if ui.button("<").on_hover_text("Previous preset").clicked() && n > 0 {
                load = Some(match state.current {
                    Some(i) => (i + n - 1) % n,
                    None => n - 1,
                });
            }
            let selected = state
                .current
                .and_then(|i| state.presets.get(i))
                .map(|e| e.preset.name.clone())
                .unwrap_or_else(|| "(unsaved)".to_string());
            ComboBox::from_id_salt("preset_combo")
                .width(150.0)
                .height(400.0)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    let mut shown_user_header = false;
                    ui.label(egui::RichText::new("Factory").size(10.0).color(TEXT_DIM));
                    for (i, e) in state.presets.iter().enumerate() {
                        if !e.is_factory() && !shown_user_header {
                            ui.separator();
                            ui.label(egui::RichText::new("User").size(10.0).color(TEXT_DIM));
                            shown_user_header = true;
                        }
                        if ui.selectable_label(state.current == Some(i), &e.preset.name).clicked() {
                            load = Some(i);
                        }
                    }
                });
            if ui.button(">").on_hover_text("Next preset").clicked() && n > 0 {
                load = Some(match state.current {
                    Some(i) => (i + 1) % n,
                    None => 0,
                });
            }
            if let Some(i) = load {
                if let Some(e) = state.presets.get(i) {
                    presets::apply(p, s, &e.preset);
                    state.save_name = e.preset.name.clone();
                    state.status = match &e.source {
                        PresetSource::Factory => "Factory preset".into(),
                        PresetSource::User(_) => "User preset".into(),
                    };
                    state.current = Some(i);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.save_name)
                    .hint_text("preset name")
                    .desired_width(110.0),
            );
            if ui.button("Save").on_hover_text("Save as a user preset").clicked() {
                save_preset(p, state);
            }
            if ui.small_button("Rescan").on_hover_text("Reload user presets from disk").clicked() {
                state.reload_presets();
            }
        });
        ui.label(egui::RichText::new(&state.status).size(10.0).color(TEXT_DIM));
    });
}

fn save_preset(p: &GrainwaveParams, state: &mut EditorState) {
    let name = state.save_name.trim().to_string();
    if name.is_empty() {
        state.status = "Enter a preset name first".into();
        return;
    }
    let preset = presets::capture(p, &name);
    match presets::save_user(&preset) {
        Ok(path) => {
            state.presets = presets::all_presets();
            state.current = state
                .presets
                .iter()
                .position(|e| e.source == PresetSource::User(path.clone()));
            state.status = format!("Saved {}", path.display());
        }
        Err(e) => state.status = format!("Save failed: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Waveform canvas
// ---------------------------------------------------------------------------

fn waveform(
    ui: &mut Ui,
    rect: Rect,
    p: &GrainwaveParams,
    visual: &VisualState,
    s: &ParamSetter,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, WELL);
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0, PANEL_EDGE), StrokeKind::Inside);

    let frozen = p.input.freeze.value();
    let inner = rect.shrink(6.0);
    let mid = inner.center().y;
    let half = inner.height() * 0.5;

    // Faint grid every 1/8 of the buffer.
    for k in 1..8 {
        let x = inner.left() + inner.width() * k as f32 / 8.0;
        painter.line_segment([pos2(x, inner.top()), pos2(x, inner.bottom())], Stroke::new(1.0, Color32::from_rgb(0x22, 0x1e, 0x1b)));
    }
    painter.line_segment([pos2(inner.left(), mid), pos2(inner.right(), mid)], Stroke::new(1.0, PANEL_EDGE));

    // Peaks.
    let wave_color = if frozen { Color32::from_rgb(0x7e, 0x93, 0x9a) } else { WAVE };
    let slice_w = inner.width() / WAVEFORM_POINTS as f32;
    for i in 0..WAVEFORM_POINTS {
        let peak = load_f32(&visual.waveform[i]);
        let a = if peak.is_finite() { peak.clamp(0.0, 1.0) } else { 0.0 };
        if a <= 0.0 {
            continue;
        }
        let x = inner.left() + (i as f32 + 0.5) * slice_w;
        let y = a.sqrt() * half; // sqrt so quiet material is still visible
        painter.line_segment([pos2(x, mid - y), pos2(x, mid + y)], Stroke::new(slice_w.max(1.0) * 0.8, wave_color));
    }

    // Write head.
    let wh = load_f32(&visual.write_head);
    if wh.is_finite() {
        let x = inner.left() + wh.clamp(0.0, 1.0) * inner.width();
        let c = if frozen { TEXT_DIM } else { HOT };
        painter.line_segment([pos2(x, inner.top()), pos2(x, inner.bottom())], Stroke::new(1.5, c));
    }

    // Grains.
    let active = visual.grain_active.load(Ordering::Relaxed);
    let reverse = visual.grain_reverse.load(Ordering::Relaxed);
    for i in 0..MAX_VISUAL_GRAINS {
        if active & (1 << i) == 0 {
            continue;
        }
        let pos = load_f32(&visual.grain_pos[i]);
        if !pos.is_finite() {
            continue;
        }
        let x = inner.left() + pos.clamp(0.0, 1.0) * inner.width();
        // Spread voices vertically so overlapping grains stay visible.
        let lane = (i % 8) as f32 / 7.0;
        let y = inner.top() + 8.0 + lane * (inner.height() - 16.0);
        let rev = reverse & (1 << i) != 0;
        let c = if rev { ACCENT_2 } else { ACCENT };
        painter.line_segment([pos2(x, y - 6.0), pos2(x, y + 6.0)], Stroke::new(1.0, c.gamma_multiply(0.6)));
        if rev {
            // Left-pointing triangle for reverse grains.
            painter.add(Shape::convex_polygon(
                vec![pos2(x - 4.0, y), pos2(x + 3.0, y - 4.0), pos2(x + 3.0, y + 4.0)],
                c,
                Stroke::NONE,
            ));
        } else {
            painter.circle_filled(pos2(x, y), 3.5, c);
        }
    }

    // Legend + freeze toggle.
    painter.text(
        pos2(inner.left() + 4.0, inner.top() + 2.0),
        Align2::LEFT_TOP,
        "buffer",
        FontId::proportional(10.0),
        TEXT_DIM,
    );
    let legend_y = inner.bottom() - 2.0;
    painter.circle_filled(pos2(inner.left() + 8.0, legend_y - 5.0), 3.0, ACCENT);
    painter.text(pos2(inner.left() + 14.0, legend_y), Align2::LEFT_BOTTOM, "forward", FontId::proportional(10.0), TEXT_DIM);
    painter.circle_filled(pos2(inner.left() + 66.0, legend_y - 5.0), 3.0, ACCENT_2);
    painter.text(pos2(inner.left() + 72.0, legend_y), Align2::LEFT_BOTTOM, "reverse", FontId::proportional(10.0), TEXT_DIM);

    let btn = Rect::from_min_size(pos2(rect.right() - 78.0, rect.top() + 6.0), vec2(70.0, 22.0));
    let text = egui::RichText::new(if frozen { "FROZEN" } else { "FREEZE" }).color(if frozen { HOT } else { TEXT });
    if ui
        .put(btn, egui::Button::new(text).selected(frozen))
        .on_hover_text("Stop recording and hold the current buffer")
        .clicked()
    {
        s.begin_set_parameter(&p.input.freeze);
        s.set_parameter(&p.input.freeze, !frozen);
        s.end_set_parameter(&p.input.freeze);
    }
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

fn knob<P: Param>(ui: &mut Ui, param: &P, s: &ParamSetter, label: &str) -> Response {
    knob_impl(ui, param, s, label, false)
}

fn knob_bipolar<P: Param>(ui: &mut Ui, param: &P, s: &ParamSetter, label: &str) -> Response {
    knob_impl(ui, param, s, label, true)
}

/// Rotary knob. Drag up/down to change (Shift for fine), double-click to reset to default.
fn knob_impl<P: Param>(ui: &mut Ui, param: &P, s: &ParamSetter, label: &str, bipolar: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(CELL_W, CELL_H), Sense::click_and_drag());
    let drag_id = resp.id.with("drag_value");

    if resp.double_clicked() {
        s.begin_set_parameter(param);
        s.set_parameter(param, param.default_plain_value());
        s.end_set_parameter(param);
    } else {
        if resp.drag_started() {
            s.begin_set_parameter(param);
            let start = param.unmodulated_normalized_value();
            ui.memory_mut(|m| m.data.insert_temp(drag_id, start));
        }
        if resp.dragged() {
            let fine = ui.input(|i| i.modifiers.shift);
            let speed = if fine { 0.0015 } else { 0.006 };
            let mut v: f32 = ui.memory(|m| m.data.get_temp(drag_id)).unwrap_or_else(|| param.unmodulated_normalized_value());
            v = (v - resp.drag_delta().y * speed).clamp(0.0, 1.0);
            ui.memory_mut(|m| m.data.insert_temp(drag_id, v));
            let plain = param.preview_plain(v);
            if param.preview_normalized(param.preview_plain(v)) != param.unmodulated_normalized_value() {
                s.set_parameter(param, plain);
            }
        }
        if resp.drag_stopped() {
            s.end_set_parameter(param);
            ui.memory_mut(|m| m.data.remove::<f32>(drag_id));
        }
    }

    let painter = ui.painter();
    let center = pos2(rect.center().x, rect.top() + 2.0 + KNOB_DIAMETER * 0.5);
    let radius = KNOB_DIAMETER * 0.5;
    let start = PI * 0.75; // 7:30 position (egui y points down)
    let sweep = PI * 1.5;
    let norm = param.modulated_normalized_value().clamp(0.0, 1.0);
    let from: f32 = if bipolar { 0.5 } else { 0.0 };

    painter.circle_filled(center, radius - 3.0, Color32::from_rgb(0x30, 0x2a, 0x25));
    arc(painter, center, radius, start, start + sweep, Stroke::new(3.0, KNOB_TRACK));
    let (a0, a1) = (start + sweep * from.min(norm), start + sweep * from.max(norm));
    if a1 - a0 > 0.001 {
        let c = if resp.hovered() || resp.dragged() { Color32::from_rgb(0xec, 0xb2, 0x6c) } else { ACCENT };
        arc(painter, center, radius, a0, a1, Stroke::new(3.0, c));
    }
    let a = start + sweep * norm;
    let dir = vec2(a.cos(), a.sin());
    painter.line_segment([center + dir * 3.0, center + dir * (radius - 4.0)], Stroke::new(2.0, TEXT));

    let label_y = rect.top() + KNOB_DIAMETER + 5.0;
    painter.text(pos2(rect.center().x, label_y), Align2::CENTER_TOP, label, FontId::proportional(10.5), TEXT);
    let value = param.to_string();
    painter.text(
        pos2(rect.center().x, label_y + 13.0),
        Align2::CENTER_TOP,
        value,
        FontId::proportional(9.5),
        TEXT_DIM,
    );

    resp.on_hover_text(format!("{}\nDrag (Shift: fine), double-click to reset", param.name()))
}

fn arc(painter: &egui::Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = ((a1 - a0).abs() / (PI / 24.0)).ceil().max(1.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    painter.add(Shape::line(pts, stroke));
}

/// Combo box for any stepped parameter (enums), with a small label above, sized like a knob cell
/// pair.
fn combo<P: Param>(ui: &mut Ui, id: impl std::hash::Hash, param: &P, s: &ParamSetter, label: &str) {
    let steps = param.step_count().unwrap_or(1).max(1);
    let current = (param.unmodulated_normalized_value() * steps as f32).round() as usize;
    ui.allocate_ui_with_layout(vec2(CELL_W * 2.0 - 4.0, CELL_H), Layout::top_down(Align::Min), |ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(label).size(10.5));
        ComboBox::from_id_salt(id)
            .width(CELL_W * 2.0 - 12.0)
            .height(320.0)
            .selected_text(egui::RichText::new(param.to_string()).size(11.0))
            .show_ui(ui, |ui| {
                for i in 0..=steps {
                    let n = i as f32 / steps as f32;
                    let text = param.normalized_value_to_string(n, false);
                    if ui.selectable_label(i == current, text).clicked() && i != current {
                        s.begin_set_parameter(param);
                        s.set_parameter(param, param.preview_plain(n));
                        s.end_set_parameter(param);
                    }
                }
            });
    });
}

/// Toggle button for a bool parameter.
fn toggle(ui: &mut Ui, param: &BoolParam, s: &ParamSetter, label: &str) -> Response {
    let on = param.value();
    let text = egui::RichText::new(label).size(11.0).color(if on { ACCENT } else { TEXT });
    let resp = ui.add(egui::Button::new(text).selected(on));
    if resp.clicked() {
        s.begin_set_parameter(param);
        s.set_parameter(param, !on);
        s.end_set_parameter(param);
    }
    resp
}

/// Toggle inside a knob-sized cell so it lines up with knobs in wrapped rows.
fn toggle_cell(ui: &mut Ui, param: &BoolParam, s: &ParamSetter, label: &str) {
    ui.allocate_ui_with_layout(vec2(CELL_W * 1.5, CELL_H), Layout::top_down(Align::Center), |ui| {
        ui.add_space(20.0);
        toggle(ui, param, s, label);
    });
}

/// Momentary button: sets the parameter true while the pointer is held on it, false on release.
/// `held` remembers whether this editor started the gesture so begin/end stay balanced.
fn momentary(ui: &mut Ui, param: &BoolParam, s: &ParamSetter, label: &str, held: &mut bool) {
    let on = param.value();
    let text = egui::RichText::new(label).size(11.0).color(if on { BG } else { TEXT });
    let mut button = egui::Button::new(text).sense(Sense::click_and_drag());
    if on {
        button = button.fill(HOT);
    }
    let resp = ui.add(button).on_hover_text("Hold");
    let down = resp.is_pointer_button_down_on();
    if down && !*held {
        s.begin_set_parameter(param);
        s.set_parameter(param, true);
        *held = true;
    } else if !down && *held {
        s.set_parameter(param, false);
        s.end_set_parameter(param);
        *held = false;
    }
}

// ---------------------------------------------------------------------------
// Randomize
// ---------------------------------------------------------------------------

/// Set a parameter to a random normalized value in `lo..hi`.
fn rnd<P: Param>(s: &ParamSetter, p: &P, rng: &mut Rng, lo: f32, hi: f32) {
    let n = lo + (hi - lo) * rng.next_f32();
    set_norm(s, p, n);
}

fn set_norm<P: Param>(s: &ParamSetter, p: &P, n: f32) {
    s.begin_set_parameter(p);
    s.set_parameter(p, p.preview_plain(n.clamp(0.0, 1.0)));
    s.end_set_parameter(p);
}

fn set_plain<P: Param>(s: &ParamSetter, p: &P, v: P::Plain) {
    s.begin_set_parameter(p);
    s.set_parameter(p, v);
    s.end_set_parameter(p);
}

fn pick<T: Copy>(rng: &mut Rng, items: &[T]) -> T {
    items[(rng.next_u32() as usize) % items.len()]
}

/// Randomize every unlocked section with musically tame ranges. Never touches mix, output gain,
/// limiter, freeze, the momentary buttons, input gain, MIDI pitch, low-CPU or oversampling.
fn randomize(p: &GrainwaveParams, s: &ParamSetter, state: &mut EditorState) {
    let locks = state.locks;
    let rng = &mut state.rng;
    for section in ALL_SECTIONS {
        if locks[section as usize] {
            continue;
        }
        match section {
            Section::Input => {
                let i = &p.input;
                if rng.chance(0.3) {
                    rnd(s, &i.channel_mode, rng, 0.0, 1.0);
                } else {
                    set_norm(s, &i.channel_mode, 0.0);
                }
                rnd(s, &i.buffer_mode, rng, 0.0, 1.0);
                rnd(s, &i.buffer_bars, rng, 0.0, 1.0);
                rnd(s, &i.buffer_seconds, rng, 0.0, 1.0);
            }
            Section::Grains => {
                let g = &p.grain;
                rnd(s, &g.size_ms, rng, 0.3, 0.95);
                rnd(s, &g.density_mode, rng, 0.0, 1.0);
                rnd(s, &g.density_hz, rng, 0.2, 0.8);
                rnd(s, &g.density_division, rng, 0.0, 1.0);
                rnd(s, &g.position, rng, 0.0, 0.6);
                rnd(s, &g.position_jitter, rng, 0.0, 0.7);
                rnd(s, &g.memory, rng, 0.0, 1.0);
                rnd(s, &g.envelope, rng, 0.0, 1.0);
                rnd(s, &g.max_voices, rng, 0.3, 0.8);
                rnd(s, &g.spread, rng, 0.0, 1.0);
            }
            Section::Playback => {
                let pb = &p.playback;
                rnd(s, &pb.reverse_chance, rng, 0.0, 1.0);
                let pitch = if rng.chance(0.6) { 0.0 } else { pick(rng, &[-12.0, -7.0, -5.0, 5.0, 7.0, 12.0]) };
                set_plain(s, &pb.pitch_st, pitch);
                set_plain(s, &pb.pitch_jitter_st, if rng.chance(0.5) { 0.0 } else { rng.next_f32() * 5.0 });
                rnd(s, &pb.pitch_quantize, rng, 0.0, 1.0);
                rnd(s, &pb.tape_stop_chance, rng, 0.0, 0.4);
                rnd(s, &pb.tape_start_chance, rng, 0.0, 0.3);
                set_plain(s, &pb.speed, pick(rng, &[0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 1.5, 2.0]));
            }
            Section::Rhythm => {
                let r = &p.rhythm;
                rnd(s, &r.stutter_division, rng, 0.0, 1.0);
                rnd(s, &r.stutter_chance, rng, 0.0, 0.5);
                rnd(s, &r.swing, rng, 0.0, 0.5);
                set_plain(s, &r.gate_enabled, rng.chance(0.3));
                for (k, step) in r.gate_steps.iter().enumerate() {
                    set_plain(s, &step.on, k == 0 || rng.chance(0.7));
                }
                rnd(s, &r.tape_stop_time_ms, rng, 0.0, 1.0);
            }
            Section::Degrade => {
                let d = &p.degrade;
                rnd(s, &d.order, rng, 0.0, 1.0);
                set_plain(s, &d.bit_depth, 4.0 + 12.0 * rng.next_f32());
                rnd(s, &d.sample_rate_hz, rng, 0.35, 1.0);
                rnd(s, &d.wow_rate_hz, rng, 0.0, 1.0);
                rnd(s, &d.wow_depth, rng, 0.0, 0.6);
                rnd(s, &d.flutter_rate_hz, rng, 0.0, 1.0);
                rnd(s, &d.flutter_depth, rng, 0.0, 0.4);
                rnd(s, &d.saturation_mode, rng, 0.0, 1.0);
                rnd(s, &d.drive, rng, 0.0, 0.7);
                rnd(s, &d.tape_age, rng, 0.0, 0.8);
                rnd(s, &d.head_bump_db, rng, 0.0, 0.66);
                rnd(s, &d.hiss, rng, 0.0, 0.4);
                rnd(s, &d.crackle_density, rng, 0.0, 0.6);
                rnd(s, &d.crackle_level, rng, 0.1, 0.6);
                rnd(s, &d.dropout_chance, rng, 0.0, 0.3);
                rnd(s, &d.dropout_depth, rng, 0.3, 0.9);
            }
            Section::Filter => {
                let f = &p.filter;
                rnd(s, &f.lowpass_hz, rng, 0.6, 1.0);
                rnd(s, &f.lowpass_res, rng, 0.0, 0.6);
                rnd(s, &f.highpass_hz, rng, 0.0, 0.4);
                rnd(s, &f.highpass_res, rng, 0.0, 0.5);
                rnd(s, &f.drift, rng, 0.0, 0.6);
            }
            Section::Space => {
                let sp = &p.space;
                rnd(s, &sp.reverb_size, rng, 0.0, 1.0);
                rnd(s, &sp.reverb_mix, rng, 0.0, 0.5);
                rnd(s, &sp.delay_division, rng, 0.0, 1.0);
                set_plain(s, &sp.delay_feedback, 0.7 * rng.next_f32());
                rnd(s, &sp.delay_mix, rng, 0.0, 0.4);
                rnd(s, &sp.delay_crush, rng, 0.0, 1.0);
            }
            Section::Modulation => {
                let m = &p.modulation;
                for lfo in &m.lfos {
                    rnd(s, &lfo.shape, rng, 0.0, 1.0);
                    rnd(s, &lfo.rate_mode, rng, 0.0, 1.0);
                    rnd(s, &lfo.rate_hz, rng, 0.1, 0.8);
                    if rng.chance(0.5) {
                        set_norm(s, &lfo.target, 0.0);
                    } else {
                        rnd(s, &lfo.target, rng, 0.0, 1.0);
                    }
                    rnd(s, &lfo.amount, rng, 0.25, 0.75);
                }
                if rng.chance(0.5) {
                    set_norm(s, &m.env_target, 0.0);
                } else {
                    rnd(s, &m.env_target, rng, 0.0, 1.0);
                }
                rnd(s, &m.env_amount, rng, 0.25, 0.75);
                rnd(s, &m.env_attack_ms, rng, 0.0, 1.0);
                rnd(s, &m.env_release_ms, rng, 0.0, 1.0);
            }
            Section::Macros => {
                let mc = &p.macros;
                rnd(s, &mc.dust, rng, 0.0, 0.7);
                rnd(s, &mc.chaos, rng, 0.0, 0.5);
                rnd(s, &mc.age, rng, 0.0, 0.7);
            }
        }
    }
}
