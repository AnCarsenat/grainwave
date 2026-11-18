//! FX sequencer grid: 6 lanes × 16 steps switching the stage fades per step.
//!
//! Lane words live in persisted atomics (`GrainwaveParams::seq_lane_atomics`): bits 0..15 =
//! step on, bit 16 = lane active. The GUI writes them with `fetch_xor` / `store`; the audio
//! thread reads them lock-free once per block.
//!
//! Mouse: click a cell to toggle it; press and drag paints the value of the first cell over
//! every cell the pointer crosses. Click the LED in front of a lane to activate it (inactive
//! lanes never gate their section). Right-click a lane name for quick patterns.

use super::theme::*;
use crate::params::{GrainwaveParams, SEQ_LANES, SEQ_LANE_ACTIVE, SEQ_STEPS};
use crate::shared::{load_f32, VisualState};
use nih_plug_egui::egui::{pos2, vec2, Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
pub struct SeqState {
    /// Value being painted while the mouse is held on the grid.
    paint: Option<bool>,
}

const STEP_MASK: u32 = (1 << SEQ_STEPS) - 1;

/// Set one step bit to `on` (no-op when already there).
pub fn set_step(lane: &AtomicU32, step: usize, on: bool) {
    let bit = 1u32 << step;
    let cur = lane.load(Ordering::Relaxed) & bit != 0;
    if cur != on {
        lane.fetch_xor(bit, Ordering::Relaxed);
    }
}

/// Replace the 16 step bits, keeping the lane-active bit.
pub fn set_steps(lane: &AtomicU32, steps: u32) {
    let active = lane.load(Ordering::Relaxed) & SEQ_LANE_ACTIVE;
    lane.store(active | (steps & STEP_MASK), Ordering::Relaxed);
}

/// Grid with lane labels on the left, filling `rect`.
pub fn sequencer_grid(ui: &mut Ui, rect: Rect, params: &GrainwaveParams, visual: &VisualState, state: &mut SeqState) {
    let lanes = params.seq_lane_atomics();
    let label_w = (rect.width() * 0.12).clamp(64.0, 96.0);
    let grid = Rect::from_min_max(pos2(rect.left() + label_w, rect.top()), rect.max);
    let row_gap = 2.0;
    let row_h = ((rect.height() - row_gap * (SEQ_LANES as f32 - 1.0)) / SEQ_LANES as f32).max(4.0);
    let gap = 2.0;
    let group_gap = 5.0;
    let cell_w = ((grid.width() - gap * 12.0 - group_gap * 3.0) / SEQ_STEPS as f32).max(4.0);
    let col_x = |step: usize| grid.left() + step as f32 * (cell_w + gap) + (step / 4) as f32 * (group_gap - gap);
    let row_y = |lane: usize| rect.top() + lane as f32 * (row_h + row_gap);
    let seq_on = params.sequencer.enabled.value();
    let cur_step = visual.seq_step.load(Ordering::Relaxed);
    let cur_step = (cur_step != u32::MAX).then_some(cur_step as usize % SEQ_STEPS);

    // Lane labels: LED toggles lane active; right-click menu with quick patterns.
    for (lane, atom) in lanes.iter().enumerate() {
        let word = atom.load(Ordering::Relaxed);
        let active = word & SEQ_LANE_ACTIVE != 0;
        let color = LANE_COLORS[lane];
        let r = Rect::from_min_size(pos2(rect.left(), row_y(lane)), vec2(label_w - 6.0, row_h));
        let resp = ui.interact(r, Id::new(("seq_lane_label", lane)), Sense::click());
        if resp.clicked() {
            atom.fetch_xor(SEQ_LANE_ACTIVE, Ordering::Relaxed);
        }
        let painter = ui.painter_at(r);
        if resp.hovered() {
            painter.rect_filled(r, 3.0, Color32::from_rgb(0x30, 0x2b, 0x27));
        }
        let led_r = (row_h * 0.3).clamp(2.5, 4.5);
        let led = pos2(r.left() + 7.0, r.center().y);
        if active {
            painter.circle_filled(led, led_r, color);
        } else {
            painter.circle_stroke(led, led_r, Stroke::new(1.0_f32, TEXT_DIM));
        }
        let font = (row_h * 0.75).clamp(8.0, 11.5);
        painter.text(
            pos2(r.left() + 15.0, r.center().y),
            Align2::LEFT_CENTER,
            LANE_NAMES[lane],
            FontId::proportional(font),
            if active { color } else { TEXT_DIM },
        );
        let resp = resp.on_hover_text(format!(
            "{} lane {}: click to {}. Right-click for patterns.",
            LANE_NAMES[lane],
            if active { "active" } else { "inactive (does not gate)" },
            if active { "deactivate" } else { "activate" }
        ));
        resp.context_menu(|ui| {
            let presets: [(&str, u32); 6] = [
                ("All steps on", 0xFFFF),
                ("Clear", 0x0000),
                ("Every 2nd", 0x5555),
                ("Every 4th", 0x1111),
                ("Off-beats", 0x4444),
                ("Last of 4", 0x8888),
            ];
            for (name, bits) in presets {
                if ui.button(name).clicked() {
                    set_steps(atom, bits);
                    atom.fetch_or(SEQ_LANE_ACTIVE, Ordering::Relaxed);
                    ui.close_menu();
                }
            }
            if ui.button("Invert").clicked() {
                atom.fetch_xor(STEP_MASK, Ordering::Relaxed);
                ui.close_menu();
            }
        });
    }

    // Cells: one interaction over the whole grid so press-and-drag paints across cells.
    let resp = ui.interact(grid, Id::new("seq_grid"), Sense::click_and_drag());
    let cell_at = |p: Pos2| -> Option<(usize, usize)> {
        if !grid.contains(p) {
            return None;
        }
        let lane = (0..SEQ_LANES).find(|&l| p.y >= row_y(l) && p.y < row_y(l) + row_h + row_gap)?;
        let step = (0..SEQ_STEPS).rev().find(|&s| p.x >= col_x(s))?;
        Some((lane, step))
    };
    // Primary button only: right-clicking a cell must not toggle it.
    if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
        if let Some((lane, step)) = resp.interact_pointer_pos().and_then(cell_at) {
            let atom = lanes[lane];
            let on = atom.load(Ordering::Relaxed) & (1 << step) != 0;
            let value = *state.paint.get_or_insert(!on);
            set_step(atom, step, value);
        }
    } else {
        state.paint = None;
    }
    let hover = resp.hover_pos().and_then(cell_at);

    let painter = ui.painter_at(rect);
    for (lane, atom) in lanes.iter().enumerate() {
        let word = atom.load(Ordering::Relaxed);
        let active = word & SEQ_LANE_ACTIVE != 0;
        let color = LANE_COLORS[lane];
        for step in 0..SEQ_STEPS {
            let cell = Rect::from_min_size(pos2(col_x(step), row_y(lane)), vec2(cell_w, row_h));
            let on = word & (1 << step) != 0;
            let (fill, edge) = cell_look(on, active, step, color);
            painter.rect_filled(cell, 2.0, fill);
            if let Some(edge) = edge {
                painter.rect_stroke(cell, 2.0, Stroke::new(1.0_f32, edge), StrokeKind::Inside);
            }
            if hover == Some((lane, step)) {
                painter.rect_stroke(cell, 2.0, Stroke::new(1.0_f32, TEXT), StrokeKind::Inside);
            }
        }
    }
    // Current step column.
    if let (true, Some(step)) = (seq_on, cur_step) {
        let x0 = col_x(step);
        let col = Rect::from_min_max(pos2(x0 - 1.0, rect.top() - 1.0), pos2(x0 + cell_w + 1.0, rect.bottom() + 1.0));
        painter.rect_stroke(col, 3.0, Stroke::new(1.5_f32, TEXT), StrokeKind::Outside);
    }
    resp.on_hover_text("Click: toggle step · drag: paint · beats of 4 are shaded");
}

