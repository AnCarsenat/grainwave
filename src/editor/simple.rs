//! Simple page: time shaper editor, a row of effect units (one "pedal" per section) and the
//! FX sequencer strip.
//!
//! Height budget (content area = window − 2 × MARGIN − TOP_H − GAP):
//!
//! | window | content | time shaper | units | sequencer |
//! |---|---|---|---|---|
//! | 960 × 640 (default) | 572 | 300 (canvas 238) | 128 | 132 |
//! | 820 × 560 (minimum) | 492 | 220 (canvas 158) | 128 | 132 |
//!
//! If the time shaper would get less than `TIME_MIN` (only possible below the minimum size),
//! the time shaper and the sequencer share one area behind two tabs, above the units row.

use super::curve::{curve_panel, CurveState};
use super::sequencer::{gain_dots, sequencer_grid};
use super::theme::*;
use super::widgets::{combo, knob, meter, power, slider, toggle};
use super::EditorState;
use crate::params::GrainwaveParams;
use crate::shared::{load_f32, VisualState};
use nih_plug::prelude::*;
use nih_plug_egui::egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, Sense, Stroke, StrokeKind, Ui};

pub const UNITS_H: f32 = 128.0;
pub const SEQ_H: f32 = 132.0;
pub const TIME_MIN: f32 = 190.0;

pub fn simple_page(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, state: &mut EditorState) {
    let time_h = rect.height() - UNITS_H - SEQ_H - 2.0 * GAP;
    if time_h >= TIME_MIN {
        let time = Rect::from_min_size(rect.min, vec2(rect.width(), time_h));
        let units = Rect::from_min_size(pos2(rect.left(), time.bottom() + GAP), vec2(rect.width(), UNITS_H));
        let seq = Rect::from_min_max(pos2(rect.left(), units.bottom() + GAP), rect.max);
        time_panel(ui, time, p, visual, s, &mut state.curve);
        units_row(ui, units, p, visual, s);
        seq_panel(ui, seq, p, visual, s, state);
    } else {
        // Fallback: tabs for time shaper / sequencer above the always-visible units row.
        let tabs = Rect::from_min_size(rect.min, vec2(rect.width(), TAB_H));
        let units = Rect::from_min_max(pos2(rect.left(), rect.bottom() - UNITS_H), rect.max);
        let area = Rect::from_min_max(pos2(rect.left(), tabs.bottom() + 2.0), pos2(rect.right(), units.top() - GAP));
        let labels = ["Time shaper", "FX sequencer"];
        let w = 120.0;
        for (i, label) in labels.iter().enumerate() {
            let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (w + 4.0), rect.top()), vec2(w, TAB_H - 2.0));
            if tab_button(ui, r, label, state.simple_tab == i, if i == 0 { C_TIME } else { TEXT }) {
                state.simple_tab = i;
            }
        }
        if state.simple_tab == 0 {
            time_panel(ui, area, p, visual, s, &mut state.curve);
        } else {
            seq_panel(ui, area, p, visual, s, state);
        }
        units_row(ui, units, p, visual, s);
    }
}

/// Flat tab button; returns true when clicked.
pub fn tab_button(ui: &mut Ui, r: Rect, label: &str, selected: bool, color: Color32) -> bool {
    let resp = ui.interact(r, Id::new(("tab", label)), Sense::click());
    let painter = ui.painter_at(r);
    let fill = if selected { PANEL } else if resp.hovered() { Color32::from_rgb(0x2a, 0x25, 0x21) } else { BG };
    painter.rect_filled(r, 4.0, fill);
    if selected {
        painter.rect_filled(Rect::from_min_max(pos2(r.left() + 4.0, r.bottom() - 2.0), pos2(r.right() - 4.0, r.bottom())), 1.0, color);
    }
    painter.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.5), if selected { color } else { TEXT_DIM });
    resp.clicked()
}

fn panel_frame(ui: &Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 5.0, PANEL);
    painter.rect_stroke(rect, 5.0, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
}

/// Time shaper: curve editor on the left, Sync/Trigger/Quantize and Depth/Smooth/Mix on the right.
fn time_panel(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, curve: &mut CurveState) {
    panel_frame(ui, rect);
    let inner = rect.shrink(PAD);
    let ctrl_w = (inner.width() * 0.24).clamp(180.0, 240.0);
    let editor = Rect::from_min_max(inner.min, pos2(inner.right() - ctrl_w - GAP, inner.bottom()));
    let ctrl = Rect::from_min_max(pos2(inner.right() - ctrl_w, inner.top()), inner.max);

    let title_w = 92.0;
    ui.painter_at(rect).text(
        pos2(inner.left(), inner.top() + 11.0),
        Align2::LEFT_CENTER,
        "TIME SHAPER",
        FontId::proportional(12.5),
        C_TIME,
    );
    curve_panel(ui, editor, title_w, p, visual, s, curve);
    time_controls(ui, ctrl, p, s);
}

