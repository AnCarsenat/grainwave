//! OWNER: feat/ui. egui editor with two pages:
//!
//! - **Simple** (default): time shaper curve editor, a row of seven effect units (one pedal
//!   per section, each with power, dry/wet and a key knob) and the FX sequencer strip.
//! - **Advanced**: one tab per section with every parameter.
//!
//! Layout is computed from the window size: every region gets a rect from fixed budgets
//! (see `theme.rs` constants and the tables in `simple.rs` / `advanced.rs`), and widgets draw
//! inside the rect they are given, so nothing overlaps or gets clipped from 820 × 560 up.
//!
//! Top bar (`TOP_H` = 46): name + status | preset browser | Randomize + locks | Simple/Advanced
//! | Mix, Output, Limiter (right-aligned). The preset dropdown takes the leftover width.
//!
//! Everything here runs on the GUI thread. The editor reads `VisualState` (atomics written by
//! the audio thread) and writes parameters only through `ParamSetter` begin/set/end.

mod advanced;
mod curve;
mod randomize;
mod sequencer;
mod simple;
mod theme;
mod widgets;
#[cfg(test)]
mod screenshot;

use crate::dsp::rng::Rng;
use crate::params::GrainwaveParams;
use crate::presets::{self, PresetEntry, PresetSource};
use crate::shared::VisualState;
use advanced::Tab;
use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, pos2, vec2, Align, Align2, ComboBox, FontId, Layout, Rect, Response, Stroke, Ui};
use nih_plug_egui::{create_egui_editor, resizable_window::ResizableWindow};
use randomize::{ALL_GROUPS, GROUP_COUNT};
use std::sync::Arc;
use std::time::Duration;
use theme::*;
use widgets::{child, knob_compact, toggle};

/// Repaint interval so the waveform, cursor and meters animate (~30 fps).
const REPAINT: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Simple,
    Advanced,
}

/// GUI-only state, never saved into parameters. Lives as long as the plugin instance, so it
/// survives closing and reopening the editor.
pub struct EditorState {
    page: Page,
    tab: Tab,
    /// Fallback tab on the Simple page when the window is too short (0 = time, 1 = sequencer).
    simple_tab: usize,
    /// Randomize locks, indexed by `Group as usize`.
    locks: [bool; GROUP_COUNT],
    presets: Vec<PresetEntry>,
    current: Option<usize>,
    save_name: String,
    status: String,
    rng: Rng,
    curve: curve::CurveState,
    seq: sequencer::SeqState,
    /// Momentary buttons: whether this editor holds them down, and whether they were drawn
    /// this frame (when not drawn while held, they are released).
    rewind_held: bool,
    tape_stop_held: bool,
    rewind_drawn: bool,
    tape_stop_drawn: bool,
}

impl Default for EditorState {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
            .unwrap_or(1);
        Self {
            page: Page::Simple,
            tab: Tab::Time,
            simple_tab: 0,
            locks: [false; GROUP_COUNT],
            presets: Vec::new(),
            current: None,
            save_name: String::new(),
            status: String::new(),
            rng: Rng::new(seed),
            curve: curve::CurveState::default(),
            seq: sequencer::SeqState::default(),
            rewind_held: false,
            tape_stop_held: false,
            rewind_drawn: false,
            tape_stop_drawn: false,
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
                .min_size(vec2(MIN_W, MIN_H))
                .show(ctx, egui_state.as_ref(), |ui| {
                    draw(ui, &params, &visual, setter, state);
                });
        },
    )
}