/// Fill and optional outline of one cell. Active lanes light their on-steps in the lane
/// color; inactive lanes (which never gate) keep their pattern visible only as a faint tint
/// with a dim outline, so they read as switched off rather than as fully lit.
fn cell_look(on: bool, active: bool, step: usize, color: Color32) -> (Color32, Option<Color32>) {
    let off = if step & 3 == 0 { CELL_OFF_BEAT } else { CELL_OFF }; // first step of each beat
    match (on, active) {
        (true, true) => (color, None),
        (true, false) => (dim(color, 0.12), Some(dim(color, 0.45))),
        (false, _) => (off, None),
    }
}

/// Small live readout of the six stage gains (used by the sequencer header).
pub fn gain_dots(ui: &Ui, rect: Rect, visual: &VisualState) {
    let painter = ui.painter_at(rect);
    let n = SEQ_LANES as f32;
    let w = rect.width() / n;
    for (i, (gain, color)) in visual.section_gain.iter().zip(LANE_COLORS).enumerate() {
        let g = load_f32(gain);
        let g = if g.is_finite() { g.clamp(0.0, 1.0) } else { 0.0 };
        let c = pos2(rect.left() + (i as f32 + 0.5) * w, rect.center().y);
        let r = (rect.height() * 0.32).min(w * 0.35);
        painter.circle_filled(c, r, dim(color, 0.15 + 0.85 * g));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(c: Color32) -> f32 {
        0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
    }

    #[test]
    fn inactive_lanes_are_dimmed() {
        for color in LANE_COLORS {
            for step in [0, 1] {
                let (lit, _) = cell_look(true, true, step, color);
                let (inactive, edge) = cell_look(true, false, step, color);
                let (off, _) = cell_look(false, true, step, color);
                assert!(edge.is_some(), "inactive on-step keeps an outline so the pattern stays readable");
                // An inactive on-step must sit much closer to an off cell than to a lit one.
                let (l_lit, l_in, l_off) = (luma(lit), luma(inactive), luma(off));
                assert!(l_in - l_off.min(l_in) < 0.25 * (l_lit - l_off), "{color:?} step {step}: {l_in} vs off {l_off}, lit {l_lit}");
            }
        }
    }

    use crate::editor::widgets::tests::drag_with;
    use nih_plug_egui::egui::PointerButton;

    #[test]
    fn only_primary_button_toggles_cells() {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(380.0, 120.0));
        // A point inside step 2 of lane 0 (cells start after the lane labels).
        let p = pos2(10.0 + 64.0 + 2.5 * 18.0, 15.0);
        for (button, toggles) in [(PointerButton::Secondary, false), (PointerButton::Middle, false), (PointerButton::Primary, true)] {
            let params = GrainwaveParams::default();
            let visual = VisualState::default();
            let mut state = SeqState::default();
            let before = params.seq_lane_atomics()[0].load(Ordering::Relaxed);
            drag_with(button, p, p, |ui| sequencer_grid(ui, rect, &params, &visual, &mut state));
            let after = params.seq_lane_atomics()[0].load(Ordering::Relaxed);
            assert_eq!(before != after, toggles, "{button:?}: lane word {before:#x} -> {after:#x}");
            if toggles {
                assert_eq!((before ^ after).count_ones(), 1, "primary click toggles exactly one step");
            }
        }
    }
}