fn time_controls(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, s: &ParamSetter) {
    let t = &p.time;
    let row = 22.0;
    let gap = 4.0;
    let r = |i: usize| Rect::from_min_size(pos2(rect.left(), rect.top() + i as f32 * (row + gap)), vec2(rect.width(), row));
    combo(ui, r(0), &t.sync, s, "Sync");
    combo(ui, r(1), &t.trigger, s, "Trigger");
    combo(ui, r(2), &t.pattern_quantize, s, "Switch");
    let knobs_top = rect.top() + 3.0 * (row + gap) + 2.0;
    let kr = Rect::from_min_max(pos2(rect.left(), knobs_top), rect.max);
    let w = kr.width() / 3.0;
    let cell = |i: usize| Rect::from_min_size(pos2(kr.left() + i as f32 * w, kr.top()), vec2(w, kr.height().min(92.0)));
    knob(ui, cell(0), &t.depth, s, "Depth", false, C_TIME);
    knob(ui, cell(1), &t.smooth, s, "Smooth", false, C_TIME);
    knob(ui, cell(2), &t.mix, s, "Mix", false, C_TIME);
}

/// One effect unit: frame, power switch header, optional vertical dry/wet fader, body.
#[allow(clippy::too_many_arguments)]
fn unit(
    ui: &mut Ui,
    rect: Rect,
    name: &str,
    color: Color32,
    on: &BoolParam,
    level: f32,
    mix: Option<(&FloatParam, &str)>,
    s: &ParamSetter,
    body: impl FnOnce(&mut Ui, Rect),
) {
    let is_on = on.value();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, PANEL);
    let edge = if is_on { dim(color, 0.25 + 0.45 * level) } else { PANEL_EDGE };
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, edge), StrokeKind::Inside);
    // Colored strip on top: the pedal's face.
    painter.rect_filled(
        Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(rect.width() - 2.0, 3.0)),
        2.0,
        if is_on { dim(color, 0.3 + 0.7 * level) } else { KNOB_TRACK },
    );
    let inner = rect.shrink(PAD);
    let header = Rect::from_min_size(inner.min + vec2(0.0, 1.0), vec2(inner.width(), HEADER_H));
    power(ui, header, on, s, name, color);

    let body_rect = Rect::from_min_max(pos2(inner.left(), header.bottom() + 4.0), inner.max);
    let mut c = super::widgets::child(ui, body_rect, nih_plug_egui::egui::Layout::top_down(nih_plug_egui::egui::Align::Min));
    c.multiply_opacity(if is_on { 0.45 + 0.55 * level } else { 0.4 });
    let rest = if let Some((param, label)) = mix {
        let col_w = 18.0;
        let fader = Rect::from_min_max(pos2(body_rect.left() + 2.0, body_rect.top() + 2.0), pos2(body_rect.left() + col_w - 2.0, body_rect.bottom() - 13.0));
        slider(&mut c, fader, param, s, color, true);
        c.painter_at(body_rect).text(
            pos2(body_rect.left() + col_w * 0.5, body_rect.bottom()),
            Align2::CENTER_BOTTOM,
            label,
            FontId::proportional(9.0),
            TEXT_DIM,
        );
        Rect::from_min_max(pos2(body_rect.left() + col_w + 2.0, body_rect.top()), body_rect.max)
    } else {
        body_rect
    };
    body(&mut c, rest);
}