fn draw(ui: &mut Ui, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, state: &mut EditorState) {
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, BG);
    let area = full.shrink(MARGIN);

    let top = Rect::from_min_size(area.min, vec2(area.width(), TOP_H));
    top_bar(ui, top, p, s, state);
    ui.painter().line_segment(
        [pos2(area.left(), top.bottom() + GAP * 0.5), pos2(area.right(), top.bottom() + GAP * 0.5)],
        Stroke::new(1.0_f32, PANEL_EDGE),
    );
    let content = Rect::from_min_max(pos2(area.left(), top.bottom() + GAP), area.max);

    state.rewind_drawn = false;
    state.tape_stop_drawn = false;
    match state.page {
        Page::Simple => simple::simple_page(ui, content, p, visual, s, state),
        Page::Advanced => advanced::advanced_page(ui, content, p, visual, s, state),
    }
    // A held momentary button that is no longer on screen (tab or page switch, editor
    // reopened) is released so the effect never sticks.
    if !state.rewind_drawn {
        widgets::release_momentary(&p.rhythm.rewind, s, &mut state.rewind_held);
    }
    if !state.tape_stop_drawn {
        widgets::release_momentary(&p.rhythm.tape_stop, s, &mut state.tape_stop_held);
    }
}

fn button_at(ui: &mut Ui, rect: Rect, text: egui::RichText, selected: bool) -> Response {
    let mut c = child(ui, rect, Layout::centered_and_justified(egui::Direction::LeftToRight));
    c.add_sized(rect.size(), egui::Button::new(text).selected(selected))
}

// ---------------------------------------------------------------------------
// Top bar
// ---------------------------------------------------------------------------

fn top_bar(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, s: &ParamSetter, state: &mut EditorState) {
    let h = rect.height();
    let cy = rect.center().y;
    let row = |x: f32, w: f32| Rect::from_min_size(pos2(x, cy - 11.0), vec2(w, 22.0));
    let sep = |ui: &Ui, x: f32| {
        ui.painter().line_segment([pos2(x, rect.top() + 6.0), pos2(x, rect.bottom() - 6.0)], Stroke::new(1.0_f32, PANEL_EDGE));
    };

    // Right group: Limiter, Output, Mix.
    let lim_w = 58.0;
    let out_w = 76.0;
    let mix_w = 66.0;
    let lim = Rect::from_min_size(pos2(rect.right() - lim_w, cy - 10.0), vec2(lim_w, 20.0));
    let out = Rect::from_min_size(pos2(lim.left() - 6.0 - out_w, rect.top()), vec2(out_w, h));
    let mix = Rect::from_min_size(pos2(out.left() - 4.0 - mix_w, rect.top()), vec2(mix_w, h));
    toggle(ui, lim, &p.output.limiter, s, "Limiter", ACCENT);
    knob_compact(ui, out, &p.output.output_gain, s, "Output", ACCENT);
    knob_compact(ui, mix, &p.output.mix, s, "Mix", ACCENT);
    let right_edge = mix.left() - 8.0;
    sep(ui, right_edge);

    // Title and status.
    // Sized from the measured text so the title is never clipped.
    let title_font = FontId::proportional(18.0);
    let title_text_w = ui.fonts(|f| f.layout_no_wrap("GRAINWAVE".into(), title_font.clone(), ACCENT).size().x);
    let title_w = title_text_w.ceil() + 2.0;
    let title = Rect::from_min_size(rect.min, vec2(title_w, h));
    let painter = ui.painter_at(title);
    painter.text(pos2(title.left(), cy + 2.0), Align2::LEFT_BOTTOM, "GRAINWAVE", title_font, ACCENT);
    painter.text(pos2(title.left() + 1.0, cy + 4.0), Align2::LEFT_TOP, &state.status, FontId::proportional(9.5), TEXT_DIM);
    if !state.status.is_empty() {
        ui.interact(title, egui::Id::new("status_tip"), egui::Sense::hover()).on_hover_text(&state.status);
    }
    let mut x = title.right() + 6.0;
    sep(ui, x);
    x += 6.0;

    // Mode and randomize widths are fixed; the preset dropdown takes what is left.
    let mode_w = 116.0;
    // Everything after the dropdown, in the order drawn below: ">" + "Save…" + gaps,
    // Randomize + locks + gaps, Simple/Advanced.
    let preset_fixed = 24.0 + 4.0 + 24.0 + 54.0 + 4.0 + 8.0;
    let rnd_fixed = 80.0 + 50.0 + 8.0;
    let after = preset_fixed + rnd_fixed + mode_w + 4.0;
    let combo_w = (right_edge - x - after).clamp(80.0, 230.0);

    // Preset browser.
    let n = state.presets.len();
    let mut load: Option<usize> = None;
    if button_at(ui, row(x, 20.0), egui::RichText::new("<"), false).on_hover_text("Previous preset").clicked() && n > 0 {
        load = Some(state.current.map_or(n - 1, |i| (i + n - 1) % n));
    }
    x += 24.0;
    let selected = state
        .current
        .and_then(|i| state.presets.get(i))
        .map(|e| e.preset.name.clone())
        .unwrap_or_else(|| "(unsaved)".to_string());
    {
        let mut c = child(ui, row(x, combo_w), Layout::left_to_right(Align::Center));
        ComboBox::from_id_salt("preset_combo")
            .width(combo_w - 2.0)
            .height(440.0)
            .selected_text(selected)
            .show_ui(&mut c, |ui| {
                let mut shown_user_header = false;
                ui.label(egui::RichText::new("Factory").size(10.0).color(TEXT_DIM));
                for (i, e) in state.presets.iter().enumerate() {
                    if !e.is_factory() && !shown_user_header {
                        ui.separator();
                        ui.label(egui::RichText::new("User").size(10.0).color(TEXT_DIM));
                        shown_user_header = true;
                    }
                    let resp = ui.selectable_label(state.current == Some(i), &e.preset.name);
                    if resp.clicked() {
                        load = Some(i);
                    }
                }
            });
    }
    x += combo_w + 4.0;
    if button_at(ui, row(x, 20.0), egui::RichText::new(">"), false).on_hover_text("Next preset").clicked() && n > 0 {
        load = Some(state.current.map_or(0, |i| (i + 1) % n));
    }
    x += 24.0;
    {
        let mut c = child(ui, row(x, 50.0), Layout::left_to_right(Align::Center));
        c.menu_button("Save…", |ui| {
            ui.set_min_width(200.0);
            ui.add(egui::TextEdit::singleline(&mut state.save_name).hint_text("preset name").desired_width(190.0));
            ui.horizontal(|ui| {
                if ui.button("Save").on_hover_text("Save as a user preset").clicked() {
                    save_preset(p, state);
                    ui.close_menu();
                }
                if ui.button("Rescan").on_hover_text("Reload user presets from disk").clicked() {
                    state.reload_presets();
                    state.status = format!("{} presets", state.presets.len());
                }
            });
            ui.label(egui::RichText::new("Saves parameters, time patterns and sequencer lanes.").size(10.0).color(TEXT_DIM));
        });
    }
    x += 54.0;
    if let Some(i) = load {
        if let Some(e) = state.presets.get(i) {
            presets::apply(p, s, &e.preset);
            state.save_name = e.preset.name.clone();
            state.status = match &e.source {
                PresetSource::Factory => "factory preset".into(),
                PresetSource::User(_) => "user preset".into(),
            };
            state.current = Some(i);
        }
    }
    x += 4.0;
    sep(ui, x);
    x += 8.0;

    // Randomize + locks.
    if button_at(ui, row(x, 76.0), egui::RichText::new("Randomize").color(ACCENT), false)
        .on_hover_text("Randomize unlocked groups. Never touches power switches, amounts, the sequencer, mix or output.")
        .clicked()
    {
        randomize::randomize(p, s, &state.locks, &mut state.rng);
        state.current = None;
        state.status = "randomized".into();
    }
    x += 80.0;
    {
        let mut c = child(ui, row(x, 46.0), Layout::left_to_right(Align::Center));
        let n_locked = state.locks.iter().filter(|l| **l).count();
        let label = if n_locked > 0 { format!("locks {n_locked}") } else { "locks".to_string() };
        c.menu_button(label, |ui| {
            ui.label(egui::RichText::new("Skip when randomizing").size(10.0).color(TEXT_DIM));
            for g in ALL_GROUPS {
                ui.checkbox(&mut state.locks[g as usize], g.label());
            }
            if ui.button("Unlock all").clicked() {
                state.locks = [false; GROUP_COUNT];
            }
        });
    }
    x += 50.0;
    sep(ui, x);
    x += 8.0;

    // Simple / Advanced.
    let half = mode_w / 2.0;
    if button_at(ui, row(x, half - 2.0), egui::RichText::new("Simple").size(11.5), state.page == Page::Simple).clicked() {
        state.page = Page::Simple;
    }
    if button_at(ui, row(x + half, half + 4.0), egui::RichText::new("Advanced").size(11.5), state.page == Page::Advanced).clicked() {
        state.page = Page::Advanced;
    }
}