fn units_row(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let n = 7.0;
    let w = (rect.width() - GAP * (n - 1.0)) / n;
    let r = |i: usize| Rect::from_min_size(pos2(rect.left() + i as f32 * (w + GAP), rect.top()), vec2(w, rect.height()));
    let gain = |i: usize| {
        let g = load_f32(&visual.section_gain[i]);
        if g.is_finite() {
            g.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let sec = &p.sections;

    unit(ui, r(0), "TIME", C_TIME, &sec.time, gain(0), Some((&p.time.mix, "MIX")), s, |ui, b| {
        knob(ui, b, &p.time.depth, s, "Depth", false, C_TIME);
    });
    unit(ui, r(1), "GRAINS", C_GRAINS, &sec.grains, gain(1), Some((&sec.grain_amount, "AMT")), s, |ui, b| {
        knob(ui, b, &p.grain.size_ms, s, "Size", false, C_GRAINS);
    });
    unit(ui, r(2), "RHYTHM", C_RHYTHM, &sec.rhythm, gain(2), Some((&sec.rhythm_mix, "MIX")), s, |ui, b| {
        knob(ui, b, &p.rhythm.stutter_chance, s, "Stutter", false, C_RHYTHM);
    });
    unit(ui, r(3), "DEGRADE", C_DEGRADE, &sec.degrade, gain(3), Some((&sec.degrade_mix, "MIX")), s, |ui, b| {
        knob(ui, b, &p.macros.dust, s, "Dust", false, C_DEGRADE);
    });
    unit(ui, r(4), "FILTER", C_FILTER, &sec.filter, gain(4), Some((&sec.filter_mix, "MIX")), s, |ui, b| {
        knob(ui, b, &p.filter.lowpass_hz, s, "Low-pass", false, C_FILTER);
    });
    unit(ui, r(5), "SPACE", C_SPACE, &sec.space, gain(5), Some((&sec.space_mix, "MIX")), s, |ui, b| {
        knob(ui, b, &p.space.reverb_mix, s, "Reverb", false, C_SPACE);
    });
    let mod_level = if sec.modulation.value() { 1.0 } else { 0.0 };
    unit(ui, r(6), "MOD", C_MOD, &sec.modulation, mod_level, None, s, |ui, b| {
        // Two live LFO meters where the fader would be, then the Chaos knob.
        let painter = ui.painter_at(b);
        for i in 0..2 {
            let m = Rect::from_min_max(
                pos2(b.left() + 2.0 + i as f32 * 7.0, b.top() + 2.0),
                pos2(b.left() + 7.0 + i as f32 * 7.0, b.bottom() - 13.0),
            );
            meter(&painter, m, load_f32(&visual.lfo_values[i]), true, C_MOD);
        }
        painter.text(pos2(b.left() + 8.0, b.bottom()), Align2::CENTER_BOTTOM, "LFO", FontId::proportional(9.0), TEXT_DIM);
        let k = Rect::from_min_max(pos2(b.left() + 20.0, b.top()), b.max);
        knob(ui, k, &p.macros.chaos, s, "Chaos", false, C_MOD);
    });
}

fn seq_panel(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, state: &mut EditorState) {
    panel_frame(ui, rect);
    let inner = rect.shrink(PAD);
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_H));
    seq_header(ui, header, p, visual, s);
    let grid = Rect::from_min_max(pos2(inner.left(), header.bottom() + 4.0), inner.max);
    sequencer_grid(ui, grid, p, visual, &mut state.seq);
}

/// Title, run switch, step rate and live stage gains. Shared with the Advanced sequencer tab.
pub fn seq_header(ui: &mut Ui, header: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let painter = ui.painter_at(header);
    painter.text(header.left_center(), Align2::LEFT_CENTER, "FX SEQUENCER", FontId::proportional(12.5), TEXT);
    let x0 = header.left() + 110.0;
    let run = Rect::from_min_size(pos2(x0, header.top()), vec2(56.0, header.height()));
    let label = if p.sequencer.enabled.value() { "RUN" } else { "OFF" };
    toggle(ui, run, &p.sequencer.enabled, s, label, ACCENT);
    let rate = Rect::from_min_size(pos2(run.right() + 8.0, header.top()), vec2(130.0, header.height()));
    combo(ui, rate, &p.sequencer.rate, s, "Step");
    let dots_w = 84.0;
    let dots = Rect::from_min_size(pos2(header.right() - dots_w, header.top()), vec2(dots_w, header.height()));
    gain_dots(ui, dots, visual);
    let hint_x = rate.right() + 12.0;
    if dots.left() - hint_x > 160.0 {
        painter.text(
            pos2(hint_x, header.center().y),
            Align2::LEFT_CENTER,
            "lit step = section plays · LED = lane active",
            FontId::proportional(10.0),
            TEXT_DIM,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_h(window_h: f32) -> f32 {
        window_h - 2.0 * MARGIN - TOP_H - GAP
    }

    #[test]
    fn all_three_regions_fit_at_min_and_default() {
        for h in [MIN_H, 640.0] {
            let time_h = content_h(h) - UNITS_H - SEQ_H - 2.0 * GAP;
            assert!(time_h >= TIME_MIN, "window height {h}: time shaper gets {time_h}");
        }
        assert_eq!(content_h(640.0), 572.0);
        assert_eq!(content_h(MIN_H), 492.0);
    }
}