fn save_preset(p: &GrainwaveParams, state: &mut EditorState) {
    let name = state.save_name.trim().to_string();
    if name.is_empty() {
        state.status = "enter a preset name first".into();
        return;
    }
    let preset = presets::capture(p, &name);
    match presets::save_user(&preset) {
        Ok(path) => {
            state.presets = presets::all_presets();
            state.current = state.presets.iter().position(|e| e.source == PresetSource::User(path.clone()));
            state.status = format!("saved {}", path.display());
        }
        Err(e) => state.status = format!("save failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    //! Headless layout checks: render every page and tab with a dummy host context and verify
    //! that interactive widgets stay on screen and never overlap.

    use super::*;
    use nih_plug_egui::egui::{Context, Pos2, RawInput};

    struct NullContext;

    impl GuiContext for NullContext {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Standalone
        }
        fn request_resize(&self) -> bool {
            false
        }
        unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
        unsafe fn raw_set_parameter_normalized(&self, _param: ParamPtr, _normalized: f32) {}
        unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
        fn get_state(&self) -> PluginState {
            unimplemented!("not used by the editor")
        }
        fn set_state(&self, _state: PluginState) {}
    }

    /// Renders two passes and returns the interactive widget rects of the last one.
    fn render(w: f32, h: f32, state: &mut EditorState) -> Vec<(egui::Id, Rect)> {
        let ctx = Context::default();
        apply_theme(&ctx);
        let params = GrainwaveParams::default();
        let visual = VisualState::default();
        let gui = NullContext;
        let setter = ParamSetter::new(&gui);
        for _ in 0..2 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                    draw(ui, &params, &visual, &setter, state);
                });
            });
        }
        ctx.viewport(|v| {
            v.prev_pass
                .widgets
                .layers()
                .flat_map(|(_, ws)| ws.iter())
                .filter(|w| w.sense.interactive())
                .map(|w| (w.id, w.rect))
                .collect()
        })
    }

    fn check(w: f32, h: f32, state: &mut EditorState, what: &str) {
        let widgets = render(w, h, state);
        assert!(widgets.len() > 10, "{what}: only {} widgets", widgets.len());
        for (id, r) in &widgets {
            assert!(r.width() >= 12.0 && r.height() >= 12.0, "{what} at {w}x{h}: widget {id:?} too small: {r:?}");
        }
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(w, h)).expand(0.5);
        for (id, r) in &widgets {
            assert!(screen.contains_rect(*r), "{what} at {w}x{h}: widget {id:?} {r:?} off screen");
        }
        for (i, (ia, a)) in widgets.iter().enumerate() {
            for (ib, b) in &widgets[i + 1..] {
                let o = a.intersect(*b);
                let overlap = o.width() > 1.0 && o.height() > 1.0;
                assert!(!overlap, "{what} at {w}x{h}: {ia:?} {a:?} overlaps {ib:?} {b:?}");
            }
        }
    }

    #[test]
    fn every_page_fits_at_min_and_default_size() {
        for (w, h) in [(MIN_W, MIN_H), (960.0, 640.0), (1400.0, 900.0)] {
            let mut state = EditorState::default();
            check(w, h, &mut state, "simple");
            state.page = Page::Advanced;
            for tab in Tab::ALL {
                state.tab = tab;
                check(w, h, &mut state, &format!("{tab:?}"));
            }
        }
    }

    #[test]
    fn tiny_window_does_not_panic() {
        let mut state = EditorState::default();
        render(400.0, 300.0, &mut state);
        state.page = Page::Advanced;
        for tab in Tab::ALL {
            state.tab = tab;
            render(400.0, 300.0, &mut state);
        }
    }
}
